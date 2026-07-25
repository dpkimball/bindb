/// Performance SLA gate.
///
/// Run with: cargo test --release -- --ignored --test-threads=1
///
/// Each test asserts that a specific operation meets the latency or throughput
/// target defined in docs/sla.md.  Tests run at M-scale (10 K objects) by
/// default; L-scale tests (100 K objects) are tagged with `scale:L` in their
/// ignore string and are expected to run on a nightly/manual schedule.
/// Thresholds are calibrated for the project's self-hosted CI runner
/// (Apple M-series, NVMe SSD).  Adjust the constants below if the runner changes.
///
/// Adding a new SLA test:
///   1. Add the target to docs/sla.md.
///   2. Write the test here with a matching threshold constant.
///   3. Use `#[ignore = "perf: ..."]` so it is excluded from the default test run.
use bindb::{BinderStore, EdgeId, ObjectId, Transaction, VectorEntry, VectorFilter, VectorIndex};
use std::collections::HashSet;
use std::time::Instant;
use tempfile::TempDir;

// ── M-scale threshold constants (10 K objects) ───────────────────────────────
// Adjust these if the self-hosted CI runner changes.

const COMMIT_THROUGHPUT_MIN: u64 = 80; // commits/sec — durable single-object commit
const COMMIT_BUFFERED_THROUGHPUT_MIN: u64 = 8_000; // commits/sec — buffered batch (OKR O2/KR2.1; measured ~260K+)
const FLUSH_WALL_MS_MAX: u64 = 20; // ms — flush() after 10 K buffered commits

const GET_P99_US_MAX: u64 = 10; // µs — HEAD lookup at M scale
const LIST_P99_MS_MAX: u64 = 2; // ms — namespace list returning 1 K results
const OBJECT_AS_OF_P99_US_MAX: u64 = 500; // µs — point-in-time at M scale
const EDGES_FROM_P99_US_MAX: u64 = 150; // µs — outgoing edge query at M scale (CI floor; isolated ~20 µs)

const TRAVERSE_DEPTH3_P99_US_MAX: u64 = 500; // µs — BFS depth 3
const TRAVERSE_DEPTH5_P99_MS_MAX: u64 = 2; // ms — BFS depth 5

const REBUILD_M_MS_MAX: u64 = 200; // ms — rebuild_indexes() at 10 K objects
const OPEN_REPLAY_M_MS_MAX: u64 = 30; // ms — WAL replay at 10 K commits (O(n) position-index replay)

// ── bulk ingest threshold constants ──────────────────────────────────────────
const COMMIT_BUFFERED_50K_THROUGHPUT_MIN: u64 = 5_000; // commits/sec — 50 K batch (OKR O2/KR2.2)
/// 500 K single-field observation import must complete in ≤ 90 s (OKR O2/KR2.3).
/// Apple Health import scenario: one object per data point, 64-byte payload.
const HEALTH_IMPORT_500K_S_MAX: f64 = 90.0;

// ── vector / ANN threshold constants ─────────────────────────────────────────
const HNSW_RECALL_100K_PMIN: f64 = 90.0; // % — recall@10 at 100 K vectors (OKR O3/KR3.2)
const VECTOR_SEARCH_P99_MS_MAX: u64 = 5; // ms — knn_search p99 at 100 K vectors (OKR O3/KR3.1)

// ── L-scale threshold constants (100 K objects) ──────────────────────────────
// get() and edges_from() are O(1) HashMap lookups — same target as M.
// list(), rebuild_indexes(), and open() scale linearly with n.

const GET_L_P99_US_MAX: u64 = 10; // µs — HashMap lookup is O(1); same target as M
const LIST_L_P99_MS_MAX: u64 = 20; // ms — namespace list returning 10 K of 100 K results
const REBUILD_L_MS_MAX: u64 = 2_000; // ms — rebuild_indexes() at 100 K objects
const OPEN_REPLAY_L_MS_MAX: u64 = 300; // ms — WAL replay at 100 K commits

// ── XL-scale threshold constants (1 M objects) ───────────────────────────────
// O(1) operations stay constant; open() requires compact() first (bare WAL
// replay at 1 M is O(n) but slow without the position index optimisation).

const GET_XL_P99_US_MAX: u64 = 10; // µs — HashMap lookup stays O(1) at 1 M
const OPEN_XL_AFTER_COMPACT_MS_MAX: u64 = 5_000; // ms — snapshot replay at 1 M
const LIST_XL_P99_MS_MAX: u64 = 500; // ms — namespace list returning 100 K of 1 M
const COMMIT_XL_BUFFERED_MIN: u64 = 5_000; // commits/sec — buffered writes must not degrade at XL
const REBUILD_XL_MS_MAX: u64 = 60_000; // ms — rebuild_indexes() at 1 M objects (O(n) WAL replay)

// ── helpers ───────────────────────────────────────────────────────────────────

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

fn eid(s: impl Into<String>) -> EdgeId {
    EdgeId::new(s)
}

/// p-th percentile (0–100) of a sorted timing slice (nanoseconds).
fn percentile(timings: &mut [u64], p: f64) -> u64 {
    timings.sort_unstable();
    let idx = ((timings.len() as f64 * p / 100.0) as usize).min(timings.len() - 1);
    timings[idx]
}

/// Build a store with `n` objects spread across `namespaces` namespaces.
fn build_store_m(dir: &TempDir, n: usize, namespaces: &[&str]) -> BinderStore {
    let store = BinderStore::open(dir.path()).unwrap();
    for i in 0..n {
        let ns = namespaces[i % namespaces.len()];
        let mut tx = Transaction::new();
        tx.put_object(
            obj(format!("{}://obj.{:06}", ns, i)),
            vec![i as u8 % 255; 64],
        );
        store.commit_buffered(tx, "test", "").unwrap();
    }
    store.flush().unwrap();
    store
}

// ── write SLAs ────────────────────────────────────────────────────────────────

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_commit_throughput_gte_80_per_sec() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 200usize;

    let start = Instant::now();
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
        store.commit(tx, "test", "").unwrap();
    }
    let elapsed = start.elapsed();

    let throughput = n as f64 / elapsed.as_secs_f64();
    assert!(
        throughput >= COMMIT_THROUGHPUT_MIN as f64,
        "commit() throughput {:.0} commits/sec < SLA minimum {} commits/sec",
        throughput,
        COMMIT_THROUGHPUT_MIN
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_commit_buffered_throughput_gte_5k_per_sec() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 10_000usize;

    let start = Instant::now();
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
        store.commit_buffered(tx, "test", "").unwrap();
    }
    store.flush().unwrap();
    let elapsed = start.elapsed();

    let throughput = n as f64 / elapsed.as_secs_f64();
    assert!(
        throughput >= COMMIT_BUFFERED_THROUGHPUT_MIN as f64,
        "commit_buffered() throughput {:.0} commits/sec < SLA minimum {} commits/sec",
        throughput,
        COMMIT_BUFFERED_THROUGHPUT_MIN
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_flush_wall_time_lte_20ms_after_10k_buffered() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();

    for i in 0..10_000usize {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
        store.commit_buffered(tx, "test", "").unwrap();
    }

    let start = Instant::now();
    store.flush().unwrap();
    let ms = start.elapsed().as_millis() as u64;

    assert!(
        ms <= FLUSH_WALL_MS_MAX,
        "flush() after 10 K buffered commits took {} ms > SLA maximum {} ms",
        ms,
        FLUSH_WALL_MS_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_commit_buffered_50k_throughput_gte_5k_per_sec() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 50_000usize;

    let start = Instant::now();
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
        store.commit_buffered(tx, "test", "").unwrap();
    }
    store.flush().unwrap();
    let elapsed = start.elapsed();

    let throughput = n as f64 / elapsed.as_secs_f64();
    assert!(
        throughput >= COMMIT_BUFFERED_50K_THROUGHPUT_MIN as f64,
        "commit_buffered() 50 K throughput {:.0} commits/sec < SLA minimum {} commits/sec",
        throughput,
        COMMIT_BUFFERED_50K_THROUGHPUT_MIN
    );
}

// ── read SLAs ─────────────────────────────────────────────────────────────────

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_get_p99_lte_10us_at_10k_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_m(&dir, 10_000, &["a"]);
    let target = obj("a://obj.005000");
    let iterations = 5_000;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.get(&target).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= GET_P99_US_MAX,
        "get() p99 {} µs > SLA maximum {} µs",
        p99,
        GET_P99_US_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_list_p99_lte_2ms_returning_1k_of_10k() {
    let dir = tempfile::tempdir().unwrap();
    // 10 namespaces × 1 K objects each = 10 K total; list("ns0://") returns 1 K.
    let namespaces: Vec<String> = (0..10).map(|i| format!("ns{}", i)).collect();
    let ns_refs: Vec<&str> = namespaces.iter().map(String::as_str).collect();
    let store = build_store_m(&dir, 10_000, &ns_refs);
    let iterations = 500;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let results = store.list("ns0://").unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
        assert_eq!(results.len(), 1_000);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000_000; // ns → ms
    assert!(
        p99 <= LIST_P99_MS_MAX,
        "list() p99 {} ms > SLA maximum {} ms",
        p99,
        LIST_P99_MS_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_object_as_of_p99_lte_500us_at_10k_versions() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("a://obj.target");

    // Write 10 K versions of the same object.
    let mut mid_commit = None;
    for i in 0..10_000usize {
        let mut tx = Transaction::new();
        tx.put_object(oid.clone(), vec![i as u8 % 255; 32]);
        let cid = store.commit_buffered(tx, "test", "").unwrap();
        if i == 5_000 {
            mid_commit = Some(cid);
        }
    }
    store.flush().unwrap();
    let at = mid_commit.unwrap();
    let iterations = 1_000;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.object_as_of(&oid, at).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= OBJECT_AS_OF_P99_US_MAX,
        "object_as_of() p99 {} µs > SLA maximum {} µs",
        p99,
        OBJECT_AS_OF_P99_US_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_edges_from_p99_lte_150us_hub_with_100_edges() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();

    let hub = obj("a://hub");
    let mut setup_tx = Transaction::new();
    setup_tx.put_object(hub.clone(), vec![0u8; 32]);
    for i in 0..100usize {
        let spoke = obj(format!("a://spoke.{i}"));
        setup_tx.put_object(spoke.clone(), vec![i as u8; 32]);
        setup_tx.put_edge(eid(format!("e{i}")), hub.clone(), spoke, "LINK", None);
    }
    store.commit(setup_tx, "test", "").unwrap();

    let iterations = 2_000;
    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let edges = store.edges_from(&hub).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
        assert_eq!(edges.len(), 100);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= EDGES_FROM_P99_US_MAX,
        "edges_from() p99 {} µs > SLA maximum {} µs",
        p99,
        EDGES_FROM_P99_US_MAX
    );
}

// ── graph SLAs ────────────────────────────────────────────────────────────────

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_traverse_depth3_p99_lte_500us_at_5k_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 5_000usize;

    // Linear chain: n0 → n1 → n2 → ... → n4999
    let mut tx = Transaction::new();
    for i in 0..n {
        tx.put_object(obj(format!("a://n{i}")), vec![0u8; 16]);
    }
    store.commit(tx, "test", "nodes").unwrap();

    for i in 0..n - 1 {
        let mut tx = Transaction::new();
        tx.put_edge(
            eid(format!("e{i}")),
            obj(format!("a://n{i}")),
            obj(format!("a://n{}", i + 1)),
            "NEXT",
            None,
        );
        store.commit_buffered(tx, "test", "edges").unwrap();
    }
    store.flush().unwrap();

    let start_node = obj("a://n0");
    let iterations = 1_000;
    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.traverse(&start_node, 3).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= TRAVERSE_DEPTH3_P99_US_MAX,
        "traverse(depth=3) p99 {} µs > SLA maximum {} µs",
        p99,
        TRAVERSE_DEPTH3_P99_US_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_traverse_depth5_p99_lte_2ms_at_5k_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 5_000usize;

    let mut tx = Transaction::new();
    for i in 0..n {
        tx.put_object(obj(format!("a://n{i}")), vec![0u8; 16]);
    }
    store.commit(tx, "test", "nodes").unwrap();

    for i in 0..n - 1 {
        let mut tx = Transaction::new();
        tx.put_edge(
            eid(format!("e{i}")),
            obj(format!("a://n{i}")),
            obj(format!("a://n{}", i + 1)),
            "NEXT",
            None,
        );
        store.commit_buffered(tx, "test", "edges").unwrap();
    }
    store.flush().unwrap();

    let start_node = obj("a://n0");
    let iterations = 500;
    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.traverse(&start_node, 5).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000_000; // ns → ms
    assert!(
        p99 <= TRAVERSE_DEPTH5_P99_MS_MAX,
        "traverse(depth=5) p99 {} ms > SLA maximum {} ms",
        p99,
        TRAVERSE_DEPTH5_P99_MS_MAX
    );
}

// ── maintenance SLAs ──────────────────────────────────────────────────────────

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_rebuild_indexes_lte_200ms_at_10k_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_m(&dir, 10_000, &["a", "b", "c"]);

    // Warm run first, then measure 5 times and take the max.
    store.rebuild_indexes().unwrap();

    let mut worst_ms = 0u64;
    for _ in 0..5 {
        let t = Instant::now();
        store.rebuild_indexes().unwrap();
        let ms = t.elapsed().as_millis() as u64;
        worst_ms = worst_ms.max(ms);
    }

    assert!(
        worst_ms <= REBUILD_M_MS_MAX,
        "rebuild_indexes() worst-of-5 {} ms > SLA maximum {} ms",
        worst_ms,
        REBUILD_M_MS_MAX
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_open_wal_replay_lte_500ms_at_10k_commits() {
    // Build a store, close it cleanly, then measure re-open time.
    let dir = tempfile::tempdir().unwrap();
    {
        let store = build_store_m(&dir, 10_000, &["a"]);
        store.close().unwrap();
    }

    let mut worst_ms = 0u64;
    for _ in 0..3 {
        let t = Instant::now();
        let store = BinderStore::open(dir.path()).unwrap();
        let ms = t.elapsed().as_millis() as u64;
        worst_ms = worst_ms.max(ms);
        store.close().unwrap();
    }

    assert!(
        worst_ms <= OPEN_REPLAY_M_MS_MAX,
        "open() WAL replay worst-of-3 {} ms > SLA maximum {} ms",
        worst_ms,
        OPEN_REPLAY_M_MS_MAX
    );
}

// ── vector / ANN SLAs ────────────────────────────────────────────────────────

/// LCG pseudo-random unit vector at dim=32 (deterministic, no rand dependency).
fn rand_unit_vec_32(seed: &mut u64) -> Vec<f32> {
    let mut v = vec![0.0f32; 32];
    let mut sq = 0.0f32;
    for x in &mut v {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let f = (*seed >> 33) as f32 / 0x7fff_ffff_u32 as f32 - 1.0;
        *x = f;
        sq += f * f;
    }
    let norm = sq.sqrt().max(1e-8);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn make_vector_entry(i: usize, v: Vec<f32>) -> VectorEntry {
    VectorEntry {
        object_id: format!("v://{i}"),
        object_version_id: format!("v://{i}-v1"),
        chunk_id: format!("c{i}"),
        chunk_index: 0,
        model_id: "sla".into(),
        dimensions: v.len() as u32,
        vector: v,
        content_hash: [0u8; 32],
        source_text: String::new(),
        binder_id: None,
        app_id: None,
    }
}

/// Build a 100 K HNSW index and return it along with the raw vectors.
fn build_hnsw_100k() -> (VectorIndex, Vec<Vec<f32>>) {
    let n = 100_000usize;
    let mut seed = 0xDEAD_BEEF_1234_u64;
    let mut idx = VectorIndex::new();
    let mut raw = Vec::with_capacity(n);
    for i in 0..n {
        let v = rand_unit_vec_32(&mut seed);
        raw.push(v.clone());
        idx.insert(make_vector_entry(i, v));
    }
    (idx, raw)
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_hnsw_recall_at_10_gte_90pct_at_100k_vectors() {
    let (hnsw, raw_vecs) = build_hnsw_100k();
    let k = 10usize;
    let filter = VectorFilter::default();
    let queries = 100usize;
    let mut seed = 0xFEED_FACE_u64;
    let mut correct = 0usize;

    for _ in 0..queries {
        let query = rand_unit_vec_32(&mut seed);
        let mut scores: Vec<(f32, usize)> = raw_vecs
            .iter()
            .enumerate()
            .map(|(i, v)| (v.iter().zip(&query).map(|(a, b)| a * b).sum::<f32>(), i))
            .collect();
        scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let ground_truth: HashSet<String> = scores[..k]
            .iter()
            .map(|(_, id)| format!("v://{id}"))
            .collect();

        let results = hnsw.knn_search(&query, &filter, k);
        correct += results
            .iter()
            .filter(|r| ground_truth.contains(&r.object_id))
            .count();
    }

    let recall_pct = correct as f64 / (queries * k) as f64 * 100.0;
    assert!(
        recall_pct >= HNSW_RECALL_100K_PMIN,
        "HNSW recall@10 at 100 K = {:.1}% < SLA minimum {:.1}%",
        recall_pct,
        HNSW_RECALL_100K_PMIN
    );
}

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_vector_search_p99_lte_5ms_at_100k_vectors() {
    let (hnsw, _) = build_hnsw_100k();
    let k = 10usize;
    let filter = VectorFilter::default();
    let iterations = 500usize;
    let mut seed = 0xCAFE_BABE_u64;
    let mut timings = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let query = rand_unit_vec_32(&mut seed);
        let t = Instant::now();
        let _ = hnsw.knn_search(&query, &filter, k);
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000_000; // ns → ms
    assert!(
        p99 <= VECTOR_SEARCH_P99_MS_MAX,
        "knn_search() p99 {} ms > SLA maximum {} ms (100 K vectors, k=10)",
        p99,
        VECTOR_SEARCH_P99_MS_MAX
    );
}

// ── L-scale SLAs (100 K objects) ─────────────────────────────────────────────
// Run with: cargo test --release -- --ignored --test-threads=1 scale:L
// Expected runtime per test: 30–120 seconds.  Run on a nightly/manual schedule.

/// Build a store with `n` objects across `namespaces` — L-scale variant.
/// Uses commit_buffered + flush to keep setup time reasonable.
fn build_store_l(dir: &TempDir, n: usize, namespaces: &[&str]) -> BinderStore {
    let store = BinderStore::open(dir.path()).unwrap();
    for i in 0..n {
        let ns = namespaces[i % namespaces.len()];
        let mut tx = Transaction::new();
        tx.put_object(
            obj(format!("{}://obj.{:08}", ns, i)),
            vec![i as u8 % 255; 64],
        );
        store.commit_buffered(tx, "test", "").unwrap();
        // Flush every 10 K to keep WAL write pressure manageable.
        if i > 0 && i % 10_000 == 0 {
            store.flush().unwrap();
        }
    }
    store.flush().unwrap();
    store
}

#[test]
#[ignore = "perf scale:L — cargo test --release -- --ignored --test-threads=1"]
fn sla_l_get_p99_lte_10us_at_100k_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_l(&dir, 100_000, &["a"]);
    let target = obj("a://obj.00050000");
    let iterations = 5_000;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.get(&target).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= GET_L_P99_US_MAX,
        "get() p99 {} µs > L-scale SLA maximum {} µs (get must stay O(1) at 100K objects)",
        p99,
        GET_L_P99_US_MAX
    );
}

#[test]
#[ignore = "perf scale:L — cargo test --release -- --ignored --test-threads=1"]
fn sla_l_list_p99_lte_20ms_returning_10k_of_100k() {
    let dir = tempfile::tempdir().unwrap();
    // 10 namespaces × 10 K objects each = 100 K total; list("ns0://") returns 10 K.
    let namespaces: Vec<String> = (0..10).map(|i| format!("ns{}", i)).collect();
    let ns_refs: Vec<&str> = namespaces.iter().map(String::as_str).collect();
    let store = build_store_l(&dir, 100_000, &ns_refs);
    let iterations = 100;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let results = store.list("ns0://").unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
        assert_eq!(results.len(), 10_000);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000_000; // ns → ms
    assert!(
        p99 <= LIST_L_P99_MS_MAX,
        "list() p99 {} ms > L-scale SLA maximum {} ms (returning 10 K of 100 K objects)",
        p99,
        LIST_L_P99_MS_MAX
    );
}

#[test]
#[ignore = "perf scale:L — cargo test --release -- --ignored --test-threads=1"]
fn sla_l_rebuild_indexes_lte_2s_at_100k_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_l(&dir, 100_000, &["a", "b", "c", "d", "e"]);

    // Warm run, then measure 3 times and take the worst.
    store.rebuild_indexes().unwrap();

    let mut worst_ms = 0u64;
    for _ in 0..3 {
        let t = Instant::now();
        store.rebuild_indexes().unwrap();
        let ms = t.elapsed().as_millis() as u64;
        worst_ms = worst_ms.max(ms);
    }

    assert!(
        worst_ms <= REBUILD_L_MS_MAX,
        "rebuild_indexes() worst-of-3 {} ms > L-scale SLA maximum {} ms (100 K objects)",
        worst_ms,
        REBUILD_L_MS_MAX
    );
}

#[test]
#[ignore = "perf scale:L — cargo test --release -- --ignored --test-threads=1"]
fn sla_l_open_wal_replay_lte_300ms_at_100k_commits() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = build_store_l(&dir, 100_000, &["a"]);
        store.close().unwrap();
    }

    let mut worst_ms = 0u64;
    for _ in 0..3 {
        let t = Instant::now();
        let store = BinderStore::open(dir.path()).unwrap();
        let ms = t.elapsed().as_millis() as u64;
        worst_ms = worst_ms.max(ms);
        store.close().unwrap();
    }

    assert!(
        worst_ms <= OPEN_REPLAY_L_MS_MAX,
        "open() WAL replay worst-of-3 {} ms > L-scale SLA maximum {} ms (100 K commits)",
        worst_ms,
        OPEN_REPLAY_L_MS_MAX
    );
}

// ── XL-scale SLAs (1 M objects) ───────────────────────────────────────────────
// Run with: cargo test --release -- --ignored --test-threads=1 scale:XL
// Expected runtime per test: 5–20 minutes. Run on a nightly schedule only.

/// Build a store with `n` objects — XL-scale variant.
/// Flushes every 100 K to keep write pressure reasonable.
fn build_store_xl(dir: &TempDir, n: usize, namespaces: &[&str]) -> BinderStore {
    let store = BinderStore::open(dir.path()).unwrap();
    for i in 0..n {
        let ns = namespaces[i % namespaces.len()];
        let mut tx = Transaction::new();
        tx.put_object(
            obj(format!("{}://obj.{:08}", ns, i)),
            vec![i as u8 % 255; 64],
        );
        store.commit_buffered(tx, "test", "").unwrap();
        if i > 0 && i % 100_000 == 0 {
            store.flush().unwrap();
        }
    }
    store.flush().unwrap();
    store
}

#[test]
#[ignore = "perf scale:XL — cargo test --release -- --ignored --test-threads=1"]
fn sla_xl_get_p99_lte_10us_at_1m_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_xl(&dir, 1_000_000, &["a"]);
    let target = obj("a://obj.00500000");
    let iterations = 5_000;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let _ = store.get(&target).unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000; // ns → µs
    assert!(
        p99 <= GET_XL_P99_US_MAX,
        "get() p99 {} µs > XL-scale SLA maximum {} µs (HashMap must stay O(1) at 1 M objects)",
        p99,
        GET_XL_P99_US_MAX
    );
}

#[test]
#[ignore = "perf scale:XL — cargo test --release -- --ignored --test-threads=1"]
fn sla_xl_open_lte_5s_after_compact_at_1m_objects() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = build_store_xl(&dir, 1_000_000, &["a"]);
        // compact() snapshots current state and truncates the WAL so reopen
        // replays only from the snapshot — much faster than bare WAL replay.
        store.compact().unwrap();
        store.close().unwrap();
    }

    let mut worst_ms = 0u64;
    for _ in 0..3 {
        let t = Instant::now();
        let store = BinderStore::open(dir.path()).unwrap();
        let ms = t.elapsed().as_millis() as u64;
        worst_ms = worst_ms.max(ms);
        store.close().unwrap();
    }

    assert!(
        worst_ms <= OPEN_XL_AFTER_COMPACT_MS_MAX,
        "open() worst-of-3 {} ms > XL-scale SLA maximum {} ms (1 M objects after compact)",
        worst_ms,
        OPEN_XL_AFTER_COMPACT_MS_MAX
    );
}

#[test]
#[ignore = "perf scale:XL — cargo test --release -- --ignored --test-threads=1"]
fn sla_xl_list_p99_lte_500ms_returning_100k_of_1m() {
    let dir = tempfile::tempdir().unwrap();
    // 10 namespaces × 100 K objects each = 1 M total; list("ns0://") returns 100 K.
    let namespaces: Vec<String> = (0..10).map(|i| format!("ns{}", i)).collect();
    let ns_refs: Vec<&str> = namespaces.iter().map(String::as_str).collect();
    let store = build_store_xl(&dir, 1_000_000, &ns_refs);
    let iterations = 20;

    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        let results = store.list("ns0://").unwrap();
        timings.push(t.elapsed().as_nanos() as u64);
        assert_eq!(results.len(), 100_000);
    }

    let p99 = percentile(&mut timings, 99.0) / 1_000_000; // ns → ms
    assert!(
        p99 <= LIST_XL_P99_MS_MAX,
        "list() p99 {} ms > XL-scale SLA maximum {} ms (returning 100 K of 1 M objects)",
        p99,
        LIST_XL_P99_MS_MAX
    );
}

#[test]
#[ignore = "perf scale:XL — cargo test --release -- --ignored --test-threads=1"]
fn sla_xl_commit_buffered_throughput_gte_3k_per_sec_at_1m_existing() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_xl(&dir, 1_000_000, &["a"]);
    // Measure a 10 K batch written on top of 1 M existing objects.
    let n = 10_000usize;

    let start = Instant::now();
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("b://new.{i:08}")), vec![0u8; 64]);
        store.commit_buffered(tx, "test", "").unwrap();
    }
    store.flush().unwrap();
    let elapsed = start.elapsed();

    let throughput = n as f64 / elapsed.as_secs_f64();
    assert!(
        throughput >= COMMIT_XL_BUFFERED_MIN as f64,
        "commit_buffered() throughput at XL scale {:.0} commits/sec < SLA minimum {} commits/sec",
        throughput,
        COMMIT_XL_BUFFERED_MIN
    );
}

#[test]
#[ignore = "perf scale:XL — cargo test --release -- --ignored --test-threads=1"]
fn sla_xl_rebuild_indexes_lte_60s_at_1m_objects() {
    let dir = tempfile::tempdir().unwrap();
    let store = build_store_xl(&dir, 1_000_000, &["a", "b", "c", "d", "e"]);

    // Single warm run, then measure once (rebuild at 1 M is slow by design).
    store.rebuild_indexes().unwrap();

    let t = Instant::now();
    store.rebuild_indexes().unwrap();
    let ms = t.elapsed().as_millis() as u64;

    assert!(
        ms <= REBUILD_XL_MS_MAX,
        "rebuild_indexes() {} ms > XL-scale SLA maximum {} ms (1 M objects)",
        ms,
        REBUILD_XL_MS_MAX
    );
}

// ── health import SLA (OKR O2/KR2.3) ─────────────────────────────────────────

#[test]
#[ignore = "perf: cargo test --release -- --ignored --test-threads=1"]
fn sla_health_import_500k_lte_90s() {
    // Simulates a full Apple Health annual import: 500 K observations, each a
    // single object with a 64-byte payload, committed buffered + one flush.
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 500_000usize;

    let start = Instant::now();
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("health://obs/{i}")), vec![0u8; 64]);
        store.commit_buffered(tx, "test", "").unwrap();
    }
    store.flush().unwrap();
    let elapsed = start.elapsed().as_secs_f64();

    assert!(
        elapsed <= HEALTH_IMPORT_500K_S_MAX,
        "500 K health import took {:.1} s — SLA ≤ {:.0} s",
        elapsed,
        HEALTH_IMPORT_500K_S_MAX
    );
}
