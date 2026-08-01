#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── RETURN id(b) tests (issue #2) ────────────────────────────────────────────

#[test]
fn return_id_populates_field() {
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse("MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' RETURN id(b)")
        .unwrap();
    assert!(q.return_id);
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 3);
    let id_val = rows[0].fields.get("id(b)").and_then(|v| v.as_str());
    assert_eq!(id_val, Some("rid://alice"));
    let id_val2 = rows[1].fields.get("id(b)").and_then(|v| v.as_str());
    assert_eq!(id_val2, Some("rid://bob"));
    store.close().unwrap();
}

#[test]
fn return_id_combined_with_payload_fields() {
    // RETURN id(b), b.name → both object_id and payload field in fields map
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' RETURN id(b), b.name",
    )
    .unwrap();
    assert!(q.return_id);
    assert!(q.return_fields.contains(&"name".to_string()));
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 3);
    let first = &rows[0];
    assert_eq!(
        first.fields.get("id(b)").and_then(|v| v.as_str()),
        Some("rid://alice")
    );
    assert_eq!(
        first.fields.get("name").and_then(|v| v.as_str()),
        Some("alice")
    );
    store.close().unwrap();
}

#[test]
fn return_id_with_order_by_name() {
    // RETURN id(b), b.score ORDER BY b.score DESC
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' RETURN id(b), b.score ORDER BY b.score DESC",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3);
    // DESC score: bob(20), carol(15), alice(10)
    assert_eq!(
        rows[0].fields.get("id(b)").and_then(|v| v.as_str()),
        Some("rid://bob")
    );
    assert_eq!(
        rows[1].fields.get("id(b)").and_then(|v| v.as_str()),
        Some("rid://carol")
    );
    assert_eq!(
        rows[2].fields.get("id(b)").and_then(|v| v.as_str()),
        Some("rid://alice")
    );
    store.close().unwrap();
}

#[test]
fn return_id_parse_flag_not_set_without_keyword() {
    // Without id(b) in RETURN, return_id should be false
    let q = GraphIrQuery::parse("MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' RETURN b.name")
        .unwrap();
    assert!(!q.return_id);
}

// ── id(b) ENDS WITH / CONTAINS (NodeIdPred extensions, issue #2) ─────────────

#[test]
fn end_pred_ends_with_filters_targets() {
    // id(b) ENDS WITH 'alice' → only rid://alice
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' AND id(b) ENDS WITH 'alice' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "rid://alice");
    store.close().unwrap();
}

#[test]
fn end_pred_contains_filters_targets() {
    // id(b) CONTAINS '//b' → rid://bob
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' AND id(b) CONTAINS '//b' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "rid://bob");
    store.close().unwrap();
}

#[test]
fn end_pred_ends_with_multiple_matches() {
    // id(b) ENDS WITH 'ol' → carol (rid://carol ends with 'ol')
    let (store, _dir) = build_return_id_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'rid://root' AND id(b) ENDS WITH 'ol' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "rid://carol");
    store.close().unwrap();
}

#[test]
fn end_pred_parse_ends_with() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'ns://root' AND id(b) ENDS WITH '://suffix' RETURN b",
    )
    .unwrap();
    assert_eq!(q.end_pred, NodeIdPred::EndsWith("://suffix".into()));
}

#[test]
fn end_pred_parse_contains() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'ns://root' AND id(b) CONTAINS 'mid' RETURN b",
    )
    .unwrap();
    assert_eq!(q.end_pred, NodeIdPred::Contains("mid".into()));
}

// ── NodeIdPred::InList — id(b) IN ['x://a', 'x://b'] ─────────────────────────

#[test]
fn id_in_list_matches_subset() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("il://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("il://a"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("il://b"), br#"{"v":2}"#.to_vec());
    tx.put_object(obj("il://c"), br#"{"v":3}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("il_e1"), obj("il://hub"), obj("il://a"), "IL", None);
    tx2.put_edge(eid("il_e2"), obj("il://hub"), obj("il://b"), "IL", None);
    tx2.put_edge(eid("il_e3"), obj("il://hub"), obj("il://c"), "IL", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:IL]->(b) WHERE id(a) = 'il://hub' AND id(b) IN ['il://a','il://c'] RETURN b.v",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let vals: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("v").and_then(|v| v.as_i64()))
        .collect();
    assert!(vals.contains(&1));
    assert!(vals.contains(&3));
    store.close().unwrap();
}

#[test]
fn id_in_list_single_entry() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ils://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("ils://x"), br#"{"n":"x"}"#.to_vec());
    tx.put_object(obj("ils://y"), br#"{"n":"y"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("ils_e1"), obj("ils://hub"), obj("ils://x"), "S", None);
    tx2.put_edge(eid("ils_e2"), obj("ils://hub"), obj("ils://y"), "S", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:S]->(b) WHERE id(a) = 'ils://hub' AND id(b) IN ['ils://x'] RETURN b.n",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("n"),
        Some(&serde_json::Value::String("x".into()))
    );
    store.close().unwrap();
}

#[test]
fn id_in_list_none_match() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("iln://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("iln://a"), br#"{"v":1}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("iln_e1"), obj("iln://hub"), obj("iln://a"), "N", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:N]->(b) WHERE id(a) = 'iln://hub' AND id(b) IN ['iln://missing'] RETURN b.v",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn id_in_list_parse_produces_in_list_pred() {
    use bindb::NodeIdPred;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' AND id(b) IN ['x://b','x://c'] RETURN b.v",
    )
    .unwrap();
    assert!(matches!(q.end_pred, NodeIdPred::InList(_)));
    if let NodeIdPred::InList(ref ids) = q.end_pred {
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"x://b".to_string()));
        assert!(ids.contains(&"x://c".to_string()));
    }
}

// ── b.field NOT LIKE 'pattern' ────────────────────────────────────────────────

#[test]
fn not_like_excludes_matching_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nl://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nl://a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("nl://b"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("nl://c"), br#"{"name":"charlie"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nl_e1"), obj("nl://hub"), obj("nl://a"), "P", None);
    tx2.put_edge(eid("nl_e2"), obj("nl://hub"), obj("nl://b"), "P", None);
    tx2.put_edge(eid("nl_e3"), obj("nl://hub"), obj("nl://c"), "P", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    // NOT LIKE 'a%' excludes alice; bob and charlie pass
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:P]->(b) WHERE id(a) = 'nl://hub' AND b.name NOT LIKE 'a%' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(!names.contains(&"alice"));
    assert!(names.contains(&"bob"));
    assert!(names.contains(&"charlie"));
    store.close().unwrap();
}

#[test]
fn not_like_parse_sets_negated_flag() {
    use bindb::CmpOp;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' AND b.name NOT LIKE 'tmp%' RETURN b.name",
    )
    .unwrap();
    assert_eq!(q.payload_preds.len(), 1);
    assert_eq!(q.payload_preds[0].op, CmpOp::Like);
    assert!(q.payload_preds[0].negated);
}

#[test]
fn not_like_all_excluded_returns_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nle://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nle://a"), br#"{"tag":"test_a"}"#.to_vec());
    tx.put_object(obj("nle://b"), br#"{"tag":"test_b"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nle_e1"), obj("nle://hub"), obj("nle://a"), "Q", None);
    tx2.put_edge(eid("nle_e2"), obj("nle://hub"), obj("nle://b"), "Q", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:Q]->(b) WHERE id(a) = 'nle://hub' AND b.tag NOT LIKE 'test%' RETURN b.tag",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

// ── RETURN b.field AS alias ───────────────────────────────────────────────────

#[test]
fn return_field_alias_renames_output_key() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("al://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("al://a"), br#"{"name":"alice"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("al_e1"), obj("al://hub"), obj("al://a"), "AL", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:AL]->(b) WHERE id(a) = 'al://hub' RETURN b.name AS n",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    // Alias "n" is used as the key, raw "name" is not present
    assert_eq!(
        rows[0].fields.get("n"),
        Some(&serde_json::Value::String("alice".into()))
    );
    assert!(!rows[0].fields.contains_key("name"));
    store.close().unwrap();
}

#[test]
fn return_field_alias_multiple_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("alm://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("alm://a"), br#"{"first":"bob","age":30}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("alm_e1"), obj("alm://hub"), obj("alm://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'alm://hub' RETURN b.first AS fname, b.age AS years",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("fname"),
        Some(&serde_json::Value::String("bob".into()))
    );
    assert_eq!(
        rows[0].fields.get("years"),
        Some(&serde_json::Value::Number(30.into()))
    );
    assert!(!rows[0].fields.contains_key("first"));
    assert!(!rows[0].fields.contains_key("age"));
    store.close().unwrap();
}

#[test]
fn return_field_alias_parse_stored_in_struct() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' RETURN b.color AS c, b.size AS s",
    )
    .unwrap();
    assert_eq!(q.return_fields, vec!["color", "size"]);
    assert_eq!(q.return_field_aliases.get("color"), Some(&"c".to_string()));
    assert_eq!(q.return_field_aliases.get("size"), Some(&"s".to_string()));
}

#[test]
fn return_field_no_alias_uses_raw_name() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nal://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nal://a"), br#"{"score":99}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nal_e1"), obj("nal://hub"), obj("nal://a"), "R", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:R]->(b) WHERE id(a) = 'nal://hub' RETURN b.score")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("score"),
        Some(&serde_json::Value::Number(99.into()))
    );
    store.close().unwrap();
}

// ── ILIKE tests ───────────────────────────────────────────────────────────────

#[test]
fn ilike_matches_case_insensitively() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ilk://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("ilk://a"), br#"{"name":"Alice"}"#.to_vec());
    tx.put_object(obj("ilk://b"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("ilk://c"), br#"{"name":"ALICE"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("ilk_e1"), obj("ilk://hub"), obj("ilk://a"), "K", None);
    tx2.put_edge(eid("ilk_e2"), obj("ilk://hub"), obj("ilk://b"), "K", None);
    tx2.put_edge(eid("ilk_e3"), obj("ilk://hub"), obj("ilk://c"), "K", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:K]->(b) WHERE id(a) = 'ilk://hub' AND b.name ILIKE 'alice' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn ilike_wildcard_pattern() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ilk2://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("ilk2://a"), br#"{"city":"New York"}"#.to_vec());
    tx.put_object(obj("ilk2://b"), br#"{"city":"new york"}"#.to_vec());
    tx.put_object(obj("ilk2://c"), br#"{"city":"Newark"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ilk2_e1"),
        obj("ilk2://hub"),
        obj("ilk2://a"),
        "K",
        None,
    );
    tx2.put_edge(
        eid("ilk2_e2"),
        obj("ilk2://hub"),
        obj("ilk2://b"),
        "K",
        None,
    );
    tx2.put_edge(
        eid("ilk2_e3"),
        obj("ilk2://hub"),
        obj("ilk2://c"),
        "K",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:K]->(b) WHERE id(a) = 'ilk2://hub' AND b.city ILIKE 'new%' RETURN b.city",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    store.close().unwrap();
}

#[test]
fn not_ilike_excludes_matching() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nilk://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nilk://a"), br#"{"tag":"Admin"}"#.to_vec());
    tx.put_object(obj("nilk://b"), br#"{"tag":"admin"}"#.to_vec());
    tx.put_object(obj("nilk://c"), br#"{"tag":"user"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("nilk_e1"),
        obj("nilk://hub"),
        obj("nilk://a"),
        "K",
        None,
    );
    tx2.put_edge(
        eid("nilk_e2"),
        obj("nilk://hub"),
        obj("nilk://b"),
        "K",
        None,
    );
    tx2.put_edge(
        eid("nilk_e3"),
        obj("nilk://hub"),
        obj("nilk://c"),
        "K",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:K]->(b) WHERE id(a) = 'nilk://hub' AND b.tag NOT ILIKE 'admin' RETURN b.tag",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tag"),
        Some(&serde_json::Value::String("user".into()))
    );
    store.close().unwrap();
}

#[test]
fn ilike_parse_stored_in_struct() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:K]->(b) WHERE id(a) = 'x://hub' AND b.name ILIKE 'al%' RETURN b.name",
    )
    .unwrap();
    assert_eq!(q.payload_preds.len(), 1);
    assert_eq!(q.payload_preds[0].op, CmpOp::ILike);
    assert_eq!(q.payload_preds[0].value, CmpValue::Text("al%".into()));
}

// ── RETURN b.* wildcard tests ──────────────────────────────────────────────────

#[test]
fn return_all_fields_basic() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("raf://hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("raf://a"),
        br#"{"name":"bob","age":25,"score":88}"#.to_vec(),
    );
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("raf_e1"), obj("raf://hub"), obj("raf://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'raf://hub' RETURN b.*")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name"),
        Some(&serde_json::Value::String("bob".into()))
    );
    assert_eq!(
        rows[0].fields.get("age"),
        Some(&serde_json::Value::Number(25.into()))
    );
    assert_eq!(
        rows[0].fields.get("score"),
        Some(&serde_json::Value::Number(88.into()))
    );
    store.close().unwrap();
}

#[test]
fn return_all_fields_no_payload() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("raf2://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("raf2://a"), br#"{}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("raf2_e1"),
        obj("raf2://hub"),
        obj("raf2://a"),
        "X",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'raf2://hub' RETURN b.*")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].fields.is_empty());
    store.close().unwrap();
}

#[test]
fn return_all_fields_multiple_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("raf3://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("raf3://a"), br#"{"x":1}"#.to_vec());
    tx.put_object(obj("raf3://b"), br#"{"y":2}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("raf3_e1"),
        obj("raf3://hub"),
        obj("raf3://a"),
        "X",
        None,
    );
    tx2.put_edge(
        eid("raf3_e2"),
        obj("raf3://hub"),
        obj("raf3://b"),
        "X",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'raf3://hub' RETURN b.*")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let x_row = rows.iter().find(|r| r.fields.contains_key("x")).unwrap();
    let y_row = rows.iter().find(|r| r.fields.contains_key("y")).unwrap();
    assert_eq!(
        x_row.fields.get("x"),
        Some(&serde_json::Value::Number(1.into()))
    );
    assert_eq!(
        y_row.fields.get("y"),
        Some(&serde_json::Value::Number(2.into()))
    );
    store.close().unwrap();
}

#[test]
fn return_all_fields_parse_flag() {
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://h' RETURN b.*").unwrap();
    assert!(q.return_all_fields);
    assert!(q.return_fields.is_empty());
}

// ── id(b) NOT IN [...] tests ──────────────────────────────────────────────────

#[test]
fn node_id_not_in_list_excludes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nin://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nin://a"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("nin://b"), br#"{"v":2}"#.to_vec());
    tx.put_object(obj("nin://c"), br#"{"v":3}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nin_e1"), obj("nin://hub"), obj("nin://a"), "E", None);
    tx2.put_edge(eid("nin_e2"), obj("nin://hub"), obj("nin://b"), "E", None);
    tx2.put_edge(eid("nin_e3"), obj("nin://hub"), obj("nin://c"), "E", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:E]->(b) WHERE id(a) = 'nin://hub' AND id(b) NOT IN ['nin://a','nin://c'] RETURN b.v",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("v"),
        Some(&serde_json::Value::Number(2.into()))
    );
    store.close().unwrap();
}

#[test]
fn node_id_not_in_list_empty_excludes_all() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nin2://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nin2://a"), br#"{"v":1}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("nin2_e1"),
        obj("nin2://hub"),
        obj("nin2://a"),
        "E",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:E]->(b) WHERE id(a) = 'nin2://hub' AND id(b) NOT IN ['nin2://a'] RETURN b.v",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn node_id_not_in_list_parse_stored() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://h' AND id(b) NOT IN ['x://a','x://b'] RETURN b.v",
    )
    .unwrap();
    assert!(matches!(q.end_pred, NodeIdPred::NotInList(_)));
    if let NodeIdPred::NotInList(ids) = &q.end_pred {
        assert_eq!(ids.len(), 2);
    }
}

#[test]
fn node_id_not_in_list_all_pass_when_no_overlap() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nin3://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nin3://a"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("nin3://b"), br#"{"v":2}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("nin3_e1"),
        obj("nin3://hub"),
        obj("nin3://a"),
        "E",
        None,
    );
    tx2.put_edge(
        eid("nin3_e2"),
        obj("nin3://hub"),
        obj("nin3://b"),
        "E",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:E]->(b) WHERE id(a) = 'nin3://hub' AND id(b) NOT IN ['other://x'] RETURN b.v",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}
