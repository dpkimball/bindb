# bindb — Index System

All indexes in bindb are **non-canonical projections** of the WAL commit log.
They can be dropped and rebuilt at any time without data loss (I-7, I-8).
This document describes each index, its structure, and its rebuild contract.

---

## Index map

```mermaid
graph LR
    WAL["WAL\n(canonical)"]

    subgraph "Version stores (full history)"
        OV["object_versions\nVec&lt;BinderObjectVersion&gt;"]
        EV["edge_versions\nVec&lt;BinderEdgeVersion&gt;"]
    end

    subgraph "HEAD indexes (current state)"
        HO["head_objects\nHashMap&lt;ObjectId, BinderObjectVersion&gt;"]
        HE["head_edges\nHashMap&lt;EdgeId, BinderEdgeVersion&gt;"]
    end

    subgraph "Graph indexes"
        GR["graph\nEdgeId adjacency sets"]
        NB["neighbours\nHashMap&lt;ObjectId, Vec&lt;ObjectId&gt;&gt;"]
    end

    subgraph "Secondary indexes"
        NI["namespace_index\nHashMap&lt;scheme, HashSet&lt;ObjectId&gt;&gt;"]
        VI["vector_index\nHashMap&lt;object_id, Vec&lt;VectorEntry&gt;&gt;"]
    end

    WAL --> OV & EV
    OV --> HO & NI
    EV --> HE & GR & NB
```

---

## Version stores

### `object_versions: Vec<BinderObjectVersion>`

The complete, immutable history of every object version ever written, in commit order.
Each `BinderObjectVersion` records:

- `object_id` — the object it belongs to
- `valid_from` — CommitId when this version became current
- `valid_to` — CommitId when superseded (`None` = still current)
- `payload` — serialized content (`None` = tombstone)
- `provenance` — optional source attribution

Used for `object_history()` and `object_as_of()` queries.

### `edge_versions: Vec<BinderEdgeVersion>`

Same structure for edges.  Used for `edge_history()` and `edge_as_of()`.

---

## HEAD indexes

### `head_objects: HeadObjectIndex`

```
HashMap<ObjectId, BinderObjectVersion>
```

Maps each ObjectId to its most recent non-tombstone version.  Updated on every commit.
Enables O(1) `get()`.

Rebuilt by scanning `object_versions` in order, keeping the last version per ObjectId
where `valid_to.is_none()`.

### `head_edges: HeadEdgeIndex`

```
HashMap<EdgeId, BinderEdgeVersion>
```

Same structure for edges.

---

## Graph indexes

### `graph: GraphIndex`

Adjacency sets:

```
HashMap<ObjectId, HashSet<EdgeId>>  ← outgoing edges
HashMap<ObjectId, HashSet<EdgeId>>  ← incoming edges
```

Enables `edges_from()`, `edges_to()`, and `edges_from_with_type()` in O(degree) time.

### `neighbours: HashMap<ObjectId, Vec<ObjectId>>`

Flat neighbour list derived from live edges.  Used by `traverse()` for BFS without
re-dereferencing the graph index on every step.

Both graph indexes are rebuilt by iterating `edge_versions` and keeping only
non-tombstone current edges.

---

## Namespace index

### `namespace_index: NamespaceObjectIndex`

```
HashMap<String, HashSet<ObjectId>>
```

The map key is the URI scheme prefix — everything up to and including `"://"`.

```
"memory://"  →  { "memory://mem.abc", "memory://mem.xyz", ... }
"obs://"     →  { "obs://health.sleep_duration_h:withings", ... }
```

Turns `list("memory://")` from O(n_all_versions_ever) into O(k_live_in_namespace).

### Lookup algorithm

1. **Fast path** — exact scheme match.  If the prefix is `"memory://"` and the index has
   that key, return the set directly.
2. **Slow path** — sub-prefix query (e.g. `"memory://mem.0"`).  Extract the scheme
   (`"memory://"`), look up that bucket, filter by `starts_with(prefix)`.
3. **Fallback** — prefix has no `"://"`.  Full scan across all buckets.

The fast path covers the vast majority of production queries.

### Maintenance

- `insert(object_id)` — called in `apply_commit_to_memory()` when a live object is written.
- `remove(object_id)` — called when an object is tombstoned or superseded.
- Empty scheme buckets are removed to keep memory stable.

Rebuilt by iterating `head_objects.all_live()` (the current live HEAD set) rather than
the full version history.

---

## Vector index

### `vector_index: VectorIndex`

```
HashMap<String, Vec<VectorEntry>>
```

Keyed by `object_id`.  Each `VectorEntry` stores:

- `chunk_id` / `chunk_index` — position within a chunked document
- `model_id` — embedding model (e.g. `"text-embedding-3-small"`)
- `dimensions` — vector length
- `vector: Vec<f32>` — the embedding
- `content_hash` — for deduplication
- `source_text` — original text for result display
- Metadata: `binder_id`, `app_id`, `namespace`

Unlike other indexes, the vector index is **not rebuilt from the WAL**.  Embeddings are
recomputed externally and inserted via `store.insert_vector()`.  The index is persisted
on clean `close()` as `vector_projection.bin`.

Search uses brute-force cosine similarity; ANN structures are a future optimization.

---

## Rebuild

`rebuild_indexes()` replays all in-memory version stores and reconstructs every index:

```rust
let new_ho = HeadObjectIndex::rebuild(&ovs);
let new_he = HeadEdgeIndex::rebuild(&evs);
let new_gr = GraphIndex::rebuild(live_edges);
let new_nb = GraphIndex::build_neighbour_map(live_edges);
let new_ni = NamespaceObjectIndex::rebuild(new_ho.all_live());
```

Each index rebuilds from the version stores directly — no index may depend on another
index during its rebuild (I-8).

The vector index is excluded from `rebuild_indexes()` because embeddings are not stored
in the WAL.
