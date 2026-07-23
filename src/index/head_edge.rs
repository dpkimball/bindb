#![allow(dead_code)]
use crate::edge::BinderEdgeVersion;
use crate::types::EdgeId;
use std::collections::HashMap;

/// Persistent HEAD index: edge_id → current BinderEdgeVersion.
/// Edge version chain is independent from the object chain (PRINCIPLES I-2).
/// Tombstones are stored; callers check `is_tombstone()` if needed.
/// Rebuilt from the commit log on open; never canonical (PRINCIPLES I-7).
#[derive(Debug, Default)]
pub struct HeadEdgeIndex {
    map: HashMap<EdgeId, BinderEdgeVersion>,
}

impl HeadEdgeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, version: BinderEdgeVersion) {
        self.map.insert(version.edge_id.clone(), version);
    }

    pub fn get(&self, edge_id: &EdgeId) -> Option<&BinderEdgeVersion> {
        self.map.get(edge_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &BinderEdgeVersion> {
        self.map.values()
    }

    pub fn all_live(&self) -> impl Iterator<Item = &BinderEdgeVersion> {
        self.map.values().filter(|v| !v.is_tombstone())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn rebuild(versions: &[BinderEdgeVersion]) -> Self {
        let mut idx = Self::new();
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }
}
