/// One-shot measurement script — prints the numbers that go into
/// docs/benchmark-comparison.md.  Not a permanent fixture.
///
/// Run: cargo run --example measure --release
use bindb::{BinderStore, EdgeId, ObjectId, Transaction};
use std::time::Instant;
use tempfile::tempdir;

fn obj(s: impl Into<String>) -> ObjectId { ObjectId::new(s) }
fn eid(s: impl Into<String>) -> EdgeId   { EdgeId::new(s) }

fn percentile(v: &mut Vec<u64>, p: f64) -> u64 {
    v.sort_unstable();
    v[((v.len() as f64 * p / 100.0) as usize).min(v.len() - 1)]
}

fn main() {
    println!("bindb measurements — release build, Apple M-series NVMe");
    println!("{}", "─".repeat(60));

    // ── 1. Durable commit throughput ─────────────────────────────────
    {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 500usize;
        let t = Instant::now();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit(tx, "m", "").unwrap();
        }
        let secs = t.elapsed().as_secs_f64();
        println!("Durable commit (commit+fsync):   {:>8.0} writes/sec   ({n} × 64-byte object)", n as f64 / secs);
    }

    // ── 2. Buffered commit throughput — at multiple scales ───────────
    for &n in &[1_000usize, 10_000, 50_000] {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let t = Instant::now();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit_buffered(tx, "m", "").unwrap();
        }
        store.flush().unwrap();
        let secs = t.elapsed().as_secs_f64();
        println!("Buffered commit ({n:>6} batch):   {:>8.0} writes/sec   (1 flush)", n as f64 / secs);
    }

    // ── 3. get() latency ─────────────────────────────────────────────
    {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 10_000usize;
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i:06}")), vec![i as u8 % 255; 64]);
            store.commit_buffered(tx, "m", "").unwrap();
        }
        store.flush().unwrap();
        let target = obj(format!("a://o{:06}", n / 2));
        let iters = 10_000usize;
        let mut ts: Vec<u64> = (0..iters).map(|_| {
            let t = Instant::now();
            let _ = store.get(&target).unwrap();
            t.elapsed().as_nanos() as u64
        }).collect();
        println!("get() p50={} ns  p99={} ns  p999={} ns  ({n} objects in store)",
            percentile(&mut ts, 50.0),
            percentile(&mut ts, 99.0),
            percentile(&mut ts, 99.9));
    }

    // ── 4. list() namespace scan ──────────────────────────────────────
    {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let total = 10_000usize;
        let ns_count = 10usize;
        for i in 0..total {
            let ns = i % ns_count;
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("ns{ns}://o{i:08}")), vec![0u8; 64]);
            store.commit_buffered(tx, "m", "").unwrap();
        }
        store.flush().unwrap();
        let iters = 2_000usize;
        let mut ts: Vec<u64> = (0..iters).map(|_| {
            let t = Instant::now();
            let r = store.list("ns0://").unwrap();
            assert_eq!(r.len(), total / ns_count);
            t.elapsed().as_nanos() as u64
        }).collect();
        println!("list() p50={} µs  p99={} µs  p999={} µs  ({} results of {} total)",
            percentile(&mut ts, 50.0) / 1000,
            percentile(&mut ts, 99.0) / 1000,
            percentile(&mut ts, 99.9) / 1000,
            total / ns_count, total);
    }

    // ── 5. BFS traverse ───────────────────────────────────────────────
    {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 5_000usize;
        let mut tx = Transaction::new();
        for i in 0..n { tx.put_object(obj(format!("a://n{i}")), vec![0u8; 16]); }
        store.commit(tx, "m", "").unwrap();
        for i in 0..n-1 {
            let mut tx = Transaction::new();
            tx.put_edge(eid(format!("e{i}")), obj(format!("a://n{i}")), obj(format!("a://n{}", i+1)), "NEXT", None);
            store.commit_buffered(tx, "m", "").unwrap();
        }
        store.flush().unwrap();
        let start = obj("a://n0");
        let iters = 2_000usize;
        let mut ts3: Vec<u64> = (0..iters).map(|_| {
            let t = Instant::now();
            let _ = store.traverse(&start, 3).unwrap();
            t.elapsed().as_nanos() as u64
        }).collect();
        let mut ts5: Vec<u64> = (0..iters).map(|_| {
            let t = Instant::now();
            let _ = store.traverse(&start, 5).unwrap();
            t.elapsed().as_nanos() as u64
        }).collect();
        println!("traverse(depth=3) p50={} µs  p99={} µs  ({n} node linear chain)",
            percentile(&mut ts3, 50.0) / 1000,
            percentile(&mut ts3, 99.0) / 1000);
        println!("traverse(depth=5) p50={} µs  p99={} µs  ({n} node linear chain)",
            percentile(&mut ts5, 50.0) / 1000,
            percentile(&mut ts5, 99.0) / 1000);
    }

    // ── 6. WAL replay / open() ────────────────────────────────────────
    {
        let dir = tempdir().unwrap();
        {
            let store = BinderStore::open(dir.path()).unwrap();
            for i in 0..10_000usize {
                let mut tx = Transaction::new();
                tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
                store.commit_buffered(tx, "m", "").unwrap();
            }
            store.flush().unwrap();
            store.close().unwrap();
        }
        let times: Vec<u64> = (0..5).map(|_| {
            let t = Instant::now();
            let store = BinderStore::open(dir.path()).unwrap();
            let ms = t.elapsed().as_millis() as u64;
            store.close().unwrap();
            ms
        }).collect();
        let mean = times.iter().sum::<u64>() / times.len() as u64;
        let max  = *times.iter().max().unwrap();
        println!("open()+WAL replay mean={mean} ms  max={max} ms  (10 K commits)");
    }

    // ── 7. rebuild_indexes() ─────────────────────────────────────────
    {
        let dir = tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        for i in 0..10_000usize {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit_buffered(tx, "m", "").unwrap();
        }
        store.flush().unwrap();
        store.rebuild_indexes().unwrap(); // warm
        let times: Vec<u64> = (0..5).map(|_| {
            let t = Instant::now();
            store.rebuild_indexes().unwrap();
            t.elapsed().as_millis() as u64
        }).collect();
        let mean = times.iter().sum::<u64>() / times.len() as u64;
        let max  = *times.iter().max().unwrap();
        println!("rebuild_indexes() mean={mean} ms  max={max} ms  (10 K objects)");
    }

    println!("{}", "─".repeat(60));
}
