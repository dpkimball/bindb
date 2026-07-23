/// Phase 13.9 — Benchmarks.
/// Targets from spec:
///   - Commit throughput at 10K / 100K objects
///   - get() latency (HEAD index lookup)
///   - Graph traversal at 10K / 100K objects
///   - Full index rebuild time
///   - Cold startup with checkpoint vs without
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

// ── commit throughput ────────────────────────────────────────────────────────

fn bench_commit_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("commit_throughput");

    for n in [100usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(
            BenchmarkId::new("single_object_per_commit", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let dir = tmp();
                    let store = BinderStore::open(dir.path()).unwrap();
                    for i in 0..n {
                        let mut tx = Transaction::new();
                        tx.put_object(obj(format!("o{i}")), vec![0u8; 64]);
                        store.commit(tx, "bench", "").unwrap();
                    }
                    store.close().unwrap();
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("ten_objects_per_commit", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let dir = tmp();
                    let store = BinderStore::open(dir.path()).unwrap();
                    for i in 0..n {
                        let mut tx = Transaction::new();
                        for j in 0..10usize {
                            tx.put_object(obj(format!("o{i}_{j}")), vec![0u8; 64]);
                        }
                        store.commit(tx, "bench", "").unwrap();
                    }
                    store.close().unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── get latency ──────────────────────────────────────────────────────────────

fn bench_get_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("get_latency");

    for n in [1_000usize, 10_000, 100_000] {
        // Setup: pre-populate store.
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("o{i}")), vec![i as u8 % 255; 32]);
            store.commit(tx, "bench", "").unwrap();
        }

        let lookup_id = obj(format!("o{}", n / 2));

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("head_get", n), &n, |b, _| {
            b.iter(|| {
                let _ = store.get(&lookup_id).unwrap();
            });
        });
    }

    group.finish();
}

// ── graph traversal ──────────────────────────────────────────────────────────

fn bench_graph_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_traversal");

    for n in [100usize, 1_000, 5_000] {
        // Build a linear chain of n nodes.
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();

        let mut tx = Transaction::new();
        for i in 0..n {
            tx.put_object(obj(format!("n{i}")), vec![0u8; 16]);
        }
        store.commit(tx, "bench", "nodes").unwrap();

        for i in 0..n - 1 {
            let mut tx = Transaction::new();
            tx.put_edge(
                eid(format!("e{i}")),
                obj(format!("n{i}")),
                obj(format!("n{}", i + 1)),
                "LINK",
                None,
            );
            store.commit(tx, "bench", "edge").unwrap();
        }

        let start = obj("n0");

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(
            BenchmarkId::new("linear_chain_full_depth", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let reachable = store.traverse(&start, n).unwrap();
                    assert_eq!(reachable.len(), n - 1);
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("linear_chain_depth_3", n), &n, |b, _| {
            b.iter(|| {
                let _ = store.traverse(&start, 3).unwrap();
            });
        });
    }

    group.finish();
}

// ── index rebuild ────────────────────────────────────────────────────────────

fn bench_index_rebuild(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_rebuild");

    for n in [1_000usize, 10_000] {
        let dir = tmp();
        let store = BinderStore::open(dir.path()).unwrap();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("o{i}")), vec![0u8; 64]);
            store.commit(tx, "bench", "").unwrap();
        }

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("rebuild_all_indexes", n), &n, |b, _| {
            b.iter(|| {
                store.rebuild_indexes().unwrap();
            });
        });
    }

    group.finish();
}

// ── cold startup ─────────────────────────────────────────────────────────────

fn bench_cold_startup(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_startup");

    for n in [1_000usize, 10_000] {
        // Build the store once, then close it (writes a checkpoint).
        let dir = tmp();
        {
            let store = BinderStore::open(dir.path()).unwrap();
            for i in 0..n {
                let mut tx = Transaction::new();
                tx.put_object(obj(format!("o{i}")), vec![0u8; 64]);
                store.commit(tx, "bench", "").unwrap();
            }
            store.close().unwrap(); // writes checkpoint
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

criterion_group!(
    benches,
    bench_commit_throughput,
    bench_get_latency,
    bench_graph_traversal,
    bench_index_rebuild,
    bench_cold_startup,
);
criterion_main!(benches);
