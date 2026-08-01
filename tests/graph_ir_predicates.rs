#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── multiple AND payload predicates ──────────────────────────────────────────

#[test]
fn multi_pred_both_must_match() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://a"), b"{}".to_vec());
    tx.put_object(
        obj("node://match"),
        br#"{"sentiment":0.9,"score":5.0}"#.to_vec(),
    );
    tx.put_object(
        obj("node://low-sentiment"),
        br#"{"sentiment":0.3,"score":5.0}"#.to_vec(),
    );
    tx.put_object(
        obj("node://high-score"),
        br#"{"sentiment":0.9,"score":15.0}"#.to_vec(),
    );
    tx.put_edge(eid("e1"), obj("hub://a"), obj("node://match"), "LINK", None);
    tx.put_edge(
        eid("e2"),
        obj("hub://a"),
        obj("node://low-sentiment"),
        "LINK",
        None,
    );
    tx.put_edge(
        eid("e3"),
        obj("hub://a"),
        obj("node://high-score"),
        "LINK",
        None,
    );
    store.commit(tx, "test", "").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'hub://a' \
             AND b.sentiment > 0.5 AND b.score < 10 RETURN b",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "node://match");
    store.close().unwrap();
}

#[test]
fn multi_pred_none_match_returns_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://a"), b"{}".to_vec());
    tx.put_object(
        obj("node://x"),
        br#"{"sentiment":0.2,"score":5.0}"#.to_vec(),
    );
    tx.put_edge(eid("e1"), obj("hub://a"), obj("node://x"), "LINK", None);
    store.commit(tx, "test", "").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'hub://a' \
             AND b.sentiment > 0.5 AND b.score < 10 RETURN b",
        )
        .unwrap();

    assert!(results.is_empty());
    store.close().unwrap();
}

#[test]
fn three_payload_preds_all_required() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://a"), b"{}".to_vec());
    tx.put_object(obj("node://all"), br#"{"a":1.0,"b":2.0,"c":3.0}"#.to_vec());
    tx.put_object(obj("node://missing-c"), br#"{"a":1.0,"b":2.0}"#.to_vec());
    tx.put_edge(eid("e1"), obj("hub://a"), obj("node://all"), "LINK", None);
    tx.put_edge(
        eid("e2"),
        obj("hub://a"),
        obj("node://missing-c"),
        "LINK",
        None,
    );
    store.commit(tx, "test", "").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'hub://a' \
             AND b.a >= 1.0 AND b.b <= 3.0 AND b.c = 3.0 RETURN b",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "node://all");
    store.close().unwrap();
}

// ── string predicate tests (STARTS WITH / ENDS WITH / CONTAINS) ──────────────

#[test]
fn string_pred_starts_with_name() {
    // AND b.name STARTS WITH 'A' → only Alice
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND b.name STARTS WITH 'A' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "sp://alice");
    store.close().unwrap();
}

#[test]
fn string_pred_ends_with_role() {
    // AND b.role ENDS WITH 'or' → "editor" matches; "viewer" ends with "er", "admin" ends with "n"
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND b.role ENDS WITH 'or' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "sp://bob");
    store.close().unwrap();
}

#[test]
fn string_pred_contains_tag() {
    // AND b.tag CONTAINS 'alpha' → alice + carol
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND b.tag CONTAINS 'alpha' RETURN b",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "sp://alice");
    assert_eq!(rows[1].object_id.0, "sp://carol");
    store.close().unwrap();
}

#[test]
fn string_pred_combined_with_eq() {
    // role = 'admin' AND name CONTAINS 'Carol' → just carol
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND b.role = 'admin' AND b.name CONTAINS 'Carol' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "sp://carol");
    store.close().unwrap();
}

#[test]
fn string_pred_no_match_returns_empty() {
    // STARTS WITH 'Z' → nobody
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND b.name STARTS WITH 'Z' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn string_pred_missing_field_skipped() {
    // sp://root has no "role" field — should not appear in results.
    // Query exactly 1 hop from alice: alice→root; root has no "role" → predicate fails → empty.
    let (store, _dir) = build_string_pred_graph();
    let mut tx = Transaction::new();
    tx.put_edge(
        eid("sp_rev"),
        obj("sp://alice"),
        obj("sp://root"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'sp://alice' AND b.role STARTS WITH 'a' RETURN b",
    )
    .unwrap();
    // Root is the only 1-hop KNOWS target from alice; root has no "role" field → empty
    let rows = store.graph_ir_projected(&q).unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn string_pred_parse_op_variants() {
    // Unit-level: verify CmpOp parsed correctly for all three string ops
    let q1 = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.f STARTS WITH 'p' RETURN b",
    )
    .unwrap();
    assert_eq!(q1.payload_preds.len(), 1);
    assert_eq!(q1.payload_preds[0].op, CmpOp::StartsWith);
    assert_eq!(q1.payload_preds[0].value, CmpValue::Text("p".into()));

    let q2 = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.f ENDS WITH 's' RETURN b",
    )
    .unwrap();
    assert_eq!(q2.payload_preds[0].op, CmpOp::EndsWith);

    let q3 = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.f CONTAINS 'oo' RETURN b",
    )
    .unwrap();
    assert_eq!(q3.payload_preds[0].op, CmpOp::Contains);
}

// ── IS NULL / IS NOT NULL predicate tests ────────────────────────────────────

#[test]
fn null_check_is_not_null_finds_present_fields() {
    // b.score IS NOT NULL → a (score=10) and c (score=0)
    let (store, _dir) = build_null_check_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'nc://root' AND b.score IS NOT NULL RETURN b",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "nc://a");
    assert_eq!(rows[1].object_id.0, "nc://c");
    store.close().unwrap();
}

#[test]
fn null_check_is_null_finds_absent_fields() {
    // b.score IS NULL → b and d (no score field)
    let (store, _dir) = build_null_check_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'nc://root' AND b.score IS NULL RETURN b",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "nc://b");
    assert_eq!(rows[1].object_id.0, "nc://d");
    store.close().unwrap();
}

#[test]
fn null_check_zero_is_not_null() {
    // score=0 is present (not null) → IS NOT NULL should include c
    let (store, _dir) = build_null_check_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'nc://root' AND b.score IS NOT NULL AND b.name = 'gamma' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "nc://c");
    store.close().unwrap();
}

#[test]
fn null_check_parse_op_variants() {
    let q1 =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.f IS NULL RETURN b")
            .unwrap();
    assert_eq!(q1.payload_preds[0].op, CmpOp::IsNull);
    assert_eq!(q1.payload_preds[0].value, CmpValue::Null);

    let q2 = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.f IS NOT NULL RETURN b",
    )
    .unwrap();
    assert_eq!(q2.payload_preds[0].op, CmpOp::IsNotNull);
}

// ── IN / NOT IN predicate tests ───────────────────────────────────────────────

#[test]
fn in_pred_filters_by_list() {
    // b.role IN ['admin', 'editor'] → alice + bob + dave
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND b.role IN ['admin', 'editor'] RETURN b",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].object_id.0, "in://alice");
    assert_eq!(rows[1].object_id.0, "in://bob");
    assert_eq!(rows[2].object_id.0, "in://dave");
    store.close().unwrap();
}

#[test]
fn not_in_pred_excludes_list() {
    // b.dept NOT IN ['eng'] → carol + dave
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND b.dept NOT IN ['eng'] RETURN b",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "in://carol");
    assert_eq!(rows[1].object_id.0, "in://dave");
    store.close().unwrap();
}

#[test]
fn in_pred_empty_list_matches_nothing() {
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND b.role IN [] RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn in_pred_combined_with_eq() {
    // dept = 'hr' AND role IN ['admin'] → dave only
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND b.dept = 'hr' AND b.role IN ['admin'] RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "in://dave");
    store.close().unwrap();
}

#[test]
fn in_pred_parse_op_variant() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.role IN ['a', 'b'] RETURN b",
    )
    .unwrap();
    assert_eq!(q.payload_preds[0].op, CmpOp::In);
    assert_eq!(
        q.payload_preds[0].value,
        CmpValue::StringList(vec!["a".into(), "b".into()])
    );

    let q2 = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' AND b.role NOT IN ['a'] RETURN b",
    )
    .unwrap();
    assert_eq!(q2.payload_preds[0].op, CmpOp::NotIn);
}

// ── RETURN DISTINCT tests ─────────────────────────────────────────────────────

#[test]
fn distinct_deduplicates_optional_expansion() {
    // When OPTIONAL MATCH matches multiple c nodes per b node, the same b node
    // appears multiple times in the final rows (once per c). DISTINCT on the
    // mandatory node's object_id removes duplicates.
    //
    // Graph: root -[LINK]-> a; a -[TAG]-> t1; a -[TAG]-> t2
    // Query: MATCH root -> a OPTIONAL MATCH a -> c RETURN DISTINCT b
    // Without DISTINCT: 2 rows (both pointing to a, optional=t1 and optional=t2).
    // With DISTINCT: 1 row (a, first optional only — first-arrival dedup).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("di://root"), b"{}".to_vec());
    tx.put_object(obj("di://a"), br#"{"name":"a"}"#.to_vec());
    tx.put_object(obj("di://t1"), br#"{"tag":"t1"}"#.to_vec());
    tx.put_object(obj("di://t2"), br#"{"tag":"t2"}"#.to_vec());
    tx.put_edge(eid("di_e1"), obj("di://root"), obj("di://a"), "LINK", None);
    tx.put_edge(eid("di_e2"), obj("di://a"), obj("di://t1"), "TAG", None);
    tx.put_edge(eid("di_e3"), obj("di://a"), obj("di://t2"), "TAG", None);
    store.commit(tx, "test", "").unwrap();

    // Without DISTINCT: a appears twice (once per tag)
    let q_no_distinct = GraphIrQuery::parse(
        "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'di://root' OPTIONAL MATCH (b)-[:TAG]->(c) RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q_no_distinct).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "without DISTINCT, 2 rows (one per optional c)"
    );

    // With DISTINCT: a appears only once
    let q_distinct = GraphIrQuery::parse(
        "MATCH (a)-[:LINK]->(b) WHERE id(a) = 'di://root' OPTIONAL MATCH (b)-[:TAG]->(c) RETURN DISTINCT b",
    )
    .unwrap();
    assert!(q_distinct.distinct);
    let rows = store.graph_ir_projected(&q_distinct).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "di://a");
    store.close().unwrap();
}

#[test]
fn distinct_preserves_single_occurrence() {
    // When each node appears only once, DISTINCT is a no-op
    let (store, _dir) = build_string_pred_graph();
    let q =
        GraphIrQuery::parse("MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' RETURN DISTINCT b")
            .unwrap();
    assert!(q.distinct);
    let rows = store.graph_ir_projected(&q).unwrap();
    // All 4 leaf nodes appear once each
    assert_eq!(rows.len(), 4);
    store.close().unwrap();
}

#[test]
fn distinct_parse_flag() {
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' RETURN DISTINCT b").unwrap();
    assert!(q.distinct);

    let q2 = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://1' RETURN b").unwrap();
    assert!(!q2.distinct);
}

// ── NOT predicate tests ───────────────────────────────────────────────────────

#[test]
fn not_pred_negates_eq() {
    // AND NOT b.role = 'admin' → bob (editor) + carol (viewer) [using in_pred graph]
    // reuse build_in_pred_graph: alice=admin, bob=editor, carol=viewer, dave=admin
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND NOT b.role = 'admin' RETURN b",
    )
    .unwrap();
    assert!(q.payload_preds[0].negated);
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "in://bob");
    assert_eq!(rows[1].object_id.0, "in://carol");
    store.close().unwrap();
}

#[test]
fn not_pred_negates_contains() {
    // AND NOT b.tag CONTAINS 'alpha' → bob + dave [using string_pred graph]
    let (store, _dir) = build_string_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sp://root' AND NOT b.tag CONTAINS 'alpha' RETURN b",
    )
    .unwrap();
    assert!(q.payload_preds[0].negated);
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by(|a, b| a.object_id.0.cmp(&b.object_id.0));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].object_id.0, "sp://bob");
    assert_eq!(rows[1].object_id.0, "sp://dave");
    store.close().unwrap();
}

#[test]
fn not_pred_combined() {
    // AND NOT b.role = 'admin' AND NOT b.dept = 'eng' → carol only (viewer, hr)
    let (store, _dir) = build_in_pred_graph();
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MEMBER]->(b) WHERE id(a) = 'in://root' AND NOT b.role = 'admin' AND NOT b.dept = 'eng' RETURN b",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "in://carol");
    store.close().unwrap();
}

// ── BETWEEN predicate (issue #2) ─────────────────────────────────────────────

#[test]
fn between_matches_nodes_in_range() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_between_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.price BETWEEN 10 AND 100 RETURN b",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "item://mid");
    store.close().unwrap();
}

#[test]
fn between_boundary_inclusive() {
    // Boundaries (10.0 and 100.0) are included.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_between_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.price BETWEEN 5 AND 150 RETURN b",
        )
        .unwrap();

    // low(5), mid(50), high(150) all included; no-price excluded (missing field)
    assert_eq!(results.len(), 3);
    store.close().unwrap();
}

#[test]
fn between_outside_range_excluded() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_between_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.price BETWEEN 200 AND 999 RETURN b",
        )
        .unwrap();

    assert!(results.is_empty());
    store.close().unwrap();
}

#[test]
fn between_combined_with_other_pred() {
    // price BETWEEN 10 AND 100 AND b.price < 60 → only item://mid (50)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_between_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.price BETWEEN 10 AND 100 AND b.price < 60 RETURN b",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "item://mid");
    store.close().unwrap();
}

#[test]
fn between_parse_roundtrip() {
    use bindb::{CmpOp, CmpValue, GraphIrQuery};
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.price BETWEEN 10 AND 100 RETURN b",
    )
    .unwrap();
    assert_eq!(q.payload_preds.len(), 1);
    let pred = &q.payload_preds[0];
    assert_eq!(pred.field, "price");
    assert_eq!(pred.op, CmpOp::Between);
    assert_eq!(pred.value, CmpValue::NumberRange(10.0, 100.0));
    assert!(!pred.negated);
}

#[test]
fn between_on_edge_payload() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://r"), b"{}".to_vec());
    tx.put_object(obj("item://a"), b"{}".to_vec());
    tx.put_object(obj("item://b"), b"{}".to_vec());
    tx.put_edge(
        eid("cheap"),
        obj("hub://r"),
        obj("item://a"),
        "SOLD",
        Some(br#"{"weight":3.0}"#.to_vec()),
    );
    tx.put_edge(
        eid("heavy"),
        obj("hub://r"),
        obj("item://b"),
        "SOLD",
        Some(br#"{"weight":20.0}"#.to_vec()),
    );
    store.commit(tx, "test", "seed").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[r:SOLD]->(b) WHERE id(a) = 'hub://r' AND r.weight BETWEEN 1 AND 5 RETURN b",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "item://a");
    store.close().unwrap();
}

// ── LIKE predicate (issue #2) ─────────────────────────────────────────────────

#[test]
fn like_prefix_wildcard() {
    // LIKE 'alice%' → alice only
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE 'alice%' RETURN b",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "node://alice");
    store.close().unwrap();
}

#[test]
fn like_suffix_wildcard() {
    // LIKE '%@corp.com' → alice and bob
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE '%@corp.com' RETURN b",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    let ids: std::collections::HashSet<_> = results
        .iter()
        .map(|r| r.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains("node://alice") && ids.contains("node://bob"));
    store.close().unwrap();
}

#[test]
fn like_both_wildcards() {
    // LIKE '%@%.com' → alice, bob, carol (all with @something.com)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE '%@%.com' RETURN b",
        )
        .unwrap();
    assert_eq!(results.len(), 3);
    store.close().unwrap();
}

#[test]
fn like_underscore_wildcard() {
    // LIKE 'US0_' → US01, US02 (2 chars: U S 0 and any 1 char)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.code LIKE 'US0_' RETURN b",
        )
        .unwrap();
    assert_eq!(results.len(), 2);
    store.close().unwrap();
}

#[test]
fn like_no_wildcards_exact() {
    // LIKE 'alice@corp.com' → exact match
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE 'alice@corp.com' RETURN b",
        )
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "node://alice");
    store.close().unwrap();
}

#[test]
fn like_parse_roundtrip() {
    use bindb::{CmpOp, CmpValue, GraphIrQuery};
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE 'a%c' RETURN b",
    )
    .unwrap();
    assert_eq!(q.payload_preds.len(), 1);
    let pred = &q.payload_preds[0];
    assert_eq!(pred.field, "name");
    assert_eq!(pred.op, CmpOp::Like);
    assert_eq!(pred.value, CmpValue::Text("a%c".into()));
}

#[test]
fn like_no_match_returns_empty() {
    // LIKE 'xyz%' → none
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_like_graph(&store);
    let results = store
        .graph_ir(
            "MATCH (a)-[:HAS]->(b) WHERE id(a) = 'hub://root' AND b.name LIKE 'xyz%' RETURN b",
        )
        .unwrap();
    assert!(results.is_empty());
    store.close().unwrap();
}

// ── Named path variable `p = (a)-[r]->(b)` (issue #2) ───────────────────────

#[test]
fn named_path_var_return_path_equivalent() {
    // `MATCH p = (a)-[:STEP*1..3]->(b) WHERE … RETURN p` must be identical to `RETURN path`
    use bindb::{BinderStore, GraphIrQuery};
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = bindb::Transaction::new();
    tx.put_object(obj("pv://a"), b"{}".to_vec());
    tx.put_object(obj("pv://b"), b"{}".to_vec());
    tx.put_object(obj("pv://c"), b"{}".to_vec());
    tx.put_edge(eid("pv_e1"), obj("pv://a"), obj("pv://b"), "LINK", None);
    tx.put_edge(eid("pv_e2"), obj("pv://b"), obj("pv://c"), "LINK", None);
    store.commit(tx, "test", "").unwrap();

    let q_named =
        GraphIrQuery::parse("MATCH p = (a)-[:LINK*1..3]->(b) WHERE id(a) = 'pv://a' RETURN p")
            .unwrap();
    let q_plain =
        GraphIrQuery::parse("MATCH (a)-[:LINK*1..3]->(b) WHERE id(a) = 'pv://a' RETURN path")
            .unwrap();

    assert!(q_named.return_path, "named var should set return_path");
    assert_eq!(q_named.path_var, Some("p".to_string()));
    assert!(q_plain.path_var.is_none());

    let rows_named = store.graph_ir_projected(&q_named).unwrap();
    let rows_plain = store.graph_ir_projected(&q_plain).unwrap();
    assert_eq!(rows_named.len(), rows_plain.len());
    for (rn, rp) in rows_named.iter().zip(rows_plain.iter()) {
        assert_eq!(rn.path, rp.path);
    }
    store.close().unwrap();
}

#[test]
fn named_path_var_stored_in_query() {
    let q =
        GraphIrQuery::parse("MATCH route = (a)-[:ROAD]->(b) WHERE id(a) = 'x://a' RETURN route")
            .unwrap();
    assert_eq!(q.path_var, Some("route".to_string()));
    assert!(q.return_path);
}

#[test]
fn no_named_path_var_when_absent() {
    let q =
        GraphIrQuery::parse("MATCH (a)-[:ROAD]->(b) WHERE id(a) = 'x://a' RETURN path").unwrap();
    assert!(q.path_var.is_none());
    assert!(q.return_path);
}

// ── OR predicate combinator ────────────────────────────────────────────────────

#[test]
fn or_predicate_returns_either_match() {
    use bindb::PayloadPredCombinator;
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("or://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("or://x"), br#"{"color":"red"}"#.to_vec());
    tx.put_object(obj("or://y"), br#"{"color":"blue"}"#.to_vec());
    tx.put_object(obj("or://z"), br#"{"color":"green"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("or_e1"), obj("or://hub"), obj("or://x"), "T", None);
    tx2.put_edge(eid("or_e2"), obj("or://hub"), obj("or://y"), "T", None);
    tx2.put_edge(eid("or_e3"), obj("or://hub"), obj("or://z"), "T", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:T]->(b) WHERE id(a) = 'or://hub' AND b.color = 'red' OR b.color = 'blue' RETURN b.color",
    )
    .unwrap();
    assert_eq!(q.pred_combinator, PayloadPredCombinator::Or);
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    let colors: std::collections::HashSet<_> = rows
        .iter()
        .filter_map(|r| r.fields.get("color").and_then(|v| v.as_str()))
        .collect();
    assert!(colors.contains("red"));
    assert!(colors.contains("blue"));
    store.close().unwrap();
}

#[test]
fn or_predicate_parse_combinator() {
    use bindb::PayloadPredCombinator;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' AND b.f = 'v1' OR b.f = 'v2' RETURN b.f",
    )
    .unwrap();
    assert_eq!(q.pred_combinator, PayloadPredCombinator::Or);
    assert_eq!(q.payload_preds.len(), 2);
}

#[test]
fn and_predicate_combinator_default() {
    use bindb::PayloadPredCombinator;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' AND b.f = 'v1' AND b.g = 'v2' RETURN b.f",
    )
    .unwrap();
    assert_eq!(q.pred_combinator, PayloadPredCombinator::And);
    assert_eq!(q.payload_preds.len(), 2);
}

#[test]
fn or_predicate_excludes_non_matching() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("exc://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("exc://a"), br#"{"tag":"keep1"}"#.to_vec());
    tx.put_object(obj("exc://b"), br#"{"tag":"keep2"}"#.to_vec());
    tx.put_object(obj("exc://c"), br#"{"tag":"drop"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("exc_e1"), obj("exc://hub"), obj("exc://a"), "L", None);
    tx2.put_edge(eid("exc_e2"), obj("exc://hub"), obj("exc://b"), "L", None);
    tx2.put_edge(eid("exc_e3"), obj("exc://hub"), obj("exc://c"), "L", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L]->(b) WHERE id(a) = 'exc://hub' AND b.tag = 'keep1' OR b.tag = 'keep2' RETURN b.tag",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

// ── Numeric IN [1, 2, 3] predicate ────────────────────────────────────────────

#[test]
fn numeric_in_list_matches_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ni://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("ni://a"), br#"{"score":10}"#.to_vec());
    tx.put_object(obj("ni://b"), br#"{"score":20}"#.to_vec());
    tx.put_object(obj("ni://c"), br#"{"score":30}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("ni_e1"), obj("ni://hub"), obj("ni://a"), "H", None);
    tx2.put_edge(eid("ni_e2"), obj("ni://hub"), obj("ni://b"), "H", None);
    tx2.put_edge(eid("ni_e3"), obj("ni://hub"), obj("ni://c"), "H", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:H]->(b) WHERE id(a) = 'ni://hub' AND b.score IN [10, 30] RETURN b.score",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let scores: Vec<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("score").and_then(|v| v.as_i64()))
        .collect();
    assert!(scores.contains(&10));
    assert!(scores.contains(&30));
    store.close().unwrap();
}

#[test]
fn numeric_not_in_list_excludes_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nni://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nni://a"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("nni://b"), br#"{"v":2}"#.to_vec());
    tx.put_object(obj("nni://c"), br#"{"v":3}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nni_e1"), obj("nni://hub"), obj("nni://a"), "K", None);
    tx2.put_edge(eid("nni_e2"), obj("nni://hub"), obj("nni://b"), "K", None);
    tx2.put_edge(eid("nni_e3"), obj("nni://hub"), obj("nni://c"), "K", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:K]->(b) WHERE id(a) = 'nni://hub' AND b.v NOT IN [2] RETURN b.v",
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
fn numeric_in_parse_produces_number_list() {
    use bindb::CmpValue;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:E]->(b) WHERE id(a) = 'x://a' AND b.score IN [1, 2, 3] RETURN b.score",
    )
    .unwrap();
    assert_eq!(q.payload_preds.len(), 1);
    assert!(matches!(q.payload_preds[0].value, CmpValue::NumberList(_)));
    if let CmpValue::NumberList(ref nums) = q.payload_preds[0].value {
        assert_eq!(nums.len(), 3);
    }
}

#[test]
fn numeric_in_no_match_empty_result() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("nem://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("nem://a"), br#"{"n":5}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("nem_e1"), obj("nem://hub"), obj("nem://a"), "M", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:M]->(b) WHERE id(a) = 'nem://hub' AND b.n IN [99, 100] RETURN b.n",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn named_path_var_with_field_projection() {
    // `MATCH p = (a)-[r]->(b) WHERE … RETURN p, b.name` — path AND field combined
    use bindb::{BinderStore, GraphIrQuery};
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = bindb::Transaction::new();
    tx.put_object(obj("pv2://a"), b"{}".to_vec());
    tx.put_object(obj("pv2://b"), br#"{"name":"beta"}"#.to_vec());
    tx.put_edge(eid("pv2_e1"), obj("pv2://a"), obj("pv2://b"), "E", None);
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse("MATCH p = (a)-[:E]->(b) WHERE id(a) = 'pv2://a' RETURN p, b.name")
        .unwrap();
    assert!(q.return_path);
    assert_eq!(q.return_fields, vec!["name"]);

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].path.is_empty());
    assert_eq!(
        rows[0].fields.get("name"),
        Some(&serde_json::Value::String("beta".into()))
    );
    store.close().unwrap();
}
