# bindb — Issue / roadmap status

Last updated: 2026-07-23. Prefer this over stale issue bodies.

| Issue | State | Shipped | Remaining / follow-up |
|-------|-------|---------|----------------------|
| [#1](https://github.com/dpkimball/bindb/issues/1) WAL compaction | Closed | `compact()`, snapshot + WAL truncate | Ops policy for when to compact |
| [#2](https://github.com/dpkimball/bindb/issues/2) Graph query IR | Closed (narrow) | Builder + `graph_ir()` MATCH (`*min..max`, types, directions, id predicates) | Full Cypher; payload WHERE; path RETURN (OKR / new issue when needed) |
| [#3](https://github.com/dpkimball/bindb/issues/3) HNSW | Closed | HNSW ≥1k live vectors/model | Latency SLA @ 100k (OKR O3) |
| [#4](https://github.com/dpkimball/bindb/issues/4) Full-text | Closed (narrow) | Manual `index_text` / `text_search` | Real FTS (tokenize, persist, rank) |
| [#5](https://github.com/dpkimball/bindb/issues/5) Aggregation / SQL | Closed (narrow) | Rust DSL + `sql_aggregate()` (`COUNT`/`AVG`/`SUM`/`MIN`/`MAX`/`P50`/`P99`, `PATH_SEGMENT`) | Full SQL; DuckDB / out-of-process projection |
| [#6](https://github.com/dpkimball/bindb/issues/6) mmap / >RAM | Open | Interim ~2 GB ceiling; `rss_xl_mb` in XL report; compact drops commit log from RAM/snap | Spike when `rss_xl_mb` stays above ~2 GB after compact (measured ~2.4 GB before commit-log drop) |
| [#7](https://github.com/dpkimball/bindb/issues/7) XL 1M | Closed | Snapshot fast open; XL get/open-after-compact PASS | Optional nightly XL job |
| [#8](https://github.com/dpkimball/bindb/issues/8) Bulk vs SQLite | Closed (mitigated) | `commit_many`, O(1) retirement | Still far from SQLite COPY (OKR O2) |
| [#9](https://github.com/dpkimball/bindb/issues/9) Checkpoint open | Closed | Snapshot fast path on `open()` | Variance / flake control (OKR O4) |

Engineering OKRs: [`okrs.md`](okrs.md).

---

## Interim scale ceiling (issue #6 — deferred)

Rough **process RSS** model for a Binder life-scale store (order-of-magnitude):

| Content | Count | Ballpark RAM |
|---------|-------|--------------|
| Live object heads | 100 K × ~200 B | ~20 MB |
| Observation heads (1 yr health) | ~500 K × ~100 B | ~50 MB |
| Version history (10× churn) | ~1–5 M records | hundreds of MB–~1 GB |
| Graph adjacency | proportional to edges | tens of MB |
| Vector index (HNSW, if large) | model-dependent | separate |

**Decision (2026-07-23):** prefer `compact()` (now drops in-memory commit log), bounded history, and projection offload before mmap. XL sample before commit-log drop measured **~2.4 GB RSS** at 1 M objects — enough to keep #6 open. Remeasure `rss_xl_mb` after compact; start mmap spike only if still ≥ ~2 GB for real Binder workspaces.

---

## How to measure XL (#7)

```bash
PERF_REPORT_XL=1 PERF_REPORT_DIR=./out cargo run --example perf_report --release
```

Adds 1M-object rows (`get_xl_p99_us`, `open_xl_ms` after compact, `rss_xl_mb`). Not CI-gated by default.

### Local samples (2026-07-23, Apple Silicon, release)

| Metric | Measured | Notes |
|--------|----------|-------|
| `get()` p99 @ 1M | 0.042 µs | PASS vs ≤10 µs |
| `open()` after compact @ 1M | **1289 ms** | PASS vs ≤5000 ms |
| `open()` M / L (snapshot path) | ~11 / ~120 ms | PASS vs 30 / 300 ms |
| `rss_xl_mb` after compact open | **~2248 MB** | Above 2 GB interim; keeps #6 open |

Ops XL open = after `compact()` / clean snapshot. `rss_xl_mb` is informational (does not fail overall report).
