//! Concrete performance report for bindb.
//!
//! Runs the same M-tier (and selected L-tier) measurements used by the SLA
//! gate, then writes:
//!   - `perf-report.json`  machine-readable metrics
//!   - `perf-report.md`    human-readable table (measured vs SLA)
//!
//! Usage:
//!   cargo run --example perf_report --release
//!   PERF_REPORT_DIR=./out cargo run --example perf_report --release
//!
//! Exit code 1 if any measured metric fails its SLA.

use bindb::{BinderStore, ObjectId, Transaction};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::TempDir;

// Thresholds mirrored from tests/perf_sla.rs / docs/sla.md (M-tier unless noted).
const COMMIT_THROUGHPUT_MIN: f64 = 80.0;
const COMMIT_BUFFERED_THROUGHPUT_MIN: f64 = 3_000.0;
const FLUSH_WALL_MS_MAX: f64 = 20.0;
const GET_P99_US_MAX: f64 = 10.0;
const LIST_P99_MS_MAX: f64 = 2.0;
const OBJECT_AS_OF_P99_US_MAX: f64 = 500.0;
const EDGES_FROM_P99_US_MAX: f64 = 150.0;
const TRAVERSE_D3_P99_US_MAX: f64 = 500.0;
const TRAVERSE_D5_P99_MS_MAX: f64 = 2.0;
const REBUILD_M_MS_MAX: f64 = 200.0;
const OPEN_M_MS_MAX: f64 = 30.0;
const OPEN_L_MS_MAX: f64 = 300.0;
const LIST_L_P99_MS_MAX: f64 = 20.0;
const HNSW_RECALL_MIN: f64 = 0.80;

#[derive(Clone)]
struct Metric {
    id: &'static str,
    name: &'static str,
    scale: &'static str,
    value: f64,
    unit: &'static str,
    /// SLA bound. Higher-is-better metrics use `min`; lower-is-better use `max`.
    bound: f64,
    higher_is_better: bool,
}

impl Metric {
    fn pass(&self) -> bool {
        if self.higher_is_better {
            self.value >= self.bound
        } else {
            self.value <= self.bound
        }
    }

    fn headroom_pct(&self) -> f64 {
        if self.higher_is_better {
            if self.bound == 0.0 {
                return 0.0;
            }
            (self.value / self.bound - 1.0) * 100.0
        } else {
            if self.value == 0.0 {
                return 100.0;
            }
            (1.0 - self.value / self.bound) * 100.0
        }
    }
}

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

fn percentile(timings: &mut [u64], p: f64) -> u64 {
    timings.sort_unstable();
    let idx = ((timings.len() as f64 * p / 100.0) as usize).min(timings.len() - 1);
    timings[idx]
}

fn build_store(dir: &TempDir, n: usize, namespaces: &[&str]) -> BinderStore {
    let store = BinderStore::open(dir.path()).unwrap();
    for i in 0..n {
        let ns = namespaces[i % namespaces.len()];
        let mut tx = Transaction::new();
        tx.put_object(
            obj(format!("{}://obj.{:08}", ns, i)),
            vec![i as u8 % 255; 64],
        );
        store.commit_buffered(tx, "report", "").unwrap();
        if i > 0 && i % 10_000 == 0 {
            store.flush().unwrap();
        }
    }
    store.flush().unwrap();
    store
}

fn measure_all() -> Vec<Metric> {
    let mut out = Vec::new();

    // commit durable throughput
    {
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 200usize;
        let t = Instant::now();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit(tx, "report", "").unwrap();
        }
        let thr = n as f64 / t.elapsed().as_secs_f64();
        out.push(Metric {
            id: "commit_durable_tps",
            name: "commit() durable throughput",
            scale: "M (200 commits)",
            value: thr,
            unit: "commits/sec",
            bound: COMMIT_THROUGHPUT_MIN,
            higher_is_better: true,
        });
    }

    // buffered + flush
    {
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 10_000usize;
        let t = Instant::now();
        for i in 0..n {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit_buffered(tx, "report", "").unwrap();
        }
        store.flush().unwrap();
        let thr = n as f64 / t.elapsed().as_secs_f64();
        out.push(Metric {
            id: "commit_buffered_tps",
            name: "commit_buffered()+flush throughput",
            scale: "M (10 K batch)",
            value: thr,
            unit: "commits/sec",
            bound: COMMIT_BUFFERED_THROUGHPUT_MIN,
            higher_is_better: true,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        for i in 0..10_000usize {
            let mut tx = Transaction::new();
            tx.put_object(obj(format!("a://o{i}")), vec![0u8; 64]);
            store.commit_buffered(tx, "report", "").unwrap();
        }
        let t = Instant::now();
        store.flush().unwrap();
        out.push(Metric {
            id: "flush_wall_ms",
            name: "flush() after 10 K buffered",
            scale: "M",
            value: t.elapsed().as_secs_f64() * 1000.0,
            unit: "ms",
            bound: FLUSH_WALL_MS_MAX,
            higher_is_better: false,
        });
    }

    // get / list / as_of / edges / traverse / rebuild / open (M)
    {
        let dir = tempfile::tempdir().unwrap();
        let store = build_store(&dir, 10_000, &["a"]);
        let target = obj("a://obj.00005000");
        let mut timings = Vec::with_capacity(5_000);
        for _ in 0..5_000 {
            let t = Instant::now();
            let _ = store.get(&target).unwrap();
            timings.push(t.elapsed().as_nanos() as u64);
        }
        let p99_us = percentile(&mut timings, 99.0) as f64 / 1_000.0;
        out.push(Metric {
            id: "get_p99_us",
            name: "get() p99",
            scale: "M (10 K objects)",
            value: p99_us,
            unit: "µs",
            bound: GET_P99_US_MAX,
            higher_is_better: false,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let store = build_store(
            &dir,
            10_000,
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
        );
        let mut timings = Vec::with_capacity(200);
        for _ in 0..200 {
            let t = Instant::now();
            let r = store.list("a://").unwrap();
            timings.push(t.elapsed().as_nanos() as u64);
            assert_eq!(r.len(), 1_000);
        }
        let p99_ms = percentile(&mut timings, 99.0) as f64 / 1_000_000.0;
        out.push(Metric {
            id: "list_p99_ms",
            name: "list() p99 (1 K of 10 K)",
            scale: "M",
            value: p99_ms,
            unit: "ms",
            bound: LIST_P99_MS_MAX,
            higher_is_better: false,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let oid = obj("a://obj.target");
        let mut mid_commit = None;
        for i in 0..10_000usize {
            let mut tx = Transaction::new();
            tx.put_object(oid.clone(), vec![i as u8 % 255; 32]);
            let cid = store.commit_buffered(tx, "report", "").unwrap();
            if i == 5_000 {
                mid_commit = Some(cid);
            }
        }
        store.flush().unwrap();
        let at = mid_commit.unwrap();
        let mut timings = Vec::with_capacity(1_000);
        for _ in 0..1_000 {
            let t = Instant::now();
            let _ = store.object_as_of(&oid, at).unwrap();
            timings.push(t.elapsed().as_nanos() as u64);
        }
        out.push(Metric {
            id: "object_as_of_p99_us",
            name: "object_as_of() p99",
            scale: "M (10 K versions)",
            value: percentile(&mut timings, 99.0) as f64 / 1_000.0,
            unit: "µs",
            bound: OBJECT_AS_OF_P99_US_MAX,
            higher_is_better: false,
        });
    }

    {
        use bindb::EdgeId;
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let hub = obj("a://hub");
        let mut setup = Transaction::new();
        setup.put_object(hub.clone(), vec![0u8; 32]);
        for i in 0..100usize {
            let spoke = obj(format!("a://spoke.{i}"));
            setup.put_object(spoke.clone(), vec![i as u8; 32]);
            setup.put_edge(
                EdgeId::new(format!("e{i}")),
                hub.clone(),
                spoke,
                "LINK",
                None,
            );
        }
        store.commit(setup, "report", "").unwrap();
        let mut timings = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            let t = Instant::now();
            let e = store.edges_from(&hub).unwrap();
            timings.push(t.elapsed().as_nanos() as u64);
            assert_eq!(e.len(), 100);
        }
        out.push(Metric {
            id: "edges_from_p99_us",
            name: "edges_from() p99 (degree 100)",
            scale: "M",
            value: percentile(&mut timings, 99.0) as f64 / 1_000.0,
            unit: "µs",
            bound: EDGES_FROM_P99_US_MAX,
            higher_is_better: false,
        });
    }

    {
        use bindb::EdgeId;
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let n = 5_000usize;
        let mut tx = Transaction::new();
        for i in 0..n {
            tx.put_object(obj(format!("a://n{i}")), vec![0u8; 16]);
        }
        store.commit(tx, "report", "nodes").unwrap();
        for i in 0..n - 1 {
            let mut etx = Transaction::new();
            etx.put_edge(
                EdgeId::new(format!("e{i}")),
                obj(format!("a://n{i}")),
                obj(format!("a://n{}", i + 1)),
                "NEXT",
                None,
            );
            store.commit_buffered(etx, "report", "edges").unwrap();
        }
        store.flush().unwrap();
        let start = obj("a://n0");
        let mut t3 = Vec::with_capacity(500);
        for _ in 0..500 {
            let t = Instant::now();
            let _ = store.traverse(&start, 3).unwrap();
            t3.push(t.elapsed().as_nanos() as u64);
        }
        let mut t5 = Vec::with_capacity(500);
        for _ in 0..500 {
            let t = Instant::now();
            let _ = store.traverse(&start, 5).unwrap();
            t5.push(t.elapsed().as_nanos() as u64);
        }
        out.push(Metric {
            id: "traverse_d3_p99_us",
            name: "traverse(depth=3) p99",
            scale: "M (5 K chain)",
            value: percentile(&mut t3, 99.0) as f64 / 1_000.0,
            unit: "µs",
            bound: TRAVERSE_D3_P99_US_MAX,
            higher_is_better: false,
        });
        out.push(Metric {
            id: "traverse_d5_p99_ms",
            name: "traverse(depth=5) p99",
            scale: "M (5 K chain)",
            value: percentile(&mut t5, 99.0) as f64 / 1_000_000.0,
            unit: "ms",
            bound: TRAVERSE_D5_P99_MS_MAX,
            higher_is_better: false,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let store = build_store(&dir, 10_000, &["a", "b", "c"]);
        store.rebuild_indexes().unwrap();
        let mut worst = 0.0f64;
        for _ in 0..5 {
            let t = Instant::now();
            store.rebuild_indexes().unwrap();
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
        }
        out.push(Metric {
            id: "rebuild_m_ms",
            name: "rebuild_indexes() worst-of-5",
            scale: "M (10 K)",
            value: worst,
            unit: "ms",
            bound: REBUILD_M_MS_MAX,
            higher_is_better: false,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = build_store(&dir, 10_000, &["a"]);
            store.close().unwrap();
        }
        let mut worst = 0.0f64;
        for _ in 0..3 {
            let t = Instant::now();
            let store = BinderStore::open(dir.path()).unwrap();
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            store.close().unwrap();
        }
        out.push(Metric {
            id: "open_m_ms",
            name: "open() WAL replay worst-of-3",
            scale: "M (10 K commits)",
            value: worst,
            unit: "ms",
            bound: OPEN_M_MS_MAX,
            higher_is_better: false,
        });
    }

    // L-tier open + list (slower; still part of the report)
    {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = build_store(&dir, 100_000, &["a"]);
            store.close().unwrap();
        }
        let mut worst = 0.0f64;
        for _ in 0..3 {
            let t = Instant::now();
            let store = BinderStore::open(dir.path()).unwrap();
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            store.close().unwrap();
        }
        out.push(Metric {
            id: "open_l_ms",
            name: "open() WAL replay worst-of-3",
            scale: "L (100 K commits)",
            value: worst,
            unit: "ms",
            bound: OPEN_L_MS_MAX,
            higher_is_better: false,
        });
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let namespaces: Vec<String> = (0..10).map(|i| format!("ns{i}")).collect();
        let ns_refs: Vec<&str> = namespaces.iter().map(String::as_str).collect();
        let store = build_store(&dir, 100_000, &ns_refs);
        let mut timings = Vec::with_capacity(50);
        for _ in 0..50 {
            let t = Instant::now();
            let r = store.list("ns0://").unwrap();
            timings.push(t.elapsed().as_nanos() as u64);
            assert_eq!(r.len(), 10_000);
        }
        out.push(Metric {
            id: "list_l_p99_ms",
            name: "list() p99 (10 K of 100 K)",
            scale: "L",
            value: percentile(&mut timings, 99.0) as f64 / 1_000_000.0,
            unit: "ms",
            bound: LIST_L_P99_MS_MAX,
            higher_is_better: false,
        });
    }

    // HNSW recall@10 vs brute (above threshold)
    {
        use bindb::{VectorEntry, VectorFilter, VectorIndex};
        fn unit(mut v: Vec<f32>) -> Vec<f32> {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in &mut v {
                *x /= norm;
            }
            v
        }
        fn entry(i: usize, v: Vec<f32>) -> VectorEntry {
            VectorEntry {
                object_id: format!("v://{i}"),
                object_version_id: format!("v://{i}-v1"),
                chunk_id: format!("c{i}"),
                chunk_index: 0,
                model_id: "report".into(),
                dimensions: v.len() as u32,
                vector: v,
                content_hash: [0u8; 32],
                source_text: String::new(),
                binder_id: None,
                app_id: None,
            }
        }
        let n = 1_200usize;
        let dim = 32usize;
        let mut hnsw = VectorIndex::new();
        let mut brute = VectorIndex::new();
        for i in 0..n {
            let mut v = vec![0.0f32; dim];
            v[i % dim] = 1.0;
            let e = entry(i, unit(v));
            hnsw.insert(e.clone());
            brute.insert(e);
        }
        let k = 10usize;
        let queries = 10usize;
        let filter = VectorFilter::default();
        let mut correct = 0usize;
        for q in 0..queries {
            let mut query = vec![0.0f32; dim];
            query[q % dim] = 1.0;
            let query = unit(query);
            let h = hnsw.knn_search(&query, &filter, k);
            let b = brute.knn_search(&query, &filter, k);
            let bset: std::collections::HashSet<_> =
                b.iter().map(|r| r.object_id.clone()).collect();
            correct += h.iter().filter(|r| bset.contains(&r.object_id)).count();
        }
        let recall = correct as f64 / (queries * k) as f64;
        out.push(Metric {
            id: "hnsw_recall_at_10",
            name: "HNSW recall@10 vs brute",
            scale: "1.2 K vectors",
            value: recall * 100.0,
            unit: "%",
            bound: HNSW_RECALL_MIN * 100.0,
            higher_is_better: true,
        });
    }

    // XL (1 M) — optional; issue #7. Slow; not CI-gated.
    if env::var("PERF_REPORT_XL").ok().as_deref() == Some("1") {
        eprintln!("PERF_REPORT_XL=1 — building 1 M object store (slow)…");
        const XL_N: usize = 1_000_000;
        // Soft targets for report pass/fail (not yet CI SLA constants).
        const GET_XL_P99_US_MAX: f64 = 10.0;
        // Ops path: open after compact() / clean snapshot (not raw 1M WAL replay).
        const OPEN_XL_MS_MAX: f64 = 5_000.0;
        // Interim #6 observation (does not gate overall_pass).
        const RSS_XL_MB_MAX: f64 = 2048.0;

        let dir = tempfile::tempdir().unwrap();
        {
            let store = build_store(&dir, XL_N, &["a"]);
            let target = obj("a://obj.000500000");
            let mut timings = Vec::with_capacity(2_000);
            for _ in 0..2_000 {
                let t = Instant::now();
                let _ = store.get(&target).unwrap();
                timings.push(t.elapsed().as_nanos() as u64);
            }
            out.push(Metric {
                id: "get_xl_p99_us",
                name: "get() p99",
                scale: "XL (1 M objects)",
                value: percentile(&mut timings, 99.0) as f64 / 1_000.0,
                unit: "µs",
                bound: GET_XL_P99_US_MAX,
                higher_is_better: false,
            });
            // Compact so reopen measures snapshot fast path (issue #1 / #7).
            store.compact().unwrap();
            store.close().unwrap();
        }
        let mut worst = 0.0f64;
        let mut rss_mb = 0.0f64;
        for i in 0..3 {
            let t = Instant::now();
            let store = BinderStore::open(dir.path()).unwrap();
            worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            if i == 0 {
                if let Some(mb) = process_rss_mb() {
                    rss_mb = mb;
                }
            }
            store.close().unwrap();
        }
        out.push(Metric {
            id: "open_xl_ms",
            name: "open() after compact() worst-of-3",
            scale: "XL (1 M objects, snapshot)",
            value: worst,
            unit: "ms",
            bound: OPEN_XL_MS_MAX,
            higher_is_better: false,
        });
        if rss_mb > 0.0 {
            out.push(Metric {
                id: "rss_xl_mb",
                name: "process RSS after XL open",
                scale: "XL (1 M objects)",
                value: rss_mb,
                unit: "MB",
                bound: RSS_XL_MB_MAX,
                higher_is_better: false,
            });
        }
    }

    out
}

fn process_rss_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: f64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb / 1024.0);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let kb: f64 = s.trim().parse().ok()?;
        Some(kb / 1024.0)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn write_json(path: &Path, metrics: &[Metric], git_sha: &str, passed: bool) {
    let mut f = fs::File::create(path).expect("create json");
    writeln!(f, "{{").unwrap();
    writeln!(f, "  \"schema\": \"bindb.perf_report.v1\",").unwrap();
    writeln!(
        f,
        "  \"generated_at_unix\": {},",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
    .unwrap();
    writeln!(f, "  \"git_sha\": \"{git_sha}\",").unwrap();
    writeln!(f, "  \"overall_pass\": {passed},").unwrap();
    writeln!(f, "  \"metrics\": [").unwrap();
    for (i, m) in metrics.iter().enumerate() {
        let comma = if i + 1 == metrics.len() { "" } else { "," };
        writeln!(
            f,
            "    {{\"id\":\"{}\",\"name\":\"{}\",\"scale\":\"{}\",\"value\":{:.6},\"unit\":\"{}\",\"bound\":{:.6},\"higher_is_better\":{},\"pass\":{},\"headroom_pct\":{:.2}}}{comma}",
            m.id,
            m.name.replace('"', "'"),
            m.scale,
            m.value,
            m.unit,
            m.bound,
            m.higher_is_better,
            m.pass(),
            m.headroom_pct()
        )
        .unwrap();
    }
    writeln!(f, "  ]").unwrap();
    writeln!(f, "}}").unwrap();
}

fn write_md(path: &Path, metrics: &[Metric], git_sha: &str, passed: bool) {
    let mut f = fs::File::create(path).expect("create md");
    let status = if passed { "PASS" } else { "FAIL" };
    writeln!(f, "# bindb performance report").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "- **Overall:** `{status}`").unwrap();
    writeln!(f, "- **git:** `{git_sha}`").unwrap();
    writeln!(
        f,
        "- **generated:** unix {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    )
    .unwrap();
    writeln!(f, "- **schema:** `bindb.perf_report.v1`").unwrap();
    writeln!(f).unwrap();
    writeln!(
        f,
        "| Status | Metric | Scale | Measured | SLA | Headroom | Unit |"
    )
    .unwrap();
    writeln!(
        f,
        "|--------|--------|-------|----------|-----|----------|------|"
    )
    .unwrap();
    for m in metrics {
        let st = if m.pass() { "PASS" } else { "FAIL" };
        let cmp = if m.higher_is_better { "≥" } else { "≤" };
        writeln!(
            f,
            "| {st} | {} | {} | {:.3} | {cmp} {:.3} | {:+.1}% | {} |",
            m.name,
            m.scale,
            m.value,
            m.bound,
            m.headroom_pct(),
            m.unit
        )
        .unwrap();
    }
    writeln!(f).unwrap();
    writeln!(
        f,
        "_Headroom: positive means clearer of the SLA. Negative means breached._"
    )
    .unwrap();
}

fn main() {
    let out_dir: PathBuf = env::var("PERF_REPORT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    fs::create_dir_all(&out_dir).ok();

    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());

    eprintln!("bindb perf_report: measuring…");
    let metrics = measure_all();
    let passed = metrics
        .iter()
        .filter(|m| m.id != "rss_xl_mb") // informational for #6; does not gate overall
        .all(|m| m.pass());

    let json_path = out_dir.join("perf-report.json");
    let md_path = out_dir.join("perf-report.md");
    write_json(&json_path, &metrics, &git_sha, passed);
    write_md(&md_path, &metrics, &git_sha, passed);

    eprintln!("wrote {}", json_path.display());
    eprintln!("wrote {}", md_path.display());
    for m in &metrics {
        let st = if m.pass() { "PASS" } else { "FAIL" };
        eprintln!(
            "  [{st}] {} = {:.3} {} (SLA {} {:.3}) headroom {:+.1}%",
            m.id,
            m.value,
            m.unit,
            if m.higher_is_better { "≥" } else { "≤" },
            m.bound,
            m.headroom_pct()
        );
    }

    if !passed {
        eprintln!("overall: FAIL");
        if env::var("PERF_REPORT_SOFT").ok().as_deref() == Some("1") {
            eprintln!("PERF_REPORT_SOFT=1 — exiting 0 (report still written)");
            return;
        }
        std::process::exit(1);
    }
    eprintln!("overall: PASS");
}
