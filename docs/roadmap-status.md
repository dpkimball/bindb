# bindb — Issue / roadmap status

Last updated: 2026-07-28 (PRs #32–#40). Prefer this over stale issue bodies.


| Issue | State | Shipped | Remaining / follow-up |
|-------|-------|---------|----------------------|
| [#1](https://github.com/dpkimball/bindb/issues/1) WAL compaction | Closed | `compact()`, snapshot + WAL truncate | Ops policy for when to compact |
| [#2](https://github.com/dpkimball/bindb/issues/2) Graph query IR | Open | Rust `GraphQuery` builder + `graph_ir()` MATCH (types, directions, id predicates, **multiple AND payload predicates** `AND b.f1 OP v AND b.f2 OP v …`; `payload_preds: Vec<PayloadPred>`; 3 integration tests) + **`RETURN b.f1, b.f2` field projections** + **`RETURN path` shortest-path collection** (`graph_ir_projected` → `GraphIrRow`) + **write IR** (`CREATE`/`MERGE` NODE + EDGE via `graph_ir_write`; 8 integration tests) + **`ORDER BY / LIMIT / SKIP`** (`id(b)` lexicographic, `depth`, `b.field` JSON numeric/string; 8 integration tests) + **`OPTIONAL MATCH`** (left-outer-join traversal; `GraphIrRow::optional`; 13 integration tests) + **`WITH b`** (`GraphIrWithClause`; pre-expansion filter/sort/page; `RETURN ORDER BY` applies post-expansion when WITH is present; 12 integration tests) + **`WITH count(b) AS cnt`** (`AggFunc`/`WithAggregation`; `graph_ir_aggregated` → `Vec<HashMap<String, Value>>`; count/sum/avg/min/max; missing-field nodes skipped for numeric aggs; 14 integration tests) + **`WITH count(b) AS cnt GROUP BY b.field`** (`GraphIrWithClause::group_by_fields`; groups matched nodes by JSON payload field; missing-field nodes in null group; ungrouped = 1-row Vec; 7 integration tests) + **`GROUP BY b.f1, b.f2`** (multi-key composite grouping; `group_by_fields: Vec<String>`; composite key = JSON array for 2+ fields; null group for nodes missing any field; 5 additional integration tests) + **edge payload predicates** (`AND r.field OP value`; `GraphIrQuery::edge_preds: Vec<PayloadPred>`; BFS skips edges whose payload doesn't match; `[r:LABEL]` variable syntax accepted in pattern; combined with node predicates; 7 integration tests) + **edge field projection** (`RETURN r.field`; `GraphIrQuery::edge_return_fields`; `GraphIrRow::edge_fields: HashMap<String, Value>`; `GraphQueryMatch::via_edge`; missing fields → null; combined with b.field and AND r. predicates; 5 integration tests) + **`RETURN path` edge IDs** (`GraphIrRow::edge_path: Vec<EdgeId>`; parallel sequence to `path`; `parent_edge_map` in BFS; `reconstruct_path` returns both node and edge sequences; 4 integration tests) + **string payload predicates** (`STARTS WITH`, `ENDS WITH`, `CONTAINS`; `CmpOp::StartsWith/EndsWith/Contains`; keyword operator parsing in `parse_payload_pred_inner`; works on `b.field` and `r.field`; 7 integration tests) | Full Cypher (named path variable `p = (a)-[r*]->(b)`) |
| [#3](https://github.com/dpkimball/bindb/issues/3) HNSW | Closed | HNSW ≥1k live vectors/model; **100K SLA gate** (`knn_search` p99 ≤5ms, recall@10 ≥90%; OKR O3) | — |
| [#4](https://github.com/dpkimball/bindb/issues/4) Full-text | Closed (narrow) | Manual `index_text` / `text_search` | Real FTS (tokenize, persist, rank) |
| [#5](https://github.com/dpkimball/bindb/issues/5) Aggregation / SQL | Open | Rust DSL + `sql_aggregate()` (COUNT/AVG/SUM/MIN/MAX/P50/P99, PATH_SEGMENT, **WHERE id STARTS WITH**) + **`export_observations_jsonl()`** (DuckDB-compatible JSONL) + **`join_observations(left, right, key_fn)`** (pure-Rust inner join across two namespace prefixes; deduplicates by `CommitId`; `Vec<JoinRow>`; 6 integration tests); _removed_: `query_sql_observations` (DuckDB subprocess — wrong dep model) + **`GROUP BY PAYLOAD_FIELD('name')`** (`SqlGroupBy::PayloadField`; groups observations by named JSON payload field; numeric keys serialised as JSON strings; None group for missing/non-JSON payloads; 6 integration tests) + **`SELECT PAYLOAD_FIELD('f1'), PAYLOAD_FIELD('f2') FROM … [WHERE …] [LIMIT n]`** columnar projection (`SqlProjectQuery`; `sql_project()` → `Vec<HashMap<String, Value>>`; missing fields → JSON null; 7 integration tests) | Full SQL; window functions |
| [#6](https://github.com/dpkimball/bindb/issues/6) mmap / >RAM | Open | Disk-backed `VersionStore` (version_log.bin, ~40 B/version in RAM); **mmap read path** (memmap2, zero seek/read syscalls after initial map); **cold-version eviction** (sweep non-live IndexEntries when cap exceeded; mmap rescan on miss); **head-cache LRU** (bounded HeadObjectIndex/HeadEdgeIndex, default 200 K; miss falls back to mmap-backed VersionStore; `open_with_head_cap()`; 6 integration tests); **payload + provenance stripping** (`payload_stripped` field; retired versions have payload + provenance heap strings freed after disk write; `rebuild_from_versions` recovers full originals from mmap before truncate; 10 integration tests); **live-only Vec** (retired entries swap-removed from `object_versions`/`edge_versions` Vecs on commit; Vec holds only live versions; VersionStore is sole source for history; `rebuild_indexes` no longer rewrites disk; `open` compacts to live-only after crash-recovery rebuild; 3 new integration tests); **crash recovery** (dirty open or empty version_log.bin triggers rebuild from full commit log; final guard fires on any path when file has no data but commits exist; 4 integration tests) | Aspirational: true mmap-backed head index for >200 K distinct live objects without LRU eviction latency |
| [#7](https://github.com/dpkimball/bindb/issues/7) XL 1M | **Closed** | Snapshot fast open; XL get/open-after-compact PASS; **formal XL SLA tests** + **nightly CI job** (02:00 UTC) + **3 new XL SLA tests** (`list` p99 ≤500ms, `commit_buffered` ≥5K/s, `rebuild_indexes` ≤60s at 1M objects); **eviction bug fix** (O(1M) sweep per commit fixed; `commit_buffered` at XL now 260K/s); **nightly watchdog** (`nightly_watchdog.yml` on `ubuntu-latest` at 02:45 UTC; opens GitHub issue on missed/failed run) | — |
| [#8](https://github.com/dpkimball/bindb/issues/8) Bulk vs SQLite | Closed (mitigated) | `commit_many`, O(1) retirement | Still far from SQLite COPY (OKR O2) |
| [#9](https://github.com/dpkimball/bindb/issues/9) Checkpoint open | Closed | Snapshot fast path on `open()` | Variance / flake control (OKR O4) |

Engineering OKRs: [`okrs.md`](okrs.md).

---

## What is open vs. closed

Issues #2, #5, #6, #7 remain **open** because each has a well-defined next step that
hasn't shipped yet. The "Shipped" column describes what landed as of this date; the
"Remaining" column is why the issue stays open.

Closing an issue on a narrow first slice (builder DSL ≠ Cypher, mmap index ≠ mmap
head map) misrepresents completion. Reopen when "Remaining" work starts.

---

## Interim scale ceiling (issue #6 — in progress)

Rough **process RSS** model for a Binder life-scale store (order-of-magnitude):

| Content | Count | Ballpark RAM |
|---------|-------|--------------|
| Live object heads (cached) | min(live, 200 K) × ~300 B | ≤ 60 MB |
| Observation heads (1 yr health, cached) | min(live, 200 K) × ~100 B | ≤ 20 MB |
| Version history index (10× churn) | ~1–5 M × ~40 B (index only) | ~40–200 MB |
| Version history payloads | ~1–5 M records | on disk (version_log.bin) |
| `object_versions` Vec (live entries only) | n_live × ~80 B struct (no heap for retired) | **~8–80 MB** |
| Graph adjacency | proportional to edges | tens of MB |
| Vector index (HNSW, if large) | model-dependent | separate |

**Change from prior baseline:** head cache is now bounded (default 200 K entries).
Cold misses fall back to the mmap-backed VersionStore (~1 µs page-cache read).
At XL with 1 M live objects only the 200 K most-recently-committed objects stay in
the HeadObjectIndex; the rest are served from disk on first access.

**Payload stripping shipped (2026-07-26)**: retired entries in `object_versions` now
have their payload zeroed to `None` immediately after `version_log.bin` confirms the
write.  A `payload_stripped: bool` field distinguishes stripped entries from genuine
delete tombstones.  `rebuild_from_versions` recovers payloads from the mmap before
truncating so historical queries remain correct.  At XL this removes the
~300 MB–1.5 GB retired-payload allocation entirely.

**Provenance stripping shipped (2026-07-26)**: retired entries also have their
`provenance` field dropped from RAM (heap-allocated strings freed).  For users with
provenance on every version this saves ~190 MB per 5 M entries.

**Live-only Vec shipped (2026-07-25)**: `object_versions` and `edge_versions` Vecs now
hold only live (valid_to=None) entries.  On commit, retiring an entry does a swap-remove
(O(1) via position map) rather than nulling its payload in place.  Tombstones are
upserted into the head cache but never pushed into the Vec.  The VersionStore is the
sole source for historical queries (`object_history`, `object_as_of`).  Crash recovery
rebuilds version_log.bin from the full WAL when available; the snapshot+delta path
hydrates the existing file and applies only the delta idempotently.  `rebuild_indexes`
now rebuilds in-memory indexes only (does not rewrite version_log.bin).
At 1 M live objects and 5 M total historical: removes ~100 MB String heap + ~400 MB
struct overhead from retired Vec entries compared to the pre-stripping baseline.

**Crash recovery shipped (2026-07-27)**: `open_inner` now handles two failure modes
that would otherwise cause silent history loss.  On a dirty open where `version_log.bin`
is missing or truncated, the snapshot+delta path rebuilds from the full in-memory commit
log via `build_version_stores` + `rebuild_from_versions` rather than calling
`hydrate_index_from_file`.  A final guard fires on any open path: if the file has no
data but commits exist, rebuild unconditionally.  4 new integration tests cover the
wiped-file, truncated-file, clean-open, and fresh-store cases.

**Measured XL (1 M objects, 2026-07-23, Apple Silicon, release, after compact):**

| Metric | Measured | SLA |
|--------|----------|-----|
| `get()` p99 | 0.042 µs | ≤ 10 µs PASS |
| `open()` after compact | 1289 ms | ≤ 5000 ms PASS |
| `rss_xl_mb` after compact open | ~2248 MB | Informational |

`rss_xl_mb` is informational — does not fail CI. The remaining RAM is dominated by
head indexes, not version payloads. Further reduction requires LRU eviction of cold
head entries, which is the next step for #6.

---

## How to measure XL (#7)

```bash
PERF_REPORT_XL=1 PERF_REPORT_DIR=./out cargo run --example perf_report --release
```

XL SLA tests run via nightly CI or manually:

```bash
cargo test --release --test perf_sla -- --ignored --test-threads=1 scale:XL
```
