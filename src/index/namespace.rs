use crate::object::BinderObjectVersion;
use crate::types::ObjectId;
use std::collections::{HashMap, HashSet};

/// Secondary index: namespace scheme → set of live ObjectIds.
///
/// Turns `list("memory://")` from an O(n_total_versions_ever) scan into
/// an O(k) lookup where k is the number of live objects in that namespace.
///
/// The index key is the scheme prefix of the ObjectId — everything up to
/// and including `"://"`.  For `"memory://memory.memory/abc"` the key is
/// `"memory://"`.  For ObjectIds that have no `"://"` the full ObjectId
/// is used as the key (degenerate case, not expected in normal usage).
///
/// Non-canonical: rebuilt from the HEAD object index; never written to WAL.
pub struct NamespaceObjectIndex {
    index: HashMap<String, HashSet<ObjectId>>,
}

impl NamespaceObjectIndex {
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
        }
    }

    /// Insert a live ObjectId into the index.
    pub fn insert(&mut self, object_id: &ObjectId) {
        let key = scheme_of(object_id);
        self.index.entry(key).or_default().insert(object_id.clone());
    }

    /// Remove an ObjectId (tombstoned or superseded).
    pub fn remove(&mut self, object_id: &ObjectId) {
        let key = scheme_of(object_id);
        if let Some(set) = self.index.get_mut(&key) {
            set.remove(object_id);
            if set.is_empty() {
                self.index.remove(&key);
            }
        }
    }

    /// All live ObjectIds whose scheme matches `prefix` exactly, OR whose
    /// full ObjectId starts with `prefix` (sub-prefix queries).
    ///
    /// Returns a `Vec` so callers own the IDs without holding the index lock.
    pub fn matching(&self, prefix: &str) -> Vec<ObjectId> {
        // Fast path: exact scheme match (the common case, e.g. "memory://").
        if let Some(set) = self.index.get(prefix) {
            return set.iter().cloned().collect();
        }

        // Slow path: prefix is a sub-scheme (e.g. "memory://memory.memory/").
        // Only scan the relevant scheme bucket.
        if let Some(scheme) = scheme_from_prefix(prefix) {
            if let Some(set) = self.index.get(scheme.as_str()) {
                return set
                    .iter()
                    .filter(|oid| oid.0.starts_with(prefix))
                    .cloned()
                    .collect();
            }
        }

        // Fallback: prefix doesn't contain "://" — scan everything.
        self.index
            .values()
            .flatten()
            .filter(|oid| oid.0.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Rebuild from a set of live (non-tombstone) object versions.
    pub fn rebuild<'a>(live: impl Iterator<Item = &'a BinderObjectVersion>) -> Self {
        let mut idx = Self::new();
        for v in live {
            if !v.is_tombstone() {
                idx.insert(&v.object_id);
            }
        }
        idx
    }
}

impl Default for NamespaceObjectIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the scheme key from an ObjectId.
/// `"memory://memory.memory/abc"` → `"memory://"`
fn scheme_of(object_id: &ObjectId) -> String {
    match object_id.0.find("://") {
        Some(i) => object_id.0[..i + 3].to_string(),
        None => object_id.0.clone(),
    }
}

/// Extract the scheme from an arbitrary prefix string.
/// `"memory://memory."` → `Some("memory://")`
fn scheme_from_prefix(prefix: &str) -> Option<String> {
    prefix.find("://").map(|i| prefix[..i + 3].to_string())
}
