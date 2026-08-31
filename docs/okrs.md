---
title: bindb OKRs (engineering)
description: Engineering OKRs for the 90-day horizon, with key results, evidence sources, and operating rules.
category: vision
---

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
| KR1.1 ✅ | Perf workflow uploads `perf-report.md` + `perf-report.json` as artifacts | GitHub Actions artifact on Perf job — present since PR #11 |
| KR1.2 ✅ | Report covers all M-tier SLA rows + open(L) + list(L) + HNSW recall@10 | All metrics present in `perf-report.json`; verified via `[PASS]` output |
| KR1.3 ✅ | `docs/benchmark-comparison.md` ANN row updated (HNSW on master) and links to how to generate the report | Updated in PR #16 |
| KR1.4 ✅ | SLA gate and report use the same thresholds (`docs/sla.md` / `tests/perf_sla.rs`) | Constants mirrored in `examples/perf_report.rs`; confirmed manually |

---

## O2 — Bulk ingest good enough for life-scale health imports

Life-scale: ~500 K observations/year. Buffered path is the bottleneck.

| KR | Target | How we know |
|----|--------|-------------|
| KR2.1 ✅ | `commit_buffered` 10 K batch ≥ **8_000** commits/sec on CI runner (raise SLA floor from 3_000) | `commit_buffered_tps` SLA constant raised; measured ~260 K/s locally (PR #16) |
| KR2.2 ✅ | `commit_buffered` 50 K batch ≥ **5_000** commits/sec (new report metric + SLA) | `commit_buffered_50k_tps` in `perf-report.json`; ~50 K/s locally (PR #16) |
| KR2.3 ✅ | Documented wall-clock for **500 K** single-field observation import ≤ **90 s** on baseline hardware | `health_import_500k_s` metric (~10 s, 88% headroom); SLA test `sla_health_import_500k_lte_90s`; row in `docs/sla.md` (PR #18) |

If KR2.* miss: attach flamegraph / lock contention notes; do not silently lower the bar.

---

## O3 — Vector search is a contract, not a demo

HNSW landed; it is not yet an SLA.

| KR | Target | How we know |
|----|--------|-------------|
| KR3.1 ✅ | Add `vector_search_p99_ms` at **100 K** vectors (dim 384 or project default) ≤ **5 ms** for k=10 | `vector_search_p99_ms` in report (~0.6 ms locally); SLA test in `perf_sla.rs` (PR #16) |
| KR3.2 ✅ | `hnsw_recall_at_10` ≥ **90%** at 100 K (raise from 80% @ 1.2 K) | `hnsw_recall_at_10` = 95.1% locally; `EF_SEARCH` raised 64→128 (PR #16) |
| KR3.3 ✅ | `docs/performance.md` complexity table says ANN (HNSW), not O(n) scan | Present: "O(log n) approx with HNSW when live count ≥ 1 000 / model; else O(n) brute cosine" |

---

## O4 — Cold open predictable at L-tier

Open() is the noisy gate today.

| KR | Target | How we know |
|----|--------|-------------|
| KR4.1 | `open_l_ms` ≤ **300** on CI with **3 consecutive** Perf runs green (no flake) | Locally ~178 ms (40% headroom); needs 3 consecutive master Perf runs to close |
| KR4.2 | After clean close, checkpoint short-circuit open at 100 K ≤ **50 ms** (or explicit "won't ship this quarter" written in issue #9 follow-up) | **Won't ship this quarter** — snapshot fast path already cuts open to ~10 ms at M and ~178 ms at L; a further 50 ms target requires invasive index mmap on open, which is the next-quarter #6 payload-stripping work. Decision: defer to next quarter alongside `object_versions` payload stripping. |
| KR4.3 ✅ | Coefficient of variation of `open_m_ms` across 5 local runs ≤ **25%** | CV = 1.2% (mean 10.2 ms, σ 0.12 ms); see measurement below |

### KR4.3 measurement (2026-07-25, Apple Silicon, release)

| Run | `open_m_ms` |
|-----|------------|
| 1 | 10.005 |
| 2 | 10.368 |
| 3 | 10.102 |
| 4 | 10.179 |
| 5 | 10.191 |

Mean: 10.169 ms · σ: 0.119 ms · **CV: 1.2%** (target ≤ 25%) ✅

---

## O5 — Decide the scale ceiling

| KR | Target | How we know |
|----|--------|-------------|
| KR5.1 ✅ | Written decision on issue **#6** (mmap / larger-than-RAM): ship spike, defer, or won't-do with rationale | Head-cache LRU shipped (PR #17); next slice documented in [issue #6 comment](https://github.com/dpkimball/bindb/issues/6#issuecomment-5079074892) and `roadmap-status.md` |
| KR5.2 ✅ | If defer/won't-do: documented max practical store size for Binder (objects + RAM) in docs | See `roadmap-status.md` § "Interim scale ceiling" — rough RSS model + measured XL numbers |
| KR5.3 ✅ | Optional XL rows in `perf-report` (`PERF_REPORT_XL=1`); issue #7 stays open until a measured 1 M run is attached | `get_xl_p99_us` (0.042 µs) + `open_xl_ms` (1289 ms) measured and attached to issue #7 |

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
