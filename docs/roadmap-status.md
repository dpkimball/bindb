# bindb — Issue / roadmap status

Last updated: 2026-07-23. Prefer this over stale issue bodies.

| Issue | State | Shipped on master | Remaining |
|-------|-------|-------------------|-----------|
| [#1](https://github.com/dpkimball/bindb/issues/1) WAL compaction | Closed | `compact()`, snapshot + WAL truncate | Ops policy for when to compact |
| [#2](https://github.com/dpkimball/bindb/issues/2) Graph query | Open | Rust builder + restricted `graph_ir()` MATCH dialect | Full Cypher; min-hop filter; payload WHERE; path RETURN |
| [#3](https://github.com/dpkimball/bindb/issues/3) HNSW | Closed | HNSW ≥1k live vectors/model | Latency SLA @ 100k (OKR O3) |
| [#4](https://github.com/dpkimball/bindb/issues/4) Full-text | Closed (narrow) | Manual `index_text` / `text_search` | Real FTS (tokenize, persist, rank) |
| [#5](https://github.com/dpkimball/bindb/issues/5) Aggregation | Open | Rust DSL + restricted `sql_aggregate()` (`COUNT`/`AVG`/`SUM`/`MIN`/`MAX`, `PATH_SEGMENT`) | Full SQL; out-of-process / DuckDB projection |
| [#6](https://github.com/dpkimball/bindb/issues/6) mmap / >RAM | Open | — | Decision + optional spike (OKR O5) |
| [#7](https://github.com/dpkimball/bindb/issues/7) XL 1M | Open (reopened) | — | Measured 1M open/get via `PERF_REPORT_XL=1` |
| [#8](https://github.com/dpkimball/bindb/issues/8) Bulk vs SQLite | Closed (mitigated) | `commit_many`, O(1) retirement | Still far from SQLite COPY (OKR O2) |
| [#9](https://github.com/dpkimball/bindb/issues/9) Checkpoint open | Closed | Snapshot fast path on `open()` | Variance / flake control (OKR O4) |

Engineering OKRs: [`okrs.md`](okrs.md).

---

## Interim scale ceiling (issue #6 — defer mmap until exceeded)

Rough **process RSS** model for a Binder life-scale store (order-of-magnitude):

| Content | Count | Ballpark RAM |
|---------|-------|--------------|
| Live object heads | 100 K × ~200 B | ~20 MB |
| Observation heads (1 yr health) | ~500 K × ~100 B | ~50 MB |
| Version history (10× churn) | ~1–5 M records | hundreds of MB–~1 GB |
| Graph adjacency | proportional to edges | tens of MB |
| Vector index (HNSW, if large) | model-dependent | separate |

**Interim decision (2026-07-23):** defer mmap until a real Binder workspace approaches **~2 GB RSS** for bindb indexes or XL open/get numbers show RAM as the blocker. Document measured RSS when filing the #6 spike. Until then, prefer compaction (`compact()`), bounded history policy, and projection offload (#5) over mmap.

---

## How to measure XL (#7)

```bash
PERF_REPORT_XL=1 PERF_REPORT_DIR=./out cargo run --example perf_report --release
```

Adds 1M-object rows to `perf-report.md` / `.json`. Slow; not CI-gated by default.

### Local XL sample (2026-07-23, Apple Silicon, release)

| Metric | Measured | SLA | Result |
|--------|----------|-----|--------|
| `get()` p99 @ 1M objects | 0.042 µs | ≤ 10 µs | PASS |
| `open()` worst-of-3 @ 1M commits | 6218 ms | ≤ 5000 ms | FAIL (−24%) |

`get` at XL is fine. `open` still exceeds the provisional SLA; treat as evidence for #7 / compaction / snapshot path, not a ship gate yet.
