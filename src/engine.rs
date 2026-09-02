use crate::aggregation::{run_aggregation, AggregateResult, ObservationAggregation};
use crate::checkpoint::{CheckpointLog, CheckpointRecord};
use crate::commit::BinderCommit;
use crate::edge::BinderEdgeVersion;
use crate::error::StoreError;
use crate::graph_query::{Direction, GraphQuery, GraphQueryMatch};
use crate::index::graph::GraphIndex;
use crate::index::head_edge::HeadEdgeIndex;
use crate::index::head_object::HeadObjectIndex;
use crate::index::namespace::NamespaceObjectIndex;
use crate::namespace::NamespaceRegistry;
use crate::object::BinderObjectVersion;
use crate::snapshot::{read_snapshot, write_snapshot, VersionSnapshot};
use crate::transaction::Transaction;
use crate::types::{CommitId, EdgeId, ObjectId};
use crate::vector::{
    deserialize_projection, serialize_projection, ProjectionCheckpoint, VectorEntry, VectorFilter,
    VectorIndex, VectorSearchResult,
};
use crate::version_store::VersionStore;
use crate::wal::Wal;
use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Secondary index: namespace scheme → live ObjectIds.  Turns list() from
    /// O(n_all_versions_ever) into O(k_live_in_namespace).  Non-canonical.
    namespace_index: Mutex<NamespaceObjectIndex>,
    namespace_registry: Mutex<NamespaceRegistry>,
    vector_index: Mutex<VectorIndex>,
    /// Position index: ObjectId → index of its current (valid_to=None) entry in
    /// `object_versions`.  Makes version retirement O(1) instead of O(n_total).
    head_object_positions: Mutex<HashMap<ObjectId, usize>>,
    /// Same for edges.
    head_edge_positions: Mutex<HashMap<EdgeId, usize>>,
    /// Number of historical commits that have been compacted away (not present
    /// in the WAL).  After compact(), new WAL frames start at offset 0 but
    /// self.commits still holds all historical commits in memory.  This offset
    /// lets close() compute the WAL-relative commit count for the snapshot.
    wal_commit_offset: Mutex<u64>,
    /// Non-canonical text projection: object_id → searchable text content.
    /// Callers register content via `index_text()` after commits.  Not stored
    /// in the WAL; rebuilt by callers on open.  Supports case-insensitive
    /// substring search via `text_search()`.
    text_index: Mutex<HashMap<ObjectId, String>>,
    /// Disk-backed version history store.  Keeps only a small index in RAM;
    /// full version structs are read from `version_log.bin` on demand.
    /// Powers `object_history()`, `object_as_of()`, `edge_history()`, `edge_as_of()`.
    version_store: Mutex<VersionStore>,
}

impl BinderStore {
    pub fn open(store_path: &Path) -> Result<Self, StoreError> {
        Self::open_inner(
            store_path,
            crate::index::head_object::DEFAULT_HEAD_CACHE_CAP,
        )
    }

    /// Like `open` but with a custom head-cache capacity.
    /// `cap = 0` disables eviction (unbounded).  Useful for tests and for
    /// deployments that want to tune memory vs. latency explicitly.
    pub fn open_with_head_cap(
        store_path: &Path,
        head_cache_cap: usize,
    ) -> Result<Self, StoreError> {
        Self::open_inner(store_path, head_cache_cap)
    }

    fn open_inner(store_path: &Path, head_cache_cap: usize) -> Result<Self, StoreError> {
        fs::create_dir_all(store_path)?;

        let dirty_marker = store_path.join(".dirty");
        let was_clean = !dirty_marker.exists();
        fs::write(&dirty_marker, b"")?;

        let wal_path = store_path.join("commits.wal");
        let checkpoint_path = store_path.join("checkpoint.log");
        let snapshot_path = store_path.join("store.snap");

        let mut checkpoint_log = CheckpointLog::open_or_create(&checkpoint_path)?;
        let last_checkpoint = checkpoint_log.last()?;

        // Snapshot fast path: load version stores (+ commit log) from snap,
        // replay only WAL frames after wal_offset. Falls back to full WAL
        // replay when the snapshot is missing, mismatched, or too old to
        // carry `commits` (needed for scan_commits completeness).
        //
        // `hydrate_ok`: clean reopen with no WAL delta — version_log.bin can
        // be indexed in place instead of rewritten.
        let mut hydrate_ok = false;
        // full_versions_for_rebuild: populated on full WAL replay paths so the
        // VersionStore can be rebuilt from complete history (live + retired).
        // None on snapshot+delta path (VersionStore is updated incrementally).
        let mut full_versions_for_rebuild: Option<(
            Vec<BinderObjectVersion>,
            Vec<BinderEdgeVersion>,
        )> = None;
        // vs_delta_commits: on snapshot+delta path, the commits that must be
        // applied to the existing version_log.bin (hydrate + apply delta).
        let mut vs_delta_commits: Vec<BinderCommit> = Vec::new();

        let (wal, commits, object_versions, edge_versions, ov_positions, ev_positions) =
            if let Some(cp) = last_checkpoint {
                if let Some(snap) = read_snapshot(&snapshot_path) {
                    if snap.wal_offset == cp.wal_offset && snap.commit_count == cp.commit_count {
                        let can_skip_full_wal = snap.commit_count == 0 || !snap.commits.is_empty();
                        if can_skip_full_wal {
                            let mut wal = Wal::open_without_replay(&wal_path)?;
                            let (delta, _) = wal.replay_from(cp.wal_offset)?;
                            // Entry ids continue after historical frames still on disk
                            // (or after compacted empty WAL + delta).
                            wal.set_next_entry_id(cp.commit_count + delta.len() as u64);
                            hydrate_ok = was_clean && delta.is_empty();

                            if !hydrate_ok {
                                vs_delta_commits = delta.clone();
                            }

                            // build_version_stores_delta now returns live-only Vecs.
                            let (ovs, evs, ov_pos, ev_pos) = if delta.is_empty() {
                                // No delta: filter snapshot to live-only (backward compat).
                                build_version_stores_delta(
                                    &delta,
                                    snap.object_versions,
                                    snap.edge_versions,
                                    snap.ov_positions,
                                    snap.ev_positions,
                                )
                            } else {
                                build_version_stores_delta(
                                    &delta,
                                    snap.object_versions,
                                    snap.edge_versions,
                                    snap.ov_positions,
                                    snap.ev_positions,
                                )
                            };

                            let mut commits = snap.commits;
                            commits.extend(delta);
                            (wal, commits, ovs, evs, ov_pos, ev_pos)
                        } else {
                            // Legacy snapshot without commits: full WAL replay.
                            let (wal, commits) = Wal::open_or_create(&wal_path)?;
                            let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
                            let (ovs, evs, ov_pos, ev_pos) =
                                compact_live_only_from_full(&full_ovs, &full_evs);
                            full_versions_for_rebuild = Some((full_ovs, full_evs));
                            (wal, commits, ovs, evs, ov_pos, ev_pos)
                        }
                    } else {
                        // Snapshot/checkpoint mismatch: full WAL replay.
                        let (wal, commits) = Wal::open_or_create(&wal_path)?;
                        let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
                        let (ovs, evs, ov_pos, ev_pos) =
                            compact_live_only_from_full(&full_ovs, &full_evs);
                        full_versions_for_rebuild = Some((full_ovs, full_evs));
                        (wal, commits, ovs, evs, ov_pos, ev_pos)
                    }
                } else {
                    // No snapshot: full WAL replay.
                    let (wal, commits) = Wal::open_or_create(&wal_path)?;
                    let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
                    let (ovs, evs, ov_pos, ev_pos) =
                        compact_live_only_from_full(&full_ovs, &full_evs);
                    full_versions_for_rebuild = Some((full_ovs, full_evs));
                    (wal, commits, ovs, evs, ov_pos, ev_pos)
                }
            } else {
                // No checkpoint: full WAL replay.
                let (wal, commits) = Wal::open_or_create(&wal_path)?;
                let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
                let (ovs, evs, ov_pos, ev_pos) = compact_live_only_from_full(&full_ovs, &full_evs);
                full_versions_for_rebuild = Some((full_ovs, full_evs));
                (wal, commits, ovs, evs, ov_pos, ev_pos)
            };

        let head = commits
            .last()
            .map(|c| c.commit_id)
            .or_else(|| head_from_versions(&object_versions, &edge_versions));
        let head_objects = HeadObjectIndex::rebuild_with_cap(&object_versions, head_cache_cap);
        let head_edges = HeadEdgeIndex::rebuild_with_cap(&edge_versions, head_cache_cap);
        let graph = GraphIndex::rebuild(edge_versions.iter().filter(|v| !v.is_tombstone()));
        let neighbours =
            GraphIndex::build_neighbour_map(edge_versions.iter().filter(|v| !v.is_tombstone()));
        // All Vec entries are now live (valid_to=None), so no filter needed.
        let namespace_index =
            NamespaceObjectIndex::rebuild(object_versions.iter().filter(|v| !v.is_tombstone()));

        // Load vector projection from previous clean close, if present.
        let vector_index = load_vector_projection(store_path);

        // Disk-backed version history:
        // - hydrate_ok (clean reopen, no delta): defer index loading until first query.
        // - full WAL replay paths: rebuild the log from complete history.
        // - snapshot+delta, clean open: hydrate existing file + apply delta.
        // - snapshot+delta, dirty open: rebuild from full commit log to cover any
        //   partial truncation of version_log.bin that occurred during the crash.
        let version_log_path = store_path.join("version_log.bin");
        let mut version_store = VersionStore::open_or_create(&version_log_path)?;
        if !hydrate_ok {
            if let Some((ref full_ovs, ref full_evs)) = full_versions_for_rebuild {
                // Full WAL replay path: complete history is already available.
                version_store.rebuild_from_versions(full_ovs, full_evs)?;
            } else if !was_clean {
                // Dirty open on snapshot+delta path: version_log.bin may have been
                // partially truncated during the crash. Rebuild from the full commit
                // log (already loaded into `commits`) to guarantee correctness.
                let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
                version_store.rebuild_from_versions(&full_ovs, &full_evs)?;
            } else {
                // Clean open with WAL delta: version_log.bin is intact; safe to hydrate.
                version_store.hydrate_index_from_file()?;
                for commit in &vs_delta_commits {
                    apply_commit_to_version_store(&mut version_store, commit);
                }
            }
        }

        // Final guard: if version_log.bin was completely wiped (e.g. the OS
        // crashed while creating/truncating the file), rebuild from the full
        // commit log now.  This covers the hydrate_ok path where deferred
        // hydration would otherwise silently return empty history results.
        if !version_store.has_data() && !commits.is_empty() {
            let (full_ovs, full_evs, _, _) = build_version_stores(&commits);
            version_store.rebuild_from_versions(&full_ovs, &full_evs)?;
        }

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
            namespace_index: Mutex::new(namespace_index),
            namespace_registry: Mutex::new(NamespaceRegistry::new()),
            vector_index: Mutex::new(vector_index),
            head_object_positions: Mutex::new(ov_positions),
            head_edge_positions: Mutex::new(ev_positions),
            wal_commit_offset: Mutex::new(0),
            text_index: Mutex::new(HashMap::new()),
            version_store: Mutex::new(version_store),
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

        self.apply_commit_to_memory(commit_id, commit);
        Ok(commit_id)
    }

    /// Like `commit` but skips the per-commit fsync.  In-memory state is
    /// updated immediately so reads are consistent, but the data is not durable
    /// until `flush()` is called.  Use for bulk ingestion where throughput
    /// matters more than per-write crash safety.
    ///
    /// Always call `flush()` (or `close()`) before the process exits.
    pub fn commit_buffered(
        &self,
        tx: Transaction,
        actor: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<CommitId, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        if tx.is_empty() {
            return Err(StoreError::EmptyTransaction);
        }

        let actor: String = actor.into();
        let message: String = message.into();
        let _write_guard = self.write_lock.lock().unwrap();

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

        // Write WAL frame without fsync — data loss window until flush() is called.
        self.wal.lock().unwrap().append_no_sync(&commit)?;

        self.apply_commit_to_memory(commit_id, commit);

        Ok(commit_id)
    }

    /// Commit multiple transactions as a single durable batch.
    ///
    /// Equivalent to calling `commit_buffered()` N times then `flush()`, but
    /// acquires the write lock and WAL file handle exactly once for the entire
    /// batch.  This eliminates per-commit lock overhead and issues a single
    /// fsync for all N frames.
    ///
    /// - Returns a `Vec<CommitId>` in the same order as the input.
    /// - An empty `batch` is a no-op that returns `Ok(vec![])`.
    /// - Any empty `Transaction` in the batch causes the entire call to fail
    ///   with `StoreError::EmptyTransaction` before touching the WAL.
    pub fn commit_many(
        &self,
        batch: Vec<(Transaction, String, String)>,
    ) -> Result<Vec<CommitId>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        for (tx, _, _) in &batch {
            if tx.is_empty() {
                return Err(StoreError::EmptyTransaction);
            }
        }

        let _write_guard = self.write_lock.lock().unwrap();

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let n = batch.len();
        let mut commit_ids = Vec::with_capacity(n);
        let mut prepared = Vec::with_capacity(n);
        let mut parent = *self.head.lock().unwrap();

        // Build all BinderCommit structs and write them to the WAL in one
        // lock hold, then fsync once.
        {
            let mut wal = self.wal.lock().unwrap();
            for (tx, actor, message) in batch {
                let commit_id = CommitId::new();
                let commit = BinderCommit {
                    commit_id,
                    parent_commit: parent,
                    timestamp_ms,
                    actor,
                    message,
                    object_changes: tx.object_changes,
                    relationship_changes: tx.relationship_changes,
                };
                wal.append_no_sync(&commit)?;
                parent = Some(commit_id);
                commit_ids.push(commit_id);
                prepared.push(commit);
            }
            wal.fsync()?;
        }

        // Pre-size shared Vecs once for the entire batch before the per-commit
        // apply loop so the inner reserve() calls in apply_commit_to_memory
        // are no-ops.
        {
            let total_objects: usize = prepared.iter().map(|c| c.object_changes.len()).sum();
            let total_edges: usize = prepared.iter().map(|c| c.relationship_changes.len()).sum();
            self.object_versions.lock().unwrap().reserve(total_objects);
            self.edge_versions.lock().unwrap().reserve(total_edges);
            self.head_object_positions
                .lock()
                .unwrap()
                .reserve(total_objects);
            self.head_edge_positions
                .lock()
                .unwrap()
                .reserve(total_edges);
        }

        for (commit_id, commit) in commit_ids.iter().copied().zip(prepared) {
            self.apply_commit_to_memory(commit_id, commit);
        }

        Ok(commit_ids)
    }

    /// Fsync the WAL — makes all preceding `commit_buffered()` calls durable.
    /// No-op if there is nothing pending.  Always called by `close()`.
    pub fn flush(&self) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.wal.lock().unwrap().fsync()
    }

    /// Apply a committed `BinderCommit` to all in-memory indexes.
    /// Called by both `commit_with_namespace` (after WAL fsync) and
    /// `commit_buffered` (after WAL write without fsync).
    ///
    /// Version retirement is O(1) per changed object/edge via the position
    /// indexes (`head_object_positions`, `head_edge_positions`) — no Vec scan.
    fn apply_commit_to_memory(&self, commit_id: CommitId, commit: BinderCommit) {
        let new_ovs = derive_object_versions_for_commit(&commit);
        let new_evs = derive_edge_versions_for_commit(&commit);

        {
            let mut ovs = self.object_versions.lock().unwrap();
            let mut hobj = self.head_objects.lock().unwrap();
            let mut nsidx = self.namespace_index.lock().unwrap();
            let mut ov_pos = self.head_object_positions.lock().unwrap();

            // Swap-remove retiring entries from the live-only Vec.
            for v in &new_ovs {
                if let Some(old_idx) = ov_pos.remove(&v.object_id) {
                    let last = ovs.len() - 1;
                    if old_idx != last {
                        ovs.swap(old_idx, last);
                        let moved = ovs[old_idx].object_id.clone();
                        ov_pos.insert(moved, old_idx);
                    }
                    ovs.pop();
                    nsidx.remove(&v.object_id);
                }
            }

            // Append live new versions and update indexes.
            let base = ovs.len();
            let mut live_i = 0usize;
            let live_count = new_ovs.iter().filter(|v| !v.is_tombstone()).count();
            ovs.reserve(live_count);
            ov_pos.reserve(live_count);
            for v in &new_ovs {
                hobj.upsert(v.clone());
                if !v.is_tombstone() {
                    ovs.push(v.clone());
                    ov_pos.insert(v.object_id.clone(), base + live_i);
                    nsidx.insert(&v.object_id);
                    live_i += 1;
                } else {
                    // tombstones: upserted into hobj (cache) but not Vec/ov_pos/nsidx
                    nsidx.remove(&v.object_id);
                }
            }
        }
        {
            let mut evs = self.edge_versions.lock().unwrap();
            let mut hedge = self.head_edges.lock().unwrap();
            let mut graph = self.graph.lock().unwrap();
            let mut nbrs = self.neighbours.lock().unwrap();
            let mut ev_pos = self.head_edge_positions.lock().unwrap();

            // Clean up graph/neighbour state for superseded edges and swap-remove from Vec.
            for new_ev in &new_evs {
                if let Some(old) = hedge.get(&new_ev.edge_id) {
                    graph.remove(old);
                    if let (Some(from), Some(to)) = (&old.from_object, &old.to_object) {
                        if let Some(tos) = nbrs.get_mut(from) {
                            tos.retain(|t| t != to);
                        }
                    }
                }
                // Swap-remove retiring edge entry from live-only Vec.
                if let Some(old_idx) = ev_pos.remove(&new_ev.edge_id) {
                    let last = evs.len() - 1;
                    if old_idx != last {
                        evs.swap(old_idx, last);
                        let moved = evs[old_idx].edge_id.clone();
                        ev_pos.insert(moved, old_idx);
                    }
                    evs.pop();
                }
            }

            // Append live new edges and update indexes.
            let base = evs.len();
            let mut live_i = 0usize;
            let live_count = new_evs.iter().filter(|v| !v.is_tombstone()).count();
            evs.reserve(live_count);
            ev_pos.reserve(live_count);
            for v in &new_evs {
                hedge.upsert(v.clone());
                if !v.is_tombstone() {
                    evs.push(v.clone());
                    ev_pos.insert(v.edge_id.clone(), base + live_i);
                    live_i += 1;
                    graph.insert(v);
                    if let (Some(from), Some(to)) = (&v.from_object, &v.to_object) {
                        nbrs.entry(from.clone()).or_default().push(to.clone());
                    }
                }
            }
        }

        // Mirror the commit into the disk-backed version store.
        // Retirements must be written before new versions so the index's
        // valid_to for the old version is set before the new one is appended.
        {
            let _ = self.ensure_version_store_ready();
            let mut vs = self.version_store.lock().unwrap();
            for v in &new_ovs {
                // Retire the old version in the disk store if it existed.
                let _ = vs.retire_object(&v.object_id, commit_id);
            }
            for v in &new_evs {
                let _ = vs.retire_edge(&v.edge_id, commit_id);
            }
            for v in &new_ovs {
                let _ = vs.append_object(v);
            }
            for v in &new_evs {
                let _ = vs.append_edge(v);
            }
        }

        *self.head.lock().unwrap() = Some(commit_id);
        self.commits.lock().unwrap().push(commit);
    }

    // ── query API ─────────────────────────────────────────────────────────────

    /// Current version of an object.
    ///
    /// Checks the in-memory head cache first (O(1)).  On a cache miss the
    /// entry may have been evicted under memory pressure; the call falls back
    /// to the mmap-backed VersionStore (one page-cache read, typically ~1 µs).
    /// Returns None if the object is unknown or has been tombstoned.
    pub fn get(&self, object_id: &ObjectId) -> Result<Option<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        // Fast path: head cache hit.
        let cached = self
            .head_objects
            .lock()
            .unwrap()
            .get(object_id)
            .filter(|v| !v.is_tombstone())
            .cloned();
        if cached.is_some() {
            return Ok(cached);
        }
        // Slow path: cache miss — read head version from the mmap'd log.
        self.ensure_version_store_ready()?;
        let v = self
            .version_store
            .lock()
            .unwrap()
            .get_head_object(object_id)
            .map_err(StoreError::Io)?
            .filter(|v| !v.is_tombstone());
        Ok(v)
    }

    /// Live object IDs whose scheme/ID matches `namespace_prefix` (same matching
    /// rules as [`Self::list`]). Does not load payloads — preferred for CLI and
    /// counting.
    pub fn list_object_ids(&self, namespace_prefix: &str) -> Result<Vec<ObjectId>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        Ok(self
            .namespace_index
            .lock()
            .unwrap()
            .matching(namespace_prefix))
    }

    /// All live (non-tombstone) current object versions whose object_id starts
    /// with `namespace_prefix`.
    ///
    /// Uses the namespace secondary index for O(k) performance where k is the
    /// number of live objects in the namespace, rather than scanning all versions.
    /// For each ID, tries the head cache; misses fall back to the VersionStore.
    pub fn list(&self, namespace_prefix: &str) -> Result<Vec<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let object_ids = self.list_object_ids(namespace_prefix)?;
        let mut results = Vec::with_capacity(object_ids.len());
        let mut misses: Vec<ObjectId> = Vec::new();
        {
            let head = self.head_objects.lock().unwrap();
            for oid in &object_ids {
                match head.get(oid).filter(|v| !v.is_tombstone()).cloned() {
                    Some(v) => results.push(v),
                    None => misses.push(oid.clone()),
                }
            }
        }
        if misses.is_empty() {
            return Ok(results);
        }
        // Resolve misses from the disk-backed VersionStore.
        self.ensure_version_store_ready()?;
        let mut vs = self.version_store.lock().unwrap();
        for oid in &misses {
            if let Some(v) = vs.get_head_object(oid).map_err(StoreError::Io)? {
                if !v.is_tombstone() {
                    results.push(v);
                }
            }
        }
        Ok(results)
    }

    /// Ensure the disk-backed version index is loaded. Clean snapshot reopens
    /// defer this until the first history/as_of query or write.
    fn ensure_version_store_ready(&self) -> Result<(), StoreError> {
        let mut vs = self.version_store.lock().unwrap();
        if !vs.index_is_empty() {
            return Ok(());
        }
        if vs.has_data() {
            // Hydrate from the existing log only. Never rebuild-from-memory here:
            // commit paths append after ensure, and a memory rebuild would
            // duplicate the versions about to be appended.
            vs.hydrate_index_from_file()?;
        }
        Ok(())
    }

    /// Full version history of an object in ascending commit order.
    /// Reads from the disk-backed `VersionStore` to avoid loading historical
    /// payloads into RAM.
    pub fn object_history(
        &self,
        object_id: &ObjectId,
    ) -> Result<Vec<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.ensure_version_store_ready()?;
        Ok(self
            .version_store
            .lock()
            .unwrap()
            .object_history(object_id)?)
    }

    /// Version of an object at a specific past commit (historical query).
    /// Returns None if the object didn't exist at that commit or was tombstoned.
    /// Reads from the disk-backed `VersionStore`.
    pub fn object_as_of(
        &self,
        object_id: &ObjectId,
        at_commit: CommitId,
    ) -> Result<Option<BinderObjectVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.ensure_version_store_ready()?;
        let result = self
            .version_store
            .lock()
            .unwrap()
            .object_as_of(object_id, at_commit)?
            .filter(|v| !v.is_tombstone());
        Ok(result)
    }

    /// Full version history of an edge.
    /// Reads from the disk-backed `VersionStore`.
    pub fn edge_history(&self, edge_id: &EdgeId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.ensure_version_store_ready()?;
        Ok(self.version_store.lock().unwrap().edge_history(edge_id)?)
    }

    /// Version of an edge at a specific past commit.
    /// Reads from the disk-backed `VersionStore`.
    pub fn edge_as_of(
        &self,
        edge_id: &EdgeId,
        at_commit: CommitId,
    ) -> Result<Option<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.ensure_version_store_ready()?;
        Ok(self
            .version_store
            .lock()
            .unwrap()
            .edge_as_of(edge_id, at_commit)?)
    }

    /// Live edges originating from an object (graph index lookup).
    pub fn edges_from(&self, object_id: &ObjectId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let edge_ids: Vec<EdgeId> = self
            .graph
            .lock()
            .unwrap()
            .edges_from(object_id)
            .cloned()
            .collect();
        self.resolve_edges(edge_ids)
    }

    /// Live edges pointing to an object.
    pub fn edges_to(&self, object_id: &ObjectId) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let edge_ids: Vec<EdgeId> = self
            .graph
            .lock()
            .unwrap()
            .edges_to(object_id)
            .cloned()
            .collect();
        self.resolve_edges(edge_ids)
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
        let edge_ids: Vec<EdgeId> = self
            .graph
            .lock()
            .unwrap()
            .edges_from_with_type(object_id, rel_type)
            .collect();
        self.resolve_edges(edge_ids)
    }

    /// Resolve a list of edge IDs to their current versions.
    /// Checks the head cache first; falls back to VersionStore on misses.
    fn resolve_edges(&self, edge_ids: Vec<EdgeId>) -> Result<Vec<BinderEdgeVersion>, StoreError> {
        let mut results = Vec::with_capacity(edge_ids.len());
        let mut misses: Vec<EdgeId> = Vec::new();
        {
            let hedge = self.head_edges.lock().unwrap();
            for eid in &edge_ids {
                match hedge.get(eid).cloned() {
                    Some(v) => results.push(v),
                    None => misses.push(eid.clone()),
                }
            }
        }
        if misses.is_empty() {
            return Ok(results);
        }
        self.ensure_version_store_ready()?;
        let mut vs = self.version_store.lock().unwrap();
        for eid in &misses {
            if let Some(v) = vs.get_head_edge(eid).map_err(StoreError::Io)? {
                results.push(v);
            }
        }
        Ok(results)
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

    // ── graph query DSL ───────────────────────────────────────────────────────

    /// Start a composable graph query from this store.
    ///
    /// ```ignore
    /// let results = store
    ///     .graph_query()
    ///     .from(&person_id)
    ///     .edge_types(["RELATES_TO"])
    ///     .max_depth(3)
    ///     .filter_node(|obj| obj.object_id.starts_with("memory://"))
    ///     .execute()?;
    /// ```
    pub fn graph_query(&self) -> GraphQuery<'_> {
        GraphQuery::new(self)
    }

    /// Execute a fully-configured `GraphQuery`.  Called by `GraphQuery::execute()`.
    pub(crate) fn execute_graph_query(
        &self,
        q: GraphQuery<'_>,
    ) -> Result<Vec<GraphQueryMatch>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let start = q.start.ok_or(StoreError::QueryMissingStart)?;

        let graph = self.graph.lock().unwrap();
        let head_edges = self.head_edges.lock().unwrap();
        let head_objects = self.head_objects.lock().unwrap();

        let mut visited: HashSet<ObjectId> = HashSet::new();
        visited.insert(start.clone());

        let mut queue: VecDeque<(ObjectId, usize)> = VecDeque::new();
        queue.push_back((start, 0));

        let mut results: Vec<GraphQueryMatch> = Vec::new();

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= q.max_depth {
                continue;
            }

            // Collect candidate edge IDs based on direction and allowed types.
            let mut edge_ids: Vec<EdgeId> = Vec::new();
            if matches!(q.direction, Direction::Outgoing | Direction::Both) {
                if q.allowed_edge_types.is_empty() {
                    edge_ids.extend(graph.edges_from(&current).cloned());
                } else {
                    for t in &q.allowed_edge_types {
                        edge_ids.extend(graph.edges_from_with_type(&current, t));
                    }
                }
            }
            if matches!(q.direction, Direction::Incoming | Direction::Both) {
                if q.allowed_edge_types.is_empty() {
                    edge_ids.extend(graph.edges_to(&current).cloned());
                } else {
                    for t in &q.allowed_edge_types {
                        // No `edges_to_with_type` yet — filter inline.
                        edge_ids.extend(
                            graph
                                .edges_to(&current)
                                .filter(|eid| {
                                    head_edges
                                        .get(eid)
                                        .and_then(|e| e.relationship_type.as_deref())
                                        == Some(t.as_str())
                                })
                                .cloned(),
                        );
                    }
                }
            }

            for eid in &edge_ids {
                let Some(edge) = head_edges.get(eid) else {
                    continue;
                };
                if edge.is_tombstone() {
                    continue;
                }

                // Apply optional edge predicate.
                if let Some(ef) = &q.edge_filter {
                    if !ef(edge) {
                        continue;
                    }
                }

                // Determine the "other" node depending on direction.
                let neighbor = match q.direction {
                    Direction::Outgoing => edge.to_object.as_ref(),
                    Direction::Incoming => edge.from_object.as_ref(),
                    Direction::Both => {
                        // Pick the end that isn't current.
                        if edge.from_object.as_ref() == Some(&current) {
                            edge.to_object.as_ref()
                        } else {
                            edge.from_object.as_ref()
                        }
                    }
                };

                let Some(neighbor) = neighbor else {
                    continue;
                };
                if visited.contains(neighbor) {
                    continue;
                }
                visited.insert(neighbor.clone());

                let Some(obj) = head_objects.get(neighbor) else {
                    continue;
                };
                if obj.is_tombstone() {
                    continue;
                }

                // Apply optional node predicate.
                let passes = q.node_filter.as_ref().is_none_or(|f| f(obj));
                if passes {
                    results.push(GraphQueryMatch {
                        object: obj.clone(),
                        depth: depth + 1,
                        via_edge: Some(edge.clone()),
                    });
                }

                // Always enqueue for further expansion regardless of filter.
                queue.push_back((neighbor.clone(), depth + 1));
            }
        }

        Ok(results)
    }

    /// Like `execute_graph_query` but also returns a `parent_map` for path
    /// reconstruction. Each entry maps a visited node to the node it was
    /// reached from; the start node has no entry (it is the root).
    /// The second map records the edge used to first reach each node (edge parent map).
    #[allow(clippy::type_complexity)]
    pub(crate) fn execute_graph_query_with_paths(
        &self,
        q: GraphQuery<'_>,
    ) -> Result<
        (
            Vec<GraphQueryMatch>,
            HashMap<ObjectId, ObjectId>,
            HashMap<ObjectId, EdgeId>,
        ),
        StoreError,
    > {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let start = q.start.clone().ok_or(StoreError::QueryMissingStart)?;

        let graph = self.graph.lock().unwrap();
        let head_edges = self.head_edges.lock().unwrap();
        let head_objects = self.head_objects.lock().unwrap();

        let mut visited: HashSet<ObjectId> = HashSet::new();
        visited.insert(start.clone());

        let mut queue: VecDeque<(ObjectId, usize)> = VecDeque::new();
        queue.push_back((start.clone(), 0));

        let mut results: Vec<GraphQueryMatch> = Vec::new();
        let mut parent_map: HashMap<ObjectId, ObjectId> = HashMap::new();
        let mut parent_edge_map: HashMap<ObjectId, EdgeId> = HashMap::new();

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= q.max_depth {
                continue;
            }

            let mut edge_ids: Vec<EdgeId> = Vec::new();
            if matches!(q.direction, Direction::Outgoing | Direction::Both) {
                if q.allowed_edge_types.is_empty() {
                    edge_ids.extend(graph.edges_from(&current).cloned());
                } else {
                    for t in &q.allowed_edge_types {
                        edge_ids.extend(graph.edges_from_with_type(&current, t));
                    }
                }
            }
            if matches!(q.direction, Direction::Incoming | Direction::Both) {
                if q.allowed_edge_types.is_empty() {
                    edge_ids.extend(graph.edges_to(&current).cloned());
                } else {
                    for t in &q.allowed_edge_types {
                        edge_ids.extend(
                            graph
                                .edges_to(&current)
                                .filter(|eid| {
                                    head_edges
                                        .get(eid)
                                        .and_then(|e| e.relationship_type.as_deref())
                                        == Some(t.as_str())
                                })
                                .cloned(),
                        );
                    }
                }
            }

            for eid in &edge_ids {
                let Some(edge) = head_edges.get(eid) else {
                    continue;
                };
                if edge.is_tombstone() {
                    continue;
                }
                if let Some(ef) = &q.edge_filter {
                    if !ef(edge) {
                        continue;
                    }
                }

                let neighbor = match q.direction {
                    Direction::Outgoing => edge.to_object.as_ref(),
                    Direction::Incoming => edge.from_object.as_ref(),
                    Direction::Both => {
                        if edge.from_object.as_ref() == Some(&current) {
                            edge.to_object.as_ref()
                        } else {
                            edge.from_object.as_ref()
                        }
                    }
                };
                let Some(neighbor) = neighbor else { continue };
                if visited.contains(neighbor) {
                    continue;
                }
                visited.insert(neighbor.clone());
                parent_map.insert(neighbor.clone(), current.clone());
                parent_edge_map.insert(neighbor.clone(), eid.clone());

                let Some(obj) = head_objects.get(neighbor) else {
                    continue;
                };
                if obj.is_tombstone() {
                    continue;
                }

                let passes = q.node_filter.as_ref().is_none_or(|f| f(obj));
                if passes {
                    results.push(GraphQueryMatch {
                        object: obj.clone(),
                        depth: depth + 1,
                        via_edge: Some(edge.clone()),
                    });
                }
                queue.push_back((neighbor.clone(), depth + 1));
            }
        }

        Ok((results, parent_map, parent_edge_map))
    }

    // ── observation aggregation ───────────────────────────────────────────────

    /// Start building an aggregation query over namespace observations.
    ///
    /// # Example
    /// ```ignore
    /// let rows = store
    ///     .aggregate_observations()
    ///     .namespace("obs://health.sleep")
    ///     .extract_f64(|b| bincode::deserialize::<f64>(b).ok())
    ///     .aggregate(Aggregate::Avg)
    ///     .execute()?;
    /// ```
    pub fn aggregate_observations(&self) -> ObservationAggregation<'_> {
        ObservationAggregation::new(self)
    }

    /// Execute a fully-configured `ObservationAggregation`.
    /// Called by `ObservationAggregation::execute()`.
    pub(crate) fn execute_aggregation(
        &self,
        q: ObservationAggregation<'_>,
    ) -> Result<Vec<AggregateResult>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let ns = q
            .namespace
            .as_deref()
            .ok_or(StoreError::AggregationMissingNamespace)?;

        // Pull live objects for the namespace (HEAD versions only).
        let object_ids = self.namespace_index.lock().unwrap().matching(ns);
        let head = self.head_objects.lock().unwrap();
        let versions: Vec<_> = object_ids
            .iter()
            .filter_map(|oid| head.get(oid).filter(|v| !v.is_tombstone()).cloned())
            .collect();
        drop(head);

        Ok(run_aggregation(versions, q))
    }

    /// Export all live observation objects in a namespace as JSONL (issue #5).
    ///
    /// Each line is a JSON object:
    /// ```json
    /// {"object_id":"obs://sleep/2024-01-01","value":7.5,"committed_at":"01JXYZ..."}
    /// ```
    ///
    /// `value` is present when the payload decodes as a little-endian f64, a
    /// bincode f64, or a UTF-8 float string.  When decoding fails, `value` is
    /// omitted and the raw payload is base64-encoded as `"payload_b64"`.
    ///
    pub fn export_observations_jsonl(&self, namespace: &str) -> Result<Vec<u8>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let object_ids = self.namespace_index.lock().unwrap().matching(namespace);
        let head = self.head_objects.lock().unwrap();
        let versions: Vec<_> = object_ids
            .iter()
            .filter_map(|oid| head.get(oid).filter(|v| !v.is_tombstone()).cloned())
            .collect();
        drop(head);

        let mut out = Vec::new();
        for v in versions {
            let oid = &v.object_id.0;
            let cid = v.valid_from.to_string();
            let payload = v.payload.as_deref().unwrap_or(&[]);

            // Try decoding as f64 (LE bytes → bincode → UTF-8 float).
            let value_f64 = try_decode_f64(payload);

            if let Some(val) = value_f64 {
                out.extend_from_slice(
                    format!(
                        "{{\"object_id\":{},\"value\":{val},\"committed_at\":{}}}\n",
                        serde_json::to_string(oid).unwrap(),
                        serde_json::to_string(&cid).unwrap(),
                    )
                    .as_bytes(),
                );
            } else if let Ok(text) = std::str::from_utf8(payload) {
                // UTF-8 payload — include as raw JSON string or object.
                let json_val: serde_json::Value = serde_json::from_str(text)
                    .unwrap_or_else(|_| serde_json::Value::String(text.to_string()));
                out.extend_from_slice(
                    format!(
                        "{{\"object_id\":{},\"payload\":{},\"committed_at\":{}}}\n",
                        serde_json::to_string(oid).unwrap(),
                        serde_json::to_string(&json_val).unwrap(),
                        serde_json::to_string(&cid).unwrap(),
                    )
                    .as_bytes(),
                );
            } else {
                // Binary payload — hex-encode for portability.
                let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
                out.extend_from_slice(
                    format!(
                        "{{\"object_id\":{},\"payload_hex\":{},\"committed_at\":{}}}\n",
                        serde_json::to_string(oid).unwrap(),
                        serde_json::to_string(&hex).unwrap(),
                        serde_json::to_string(&cid).unwrap(),
                    )
                    .as_bytes(),
                );
            }
        }
        Ok(out)
    }

    /// Inner-join observations from two namespace prefixes on a derived key (issue #5).
    ///
    /// `key_fn` is called for each live observation in both namespaces and should
    /// return a short string (e.g. a date, a device ID) that identifies the join
    /// key.  Objects for which `key_fn` returns `None` are excluded.  When multiple
    /// observations in one namespace share the same key, the most recently committed
    /// one (highest `CommitId`) is used.
    ///
    /// Returns one [`JoinRow`] per key present in BOTH namespaces, sorted by key.
    ///
    /// # Example
    /// ```ignore
    /// let rows = store.join_observations(
    ///     "obs://health.sleep",
    ///     "obs://health.steps",
    ///     |v| v.object_id.0.rsplit('/').next().map(|s| s.to_owned()),
    /// )?;
    /// ```
    pub fn join_observations(
        &self,
        left_namespace: &str,
        right_namespace: &str,
        key_fn: impl Fn(&BinderObjectVersion) -> Option<String>,
    ) -> Result<Vec<crate::aggregation::JoinRow>, StoreError> {
        let left_obs = self.list(left_namespace)?;
        let right_obs = self.list(right_namespace)?;

        let mut left_map: HashMap<String, BinderObjectVersion> = HashMap::new();
        for v in left_obs {
            if let Some(key) = key_fn(&v) {
                left_map
                    .entry(key)
                    .and_modify(|existing| {
                        if v.valid_from > existing.valid_from {
                            *existing = v.clone();
                        }
                    })
                    .or_insert(v);
            }
        }

        let mut right_map: HashMap<String, BinderObjectVersion> = HashMap::new();
        for v in right_obs {
            if let Some(key) = key_fn(&v) {
                right_map
                    .entry(key)
                    .and_modify(|existing| {
                        if v.valid_from > existing.valid_from {
                            *existing = v.clone();
                        }
                    })
                    .or_insert(v);
            }
        }

        let mut rows: Vec<crate::aggregation::JoinRow> = left_map
            .into_iter()
            .filter_map(|(key, left)| {
                right_map
                    .remove(&key)
                    .map(|right| crate::aggregation::JoinRow { key, left, right })
            })
            .collect();

        rows.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(rows)
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

        // Rebuild position maps: all Vec entries are now live (valid_to=None),
        // so no filter is needed.
        let new_ov_pos: HashMap<ObjectId, usize> = ovs
            .iter()
            .enumerate()
            .map(|(i, v)| (v.object_id.clone(), i))
            .collect();
        let new_ev_pos: HashMap<EdgeId, usize> = evs
            .iter()
            .enumerate()
            .map(|(i, v)| (v.edge_id.clone(), i))
            .collect();

        // Note: rebuild_from_versions is NOT called here — the Vec is already
        // live-only and the VersionStore on disk is authoritative for history.
        // The VersionStore is updated incrementally by apply_commit_to_memory.

        // Rebuild namespace index from live Vec entries.
        let new_namespace_index =
            NamespaceObjectIndex::rebuild(ovs.iter().filter(|v| !v.is_tombstone()));

        drop(ovs);
        drop(evs);

        *self.head_objects.lock().unwrap() = new_head_objects;
        *self.head_edges.lock().unwrap() = new_head_edges;
        *self.graph.lock().unwrap() = new_graph;
        *self.neighbours.lock().unwrap() = new_neighbours;
        *self.namespace_index.lock().unwrap() = new_namespace_index;
        *self.head_object_positions.lock().unwrap() = new_ov_pos;
        *self.head_edge_positions.lock().unwrap() = new_ev_pos;

        Ok(())
    }

    /// Compact the store: write a snapshot of the current state and truncate
    /// the WAL to zero bytes.
    ///
    /// After `compact()`, `open()` takes the snapshot fast path and only
    /// replays WAL frames written after this call.  The WAL cannot grow
    /// unboundedly when `compact()` is called periodically (e.g. on close,
    /// after a bulk import, or when WAL size exceeds a threshold).
    ///
    /// Safe to call while the store is open.  Acquires the write lock so no
    /// commits are interleaved during the snapshot + truncate sequence.
    pub fn compact(&self) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }

        let _write_guard = self.write_lock.lock().unwrap();

        // 1. Fsync any buffered WAL writes so the snapshot is consistent.
        self.wal.lock().unwrap().fsync()?;

        // 2. Snapshot current in-memory version stores.
        //    commit_count=0 and wal_offset=0 because the WAL is about to be
        //    truncated; from the perspective of a future open(), the snapshot
        //    covers "0 WAL frames from offset 0" — all state comes from the
        //    snapshot directly, and any WAL frames present are the delta.
        let snapshot = {
            let ovs = self.object_versions.lock().unwrap().clone();
            let evs = self.edge_versions.lock().unwrap().clone();
            let ov_pos = self.head_object_positions.lock().unwrap().clone();
            let ev_pos = self.head_edge_positions.lock().unwrap().clone();
            VersionSnapshot {
                commit_count: 0,
                wal_offset: 0,
                object_versions: ovs,
                edge_versions: evs,
                ov_positions: ov_pos,
                ev_positions: ev_pos,
                // Compacted snapshots omit commit history; scan_commits after
                // reopen returns only post-compact WAL frames.
                commits: Vec::new(),
            }
        };
        write_snapshot(&self.store_path.join("store.snap"), &snapshot)?;

        // 3. Truncate WAL — all frames are now captured in the snapshot.
        self.wal.lock().unwrap().truncate_to_start()?;

        // 4. Write checkpoint pointing at the new (empty) WAL start.
        //    commit_count = 0 because the trimmed WAL has zero frames before
        //    offset 0.  The snapshot holds all historical state.
        self.checkpoint_log
            .lock()
            .unwrap()
            .write(CheckpointRecord {
                wal_offset: 0,
                commit_count: 0,
            })?;

        // 5. Drop in-memory commit history — versions live in the snapshot.
        //    scan_commits() after compact returns only post-compact frames.
        self.commits.lock().unwrap().clear();
        *self.wal_commit_offset.lock().unwrap() = 0;

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

    // ── full-text search projection API ───────────────────────────────────────

    /// Register searchable text for an object.  The text is stored in a
    /// non-canonical in-memory projection — it is NOT written to the WAL and
    /// is not rebuilt automatically on open.  Callers are responsible for
    /// re-indexing after `BinderStore::open()`.
    ///
    /// Calling `index_text` with the same `object_id` replaces the previous
    /// entry.  Passing an empty string is equivalent to calling
    /// `clear_text_index`.
    pub fn index_text(
        &self,
        object_id: &ObjectId,
        text: impl Into<String>,
    ) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let text: String = text.into();
        let mut idx = self.text_index.lock().unwrap();
        if text.is_empty() {
            idx.remove(object_id);
        } else {
            idx.insert(object_id.clone(), text);
        }
        Ok(())
    }

    /// Case-insensitive substring search over the text projection.
    /// Returns the `ObjectId`s of all objects whose indexed text contains
    /// `query` as a substring (Unicode-aware lowercase comparison).
    ///
    /// Complexity: O(n · m) where n = number of indexed objects and m = avg
    /// text length.  Intended for small-to-medium datasets (<100 K entries).
    pub fn text_search(&self, query: &str) -> Result<Vec<ObjectId>, StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        let q = query.to_lowercase();
        let idx = self.text_index.lock().unwrap();
        Ok(idx
            .iter()
            .filter(|(_, text)| text.to_lowercase().contains(&q))
            .map(|(oid, _)| oid.clone())
            .collect())
    }

    /// Remove an object's entry from the text projection.  No-op if the
    /// object was never indexed.
    pub fn clear_text_index(&self, object_id: &ObjectId) -> Result<(), StoreError> {
        if *self.closed.lock().unwrap() {
            return Err(StoreError::Closed);
        }
        self.text_index.lock().unwrap().remove(object_id);
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

        // Write checkpoint + version store snapshot so next open can skip
        // full WAL replay and load the pre-built stores directly.
        //
        // commit_count is WAL-relative: the number of commits present in the
        // WAL file up to wal_offset.  After compact(), wal_commit_offset > 0
        // because the WAL was trimmed; new commits begin at WAL offset 0 but
        // self.commits still holds all historical commits in memory.
        let total_commits = self.commits.lock().unwrap().len() as u64;
        let wal_commit_offset = *self.wal_commit_offset.lock().unwrap();
        let wal_relative_count = total_commits - wal_commit_offset;
        let wal_offset = wal.current_offset();
        drop(wal);

        self.checkpoint_log
            .lock()
            .unwrap()
            .write(CheckpointRecord {
                wal_offset,
                commit_count: wal_relative_count,
            })?;

        // Snapshot: serialize the version Vecs, position maps, and commit log.
        // On next open() this skips per-frame WAL deserialization for history.
        let snapshot = {
            let ovs = self.object_versions.lock().unwrap().clone();
            let evs = self.edge_versions.lock().unwrap().clone();
            let ov_pos = self.head_object_positions.lock().unwrap().clone();
            let ev_pos = self.head_edge_positions.lock().unwrap().clone();
            // Persist only WAL-relative commits (post-compact history is empty).
            let commits = {
                let all = self.commits.lock().unwrap();
                let start = (wal_commit_offset as usize).min(all.len());
                all[start..].to_vec()
            };
            VersionSnapshot {
                commit_count: wal_relative_count,
                wal_offset,
                object_versions: ovs,
                edge_versions: evs,
                ov_positions: ov_pos,
                ev_positions: ev_pos,
                commits,
            }
        };
        // Non-fatal: if snapshot write fails, next open falls back to WAL replay.
        let _ = write_snapshot(&self.store_path.join("store.snap"), &snapshot);

        // Persist vector projection so it survives a clean restart.
        save_vector_projection(&self.store_path, &self.vector_index.lock().unwrap());

        // Flush the disk-backed version store (non-fatal).
        let _ = self.version_store.lock().unwrap().flush();

        if self.dirty_marker.exists() {
            fs::remove_file(&self.dirty_marker)?;
        }

        Ok(())
    }
}

// ── private helpers ────────────────────────────────────────────────────────────

use crate::edge::EdgeVersionId;
use crate::object::ObjectVersionId;

type VersionStores = (
    Vec<BinderObjectVersion>,
    Vec<BinderEdgeVersion>,
    HashMap<ObjectId, usize>,
    HashMap<EdgeId, usize>,
);

fn head_from_versions(ovs: &[BinderObjectVersion], evs: &[BinderEdgeVersion]) -> Option<CommitId> {
    ovs.iter()
        .map(|v| v.commit_id)
        .chain(evs.iter().map(|v| v.commit_id))
        .max()
}

/// Snapshot-based open: start from the snapshot's version stores and apply
/// only the delta commits (WAL frames written after the snapshot offset).
/// Returns live-only Vecs (retired entries are swap-removed, tombstones excluded).
fn build_version_stores_delta(
    delta_commits: &[BinderCommit],
    ovs: Vec<BinderObjectVersion>,
    evs: Vec<BinderEdgeVersion>,
    _ov_pos: HashMap<ObjectId, usize>, // ignored — positions rebuilt from filtered vecs
    _ev_pos: HashMap<EdgeId, usize>,
) -> VersionStores {
    // Filter incoming snapshot vecs to live-only (backward compat: old snapshots had retired entries).
    let mut ovs: Vec<BinderObjectVersion> =
        ovs.into_iter().filter(|v| v.valid_to.is_none()).collect();
    let mut evs: Vec<BinderEdgeVersion> =
        evs.into_iter().filter(|v| v.valid_to.is_none()).collect();
    let mut ov_pos: HashMap<ObjectId, usize> = ovs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.object_id.clone(), i))
        .collect();
    let mut ev_pos: HashMap<EdgeId, usize> = evs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.edge_id.clone(), i))
        .collect();

    for commit in delta_commits {
        let new_ovs = derive_object_versions_for_commit(commit);
        let new_evs = derive_edge_versions_for_commit(commit);

        // Swap-remove retiring object entries.
        for v in &new_ovs {
            if let Some(old_idx) = ov_pos.remove(&v.object_id) {
                let last = ovs.len() - 1;
                if old_idx != last {
                    ovs.swap(old_idx, last);
                    let moved = ovs[old_idx].object_id.clone();
                    ov_pos.insert(moved, old_idx);
                }
                ovs.pop();
            }
        }
        // Append live new versions (skip tombstones).
        let base = ovs.len();
        let mut live_i = 0usize;
        ovs.reserve(new_ovs.len());
        ov_pos.reserve(new_ovs.len());
        for v in &new_ovs {
            if !v.is_tombstone() {
                ovs.push(v.clone());
                ov_pos.insert(v.object_id.clone(), base + live_i);
                live_i += 1;
            }
        }

        // Swap-remove retiring edge entries.
        for v in &new_evs {
            if let Some(old_idx) = ev_pos.remove(&v.edge_id) {
                let last = evs.len() - 1;
                if old_idx != last {
                    evs.swap(old_idx, last);
                    let moved = evs[old_idx].edge_id.clone();
                    ev_pos.insert(moved, old_idx);
                }
                evs.pop();
            }
        }
        // Append live new edge versions (skip tombstones).
        let base = evs.len();
        let mut live_i = 0usize;
        evs.reserve(new_evs.len());
        ev_pos.reserve(new_evs.len());
        for v in &new_evs {
            if !v.is_tombstone() {
                evs.push(v.clone());
                ev_pos.insert(v.edge_id.clone(), base + live_i);
                live_i += 1;
            }
        }
    }

    (ovs, evs, ov_pos, ev_pos)
}

/// Compact a full-history Vec (live + retired) to live-only.
/// Returns the compacted vecs and rebuilt position maps.
fn compact_live_only_from_full(
    full_ovs: &[BinderObjectVersion],
    full_evs: &[BinderEdgeVersion],
) -> VersionStores {
    let ovs: Vec<BinderObjectVersion> = full_ovs
        .iter()
        .filter(|v| v.valid_to.is_none())
        .cloned()
        .collect();
    let evs: Vec<BinderEdgeVersion> = full_evs
        .iter()
        .filter(|v| v.valid_to.is_none())
        .cloned()
        .collect();
    let ov_pos: HashMap<ObjectId, usize> = ovs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.object_id.clone(), i))
        .collect();
    let ev_pos: HashMap<EdgeId, usize> = evs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.edge_id.clone(), i))
        .collect();
    (ovs, evs, ov_pos, ev_pos)
}

/// Apply one commit's changes to a VersionStore idempotently.
/// Skips versions that are already in the store (by valid_from == commit_id).
fn apply_commit_to_version_store(vs: &mut VersionStore, commit: &BinderCommit) {
    for v in derive_object_versions_for_commit(commit) {
        if !vs.object_has_version_at(&v.object_id, v.valid_from) {
            let _ = vs.retire_object(&v.object_id, commit.commit_id);
            let _ = vs.append_object(&v);
        }
    }
    for v in derive_edge_versions_for_commit(commit) {
        if !vs.edge_has_version_at(&v.edge_id, v.valid_from) {
            let _ = vs.retire_edge(&v.edge_id, commit.commit_id);
            let _ = vs.append_edge(&v);
        }
    }
}

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

/// Replay all commits into version Vecs and position indexes.
/// O(n_commits) — uses position maps for O(1) retirement per object/edge.
fn build_version_stores(commits: &[BinderCommit]) -> VersionStores {
    // Pre-size to commits.len() — each commit produces ≥1 version entry,
    // so this avoids all reallocation in the common single-object-per-commit case.
    let n = commits.len();
    let mut ovs: Vec<BinderObjectVersion> = Vec::with_capacity(n);
    let mut evs: Vec<BinderEdgeVersion> = Vec::with_capacity(n);
    let mut ov_pos: HashMap<ObjectId, usize> = HashMap::with_capacity(n);
    let mut ev_pos: HashMap<EdgeId, usize> = HashMap::with_capacity(n);

    for commit in commits {
        let new_ovs = derive_object_versions_for_commit(commit);
        let new_evs = derive_edge_versions_for_commit(commit);

        for v in &new_ovs {
            if let Some(&old_idx) = ov_pos.get(&v.object_id) {
                ovs[old_idx].valid_to = Some(commit.commit_id);
            }
        }
        let base = ovs.len();
        ovs.extend(new_ovs.iter().cloned());
        for (i, v) in new_ovs.iter().enumerate() {
            ov_pos.insert(v.object_id.clone(), base + i);
        }

        for v in &new_evs {
            if let Some(&old_idx) = ev_pos.get(&v.edge_id) {
                evs[old_idx].valid_to = Some(commit.commit_id);
            }
        }
        let base = evs.len();
        evs.extend(new_evs.iter().cloned());
        for (i, v) in new_evs.iter().enumerate() {
            ev_pos.insert(v.edge_id.clone(), base + i);
        }
    }

    (ovs, evs, ov_pos, ev_pos)
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
            payload_stripped: false,
        })
        .collect()
}

/// Attempt to decode `payload` as an f64 using LE bytes, bincode, or UTF-8.
fn try_decode_f64(payload: &[u8]) -> Option<f64> {
    if payload.len() == 8 {
        if let Ok(arr) = <[u8; 8]>::try_from(payload) {
            return Some(f64::from_le_bytes(arr));
        }
    }
    if let Ok(v) = bincode::deserialize::<f64>(payload) {
        return Some(v);
    }
    std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
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
            payload_stripped: false,
        })
        .collect()
}
