#![allow(dead_code)]
use crate::object::BinderObjectVersion;
use crate::types::ObjectId;
use std::collections::{BTreeMap, HashMap};

/// Default maximum number of live head entries kept in RAM.
/// Below this threshold every get() is a pure O(1) HashMap hit.
/// Above it the oldest-inserted entries are evicted; misses fall back to
/// VersionStore (mmap read, page-cache-backed).
///
/// Chosen so M-tier (10 K) and L-tier (100 K) stores never evict, while
/// XL-tier (1 M) caps memory at ~200 K × ~300 B ≈ 60 MB instead of 300 MB.
pub const DEFAULT_HEAD_CACHE_CAP: usize = 200_000;

/// Bounded LRU-ish head index: object_id → current BinderObjectVersion.
///
/// "Current" means `valid_to = None`.  Tombstones are stored as-is;
/// callers must check `is_tombstone()` to skip deleted objects.
///
/// Eviction policy: insertion-order (FIFO).  When `map.len() > cap` the
/// entry with the smallest insertion generation is evicted.  Frequently-
/// updated objects always advance their generation on each upsert, so they
/// naturally stay warm.
///
/// When `cap == 0` the index is unbounded (original behaviour).
///
/// Rebuilt from the commit log on open; never canonical (PRINCIPLES I-7).
#[derive(Debug)]
pub struct HeadObjectIndex {
    map: HashMap<ObjectId, (BinderObjectVersion, u64)>,
    /// generation → object_id; smallest-key entry is evicted first.
    order: BTreeMap<u64, ObjectId>,
    gen: u64,
    cap: usize,
}

impl Default for HeadObjectIndex {
    fn default() -> Self {
        Self::with_cap(DEFAULT_HEAD_CACHE_CAP)
    }
}

impl HeadObjectIndex {
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

    /// Insert or replace the current version for object_id.
    pub fn upsert(&mut self, version: BinderObjectVersion) {
        self.gen += 1;
        let id = version.object_id.clone();
        // Remove the old order entry so the eviction queue stays compact.
        if let Some((_, old_gen)) = self.map.get(&id) {
            self.order.remove(old_gen);
        }
        self.map.insert(id.clone(), (version, self.gen));
        self.order.insert(self.gen, id);
        self.evict_excess();
    }

    /// Return the current version if it is cached, or None on a cache miss.
    /// Callers must fall back to VersionStore when None is returned and the
    /// object_id exists (i.e., is not simply unknown).
    pub fn get(&self, object_id: &ObjectId) -> Option<&BinderObjectVersion> {
        self.map.get(object_id).map(|(v, _)| v)
    }

    /// All current versions (including tombstones) that are in the cache.
    /// May be incomplete when eviction is active — callers that need all
    /// live objects should iterate over the VersionStore index instead.
    pub fn all(&self) -> impl Iterator<Item = &BinderObjectVersion> {
        self.map.values().map(|(v, _)| v)
    }

    /// All non-tombstone current versions in the cache.
    /// See `all()` for the completeness caveat.
    pub fn all_live(&self) -> impl Iterator<Item = &BinderObjectVersion> {
        self.map.values().filter_map(|(v, _)| {
            if v.is_tombstone() {
                None
            } else {
                Some(v)
            }
        })
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Rebuild the index from a sorted slice of all object versions.
    /// Versions should be in ascending commit order within each object_id
    /// so that the most-recently-committed objects end up with the highest
    /// generations (and therefore stay warm longest after an initial fill).
    pub fn rebuild(versions: &[BinderObjectVersion]) -> Self {
        let mut idx = Self::default();
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }

    pub fn rebuild_with_cap(versions: &[BinderObjectVersion], cap: usize) -> Self {
        let mut idx = Self::with_cap(cap);
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }

    // ── private ───────────────────────────────────────────────────────────────

    fn evict_excess(&mut self) {
        if self.cap == 0 {
            return;
        }
        while self.map.len() > self.cap {
            // Pop the smallest generation (oldest inserted).
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
