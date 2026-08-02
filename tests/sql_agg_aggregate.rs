#[path = "sql_agg_helpers.rs"]
mod helpers;
use helpers::*;

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

// ── HAVING clause tests (issue #5) ───────────────────────────────────────────

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

// ── COUNT(DISTINCT PAYLOAD_FIELD('f')) tests (Feature 3) ──────────────────────

#[test]
fn sql_count_distinct_categories() {
    // 3 distinct category values: run, bike, swim
    // The no-category row (obs://activity:2024-01-05) contributes null — excluded from count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_activity_store(&store);
    let results = store
        .sql_aggregate(
            "SELECT COUNT(DISTINCT PAYLOAD_FIELD('category')) FROM 'obs://activity'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    // count_distinct stores string representation: "\"run\"", "\"bike\"", "\"swim\"" → 3 distinct
    assert_eq!(results[0].value as usize, 3);
    store.close().unwrap();
}

#[test]
fn sql_count_distinct_with_group_by() {
    // When grouping by category, each group has its own distinct duration values.
    // run: durations 30 and 45 → 2 distinct
    // bike: duration 60 → 1 distinct
    // swim: duration 25 → 1 distinct
    // null (no category): duration 10 → 1 distinct
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_activity_store(&store);
    let results = store
        .sql_aggregate(
            "SELECT COUNT(DISTINCT PAYLOAD_FIELD('duration')) FROM 'obs://activity' GROUP BY PAYLOAD_FIELD('category')",
        )
        .unwrap();
    assert_eq!(results.len(), 4);
    // Find the "run" group — json serialization wraps strings in quotes, so group key = "run"
    let run_row = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("run"))
        .expect("run group");
    assert_eq!(run_row.value as usize, 2); // 30 and 45 are distinct
    let bike_row = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("bike"))
        .expect("bike group");
    assert_eq!(bike_row.value as usize, 1);
    store.close().unwrap();
}

#[test]
fn sql_count_distinct_single_observation() {
    // One observation: count of distinct scores = 1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = bindb::Transaction::new();
    tx.put_object(
        oid("obs://single:x"),
        br#"{"score":42}"#.to_vec(),
    );
    store.commit(tx, "test", "seed").unwrap();
    let results = store
        .sql_aggregate(
            "SELECT COUNT(DISTINCT PAYLOAD_FIELD('score')) FROM 'obs://single'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as usize, 1);
    store.close().unwrap();
}

// ── PERCENTILE_DISC tests (Feature 4) ─────────────────────────────────────────

#[test]
fn sql_percentile_disc_median_matches_p50() {
    let (store, _dir) = build_between_sql_store();
    // Scores: [10, 20, 30, 40, 50] — median (p50) = 30
    let results = store
        .sql_aggregate(
            "SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('score')) FROM 'obs://scores'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as i64, 30);
    store.close().unwrap();
}

#[test]
fn sql_percentile_disc_p25() {
    let (store, _dir) = build_between_sql_store();
    // Scores: [10, 20, 30, 40, 50]; p=0.25, idx=round(0.25*4)=round(1.0)=1 → values[1]=20
    let results = store
        .sql_aggregate(
            "SELECT PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('score')) FROM 'obs://scores'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as i64, 20);
    store.close().unwrap();
}

#[test]
fn sql_percentile_disc_p75() {
    let (store, _dir) = build_between_sql_store();
    // Scores: [10, 20, 30, 40, 50]; p=0.75, idx=round(0.75*4)=round(3.0)=3 → values[3]=40
    let results = store
        .sql_aggregate(
            "SELECT PERCENTILE_DISC(0.75) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('score')) FROM 'obs://scores'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as i64, 40);
    store.close().unwrap();
}

#[test]
fn sql_percentile_disc_p0_is_minimum() {
    let (store, _dir) = build_between_sql_store();
    // p=0.0 → index 0 → 10
    let results = store
        .sql_aggregate(
            "SELECT PERCENTILE_DISC(0.0) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('score')) FROM 'obs://scores'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as i64, 10);
    store.close().unwrap();
}

#[test]
fn sql_percentile_disc_p1_is_maximum() {
    let (store, _dir) = build_between_sql_store();
    // p=1.0 → last element → 50
    let results = store
        .sql_aggregate(
            "SELECT PERCENTILE_DISC(1.0) WITHIN GROUP (ORDER BY PAYLOAD_FIELD('score')) FROM 'obs://scores'",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value as i64, 50);
    store.close().unwrap();
}
