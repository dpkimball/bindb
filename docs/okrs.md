# bindb OKRs (engineering)

Horizon: **90 days** from 2026-07-23. Owner: bindb maintainers.
Evidence source of truth: `cargo run --example perf_report --release` → `perf-report.md` / `perf-report.json` (also CI artifact).

These are contracts, not aspirations dressed as wins. A KR is done only when the
report (or linked PR) shows the number.

---

## O1 — Make performance evidence trustworthy

Stop arguing from markdown narrative. Every labeled Perf run and master push
must leave a concrete report.

| KR | Target | How we know |
|----|--------|-------------|
| KR1.1 | Perf workflow uploads `perf-report.md` + `perf-report.json` as artifacts | GitHub Actions artifact on Perf job |
| KR1.2 | Report covers all M-tier SLA rows + open(L) + list(L) + HNSW recall@10 | Schema `bindb.perf_report.v1` field list |
| KR1.3 | `docs/benchmark-comparison.md` ANN row updated (HNSW on master) and links to how to generate the report | Doc PR merged |
| KR1.4 | SLA gate and report use the same thresholds (`docs/sla.md` / `tests/perf_sla.rs`) | Diff review; fail CI if thresholds diverge (manual check this quarter) |

---

## O2 — Bulk ingest good enough for life-scale health imports

Life-scale: ~500 K observations/year. Buffered path is the bottleneck.

| KR | Target | How we know |
|----|--------|-------------|
| KR2.1 | `commit_buffered` 10 K batch ≥ **8_000** commits/sec on CI runner (raise SLA floor from 3_000) | `commit_buffered_tps` in report + SLA constant bump |
| KR2.2 | `commit_buffered` 50 K batch ≥ **5_000** commits/sec (new report metric + SLA) | New metric id in `perf-report.json` |
| KR2.3 | Documented wall-clock for **500 K** single-field observation import ≤ **90 s** on baseline hardware | Scripted import in report or separate `examples/health_import_bench.rs`; number checked into report comment |

If KR2.* miss: attach flamegraph / lock contention notes; do not silently lower the bar.

---

## O3 — Vector search is a contract, not a demo

HNSW landed; it is not yet an SLA.

| KR | Target | How we know |
|----|--------|-------------|
| KR3.1 | Add `vector_search_p99_ms` at **100 K** vectors (dim 384 or project default) ≤ **5 ms** for k=10 | New SLA + report row |
| KR3.2 | `hnsw_recall_at_10` ≥ **90%** at 100 K (raise from 80% @ 1.2 K) | Report + `tests/vector_index.rs` |
| KR3.3 | `docs/performance.md` complexity table says ANN (HNSW), not O(n) scan | Doc fix merged |

---

## O4 — Cold open predictable at L-tier

Open() is the noisy gate today.

| KR | Target | How we know |
|----|--------|-------------|
| KR4.1 | `open_l_ms` ≤ **300** on CI with **3 consecutive** Perf runs green (no flake) | Three master Perf runs; report values archived |
| KR4.2 | After clean close, checkpoint short-circuit open at 100 K ≤ **50 ms** (or explicit “won’t ship this quarter” written in issue #9 follow-up) | Report metric or closed decision issue |
| KR4.3 | Coefficient of variation of `open_m_ms` across 5 local runs ≤ **25%** | Local script output attached to issue |

---

## O5 — Decide the scale ceiling

| KR | Target | How we know |
|----|--------|-------------|
| KR5.1 | Written decision on issue **#6** (mmap / larger-than-RAM): ship spike, defer, or won’t-do with rationale | Issue comment + label |
| KR5.2 | If defer/won’t-do: documented max practical store size for Binder (objects + RAM) in `docs/sla.md` | Doc section merged |
| KR5.3 | If ship: spike PR with 1 M object open/get numbers in `perf-report` (XL row, not necessarily gated) | Report contains XL metrics |

---

## Non-goals (this quarter)

- Matching DuckDB/SQLite bulk COPY throughput
- Multi-process writers
- Cypher parity (#2) as a perf OKR (product/query surface, not latency)
- Public marketing comparison page (Pages site) until KR1.1 is green for 2 weeks

---

## Operating rules

1. Perf PR label required to run gate + report (unchanged).
2. Changing an SLA constant requires updating `docs/sla.md`, `tests/perf_sla.rs`, and `examples/perf_report.rs` in the same PR.
3. Failed OKR → remediation plan in the PR that would have closed it; no silent threshold relaxation without that writeup.
