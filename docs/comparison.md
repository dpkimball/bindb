# bindb — Database Comparison

bindb is not a general-purpose database.  It is purpose-built for semantic, graph, temporal,
and AI-native applications where a single person's or organization's complete data history
must be stored, related, searched, and reasoned over without the operational complexity of
running four or five separate database systems.

This document compares bindb against the databases it replaces or complements.

---

## The multi-database problem

A typical semantic application stack requires:

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   MongoDB   │  │    Neo4j    │  │   Weaviate  │  │ ClickHouse  │  │    MySQL    │
│  documents  │  │    graph    │  │   vectors   │  │  analytics  │  │  relations  │
└─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘
       │                │                │                │                │
       └────────────────┴────────────────┴────────────────┴────────────────┘
                                         │
                              Application must join,
                              sync, and reconcile
                              across all five systems
```

Each database is excellent at its primary concern.  The cost is:

- **Cross-domain queries** require application-layer joins across network boundaries
- **Consistency** — a write that touches two databases is not atomic
- **Operational burden** — five systems to deploy, monitor, backup, and upgrade
- **Schema duplication** — the same entity is modeled differently in each store
- **Semantic loss** — relationships between a memory (MongoDB), its graph context (Neo4j),
  its embedding (Weaviate), its behavioral metrics (ClickHouse), and its provenance record
  (MySQL) exist only as implicit foreign keys maintained by application code

bindb replaces this architecture with one embedded engine.

---

## Feature comparison matrix

| Capability | bindb | SQLite | DuckDB | MongoDB | Neo4j | Weaviate | ClickHouse | PostgreSQL |
|-----------|:-----:|:------:|:------:|:-------:|:-----:|:--------:|:----------:|:----------:|
| **Embedded — no daemon** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Single-directory deploy** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Document / object storage** | ✅ | ⚠️ | ⚠️ | ✅ | ⚠️ | ⚠️ | ⚠️ | ⚠️ |
| **Graph relationships** | ✅ | ❌ | ❌ | ❌ | ✅ | ⚠️ | ❌ | ❌ |
| **Vector / semantic search** | ✅ | ❌ | ⚠️ | ❌ | ❌ | ✅ | ❌ | ⚠️ |
| **Time-series / observations** | ✅ | ❌ | ⚠️ | ❌ | ❌ | ❌ | ✅ | ⚠️ |
| **Analytical aggregations** | ⚠️ | ⚠️ | ✅ | ⚠️ | ❌ | ❌ | ✅ | ✅ |
| **Full version history** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Point-in-time queries** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ⚠️ | ❌ |
| **Provenance on every write** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **WAL-first durability** | ✅ | ✅ | ⚠️ | ✅ | ✅ | ⚠️ | ✅ | ✅ |
| **ACID transactions** | ⚠️ | ✅ | ✅ | ✅ | ✅ | ❌ | ⚠️ | ✅ |
| **SQL query language** | ⚠️ | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ | ✅ |
| **Full-text search** | ❌ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | ⚠️ | ✅ |
| **Schema enforcement** | ❌ | ⚠️ | ✅ | ❌ | ⚠️ | ✅ | ✅ | ✅ |
| **Horizontal scaling** | ❌ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Multi-process / concurrent writers** | ❌ | ⚠️ | ⚠️ | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Zero operational overhead** | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |

**Key:** ✅ First-class support · ⚠️ Partial or via extension · ❌ Not supported

---

## vs SQLite

SQLite is the closest architectural peer: embedded, single-file, WAL-based, zero ops.
It is the right choice for relational data with a stable schema.

| Dimension | bindb | SQLite |
|-----------|-------|--------|
| Data model | Objects, edges, observations, vectors | Relational tables |
| Query language | Rust API | SQL |
| Graph traversal | BFS built-in | Recursive CTEs (workaround) |
| Vector search | Cosine similarity built-in | Via sqlite-vss extension |
| Version history | Every write versioned automatically | Manual audit tables |
| Provenance | Built-in field on every write | Manual columns |
| Temporal queries | `object_as_of(id, commit)` | Manual effective-date columns |
| Semantic search | Embedding index built-in | Not available in core |
| Schema | Schema-free (raw bytes per object) | Strict column schema |
| Concurrent writers | Single process | WAL mode: multiple readers, one writer |

**Choose SQLite when:** the data is relational, schemas are known upfront, SQL is needed,
or the team has existing SQL expertise.

**Choose bindb when:** the data is semantic and graph-connected, version history and
provenance matter, or the application needs to query across objects, relationships,
embeddings, and observations without application-layer joins.

---

## vs DuckDB

DuckDB is the embedded database most similar to bindb in deployment model — no daemon,
single directory, library API, zero ops.  Where SQLite is "the embedded OLTP database",
DuckDB is "the embedded OLAP database."  It is an exceptional choice for analytical
workloads and increasingly popular as a local-first data science tool.

| Dimension | bindb | DuckDB |
|-----------|-------|--------|
| Deployment | Embedded library | Embedded library |
| Primary orientation | OLTP — many small writes, semantic reads | OLAP — large scans, aggregations |
| Storage model | Append-only WAL (row-oriented) | Columnar (PAX layout) |
| Write throughput | ~120 durable commits/sec; ~5 K+ buffered/sec | Excellent for bulk inserts; slow for single-row OLTP |
| Analytical read throughput | Not applicable (in-memory row indexes) | Sub-second over millions of rows via vectorized execution |
| Query language | Rust API + restricted SQL dialect | SQL (fully PostgreSQL-compatible) |
| Graph relationships | First-class BFS traversal | Not available |
| Vector / semantic search | Cosine similarity built-in | Array distance functions (not ANN) |
| Time-series / observations | Typed observation model | Via SQL window functions |
| Full version history | Every write versioned automatically | Not available |
| Point-in-time queries | `object_as_of(id, commit)` | Not available |
| Provenance | Built-in per-write field | Manual columns |
| Schema | Schema-free (raw bytes) | Strongly typed columns |
| External data sources | No | Reads Parquet, CSV, JSON, Arrow, S3 directly |
| Python / data science integration | No | Excellent (native pandas/polars integration) |
| Multi-process writes | Single process | Single writer (multiple readers) |
| Horizontal scaling | No | MotherDuck (cloud extension) |

**The OLTP / OLAP divide:**  DuckDB is deliberately not optimized for high-frequency small
writes.  Its columnar storage shines when scanning millions of rows with filters and
aggregations — the opposite of bindb's workload of thousands of individual object commits
per session.  Attempting to use DuckDB as an OLTP store (one `INSERT` per user action)
produces poor performance; attempting to use bindb for ad-hoc analytical SQL is not
possible today.

**The analytical gap:**  DuckDB's SQL engine handles millions of rows with vectorized
columnar execution, JIT compilation, and native Parquet/S3 integration.  bindb's
`sql_aggregate` and `sql_project` layers cover GROUP BY, window functions, and common
aggregates (COUNT/SUM/AVG/P50/P99/STDDEV) over the in-memory observation store — sufficient
for application-scale analytics.  At data-science scale (tens of millions of rows, external
data joins, pandas integration) DuckDB remains the better tool.

**Complementary, not competing:**  bindb and DuckDB serve different workloads.  bindb is
the live operational store — every user action commits with full provenance and version
history, and application-scale analytical queries run via the built-in SQL dialect.
DuckDB excels as a data-science projection layer when the workload grows to millions of
rows, requires external data sources, or needs pandas/polars integration.

**Choose DuckDB when:** the workload is primarily analytical (aggregations, window
functions, large scans), SQL is required, external data sources (Parquet, CSV, S3) need
to be joined with application data, or data science tooling (pandas, polars) is the
primary consumer.

**Choose bindb when:** the workload is OLTP (many small writes per session), version
history and provenance are required, graph relationships are central, or the data is
semantic and multi-modal (objects + edges + observations + vectors).

---

## vs MongoDB

MongoDB excels at flexible document storage at scale with a rich query language.

| Dimension | bindb | MongoDB |
|-----------|-------|---------|
| Deployment | Embedded library | Daemon (mongod) |
| Operational overhead | None | Significant (replication, backups, monitoring) |
| Data model | Objects + edges + vectors + observations | Documents + arrays |
| Graph queries | BFS traversal built-in | `$graphLookup` (limited depth, slow) |
| Vector search | Built-in | Atlas Vector Search (cloud only) |
| Version history | Every commit immutable, full history | Change streams (not queryable history) |
| Point-in-time | `object_as_of(id, commit)` | Not available |
| Provenance | Per-write provenance field | Not available |
| Query language | Rust API | MQL / aggregation pipeline |
| Horizontal scaling | Single process | Sharding + replica sets |
| Full-text search | Not available | Atlas Search / text indexes |

**Choose MongoDB when:** the application needs horizontal scale, a rich aggregation
pipeline, Atlas cloud integration, or multi-process concurrent writes at high volume.

**Choose bindb when:** the application is desktop or single-tenant, graph relationships
and version history are core requirements, or operational simplicity is a priority.

---

## vs Neo4j

Neo4j is the leading native graph database with a first-class Cypher query language.

| Dimension | bindb | Neo4j |
|-----------|-------|-------|
| Deployment | Embedded library | Daemon (JVM) |
| Memory footprint | Rust process memory only | JVM heap (typically 1–8 GB) |
| Graph model | Directed edges with type + payload | Property graph (nodes + relationships) |
| Graph query language | Rust API + BFS traversal | Cypher (expressive pattern matching) |
| Document storage | Objects with arbitrary byte payload | Node properties (limited types) |
| Vector search | Built-in | Via vector index plugin |
| Version history | Full WAL history on every write | Enterprise only (point-in-time recovery) |
| Temporal queries | `object_as_of(id, commit)` | Not available in Community |
| Provenance | Built-in | Manual properties |
| Horizontal scaling | Single process | Causal clustering (Enterprise) |

**The Cypher gap:** Cypher pattern matching is significantly more expressive than bindb's
BFS API.  Queries like "find all Memories connected to a Person within 3 hops where the
edge type is RELATES_TO and the Memory has a sentiment score > 0.8" require application
code in bindb today.  A declarative query layer is on the roadmap.

**Choose Neo4j when:** complex graph pattern matching is the primary use case, Cypher
expertise exists on the team, or Enterprise features (clustering, point-in-time backup) are
required.

**Choose bindb when:** the graph is one of several data concerns (alongside documents,
observations, and embeddings), the JVM overhead is unacceptable for a desktop application,
or version history on graph structure itself is required.

---

## vs Weaviate

Weaviate is a purpose-built vector database with semantic search as its primary concern.

| Dimension | bindb | Weaviate |
|-----------|-------|---------|
| Deployment | Embedded library | Daemon (Docker / cloud) |
| Vector search | Brute-force cosine | HNSW approximate nearest neighbor |
| Search quality at scale | Exact (no approximation error) | Approximate (tunable recall) |
| Search performance at scale | O(n) — degrades past ~100K vectors | O(log n) — scales to billions |
| Object storage | Arbitrary byte payload | Schema-enforced properties |
| Graph relationships | First-class edges + traversal | Cross-references (limited) |
| Version history | Full WAL history | Not available |
| Provenance | Built-in | Not available |
| Multi-modal | Objects + edges + observations | Objects + vectors |
| Operational overhead | None | Significant |

**The ANN gap:** bindb's vector search is brute-force cosine similarity — correct and
simple, but O(n) in the number of indexed vectors.  At < 100 K vectors this is fast enough
for most desktop applications (< 10 ms per query).  Beyond 100 K vectors, an ANN structure
(HNSW, IVF) is needed.  This is a planned Phase 2 capability.

**Choose Weaviate when:** vector search is the dominant query pattern, the vector corpus
exceeds 100 K entries, or approximate-but-fast search is acceptable.

**Choose bindb when:** semantic search is one of several query concerns alongside graph
traversal, version history, and observations, or the application is desktop/embedded and
cannot run a Docker container.

---

## vs ClickHouse

ClickHouse is a columnar OLAP database optimized for time-series analytics at massive scale.

| Dimension | bindb | ClickHouse |
|-----------|-------|-----------|
| Deployment | Embedded library | Daemon (significant ops) |
| Write model | Append-only WAL, one commit at a time | Columnar batch inserts |
| Read model | In-memory row-oriented indexes | Column scans, vectorized execution |
| Time-series | Observations stored as objects | MergeTree engine (purpose-built) |
| Aggregations | Application-level only | SQL aggregations, window functions |
| Query speed on billions of rows | Not applicable (in-memory) | Sub-second |
| Graph queries | First-class | Not available |
| Version history | Full WAL history | Not available |
| Provenance | Built-in | Manual columns |
| Scale | Single process, RAM-bounded | Petabyte scale |

**Choose ClickHouse when:** the primary use case is analytical queries over billions of
rows, SQL aggregations and window functions are required, or the data volume exceeds what
fits in RAM.

**Choose bindb when:** observations are one of several data concerns alongside graph
relationships and semantic search, the data volume is life-scale (millions, not billions of
rows), or the application cannot operate a ClickHouse cluster.

---

## vs PostgreSQL (with extensions)

PostgreSQL with `pgvector`, `TimescaleDB`, and `Apache AGE` is the closest
all-in-one alternative.

| Dimension | bindb | PostgreSQL + extensions |
|-----------|-------|------------------------|
| Deployment | Embedded library | Daemon + extension management |
| Graph queries | BFS built-in | AGE (Cypher on Postgres) — complex setup |
| Vector search | Built-in | pgvector (IVF/HNSW) |
| Time-series | Observation model built-in | TimescaleDB hypertables |
| Version history | Every write versioned, WAL replay | Not available (WAL is write-ahead, not queryable history) |
| Point-in-time | `object_as_of(id, commit)` | Not available |
| Provenance | Per-write provenance field | Manual columns |
| Schema | Schema-free | Strict typing |
| SQL | No | Yes — full SQL + extensions |
| Multi-process | No | Yes |
| Horizontal scaling | No | Via Citus / read replicas |
| Zero ops | Yes | No |

PostgreSQL + extensions is the strongest general-purpose alternative when SQL fluency,
multi-process access, and mature tooling (pg_dump, replication, monitoring) are priorities.

**The extension complexity problem:** combining pgvector + TimescaleDB + AGE requires
managing three separate extension versions, potential conflicts, and different query APIs
for each concern.  Joins across these concerns (e.g., "find memories similar to this
embedding that are connected to this health observation via a graph path") require
complex multi-step queries.

**Choose PostgreSQL when:** relational data + SQL expertise + horizontal scaling + mature
ecosystem are the priority constraints.

**Choose bindb when:** version history, provenance, and the unified object-graph-vector
model are core product requirements, and operational simplicity matters.

---

## Where bindb wins today

**Unified model across all data types.**  A single `commit()` can write an Object, an
Edge, and attach metadata — all with the same consistency guarantee, the same version
history, the same provenance.  No application-layer sync required.

**Version history as a first-class primitive.**  Every write is immutable.  Time travel
(`object_as_of`) and audit trails are structural features, not bolt-on logging.  No other
database in this comparison provides this without significant custom engineering.

**Provenance on every write.**  Source attribution (`Plugin`, `Import`, `UserEdit`,
`System`) is a typed field on every object version.  The lineage of every data point is
always recoverable.

**Zero operational overhead.**  `BinderStore::open(path)` is the entire deployment.
No daemon, no Docker, no connection pool, no backup agent.  The store directory is
self-contained — copy the folder, restore the data.

**Semantic data by default.**  The ObjectId URI convention (`scheme://path`) makes
namespace-based queries O(k) via the secondary index.  The observation model makes
metric-typed time series a first-class data type, not a workaround built on generic
documents.

---

## Where bindb does not win yet

**No full-text search.**  There is no text index.  Callers must implement their own
tokenization or use the vector index for semantic similarity.

**ANN vector search has a floor.**  The vector index auto-switches from brute-force to
HNSW at 1,000 vectors per model bucket.  Below that threshold, search is O(n).  There
is no persistence for the HNSW graph — it is rebuilt in memory on each open.

**Single-process only.**  One process holds the store at a time.  Multi-user concurrent
access requires a network layer (gRPC, HTTP) in front of bindb.

**Head index is capped, not unbounded.**  The live-object head cache holds up to 200,000
entries by default; misses fall back to a mmap-backed version file (~1 µs read).
Deployments with more than 200,000 distinct live objects pay a per-miss latency cost
unless `open_with_head_cap` is used to raise or remove the cap.

**Restricted SQL only.**  `sql_aggregate` and `sql_project` cover GROUP BY, window
functions, and common aggregates over the in-memory observation store.  Joins,
subqueries, and queries from non-Rust clients are not supported.

---

## Decision guide

```
Need SQL + relational schema + multi-process?              → PostgreSQL or SQLite
Need embedded SQL analytics + aggregations + Parquet/S3?   → DuckDB
Need petabyte analytics + columnar SQL at daemon scale?    → ClickHouse
Need complex Cypher graph patterns at enterprise scale?    → Neo4j
Need ANN vector search at > 100 K vectors?                 → Weaviate

Need unified objects + graph + vectors + observations
  in a single embedded store, with full version history
  and provenance, for a desktop or single-tenant app?      → bindb
```

bindb is the right choice when the application's **primary complexity is in the data
relationships and history**, not in the query language or the scale of concurrent writers.
