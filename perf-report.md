# bindb performance report

- **Overall:** `PASS`
- **git:** `a2d98ba`
- **generated:** unix 1784900810
- **schema:** `bindb.perf_report.v1`

| Status | Metric | Scale | Measured | SLA | Headroom | Unit |
|--------|--------|-------|----------|-----|----------|------|
| PASS | commit() durable throughput | M (200 commits) | 255.167 | ≥ 80.000 | +219.0% | commits/sec |
| PASS | commit_buffered()+flush throughput | M (10 K batch) | 53520.107 | ≥ 3000.000 | +1684.0% | commits/sec |
| PASS | flush() after 10 K buffered | M | 5.668 | ≤ 20.000 | +71.7% | ms |
| PASS | get() p99 | M (10 K objects) | 0.084 | ≤ 10.000 | +99.2% | µs |
| PASS | list() p99 (1 K of 10 K) | M | 0.242 | ≤ 2.000 | +87.9% | ms |
| PASS | object_as_of() p99 | M (10 K versions) | 8.791 | ≤ 500.000 | +98.2% | µs |
| PASS | edges_from() p99 (degree 100) | M | 15.458 | ≤ 150.000 | +89.7% | µs |
| PASS | traverse(depth=3) p99 | M (5 K chain) | 0.750 | ≤ 500.000 | +99.9% | µs |
| PASS | traverse(depth=5) p99 | M (5 K chain) | 0.001 | ≤ 2.000 | +99.9% | ms |
| PASS | rebuild_indexes() worst-of-5 | M (10 K) | 44.943 | ≤ 200.000 | +77.5% | ms |
| PASS | open() WAL replay worst-of-3 | M (10 K commits) | 9.133 | ≤ 30.000 | +69.6% | ms |
| PASS | open() WAL replay worst-of-3 | L (100 K commits) | 99.895 | ≤ 300.000 | +66.7% | ms |
| PASS | list() p99 (10 K of 100 K) | L | 4.587 | ≤ 20.000 | +77.1% | ms |
| PASS | HNSW recall@10 vs brute | 1.2 K vectors | 100.000 | ≥ 80.000 | +25.0% | % |

_Headroom: positive means clearer of the SLA. Negative means breached._
