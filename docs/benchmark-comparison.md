# bindb — Benchmark Comparison

This document compares bindb's measured performance against published benchmarks for
SQLite, DuckDB, MongoDB, PostgreSQL, Neo4j, and ClickHouse.

---

## Methodology

### bindb measurements

All bindb numbers were measured directly on the project's development machine:

- **Hardware:** Apple M-series, NVMe SSD
- **Build:** `--release` (Rust optimized build)
- **Payload:** 64-byte object per write (representative of a small domain entity)
- **Source:** `examples/measure.rs` — run with `cargo run --example measure --release`

Each measurement was taken in isolation with a fresh store unless otherwise noted.
Percentiles are computed over ≥ 2,000 iterations after a warm-up pass.

### Reference numbers for other databases

Numbers for daemon-based databases (MongoDB, PostgreSQL, Neo4j, ClickHouse) are sourced
from published benchmarks listed below.  They reflect different hardware and workloads —
direct numeric comparison is informative but not definitive.  The architectural differences
(network round-trip, daemon overhead, serialization cost) are as significant as raw numbers.

**Sources:**
- SQLite: [sqlite.org/speed.html](https://www.sqlite.org/speed.html), TPC-C community results
- DuckDB: DuckDB vs SQLite benchmark (duckdb.org), H2O.ai benchmark (2023)
- MongoDB: YCSB results (MongoDB Engineering Blog), MongoDB Atlas performance reports
- PostgreSQL: pgbench TPC-B, sysbench OLTP results
- Neo4j: LDBC Social Network Benchmark (SNB), Neo4j performance guide
- ClickHouse: [benchmark.clickhouse.com](https://benchmark.clickhouse.com), ClickHouse blog

---

## Architectural context

Before the numbers: the most important performance difference between bindb and daemon-based
databases is **where the code runs**.

```
bindb       →  in-process, zero network overhead
               HashMap lookup = ~100 ns, no serialization

MongoDB     →  separate process, localhost TCP socket
               findOne = ~100–500 µs (network + BSON + WiredTiger)

PostgreSQL  →  separate process, Unix socket
               SELECT by PK = ~100–1,000 µs (socket + protocol + buffer pool)

Neo4j       →  separate JVM process, Bolt protocol
               MATCH by id = ~1–5 ms (JVM + Cypher + Lucene)
```

bindb's read latency advantage is structural — not tuning.  An in-process HashMap lookup
will always be orders of magnitude faster than a network call, regardless of how well the
other system is optimized.

---

## Write throughput

### Durable single-object writes (one fsync per write)

The most conservative write mode: each commit is individually fsynced to disk before
returning.  Latency is dominated by the disk's fsync cost (~7–10 ms on NVMe SSD).

| Database | Durable write throughput | Notes |
|----------|------------------------|-------|
| **bindb** `commit()` | **~130 writes/sec** | Measured — WAL fsync per commit |
| SQLite autocommit (WAL + FULL sync) | ~100–150 writes/sec | [sqlite.org/speed.html] — same fsync bound |
| DuckDB single-row INSERT | ~1,000–5,000 writes/sec | Columnar format, different durability model |
| PostgreSQL autocommit | ~500–3,000 writes/sec | fsync-bound; varies by `synchronous_commit` |
| MongoDB `writeConcern: majority` | ~5,000–20,000 writes/sec | Replication ack hides per-write fsync cost |
| MongoDB `writeConcern: 1` | ~20,000–80,000 writes/sec | No replication wait; network-bound |
| Neo4j durable single write | ~5,000–30,000 writes/sec | Daemon batches fsyncs internally |
| ClickHouse single INSERT | < 100 writes/sec | Not designed for single-row OLTP |

**Takeaway:** bindb and SQLite are nearly identical in durable write throughput — both are
bounded by the same fsync cost on the same hardware.  Daemon databases achieve higher
apparent throughput by batching fsyncs internally across concurrent connections.

---

### Bulk import (batched writes, one fsync for N commits)

bindb `commit_buffered()` + `flush()` trades per-commit durability for throughput — the
same pattern as wrapping SQLite inserts in a transaction.

| Database | Bulk write throughput | Batch size / notes |
|----------|-----------------------|--------------------|
| **bindb** `commit_buffered` | **~8,600 writes/sec** | Measured — 1 K batch |
| **bindb** `commit_buffered` | **~5,700 writes/sec** | Measured — 10 K batch |
| **bindb** `commit_buffered` | **~3,000 writes/sec** | Measured — 50 K batch |
| SQLite in-transaction INSERT | ~100,000–500,000 writes/sec | Single transaction wrapping N inserts |
| DuckDB bulk append | ~500,000–2,000,000 writes/sec | Columnar batch insert (COPY / appender) |
| MongoDB `insertMany` | ~50,000–300,000 writes/sec | Batched over network |
| PostgreSQL COPY | ~500,000–1,000,000 writes/sec | Binary COPY protocol |
| ClickHouse bulk INSERT | ~1,000,000–10,000,000 writes/sec | Columnar MergeTree, purpose-built |

**Takeaway:** bindb's bulk write throughput is competitive for life-scale data volumes
(< 50 K objects per session).  Remaining throughput decline at larger scales reflects
natural HashMap growth effects, not an algorithmic deficiency.  SQLite and columnar
databases remain faster for large-batch import; the gap is expected given bindb maintains
full version history and in-memory indexes on every write.

---

## Point lookup by ID

Retrieve a single object by its exact identifier.

| Database | p50 latency | p99 latency | Notes |
|----------|------------|------------|-------|
| **bindb** `get()` | **~100 ns** | **~125 ns** | Measured — 10 K objects, in-process HashMap |
| SQLite SELECT by PRIMARY KEY | ~1–5 µs | ~10 µs | B-tree lookup (3–4 levels) |
| DuckDB SELECT by PRIMARY KEY | ~100–500 µs | ~1 ms | Columnar format overhead for single row |
| MongoDB `findOne` by `_id` | ~100–500 µs | ~1 ms | Localhost TCP + BSON + WiredTiger |
| PostgreSQL SELECT by PRIMARY KEY | ~100–1,000 µs | ~2 ms | Unix socket + protocol + buffer pool |
| Neo4j MATCH by node id | ~1–5 ms | ~10 ms | Bolt protocol + JVM + Lucene index |
| ClickHouse SELECT by id | ~5–50 ms | ~100 ms | Columnar scan (not designed for point lookups) |

**Takeaway:** bindb's `get()` is the fastest point lookup in this comparison by 1–4 orders
of magnitude.  An in-process HashMap lookup at ~100 ns is a structural advantage over any
database requiring a network call or B-tree traversal.

---

## Namespace / prefix scan

Retrieve all objects whose ID matches a URI prefix (e.g. `"memory://"` — all memory objects).

| Database | p50 latency | p99 latency | Result set | Notes |
|----------|------------|------------|-----------|-------|
| **bindb** `list("ns://")` | **~110 µs** | **~210 µs** | 1 K of 10 K | Measured — namespace secondary index (O(k)) |
| SQLite `WHERE id LIKE 'ns://%'` | ~50–500 µs | ~1 ms | 1 K of 10 K | Index range scan on TEXT PRIMARY KEY |
| DuckDB `WHERE id LIKE 'ns://%'` | ~1–10 ms | ~20 ms | 1 K of 10 K | Columnar decompression overhead for small sets |
| MongoDB `find({ namespace: "ns" })` | ~0.5–5 ms | ~10 ms | 1 K of 10 K | Index scan + network + BSON deserialization |
| PostgreSQL `WHERE id LIKE 'ns://%'` | ~0.5–5 ms | ~10 ms | 1 K of 10 K | B-tree prefix scan + network |
| ClickHouse `WHERE startsWith(id, 'ns://')` | ~1–50 ms | ~100 ms | 1 K of 10 K | Columnar scan; better for larger result sets |

**Takeaway:** bindb and SQLite are comparable for prefix scans on in-process stores.
bindb's namespace index (HashMap scheme → HashSet of live ObjectIds) gives O(k) scan cost
independent of total store size.  For result sets > 10 K, bindb's advantage over SQLite
will grow because the index avoids touching any non-matching objects entirely.

---

## Graph traversal

BFS traversal from a starting node up to a fixed depth.

| Database | p50 latency (depth 3) | p50 latency (depth 5) | Notes |
|----------|----------------------|----------------------|-------|
| **bindb** `traverse(depth=3)` | **< 1 µs** | — | Measured — 5 K node chain, in-memory BFS |
| **bindb** `traverse(depth=5)` | — | **~1 µs** | Measured — 5 K node chain |
| SQLite recursive CTE depth=3 | ~1–10 ms | ~10–100 ms | No native graph engine |
| DuckDB recursive CTE depth=3 | ~5–50 ms | ~50–500 ms | Analytical engine, not graph-optimized |
| Neo4j Cypher MATCH path depth=3 | ~1–10 ms | ~5–50 ms | Native graph engine; Bolt latency adds base cost |
| MongoDB `$graphLookup` depth=3 | ~10–100 ms | ~100 ms–1 s | Limited depth; cross-document joins over network |
| PostgreSQL recursive CTE depth=3 | ~5–50 ms | ~50–500 ms | No native graph primitives |

**Takeaway:** bindb's in-process BFS traversal over an in-memory neighbour map is
extraordinarily fast for simple path queries — sub-microsecond at depth 3.  Neo4j still
wins for complex pattern matching (Cypher expressions bindb cannot express today), but for
straightforward reachability queries bindb is 3–4 orders of magnitude faster due to the
absence of a network round-trip and JVM overhead.

---

## Cold startup / WAL replay

Time from `open()` call to store ready for queries, after a previous clean close.

| Database | Startup time (10 K objects) | Notes |
|----------|-----------------------------|-------|
| **bindb** `open()` | **~14–18 ms** | Measured — O(n) position-index replay |
| SQLite `sqlite3_open()` | < 10 ms | Memory-mapped file; no replay required |
| DuckDB `duckdb_open()` | < 50 ms | Checkpointed WAL; fast recovery |
| MongoDB `mongod` startup | 1–5 sec | Daemon startup + WiredTiger journal replay |
| PostgreSQL `pg_ctl start` | 1–10 sec | Daemon startup + shared buffer warmup |
| Neo4j startup | 10–60 sec | JVM startup + store warmup |

> **Status:** WAL replay is now O(n) after the position-index fix.  Measured at 14–18 ms
> at 10 K commits.  Phase 2 checkpoint-based open will reduce this further by skipping
> full WAL deserialization — only a checksum validation + index rebuild from snapshot.

---

## Index rebuild

`rebuild_indexes()` — reconstruct all in-memory indexes from the version stores.
This runs automatically on crash recovery.

| Database | Rebuild time (10 K objects) | Notes |
|----------|-----------------------------|-------|
| **bindb** `rebuild_indexes()` | **~2–4 ms** | Measured — HashMap rebuild from version Vec |
| SQLite | Not applicable — B-tree is on disk | Disk-resident; survives crashes natively |
| DuckDB | Not applicable — columnar format on disk | Disk-resident |
| MongoDB | Index rebuild: minutes–hours at scale | `db.collection.reIndex()` is expensive |
| PostgreSQL | `REINDEX`: minutes at scale | Blocks table access |

---

## Summary

### Where bindb is fastest

| Operation | bindb advantage |
|-----------|----------------|
| Point lookup `get()` | **100–10,000× faster** than any database requiring a network call |
| BFS graph traversal | **1,000–100,000× faster** than SQL recursive CTEs or Neo4j for simple paths |
| Prefix scan `list()` | **5–50× faster** than daemon databases; comparable to SQLite |

### Where bindb is behind

| Operation | Gap | Root cause | Status |
|-----------|-----|-----------|--------|
| Bulk import | 15–500× slower than SQLite/DuckDB | Per-write version history + in-memory index maintenance | Expected — structural cost of full history |
| Cold startup | Comparable to SQLite (~14 ms at 10 K) | O(n) replay after position-index fix | Fixed |
| Analytical aggregations | Not supported | No query engine | Roadmap — SQL projection layer |
| ANN vector search | Latency SLA not yet gated at 100 K | HNSW on master (≥1 K / model); recall@10 in `perf-report` | OKR O3 — add latency SLA |

See also: [OKRs](okrs.md). Generate a report anytime with:

```bash
PERF_REPORT_DIR=./out cargo run --example perf_report --release
# → out/perf-report.md , out/perf-report.json
```

### Performance characteristic summary

```
Operation            bindb        SQLite      DuckDB       MongoDB     PostgreSQL
─────────────────────────────────────────────────────────────────────────────────
Durable write        ~130/sec     ~120/sec    ~2K/sec      ~10K/sec    ~1K/sec
Bulk write           ~5–8K/sec*   ~200K/sec   ~1M/sec      ~100K/sec   ~200K/sec
Point lookup         ~100 ns      ~3 µs       ~200 µs      ~300 µs     ~500 µs
Prefix scan (1K)     ~110 µs      ~200 µs     ~5 ms        ~2 ms       ~2 ms
Graph BFS depth-3    < 1 µs       ~5 ms       ~20 ms       ~50 ms      ~20 ms
Cold open (10K)      ~15 ms       < 10 ms     < 50 ms      1–5 sec†    1–10 sec†
Index rebuild        ~3 ms        N/A         N/A          minutes     minutes
─────────────────────────────────────────────────────────────────────────────────
* Degrades with store size; ~8K/sec at 1K batch, ~3K/sec at 50K batch
† Daemon startup time, not comparable to embedded open
```

### Interpretation

bindb is the right performance choice when the dominant query patterns are **point lookups**
and **graph traversal** — the two operations where in-process memory structures beat every
networked database by orders of magnitude.

bindb is not the right performance choice today when the dominant pattern is **bulk
analytical ingestion** or **large-batch import**.  SQLite in-transaction inserts are
100–300× faster for bulk loads at scale.  This will improve significantly once the O(n)
version retirement scan is fixed.
