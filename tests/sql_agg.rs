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
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://activity' GROUP BY PAYLOAD_FIELD('category')")
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
    assert_eq!(
        find(None),
        Some(1),
        "missing-field observations in None group"
    );
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

    assert!(results
        .iter()
        .any(|r| r.group_key.as_deref() == Some("run")));
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
    assert_eq!(q.id_filter, None);
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
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id STARTS WITH 'a' LIMIT 5",
    )
    .unwrap();
    assert_eq!(
        q.id_filter,
        Some(bindb::SqlIdFilter::StartsWith("a".into()))
    );
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
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
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
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id STARTS WITH 'a'")
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
    store.close().unwrap();
}

#[test]
fn project_where_id_ends_with() {
    // IDs: obs://proj:a, :b, :c — ENDS WITH 'c' → only carol
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id ENDS WITH 'c'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("carol"));
    store.close().unwrap();
}

#[test]
fn project_where_id_contains() {
    // IDs: obs://proj:a, :b, :c — CONTAINS 'b' → only bob
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id CONTAINS 'b'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("bob"));
    store.close().unwrap();
}

#[test]
fn project_where_id_equals() {
    // Exact match — id = 'a' → only alice
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id = 'a'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
    store.close().unwrap();
}

#[test]
fn project_where_id_and_payload_filter() {
    // WHERE id STARTS WITH 'a' AND PAYLOAD_FIELD('score') > 5 → alice only
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://proj' WHERE id STARTS WITH 'a' AND PAYLOAD_FIELD('score') > 5",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
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

// ── HAVING clause tests (issue #5) ───────────────────────────────────────────

fn build_having_store() -> (BinderStore, TempDir) {
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

#[test]
fn having_count_gt_filters_small_groups() {
    // HAVING COUNT(*) > 1 → keep a (3) and b (2), drop c (1)
    let (store, _dir) = build_having_store();
    let mut results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://sensors' GROUP BY PATH_SEGMENT(2) HAVING COUNT(*) > 1",
        )
        .unwrap();
    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("a"), Some("b")]);
    store.close().unwrap();
}

#[test]
fn having_count_gte_boundary() {
    // HAVING COUNT(*) >= 3 → only a (3)
    let (store, _dir) = build_having_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://sensors' GROUP BY PATH_SEGMENT(2) HAVING COUNT(*) >= 3",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].group_key.as_deref(), Some("a"));
    assert_eq!(results[0].count, 3);
    store.close().unwrap();
}

#[test]
fn having_value_gt_filters_low_avg() {
    // HAVING AVG(VALUE) > 6.0 → a (7.0) and c (9.0), drop b (4.0)
    let (store, _dir) = build_having_store();
    let mut results = store
        .sql_aggregate(
            "SELECT AVG(VALUE) FROM 'obs://sensors' GROUP BY PATH_SEGMENT(2) HAVING AVG(VALUE) > 6.0",
        )
        .unwrap();
    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("a"), Some("c")]);
    store.close().unwrap();
}

#[test]
fn having_no_group_by_filters_total() {
    // No GROUP BY + HAVING COUNT(*) > 10 → empty (only 6 total observations)
    let (store, _dir) = build_having_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://sensors' HAVING COUNT(*) > 10")
        .unwrap();
    assert!(results.is_empty());
    store.close().unwrap();
}

#[test]
fn having_no_group_by_passes_when_matching() {
    // HAVING COUNT(*) >= 6 → one ungrouped result with count = 6
    let (store, _dir) = build_having_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://sensors' HAVING COUNT(*) >= 6")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 6);
    store.close().unwrap();
}

#[test]
fn having_parse_error_on_bad_syntax() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let err = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://x' HAVING invalid")
        .unwrap_err();
    assert!(err.to_string().contains("invalid argument"));
    store.close().unwrap();
}

// ── ORDER BY tests for sql_aggregate (issue #5) ──────────────────────────────

fn build_order_store() -> (BinderStore, TempDir) {
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

#[test]
fn order_by_value_asc() {
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY value ASC",
        )
        .unwrap();
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("y"), Some("z"), Some("x")]);
    store.close().unwrap();
}

#[test]
fn order_by_value_desc() {
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY value DESC",
        )
        .unwrap();
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("x"), Some("z"), Some("y")]);
    store.close().unwrap();
}

#[test]
fn order_by_count_desc() {
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY count DESC",
        )
        .unwrap();
    // x=3, z=2, y=1
    assert_eq!(results[0].count, 3);
    assert_eq!(results[1].count, 2);
    assert_eq!(results[2].count, 1);
    store.close().unwrap();
}

#[test]
fn order_by_group_key_desc() {
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY group_key DESC",
        )
        .unwrap();
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("z"), Some("y"), Some("x")]);
    store.close().unwrap();
}

#[test]
fn order_by_default_asc() {
    // No direction keyword → defaults to ASC
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate("SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY value")
        .unwrap();
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("y"), Some("z"), Some("x")]);
    store.close().unwrap();
}

#[test]
fn order_by_with_having() {
    // HAVING COUNT(*) > 1 removes y; ORDER BY value DESC → x then z
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate("SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) HAVING COUNT(*) > 1 ORDER BY value DESC")
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].group_key.as_deref(), Some("x"));
    assert_eq!(results[1].group_key.as_deref(), Some("z"));
    store.close().unwrap();
}

// ── LIMIT tests for sql_aggregate (issue #5) ─────────────────────────────────

#[test]
fn agg_limit_caps_group_count() {
    // 3 groups (x=3, y=1, z=2); ORDER BY count DESC LIMIT 2 → x then z
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY count DESC LIMIT 2",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].count, 3); // x
    assert_eq!(results[1].count, 2); // z
    store.close().unwrap();
}

#[test]
fn agg_limit_larger_than_results() {
    // LIMIT 100 on 3 groups → all 3 returned
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) LIMIT 100")
        .unwrap();
    assert_eq!(results.len(), 3);
    store.close().unwrap();
}

#[test]
fn agg_limit_with_having() {
    // HAVING removes y → 2 groups; LIMIT 1 → 1
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) HAVING COUNT(*) > 1 ORDER BY count DESC LIMIT 1",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3); // x
    store.close().unwrap();
}

// ── OFFSET tests for sql_aggregate (issue #5) ────────────────────────────────

#[test]
fn agg_offset_skips_first_n() {
    // 3 groups sorted by value ASC: y(2.0), z(5.0), x(9.0)
    // OFFSET 1 → skip y → z, x
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY value ASC OFFSET 1",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].group_key.as_deref(), Some("z"));
    store.close().unwrap();
}

#[test]
fn agg_offset_with_limit() {
    // OFFSET 1 LIMIT 1: middle element → z
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate(
            "SELECT AVG(VALUE) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) ORDER BY value ASC OFFSET 1 LIMIT 1",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].group_key.as_deref(), Some("z"));
    store.close().unwrap();
}

#[test]
fn agg_offset_beyond_end_is_empty() {
    let (store, _dir) = build_order_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://ord' GROUP BY PATH_SEGMENT(2) OFFSET 100")
        .unwrap();
    assert!(results.is_empty());
    store.close().unwrap();
}

// ── SqlIdFilter WHERE variants (issue #5) ────────────────────────────────────

fn build_id_filter_store() -> (BinderStore, TempDir) {
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

#[test]
fn id_filter_starts_with_selects_prefix() {
    // WHERE id STARTS WITH 'sensor' → 3 obs
    let (store, _dir) = build_id_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://devices' WHERE id STARTS WITH 'sensor'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    store.close().unwrap();
}

#[test]
fn id_filter_ends_with_selects_suffix() {
    // WHERE id ENDS WITH 'room1' → sensor:temperature:room1, sensor:humidity:room1, actuator:fan:room1
    let (store, _dir) = build_id_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://devices' WHERE id ENDS WITH 'room1'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    store.close().unwrap();
}

#[test]
fn id_filter_contains_selects_substring() {
    // WHERE id CONTAINS 'temperature' → room1 and room2
    let (store, _dir) = build_id_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://devices' WHERE id CONTAINS 'temperature'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

#[test]
fn id_filter_equals_selects_exact() {
    // WHERE id = 'sensor:humidity:room1' → exactly 1 observation
    let (store, _dir) = build_id_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://devices' WHERE id = 'sensor:humidity:room1'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 1);
    store.close().unwrap();
}

#[test]
fn id_filter_parse_roundtrip() {
    use bindb::{SqlAggregateQuery, SqlIdFilter};
    let q = SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://x' WHERE id ENDS WITH 'suffix'")
        .unwrap();
    assert_eq!(q.id_filter, Some(SqlIdFilter::EndsWith("suffix".into())));

    let q2 = SqlAggregateQuery::parse("SELECT AVG(VALUE) FROM 'obs://x' WHERE id CONTAINS 'mid'")
        .unwrap();
    assert_eq!(q2.id_filter, Some(SqlIdFilter::Contains("mid".into())));

    let q3 =
        SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://x' WHERE id = 'exact:id'").unwrap();
    assert_eq!(q3.id_filter, Some(SqlIdFilter::Equals("exact:id".into())));
}

#[test]
fn id_filter_combined_with_group_by() {
    // WHERE id CONTAINS 'sensor' GROUP BY PATH_SEGMENT(3) → temperature, humidity groups
    let (store, _dir) = build_id_filter_store();
    let mut results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://devices' WHERE id CONTAINS 'sensor' GROUP BY PATH_SEGMENT(3)",
        )
        .unwrap();
    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("humidity"), Some("temperature")]);
    store.close().unwrap();
}

// ── WHERE PAYLOAD_FIELD filter for sql_project (issue #5) ────────────────────

fn build_payload_filter_store() -> (BinderStore, TempDir) {
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

#[test]
fn project_payload_filter_eq_string() {
    // WHERE PAYLOAD_FIELD('status') = 'active' → alice and carol
    let (store, _dir) = build_payload_filter_store();
    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('status') = 'active'",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(rows[1].get("name").and_then(|v| v.as_str()), Some("carol"));
    store.close().unwrap();
}

#[test]
fn project_payload_filter_gte_num() {
    // WHERE PAYLOAD_FIELD('score') >= 75 → alice(90) and carol(75)
    let (store, _dir) = build_payload_filter_store();
    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('score') >= 75",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
    assert_eq!(rows[1].get("name").and_then(|v| v.as_str()), Some("carol"));
    store.close().unwrap();
}

#[test]
fn project_payload_filter_lt_num() {
    // WHERE PAYLOAD_FIELD('score') < 70 → bob(40) and dave(60)
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('score') < 70",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn project_payload_filter_is_null_skips_present_field() {
    // WHERE PAYLOAD_FIELD('status') IS NULL → dave only (no status field)
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('status') IS NULL",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("dave"));
    store.close().unwrap();
}

#[test]
fn project_payload_filter_is_not_null_returns_present() {
    // WHERE PAYLOAD_FIELD('status') IS NOT NULL → alice, bob, carol (not dave)
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('status') IS NOT NULL",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn project_payload_filter_contains_string() {
    // WHERE PAYLOAD_FIELD('name') CONTAINS 'ar' → carol
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('name') CONTAINS 'ar'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("carol"));
    store.close().unwrap();
}

#[test]
fn project_payload_filter_combined_with_limit() {
    // WHERE PAYLOAD_FIELD('score') > 50 LIMIT 2 → at most 2 of alice/carol/dave
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('score') > 50 LIMIT 2",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn project_payload_filter_parse_roundtrip() {
    use bindb::{SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' WHERE PAYLOAD_FIELD('score') >= 10",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "score".into(),
            op: SqlPayloadOp::Gte,
            value: SqlPayloadValue::Num(10.0),
        }
    );
    assert_eq!(q.id_filter, None);
}

// ── WHERE PAYLOAD_FIELD filter for sql_aggregate (issue #5) ──────────────────

fn build_agg_payload_filter_store() -> (BinderStore, TempDir) {
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

#[test]
fn agg_payload_filter_eq_string_count() {
    // WHERE PAYLOAD_FIELD('type') = 'cpu' → 3 cpu obs
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    store.close().unwrap();
}

#[test]
fn agg_payload_filter_with_group_by() {
    // WHERE PAYLOAD_FIELD('type') = 'cpu' GROUP BY PAYLOAD_FIELD('region')
    let (store, _dir) = build_agg_payload_filter_store();
    let mut results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' GROUP BY PAYLOAD_FIELD('region')",
        )
        .unwrap();
    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
    // us: h1:c1 and h1:c2 = 2; eu: h2:c1 = 1
    let keys: Vec<_> = results.iter().map(|r| r.group_key.as_deref()).collect();
    assert_eq!(keys, vec![Some("eu"), Some("us")]);
    assert_eq!(results[0].count, 1); // eu
    assert_eq!(results[1].count, 2); // us
    store.close().unwrap();
}

#[test]
fn agg_payload_filter_is_null_excludes_with_field() {
    // WHERE PAYLOAD_FIELD('type') IS NULL → all obs have 'type' field → no groups produced
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') IS NULL")
        .unwrap();
    assert!(
        results.is_empty(),
        "all obs have 'type' field so IS NULL produces no groups"
    );
    store.close().unwrap();
}

#[test]
fn agg_payload_filter_combined_id_and_payload_filter() {
    // WHERE id STARTS WITH 'h1' AND PAYLOAD_FIELD('type') = 'cpu' → h1:c1 and h1:c2
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE id STARTS WITH 'h1' AND PAYLOAD_FIELD('type') = 'cpu'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

#[test]
fn agg_payload_filter_parse_roundtrip() {
    use bindb::{SqlAggregateQuery, SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue};
    let q = SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://x' WHERE PAYLOAD_FIELD('status') = 'ok'",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "status".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("ok".into()),
        }
    );
    assert_eq!(q.id_filter, None);
}

// ── ORDER BY PAYLOAD_FIELD for sql_project (issue #5) ────────────────────────

fn build_order_by_store() -> (BinderStore, TempDir) {
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

#[test]
fn project_order_by_numeric_asc() {
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('price') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('price') ASC",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["cherry", "banana", "date", "apple"]);
    store.close().unwrap();
}

#[test]
fn project_order_by_numeric_desc() {
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('price') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('price') DESC",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["apple", "date", "banana", "cherry"]);
    store.close().unwrap();
}

#[test]
fn project_order_by_string_asc() {
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('name') ASC",
        )
        .unwrap();
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["apple", "banana", "cherry", "date"]);
    store.close().unwrap();
}

#[test]
fn project_order_by_with_limit() {
    // Order by price DESC then take top 2 → apple (3.0), date (2.0)
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('price') DESC LIMIT 2",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["apple", "date"]);
    store.close().unwrap();
}

#[test]
fn project_order_by_null_sorts_last() {
    // One item has no price; nulls should sort after all real values when ASC.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://things:1"),
        br#"{"name":"z","price":5.0}"#.to_vec(),
    );
    tx.put_object(oid("obs://things:2"), br#"{"name":"a"}"#.to_vec()); // no price
    tx.put_object(
        oid("obs://things:3"),
        br#"{"name":"m","price":2.0}"#.to_vec(),
    );
    store.commit(tx, "test", "seed").unwrap();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('price') FROM 'obs://things' ORDER BY PAYLOAD_FIELD('price') ASC",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // null price should be last
    let last = rows.last().unwrap();
    assert_eq!(last.get("price"), Some(&serde_json::Value::Null));
    assert_eq!(last.get("name").and_then(|v| v.as_str()), Some("a"));
    store.close().unwrap();
}

#[test]
fn project_order_by_parse_roundtrip() {
    use bindb::{AggOrderDirection, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' ORDER BY PAYLOAD_FIELD('score') DESC LIMIT 10",
    )
    .unwrap();
    assert_eq!(q.order_by.len(), 1);
    assert_eq!(q.order_by[0].field, "score");
    assert_eq!(q.order_by[0].direction, AggOrderDirection::Desc);
    assert_eq!(q.limit, Some(10));
    assert_eq!(q.id_filter, None);
}

// ── OFFSET for sql_project (issue #5) ────────────────────────────────────────

#[test]
fn project_offset_skips_rows() {
    // 4 items ordered by price ASC: cherry(0.5), banana(1.5), date(2.0), apple(3.0)
    // OFFSET 2 → date and apple
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('price') ASC OFFSET 2",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["date", "apple"]);
    store.close().unwrap();
}

#[test]
fn project_offset_and_limit_windowed() {
    // OFFSET 1 LIMIT 2 → banana(1.5), date(2.0) (middle window)
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('price') ASC OFFSET 1 LIMIT 2",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert_eq!(names, vec!["banana", "date"]);
    store.close().unwrap();
}

#[test]
fn project_offset_beyond_end_returns_empty() {
    let (store, _dir) = build_order_by_store();
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://items' OFFSET 100")
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

// ── SELECT id for sql_project (issue #5) ─────────────────────────────────────

#[test]
fn project_select_id_returns_object_id() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store.sql_project("SELECT id FROM 'obs://proj'").unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        let id = row.get("id").and_then(|v| v.as_str()).unwrap();
        assert!(
            id.starts_with("obs://proj:"),
            "id should be full object_id, got {id}"
        );
    }
    store.close().unwrap();
}

#[test]
fn project_select_id_and_payload_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT id, PAYLOAD_FIELD('name') FROM 'obs://proj'")
        .unwrap();
    assert_eq!(rows.len(), 3);
    let alice_row = rows
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("alice"))
        .unwrap();
    assert_eq!(
        alice_row.get("id").and_then(|v| v.as_str()),
        Some("obs://proj:a")
    );
    store.close().unwrap();
}

#[test]
fn project_select_id_with_filter() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT id FROM 'obs://proj' WHERE id STARTS WITH 'b'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("id").and_then(|v| v.as_str()),
        Some("obs://proj:b")
    );
    store.close().unwrap();
}

#[test]
fn project_select_id_order_by_id() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_projection_store(&store);
    let rows = store
        .sql_project("SELECT id, PAYLOAD_FIELD('name') FROM 'obs://proj' ORDER BY id ASC")
        .unwrap();
    assert_eq!(rows.len(), 3);
    let ids: Vec<_> = rows.iter().filter_map(|r| r.get("id")?.as_str()).collect();
    // IDs are obs://proj:a, obs://proj:b, obs://proj:c → lexicographic order
    assert_eq!(ids, vec!["obs://proj:a", "obs://proj:b", "obs://proj:c"]);
    store.close().unwrap();
}

#[test]
fn project_select_id_parse_roundtrip() {
    let q = SqlProjectQuery::parse("SELECT id, PAYLOAD_FIELD('score') FROM 'obs://x'").unwrap();
    assert!(q.return_id);
    assert_eq!(q.fields, vec!["score"]);
}

#[test]
fn project_select_id_only_parse() {
    let q = SqlProjectQuery::parse("SELECT id FROM 'obs://x'").unwrap();
    assert!(q.return_id);
    assert!(q.fields.is_empty());
}

// ── LIKE for sql_project and sql_aggregate (issue #5) ────────────────────────

#[test]
fn project_like_prefix() {
    // LIKE 'alice%' → only alice row
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('name') LIKE 'alice%'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("alice"));
    store.close().unwrap();
}

#[test]
fn project_like_suffix() {
    // LIKE '%ol' → carol
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('name') LIKE '%ol'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("carol"));
    store.close().unwrap();
}

#[test]
fn project_like_both_percent() {
    // LIKE '%o%' → bob, carol (both contain 'o')
    let (store, _dir) = build_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('name') LIKE '%o%'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: std::collections::HashSet<_> = rows
        .iter()
        .filter_map(|r| r.get("name")?.as_str())
        .collect();
    assert!(names.contains("bob") && names.contains("carol"));
    store.close().unwrap();
}

#[test]
fn project_like_parse_roundtrip() {
    use bindb::{SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' WHERE PAYLOAD_FIELD('name') LIKE 'a%'",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "name".into(),
            op: SqlPayloadOp::Like,
            value: SqlPayloadValue::Str("a%".into()),
        }
    );
}

#[test]
fn agg_like_counts_matching() {
    // COUNT(*) WHERE PAYLOAD_FIELD('name') LIKE '%o%' → 2 (bob, carol)
    let (store, _dir) = build_payload_filter_store();
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://users' WHERE PAYLOAD_FIELD('name') LIKE '%o%'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

// ── Chained AND PAYLOAD_FIELD filters (sql_aggregate + sql_project) ──────────

#[test]
fn agg_chained_payload_filter_two_conditions() {
    // WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us' → 2
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

#[test]
fn agg_chained_payload_filter_no_match() {
    // WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'au' → 0
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'au'",
        )
        .unwrap();
    assert!(results.is_empty());
    store.close().unwrap();
}

#[test]
fn agg_chained_payload_filter_with_id_filter() {
    // WHERE id STARTS WITH 'h1' AND PAYLOAD_FIELD('type') = 'cpu' → 2 (h1:c1, h1:c2)
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE id STARTS WITH 'h1' AND PAYLOAD_FIELD('type') = 'cpu'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

#[test]
fn agg_chained_payload_filter_parse_roundtrip() {
    use bindb::{SqlAggregateQuery, SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue};
    let q = SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://x' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us'",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 2);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "type".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("cpu".into()),
        }
    );
    assert_eq!(
        q.payload_filters[1],
        SqlPayloadFilter {
            field: "region".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("us".into()),
        }
    );
}

#[test]
fn project_chained_payload_filter_narrows_results() {
    // SELECT name WHERE type = 'cpu' AND region = 'us' → 2 rows
    let (store, _dir) = build_agg_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('type') FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row["type"], serde_json::Value::String("cpu".into()));
    }
    store.close().unwrap();
}

#[test]
fn project_chained_payload_filter_parse_roundtrip() {
    use bindb::{SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us'",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 2);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "type".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("cpu".into()),
        }
    );
    assert_eq!(
        q.payload_filters[1],
        SqlPayloadFilter {
            field: "region".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("us".into()),
        }
    );
}

// ── BETWEEN for sql_project and sql_aggregate (issue #5) ─────────────────────

fn build_between_sql_store() -> (BinderStore, TempDir) {
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

#[test]
fn project_between_filters_range() {
    let (store, _dir) = build_between_sql_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://scores' WHERE PAYLOAD_FIELD('score') BETWEEN 20 AND 40",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    let scores: Vec<i64> = rows
        .iter()
        .map(|r| r["score"].as_f64().unwrap() as i64)
        .collect();
    assert!(scores.contains(&20) && scores.contains(&30) && scores.contains(&40));
    store.close().unwrap();
}

#[test]
fn project_between_boundary_inclusive() {
    let (store, _dir) = build_between_sql_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://scores' WHERE PAYLOAD_FIELD('score') BETWEEN 10 AND 10",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["score"].as_f64().unwrap() as i64, 10);
    store.close().unwrap();
}

#[test]
fn project_between_no_match() {
    let (store, _dir) = build_between_sql_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://scores' WHERE PAYLOAD_FIELD('score') BETWEEN 60 AND 100",
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn agg_between_count() {
    let (store, _dir) = build_between_sql_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://scores' WHERE PAYLOAD_FIELD('score') BETWEEN 20 AND 40",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    store.close().unwrap();
}

#[test]
fn between_parse_roundtrip_sql() {
    use bindb::{SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('score') FROM 'obs://x' WHERE PAYLOAD_FIELD('score') BETWEEN 10 AND 50",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "score".into(),
            op: SqlPayloadOp::Between,
            value: SqlPayloadValue::NumRange(10.0, 50.0),
        }
    );
}

#[test]
fn between_chained_with_and_filter() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (id, score, kind) in [
        ("a", 10u32, "x"),
        ("b", 20, "y"),
        ("c", 30, "y"),
        ("d", 40, "x"),
        ("e", 50, "x"),
    ] {
        tx.put_object(
            oid(&format!("obs://items:{id}")),
            format!(r#"{{"score":{score},"kind":"{kind}"}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "seed").unwrap();
    // BETWEEN 15 AND 45 AND kind = 'y' → b(20) and c(30)
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://items' WHERE PAYLOAD_FIELD('score') BETWEEN 15 AND 45 AND PAYLOAD_FIELD('kind') = 'y'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

// ── OR disjunction for sql_project and sql_aggregate (issue #5) ──────────────

#[test]
fn project_or_filter_selects_either() {
    // type='cpu' OR type='memory' → all 5
    let (store, _dir) = build_agg_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('type') FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' OR PAYLOAD_FIELD('type') = 'memory'",
        )
        .unwrap();
    assert_eq!(rows.len(), 5);
    store.close().unwrap();
}

#[test]
fn project_or_filter_partial_match() {
    // type='cpu' OR region='eu' → 4 unique (3 cpu + eu-memory; h2:c1 is both → counted once)
    let (store, _dir) = build_agg_payload_filter_store();
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('type') FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' OR PAYLOAD_FIELD('region') = 'eu'",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    store.close().unwrap();
}

#[test]
fn agg_or_filter_count() {
    let (store, _dir) = build_agg_payload_filter_store();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://metrics' WHERE PAYLOAD_FIELD('type') = 'cpu' OR PAYLOAD_FIELD('type') = 'memory'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 5);
    store.close().unwrap();
}

#[test]
fn or_parse_roundtrip() {
    use bindb::{
        SqlFilterCombinator, SqlPayloadFilter, SqlPayloadOp, SqlPayloadValue, SqlProjectQuery,
    };
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('type') FROM 'obs://x' WHERE PAYLOAD_FIELD('type') = 'a' OR PAYLOAD_FIELD('type') = 'b'",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 2);
    assert_eq!(q.payload_combinator, SqlFilterCombinator::Or);
    assert_eq!(
        q.payload_filters[0],
        SqlPayloadFilter {
            field: "type".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("a".into()),
        }
    );
}

#[test]
fn and_parse_preserves_combinator() {
    use bindb::{SqlFilterCombinator, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('type') FROM 'obs://x' WHERE PAYLOAD_FIELD('type') = 'cpu' AND PAYLOAD_FIELD('region') = 'us'",
    )
    .unwrap();
    assert_eq!(q.payload_combinator, SqlFilterCombinator::And);
    assert_eq!(q.payload_filters.len(), 2);
}

// ── SELECT DISTINCT (sql_project) ─────────────────────────────────────────────

#[test]
fn select_distinct_removes_duplicate_rows() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("ns://a"), br#"{"color":"red"}"#.to_vec());
    tx.put_object(oid("ns://b"), br#"{"color":"red"}"#.to_vec());
    tx.put_object(oid("ns://c"), br#"{"color":"blue"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DISTINCT PAYLOAD_FIELD('color') FROM 'ns://'")
        .unwrap();
    assert_eq!(rows.len(), 2);
    let colors: std::collections::HashSet<_> = rows
        .iter()
        .filter_map(|r| r.get("color").and_then(|v| v.as_str()))
        .collect();
    assert!(colors.contains("red"));
    assert!(colors.contains("blue"));
    store.close().unwrap();
}

#[test]
fn select_distinct_without_dupes_returns_all() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("nd://x"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("nd://y"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("nd://z"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DISTINCT PAYLOAD_FIELD('v') FROM 'nd://'")
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn select_distinct_multi_field_dedup() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("mf://1"), br#"{"a":"x","b":"y"}"#.to_vec());
    tx.put_object(oid("mf://2"), br#"{"a":"x","b":"y"}"#.to_vec());
    tx.put_object(oid("mf://3"), br#"{"a":"x","b":"z"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DISTINCT PAYLOAD_FIELD('a'), PAYLOAD_FIELD('b') FROM 'mf://'")
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn select_distinct_with_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 0..6u32 {
        let payload = format!(r#"{{"g":{}}}"#, i % 3);
        tx.put_object(oid(&format!("lim://item{}", i)), payload.into_bytes());
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DISTINCT PAYLOAD_FIELD('g') FROM 'lim://' LIMIT 2")
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn select_distinct_parse_flag() {
    let q = SqlProjectQuery::parse("SELECT DISTINCT PAYLOAD_FIELD('x') FROM 'ns://'").unwrap();
    assert!(q.distinct);
    let q2 = SqlProjectQuery::parse("SELECT PAYLOAD_FIELD('x') FROM 'ns://'").unwrap();
    assert!(!q2.distinct);
}

// ── WHERE id IN ('a','b','c') ──────────────────────────────────────────────────
// Convention: namespace is `obs://ns`, object IDs are `obs://ns:suffix`.
// The execution strips the `obs://ns:` prefix so `id` equals the suffix.

#[test]
fn id_in_filter_project() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://items:alpha"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://items:beta"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://items:gamma"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('v') FROM 'obs://items' WHERE id IN ('alpha','gamma')")
        .unwrap();
    assert_eq!(rows.len(), 2);
    let vals: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.get("v").and_then(|v| v.as_i64()))
        .collect();
    assert!(vals.contains(&1));
    assert!(vals.contains(&3));
    store.close().unwrap();
}

#[test]
fn id_in_filter_single_item() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sing:one"), br#"{"n":"one"}"#.to_vec());
    tx.put_object(oid("obs://sing:two"), br#"{"n":"two"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('n') FROM 'obs://sing' WHERE id IN ('one')")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_str()), Some("one"));
    store.close().unwrap();
}

#[test]
fn id_in_filter_aggregate() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scores:p"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://scores:q"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://scores:r"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://scores' WHERE id IN ('p','r')")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 2);
    store.close().unwrap();
}

#[test]
fn id_in_filter_none_match() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nm:a"), br#"{"x":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('x') FROM 'obs://nm' WHERE id IN ('missing')")
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn id_in_filter_combined_with_payload() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://combo:a"), br#"{"status":"on"}"#.to_vec());
    tx.put_object(oid("obs://combo:b"), br#"{"status":"off"}"#.to_vec());
    tx.put_object(oid("obs://combo:c"), br#"{"status":"on"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    // id IN ('a','b','c') narrows to all three; payload filter keeps only 'on' → 2 rows
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('status') FROM 'obs://combo' WHERE id IN ('a','b','c') AND PAYLOAD_FIELD('status') = 'on'")
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

// ── PAYLOAD_FIELD IN ('a','b','c') / NOT IN ───────────────────────────────────

#[test]
fn payload_field_in_list_filters_rows() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://pf:a"), br#"{"color":"red"}"#.to_vec());
    tx.put_object(oid("obs://pf:b"), br#"{"color":"blue"}"#.to_vec());
    tx.put_object(oid("obs://pf:c"), br#"{"color":"green"}"#.to_vec());
    tx.put_object(oid("obs://pf:d"), br#"{"color":"yellow"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('color') FROM 'obs://pf' WHERE PAYLOAD_FIELD('color') IN ('red','green')")
        .unwrap();
    assert_eq!(rows.len(), 2);
    let colors: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("color").and_then(|v| v.as_str()))
        .collect();
    assert!(colors.contains(&"red"));
    assert!(colors.contains(&"green"));
    store.close().unwrap();
}

#[test]
fn payload_field_not_in_list_excludes_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ni:a"), br#"{"tier":"free"}"#.to_vec());
    tx.put_object(oid("obs://ni:b"), br#"{"tier":"pro"}"#.to_vec());
    tx.put_object(oid("obs://ni:c"), br#"{"tier":"enterprise"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('tier') FROM 'obs://ni' WHERE PAYLOAD_FIELD('tier') NOT IN ('pro')")
        .unwrap();
    assert_eq!(rows.len(), 2);
    let tiers: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("tier").and_then(|v| v.as_str()))
        .collect();
    assert!(!tiers.contains(&"pro"));
    store.close().unwrap();
}

#[test]
fn payload_field_in_list_aggregate_count() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://pia:a"), b"1.0".to_vec());
    tx.put_object(oid("obs://pia:b"), b"2.0".to_vec());
    tx.put_object(oid("obs://pia:c"), b"3.0".to_vec());
    store.commit(tx, "test", "").unwrap();

    // Use PAYLOAD_FIELD IN with non-JSON objects — all will be treated as null payload fields,
    // so none match, count = 0. Use a JSON-carrying namespace instead.
    let dir2 = tmp();
    let store2 = BinderStore::open(dir2.path()).unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_object(oid("obs://jpia:a"), br#"{"env":"prod"}"#.to_vec());
    tx2.put_object(oid("obs://jpia:b"), br#"{"env":"staging"}"#.to_vec());
    tx2.put_object(oid("obs://jpia:c"), br#"{"env":"dev"}"#.to_vec());
    tx2.put_object(oid("obs://jpia:d"), br#"{"env":"prod"}"#.to_vec());
    store2.commit(tx2, "test", "").unwrap();

    let results = store2
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://jpia' WHERE PAYLOAD_FIELD('env') IN ('prod','staging')",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    store2.close().unwrap();
}

#[test]
fn payload_field_in_empty_list_none_match() {
    // An empty IN list should match nothing. parse_quoted_list allows 0 items.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://emp:a"), br#"{"x":"y"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('x') FROM 'obs://emp' WHERE PAYLOAD_FIELD('x') IN ()")
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn payload_field_in_parse_produces_str_list() {
    use bindb::{SqlPayloadOp, SqlPayloadValue, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('color') FROM 'obs://x' WHERE PAYLOAD_FIELD('color') IN ('red','blue')",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(q.payload_filters[0].op, SqlPayloadOp::In);
    assert_eq!(
        q.payload_filters[0].value,
        SqlPayloadValue::StrList(vec!["red".into(), "blue".into()])
    );
}

// ── PAYLOAD_FIELD NOT LIKE 'pat%' ─────────────────────────────────────────────

#[test]
fn payload_field_not_like_excludes_matching() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://pnl:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(oid("obs://pnl:b"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(oid("obs://pnl:c"), br#"{"name":"anna"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    // NOT LIKE 'a%' excludes alice and anna; only bob passes
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') FROM 'obs://pnl' WHERE PAYLOAD_FIELD('name') NOT LIKE 'a%'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("bob"));
    store.close().unwrap();
}

#[test]
fn payload_field_not_like_aggregate() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://pnla:a"), br#"{"tag":"tmp_a"}"#.to_vec());
    tx.put_object(oid("obs://pnla:b"), br#"{"tag":"prod_b"}"#.to_vec());
    tx.put_object(oid("obs://pnla:c"), br#"{"tag":"tmp_c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://pnla' WHERE PAYLOAD_FIELD('tag') NOT LIKE 'tmp%'",
        )
        .unwrap();
    assert_eq!(results[0].count, 1);
    store.close().unwrap();
}

#[test]
fn payload_field_not_like_parse_op() {
    use bindb::{SqlPayloadOp, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' WHERE PAYLOAD_FIELD('name') NOT LIKE 'tmp%'",
    )
    .unwrap();
    assert_eq!(q.payload_filters[0].op, SqlPayloadOp::NotLike);
}

// ── SELECT PAYLOAD_FIELD('f') AS alias ────────────────────────────────────────

#[test]
fn select_payload_field_alias_renames_output_key() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://alias:a"), br#"{"name":"alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('name') AS n FROM 'obs://alias'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_str()), Some("alice"));
    assert!(!rows[0].contains_key("name"));
    store.close().unwrap();
}

#[test]
fn select_payload_field_alias_multiple_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://malias:a"),
        br#"{"first":"bob","score":42}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('first') AS fname, PAYLOAD_FIELD('score') AS pts FROM 'obs://malias'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("fname").and_then(|v| v.as_str()), Some("bob"));
    assert_eq!(rows[0].get("pts").and_then(|v| v.as_i64()), Some(42));
    assert!(!rows[0].contains_key("first"));
    assert!(!rows[0].contains_key("score"));
    store.close().unwrap();
}

#[test]
fn select_payload_field_alias_parse_stored() {
    use bindb::SqlProjectQuery;
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') AS n, PAYLOAD_FIELD('age') AS years FROM 'obs://x'",
    )
    .unwrap();
    assert_eq!(q.fields, vec!["name", "age"]);
    assert_eq!(q.field_aliases.get("name"), Some(&"n".to_string()));
    assert_eq!(q.field_aliases.get("age"), Some(&"years".to_string()));
}

#[test]
fn select_payload_field_no_alias_uses_raw_name() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://noalias:a"), br#"{"score":7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('score') FROM 'obs://noalias'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(7));
    store.close().unwrap();
}

// ── GROUP BY PAYLOAD_FIELD('f1'), PAYLOAD_FIELD('f2') ────────────────────────

#[test]
fn multi_payload_group_by_two_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://mgb:a"),
        br#"{"region":"us","tier":"free"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://mgb:b"),
        br#"{"region":"us","tier":"free"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://mgb:c"),
        br#"{"region":"eu","tier":"pro"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://mgb:d"),
        br#"{"region":"us","tier":"pro"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://mgb' GROUP BY PAYLOAD_FIELD('region'), PAYLOAD_FIELD('tier')",
        )
        .unwrap();
    // Groups: ["us","free"]:2, ["eu","pro"]:1, ["us","pro"]:1
    assert_eq!(results.len(), 3);
    let us_free = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some(r#"["us","free"]"#))
        .expect("us/free group");
    assert_eq!(us_free.count, 2);
    store.close().unwrap();
}

#[test]
fn multi_payload_group_by_missing_field_uses_null() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://mnull:a"), br#"{"f1":"x","f2":"y"}"#.to_vec());
    tx.put_object(oid("obs://mnull:b"), br#"{"f1":"x"}"#.to_vec()); // f2 missing
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://mnull' GROUP BY PAYLOAD_FIELD('f1'), PAYLOAD_FIELD('f2')",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    let null_group = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some(r#"["x",null]"#))
        .expect("null group for missing f2");
    assert_eq!(null_group.count, 1);
    store.close().unwrap();
}

#[test]
fn multi_payload_group_by_parse_produces_variant() {
    use bindb::{SqlAggregateQuery, SqlGroupBy};
    let q = SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://x' GROUP BY PAYLOAD_FIELD('region'), PAYLOAD_FIELD('tier')",
    )
    .unwrap();
    assert_eq!(
        q.group_by,
        SqlGroupBy::MultiPayloadField(vec!["region".into(), "tier".into()])
    );
}

#[test]
fn multi_payload_group_by_single_field_unchanged() {
    use bindb::{SqlAggregateQuery, SqlGroupBy};
    let q =
        SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://x' GROUP BY PAYLOAD_FIELD('region')")
            .unwrap();
    assert_eq!(q.group_by, SqlGroupBy::PayloadField("region".into()));
}

#[test]
fn multi_payload_group_by_with_having() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for _ in 0..3 {
        tx.put_object(
            oid(&format!("obs://mhav:{}", uuid())),
            br#"{"env":"prod","svc":"api"}"#.to_vec(),
        );
    }
    tx.put_object(
        oid("obs://mhav:single"),
        br#"{"env":"prod","svc":"db"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let results = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://mhav' GROUP BY PAYLOAD_FIELD('env'), PAYLOAD_FIELD('svc') HAVING COUNT(*) >= 2",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].group_key.as_deref().unwrap().contains("api"));
    store.close().unwrap();
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        .to_string()
}

// ── ILIKE tests ───────────────────────────────────────────────────────────────

#[test]
fn sql_ilike_matches_case_insensitively() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ilk:a"), br#"{"name":"Alice"}"#.to_vec());
    tx.put_object(oid("obs://ilk:b"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(oid("obs://ilk:c"), br#"{"name":"ALICE"}"#.to_vec());
    tx.put_object(oid("obs://ilk:d"), br#"{"name":"Bob"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://ilk' WHERE PAYLOAD_FIELD('name') ILIKE 'alice'",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn sql_ilike_wildcard() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ilk2:a"), br#"{"city":"New York"}"#.to_vec());
    tx.put_object(oid("obs://ilk2:b"), br#"{"city":"new york"}"#.to_vec());
    tx.put_object(oid("obs://ilk2:c"), br#"{"city":"Newark"}"#.to_vec());
    tx.put_object(oid("obs://ilk2:d"), br#"{"city":"Boston"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('city') FROM 'obs://ilk2' WHERE PAYLOAD_FIELD('city') ILIKE 'new%'",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn sql_not_ilike_excludes_matching() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nilk:a"), br#"{"role":"Admin"}"#.to_vec());
    tx.put_object(oid("obs://nilk:b"), br#"{"role":"admin"}"#.to_vec());
    tx.put_object(oid("obs://nilk:c"), br#"{"role":"user"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('role') FROM 'obs://nilk' WHERE PAYLOAD_FIELD('role') NOT ILIKE 'admin'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("role").and_then(|v| v.as_str()), Some("user"));
    store.close().unwrap();
}

#[test]
fn sql_ilike_in_aggregate_where() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://ilk3:a"),
        br#"{"tag":"Alpha","val":1.0}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://ilk3:b"),
        br#"{"tag":"alpha","val":2.0}"#.to_vec(),
    );
    tx.put_object(oid("obs://ilk3:c"), br#"{"tag":"beta","val":3.0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let result = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://ilk3' WHERE PAYLOAD_FIELD('tag') ILIKE 'alpha'")
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].count, 2);
    store.close().unwrap();
}

// ── Multi-condition HAVING tests ──────────────────────────────────────────────

#[test]
fn having_and_two_conditions() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // group A: 3 items
    for i in 0..3 {
        tx.put_object(
            oid(&format!("obs://hav:a{i}")),
            format!(r#"{{"g":"A","v":{}}}"#, i as f64 + 1.0).into_bytes(),
        );
    }
    // group B: 1 item
    tx.put_object(oid("obs://hav:b0"), br#"{"g":"B","v":10.0}"#.to_vec());
    // group C: 5 items
    for i in 0..5 {
        tx.put_object(
            oid(&format!("obs://hav:c{i}")),
            format!(r#"{{"g":"C","v":{}}}"#, i as f64 + 1.0).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    // HAVING COUNT(*) >= 2 AND COUNT(*) <= 4 → only group A (3)
    let result = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://hav' GROUP BY PAYLOAD_FIELD('g') HAVING COUNT(*) >= 2 AND COUNT(*) <= 4",
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].group_key.as_deref(), Some("A"));
    store.close().unwrap();
}

#[test]
fn having_or_two_conditions() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 0..2 {
        tx.put_object(oid(&format!("obs://hav2:a{i}")), br#"{"g":"A"}"#.to_vec());
    }
    for i in 0..4 {
        tx.put_object(oid(&format!("obs://hav2:b{i}")), br#"{"g":"B"}"#.to_vec());
    }
    for i in 0..3 {
        tx.put_object(oid(&format!("obs://hav2:c{i}")), br#"{"g":"C"}"#.to_vec());
    }
    store.commit(tx, "test", "").unwrap();

    // HAVING COUNT(*) = 2 OR COUNT(*) = 4 → groups A and B
    let result = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://hav2' GROUP BY PAYLOAD_FIELD('g') HAVING COUNT(*) = 2 OR COUNT(*) = 4",
        )
        .unwrap();
    assert_eq!(result.len(), 2);
    let keys: Vec<_> = result
        .iter()
        .map(|r| r.group_key.as_deref().unwrap_or(""))
        .collect();
    assert!(keys.contains(&"A"));
    assert!(keys.contains(&"B"));
    store.close().unwrap();
}

#[test]
fn having_single_condition_still_works() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 0..3 {
        tx.put_object(oid(&format!("obs://hav3:a{i}")), br#"{"g":"A"}"#.to_vec());
    }
    tx.put_object(oid("obs://hav3:b0"), br#"{"g":"B"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let result = store
        .sql_aggregate(
            "SELECT COUNT(*) FROM 'obs://hav3' GROUP BY PAYLOAD_FIELD('g') HAVING COUNT(*) > 1",
        )
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].group_key.as_deref(), Some("A"));
    store.close().unwrap();
}

#[test]
fn having_and_parse_stores_combinator() {
    let q = bindb::SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://x' GROUP BY PAYLOAD_FIELD('g') HAVING COUNT(*) > 1 AND COUNT(*) < 10",
    )
    .unwrap();
    assert_eq!(q.having.len(), 2);
    assert_eq!(q.having_combinator, bindb::SqlFilterCombinator::And);
}

#[test]
fn having_or_parse_stores_combinator() {
    let q = bindb::SqlAggregateQuery::parse(
        "SELECT COUNT(*) FROM 'obs://x' GROUP BY PAYLOAD_FIELD('g') HAVING COUNT(*) = 1 OR COUNT(*) = 5",
    )
    .unwrap();
    assert_eq!(q.having.len(), 2);
    assert_eq!(q.having_combinator, bindb::SqlFilterCombinator::Or);
}

// ── UPPER/LOWER/TRIM(PAYLOAD_FIELD) in SELECT tests ──────────────────────────

#[test]
fn select_upper_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sut:a"), br#"{"name":"alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT UPPER(PAYLOAD_FIELD('name')) FROM 'obs://sut'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("ALICE"));
    store.close().unwrap();
}

#[test]
fn select_lower_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slt:a"), br#"{"tag":"HELLO"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOWER(PAYLOAD_FIELD('tag')) FROM 'obs://slt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_trim_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://str:a"), br#"{"val":"  hi  "}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TRIM(PAYLOAD_FIELD('val')) FROM 'obs://str'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("val").and_then(|v| v.as_str()), Some("hi"));
    store.close().unwrap();
}

#[test]
fn select_transform_with_alias() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sta:a"), br#"{"city":"london"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT UPPER(PAYLOAD_FIELD('city')) AS c FROM 'obs://sta'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("c").and_then(|v| v.as_str()), Some("LONDON"));
    assert!(!rows[0].contains_key("city"));
    store.close().unwrap();
}

// ── Multi-field ORDER BY tests ────────────────────────────────────────────────

#[test]
fn multi_order_by_two_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://mob:a"), br#"{"dept":"eng","rank":2}"#.to_vec());
    tx.put_object(oid("obs://mob:b"), br#"{"dept":"eng","rank":1}"#.to_vec());
    tx.put_object(oid("obs://mob:c"), br#"{"dept":"art","rank":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('dept'), PAYLOAD_FIELD('rank') FROM 'obs://mob' ORDER BY PAYLOAD_FIELD('dept') ASC, PAYLOAD_FIELD('rank') ASC",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // art/1, eng/1, eng/2
    assert_eq!(rows[0].get("dept").and_then(|v| v.as_str()), Some("art"));
    assert_eq!(rows[1].get("dept").and_then(|v| v.as_str()), Some("eng"));
    assert_eq!(rows[1].get("rank").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[2].get("dept").and_then(|v| v.as_str()), Some("eng"));
    assert_eq!(rows[2].get("rank").and_then(|v| v.as_i64()), Some(2));
    store.close().unwrap();
}

#[test]
fn multi_order_by_asc_desc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://moad:a"), br#"{"dept":"eng","rank":2}"#.to_vec());
    tx.put_object(oid("obs://moad:b"), br#"{"dept":"eng","rank":1}"#.to_vec());
    tx.put_object(oid("obs://moad:c"), br#"{"dept":"art","rank":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('dept'), PAYLOAD_FIELD('rank') FROM 'obs://moad' ORDER BY PAYLOAD_FIELD('dept') ASC, PAYLOAD_FIELD('rank') DESC",
        )
        .unwrap();
    // art/3, eng/2, eng/1
    assert_eq!(rows[0].get("dept").and_then(|v| v.as_str()), Some("art"));
    assert_eq!(rows[1].get("rank").and_then(|v| v.as_i64()), Some(2));
    assert_eq!(rows[2].get("rank").and_then(|v| v.as_i64()), Some(1));
    store.close().unwrap();
}

#[test]
fn multi_order_by_parse_stored() {
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('a'), PAYLOAD_FIELD('b') FROM 'obs://x' ORDER BY PAYLOAD_FIELD('a') ASC, PAYLOAD_FIELD('b') DESC",
    )
    .unwrap();
    assert_eq!(q.order_by.len(), 2);
    assert_eq!(q.order_by[0].field, "a");
    assert_eq!(q.order_by[1].field, "b");
    assert_eq!(q.order_by[1].direction, bindb::AggOrderDirection::Desc);
}

#[test]
fn single_order_by_still_works() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sob:a"), br#"{"score":3}"#.to_vec());
    tx.put_object(oid("obs://sob:b"), br#"{"score":1}"#.to_vec());
    tx.put_object(oid("obs://sob:c"), br#"{"score":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://sob' ORDER BY PAYLOAD_FIELD('score') ASC",
        )
        .unwrap();
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[1].get("score").and_then(|v| v.as_i64()), Some(2));
    assert_eq!(rows[2].get("score").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

// ── WHERE id NOT IN ──────────────────────────────────────────────────────────

#[test]
fn id_not_in_project_excludes_listed() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ni:a"), br#"{"x":1}"#.to_vec());
    tx.put_object(oid("obs://ni:b"), br#"{"x":2}"#.to_vec());
    tx.put_object(oid("obs://ni:c"), br#"{"x":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('x') FROM 'obs://ni' WHERE id NOT IN ('a', 'b')")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("x").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

#[test]
fn id_not_in_aggregate_excludes_listed() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nia:a"), br#"{"v":10}"#.to_vec());
    tx.put_object(oid("obs://nia:b"), br#"{"v":20}"#.to_vec());
    tx.put_object(oid("obs://nia:c"), br#"{"v":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    // Exclude 'a' → only b and c remain → count = 2
    let rows = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://nia' WHERE id NOT IN ('a')")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
    store.close().unwrap();
}

#[test]
fn id_not_in_empty_list_keeps_all() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://niel:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://niel:b"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    // NOT IN with empty list matches everything
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('v') FROM 'obs://niel' WHERE id NOT IN ()")
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn id_not_in_parses_correctly() {
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('f') FROM 'obs://x' WHERE id NOT IN ('p', 'q')",
    )
    .unwrap();
    assert!(matches!(
        q.id_filter,
        Some(bindb::SqlIdFilter::NotIn(ref vs)) if vs == &vec!["p".to_string(), "q".to_string()]
    ));
}

// ── PAYLOAD_FIELD numeric IN / NOT IN ────────────────────────────────────────

#[test]
fn payload_numeric_in_matches_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://numIN:a"), br#"{"score":1}"#.to_vec());
    tx.put_object(oid("obs://numIN:b"), br#"{"score":2}"#.to_vec());
    tx.put_object(oid("obs://numIN:c"), br#"{"score":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://numIN' WHERE PAYLOAD_FIELD('score') IN (1, 3)",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let scores: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|v| v.as_i64()))
        .collect();
    assert!(scores.contains(&1));
    assert!(scores.contains(&3));
    store.close().unwrap();
}

#[test]
fn payload_numeric_not_in_excludes_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://numNI:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://numNI:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://numNI:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://numNI' WHERE PAYLOAD_FIELD('score') NOT IN (10, 20)",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(30));
    store.close().unwrap();
}

#[test]
fn payload_numeric_in_aggregate_filter() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://numINa:a"), br#"{"tier":1}"#.to_vec());
    tx.put_object(oid("obs://numINa:b"), br#"{"tier":2}"#.to_vec());
    tx.put_object(oid("obs://numINa:c"), br#"{"tier":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://numINa' WHERE PAYLOAD_FIELD('tier') IN (1, 3)")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
    store.close().unwrap();
}

#[test]
fn payload_numeric_in_parses_correctly() {
    use bindb::{SqlPayloadOp, SqlPayloadValue};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('n') FROM 'obs://x' WHERE PAYLOAD_FIELD('n') IN (5, 10, 15)",
    )
    .unwrap();
    assert_eq!(q.payload_filters.len(), 1);
    assert_eq!(q.payload_filters[0].op, SqlPayloadOp::In);
    assert!(matches!(
        &q.payload_filters[0].value,
        SqlPayloadValue::NumList(ns) if ns.len() == 3
    ));
}

// ── ABS/ROUND/CEIL/FLOOR(PAYLOAD_FIELD) and LENGTH(PAYLOAD_FIELD) tests ──────

#[test]
fn select_abs_of_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sabn:a"), br#"{"delta":-7.5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ABS(PAYLOAD_FIELD('delta')) FROM 'obs://sabn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("delta").and_then(|v| v.as_f64()), Some(7.5));
    store.close().unwrap();
}

#[test]
fn select_round_of_decimal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srod:a"), br#"{"price":4.6}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ROUND(PAYLOAD_FIELD('price')) FROM 'obs://srod'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("price").and_then(|v| v.as_f64()), Some(5.0));
    store.close().unwrap();
}

#[test]
fn select_ceil_and_floor() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaf:a"), br#"{"val":3.2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let ceil_rows = store
        .sql_project("SELECT CEIL(PAYLOAD_FIELD('val')) FROM 'obs://scaf'")
        .unwrap();
    assert_eq!(ceil_rows[0].get("val").and_then(|v| v.as_f64()), Some(4.0));

    let floor_rows = store
        .sql_project("SELECT FLOOR(PAYLOAD_FIELD('val')) FROM 'obs://scaf'")
        .unwrap();
    assert_eq!(floor_rows[0].get("val").and_then(|v| v.as_f64()), Some(3.0));
    store.close().unwrap();
}

#[test]
fn select_numeric_transform_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sntsp:a"), br#"{"label":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ABS(PAYLOAD_FIELD('label')) FROM 'obs://sntsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("label").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_length_of_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slos:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('word')) FROM 'obs://slos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

#[test]
fn select_length_of_multiple_rows() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slmr:a"), br#"{"tag":"hi"}"#.to_vec());
    tx.put_object(oid("obs://slmr:b"), br#"{"tag":"world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('tag')) FROM 'obs://slmr' ORDER BY PAYLOAD_FIELD('tag') ASC")
        .unwrap();
    rows.sort_by_key(|r| r.get("tag").and_then(|v| v.as_u64()).unwrap_or(0));
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(rows[1].get("tag").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

#[test]
fn select_length_on_number_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slnp:a"), br#"{"count":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('count')) FROM 'obs://slnp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("count").and_then(|v| v.as_u64()), Some(42));
    store.close().unwrap();
}

#[test]
fn select_length_transform_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LENGTH(PAYLOAD_FIELD('name')) FROM 'obs://x'").unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("name"),
        Some(SqlFieldTransform::Length)
    ));
}

// ── COALESCE / REPLACE(PAYLOAD_FIELD) in SELECT tests ─────────────────────────

#[test]
fn select_coalesce_present_field_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scpf:a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('name'), 'unknown') FROM 'obs://scpf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("Alice"));
    store.close().unwrap();
}

#[test]
fn select_coalesce_missing_field_uses_fallback() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scmf:a"), br#"{"other":"x"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('name'), 'N/A') FROM 'obs://scmf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("N/A"));
    store.close().unwrap();
}

#[test]
fn select_coalesce_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT COALESCE(PAYLOAD_FIELD('label'), 'default') FROM 'obs://x'")
            .unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("label"),
        Some(SqlFieldTransform::Coalesce(s)) if s == "default"
    ));
}

#[test]
fn select_coalesce_with_alias() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwa:a"), br#"{}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('val'), 'none') AS v FROM 'obs://scwa'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("v").and_then(|v| v.as_str()), Some("none"));
    assert!(!rows[0].contains_key("val"));
    store.close().unwrap();
}

#[test]
fn select_replace_substitutes_substring() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srss:a"), br#"{"tag":"hello-world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('tag'), '-', '_') FROM 'obs://srss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("tag").and_then(|v| v.as_str()),
        Some("hello_world")
    );
    store.close().unwrap();
}

#[test]
fn select_replace_all_occurrences() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srao:a"), br#"{"path":"a/b/c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('path'), '/', '.') FROM 'obs://srao'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("path").and_then(|v| v.as_str()), Some("a.b.c"));
    store.close().unwrap();
}

#[test]
fn select_replace_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srnsp:a"), br#"{"n":99}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('n'), '9', '0') FROM 'obs://srnsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(99));
    store.close().unwrap();
}

#[test]
fn select_replace_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse(
        "SELECT REPLACE(PAYLOAD_FIELD('text'), 'foo', 'bar') FROM 'obs://x'",
    )
    .unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("text"),
        Some(SqlFieldTransform::Replace(f, t)) if f == "foo" && t == "bar"
    ));
}

// ── SUBSTRING / CAST(PAYLOAD_FIELD) in SELECT tests ───────────────────────────

#[test]
fn select_substring_extracts_chars() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssec:a"), br#"{"code":"ABCDE"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SUBSTRING(PAYLOAD_FIELD('code'), 1, 3) FROM 'obs://ssec'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("code").and_then(|v| v.as_str()), Some("BCD"));
    store.close().unwrap();
}

#[test]
fn select_substring_start_beyond_length_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sssbe:a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SUBSTRING(PAYLOAD_FIELD('tag'), 10, 3) FROM 'obs://sssbe'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_substring_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SUBSTRING(PAYLOAD_FIELD('name'), 2, 4) FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("name"),
        Some(SqlFieldTransform::Substring(2, 4))
    ));
}

#[test]
fn select_cast_as_int_from_float() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaif:a"), br#"{"price":9.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('price') AS INT) FROM 'obs://scaif'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("price").and_then(|v| v.as_i64()), Some(9));
    store.close().unwrap();
}

#[test]
fn select_cast_as_int_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaifs:a"), br#"{"count":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('count') AS INT) FROM 'obs://scaifs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("count").and_then(|v| v.as_i64()), Some(42));
    store.close().unwrap();
}

#[test]
fn select_cast_as_float_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaff:a"), br#"{"temp":"2.5"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('temp') AS FLOAT) FROM 'obs://scaff'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("temp").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.5).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cast_as_string_from_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scasfn:a"), br#"{"id":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('id') AS STRING) FROM 'obs://scasfn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("id").and_then(|v| v.as_str()), Some("100"));
    store.close().unwrap();
}

#[test]
fn select_cast_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT CAST(PAYLOAD_FIELD('score') AS INT) FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("score"),
        Some(SqlFieldTransform::CastInt)
    ));
}

// ── NULLIF / LPAD / RPAD(PAYLOAD_FIELD) in SELECT tests ───────────────────────

#[test]
fn select_nullif_matching_produces_null() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://snmpn:a"), br#"{"status":"N/A"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT NULLIF(PAYLOAD_FIELD('status'), 'N/A') FROM 'obs://snmpn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("status"), Some(&serde_json::Value::Null));
    store.close().unwrap();
}

#[test]
fn select_nullif_non_matching_kept() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://snnmk:a"), br#"{"status":"active"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT NULLIF(PAYLOAD_FIELD('status'), 'N/A') FROM 'obs://snnmk'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("status").and_then(|v| v.as_str()),
        Some("active")
    );
    store.close().unwrap();
}

#[test]
fn select_nullif_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT NULLIF(PAYLOAD_FIELD('tag'), 'none') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("tag"),
        Some(SqlFieldTransform::NullIf(s)) if s == "none"
    ));
}

#[test]
fn select_lpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slpss:a"), br#"{"code":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LPAD(PAYLOAD_FIELD('code'), 5, '0') FROM 'obs://slpss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("code").and_then(|v| v.as_str()), Some("00042"));
    store.close().unwrap();
}

#[test]
fn select_lpad_already_wide_string_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slpawsu:a"), br#"{"tag":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LPAD(PAYLOAD_FIELD('tag'), 3, ' ') FROM 'obs://slpawsu'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_rpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srpss:a"), br#"{"name":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RPAD(PAYLOAD_FIELD('name'), 5, '.') FROM 'obs://srpss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("hi..."));
    store.close().unwrap();
}

#[test]
fn select_lpad_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LPAD(PAYLOAD_FIELD('code'), 8, '0') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("code"),
        Some(SqlFieldTransform::Lpad(8, s)) if s == "0"
    ));
}

#[test]
fn select_rpad_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT RPAD(PAYLOAD_FIELD('label'), 10, ' ') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("label"),
        Some(SqlFieldTransform::Rpad(10, s)) if s == " "
    ));
}
