# bindb — Performance SLAs

This document defines the performance contracts for bindb operations.  SLAs are enforced
automatically on every PR via the `perf` CI job (`tests/perf_sla.rs`).  Trend data is
stored per-run and compared across PRs via Criterion benchmarks (`benches/store_bench.rs`).

---

## Scale tiers

All SLAs are defined at one or more scale tiers:

| Tier | Objects in store | Edges | Versions per object |
|------|-----------------|-------|---------------------|
| **S** | 1 K | 1 K | 1 (append-only load) |
| **M** | 10 K | 10 K | 1 |
| **L** | 100 K | 50 K | 1 |
| **XL** | 1 M | — | — | (aspirational; not yet in CI) |

Life-scale context: a single user accumulates ~500 K health observations per year,
~50 K memories over 10 years, and ~10 K–100 K financial records.
The **L** tier represents a well-used store after 2–3 years.

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
| Throughput at **M** (10 K batch, CI floor) | ≥ 3 K commits/sec |
| Throughput at **M** (10 K batch, isolated peak) | ≥ 5.5 K commits/sec |
| `flush()` wall time after 10 K buffered commits | ≤ 20 ms |

Rationale: Apple Health import (500 K data points) must complete in < 2 minutes.
5 K commits/sec means 500 K points takes ~100 seconds.

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
| **M** (10 K objects, hub with 100 edges) | 100 | ≤ 20 µs | ≤ 100 µs |

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

> **Note:** WAL replay is now O(n) after the position-index fix (previously O(n²)).
> M-tier measured at ~14 ms.  Phase 2 checkpoint-based open will reduce this further
> by avoiding full deserialization of all historical commits.

---

## Enforcement

### CI SLA gate (`perf` job)

`tests/perf_sla.rs` contains `#[ignore]` tests that assert on percentile timings.
The `perf` CI job runs them with:

```bash
cargo test --release -- --ignored --test-threads=1
```

These tests run at **M** scale (10 K objects) with enough iterations (1 K+) to produce
stable percentile estimates.  They fail the PR if any SLA is violated.

All SLA thresholds in the test file are calibrated for the project's self-hosted CI runner.
If the runner changes, re-calibrate by running the suite locally on the new hardware and
adjusting the constants at the top of `tests/perf_sla.rs`.

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

SLAs are calibrated for the project's self-hosted CI runner (Apple M-series, NVMe SSD).
Absolute numbers will differ on other hardware; the ratios between operations should be
stable.  Cloud-hosted GitHub runners are significantly slower — do not use them as the
SLA gate machine.
