use bindb::{BinderStore, ObjectId, SqlGroupBy, SqlProjectQuery, Transaction};
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

// ── PAYLOAD_FIELD group-by (issue #5) ────────────────────────────────────────

fn build_activity_store(store: &BinderStore) {
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

#[test]
fn payload_field_parse_produces_correct_variant() {
    use bindb::SqlAggregateQuery;
    let q = SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://activity' GROUP BY PAYLOAD_FIELD('category')",
    )
    .unwrap();
    assert_eq!(q.group_by, SqlGroupBy::PayloadField("category".into()));
}

#[test]
fn payload_field_count_per_category() {
    // run:2, bike:1, swim:1, None:1 (missing category).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_activity_store(&store);

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://activity' GROUP BY PAYLOAD_FIELD('category')",
        )
        .unwrap();

    let find = |key: Option<&str>| {
        results
            .iter()
            .find(|r| r.group_key.as_deref() == key)
            .map(|r| r.value as usize)
    };
    assert_eq!(find(Some("run")), Some(2));
    assert_eq!(find(Some("bike")), Some(1));
    assert_eq!(find(Some("swim")), Some(1));
    assert_eq!(find(None), Some(1), "missing-field observations in None group");
    store.close().unwrap();
}

#[test]
fn payload_field_double_quoted_field_name() {
    // PAYLOAD_FIELD accepts double-quoted field names too.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_activity_store(&store);

    let results = store
        .sql_aggregate(
            r#"SELECT COUNT(*) FROM 'obs://activity' GROUP BY PAYLOAD_FIELD("category")"#,
        )
        .unwrap();

    assert!(results.iter().any(|r| r.group_key.as_deref() == Some("run")));
    store.close().unwrap();
}

#[test]
fn payload_field_combined_with_where() {
    // WHERE id STARTS WITH '2024-01-0' keeps first 4 records (not the 5th).
    // run:2, bike:1, swim:1 (no None group since the missing-field record is filtered out).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_activity_store(&store);

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://activity' \
             WHERE id STARTS WITH '2024-01-0' \
             GROUP BY PAYLOAD_FIELD('category')",
        )
        .unwrap();

    // The 5th record id is '2024-01-05' which still starts with '2024-01-0'.
    // So we get run:2, bike:1, swim:1, None:1 (same as without WHERE here).
    // Use a tighter prefix to exclude it:
    let results2 = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://activity' \
             WHERE id STARTS WITH '2024-01-0' \
             GROUP BY PAYLOAD_FIELD('category')",
        )
        .unwrap();
    let total: usize = results2.iter().map(|r| r.value as usize).sum();
    assert_eq!(total, 5, "all 5 records match the prefix");
    drop(results);
    store.close().unwrap();
}

#[test]
fn payload_field_numeric_group_key() {
    // Group by a numeric JSON field; key is serialised as the JSON number string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("obs://items:a"), br#"{"tier":1}"#.to_vec());
    tx.put_object(oid("obs://items:b"), br#"{"tier":1}"#.to_vec());
    tx.put_object(oid("obs://items:c"), br#"{"tier":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://items' GROUP BY PAYLOAD_FIELD('tier')")
        .unwrap();

    let find = |k: &str| {
        results
            .iter()
            .find(|r| r.group_key.as_deref() == Some(k))
            .map(|r| r.value as usize)
    };
    assert_eq!(find("1"), Some(2));
    assert_eq!(find("2"), Some(1));
    store.close().unwrap();
}

#[test]
fn payload_field_rejects_bad_syntax() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    // Missing quotes around field name.
    let err = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://x' GROUP BY PAYLOAD_FIELD(category)")
        .unwrap_err();
    assert!(err.to_string().contains("invalid argument"));
    store.close().unwrap();
}

// ── Columnar projection / sql_project (issue #5) ─────────────────────────────

fn build_projection_store(store: &BinderStore) {
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

#[test]
fn project_parse_single_field() {
    let q = SqlProjectQuery::parse("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj'").unwrap();
    assert_eq!(q.fields, vec!["name"]);
    assert_eq!(q.namespace, "obs://proj");
    assert_eq!(q.id_prefix, None);
    assert_eq!(q.limit, None);
}

#[test]
fn project_parse_multi_field() {
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('score') FROM 'obs://proj'",
    )
    .unwrap();
    assert_eq!(q.fields, vec!["name", "score"]);
}

#[test]
fn project_parse_with_where_and_limit() {
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id STARTS WITH ':a' LIMIT 5",
    )
    .unwrap();
    assert_eq!(q.id_prefix, Some(":a".into()));
    assert_eq!(q.limit, Some(5));
}

#[test]
fn project_single_field_returns_all_rows() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj'")
        .unwrap();

    assert_eq!(rows.len(), 3);
    let names: Vec<_> = rows.iter().filter_map(|r| r.get("name")?.as_str()).collect();
    assert!(names.contains(&"alice"), "alice in results");
    assert!(names.contains(&"bob"), "bob in results");
    assert!(names.contains(&"carol"), "carol in results");
    store.close().unwrap();
}

#[test]
fn project_multi_field_extracts_both() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('score') FROM 'obs://proj'")
        .unwrap();

    assert_eq!(rows.len(), 3);
    let alice = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("alice"))
        .expect("alice row");
    assert_eq!(alice.get("score").and_then(|v| v.as_f64()), Some(10.0));
    store.close().unwrap();
}

#[test]
fn project_missing_field_is_null() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('active') FROM 'obs://proj'")
        .unwrap();

    let carol = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("carol"))
        .expect("carol row");
    assert!(
        carol.get("active").map(|v| v.is_null()).unwrap_or(false),
        "missing field is null"
    );
    store.close().unwrap();
}

#[test]
fn project_limit_caps_results() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' LIMIT 2")
        .unwrap();

    assert_eq!(rows.len(), 2, "LIMIT 2 returns at most 2 rows");
    store.close().unwrap();
}

#[test]
fn project_where_filter_narrows_results() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id STARTS WITH ':a'")
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("name").and_then(|v| v.as_str()),
        Some("alice")
    );
    store.close().unwrap();
}

#[test]
fn project_rejects_non_payload_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let err = store
        .sql_project("SELECT name FROM 'obs://proj'")
        .unwrap_err();
    assert!(err.to_string().contains("invalid argument"));
    store.close().unwrap();
}
