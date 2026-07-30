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

// ── REVERSE / INITCAP / REPEAT(PAYLOAD_FIELD) in SELECT tests ───────────────

#[test]
fn select_reverse_reverses_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srevrs:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REVERSE(PAYLOAD_FIELD('word')) FROM 'obs://srevrs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("olleh"));
    store.close().unwrap();
}

#[test]
fn select_reverse_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT REVERSE(PAYLOAD_FIELD('word')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Reverse)
    ));
}

#[test]
fn select_initcap_capitalises_words() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sicpw:a"), br#"{"title":"hello world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT INITCAP(PAYLOAD_FIELD('title')) FROM 'obs://sicpw'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("title").and_then(|v| v.as_str()),
        Some("Hello World")
    );
    store.close().unwrap();
}

#[test]
fn select_initcap_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT INITCAP(PAYLOAD_FIELD('title')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("title"),
        Some(SqlFieldTransform::InitCap)
    ));
}

#[test]
fn select_repeat_duplicates_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sreps:a"), br#"{"sep":"-"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('sep'), 3) FROM 'obs://sreps'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("sep").and_then(|v| v.as_str()), Some("---"));
    store.close().unwrap();
}

#[test]
fn select_repeat_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT REPEAT(PAYLOAD_FIELD('sep'), 4) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("sep"),
        Some(SqlFieldTransform::Repeat(4))
    ));
}

#[test]
fn select_repeat_zero_gives_empty_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srep0:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('tag'), 0) FROM 'obs://srep0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_repeat_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srepnsp:a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('score'), 3) FROM 'obs://srepnsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(42));
    store.close().unwrap();
}

// ── LEFT / RIGHT / SIGN(PAYLOAD_FIELD) in SELECT tests ──────────────────────

#[test]
fn select_left_extracts_prefix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sleft:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LEFT(PAYLOAD_FIELD('word'), 3) FROM 'obs://sleft'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("hel"));
    store.close().unwrap();
}

#[test]
fn select_left_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LEFT(PAYLOAD_FIELD('word'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Left(3))
    ));
}

#[test]
fn select_right_extracts_suffix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srght:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RIGHT(PAYLOAD_FIELD('word'), 3) FROM 'obs://srght'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("llo"));
    store.close().unwrap();
}

#[test]
fn select_right_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT RIGHT(PAYLOAD_FIELD('word'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Right(3))
    ));
}

#[test]
fn select_sign_positive_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignp:a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('score')) FROM 'obs://ssignp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(1));
    store.close().unwrap();
}

#[test]
fn select_sign_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignn:a"), br#"{"val":-7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('val')) FROM 'obs://ssignn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("val").and_then(|v| v.as_i64()), Some(-1));
    store.close().unwrap();
}

#[test]
fn select_sign_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignz:a"), br#"{"val":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('val')) FROM 'obs://ssignz'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("val").and_then(|v| v.as_i64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_sign_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SIGN(PAYLOAD_FIELD('score')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("score"),
        Some(SqlFieldTransform::Sign)
    ));
}

// ── SQRT / POWER(PAYLOAD_FIELD) in SELECT tests ──────────────────────────────

#[test]
fn select_sqrt_of_perfect_square() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssqrt:a"), br#"{"n":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SQRT(PAYLOAD_FIELD('n')) FROM 'obs://ssqrt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_sqrt_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SQRT(PAYLOAD_FIELD('n')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Sqrt)
    ));
}

#[test]
fn select_sqrt_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssqrts:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SQRT(PAYLOAD_FIELD('tag')) FROM 'obs://ssqrts'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_power_squares_a_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spow:a"), br#"{"n":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 2) FROM 'obs://spow'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 9.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_fractional_exponent_gives_sqrt() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spowf:a"), br#"{"n":4}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 0.5) FROM 'obs://spowf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT POWER(PAYLOAD_FIELD('n'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Power(e)) if (e - 3.0).abs() < 1e-9
    ));
}

#[test]
fn select_power_zero_exponent_gives_one() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spow0:a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 0) FROM 'obs://spow0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spows:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('tag'), 2) FROM 'obs://spows'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_mod_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://smod:a"), br#"{"n":10}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT MOD(PAYLOAD_FIELD('n'), 3) FROM 'obs://smod'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_mod_parse() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT MOD(PAYLOAD_FIELD('n'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Mod(d)) if (d - 3.0).abs() < 1e-9
    ));
}

#[test]
fn select_mod_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://smods:a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT MOD(PAYLOAD_FIELD('tag'), 3) FROM 'obs://smods'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("hi"));
    store.close().unwrap();
}

#[test]
fn select_strpos_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sstrpos:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT STRPOS(PAYLOAD_FIELD('word'), 'l') FROM 'obs://sstrpos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn select_strpos_not_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sstrpno:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT STRPOS(PAYLOAD_FIELD('word'), 'z') FROM 'obs://sstrpno'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_strpos_parse() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT STRPOS(PAYLOAD_FIELD('word'), 'l') FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Strpos(n)) if n == "l"
    ));
}

#[test]
fn select_split_part_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssplitpt:a"), br#"{"csv":"a,b,c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 2) FROM 'obs://ssplitpt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("csv").and_then(|v| v.as_str()), Some("b"));
    store.close().unwrap();
}

#[test]
fn select_split_part_out_of_bounds() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssplitob:a"), br#"{"csv":"a,b"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 9) FROM 'obs://ssplitob'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("csv").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_split_part_parse() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 2) FROM 'obs://x'")
            .unwrap();
    assert!(matches!(
        q.field_transforms.get("csv"),
        Some(SqlFieldTransform::SplitPart(d, 2)) if d == ","
    ));
}

#[test]
fn select_ltrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sltrim:a"), br#"{"s":"  hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LTRIM(PAYLOAD_FIELD('s')) FROM 'obs://sltrim'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_rtrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srtrim:a"), br#"{"s":"hello  "}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RTRIM(PAYLOAD_FIELD('s')) FROM 'obs://srtrim'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_log_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG(PAYLOAD_FIELD('n')) FROM 'obs://slog'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_log_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog0:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG(PAYLOAD_FIELD('n')) FROM 'obs://slog0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_exp_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sexp:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT EXP(PAYLOAD_FIELD('n')) FROM 'obs://sexp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_exp_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sexps:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT EXP(PAYLOAD_FIELD('tag')) FROM 'obs://sexps'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_log2_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog2:a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG2(PAYLOAD_FIELD('n')) FROM 'obs://slog2'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_log2_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog2z:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG2(PAYLOAD_FIELD('n')) FROM 'obs://slog2z'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_log10_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog10:a"), br#"{"n":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG10(PAYLOAD_FIELD('n')) FROM 'obs://slog10'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_radians_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srad:a"), br#"{"deg":180}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RADIANS(PAYLOAD_FIELD('deg')) FROM 'obs://srad'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("deg").and_then(|v| v.as_f64()).unwrap();
    assert!((v - std::f64::consts::PI).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_degrees_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://sdeg:a"),
        format!(r#"{{"rad":{}}}"#, std::f64::consts::PI).into_bytes(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DEGREES(PAYLOAD_FIELD('rad')) FROM 'obs://sdeg'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("rad").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 180.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_radians_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srads:a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RADIANS(PAYLOAD_FIELD('s')) FROM 'obs://srads'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("north"));
    store.close().unwrap();
}

#[test]
fn select_degrees_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sdegs:a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DEGREES(PAYLOAD_FIELD('s')) FROM 'obs://sdegs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("north"));
    store.close().unwrap();
}

#[test]
fn select_sin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssin:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIN(PAYLOAD_FIELD('n')) FROM 'obs://ssin'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_sin_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssins:a"), br#"{"s":"wave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIN(PAYLOAD_FIELD('s')) FROM 'obs://ssins'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("wave"));
    store.close().unwrap();
}

#[test]
fn select_cos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scos:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COS(PAYLOAD_FIELD('n')) FROM 'obs://scos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_tan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://stan:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TAN(PAYLOAD_FIELD('n')) FROM 'obs://stan'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_atan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://satan:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ATAN(PAYLOAD_FIELD('n')) FROM 'obs://satan'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_atan_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://satans:a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ATAN(PAYLOAD_FIELD('s')) FROM 'obs://satans'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("arc"));
    store.close().unwrap();
}

#[test]
fn select_asin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sasin:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ASIN(PAYLOAD_FIELD('n')) FROM 'obs://sasin'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_asin_out_of_domain_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sasinood:a"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ASIN(PAYLOAD_FIELD('n')) FROM 'obs://sasinood'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_i64()), Some(2));
    store.close().unwrap();
}

#[test]
fn select_acos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sacos:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ACOS(PAYLOAD_FIELD('n')) FROM 'obs://sacos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_acos_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sacosstr:a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ACOS(PAYLOAD_FIELD('s')) FROM 'obs://sacosstr'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("arc"));
    store.close().unwrap();
}

#[test]
fn select_trunc_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://struncn:a"), br#"{"n":-1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TRUNC(PAYLOAD_FIELD('n')) FROM 'obs://struncn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - (-1.0)).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_trunc_positive() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://struncp:a"), br#"{"n":1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TRUNC(PAYLOAD_FIELD('n')) FROM 'obs://struncp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cbrt_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scbrt:a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CBRT(PAYLOAD_FIELD('n')) FROM 'obs://scbrt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cbrt_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scbrtstr:a"), br#"{"s":"root"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CBRT(PAYLOAD_FIELD('s')) FROM 'obs://scbrtstr'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("root"));
    store.close().unwrap();
}

// --- Window functions ---

#[test]
fn window_row_number_global() {
    // Three rows with different scores; ROW_NUMBER() OVER (ORDER BY score ASC) = 1,2,3
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrng:a"), br#"{"score":30}"#.to_vec());
    tx.put_object(oid("obs://wrng:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrng:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), ROW_NUMBER() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS rn FROM 'obs://wrng'",
        )
        .unwrap();
    // Sort output by score so assertions are deterministic.
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(rows[1].get("rn").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn window_row_number_partition_by() {
    // Two departments; ROW_NUMBER() per dept ordered by score.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://wrnp:a"),
        br#"{"dept":"eng","score":10}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://wrnp:b"),
        br#"{"dept":"eng","score":20}"#.to_vec(),
    );
    tx.put_object(oid("obs://wrnp:c"), br#"{"dept":"mkt","score":5}"#.to_vec());
    tx.put_object(
        oid("obs://wrnp:d"),
        br#"{"dept":"mkt","score":15}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('dept'), PAYLOAD_FIELD('score'), \
             ROW_NUMBER() OVER (PARTITION BY PAYLOAD_FIELD('dept') ORDER BY PAYLOAD_FIELD('score') ASC) AS rn \
             FROM 'obs://wrnp'",
        )
        .unwrap();
    // Each dept should have rn=1 and rn=2; no rn > 2.
    let max_rn = rows
        .iter()
        .filter_map(|r| r.get("rn").and_then(|v| v.as_u64()))
        .max()
        .unwrap();
    assert_eq!(max_rn, 2);
    // rn=1 for score=10 in eng and score=5 in mkt.
    let rn1_scores: std::collections::HashSet<i64> = rows
        .iter()
        .filter(|r| r.get("rn").and_then(|v| v.as_u64()) == Some(1))
        .filter_map(|r| r.get("score").and_then(|v| v.as_i64()))
        .collect();
    assert!(rn1_scores.contains(&10));
    assert!(rn1_scores.contains(&5));
    store.close().unwrap();
}

#[test]
fn window_rank_ties() {
    // Two rows with same score get same rank; next rank skips.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrt:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrt:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wrt:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), RANK() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS rnk \
             FROM 'obs://wrt'",
        )
        .unwrap();
    let ranks: std::collections::HashSet<u64> = rows
        .iter()
        .filter_map(|r| r.get("rnk").and_then(|v| v.as_u64()))
        .collect();
    // Ranks should be {1, 3} — two rows at rank 1 (tie), then rank 3 skips 2.
    assert!(ranks.contains(&1));
    assert!(ranks.contains(&3));
    assert!(!ranks.contains(&2));
    store.close().unwrap();
}

#[test]
fn window_dense_rank_ties() {
    // Two rows with same score get same dense_rank; next dense_rank does NOT skip.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wdrt:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wdrt:b"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wdrt:c"), br#"{"score":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), DENSE_RANK() OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS dr \
             FROM 'obs://wdrt'",
        )
        .unwrap();
    let dense_ranks: std::collections::HashSet<u64> = rows
        .iter()
        .filter_map(|r| r.get("dr").and_then(|v| v.as_u64()))
        .collect();
    // Dense ranks should be {1, 2} — no gap.
    assert!(dense_ranks.contains(&1));
    assert!(dense_ranks.contains(&2));
    assert!(!dense_ranks.contains(&3));
    store.close().unwrap();
}

#[test]
fn window_rank_desc_order() {
    // RANK() ORDER BY score DESC — highest score gets rank 1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wrdo:a"), br#"{"score":100}"#.to_vec());
    tx.put_object(oid("obs://wrdo:b"), br#"{"score":50}"#.to_vec());
    tx.put_object(oid("obs://wrdo:c"), br#"{"score":75}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), RANK() OVER (ORDER BY PAYLOAD_FIELD('score') DESC) AS rnk \
             FROM 'obs://wrdo'",
        )
        .unwrap();
    let rank_for = |score: i64| {
        rows.iter()
            .find(|r| r.get("score").and_then(|v| v.as_i64()) == Some(score))
            .and_then(|r| r.get("rnk").and_then(|v| v.as_u64()))
            .unwrap()
    };
    assert_eq!(rank_for(100), 1);
    assert_eq!(rank_for(75), 2);
    assert_eq!(rank_for(50), 3);
    store.close().unwrap();
}

#[test]
fn window_with_limit() {
    // Window functions are computed before LIMIT; LIMIT then trims the output.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 1u64..=5 {
        tx.put_object(
            oid(&format!("obs://wlim:{i}")),
            format!(r#"{{"n":{i}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), ROW_NUMBER() OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS rn \
             FROM 'obs://wlim' ORDER BY PAYLOAD_FIELD('n') ASC LIMIT 3",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // Row numbers in first 3 rows should be 1, 2, 3.
    let rns: Vec<u64> = rows
        .iter()
        .filter_map(|r| r.get("rn").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(rns, vec![1, 2, 3]);
    store.close().unwrap();
}

// --- LAG / LEAD window functions ---

#[test]
fn window_lag_basic() {
    // LAG(score, 1) returns the previous row's score within the global partition ordered by score ASC.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlag:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wlag:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wlag:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             LAG(PAYLOAD_FIELD('score'), 1) OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS prev \
             FROM 'obs://wlag'",
        )
        .unwrap();
    // Sort rows by score for deterministic assertions.
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[0].get("prev"), Some(&serde_json::Value::Null)); // no previous row
    assert_eq!(rows[1].get("score").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("prev").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[2].get("score").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("prev").and_then(|v| v.as_i64()), Some(20));
    store.close().unwrap();
}

#[test]
fn window_lead_basic() {
    // LEAD(score, 1) returns the next row's score.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlead:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wlead:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wlead:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             LEAD(PAYLOAD_FIELD('score'), 1) OVER (ORDER BY PAYLOAD_FIELD('score') ASC) AS nxt \
             FROM 'obs://wlead'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("score").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(10));
    assert_eq!(rows[0].get("nxt").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("score").and_then(|v| v.as_i64()), Some(20));
    assert_eq!(rows[1].get("nxt").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("score").and_then(|v| v.as_i64()), Some(30));
    assert_eq!(rows[2].get("nxt"), Some(&serde_json::Value::Null)); // no next row
    store.close().unwrap();
}

#[test]
fn window_lag_offset_2() {
    // LAG with offset=2 skips one row.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for i in 1u64..=4 {
        tx.put_object(
            oid(&format!("obs://wlag2:{i}")),
            format!(r#"{{"n":{i}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), \
             LAG(PAYLOAD_FIELD('n'), 2) OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS prev2 \
             FROM 'obs://wlag2'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("n").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(rows[0].get("prev2"), Some(&serde_json::Value::Null));
    assert_eq!(rows[1].get("prev2"), Some(&serde_json::Value::Null));
    assert_eq!(rows[2].get("prev2").and_then(|v| v.as_i64()), Some(1)); // 3 - 2 = 1
    assert_eq!(rows[3].get("prev2").and_then(|v| v.as_i64()), Some(2)); // 4 - 2 = 2
    store.close().unwrap();
}

#[test]
fn window_lag_partition_by() {
    // LAG per partition: two groups (cat A and B); LAG resets at partition boundary.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wlagp:a1"), br#"{"cat":"A","v":1}"#.to_vec());
    tx.put_object(oid("obs://wlagp:a2"), br#"{"cat":"A","v":2}"#.to_vec());
    tx.put_object(oid("obs://wlagp:b1"), br#"{"cat":"B","v":10}"#.to_vec());
    tx.put_object(oid("obs://wlagp:b2"), br#"{"cat":"B","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('cat'), PAYLOAD_FIELD('v'), \
             LAG(PAYLOAD_FIELD('v'), 1) OVER (PARTITION BY PAYLOAD_FIELD('cat') ORDER BY PAYLOAD_FIELD('v') ASC) AS prev \
             FROM 'obs://wlagp'",
        )
        .unwrap();

    let get = |cat: &str, v: i64| {
        rows.iter()
            .find(|r| {
                r.get("cat").and_then(|x| x.as_str()) == Some(cat)
                    && r.get("v").and_then(|x| x.as_i64()) == Some(v)
            })
            .map(|r| r.get("prev").cloned().unwrap_or(serde_json::Value::Null))
            .unwrap()
    };
    // First in each partition: no lag.
    assert_eq!(get("A", 1), serde_json::Value::Null);
    assert_eq!(get("B", 10), serde_json::Value::Null);
    // Second in each partition: lag = first value.
    assert_eq!(get("A", 2).as_i64(), Some(1));
    assert_eq!(get("B", 20).as_i64(), Some(10));
    store.close().unwrap();
}

#[test]
fn window_lag_lead_combined() {
    // Both LAG and LEAD in the same SELECT.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wll:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://wll:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://wll:c"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             LAG(PAYLOAD_FIELD('v'), 1) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS prev, \
             LEAD(PAYLOAD_FIELD('v'), 1) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nxt \
             FROM 'obs://wll'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));

    // Middle row (v=2): prev=1, next=3.
    assert_eq!(rows[1].get("prev").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[1].get("nxt").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

#[test]
fn window_lead_different_field() {
    // LEAD can reference a different field than the one in SELECT.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://wldf:a"),
        br#"{"rank":1,"name":"alice"}"#.to_vec(),
    );
    tx.put_object(oid("obs://wldf:b"), br#"{"rank":2,"name":"bob"}"#.to_vec());
    tx.put_object(
        oid("obs://wldf:c"),
        br#"{"rank":3,"name":"carol"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('rank'), \
             LEAD(PAYLOAD_FIELD('name'), 1) OVER (ORDER BY PAYLOAD_FIELD('rank') ASC) AS next_name \
             FROM 'obs://wldf'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("rank").and_then(|v| v.as_i64()).unwrap_or(0));

    assert_eq!(
        rows[0].get("next_name").and_then(|v| v.as_str()),
        Some("bob")
    );
    assert_eq!(
        rows[1].get("next_name").and_then(|v| v.as_str()),
        Some("carol")
    );
    assert_eq!(rows[2].get("next_name"), Some(&serde_json::Value::Null));
    store.close().unwrap();
}

// ── Aggregate window functions ──────────────────────────────────────────────

#[test]
fn window_sum_full_partition() {
    // SUM with no ORDER BY → every row gets the partition total.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsf:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(oid("obs://wsf:b"), br#"{"score":20}"#.to_vec());
    tx.put_object(oid("obs://wsf:c"), br#"{"score":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score'), \
             SUM(PAYLOAD_FIELD('score')) OVER () AS total \
             FROM 'obs://wsf'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("total").and_then(|v| v.as_f64()), Some(60.0));
    }
    store.close().unwrap();
}

#[test]
fn window_sum_running() {
    // SUM with ORDER BY → running cumulative sum.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsr:a"), br#"{"n":1}"#.to_vec());
    tx.put_object(oid("obs://wsr:b"), br#"{"n":2}"#.to_vec());
    tx.put_object(oid("obs://wsr:c"), br#"{"n":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('n'), \
             SUM(PAYLOAD_FIELD('n')) OVER (ORDER BY PAYLOAD_FIELD('n') ASC) AS running \
             FROM 'obs://wsr'",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("n").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("running").and_then(|v| v.as_f64()), Some(1.0));
    assert_eq!(rows[1].get("running").and_then(|v| v.as_f64()), Some(3.0));
    assert_eq!(rows[2].get("running").and_then(|v| v.as_f64()), Some(6.0));
    store.close().unwrap();
}

#[test]
fn window_avg_full_partition() {
    // AVG with no ORDER BY → every row gets the partition average.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wav:a"), br#"{"x":10}"#.to_vec());
    tx.put_object(oid("obs://wav:b"), br#"{"x":20}"#.to_vec());
    tx.put_object(oid("obs://wav:c"), br#"{"x":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('x'), \
             AVG(PAYLOAD_FIELD('x')) OVER () AS avg_x \
             FROM 'obs://wav'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("avg_x").and_then(|v| v.as_f64()), Some(20.0));
    }
    store.close().unwrap();
}

#[test]
fn window_count_star() {
    // COUNT(*) OVER () → every row gets total row count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wcs:a"), br#"{"k":1}"#.to_vec());
    tx.put_object(oid("obs://wcs:b"), br#"{"k":2}"#.to_vec());
    tx.put_object(oid("obs://wcs:c"), br#"{"k":3}"#.to_vec());
    tx.put_object(oid("obs://wcs:d"), br#"{"k":4}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('k'), \
             COUNT(*) OVER () AS cnt \
             FROM 'obs://wcs'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("cnt").and_then(|v| v.as_f64()), Some(4.0));
    }
    store.close().unwrap();
}

#[test]
fn window_min_max() {
    // MIN and MAX over full partition.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wmm:a"), br#"{"v":5}"#.to_vec());
    tx.put_object(oid("obs://wmm:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://wmm:c"), br#"{"v":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             MIN(PAYLOAD_FIELD('v')) OVER () AS lo, \
             MAX(PAYLOAD_FIELD('v')) OVER () AS hi \
             FROM 'obs://wmm'",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("lo").and_then(|v| v.as_f64()), Some(2.0));
        assert_eq!(r.get("hi").and_then(|v| v.as_f64()), Some(8.0));
    }
    store.close().unwrap();
}

#[test]
fn window_sum_partition_by() {
    // SUM OVER (PARTITION BY group) → each group gets its own total.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wsp:a"), br#"{"grp":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wsp:b"), br#"{"grp":"x","v":2}"#.to_vec());
    tx.put_object(oid("obs://wsp:c"), br#"{"grp":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wsp:d"), br#"{"grp":"y","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('grp'), PAYLOAD_FIELD('v'), \
             SUM(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('grp')) AS grp_total \
             FROM 'obs://wsp'",
        )
        .unwrap();

    for r in &rows {
        let grp = r.get("grp").and_then(|v| v.as_str()).unwrap_or("");
        let total = r.get("grp_total").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if grp == "x" {
            assert_eq!(total, 3.0);
        } else {
            assert_eq!(total, 30.0);
        }
    }
    store.close().unwrap();
}

// ── Named window functions ──────────────────────────────────────────────────

#[test]
fn named_window_row_number() {
    // WINDOW w AS (...) reused by ROW_NUMBER() OVER w.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwrn:a"), br#"{"grp":"x","v":3}"#.to_vec());
    tx.put_object(oid("obs://nwrn:b"), br#"{"grp":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://nwrn:c"), br#"{"grp":"x","v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), ROW_NUMBER() OVER w AS rn \
             FROM 'obs://nwrn' \
             WINDOW w AS (ORDER BY PAYLOAD_FIELD('v') ASC)",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[1].get("rn").and_then(|v| v.as_i64()), Some(2));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_i64()), Some(3));
    store.close().unwrap();
}

#[test]
fn named_window_shared_across_two_functions() {
    // Same named window referenced by both ROW_NUMBER and SUM.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nws2:a"), br#"{"v":10}"#.to_vec());
    tx.put_object(oid("obs://nws2:b"), br#"{"v":20}"#.to_vec());
    tx.put_object(oid("obs://nws2:c"), br#"{"v":30}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             ROW_NUMBER() OVER w AS rn, \
             SUM(PAYLOAD_FIELD('v')) OVER w AS running \
             FROM 'obs://nws2' \
             WINDOW w AS (ORDER BY PAYLOAD_FIELD('v') ASC)",
        )
        .unwrap();
    let mut rows = rows;
    rows.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    // rn: 1, 2, 3
    assert_eq!(rows[0].get("rn").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(rows[2].get("rn").and_then(|v| v.as_i64()), Some(3));
    // running sum: 10, 30, 60
    assert_eq!(rows[0].get("running").and_then(|v| v.as_f64()), Some(10.0));
    assert_eq!(rows[1].get("running").and_then(|v| v.as_f64()), Some(30.0));
    assert_eq!(rows[2].get("running").and_then(|v| v.as_f64()), Some(60.0));
    store.close().unwrap();
}

#[test]
fn named_window_partition_by() {
    // Named window with PARTITION BY — SUM resets per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwpb:a"), br#"{"g":"a","v":1}"#.to_vec());
    tx.put_object(oid("obs://nwpb:b"), br#"{"g":"a","v":2}"#.to_vec());
    tx.put_object(oid("obs://nwpb:c"), br#"{"g":"b","v":10}"#.to_vec());
    tx.put_object(oid("obs://nwpb:d"), br#"{"g":"b","v":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             SUM(PAYLOAD_FIELD('v')) OVER w AS grp_total \
             FROM 'obs://nwpb' \
             WINDOW w AS (PARTITION BY PAYLOAD_FIELD('g'))",
        )
        .unwrap();
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let t = r.get("grp_total").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if g == "a" {
            assert_eq!(t, 3.0);
        } else {
            assert_eq!(t, 30.0);
        }
    }
    store.close().unwrap();
}

#[test]
fn named_window_multiple_definitions() {
    // Two different named windows in one WINDOW clause.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwmd:a"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://nwmd:b"), br#"{"g":"x","v":3}"#.to_vec());
    tx.put_object(oid("obs://nwmd:c"), br#"{"g":"y","v":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             ROW_NUMBER() OVER w1 AS rn, \
             SUM(PAYLOAD_FIELD('v')) OVER w2 AS grp_sum \
             FROM 'obs://nwmd' \
             WINDOW w1 AS (ORDER BY PAYLOAD_FIELD('v') ASC), \
                    w2 AS (PARTITION BY PAYLOAD_FIELD('g'))",
        )
        .unwrap();
    // rn is global across all rows by v order
    let mut by_v = rows.clone();
    by_v.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    assert_eq!(by_v[0].get("rn").and_then(|v| v.as_i64()), Some(1)); // v=3
    assert_eq!(by_v[1].get("rn").and_then(|v| v.as_i64()), Some(2)); // v=5
    assert_eq!(by_v[2].get("rn").and_then(|v| v.as_i64()), Some(3)); // v=8
                                                                     // grp_sum: group x = 8, group y = 8
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let s = r.get("grp_sum").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if g == "x" {
            assert_eq!(s, 8.0);
        } else {
            assert_eq!(s, 8.0); // y group has only v=8
        }
    }
    store.close().unwrap();
}

#[test]
fn named_window_unknown_name_errors() {
    // Referencing an undefined window name returns an error.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwun:a"), br#"{"v":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let result =
        store.sql_project("SELECT ROW_NUMBER() OVER missing_window AS rn FROM 'obs://nwun'");
    assert!(result.is_err());
    store.close().unwrap();
}

#[test]
fn named_window_count_star() {
    // COUNT(*) OVER named window gives total row count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://nwcs:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://nwcs:b"), br#"{"v":2}"#.to_vec());
    tx.put_object(oid("obs://nwcs:c"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), COUNT(*) OVER w AS cnt \
             FROM 'obs://nwcs' \
             WINDOW w AS ()",
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.get("cnt").and_then(|v| v.as_f64()), Some(3.0));
    }
    store.close().unwrap();
}

#[test]
fn sql_case_when_basic() {
    // SELECT CASE PAYLOAD_FIELD('role') WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwb:a"), br#"{"role":"admin"}"#.to_vec());
    tx.put_object(oid("obs://scwb:b"), br#"{"role":"user"}"#.to_vec());
    tx.put_object(oid("obs://scwb:c"), br#"{"role":"guest"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT CASE PAYLOAD_FIELD('role') \
             WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier \
             FROM 'obs://scwb'",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    let mut tiers: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.get("tier")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    tiers.sort();
    assert_eq!(tiers, vec!["A", "U", "X"]);
    store.close().unwrap();
}

#[test]
fn sql_case_when_no_else() {
    // Without ELSE, unmatched rows receive empty string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwne:a"), br#"{"status":"active"}"#.to_vec());
    tx.put_object(oid("obs://scwne:b"), br#"{"status":"inactive"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT CASE PAYLOAD_FIELD('status') WHEN 'active' THEN 'yes' END AS flag \
             FROM 'obs://scwne'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let yes = rows
        .iter()
        .filter(|r| r.get("flag").and_then(|v| v.as_str()) == Some("yes"))
        .count();
    assert_eq!(yes, 1);
    store.close().unwrap();
}

#[test]
fn sql_case_when_combined_with_field() {
    // CASE alongside a regular PAYLOAD_FIELD projection.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://scwcf:a"),
        br#"{"name":"alice","role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://scwcf:b"),
        br#"{"name":"bob","role":"user"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), \
             CASE PAYLOAD_FIELD('role') WHEN 'admin' THEN 'elevated' ELSE 'standard' END AS access \
             FROM 'obs://scwcf'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let access = r.get("access").and_then(|v| v.as_str()).unwrap_or("");
        if name == "alice" {
            assert_eq!(access, "elevated");
        } else if name == "bob" {
            assert_eq!(access, "standard");
        }
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_basic() {
    // NTH_VALUE returns the nth row's field value from the partition-ordered sequence.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 30), ("c", 20), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wnvb:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 2) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nth2 \
             FROM 'obs://wnvb'",
        )
        .unwrap();
    // 2nd value in ascending order is v=20; all rows share it.
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("nth2").and_then(|v| v.as_i64()), Some(20));
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_out_of_bounds() {
    // NTH_VALUE returns null when n exceeds the partition size.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wnvob:a"), br#"{"v":1}"#.to_vec());
    tx.put_object(oid("obs://wnvob:b"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 5) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS nth5 \
             FROM 'obs://wnvob'",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(r.get("nth5").is_none_or(|v| v.is_null()));
    }
    store.close().unwrap();
}

#[test]
fn window_nth_value_partition_by() {
    // NTH_VALUE with PARTITION BY resets per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wnvp:a"), br#"{"g":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wnvp:b"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://wnvp:c"), br#"{"g":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wnvp:d"), br#"{"g":"y","v":50}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             NTH_VALUE(PAYLOAD_FIELD('v'), 2) \
             OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS nth2 \
             FROM 'obs://wnvp'",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let nth2 = r.get("nth2").and_then(|v| v.as_i64()).unwrap_or(-1);
        // 2nd in group x (v=1,5) is 5; 2nd in group y (v=10,50) is 50
        if g == "x" {
            assert_eq!(nth2, 5, "group x 2nd");
        } else {
            assert_eq!(nth2, 50, "group y 2nd");
        }
    }
    store.close().unwrap();
}

#[test]
fn window_ntile_basic() {
    // NTILE(3) over 6 ordered rows assigns equal-size bucket numbers 1,1,2,2,3,3.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [
        ("a", 10),
        ("b", 20),
        ("c", 30),
        ("d", 40),
        ("e", 50),
        ("f", 60),
    ] {
        tx.put_object(
            oid(&format!("obs://ntb:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             NTILE(3) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS bucket \
             FROM 'obs://ntb'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    // buckets: [1,1,2,2,3,3] for v=[10,20,30,40,50,60]
    let expected = [1i64, 1, 2, 2, 3, 3];
    for (r, &exp) in sorted.iter().zip(expected.iter()) {
        assert_eq!(r.get("bucket").and_then(|v| v.as_i64()), Some(exp));
    }
    store.close().unwrap();
}

#[test]
fn window_first_value() {
    // FIRST_VALUE returns the value from the first row in the partition ordering.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 3), ("b", 1), ("c", 4), ("d", 2)] {
        tx.put_object(
            oid(&format!("obs://wfv:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             FIRST_VALUE(PAYLOAD_FIELD('v')) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS fv \
             FROM 'obs://wfv'",
        )
        .unwrap();
    // All rows share FIRST_VALUE = 1 (smallest v after ORDER BY ASC).
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("fv").and_then(|v| v.as_i64()), Some(1));
    }
    store.close().unwrap();
}

#[test]
fn window_last_value() {
    // LAST_VALUE returns the value from the last row in the partition ordering.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 3), ("b", 1), ("c", 4), ("d", 2)] {
        tx.put_object(
            oid(&format!("obs://wlv:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             LAST_VALUE(PAYLOAD_FIELD('v')) OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS lv \
             FROM 'obs://wlv'",
        )
        .unwrap();
    // All rows share LAST_VALUE = 4 (largest v after ORDER BY ASC).
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert_eq!(r.get("lv").and_then(|v| v.as_i64()), Some(4));
    }
    store.close().unwrap();
}

#[test]
fn window_percent_rank() {
    // PERCENT_RANK over 4 distinct-value rows: 0.0, 1/3, 2/3, 1.0.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 20), ("c", 30), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wpr:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             PERCENT_RANK() OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS pr \
             FROM 'obs://wpr'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    let eps = 1e-9;
    let expected = [0.0f64, 1.0 / 3.0, 2.0 / 3.0, 1.0];
    for (r, &exp) in sorted.iter().zip(expected.iter()) {
        let pr = r.get("pr").and_then(|v| v.as_f64()).unwrap_or(-1.0);
        assert!((pr - exp).abs() < eps, "expected {exp}, got {pr}");
    }
    store.close().unwrap();
}

#[test]
fn window_cume_dist_with_ties() {
    // CUME_DIST: ties share the same value; rows with v=20 both get 3/4.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("a", 10), ("b", 20), ("c", 20), ("d", 40)] {
        tx.put_object(
            oid(&format!("obs://wcd:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             CUME_DIST() OVER (ORDER BY PAYLOAD_FIELD('v') ASC) AS cd \
             FROM 'obs://wcd'",
        )
        .unwrap();
    let mut sorted = rows.clone();
    sorted.sort_by_key(|r| r.get("v").and_then(|v| v.as_i64()).unwrap_or(0));
    let eps = 1e-9;
    // v=10 → 1/4, v=20 (×2) → 3/4, v=40 → 4/4
    let cd0 = sorted[0].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd1 = sorted[1].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd2 = sorted[2].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    let cd3 = sorted[3].get("cd").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    assert!((cd0 - 0.25).abs() < eps, "got {cd0}");
    assert!((cd1 - 0.75).abs() < eps, "got {cd1}");
    assert!((cd2 - 0.75).abs() < eps, "got {cd2}");
    assert!((cd3 - 1.0).abs() < eps, "got {cd3}");
    store.close().unwrap();
}

#[test]
fn window_first_last_with_partition_by() {
    // FIRST_VALUE and LAST_VALUE with PARTITION BY reset per group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://wflp:a"), br#"{"g":"x","v":1}"#.to_vec());
    tx.put_object(oid("obs://wflp:b"), br#"{"g":"x","v":5}"#.to_vec());
    tx.put_object(oid("obs://wflp:c"), br#"{"g":"y","v":10}"#.to_vec());
    tx.put_object(oid("obs://wflp:d"), br#"{"g":"y","v":50}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('g'), PAYLOAD_FIELD('v'), \
             FIRST_VALUE(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS fv, \
             LAST_VALUE(PAYLOAD_FIELD('v')) OVER (PARTITION BY PAYLOAD_FIELD('g') ORDER BY PAYLOAD_FIELD('v') ASC) AS lv \
             FROM 'obs://wflp'",
        )
        .unwrap();
    for r in &rows {
        let g = r.get("g").and_then(|v| v.as_str()).unwrap_or("");
        let fv = r.get("fv").and_then(|v| v.as_i64()).unwrap_or(-1);
        let lv = r.get("lv").and_then(|v| v.as_i64()).unwrap_or(-1);
        if g == "x" {
            assert_eq!(fv, 1, "group x first");
            assert_eq!(lv, 5, "group x last");
        } else {
            assert_eq!(fv, 10, "group y first");
            assert_eq!(lv, 50, "group y last");
        }
    }
    store.close().unwrap();
}
