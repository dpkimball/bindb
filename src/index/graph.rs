#![allow(dead_code)]
use crate::edge::BinderEdgeVersion;
use crate::types::{EdgeId, ObjectId};
use std::collections::{HashMap, HashSet, VecDeque};

/// Graph lookup indexes: from, to, type, and pair.
/// All four indexes are derived from live (non-tombstone) edge HEAD state.
/// Rebuilt from edge versions on open; never canonical (PRINCIPLES I-7).
/// Each index is independent — no index depends on another (PRINCIPLES I-8).
#[derive(Debug, Default)]
pub struct GraphIndex {
    /// from_object → [edge_id, ...]
    from_index: HashMap<ObjectId, Vec<EdgeId>>,
    /// to_object → [edge_id, ...]
    to_index: HashMap<ObjectId, Vec<EdgeId>>,
    /// relationship_type → [edge_id, ...]
    type_index: HashMap<String, Vec<EdgeId>>,
    /// (from, to) → [edge_id, ...]
    pair_index: HashMap<(ObjectId, ObjectId), Vec<EdgeId>>,
}

impl GraphIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a live edge into all four sub-indexes. Skip tombstones.
    pub fn insert(&mut self, edge: &BinderEdgeVersion) {
        let from = match edge.from_object.clone() {
            Some(f) => f,
            None => return,
        };
        let to = match edge.to_object.clone() {
            Some(t) => t,
            None => return,
        };
        let rel = match edge.relationship_type.clone() {
            Some(r) => r,
            None => return,
        };

        let eid = edge.edge_id.clone();
        self.from_index
            .entry(from.clone())
            .or_default()
            .push(eid.clone());
        self.to_index
            .entry(to.clone())
            .or_default()
            .push(eid.clone());
        self.type_index.entry(rel).or_default().push(eid.clone());
        self.pair_index.entry((from, to)).or_default().push(eid);
    }

    /// Remove an edge from all four sub-indexes.
    pub fn remove(&mut self, edge: &BinderEdgeVersion) {
        let eid = &edge.edge_id;
        if let Some(from) = &edge.from_object {
            if let Some(v) = self.from_index.get_mut(from) {
                v.retain(|e| e != eid);
            }
        }
        if let Some(to) = &edge.to_object {
            if let Some(v) = self.to_index.get_mut(to) {
                v.retain(|e| e != eid);
            }
        }
        if let Some(rel) = &edge.relationship_type {
            if let Some(v) = self.type_index.get_mut(rel) {
                v.retain(|e| e != eid);
            }
        }
        if let (Some(from), Some(to)) = (&edge.from_object, &edge.to_object) {
            let key = (from.clone(), to.clone());
            if let Some(v) = self.pair_index.get_mut(&key) {
                v.retain(|e| e != eid);
            }
        }
    }

    // ── query API ─────────────────────────────────────────────────────────────

    pub fn edges_from<'a>(&'a self, from: &ObjectId) -> impl Iterator<Item = &'a EdgeId> {
        self.from_index
            .get(from)
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
    }

    pub fn edges_to<'a>(&'a self, to: &ObjectId) -> impl Iterator<Item = &'a EdgeId> {
        self.to_index
            .get(to)
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
    }

    /// Returns EdgeIds (owned) that originate from `from` and have `rel_type`.
    /// Returns owned values because the intersection must be collected to escape
    /// the borrow of both sub-indexes simultaneously.
    pub fn edges_from_with_type(
        &self,
        from: &ObjectId,
        rel_type: &str,
    ) -> impl Iterator<Item = EdgeId> {
        let from_edges: HashSet<&EdgeId> = self
            .from_index
            .get(from)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        let type_edges: HashSet<&EdgeId> = self
            .type_index
            .get(rel_type)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        from_edges
            .intersection(&type_edges)
            .map(|eid| (*eid).clone())
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn edges_between<'a>(
        &'a self,
        from: &ObjectId,
        to: &ObjectId,
    ) -> impl Iterator<Item = &'a EdgeId> {
        self.pair_index
            .get(&(from.clone(), to.clone()))
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
    }

    /// BFS over the neighbour map: all live objects reachable from `start`
    /// within `max_depth` hops. `is_alive` prunes tombstoned objects.
    /// The neighbour map is built from live edges at rebuild time and is kept
    /// separate so traversal doesn't cross-depend on HeadObjectIndex (I-8).
    pub fn traverse_outgoing_with_neighbours(
        neighbours: &HashMap<ObjectId, Vec<ObjectId>>,
        start: &ObjectId,
        max_depth: usize,
        is_alive: impl Fn(&ObjectId) -> bool,
    ) -> Vec<ObjectId> {
        let mut visited: HashSet<ObjectId> = HashSet::new();
        visited.insert(start.clone());

        let mut queue: VecDeque<(ObjectId, usize)> = VecDeque::new();
        queue.push_back((start.clone(), 0));

        let mut result: Vec<ObjectId> = Vec::new();

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            if let Some(nexts) = neighbours.get(&current) {
                for next in nexts {
                    if !visited.contains(next) && is_alive(next) {
                        visited.insert(next.clone());
                        result.push(next.clone());
                        queue.push_back((next.clone(), depth + 1));
                    }
                }
            }
        }

        result
    }

    /// Rebuild the graph index from scratch from all live edge HEAD versions.
    pub fn rebuild<'a>(live_edges: impl Iterator<Item = &'a BinderEdgeVersion>) -> Self {
        let mut idx = Self::new();
        for edge in live_edges {
            idx.insert(edge);
        }
        idx
    }

    /// Build a neighbour map: from_object → [to_object, ...] for all live edges.
    pub fn build_neighbour_map<'a>(
        live_edges: impl Iterator<Item = &'a BinderEdgeVersion>,
    ) -> HashMap<ObjectId, Vec<ObjectId>> {
        let mut map: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for edge in live_edges {
            if let (Some(from), Some(to)) = (&edge.from_object, &edge.to_object) {
                map.entry(from.clone()).or_default().push(to.clone());
            }
        }
        map
    }
}
