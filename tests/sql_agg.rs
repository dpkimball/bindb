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
    assert_eq!(
        q.payload_filter,
        Some(SqlPayloadFilter {
            field: "score".into(),
            op: SqlPayloadOp::Gte,
            value: SqlPayloadValue::Num(10.0),
        })
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
    assert_eq!(
        q.payload_filter,
        Some(SqlPayloadFilter {
            field: "status".into(),
            op: SqlPayloadOp::Eq,
            value: SqlPayloadValue::Str("ok".into()),
        })
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
    use bindb::{AggOrderDirection, SqlProjectOrderBy, SqlProjectQuery};
    let q = SqlProjectQuery::parse(
        "SELECT PAYLOAD_FIELD('name') FROM 'obs://x' ORDER BY PAYLOAD_FIELD('score') DESC LIMIT 10",
    )
    .unwrap();
    assert_eq!(
        q.order_by,
        Some(SqlProjectOrderBy {
            field: "score".into(),
            direction: AggOrderDirection::Desc,
        })
    );
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
    assert_eq!(
        q.payload_filter,
        Some(SqlPayloadFilter {
            field: "name".into(),
            op: SqlPayloadOp::Like,
            value: SqlPayloadValue::Str("a%".into()),
        })
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
