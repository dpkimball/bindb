#[path = "sql_agg_helpers.rs"]
mod helpers;
use helpers::*;

// ── SqlProjectUnionQuery ────────────────────────────────────────────────────

#[test]
fn sql_union_all_combines_branches() {
    let dir = tmp();
    let store = build_union_store(dir.path());
    let rows = store
        .sql_project_union(
            "SELECT PAYLOAD_FIELD('name') FROM 'obs://users' \
             UNION ALL \
             SELECT PAYLOAD_FIELD('name') FROM 'obs://guests'",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Bob"));
    assert!(names.contains(&"Carol"));
    assert!(names.contains(&"Dave"));
    store.close().unwrap();
}

#[test]
fn sql_union_deduplicates() {
    let dir = tmp();
    // Two branches both see the same rows — plain UNION should deduplicate.
    let store = build_union_store(dir.path());
    let rows = store
        .sql_project_union(
            "SELECT PAYLOAD_FIELD('role') FROM 'obs://guests' \
             UNION \
             SELECT PAYLOAD_FIELD('role') FROM 'obs://guests'",
        )
        .unwrap();
    // Both guests have role=guest → only 1 unique row after UNION.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("role").and_then(|v| v.as_str()), Some("guest"));
    store.close().unwrap();
}

#[test]
fn sql_union_all_preserves_duplicates() {
    let dir = tmp();
    let store = build_union_store(dir.path());
    let rows = store
        .sql_project_union(
            "SELECT PAYLOAD_FIELD('role') FROM 'obs://guests' \
             UNION ALL \
             SELECT PAYLOAD_FIELD('role') FROM 'obs://guests'",
        )
        .unwrap();
    // 2 guests × 2 branches = 4 rows with UNION ALL.
    assert_eq!(rows.len(), 4);
    store.close().unwrap();
}

#[test]
fn sql_union_parse_requires_two_branches() {
    let err = SqlProjectUnionQuery::parse("SELECT PAYLOAD_FIELD('name') FROM 'obs://users'");
    assert!(err.is_err());
}

#[test]
fn sql_union_cross_namespace() {
    let dir = tmp();
    let store = build_union_store(dir.path());
    let rows = store
        .sql_project_union(
            "SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('role') FROM 'obs://users' \
             UNION ALL \
             SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('role') FROM 'obs://guests'",
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    let roles: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.get("role").and_then(|v| v.as_str()))
        .collect();
    assert!(roles.contains(&"admin"));
    assert!(roles.contains(&"user"));
    assert!(roles.contains(&"guest"));
    store.close().unwrap();
}

// ── COLLECT aggregate ────────────────────────────────────────────────────────

#[test]
fn sql_collect_basic_array() {
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate("SELECT COLLECT(PAYLOAD_FIELD('name')) FROM 'obs://team'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    let arr = results[0]
        .collected
        .as_ref()
        .and_then(|v| v.as_array())
        .expect("collected should be an array");
    assert_eq!(arr.len(), 3);
    let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Bob"));
    assert!(names.contains(&"Carol"));
    store.close().unwrap();
}

#[test]
fn sql_collect_grouped_by_role() {
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate(
            "SELECT COLLECT(PAYLOAD_FIELD('name')) FROM 'obs://team' GROUP BY PAYLOAD_FIELD('role')",
        )
        .unwrap();
    // admin: Alice, Carol; user: Bob
    assert_eq!(results.len(), 2);
    let admin = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("admin"))
        .expect("admin group");
    let admin_arr = admin
        .collected
        .as_ref()
        .and_then(|v| v.as_array())
        .expect("admin array");
    assert_eq!(admin_arr.len(), 2);
    let admin_names: Vec<&str> = admin_arr.iter().filter_map(|v| v.as_str()).collect();
    assert!(admin_names.contains(&"Alice"));
    assert!(admin_names.contains(&"Carol"));

    let user = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("user"))
        .expect("user group");
    let user_arr = user
        .collected
        .as_ref()
        .and_then(|v| v.as_array())
        .expect("user array");
    assert_eq!(user_arr.len(), 1);
    assert_eq!(user_arr[0].as_str(), Some("Bob"));
    store.close().unwrap();
}

#[test]
fn sql_collect_non_collect_result_has_no_collected() {
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate("SELECT COUNT(*) FROM 'obs://team'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].collected.is_none());
    store.close().unwrap();
}

#[test]
fn sql_collect_parse_rejects_bad_form() {
    let err = SqlAggregateQuery::parse("SELECT COLLECT(name) FROM 'obs://ns'");
    assert!(err.is_err());
    let err2 = SqlAggregateQuery::parse("SELECT COLLECT(PAYLOAD_FIELD('f') FROM 'obs://ns'");
    assert!(err2.is_err());
}

// ── STRING_AGG ──────────────────────────────────────────────────────────────

#[test]
fn sql_string_agg_basic_concat() {
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate("SELECT STRING_AGG(PAYLOAD_FIELD('name'), ', ') FROM 'obs://team'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].count, 3);
    let joined = results[0]
        .collected
        .as_ref()
        .and_then(|v| v.as_str())
        .expect("collected should be a string");
    // Order is iteration order; just verify all names appear separated by ", "
    let parts: Vec<&str> = joined.split(", ").collect();
    assert_eq!(parts.len(), 3);
    assert!(parts.contains(&"Alice"));
    assert!(parts.contains(&"Bob"));
    assert!(parts.contains(&"Carol"));
    store.close().unwrap();
}

#[test]
fn sql_string_agg_grouped_by_role() {
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate(
            "SELECT STRING_AGG(PAYLOAD_FIELD('name'), '|') FROM 'obs://team' GROUP BY PAYLOAD_FIELD('role')",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    let admin = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("admin"))
        .expect("admin group");
    let admin_str = admin
        .collected
        .as_ref()
        .and_then(|v| v.as_str())
        .expect("admin string");
    // Alice and Carol are admins
    let admin_parts: Vec<&str> = admin_str.split('|').collect();
    assert_eq!(admin_parts.len(), 2);
    assert!(admin_parts.contains(&"Alice"));
    assert!(admin_parts.contains(&"Carol"));

    let user = results
        .iter()
        .find(|r| r.group_key.as_deref() == Some("user"))
        .expect("user group");
    assert_eq!(
        user.collected.as_ref().and_then(|v| v.as_str()),
        Some("Bob")
    );
    store.close().unwrap();
}

#[test]
fn sql_string_agg_non_string_field_skipped() {
    // 'score' is a number — STRING_AGG skips it; result is empty string
    let dir = tmp();
    let store = build_collect_store(dir.path());
    let results = store
        .sql_aggregate("SELECT STRING_AGG(PAYLOAD_FIELD('score'), ',') FROM 'obs://team'")
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].collected.as_ref().and_then(|v| v.as_str()),
        Some("")
    );
    store.close().unwrap();
}

#[test]
fn sql_string_agg_parse_error_missing_sep() {
    let err =
        SqlAggregateQuery::parse("SELECT STRING_AGG(PAYLOAD_FIELD('name')) FROM 'obs://team'");
    assert!(err.is_err());
}

// ── ARRAY_LENGTH in sql_project ──────────────────────────────────────────────

#[test]
fn sql_project_array_length_basic() {
    let dir = tmp();
    let store = build_array_store(dir.path());
    let mut rows = store
        .sql_project("SELECT ARRAY_LENGTH(PAYLOAD_FIELD('tags')) AS tag_count FROM 'obs://arr'")
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("tag_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX)
    });
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["tag_count"].as_u64(), Some(0));
    assert_eq!(rows[1]["tag_count"].as_u64(), Some(1));
    assert_eq!(rows[2]["tag_count"].as_u64(), Some(3));
    store.close().unwrap();
}

#[test]
fn sql_project_array_length_non_array_passes_through() {
    let dir = tmp();
    let store = build_array_store(dir.path());
    // 'score' is a number, not an array — ARRAY_LENGTH passes through unchanged
    let rows = store
        .sql_project("SELECT ARRAY_LENGTH(PAYLOAD_FIELD('score')) AS len FROM 'obs://arr'")
        .unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        // score is a number — passes through (not an array)
        let v = &row["len"];
        assert!(v.is_number(), "expected number pass-through, got {v:?}");
    }
    store.close().unwrap();
}

// ── STDDEV / VARIANCE aggregate (issue #5) ───────────────────────────────────

#[test]
fn sql_stddev_population() {
    // sleep values: [6.0, 7.0, 8.0] — mean=7, variance=2/3, stddev=sqrt(2/3)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);
    let results = store
        .sql_aggregate("SELECT STDDEV(VALUE) FROM 'obs://health.sleep'")
        .unwrap();
    assert_eq!(results.len(), 1);
    let expected = (2.0f64 / 3.0).sqrt();
    assert!(
        (results[0].value - expected).abs() < 1e-9,
        "expected {expected}, got {}",
        results[0].value
    );
    store.close().unwrap();
}

#[test]
fn sql_variance_population() {
    // sleep values: [6.0, 7.0, 8.0] — variance = 2/3
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sleep_store(&store);
    let results = store
        .sql_aggregate("SELECT VARIANCE(VALUE) FROM 'obs://health.sleep'")
        .unwrap();
    assert_eq!(results.len(), 1);
    let expected = 2.0f64 / 3.0;
    assert!(
        (results[0].value - expected).abs() < 1e-9,
        "expected {expected}, got {}",
        results[0].value
    );
    store.close().unwrap();
}

#[test]
fn sql_stddev_single_element_returns_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://single:one"), encode(5.0));
    store.commit(tx, "test", "seed").unwrap();
    let results = store
        .sql_aggregate("SELECT STDDEV(VALUE) FROM 'obs://single'")
        .unwrap();
    assert_eq!(results.len(), 1);
    // single-element population stddev is 0
    assert!((results[0].value).abs() < 1e-9);
    store.close().unwrap();
}

// ── WHERE PAYLOAD_FIELD HAS — array containment filter (issue #5) ────────────

#[test]
fn project_payload_filter_has_element() {
    // HAS 'perf' → only obs://arr:one has that tag
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('tags') FROM 'obs://arr' WHERE PAYLOAD_FIELD('tags') HAS 'perf'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let tags = rows[0]["tags"].as_array().expect("tags must be an array");
    assert!(tags.iter().any(|v| v.as_str() == Some("perf")));
    store.close().unwrap();
}

#[test]
fn project_payload_filter_has_no_match() {
    // HAS 'missing' → no rows match
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('tags') FROM 'obs://arr' WHERE PAYLOAD_FIELD('tags') HAS 'missing'",
        )
        .unwrap();
    assert_eq!(rows.len(), 0);
    store.close().unwrap();
}

#[test]
fn project_payload_filter_has_empty_array() {
    // The empty-tags row should not match any HAS predicate
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('tags') FROM 'obs://arr' WHERE PAYLOAD_FIELD('tags') HAS 'graph'",
        )
        .unwrap();
    // obs://arr:two has 'graph'; obs://arr:three has empty tags — only one match
    assert_eq!(rows.len(), 1);
    store.close().unwrap();
}

#[test]
fn project_payload_filter_has_non_array_field_no_match() {
    // 'score' is a number, not an array — HAS always returns false
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('score') FROM 'obs://arr' WHERE PAYLOAD_FIELD('score') HAS '1'",
        )
        .unwrap();
    assert_eq!(rows.len(), 0);
    store.close().unwrap();
}

// ── PAYLOAD_FIELD('f')[n] — array element index in SELECT (issue #5) ─────────

#[test]
fn project_payload_field_index_first_element() {
    // tags[0] for each row
    let dir = tmp();
    let store = build_array_store(dir.path());
    let mut rows = store
        .sql_project("SELECT PAYLOAD_FIELD('tags')[0] AS first_tag FROM 'obs://arr'")
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("first_tag")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default()
    });
    // obs://arr:one → "rust", obs://arr:two → "graph", obs://arr:three → null (empty)
    assert_eq!(rows.len(), 3);
    let first: Vec<_> = rows
        .iter()
        .map(|r| {
            r.get("first_tag")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        })
        .collect();
    assert!(
        first.iter().any(|v| v.as_str() == Some("rust")),
        "expected 'rust' as first tag of obs://arr:one"
    );
    assert!(
        first.iter().any(|v| v.as_str() == Some("graph")),
        "expected 'graph' as first tag of obs://arr:two"
    );
    assert!(
        first.iter().any(|v| v.is_null()),
        "empty array index should be null"
    );
    store.close().unwrap();
}

#[test]
fn project_payload_field_index_out_of_bounds_is_null() {
    // tags[99] → null for all rows
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('tags')[99] AS t FROM 'obs://arr'")
        .unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert!(
            row.get("t").map(|v| v.is_null()).unwrap_or(true),
            "out-of-bounds index must be null"
        );
    }
    store.close().unwrap();
}

#[test]
fn project_payload_field_index_non_array_is_null() {
    // score is a number — index access returns null
    let dir = tmp();
    let store = build_array_store(dir.path());
    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('score')[0] AS s FROM 'obs://arr'")
        .unwrap();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert!(
            row.get("s").map(|v| v.is_null()).unwrap_or(true),
            "non-array index must be null"
        );
    }
    store.close().unwrap();
}
