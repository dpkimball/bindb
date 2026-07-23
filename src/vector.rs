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

/// In-memory vector index. Rebuildable projection — not backed by WAL.
pub struct VectorIndex {
    entries: HashMap<String, Vec<VectorEntry>>, // keyed by object_id
    checkpoint: ProjectionCheckpoint,
}

impl VectorIndex {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            checkpoint: ProjectionCheckpoint::default(),
        }
    }

    pub fn insert(&mut self, entry: VectorEntry) {
        let slots = self.entries.entry(entry.object_id.clone()).or_default();
        if let Some(pos) = slots.iter().position(|e| e.chunk_id == entry.chunk_id) {
            slots[pos] = entry;
        } else {
            slots.push(entry);
        }
    }

    pub fn tombstone_object(&mut self, object_id: &str) {
        self.entries.remove(object_id);
    }

    pub fn entries_for_object(&self, object_id: &str) -> Vec<VectorEntry> {
        self.entries.get(object_id).cloned().unwrap_or_default()
    }

    pub fn all_entries(&self) -> impl Iterator<Item = &VectorEntry> {
        self.entries.values().flat_map(|v| v.iter())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.checkpoint.generation += 1;
    }

    pub fn checkpoint(&self) -> &ProjectionCheckpoint {
        &self.checkpoint
    }

    pub fn update_checkpoint(&mut self, checkpoint: ProjectionCheckpoint) {
        self.checkpoint = checkpoint;
    }

    /// Brute-force cosine similarity. Assumes unit-normalised vectors.
    pub fn knn_search(
        &self,
        query: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Vec<VectorSearchResult> {
        let mut scored: Vec<(f32, &VectorEntry)> = self
            .all_entries()
            .filter(|e| {
                if let Some(mid) = &filter.model_id {
                    if &e.model_id != mid {
                        return false;
                    }
                }
                if let Some(ns) = &filter.namespace {
                    if object_id_namespace(&e.object_id).is_none_or(|n| n != ns) {
                        return false;
                    }
                }
                if let Some(ot) = &filter.object_type {
                    if object_id_type(&e.object_id).is_none_or(|t| t != ot) {
                        return false;
                    }
                }
                if let Some(bid) = &filter.binder_id {
                    if e.binder_id.as_deref() != Some(bid.as_str()) {
                        return false;
                    }
                }
                if let Some(aid) = &filter.app_id {
                    if e.app_id.as_deref() != Some(aid.as_str()) {
                        return false;
                    }
                }
                true
            })
            .map(|e| (cosine(query, &e.vector), e))
            .collect();

        // Primary: descending score. Secondary: ascending object_id for deterministic ties.
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

/// Extract namespace from an object_id of the form `namespace://type/id`.
fn object_id_namespace(object_id: &str) -> Option<&str> {
    object_id.split_once("://").map(|(ns, _)| ns)
}

/// Extract object_type from an object_id of the form `namespace://type/id`.
fn object_id_type(object_id: &str) -> Option<&str> {
    object_id.split_once("://")?.1.split('/').next()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
