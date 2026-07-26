use bindb::{Aggregate, BinderStore, ObjectId, StoreError, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
fn oid(s: &str) -> ObjectId {
    ObjectId::new(s)
}

// Payload helpers: store f64 as little-endian bytes.
fn encode(v: f64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn decode(b: &[u8]) -> Option<f64> {
    b.try_into().ok().map(f64::from_le_bytes)
}

// Build a store with observations in `obs://health.sleep` namespace.
// Values: 7.0, 8.0, 6.0 hours (sources: withings, oura, withings).
fn build_sleep_store(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep:withings:01"), encode(7.0));
    tx.put_object(oid("obs://health.sleep:withings:02"), encode(6.0));
    tx.put_object(oid("obs://health.sleep:oura:01"), encode(8.0));
    store.commit(tx, "test", "seed").unwrap();
}

// ── count ─────────────────────────────────────────────────────────────────────

#[test]
fn count_returns_total_observations() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .aggregate(Aggregate::Count)
        .execute()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    assert_eq!(results[0].value as usize, 3);
    store.close().unwrap();
}

// ── sum / avg / min / max ─────────────────────────────────────────────────────

#[test]
fn sum_totals_extracted_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::Sum)
        .execute()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!((results[0].value - 21.0).abs() < 1e-9, "7+8+6 = 21");
    store.close().unwrap();
}

#[test]
fn avg_computes_mean() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::Avg)
        .execute()
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!((results[0].value - 7.0).abs() < 1e-9, "avg of 7,8,6 = 7");
    store.close().unwrap();
}

#[test]
fn min_returns_smallest_value() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::Min)
        .execute()
        .unwrap();

    assert!((results[0].value - 6.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn max_returns_largest_value() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::Max)
        .execute()
        .unwrap();

    assert!((results[0].value - 8.0).abs() < 1e-9);
    store.close().unwrap();
}

// ── percentiles ───────────────────────────────────────────────────────────────

#[test]
fn p50_returns_median() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::P50)
        .execute()
        .unwrap();

    // Sorted: [6, 7, 8] → median = 7.
    assert!((results[0].value - 7.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn p99_on_small_set_returns_max() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .aggregate(Aggregate::P99)
        .execute()
        .unwrap();

    // With 3 values and idx = round(0.99 * 2) = 2, P99 == max == 8.
    assert!((results[0].value - 8.0).abs() < 1e-9);
    store.close().unwrap();
}

// ── group_by ─────────────────────────────────────────────────────────────────

#[test]
fn group_by_source_produces_per_source_averages() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    // Object IDs look like: obs://health.sleep:SOURCE:serial
    // Split on ':' to extract the source (index 2).
    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .group_by(|v| v.object_id.0.split(':').nth(2).map(str::to_owned))
        .aggregate(Aggregate::Avg)
        .execute()
        .unwrap();

    // Two groups: oura and withings.
    assert_eq!(results.len(), 2, "expected oura + withings groups");

    let oura = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("oura"))
        .expect("oura group missing");
    assert!((oura.value - 8.0).abs() < 1e-9, "oura avg = 8.0");

    let withings = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("withings"))
        .expect("withings group missing");
    assert!(
        (withings.value - 6.5).abs() < 1e-9,
        "withings avg of 7,6 = 6.5"
    );
    store.close().unwrap();
}

#[test]
fn group_by_count_is_per_group() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .group_by(|v| v.object_id.0.split(':').nth(2).map(str::to_owned))
        .aggregate(Aggregate::Count)
        .execute()
        .unwrap();

    let oura = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("oura"))
        .unwrap();
    assert_eq!(oura.count, 1);

    let withings = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("withings"))
        .unwrap();
    assert_eq!(withings.count, 2);
    store.close().unwrap();
}

// ── filter ────────────────────────────────────────────────────────────────────

#[test]
fn filter_excludes_non_matching_objects() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    // Only include oura observations.
    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .extract_f64(decode)
        .filter(|v| v.object_id.0.contains(":oura:"))
        .aggregate(Aggregate::Count)
        .execute()
        .unwrap();

    assert_eq!(results[0].count, 1, "only 1 oura observation");
    store.close().unwrap();
}

// ── error cases ───────────────────────────────────────────────────────────────

#[test]
fn execute_without_namespace_returns_error() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let result = store
        .aggregate_observations()
        .aggregate(Aggregate::Count)
        .execute();

    assert!(
        matches!(result, Err(StoreError::AggregationMissingNamespace)),
        "missing namespace must return AggregationMissingNamespace"
    );
    store.close().unwrap();
}

#[test]
fn empty_namespace_returns_empty_results() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let results = store
        .aggregate_observations()
        .namespace("obs://nonexistent")
        .aggregate(Aggregate::Count)
        .execute()
        .unwrap();

    // No objects → no groups → empty result.
    assert!(results.is_empty());
    store.close().unwrap();
}

// ── result ordering ───────────────────────────────────────────────────────────

#[test]
fn results_are_sorted_by_group_key() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .aggregate_observations()
        .namespace("obs://health.sleep")
        .group_by(|v| v.object_id.0.split(':').nth(2).map(str::to_owned))
        .aggregate(Aggregate::Count)
        .execute()
        .unwrap();

    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "results must be in sorted group_key order");
    store.close().unwrap();
}

// ── join_observations ─────────────────────────────────────────────────────────

fn date_key(v: &bindb::BinderObjectVersion) -> Option<String> {
    v.object_id.0.rsplit('/').next().map(|s| s.to_owned())
}

fn build_daily_store(store: &BinderStore) {
    // sleep: obs://health.sleep/2024-01-01 .. 03
    // steps: obs://health.steps/2024-01-01 .. 03 + 04 (no matching sleep)
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep/2024-01-01"), encode(7.0));
    tx.put_object(oid("obs://health.sleep/2024-01-02"), encode(8.0));
    tx.put_object(oid("obs://health.sleep/2024-01-03"), encode(6.5));
    tx.put_object(oid("obs://health.steps/2024-01-01"), encode(9000.0));
    tx.put_object(oid("obs://health.steps/2024-01-02"), encode(11000.0));
    tx.put_object(oid("obs://health.steps/2024-01-03"), encode(7500.0));
    tx.put_object(oid("obs://health.steps/2024-01-04"), encode(8200.0));
    store.commit(tx, "test", "daily").unwrap();
}

#[test]
fn join_inner_matches_shared_keys() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_daily_store(&store);

    let rows = store
        .join_observations("obs://health.sleep", "obs://health.steps", date_key)
        .unwrap();

    // Only 3 dates have both sleep and steps; 2024-01-04 has no sleep entry.
    assert_eq!(rows.len(), 3);
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys, vec!["2024-01-01", "2024-01-02", "2024-01-03"]);
    store.close().unwrap();
}

#[test]
fn join_values_are_correct() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_daily_store(&store);

    let rows = store
        .join_observations("obs://health.sleep", "obs://health.steps", date_key)
        .unwrap();

    // Verify left/right payloads decode to the expected f64 values.
    for row in &rows {
        let sleep = decode(row.left.payload.as_deref().unwrap_or(&[]));
        let steps = decode(row.right.payload.as_deref().unwrap_or(&[]));
        assert!(sleep.is_some(), "sleep payload must decode for {}", row.key);
        assert!(steps.is_some(), "steps payload must decode for {}", row.key);
    }
    let jan1 = rows.iter().find(|r| r.key == "2024-01-01").unwrap();
    assert_eq!(decode(jan1.left.payload.as_deref().unwrap()), Some(7.0));
    assert_eq!(decode(jan1.right.payload.as_deref().unwrap()), Some(9000.0));
    store.close().unwrap();
}

#[test]
fn join_returns_empty_when_no_overlap() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("obs://a/2024-01-01"), encode(1.0));
    tx.put_object(oid("obs://b/2024-01-02"), encode(2.0));
    store.commit(tx, "test", "no-overlap").unwrap();

    let rows = store
        .join_observations("obs://a", "obs://b", date_key)
        .unwrap();

    assert!(rows.is_empty(), "disjoint key sets must produce empty join");
    store.close().unwrap();
}

#[test]
fn join_deduplicates_by_latest_commit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Two sleep observations share the same date key; second commit should win.
    let mut tx1 = Transaction::new();
    tx1.put_object(oid("obs://health.sleep/2024-01-01"), encode(7.0));
    store.commit(tx1, "test", "first").unwrap();

    // Different object_id but same date key — should replace in the join map.
    let mut tx2 = Transaction::new();
    tx2.put_object(oid("obs://health.sleep_v2/2024-01-01"), encode(7.5));
    store.commit(tx2, "test", "second").unwrap();

    let mut tx3 = Transaction::new();
    tx3.put_object(oid("obs://health.steps/2024-01-01"), encode(9000.0));
    store.commit(tx3, "test", "steps").unwrap();

    // Join sleep (both variants share key "2024-01-01") with steps.
    let rows = store
        .join_observations("obs://health.sleep", "obs://health.steps", date_key)
        .unwrap();

    // Only 1 row for the date; left comes from the sleep namespace only.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "2024-01-01");
    store.close().unwrap();
}

#[test]
fn join_excludes_none_keys() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep/2024-01-01"), encode(7.0));
    tx.put_object(oid("obs://health.steps/2024-01-01"), encode(9000.0));
    store.commit(tx, "test", "seed").unwrap();

    // Key function returns None for everything — join must be empty.
    let rows = store
        .join_observations("obs://health.sleep", "obs://health.steps", |_| None)
        .unwrap();

    assert!(rows.is_empty(), "None key_fn must produce empty join");
    store.close().unwrap();
}

#[test]
fn join_results_are_sorted_by_key() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep/2024-01-03"), encode(6.5));
    tx.put_object(oid("obs://health.sleep/2024-01-01"), encode(7.0));
    tx.put_object(oid("obs://health.sleep/2024-01-02"), encode(8.0));
    tx.put_object(oid("obs://health.steps/2024-01-01"), encode(9000.0));
    tx.put_object(oid("obs://health.steps/2024-01-02"), encode(11000.0));
    tx.put_object(oid("obs://health.steps/2024-01-03"), encode(7500.0));
    store.commit(tx, "test", "seed").unwrap();

    let rows = store
        .join_observations("obs://health.sleep", "obs://health.steps", date_key)
        .unwrap();

    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "join results must be sorted by key");
    store.close().unwrap();
}
