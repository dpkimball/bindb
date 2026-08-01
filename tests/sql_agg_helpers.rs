#[allow(dead_code, unused_imports)]
pub use bindb::{
    BinderStore, ObjectId, SqlAggregateQuery, SqlGroupBy, SqlProjectQuery, SqlProjectUnionQuery,
    Transaction,
};
pub use tempfile::TempDir;

pub fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
pub fn oid(s: &str) -> ObjectId {
    ObjectId::new(s)
}
pub fn encode(v: f64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

pub fn build_sleep_store(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://health.sleep:withings:01"), encode(7.0));
    tx.put_object(oid("obs://health.sleep:withings:02"), encode(6.0));
    tx.put_object(oid("obs://health.sleep:oura:01"), encode(8.0));
    store.commit(tx, "test", "seed").unwrap();
}

pub fn build_activity_store(store: &BinderStore) {
    // Observations with JSON payloads: {"category": "...", "duration": N}
    // Two runs (category="run"), one bike (category="bike"), one swim (category="swim").
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://activity:2024-01-01"),
        br#"{"category":"run","duration":30}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://activity:2024-01-02"),
        br#"{"category":"run","duration":45}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://activity:2024-01-03"),
        br#"{"category":"bike","duration":60}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://activity:2024-01-04"),
        br#"{"category":"swim","duration":25}"#.to_vec(),
    );
    // One observation missing the category field.
    tx.put_object(
        oid("obs://activity:2024-01-05"),
        br#"{"duration":10}"#.to_vec(),
    );
    store.commit(tx, "test", "seed").unwrap();
}

pub fn build_projection_store(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://proj:a"),
        br#"{"name":"alice","score":10,"active":true}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://proj:b"),
        br#"{"name":"bob","score":20,"active":false}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://proj:c"),
        br#"{"name":"carol","score":30}"#.to_vec(), // no active field
    );
    store.commit(tx, "test", "seed").unwrap();
}

pub fn build_having_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // Three sensors with different observation counts; values vary by sensor
    // sensor:a — 3 observations, avg = (6+7+8)/3 = 7.0
    tx.put_object(oid("obs://sensors:a:1"), encode(6.0));
    tx.put_object(oid("obs://sensors:a:2"), encode(7.0));
    tx.put_object(oid("obs://sensors:a:3"), encode(8.0));
    // sensor:b — 2 observations, avg = (3+5)/2 = 4.0
    tx.put_object(oid("obs://sensors:b:1"), encode(3.0));
    tx.put_object(oid("obs://sensors:b:2"), encode(5.0));
    // sensor:c — 1 observation, avg = 9.0
    tx.put_object(oid("obs://sensors:c:1"), encode(9.0));
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_order_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // group x: 3 obs, avg = 9.0
    tx.put_object(oid("obs://ord:x:1"), encode(9.0));
    tx.put_object(oid("obs://ord:x:2"), encode(9.0));
    tx.put_object(oid("obs://ord:x:3"), encode(9.0));
    // group y: 1 obs, avg = 2.0
    tx.put_object(oid("obs://ord:y:1"), encode(2.0));
    // group z: 2 obs, avg = 5.0
    tx.put_object(oid("obs://ord:z:1"), encode(5.0));
    tx.put_object(oid("obs://ord:z:2"), encode(5.0));
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_id_filter_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // IDs: sensor:temperature:room1, sensor:humidity:room1, sensor:temperature:room2, actuator:fan:room1
    tx.put_object(oid("obs://devices:sensor:temperature:room1"), encode(21.5));
    tx.put_object(oid("obs://devices:sensor:humidity:room1"), encode(55.0));
    tx.put_object(oid("obs://devices:sensor:temperature:room2"), encode(22.0));
    tx.put_object(oid("obs://devices:actuator:fan:room1"), encode(1.0));
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_payload_filter_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://users:1"),
        br#"{"name":"alice","score":90,"status":"active"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://users:2"),
        br#"{"name":"bob","score":40,"status":"inactive"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://users:3"),
        br#"{"name":"carol","score":75,"status":"active"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://users:4"),
        br#"{"name":"dave","score":60}"#.to_vec(), // no status field
    );
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_agg_payload_filter_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // ns: obs://metrics; 3 cpu, 2 memory; 3 in 'us' region (h1), 2 in 'eu' region (h2)
    tx.put_object(
        oid("obs://metrics:h1:c1"),
        br#"{"type":"cpu","region":"us"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://metrics:h1:c2"),
        br#"{"type":"cpu","region":"us"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://metrics:h1:m1"),
        br#"{"type":"memory","region":"us"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://metrics:h2:c1"),
        br#"{"type":"cpu","region":"eu"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://metrics:h2:m1"),
        br#"{"type":"memory","region":"eu"}"#.to_vec(),
    );
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_order_by_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://items:1"),
        br#"{"name":"banana","price":1.5}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://items:2"),
        br#"{"name":"apple","price":3.0}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://items:3"),
        br#"{"name":"cherry","price":0.5}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://items:4"),
        br#"{"name":"date","price":2.0}"#.to_vec(),
    );
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_between_sql_store() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (id, score) in [("a", 10), ("b", 20), ("c", 30), ("d", 40), ("e", 50)] {
        tx.put_object(
            oid(&format!("obs://scores:{id}")),
            format!(r#"{{"score":{score}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "seed").unwrap();
    (store, dir)
}

pub fn build_union_store(dir: &std::path::Path) -> BinderStore {
    let store = BinderStore::open(dir).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://users:alice"),
        br#"{"name":"Alice","role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://users:bob"),
        br#"{"name":"Bob","role":"user"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://guests:carol"),
        br#"{"name":"Carol","role":"guest"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://guests:dave"),
        br#"{"name":"Dave","role":"guest"}"#.to_vec(),
    );
    store.commit(tx, "test", "union_store").unwrap();
    store
}

pub fn build_collect_store(dir: &std::path::Path) -> bindb::BinderStore {
    let store = bindb::BinderStore::open(dir).unwrap();
    let mut tx = bindb::Transaction::new();
    tx.put_object(
        oid("obs://team:alice"),
        br#"{"name":"Alice","role":"admin","score":90}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://team:bob"),
        br#"{"name":"Bob","role":"user","score":70}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://team:carol"),
        br#"{"name":"Carol","role":"admin","score":85}"#.to_vec(),
    );
    store.commit(tx, "test", "collect_store").unwrap();
    store
}

pub fn build_array_store(dir: &std::path::Path) -> bindb::BinderStore {
    let store = bindb::BinderStore::open(dir).unwrap();
    let mut tx = bindb::Transaction::new();
    tx.put_object(
        oid("obs://arr:one"),
        br#"{"tags":["rust","perf","db"],"score":3}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://arr:two"),
        br#"{"tags":["graph"],"score":1}"#.to_vec(),
    );
    tx.put_object(oid("obs://arr:three"), br#"{"tags":[],"score":0}"#.to_vec());
    store.commit(tx, "test", "array_store").unwrap();
    store
}
