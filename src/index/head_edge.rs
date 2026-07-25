#![allow(dead_code)]
use crate::edge::BinderEdgeVersion;
use crate::types::EdgeId;
use std::collections::{BTreeMap, HashMap};

/// Default cap for the edge head cache.  Edges are typically fewer than
/// objects; 200 K covers most real workloads without eviction.
pub const DEFAULT_EDGE_CACHE_CAP: usize = 200_000;

/// Bounded LRU-ish head index: edge_id → current BinderEdgeVersion.
/// Mirrors HeadObjectIndex — see that type for eviction semantics.
///
/// Edge version chain is independent from the object chain (PRINCIPLES I-2).
/// Tombstones are stored; callers check `is_tombstone()` if needed.
/// Rebuilt from the commit log on open; never canonical (PRINCIPLES I-7).
#[derive(Debug)]
pub struct HeadEdgeIndex {
    map: HashMap<EdgeId, (BinderEdgeVersion, u64)>,
    order: BTreeMap<u64, EdgeId>,
    gen: u64,
    cap: usize,
}

impl Default for HeadEdgeIndex {
    fn default() -> Self {
        Self::with_cap(DEFAULT_EDGE_CACHE_CAP)
    }
}

impl HeadEdgeIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cap(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: BTreeMap::new(),
            gen: 0,
            cap,
        }
    }

    pub fn upsert(&mut self, version: BinderEdgeVersion) {
        self.gen += 1;
        let id = version.edge_id.clone();
        if let Some((_, old_gen)) = self.map.get(&id) {
            self.order.remove(old_gen);
        }
        self.map.insert(id.clone(), (version, self.gen));
        self.order.insert(self.gen, id);
        self.evict_excess();
    }

    pub fn get(&self, edge_id: &EdgeId) -> Option<&BinderEdgeVersion> {
        self.map.get(edge_id).map(|(v, _)| v)
    }

    pub fn all(&self) -> impl Iterator<Item = &BinderEdgeVersion> {
        self.map.values().map(|(v, _)| v)
    }

    pub fn all_live(&self) -> impl Iterator<Item = &BinderEdgeVersion> {
        self.map
            .values()
            .filter_map(|(v, _)| if v.is_tombstone() { None } else { Some(v) })
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn rebuild(versions: &[BinderEdgeVersion]) -> Self {
        let mut idx = Self::default();
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }

    pub fn rebuild_with_cap(versions: &[BinderEdgeVersion], cap: usize) -> Self {
        let mut idx = Self::with_cap(cap);
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }

    fn evict_excess(&mut self) {
        if self.cap == 0 {
            return;
        }
        while self.map.len() > self.cap {
            if let Some((&gen, id)) = self.order.iter().next() {
                let id = id.clone();
                self.order.remove(&gen);
                self.map.remove(&id);
            } else {
                break;
            }
        }
    }
}
