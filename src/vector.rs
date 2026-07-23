use crate::vector_hnsw::{dot, HnswGraph, EF_SEARCH, HNSW_THRESHOLD};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    pub object_id: String,
    pub object_version_id: String,
    pub chunk_id: String,
    pub chunk_index: u32,
    pub model_id: String,
    pub dimensions: u32,
    pub vector: Vec<f32>,
    pub content_hash: [u8; 32],
    pub source_text: String,
    /// Optional metadata for filtering.
    pub binder_id: Option<String>,
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VectorFilter {
    pub model_id: Option<String>,
    pub namespace: Option<String>,
    pub object_type: Option<String>,
    pub binder_id: Option<String>,
    pub app_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub object_id: String,
    pub object_version_id: String,
    pub chunk_id: String,
    pub similarity_score: f32,
    pub matched_text: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectionCheckpoint {
    pub generation: u64,
    pub model_id: String,
    pub dimensions: u32,
    pub caught_up_to_commit: Option<String>,
    pub indexed_count: u64,
}

// ── per-model HNSW subindex ───────────────────────────────────────────────────

/// One HNSW graph per (model_id).  All chunks from the same model share a
/// graph; different models have separate graphs because their vector spaces
/// are incomparable.
///
/// Node IDs are stable indices into `flat_entries`.  Deleted chunks are
/// soft-deleted in both `graph` and `chunk_to_node`.
struct ModelIndex {
    graph: HnswGraph,
    /// Flat vector storage backing the HNSW graph.  Index = node_id.
    flat_vectors: Vec<Vec<f32>>,
    /// Flat entry storage for result reconstruction.  Index = node_id.
    flat_entries: Vec<VectorEntry>,
    /// chunk_id → node_id, for dedup on re-insert.
    chunk_to_node: HashMap<String, usize>,
    /// object_id → [node_ids], for tombstoning all chunks of an object.
    object_to_nodes: HashMap<String, Vec<usize>>,
}

impl ModelIndex {
    fn new() -> Self {
        Self {
            graph: HnswGraph::new(),
            flat_vectors: Vec::new(),
            flat_entries: Vec::new(),
            chunk_to_node: HashMap::new(),
            object_to_nodes: HashMap::new(),
        }
    }

    fn insert(&mut self, entry: VectorEntry) {
        // If this chunk_id already exists, soft-delete the old node.
        if let Some(&old_id) = self.chunk_to_node.get(&entry.chunk_id) {
            self.graph.delete(old_id);
        }
        let node_id = self.flat_vectors.len();
        self.flat_vectors.push(entry.vector.clone());
        self.chunk_to_node.insert(entry.chunk_id.clone(), node_id);
        self.object_to_nodes
            .entry(entry.object_id.clone())
            .or_default()
            .push(node_id);
        self.flat_entries.push(entry);
        self.graph.insert(node_id, &self.flat_vectors);
    }

    fn tombstone_object(&mut self, object_id: &str) {
        if let Some(node_ids) = self.object_to_nodes.remove(object_id) {
            for id in node_ids {
                self.graph.delete(id);
            }
        }
    }

    fn live_count(&self) -> usize {
        self.graph.live_count()
    }
}

// ── VectorIndex ───────────────────────────────────────────────────────────────

/// In-memory vector index. Rebuildable projection — not backed by WAL.
///
/// Automatically switches between brute-force (< HNSW_THRESHOLD entries)
/// and HNSW ANN (≥ HNSW_THRESHOLD) per model bucket.
pub struct VectorIndex {
    /// Existing per-object map: preserved for object-level API surface and
    /// for the brute-force path.  object_id → chunks.
    entries: HashMap<String, Vec<VectorEntry>>,
    /// Per-model HNSW sub-indexes.
    model_indexes: HashMap<String, ModelIndex>,
    checkpoint: ProjectionCheckpoint,
}

impl VectorIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            model_indexes: HashMap::new(),
            checkpoint: ProjectionCheckpoint::default(),
        }
    }

    pub fn insert(&mut self, entry: VectorEntry) {
        // Update per-object map (for brute-force fallback + object API).
        let slots = self.entries.entry(entry.object_id.clone()).or_default();
        if let Some(pos) = slots.iter().position(|e| e.chunk_id == entry.chunk_id) {
            slots[pos] = entry.clone();
        } else {
            slots.push(entry.clone());
        }

        // Update HNSW model sub-index.
        let midx = self
            .model_indexes
            .entry(entry.model_id.clone())
            .or_insert_with(ModelIndex::new);
        midx.insert(entry);
    }

    pub fn tombstone_object(&mut self, object_id: &str) {
        self.entries.remove(object_id);
        for midx in self.model_indexes.values_mut() {
            midx.tombstone_object(object_id);
        }
    }

    pub fn entries_for_object(&self, object_id: &str) -> Vec<VectorEntry> {
        self.entries.get(object_id).cloned().unwrap_or_default()
    }

    pub fn all_entries(&self) -> impl Iterator<Item = &VectorEntry> {
        self.entries.values().flat_map(|v| v.iter())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.model_indexes.clear();
        self.checkpoint.generation += 1;
    }

    pub fn checkpoint(&self) -> &ProjectionCheckpoint {
        &self.checkpoint
    }

    pub fn update_checkpoint(&mut self, checkpoint: ProjectionCheckpoint) {
        self.checkpoint = checkpoint;
    }

    /// Nearest-neighbor search.
    ///
    /// Uses HNSW when the relevant model bucket has ≥ HNSW_THRESHOLD live
    /// entries; falls back to brute-force cosine scan otherwise.
    pub fn knn_search(
        &self,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Vec<VectorSearchResult> {
        // Choose the right model bucket when model_id is specified.
        if let Some(mid) = &filter.model_id {
            if let Some(midx) = self.model_indexes.get(mid) {
                if midx.live_count() >= HNSW_THRESHOLD {
                    return self.hnsw_search(midx, query, filter, limit);
                }
            }
        } else {
            // No model filter — check if every model bucket is large enough.
            // If any bucket is large, use HNSW for that bucket; collect results
            // across all buckets and merge.  Buckets below threshold use brute force.
            let any_large = self
                .model_indexes
                .values()
                .any(|m| m.live_count() >= HNSW_THRESHOLD);
            if any_large {
                return self.cross_model_search(query, filter, limit);
            }
        }
        // Brute-force fallback.
        self.brute_force_search(query, filter, limit)
    }

    // ── search implementations ────────────────────────────────────────────────

    fn hnsw_search(
        &self,
        midx: &ModelIndex,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Vec<VectorSearchResult> {
        // Overfetch to account for post-filter losses (namespace, binder_id, app_id).
        let ef = (EF_SEARCH.max(limit * 4)).min(midx.live_count());
        let candidates = midx
            .graph
            .search(&midx.flat_vectors, query, ef, EF_SEARCH.max(limit * 4));

        let mut results: Vec<VectorSearchResult> = candidates
            .into_iter()
            .filter_map(|(score, node_id)| {
                let entry = midx.flat_entries.get(node_id)?;
                if !passes_filter(entry, filter) {
                    return None;
                }
                Some(VectorSearchResult {
                    object_id: entry.object_id.clone(),
                    object_version_id: entry.object_version_id.clone(),
                    chunk_id: entry.chunk_id.clone(),
                    similarity_score: score,
                    matched_text: entry.source_text.clone(),
                    model_id: entry.model_id.clone(),
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });
        results.truncate(limit);
        results
    }

    fn cross_model_search(
        &self,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Vec<VectorSearchResult> {
        let mut all: Vec<VectorSearchResult> = Vec::new();
        for (model_id, midx) in &self.model_indexes {
            // Skip model buckets that don't match the filter (if any).
            if filter.model_id.as_deref().is_some_and(|m| m != model_id) {
                continue;
            }
            if midx.live_count() >= HNSW_THRESHOLD {
                all.extend(self.hnsw_search(midx, query, filter, limit));
            } else {
                // Brute-force only within this model bucket.
                let bucket_filter = VectorFilter {
                    model_id: Some(model_id.clone()),
                    ..filter.clone()
                };
                all.extend(self.brute_force_search(query, &bucket_filter, limit));
            }
        }
        all.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.object_id.cmp(&b.object_id))
        });
        all.truncate(limit);
        all
    }

    fn brute_force_search(
        &self,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Vec<VectorSearchResult> {
        let mut scored: Vec<(f32, &VectorEntry)> = self
            .all_entries()
            .filter(|e| passes_filter(e, filter))
            .map(|e| (dot(query, &e.vector), e))
            .collect();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.object_id.cmp(&b.1.object_id))
        });
        scored.truncate(limit);

        scored
            .into_iter()
            .map(|(score, e)| VectorSearchResult {
                object_id: e.object_id.clone(),
                object_version_id: e.object_version_id.clone(),
                chunk_id: e.chunk_id.clone(),
                similarity_score: score,
                matched_text: e.source_text.clone(),
                model_id: e.model_id.clone(),
            })
            .collect()
    }
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ── persistence helpers ────────────────────────────────────────────────────────

/// Snapshot format for persisting the vector projection on clean close.
#[derive(Serialize, Deserialize)]
struct VectorProjectionSnapshot {
    entries: Vec<VectorEntry>,
    checkpoint: ProjectionCheckpoint,
}

/// Serialize the entire vector index to bytes (bincode).
pub(crate) fn serialize_projection(
    index: &VectorIndex,
) -> Result<Vec<u8>, Box<bincode::ErrorKind>> {
    let snapshot = VectorProjectionSnapshot {
        entries: index.all_entries().cloned().collect(),
        checkpoint: index.checkpoint().clone(),
    };
    bincode::serialize(&snapshot)
}

/// Deserialize a previously saved projection. Returns `(entries, checkpoint)`.
/// Returns `None` if the data is corrupt (caller starts fresh).
pub(crate) fn deserialize_projection(
    data: &[u8],
) -> Option<(Vec<VectorEntry>, ProjectionCheckpoint)> {
    bincode::deserialize::<VectorProjectionSnapshot>(data)
        .ok()
        .map(|s| (s.entries, s.checkpoint))
}

// ── helpers ────────────────────────────────────────────────────────────────────

fn passes_filter(entry: &VectorEntry, filter: &VectorFilter) -> bool {
    if let Some(mid) = &filter.model_id {
        if &entry.model_id != mid {
            return false;
        }
    }
    if let Some(ns) = &filter.namespace {
        if object_id_namespace(&entry.object_id).is_none_or(|n| n != ns) {
            return false;
        }
    }
    if let Some(ot) = &filter.object_type {
        if object_id_type(&entry.object_id).is_none_or(|t| t != ot) {
            return false;
        }
    }
    if let Some(bid) = &filter.binder_id {
        if entry.binder_id.as_deref() != Some(bid.as_str()) {
            return false;
        }
    }
    if let Some(aid) = &filter.app_id {
        if entry.app_id.as_deref() != Some(aid.as_str()) {
            return false;
        }
    }
    true
}

/// Extract namespace from an object_id of the form `namespace://type/id`.
fn object_id_namespace(object_id: &str) -> Option<&str> {
    object_id.split_once("://").map(|(ns, _)| ns)
}

/// Extract object_type from an object_id of the form `namespace://type/id`.
fn object_id_type(object_id: &str) -> Option<&str> {
    object_id.split_once("://")?.1.split('/').next()
}
