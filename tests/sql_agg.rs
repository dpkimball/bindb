use bindb::{BinderStore, ObjectId, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
fn oid(s: &str) -> ObjectId {
    ObjectId::new(s)
}
fn encode(v: f64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn build_sleep_store(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep:withings:01"), encode(7.0));
    tx.put_object(oid("obs://health.sleep:withings:02"), encode(6.0));
    tx.put_object(oid("obs://health.sleep:oura:01"), encode(8.0));
    store.commit(tx, "test", "seed").unwrap();
}

#[test]
fn sql_count() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://health.sleep'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as usize, 3);
    store.close().unwrap();
}

#[test]
fn sql_avg() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    let results = store
        .sql_aggregate("SELECT AVG(VALUE) FROM 'obs://health.sleep'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!((results[0].value - 7.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn sql_avg_group_by_source() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);

    // obs:// health.sleep withings|oura …
    // segments: ["obs", "health.sleep", "withings"|"oura", "01"|"02"]
    let results = store
        .sql_aggregate("SELECT AVG(VALUE) FROM 'obs://health.sleep' GROUP BY PATH_SEGMENT(2)")
        .unwrap();
    assert_eq!(results.len(), 2);
    let withings = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("withings"))
        .expect("withings group");
    let oura = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("oura"))
        .expect("oura group");
    assert!((withings.value - 6.5).abs() < 1e-9, "avg(7,6)=6.5");
    assert!((oura.value - 8.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn sql_rejects_unknown() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let err = store
        .sql_aggregate("SELECT * FROM 'obs://health.sleep'")
        .unwrap_err();
    assert!(err.to_string().contains("invalid argument"));
    store.close().unwrap();
}
