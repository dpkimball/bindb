# bindb — Issue / roadmap status

Last updated: 2026-07-24. Prefer this over stale issue bodies.

| Issue | State | Shipped | Remaining / follow-up |
|-------|-------|---------|----------------------|
| [#1](https://github.com/dpkimball/bindb/issues/1) WAL compaction | Closed | `compact()`, snapshot + WAL truncate | Ops policy for when to compact |
| [#2](https://github.com/dpkimball/bindb/issues/2) Graph query IR | Open | Rust `GraphQuery` builder + `graph_ir()` MATCH (types, directions, id predicates, **JSON payload WHERE** `AND b.field OP value`) + **`RETURN b.f1, b.f2` field projections** (`graph_ir_projected` → `GraphIrRow`) | Full Cypher; path collection; write clauses |
| [#3](https://github.com/dpkimball/bindb/issues/3) HNSW | Closed | HNSW ≥1k live vectors/model | Latency SLA @ 100k (OKR O3) |
| [#4](https://github.com/dpkimball/bindb/issues/4) Full-text | Closed (narrow) | Manual `index_text` / `text_search` | Real FTS (tokenize, persist, rank) |
| [#5](https://github.com/dpkimball/bindb/issues/5) Aggregation / SQL | Open | Rust DSL + `sql_aggregate()` (COUNT/AVG/SUM/MIN/MAX/P50/P99, PATH_SEGMENT, **WHERE id STARTS WITH**) + **`export_observations_jsonl()`** (DuckDB-compatible JSONL) | Full SQL; joins; DuckDB / out-of-process projection |
| [#6](https://github.com/dpkimball/bindb/issues/6) mmap / >RAM | Open | Disk-backed `VersionStore` (version_log.bin, ~40 B/version in RAM); **mmap read path** (memmap2, zero seek/read syscalls after initial map) | Cold-version LRU eviction; mmap for head indexes |
| [#7](https://github.com/dpkimball/bindb/issues/7) XL 1M | Open | Snapshot fast open; XL get/open-after-compact PASS; **formal XL SLA tests** + **nightly CI job** (02:00 UTC) | Additional XL SLA metrics; nightly runner uptime guarantee |
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
| Live object heads | 100 K × ~200 B | ~20 MB |
| Observation heads (1 yr health) | ~500 K × ~100 B | ~50 MB |
| Version history index (10× churn) | ~1–5 M × ~40 B (index only) | ~40–200 MB |
| Version history payloads | ~1–5 M records | on disk (version_log.bin) |
| Graph adjacency | proportional to edges | tens of MB |
| Vector index (HNSW, if large) | model-dependent | separate |

**Change from prior baseline:** version payloads are now on disk. The ~40 B/version
index entry stays in RAM; `read_record_at` hits the mmap (OS page cache) rather than
the heap. This reduces steady-state RSS for history-heavy stores by the payload size
(~150–200 B/version saved from heap → page cache).

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
