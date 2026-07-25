#![allow(clippy::unnecessary_sort_by)]
/// HNSW — Hierarchical Navigable Small World approximate nearest-neighbor index.
///
/// Implements the algorithm from Malkov & Yashunin 2018.
/// Supports incremental insertion and soft deletion.
/// All vectors must be unit-normalised; similarity is the dot product.
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

// ── tunables ──────────────────────────────────────────────────────────────────

/// Max connections per node at layers ≥ 1.
const M: usize = 16;
/// Max connections per node at layer 0 (paper recommends 2 × M).
const M_MAX_0: usize = 32;
/// Beam width during graph construction (higher = better recall, slower insert).
const EF_CONSTRUCTION: usize = 200;
/// Default beam width for queries (higher = better recall, slower search).
/// 128 gives ≥90% recall@10 at 100K vectors / dim=32; well under 5ms p99.
pub const EF_SEARCH: usize = 128;
/// Fall back to brute-force below this many live nodes.
pub const HNSW_THRESHOLD: usize = 1_000;

// ── ordered f32 ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
struct Score(f32);

impl Eq for Score {}
impl Ord for Score {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&o.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}
impl PartialOrd for Score {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

// ── HnswGraph ─────────────────────────────────────────────────────────────────

/// HNSW graph over an externally-owned flat vector slice.
///
/// Node IDs are indices into the caller's `vectors: &[Vec<f32>]`.  The
/// caller must never reuse or compact node IDs; use `delete()` instead.
pub struct HnswGraph {
    /// `connections[id][layer]` = neighbor node IDs at that layer.
    connections: Vec<Vec<Vec<usize>>>,
    /// `levels[id]` = highest layer the node participates in.
    levels: Vec<usize>,
    /// Soft-deleted IDs: present in the graph for connectivity but excluded
    /// from search results.
    deleted: HashSet<usize>,
    entry_point: Option<usize>,
    max_level: usize,
    live: usize,
    /// LCG random state for level assignment.
    rng: u64,
}

impl HnswGraph {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            levels: Vec::new(),
            deleted: HashSet::new(),
            entry_point: None,
            max_level: 0,
            live: 0,
            rng: 0x5851_F42D_4C95_7F2D,
        }
    }

    pub fn live_count(&self) -> usize {
        self.live
    }

    /// Insert `node_id` into the graph.  `vectors[node_id]` must be set
    /// by the caller before this call.
    pub fn insert(&mut self, node_id: usize, vectors: &[Vec<f32>]) {
        let level = self.random_level();

        // Grow the per-node tables.
        if self.connections.len() <= node_id {
            self.connections.resize_with(node_id + 1, Vec::new);
            self.levels.resize(node_id + 1, 0);
        }
        self.connections[node_id] = vec![Vec::new(); level + 1];
        self.levels[node_id] = level;
        self.live += 1;

        let Some(ep) = self.entry_point else {
            self.entry_point = Some(node_id);
            self.max_level = level;
            return;
        };

        let q = &vectors[node_id];
        let mut eps = vec![ep];

        // Greedy descent from max_level down to level+1 (1-nearest each).
        for lc in (level + 1..=self.max_level).rev() {
            let w = self.search_layer(vectors, q, &eps, 1, lc);
            eps = top_n(&w, 1);
        }

        // Build connections level by level from min(level, max_level) → 0.
        for lc in (0..=level.min(self.max_level)).rev() {
            let m_max = m_max_for(lc);
            let w = self.search_layer(vectors, q, &eps, EF_CONSTRUCTION, lc);
            let neighbors = select_neighbors(vectors, q, &w, m_max);

            self.connections[node_id][lc].extend_from_slice(&neighbors);

            for &nb in &neighbors {
                let Some(nb_conn) = self.connections.get_mut(nb) else {
                    continue;
                };
                if nb_conn.len() <= lc {
                    continue;
                }
                let conn = &mut nb_conn[lc];
                if !conn.contains(&node_id) {
                    conn.push(node_id);
                    if conn.len() > m_max {
                        let nb_vec = &vectors[nb];
                        let mut scored: Vec<(Score, usize)> = conn
                            .iter()
                            .filter_map(|&n| vectors.get(n).map(|v| (Score(dot(nb_vec, v)), n)))
                            .collect();
                        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));
                        *conn = scored.into_iter().take(m_max).map(|(_, n)| n).collect();
                    }
                }
            }

            eps = if neighbors.is_empty() {
                vec![ep]
            } else {
                neighbors
            };
        }

        if level > self.max_level {
            self.entry_point = Some(node_id);
            self.max_level = level;
        }
    }

    /// Soft-delete `node_id`.  Excluded from future search results; the node
    /// remains in the graph to preserve connectivity.
    pub fn delete(&mut self, node_id: usize) {
        if self.deleted.insert(node_id) && self.live > 0 {
            self.live -= 1;
        }
    }

    #[allow(dead_code)]
    pub fn is_deleted(&self, node_id: usize) -> bool {
        self.deleted.contains(&node_id)
    }

    /// Search for the `k` nearest non-deleted nodes to `query`.
    /// `ef` controls the beam width (≥ k; higher = better recall).
    /// Returns `(similarity_score, node_id)` sorted by descending score.
    pub fn search(
        &self,
        vectors: &[Vec<f32>],
        query: &[f32],
        k: usize,
        ef: usize,
    ) -> Vec<(f32, usize)> {
        let Some(ep) = self.entry_point else {
            return Vec::new();
        };
        if vectors.is_empty() || k == 0 {
            return Vec::new();
        }

        let mut eps = vec![ep];

        // Greedy descent from top layer to layer 1.
        for lc in (1..=self.max_level).rev() {
            let w = self.search_layer(vectors, query, &eps, 1, lc);
            eps = top_n(&w, 1);
            if eps.is_empty() {
                eps = vec![ep];
            }
        }

        // Beam search at layer 0.
        let w = self.search_layer(vectors, query, &eps, ef.max(k), 0);

        let mut results: Vec<(f32, usize)> = w
            .into_iter()
            .filter(|(_, id)| !self.deleted.contains(id))
            .map(|(s, id)| (s.0, id))
            .collect();
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    // ── private ───────────────────────────────────────────────────────────────

    fn random_level(&mut self) -> usize {
        self.rng = self
            .rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let r = (self.rng >> 33) as f64 / u32::MAX as f64;
        ((-r.ln()) / (M as f64).ln()) as usize
    }

    /// SEARCH-LAYER: returns all candidates found at `layer` with beam width `ef`.
    fn search_layer(
        &self,
        vectors: &[Vec<f32>],
        query: &[f32],
        entry_points: &[usize],
        ef: usize,
        layer: usize,
    ) -> Vec<(Score, usize)> {
        let mut visited: HashSet<usize> = HashSet::new();
        // max-heap by score: pop = nearest candidate to expand next
        let mut candidates: BinaryHeap<(Score, usize)> = BinaryHeap::new();
        // min-heap by score: pop = furthest result (evict when |W| > ef)
        let mut results: BinaryHeap<Reverse<(Score, usize)>> = BinaryHeap::new();

        for &ep in entry_points {
            if ep >= vectors.len() {
                continue;
            }
            let sim = Score(dot(query, &vectors[ep]));
            visited.insert(ep);
            candidates.push((sim, ep));
            results.push(Reverse((sim, ep)));
        }

        while let Some(&(c_sim, c_id)) = candidates.peek() {
            let f_sim = results
                .peek()
                .map(|Reverse((s, _))| *s)
                .unwrap_or(Score(f32::NEG_INFINITY));
            // All remaining candidates are worse than the worst result — done.
            if c_sim < f_sim && results.len() >= ef {
                break;
            }
            candidates.pop();

            let conns = match self.connections.get(c_id) {
                Some(c) if c.len() > layer => &c[layer],
                _ => continue,
            };

            for &nb in conns {
                if visited.contains(&nb) || nb >= vectors.len() {
                    continue;
                }
                visited.insert(nb);
                let sim = Score(dot(query, &vectors[nb]));
                let f_sim2 = results
                    .peek()
                    .map(|Reverse((s, _))| *s)
                    .unwrap_or(Score(f32::NEG_INFINITY));
                if results.len() < ef || sim > f_sim2 {
                    candidates.push((sim, nb));
                    results.push(Reverse((sim, nb)));
                    if results.len() > ef {
                        results.pop();
                    }
                }
            }
        }

        results.into_iter().map(|Reverse(pair)| pair).collect()
    }
}

// ── free helpers ──────────────────────────────────────────────────────────────

fn m_max_for(layer: usize) -> usize {
    if layer == 0 {
        M_MAX_0
    } else {
        M
    }
}

fn top_n(candidates: &[(Score, usize)], n: usize) -> Vec<usize> {
    let mut v = candidates.to_vec();
    v.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(n).map(|(_, id)| id).collect()
}

fn select_neighbors(
    vectors: &[Vec<f32>],
    _q: &[f32],
    candidates: &[(Score, usize)],
    m: usize,
) -> Vec<usize> {
    let mut v = candidates.to_vec();
    v.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    v.retain(|(_, id)| *id < vectors.len());
    v.into_iter().take(m).map(|(_, id)| id).collect()
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
