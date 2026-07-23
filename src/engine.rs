use crate::checkpoint::{CheckpointLog, CheckpointRecord};
use crate::commit::BinderCommit;
use crate::edge::BinderEdgeVersion;
use crate::error::StoreError;
use crate::index::graph::GraphIndex;
use crate::index::head_edge::HeadEdgeIndex;
use crate::index::head_object::HeadObjectIndex;
use crate::namespace::NamespaceRegistry;
use crate::object::BinderObjectVersion;
use crate::query;
use crate::transaction::Transaction;
use crate::types::{CommitId, EdgeId, ObjectId};
use crate::vector::{
    deserialize_projection, serialize_projection, ProjectionCheckpoint, VectorEntry, VectorFilter,
    VectorIndex, VectorSearchResult,
};
use crate::wal::Wal;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct BinderStore {
    store_path: PathBuf,
    dirty_marker: PathBuf,
    wal: Mutex<Wal>,
    checkpoint_log: Mutex<CheckpointLog>,
    commits: Mutex<Vec<BinderCommit>>,
    head: Mutex<Option<CommitId>>,
    write_lock: Mutex<()>,
    closed: Mutex<bool>,
    object_versions: Mutex<Vec<BinderObjectVersion>>,
    edge_versions: Mutex<Vec<BinderEdgeVersion>>,
    head_objects: Mutex<HeadObjectIndex>,
    head_edges: Mutex<HeadEdgeIndex>,
    graph: Mutex<GraphIndex>,
    neighbours: Mutex<HashMap<ObjectId, Vec<ObjectId>>>,
    namespace_registry: Mutex<NamespaceRegistry>,
    vector_index: Mutex<VectorIndex>,
}

impl BinderStore {
    pub fn open(store_path: &Path) -> Result<Self, StoreError> {
        fs::create_dir_all(store_path)?;

        let dirty_marker = store_path.join(".dirty");
        fs::write(&dirty_marker, b"")?;

        let wal_path = store_path.join("commits.wal");
        let checkpoint_path = store_path.join("checkpoint.log");

        let mut checkpoint_log = CheckpointLog::open_or_create(&checkpoint_path)?;
        let _last_checkpoint = checkpoint_log.last()?;

        // In V1 we always replay from the beginning. Checkpoint is written on
        // close and read on open to bound replay cost in future phases.
        let (wal, commits) = Wal::open_or_create(&wal_path)?;

        let head = commits.last().map(|c| c.commit_id);
        let (object_versions, edge_versions) = build_version_stores(&commits);
        let head_objects = HeadObjectIndex::rebuild(&object_versions);
        let head_edges = HeadEdgeIndex::rebuild(&edge_versions);
        let graph = GraphIndex::rebuild(edge_versions.iter().filter(|v| !v.is_tombstone()));
        let neighbours =
            GraphIndex::build_neighbour_map(edge_versions.iter().filter(|v| !v.is_tombstone()));

        // Load vector projection from previous clean close, if present.
        let vector_index = load_vector_projection(store_path);

        Ok(Self {
            store_path: store_path.to_path_buf(),
            dirty_marker,
            wal: Mutex::new(wal),
            checkpoint_log: Mutex::new(checkpoint_log),
            commits: Mutex::new(commits),
            head: Mutex::new(head),
            write_lock: Mutex::new(()),
            closed: Mutex::new(false),
            object_versions: Mutex::new(object_versions),
            edge_versions: Mutex::new(edge_versions),
            head_objects: Mutex::new(head_objects),
            head_edges: Mutex::new(head_edges),
            graph: Mutex::new(graph),
            neighbours: Mutex::new(neighbours),
            namespace_registry: Mutex::new(NamespaceRegistry::new()),
            vector_index: Mutex::new(vector_index),
        })
    }

    // ── namespace registry ────────────────────────────────────────────────────

    /// Register a namespace as owned by actor. Must be called before commits
    /// that write to that namespace. Safe to call multiple times (idempotent
    /// for the same owner). Called at workspace open from AppRegistry.
    pub fn register_namespace(
        &self,
        namespace: impl Into<String>,
        actor: impl Into<String>,
    ) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.namespace_registry
            .lock()
            .unwrap()
            .register(namespace, actor);
        Ok(())
    }

    // ── write API ─────────────────────────────────────────────────────────────

    /// Commit a transaction. `namespace` is the namespace claim for all object
    /// writes in this transaction. Edge writes are exempt from namespace checks.
    /// Returns the new CommitId on success.
    pub fn commit(
        &self,
        tx: Transaction,
        actor: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<CommitId, StoreError> {
        self.commit_with_namespace(tx, actor, message, "")
    }

    /// Like `commit` but with an explicit namespace claim that is enforced
    /// against the registry before the WAL write.
    pub fn commit_with_namespace(
        &self,
        tx: Transaction,
        actor: impl Into<String>,
        message: impl Into<String>,
        namespace: &str,
    ) -> Result<CommitId, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }

        if tx.is_empty() {
            return Err(StoreError::EmptyTransaction);
        }

        let actor: String = actor.into();
        let message: String = message.into();

        // Namespace check before acquiring write lock — reject early.
        if !namespace.is_empty() {
            self.namespace_registry
                .lock()
                .unwrap()
                .check(namespace, &actor)?;
        }

        let _write_guard: MutexGuard<()> = self.write_lock.lock().unwrap();

        let parent_commit = *self.head.lock().unwrap();
        let commit_id = CommitId::new();
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let commit = BinderCommit {
            commit_id,
            parent_commit,
            timestamp_ms,
            actor,
            message,
            object_changes: tx.object_changes,
            relationship_changes: tx.relationship_changes,
        };

        // WAL-first: fsync before updating in-memory state (I-3).
        self.wal.lock().unwrap().append(&commit)?;

        // Derive version records.
        let new_ovs = derive_object_versions_for_commit(&commit);
        let new_evs = derive_edge_versions_for_commit(&commit);

        // Update version stores and HEAD indexes atomically (under write_lock).
        {
            let mut ovs = self.object_versions.lock().unwrap();
            let mut hobj = self.head_objects.lock().unwrap();
            retire_superseded_object_versions(&mut ovs, &new_ovs, commit_id);
            for v in &new_ovs {
                hobj.upsert(v.clone());
            }
            ovs.extend(new_ovs);
        }
        {
            let mut evs = self.edge_versions.lock().unwrap();
            let mut hedge = self.head_edges.lock().unwrap();
            let mut graph = self.graph.lock().unwrap();
            let mut nbrs = self.neighbours.lock().unwrap();

            // Remove old edge HEAD entries from graph index before retiring.
            for new_ev in &new_evs {
                if let Some(old) = hedge.get(&new_ev.edge_id) {
                    graph.remove(old);
                    // Remove from neighbour map.
                    if let (Some(from), Some(to)) = (&old.from_object, &old.to_object) {
                        if let Some(tos) = nbrs.get_mut(from) {
                            tos.retain(|t| t != to);
                        }
                    }
                }
            }

            retire_superseded_edge_versions(&mut evs, &new_evs, commit_id);

            for v in &new_evs {
                hedge.upsert(v.clone());
                if !v.is_tombstone() {
                    graph.insert(v);
                    if let (Some(from), Some(to)) = (&v.from_object, &v.to_object) {
                        nbrs.entry(from.clone()).or_default().push(to.clone());
                    }
                }
            }
            evs.extend(new_evs);
        }

        *self.head.lock().unwrap() = Some(commit_id);
        self.commits.lock().unwrap().push(commit);

        Ok(commit_id)
    }

    // ── query API ─────────────────────────────────────────────────────────────

    /// Current version of an object (O(1) HEAD index lookup).
    /// Returns None if the object is unknown or has been tombstoned.
    pub fn get(&self, object_id: &ObjectId) -> Result<Option<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let v = self
            .head_objects
            .lock()
            .unwrap()
            .get(object_id)
            .filter(|v| !v.is_tombstone())
            .cloned();
        Ok(v)
    }

    /// All live (non-tombstone) current object versions whose object_id starts
    /// with `namespace_prefix`.
    pub fn list(&self, namespace_prefix: &str) -> Result<Vec<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let ovs = self.object_versions.lock().unwrap();
        Ok(query::list_live_by_namespace(&ovs, namespace_prefix)
            .into_iter()
            .cloned()
            .collect())
    }

    /// Full version history of an object in ascending commit order.
    pub fn object_history(
        &self,
        object_id: &ObjectId,
    ) -> Result<Vec<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .object_versions
            .lock()
            .unwrap()
            .iter()
            .filter(|v| &v.object_id == object_id)
            .cloned()
            .collect())
    }

    /// Version of an object at a specific past commit (historical query).
    /// Returns None if the object didn't exist at that commit or was tombstoned.
    pub fn object_as_of(
        &self,
        object_id: &ObjectId,
        at_commit: CommitId,
    ) -> Result<Option<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let ovs = self.object_versions.lock().unwrap();
        let result = query::object_as_of(&ovs, object_id, at_commit)
            .filter(|v| !v.is_tombstone())
            .cloned();
        Ok(result)
    }

    /// Full version history of an edge.
    pub fn edge_history(&self, edge_id: &EdgeId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .edge_versions
            .lock()
            .unwrap()
            .iter()
            .filter(|v| &v.edge_id == edge_id)
            .cloned()
            .collect())
    }

    /// Version of an edge at a specific past commit.
    pub fn edge_as_of(
        &self,
        edge_id: &EdgeId,
        at_commit: CommitId,
    ) -> Result<Option<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let evs = self.edge_versions.lock().unwrap();
        Ok(query::edge_as_of(&evs, edge_id, at_commit).cloned())
    }

    /// Live edges originating from an object (graph index lookup).
    pub fn edges_from(&self, object_id: &ObjectId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let graph = self.graph.lock().unwrap();
        let hedge = self.head_edges.lock().unwrap();
        let edge_ids: Vec<EdgeId> = graph.edges_from(object_id).cloned().collect();
        Ok(edge_ids
            .into_iter()
            .filter_map(|eid| hedge.get(&eid).cloned())
            .collect())
    }

    /// Live edges pointing to an object.
    pub fn edges_to(&self, object_id: &ObjectId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let graph = self.graph.lock().unwrap();
        let hedge = self.head_edges.lock().unwrap();
        let edge_ids: Vec<EdgeId> = graph.edges_to(object_id).cloned().collect();
        Ok(edge_ids
            .into_iter()
            .filter_map(|eid| hedge.get(&eid).cloned())
            .collect())
    }

    /// Live edges from an object of a specific relationship type.
    pub fn edges_from_with_type(
        &self,
        object_id: &ObjectId,
        rel_type: &str,
    ) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let graph = self.graph.lock().unwrap();
        let hedge = self.head_edges.lock().unwrap();
        let edge_ids: Vec<EdgeId> = graph.edges_from_with_type(object_id, rel_type).collect();
        Ok(edge_ids
            .into_iter()
            .filter_map(|eid| hedge.get(&eid).cloned())
            .collect())
    }

    /// BFS traversal: all live objects reachable from `start` within `max_depth` hops.
    pub fn traverse(
        &self,
        start: &ObjectId,
        max_depth: usize,
    ) -> Result<Vec<ObjectId>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let neighbours = self.neighbours.lock().unwrap();
        let head_objects = self.head_objects.lock().unwrap();
        let result =
            GraphIndex::traverse_outgoing_with_neighbours(&neighbours, start, max_depth, |oid| {
                head_objects.get(oid).is_some_and(|v| !v.is_tombstone())
            });
        Ok(result)
    }

    // ── maintenance ───────────────────────────────────────────────────────────

    /// Explicitly rebuild all indexes from the commit log.
    /// Used after corruption detection or manual recovery.
    /// Safe to call at any time; does not alter the WAL or version stores.
    pub fn rebuild_indexes(&self) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let ovs = self.object_versions.lock().unwrap();
        let evs = self.edge_versions.lock().unwrap();

        let new_head_objects = HeadObjectIndex::rebuild(&ovs);
        let new_head_edges = HeadEdgeIndex::rebuild(&evs);
        let new_graph = GraphIndex::rebuild(evs.iter().filter(|v| !v.is_tombstone()));
        let new_neighbours =
            GraphIndex::build_neighbour_map(evs.iter().filter(|v| !v.is_tombstone()));

        drop(ovs);
        drop(evs);

        *self.head_objects.lock().unwrap() = new_head_objects;
        *self.head_edges.lock().unwrap() = new_head_edges;
        *self.graph.lock().unwrap() = new_graph;
        *self.neighbours.lock().unwrap() = new_neighbours;

        Ok(())
    }

    pub fn scan_commits(&self) -> Result<Vec<BinderCommit>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self.commits.lock().unwrap().clone())
    }

    pub fn head(&self) -> Result<Option<CommitId>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(*self.head.lock().unwrap())
    }

    // ── vector projection API ──────────────────────────────────────────────────

    pub fn insert_vector(&self, entry: VectorEntry) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.vector_index.lock().unwrap().insert(entry);
        Ok(())
    }

    pub fn tombstone_object_vectors(&self, object_id: &str) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.vector_index
            .lock()
            .unwrap()
            .tombstone_object(object_id);
        Ok(())
    }

    pub fn vector_search(
        &self,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .vector_index
            .lock()
            .unwrap()
            .knn_search(query, filter, limit))
    }

    pub fn vector_entries_for_object(
        &self,
        object_id: &str,
    ) -> Result<Vec<VectorEntry>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .vector_index
            .lock()
            .unwrap()
            .entries_for_object(object_id))
    }

    pub fn clear_vector_projection(&self) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.vector_index.lock().unwrap().clear();
        // Immediately remove persisted projection so rebuild starts clean on next open.
        let _ = fs::remove_file(self.store_path.join("vectors.proj"));
        Ok(())
    }

    pub fn all_vector_entries(&self) -> Result<Vec<VectorEntry>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .vector_index
            .lock()
            .unwrap()
            .all_entries()
            .cloned()
            .collect())
    }

    pub fn vector_checkpoint(&self) -> Result<ProjectionCheckpoint, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self.vector_index.lock().unwrap().checkpoint().clone())
    }

    pub fn update_vector_checkpoint(
        &self,
        checkpoint: ProjectionCheckpoint,
    ) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.vector_index
            .lock()
            .unwrap()
            .update_checkpoint(checkpoint);
        Ok(())
    }

    pub fn close(self) -> Result<(), StoreError> {
        {
            let mut closed = self.closed.lock().unwrap();
            if *closed {
                return Ok(());
            }
            *closed = true;
        }

        let mut wal = self.wal.lock().unwrap();
        wal.fsync()?;

        // Write a checkpoint so next open replays only new WAL entries.
        let commit_count = self.commits.lock().unwrap().len() as u64;
        let wal_offset = wal.current_offset();
        drop(wal);

        self.checkpoint_log
            .lock()
            .unwrap()
            .write(CheckpointRecord {
                wal_offset,
                commit_count,
            })?;

        // Persist vector projection so it survives a clean restart.
        save_vector_projection(&self.store_path, &self.vector_index.lock().unwrap());

        if self.dirty_marker.exists() {
            fs::remove_file(&self.dirty_marker)?;
        }

        Ok(())
    }
}

// ── private helpers ────────────────────────────────────────────────────────────

use crate::edge::EdgeVersionId;
use crate::object::ObjectVersionId;

fn load_vector_projection(store_path: &Path) -> VectorIndex {
    let path = store_path.join("vectors.proj");
    if let Ok(data) = fs::read(&path) {
        if let Some((entries, checkpoint)) = deserialize_projection(&data) {
            let mut idx = VectorIndex::new();
            for entry in entries {
                idx.insert(entry);
            }
            idx.update_checkpoint(checkpoint);
            return idx;
        }
    }
    VectorIndex::new()
}

fn save_vector_projection(store_path: &Path, index: &VectorIndex) {
    let path = store_path.join("vectors.proj");
    if let Ok(data) = serialize_projection(index) {
        let _ = fs::write(&path, data);
    }
}

fn build_version_stores(
    commits: &[BinderCommit],
) -> (Vec<BinderObjectVersion>, Vec<BinderEdgeVersion>) {
    let mut ovs: Vec<BinderObjectVersion> = Vec::new();
    let mut evs: Vec<BinderEdgeVersion> = Vec::new();

    for commit in commits {
        let new_ovs = derive_object_versions_for_commit(commit);
        let new_evs = derive_edge_versions_for_commit(commit);
        retire_superseded_object_versions(&mut ovs, &new_ovs, commit.commit_id);
        retire_superseded_edge_versions(&mut evs, &new_evs, commit.commit_id);
        ovs.extend(new_ovs);
        evs.extend(new_evs);
    }

    (ovs, evs)
}

fn derive_object_versions_for_commit(commit: &BinderCommit) -> Vec<BinderObjectVersion> {
    commit
        .object_changes
        .iter()
        .map(|change| BinderObjectVersion {
            version_id: ObjectVersionId::new(),
            object_id: change.object_id.clone(),
            commit_id: commit.commit_id,
            valid_from: commit.commit_id,
            valid_to: None,
            payload: change.payload.clone(),
            provenance: change.provenance.clone(),
        })
        .collect()
}

fn derive_edge_versions_for_commit(commit: &BinderCommit) -> Vec<BinderEdgeVersion> {
    commit
        .relationship_changes
        .iter()
        .map(|change| BinderEdgeVersion {
            version_id: EdgeVersionId::new(),
            edge_id: change.edge_id.clone(),
            commit_id: commit.commit_id,
            valid_from: commit.commit_id,
            valid_to: None,
            from_object: change.from_object.clone(),
            to_object: change.to_object.clone(),
            relationship_type: change.relationship_type.clone(),
            payload: change.payload.clone(),
            provenance: change.provenance.clone(),
        })
        .collect()
}

fn retire_superseded_object_versions(
    existing: &mut [BinderObjectVersion],
    new_versions: &[BinderObjectVersion],
    retiring_commit_id: CommitId,
) {
    let superseded: HashSet<&ObjectId> = new_versions.iter().map(|v| &v.object_id).collect();
    for v in existing.iter_mut() {
        if superseded.contains(&v.object_id) && v.valid_to.is_none() {
            v.valid_to = Some(retiring_commit_id);
        }
    }
}

fn retire_superseded_edge_versions(
    existing: &mut [BinderEdgeVersion],
    new_versions: &[BinderEdgeVersion],
    retiring_commit_id: CommitId,
) {
    let superseded: HashSet<&EdgeId> = new_versions.iter().map(|v| &v.edge_id).collect();
    for v in existing.iter_mut() {
        if superseded.contains(&v.edge_id) && v.valid_to.is_none() {
            v.valid_to = Some(retiring_commit_id);
        }
    }
}
