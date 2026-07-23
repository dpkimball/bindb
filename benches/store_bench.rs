/// Criterion benchmark suite.
///
/// Run: cargo bench --bench store_bench
///
/// Each benchmark measures the steady-state latency of a single operation
/// against a pre-built store of varying size.  Store setup happens OUTSIDE
/// `b.iter()` so Criterion only times the operation itself.
///
/// For pass/fail SLA enforcement see tests/perf_sla.rs.
use bindb::{BinderStore, EdgeId, ObjectId, Transaction};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

fn eid(s: impl Into<String>) -> EdgeId {
    EdgeId::new(s)
}

/// Build a store with `n` single-object commits across `ns_count` namespaces.
/// All writes are buffered + flushed so setup is fast.
fn build_store(dir: &TempDir, n: usize, ns_count: usize) -> BinderStore {
    let store = BinderStore::open(dir.path()).unwrap();
    for i in 0..n {
        let ns = i % ns_count;
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("ns{ns}://obj.{i:08}")), vec![i as u8 % 255; 64]);
        store.commit_buffered(tx, "bench", "").unwrap();
    }
    store.flush().unwrap();
    store
}

// ── durable commit throughput ─────────────────────────────────────────────────
// Measures a single durable commit() (WAL write + fsync) in steady state.
// Store is pre-built with n objects so we see how WAL size affects commit time.

fn bench_commit_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("commit_throughput");
    group.throughput(Throughput::Elements(1));

    for existing_n in [0usize, 1_000, 10_000] {
        let dir = tmp();
        let store = build_store(&dir, existing_n, 1);
        // Counter starts after the pre-built objects so keys don't collide.
        let mut counter = existing_n;

        group.bench_with_input(
            BenchmarkId::new("single_object_durable", existing_n),
            &existing_n,
            |b, _| {
                b.iter(|| {
                    let mut tx = Transaction::new();
                    tx.put_object(obj(format!("a://o{counter}")), vec![0u8; 64]);
                    store.commit(tx, "bench", "").unwrap();
                    counter += 1;
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("ten_objects_durable", existing_n),
            &existing_n,
            |b, _| {
                b.iter(|| {
                    let mut tx = Transaction::new();
                    for j in 0..10usize {
                        tx.put_object(obj(format!("a://o{counter}_{j}")), vec![0u8; 64]);
                    }
                    store.commit(tx, "bench", "").unwrap();
                    counter += 1;
                });
            },
        );
    }

    group.finish();
}

// ── buffered commit + flush throughput ────────────────────────────────────────
// Measures a fixed batch of buffered commits followed by a flush.
// Each iteration commits BATCH_SIZE objects to an existing store then fsyncs.

fn bench_commit_buffered_throughput(c: &mut Criterion) {
    const BATCH_SIZE: usize = 500;
    let mut group = c.benchmark_group("commit_buffered_throughput");
    group.throughput(Throughput::Elements(BATCH_SIZE as u64));

    for existing_n in [0usize, 10_000, 100_000] {
        let dir = tmp();
        let store = build_store(&dir, existing_n, 1);
        let mut counter = existing_n;

        group.bench_with_input(
            BenchmarkId::new("batch_500_plus_flush", existing_n),
            &existing_n,
            |b, _| {
                b.iter(|| {
                    for _ in 0..BATCH_SIZE {
                        let mut tx = Transaction::new();
                        tx.put_object(obj(format!("a://o{counter}")), vec![0u8; 64]);
                        store.commit_buffered(tx, "bench", "").unwrap();
                        counter += 1;
                    }
                    store.flush().unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── flush wall time ───────────────────────────────────────────────────────────
// Measures only the flush() call after accumulating n buffered commits.
// Uses iter_custom so only the flush is timed (not the buffered writes).

fn bench_flush(c: &mut Criterion) {
    let mut group = c.benchmark_group("flush");

    for n in [1_000usize, 10_000] {
        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(
            BenchmarkId::new("flush_after_n_buffered_commits", n),
            &n,
            |b, &n| {
                b.iter_custom(|iters| {
                    let mut total = std::time::Duration::ZERO;
                    for iter in 0..iters {
                        let dir = tmp();
                        let store = BinderStore::open(dir.path()).unwrap();
                        for i in 0..n {
                            let mut tx = Transaction::new();
                            tx.put_object(obj(format!("a://o{iter}_{i}")), vec![0u8; 64]);
                            store.commit_buffered(tx, "bench", "").unwrap();
                        }
                        let t = std::time::Instant::now();
                        store.flush().unwrap();
                        total += t.elapsed();
                    }
                    total
                });
            },
        );
    }

    group.finish();
}

// ── get latency ───────────────────────────────────────────────────────────────

fn bench_get_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_latency");
    group.throughput(Throughput::Elements(1));

    for n in [1_000usize, 10_000, 100_000] {
        let dir = tmp();
        let store = build_store(&dir, n, 1);
        let target = obj(format!("ns0://obj.{:08}", n / 2));

        group.bench_with_input(BenchmarkId::new("head_get", n), &n, |b, _| {
            b.iter(|| {
                let _ = store.get(&target).unwrap();
            });
        });
    }

    group.finish();
}

// ── namespace list ────────────────────────────────────────────────────────────

fn bench_list_by_namespace(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_by_namespace");

    for (total, ns_count) in [(10_000, 10), (100_000, 10)] {
        let results_per_ns = total / ns_count;
        let dir = tmp();
        let store = build_store(&dir, total, ns_count);

        group.throughput(Throughput::Elements(results_per_ns as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("list_{results_per_ns}_of_{total}"), total),
            &total,
            |b, _| {
                b.iter(|| {
                    let results = store.list("ns0://").unwrap();
                    assert_eq!(results.len(), results_per_ns);
                });
            },
        );
    }

    group.finish();
}

// ── object_as_of (point-in-time) ──────────────────────────────────────────────

fn bench_object_as_of(c: &mut Criterion) {
    let mut group = c.benchmark_group("object_as_of");
    group.throughput(Throughput::Elements(1));

    for n_versions in [1_000usize, 10_000] {
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();
        let oid = obj("a://versioned_obj");

        let mut mid_commit = None;
        for i in 0..n_versions {
            let mut tx = Transaction::new();
            tx.put_object(oid.clone(), vec![i as u8 % 255; 32]);
            let cid = store.commit_buffered(tx, "bench", "").unwrap();
            if i == n_versions / 2 {
                mid_commit = Some(cid);
            }
        }
        store.flush().unwrap();
        let at = mid_commit.unwrap();

        group.bench_with_input(
            BenchmarkId::new("midpoint_lookup", n_versions),
            &n_versions,
            |b, _| {
                b.iter(|| {
                    let _ = store.object_as_of(&oid, at).unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── edges_from ────────────────────────────────────────────────────────────────

fn bench_edges_from(c: &mut Criterion) {
    let mut group = c.benchmark_group("edges_from");

    for degree in [10usize, 100, 1_000] {
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();
        let hub = obj("a://hub");

        let mut tx = Transaction::new();
        tx.put_object(hub.clone(), vec![0u8; 32]);
        for i in 0..degree {
            let spoke = obj(format!("a://spoke.{i}"));
            tx.put_object(spoke.clone(), vec![i as u8 % 255; 32]);
            tx.put_edge(eid(format!("e{i}")), hub.clone(), spoke, "LINK", None);
        }
        store.commit(tx, "bench", "").unwrap();

        group.throughput(Throughput::Elements(degree as u64));
        group.bench_with_input(BenchmarkId::new("hub_degree", degree), &degree, |b, &d| {
            b.iter(|| {
                let edges = store.edges_from(&hub).unwrap();
                assert_eq!(edges.len(), d);
            });
        });
    }

    group.finish();
}

// ── graph traversal ───────────────────────────────────────────────────────────

fn bench_graph_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_traversal");

    for n in [1_000usize, 5_000] {
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();

        let mut tx = Transaction::new();
        for i in 0..n {
            tx.put_object(obj(format!("a://n{i}")), vec![0u8; 16]);
        }
        store.commit(tx, "bench", "nodes").unwrap();

        for i in 0..n - 1 {
            let mut tx = Transaction::new();
            tx.put_edge(
                eid(format!("e{i}")),
                obj(format!("a://n{i}")),
                obj(format!("a://n{}", i + 1)),
                "LINK",
                None,
            );
            store.commit_buffered(tx, "bench", "edge").unwrap();
        }
        store.flush().unwrap();

        let start = obj("a://n0");

        for depth in [3usize, 5] {
            group.throughput(Throughput::Elements(depth as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("linear_{n}_nodes_depth_{depth}"), n),
                &n,
                |b, _| {
                    b.iter(|| {
                        let _ = store.traverse(&start, depth).unwrap();
                    });
                },
            );
        }
    }

    group.finish();
}

// ── index rebuild ─────────────────────────────────────────────────────────────

fn bench_index_rebuild(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_rebuild");

    for n in [1_000usize, 10_000, 100_000] {
        let dir = tmp();
        let store = build_store(&dir, n, 5);

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("rebuild_all_indexes", n), &n, |b, _| {
            b.iter(|| {
                store.rebuild_indexes().unwrap();
            });
        });
    }

    group.finish();
}

// ── cold startup (WAL replay) ─────────────────────────────────────────────────

fn bench_cold_startup(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_startup");

    for n in [1_000usize, 10_000] {
        let dir = tmp();
        {
            let store = build_store(&dir, n, 5);
            store.close().unwrap();
        }

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(
            BenchmarkId::new("reopen_after_clean_close", n),
            &n,
            |b, _| {
                b.iter(|| {
                    let store = BinderStore::open(dir.path()).unwrap();
                    let _ = store.head().unwrap();
                    store.close().unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_commit_throughput,
    bench_commit_buffered_throughput,
    bench_flush,
    bench_get_latency,
    bench_list_by_namespace,
    bench_object_as_of,
    bench_edges_from,
    bench_graph_traversal,
    bench_index_rebuild,
    bench_cold_startup,
);
criterion_main!(benches);
