---
title: bindb — Performance SLAs
description: Performance contracts for bindb operations across scale tiers, enforced automatically by CI.
category: architecture
---

# bindb — Performance SLAs

This document defines the performance contracts for bindb operations.  SLAs are enforced
automatically on every PR via the `perf` CI job (`tests/perf_sla.rs`).  Trend data is
stored per-run and compared across PRs via Criterion benchmarks (`benches/store_bench.rs`).

---

## Scale tiers

All SLAs are defined at one or more scale tiers:

| Tier | Objects in store | Edges | CI enforcement |
|------|-----------------|-------|----------------|
| **S** | 1 K | 1 K | — |
| **M** | 10 K | 10 K | Per-push + labeled PRs (`perf` job; `sla_` tests, excluding `sla_l_` and `sla_xl_`) |
| **L** | 100 K | 50 K | Nightly (`nightly.yml`; `sla_l_` tests) |
| **XL** | 1 M | — | Nightly (`nightly.yml`; `sla_xl_` tests); manual: `PERF_REPORT_XL=1` on `perf_report` ([#7](https://github.com/dpkimball/bindb/issues/7)) |

Life-scale context: a single user accumulates ~500 K health observations per year,
~50 K memories over 10 years, and ~10 K–100 K financial records.
The **L** tier represents a well-used store after 2–3 years.

Interim RAM ceiling / mmap deferral: see [`roadmap-status.md`](roadmap-status.md) (issue #6).

---

## Write SLAs

### Durable commit (`commit()`)

Each call fsyncs the WAL.  Throughput is fsync-latency-bound.

| Metric | Target |
|--------|--------|
| Throughput at **M** | ≥ 80 commits/sec (single object per commit) |
| Throughput at **M** | ≥ 60 commits/sec (10 objects per commit) |
| p99 latency at **M** | ≤ 25 ms |

Rationale: user-facing mutations (create memory, log observation) must feel instantaneous.
80 commits/sec allows bursts of 80 rapid writes without queuing.

### Buffered commit + flush (`commit_buffered()` + `flush()`)

One fsync for an entire batch.  Throughput is serialization-bound.

| Metric | Target |
|--------|--------|
| Throughput at **M** (10 K batch, CI floor) | ≥ 8 K commits/sec |
| Throughput at **M** (50 K batch, CI floor) | ≥ 5 K commits/sec |
| `flush()` wall time after 10 K buffered commits | ≤ 20 ms |
| **Apple Health import: 500 K observations, 64-byte payload** | **≤ 90 s end-to-end** |

Rationale: Apple Health import (500 K data points) must complete in < 2 minutes.
8 K commits/sec means 500 K points takes ~62 seconds at the 10 K-batch floor.
The 90 s end-to-end ceiling (OKR O2/KR2.3) accounts for serialization + the single
`flush()` and is measured in `perf_sla.rs` (`sla_health_import_500k_lte_90s`) and
`examples/perf_report.rs` (`health_import_500k_s` metric).

---

## Read SLAs

All read SLAs assume a warm store (indexes in memory, no WAL replay).

### Object get (`get()`)

| Scale | p50 | p99 |
|-------|-----|-----|
| **M** (10 K objects) | ≤ 1 µs | ≤ 10 µs |
| **L** (100 K objects) | ≤ 1 µs | ≤ 10 µs |

`get()` is a HashMap lookup; it must be O(1) and sub-microsecond in the common case.

### Namespace list (`list()`)

| Scale | Results returned | p50 | p99 |
|-------|-----------------|-----|-----|
| **M** (10 K total, 1 K in namespace) | 1 K | ≤ 500 µs | ≤ 2 ms |
| **L** (100 K total, 10 K in namespace) | 10 K | ≤ 5 ms | ≤ 20 ms |

`list()` must use the namespace index, not a full scan.  A full scan at **L** would touch
100 K version records; the index reduces this to touching only the k live objects.

### Point-in-time query (`object_as_of()`)

| Scale | p50 | p99 |
|-------|-----|-----|
| **M** (10 K objects, 10 K versions) | ≤ 50 µs | ≤ 500 µs |

### Edge query (`edges_from()`)

| Scale | Degree | p50 | p99 |
|-------|--------|-----|-----|
| **M** (10 K objects, hub with 100 edges) | 100 | ≤ 20 µs | ≤ 150 µs |

CI gate uses 150 µs p99 (isolated runs are ~20 µs; shared CI runner adds noise).

---

## Graph SLAs

### BFS traversal (`traverse()`)

| Scale | Depth | p50 | p99 |
|-------|-------|-----|-----|
| **M** (5 K nodes, linear chain) | 3 | ≤ 100 µs | ≤ 500 µs |
| **M** (5 K nodes, linear chain) | 5 | ≤ 500 µs | ≤ 2 ms |

---

## Maintenance SLAs

### Index rebuild (`rebuild_indexes()`)

| Scale | Wall time |
|-------|-----------|
| **M** (10 K objects) | ≤ 200 ms |
| **L** (100 K objects) | ≤ 2 s |

### Cold startup / WAL replay (`open()`)

| Scale | Wall time |
|-------|-----------|
| **M** (10 K commits) | ≤ 30 ms |
| **L** (100 K commits) | ≤ 300 ms |

> **Note:** Clean `open()` after checkpoint/snapshot skips full WAL frame replay and
> defers version-log index hydration until the first history/as_of/write. Raw
> unrecompacted WAL open at XL remains slow; ops path is `compact()` then reopen
> (see issue #7 / `PERF_REPORT_XL=1`).

---

## graph_ir extended-RETURN SLAs (issue #2)

These SLAs cover the new graph_ir RETURN capabilities: zero-hop matches, arithmetic
expressions, string concatenation, edge label projection (`type(r)`), traversal depth,
and JSON array element indexing (`b.field[n]`).  Scale is 1 K nodes, hub-and-spoke
topology, depth-1 traversal.

| Feature | Scale | p99 |
|---------|-------|-----|
| Zero-hop match (`[*0..1]` start-node predicate) | 1 K nodes | ≤ 10 ms |
| Arithmetic RETURN (`b.x + b.y AS sum`) | 1 K nodes, depth 1 | ≤ 10 ms |
| `RETURN type(r)` — edge label projection | 1 K nodes, depth 1 | ≤ 10 ms |
| `RETURN depth` — traversal depth | 1 K nodes, depth 1 | ≤ 10 ms |
| `RETURN b.field[n]` — JSON array element | 1 K nodes, depth 1 | ≤ 10 ms |

---

## sql_project computed-column SLAs (issue #5)

These SLAs apply when the SELECT list includes arithmetic, string concat, or searched
CASE WHEN computed columns.  Scale is 1 K JSON-payload objects.

| Feature | Scale | p99 |
|---------|-------|-----|
| Arithmetic (`+`, `-`, `*`, `/`) | 1 K rows | ≤ 100 ms |
| String concat (`\|\|`) | 1 K rows | ≤ 100 ms |
| Searched CASE WHEN | 1 K rows | ≤ 100 ms |

---

## sql_project CASE WHEN / NTH_VALUE SLAs (issue #5)

These SLAs apply when the SELECT list contains a CASE WHEN expression or an NTH_VALUE
window function.  Scale is 1 K JSON-payload objects.

| Feature | Scale | p99 |
|---------|-------|-----|
| Simple CASE WHEN (field equality branches) | 1 K rows | ≤ 100 ms |
| NTH_VALUE window (global partition, ordered) | 1 K rows | ≤ 100 ms |

---

## graph_ir scan-MATCH SLAs (issue #2)

Applies when `graph_ir_scan_projected` iterates a namespace prefix and filters by payload
predicates.  No graph traversal — pure scan + filter + project.

| Feature | Scale | p99 |
|---------|-------|-----|
| Scan MATCH with payload filter | 1 K nodes | ≤ 10 ms |

---

## sql_project UNION SLAs (issue #5)

Applies when `sql_project_union` / `sql_project_union_parsed` merges two SELECT branches.

| Feature | Scale | p99 |
|---------|-------|-----|
| UNION ALL (two branches, 1 K rows each) | 2 K rows total | ≤ 100 ms |

---

## graph_ir multi-start fan-out SLAs (issue #2)

Applies when `WHERE id(a) IN ['hub1', 'hub2']` fans out BFS from multiple start nodes and
merges results.  Scale is 2 hubs × 500 spokes each (1 K total result rows).

| Feature | Scale | p99 |
|---------|-------|-----|
| Multi-start fan-out (2 hubs, 500 spokes each) | 1 K rows | ≤ 20 ms |

---

## sql COLLECT / STRING_AGG aggregate SLAs (issue #5)

Applies when `COLLECT(PAYLOAD_FIELD('field'))` builds JSON arrays per group, or
`STRING_AGG(PAYLOAD_FIELD('field'), 'sep')` concatenates string values per group.
Scale is 1 K observations grouped into 10 buckets (100 items per group).

| Feature | Scale | p99 |
|---------|-------|-----|
| COLLECT with GROUP BY (10 groups × 100 items) | 1 K observations | ≤ 50 ms |
| STRING_AGG with GROUP BY (10 groups × 100 items) | 1 K observations | ≤ 50 ms |

---

## graph_ir HAS / ARRAY_LENGTH SLAs (issue #2)

Applies when `WHERE b.field HAS 'val'` checks JSON array element containment, or
`RETURN ARRAY_LENGTH(b.field)` projects the element count of a JSON array field.
Scale is 1 K nodes in a hub-and-spoke topology, depth-1 traversal.

| Feature | Scale | p99 |
|---------|-------|-----|
| `WHERE b.field HAS 'val'` — array containment filter | 1 K nodes, depth 1 | ≤ 10 ms |
| `RETURN ARRAY_LENGTH(b.field)` — array element count | 1 K nodes, depth 1 | ≤ 10 ms |

---

## graph_ir P50 / P99 / STDDEV / VARIANCE WITH aggregation SLAs (issue #2)

Applies when a `WITH` aggregating clause uses `p50(b.field)`, `p99(b.field)`,
`stddev(b.field)`, or `variance(b.field)`.  Scale is 1 K nodes, hub-and-spoke,
depth-1 traversal; measures full `graph_ir_aggregated` call including traversal.

| Feature | Scale | p99 |
|---------|-------|-----|
| `WITH p50(b.field) AS m` — median aggregation | 1 K nodes, depth 1 | ≤ 10 ms |
| `WITH stddev(b.field) AS s` — population std-dev | 1 K nodes, depth 1 | ≤ 10 ms |

---

## sql STDDEV / VARIANCE aggregate SLAs (issue #5)

Applies when `SELECT STDDEV(VALUE)` or `SELECT VARIANCE(VALUE)` aggregates
binary-encoded float observations.  Scale is 1 K observations.

| Feature | Scale | p99 |
|---------|-------|-----|
| `STDDEV(VALUE)` — population std-dev | 1 K observations | ≤ 50 ms |

---

## sql_project HAS / Index SLAs (issue #5)

Applies when `WHERE PAYLOAD_FIELD('f') HAS 'val'` checks JSON array containment,
or `PAYLOAD_FIELD('f')[n]` selects the nth element of an array field in SELECT.
Scale is 1 K JSON-payload objects.

| Feature | Scale | p99 |
|---------|-------|-----|
| `WHERE PAYLOAD_FIELD('f') HAS 'val'` — array containment filter | 1 K rows | ≤ 10 ms |
| `PAYLOAD_FIELD('f')[n]` — array element index in SELECT | 1 K rows | ≤ 10 ms |

---

## Vector search SLAs (ANN / HNSW)

Applies when the `VectorIndex` has ≥ 1 K live vectors per model (below this threshold the
implementation falls back to brute-force cosine scan).

| Metric | Scale | Target |
|--------|-------|--------|
| `knn_search()` p99 (k=10) | 100 K vectors | ≤ 5 ms |
| recall@10 vs exact brute-force | 100 K vectors, dim 32 | ≥ 90% |

Rationale: semantic search over a user's full memory / document corpus (~100 K embeddings)
must return results within a single UI round-trip budget.  90% recall ensures fewer than
1 missing result per 10-result page in expectation.

---

## graph-ir UNWIND and aggregation HAVING SLAs (issue #2)

Applies when `UNWIND b.field AS item` expands array fields into rows, or when `HAVING`
filters aggregation results. Scale is 1 K nodes, hub-and-spoke topology.

| Feature | Scale | p99 |
|---------|-------|-----|
| `UNWIND b.field AS item` — array field expansion | 1 K nodes, depth 1 | ≤ 10 ms |
| `WITH count(b) AS cnt HAVING cnt > n` — post-aggregation filter | 1 K nodes, depth 1 | ≤ 10 ms |

---

## sql COUNT(DISTINCT) / PERCENTILE_DISC aggregate SLAs (issue #5)

Applies when `COUNT(DISTINCT PAYLOAD_FIELD('f'))` counts unique field values per group,
or `PERCENTILE_DISC(p) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('f'))` computes an arbitrary
percentile of a JSON field's numeric values. Scale is 1 K observations.

| Feature | Scale | p99 |
|---------|-------|-----|
| `COUNT(DISTINCT PAYLOAD_FIELD('f'))` | 1 K observations | ≤ 50 ms |
| `PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('f'))` | 1 K observations | ≤ 50 ms |

---

## Enforcement

### CI SLA gate (`perf` job)

`tests/perf_sla.rs` contains `#[ignore]` tests that assert on percentile timings.
The per-push and labeled-PR SLA gate runs M-scale tests only:

```bash
cargo test --release --test perf_sla -- --ignored --test-threads=1 \
  --skip sla_l_ --skip sla_xl_
```

L-scale and XL-scale tests run nightly via `nightly.yml`.  Separating scale tiers
prevents long-running XL builds from consuming the 20-minute per-push timeout and
from adding I/O contention that destabilises M-scale fsync measurements.

**Measurement methodology for fsync-bound tests:**
The durable `commit()` throughput test and `flush()` wall-time test use median-of-N
measurements rather than a single wall-clock sample.  fsync latency on a shared CI runner
can spike transiently due to concurrent I/O from other jobs; a single slow fsync inflates
total wall-clock but leaves the p50 (median) unaffected.  The gate fails only when the
_majority_ of samples are slow — which indicates a genuine throughput regression rather
than transient runner contention.

All SLA thresholds were originally calibrated on Apple M-series hardware with NVMe SSD.
If running on different hardware (including cloud-hosted runners), re-calibrate by running
the suite locally and adjusting the constants at the top of `tests/perf_sla.rs`.

### Trend tracking (`bench` job)

The `bench` CI job runs `cargo bench --bench store_bench` and stores results via
`benchmark-action/github-action-benchmark`.  Results are posted as PR comments
showing percentage change from the master baseline.  This job does **not** fail
the PR; it provides trend visibility.

---

## Updating SLAs

When introducing a feature that legitimately changes performance characteristics:

1. Update the targets in this document.
2. Update the corresponding constants in `tests/perf_sla.rs`.
3. Include the rationale in the PR description.
4. Update the criterion baseline if throughput improves (delete stored baseline to reset).

SLA regressions must be justified.  "It works well enough" is not sufficient — attach
profiling data and a remediation plan.

---

## Baseline hardware

SLAs were originally calibrated on Apple M-series hardware with NVMe SSD.
Absolute numbers will differ on other hardware; the ratios between operations should be
stable.  If the nightly SLA tests fail consistently, recalibrate the thresholds in
`tests/perf_sla.rs` for your CI environment.
