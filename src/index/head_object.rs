#![allow(dead_code)]
use crate::object::BinderObjectVersion;
use crate::types::ObjectId;
use std::collections::HashMap;

/// Persistent HEAD index: object_id → current BinderObjectVersion.
/// "Current" means `valid_to = None`. Tombstones are stored as-is — callers
/// must call `is_tombstone()` if they want to skip deleted objects.
/// Rebuilt from the commit log on open; never canonical (PRINCIPLES I-7).
#[derive(Debug, Default)]
pub struct HeadObjectIndex {
    map: HashMap<ObjectId, BinderObjectVersion>,
}

impl HeadObjectIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the current version for object_id.
    pub fn upsert(&mut self, version: BinderObjectVersion) {
        self.map.insert(version.object_id.clone(), version);
    }

    /// Return the current version, or None if the object is unknown.
    pub fn get(&self, object_id: &ObjectId) -> Option<&BinderObjectVersion> {
        self.map.get(object_id)
    }

    /// All current versions (including tombstones).
    pub fn all(&self) -> impl Iterator<Item = &BinderObjectVersion> {
        self.map.values()
    }

    /// All non-tombstone current versions.
    pub fn all_live(&self) -> impl Iterator<Item = &BinderObjectVersion> {
        self.map.values().filter(|v| !v.is_tombstone())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Rebuild the entire index from a sorted slice of all object versions.
    /// Versions must be in ascending commit order within each object_id.
    pub fn rebuild(versions: &[BinderObjectVersion]) -> Self {
        let mut idx = Self::new();
        for v in versions {
            if v.valid_to.is_none() {
                idx.upsert(v.clone());
            }
        }
        idx
    }
}
