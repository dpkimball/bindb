#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── Edge payload predicates / AND r.field OP value (issue #2) ────────────────

#[test]
fn edge_pred_parse_captures_r_field() {
    // Parser: AND r.weight > 0.5 → edge_preds = [PayloadPred { field: "weight", op: Gt, value: Number(0.5) }]
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' AND r.weight > 0.5 RETURN b",
    )
    .unwrap();
    assert_eq!(q.edge_preds.len(), 1);
    assert_eq!(q.edge_preds[0].field, "weight");
    assert_eq!(q.edge_preds[0].op, CmpOp::Gt);
    assert_eq!(q.edge_preds[0].value, CmpValue::Number(0.5));
    assert!(q.payload_preds.is_empty(), "b preds unaffected");
}

#[test]
fn edge_pred_filters_by_numeric_weight() {
    // AND r.weight > 0.5 should return only strong1 and strong2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let matches = store
        .graph_ir("MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' AND r.weight > 0.5 RETURN b")
        .unwrap();

    let ids: Vec<_> = matches
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert_eq!(ids.len(), 2, "only strong1 + strong2 pass weight > 0.5");
    assert!(ids.contains(&"n://strong1"));
    assert!(ids.contains(&"n://strong2"));
    store.close().unwrap();
}

#[test]
fn edge_pred_string_eq_filters_by_kind() {
    // AND r.kind = 'weak' should return only weak1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let matches = store
        .graph_ir("MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' AND r.kind = 'weak' RETURN b")
        .unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].object.object_id.0, "n://weak1");
    store.close().unwrap();
}

#[test]
fn edge_pred_no_payload_edges_excluded() {
    // Edges with None payload don't match any predicate → untyped node excluded.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let matches = store
        .graph_ir("MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' AND r.weight >= 0.1 RETURN b")
        .unwrap();

    let ids: Vec<_> = matches
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(!ids.contains(&"n://untyped"), "no-payload edge excluded");
    assert_eq!(ids.len(), 3, "strong1, strong2, weak1 all pass");
    store.close().unwrap();
}

#[test]
fn edge_pred_combined_with_node_pred() {
    // AND r.weight > 0.5 AND b.label = 's1' → only strong1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let matches = store
        .graph_ir(
            "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' \
             AND r.weight > 0.5 AND b.label = 's1' RETURN b",
        )
        .unwrap();

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].object.object_id.0, "n://strong1");
    store.close().unwrap();
}

#[test]
fn edge_pred_multiple_r_preds() {
    // AND r.weight > 0.5 AND r.kind = 'strong' → strong1 and strong2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' \
         AND r.weight > 0.5 AND r.kind = 'strong' RETURN b",
    )
    .unwrap();
    assert_eq!(q.edge_preds.len(), 2);

    let matches = store.graph_ir_parsed(&q).unwrap();
    assert_eq!(matches.len(), 2);
    store.close().unwrap();
}

#[test]
fn edge_pred_rel_var_in_pattern_accepted() {
    // [r:REL] syntax in the pattern is valid (r is stripped from rel body).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let matches = store
        .graph_ir("MATCH (a)-[r:REL]->(b) WHERE id(a) = 'h://wroot' AND r.weight > 0.5 RETURN b")
        .unwrap();

    assert_eq!(matches.len(), 2, "same result with or without r in pattern");
    store.close().unwrap();
}

// ── RETURN r.field — edge payload projection (issue #2) ──────────────────────

#[test]
fn edge_return_parse_captures_r_fields() {
    // Parser: RETURN r.weight, b.label → edge_return_fields=["weight"], return_fields=["label"]
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' RETURN r.weight, b.label",
    )
    .unwrap();
    assert_eq!(q.edge_return_fields, vec!["weight"]);
    assert_eq!(q.return_fields, vec!["label"]);
}

#[test]
fn edge_return_projects_edge_field() {
    // RETURN r.weight populates GraphIrRow::edge_fields["weight"].
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' RETURN r.weight, b.label",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    let strong1 = rows
        .iter()
        .find(|r| r.object_id.0 == "n://strong1")
        .expect("strong1 row");
    assert_eq!(
        strong1.edge_fields.get("weight").and_then(|v| v.as_f64()),
        Some(0.9)
    );
    assert_eq!(
        strong1.fields.get("label").and_then(|v| v.as_str()),
        Some("s1")
    );
    store.close().unwrap();
}

#[test]
fn edge_return_missing_field_is_null() {
    // Edge with no payload → edge_fields["weight"] = null.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let q = GraphIrQuery::parse("MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' RETURN r.weight")
        .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    let untyped = rows
        .iter()
        .find(|r| r.object_id.0 == "n://untyped")
        .expect("untyped row");
    assert!(
        untyped
            .edge_fields
            .get("weight")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "no-payload edge produces null for r.weight"
    );
    store.close().unwrap();
}

#[test]
fn edge_return_combined_with_edge_pred() {
    // AND r.weight > 0.5 RETURN r.weight, b.label — filter + project from same edge.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' \
         AND r.weight > 0.5 RETURN r.weight, r.kind, b.label",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 2, "only strong1 + strong2");
    for row in &rows {
        let w = row
            .edge_fields
            .get("weight")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!(w > 0.5);
        assert_eq!(
            row.edge_fields.get("kind").and_then(|v| v.as_str()),
            Some("strong")
        );
    }
    store.close().unwrap();
}

#[test]
fn edge_return_r_only_no_b_fields() {
    // RETURN r.weight with no b.field — fields empty, edge_fields populated.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_weighted_graph(&store);

    let q = GraphIrQuery::parse("MATCH (a)-[:REL]->(b) WHERE id(a) = 'h://wroot' RETURN r.kind")
        .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 4);
    let kinds: Vec<_> = rows
        .iter()
        .filter_map(|r| r.edge_fields.get("kind")?.as_str())
        .collect();
    assert!(kinds.contains(&"strong"));
    assert!(kinds.contains(&"weak"));
    store.close().unwrap();
}

// ── RETURN path edge_path — edge IDs along path (issue #2) ───────────────────

#[test]
fn path_edge_path_single_hop() {
    // Chain a -[e1]-> b: edge_path should contain ["e1"].
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("ep://a"), b"{}".to_vec());
    tx.put_object(obj("ep://b"), b"{}".to_vec());
    tx.put_edge(eid("ep_e1"), obj("ep://a"), obj("ep://b"), "HOP", None);
    store.commit(tx, "test", "").unwrap();

    let q =
        GraphIrQuery::parse("MATCH (a)-[:HOP]->(b) WHERE id(a) = 'ep://a' RETURN path").unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        row.path.iter().map(|id| id.0.as_str()).collect::<Vec<_>>(),
        vec!["ep://a", "ep://b"]
    );
    assert_eq!(
        row.edge_path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["ep_e1"]
    );
    store.close().unwrap();
}

#[test]
fn path_edge_path_multi_hop() {
    // Chain a -[e1]-> b -[e2]-> c: edge_path for c should be ["e1", "e2"].
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("ep2://a"), b"{}".to_vec());
    tx.put_object(obj("ep2://b"), b"{}".to_vec());
    tx.put_object(obj("ep2://c"), b"{}".to_vec());
    tx.put_edge(eid("ep2_e1"), obj("ep2://a"), obj("ep2://b"), "HOP2", None);
    tx.put_edge(eid("ep2_e2"), obj("ep2://b"), obj("ep2://c"), "HOP2", None);
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse("MATCH (a)-[:HOP2*1..3]->(b) WHERE id(a) = 'ep2://a' RETURN path")
        .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    let row_c = rows.iter().find(|r| r.object_id.0 == "ep2://c").unwrap();
    assert_eq!(
        row_c
            .path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["ep2://a", "ep2://b", "ep2://c"]
    );
    assert_eq!(
        row_c
            .edge_path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["ep2_e1", "ep2_e2"]
    );
    // edge_path length = path length - 1
    assert_eq!(row_c.edge_path.len(), row_c.path.len() - 1);
    store.close().unwrap();
}

#[test]
fn path_edge_path_empty_when_not_requested() {
    // Without RETURN path, edge_path is empty.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("ep3://a"), b"{}".to_vec());
    tx.put_object(obj("ep3://b"), b"{}".to_vec());
    tx.put_edge(eid("ep3_e1"), obj("ep3://a"), obj("ep3://b"), "HOP3", None);
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse("MATCH (a)-[:HOP3]->(b) WHERE id(a) = 'ep3://a' RETURN b").unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert!(rows[0].edge_path.is_empty());
    store.close().unwrap();
}

#[test]
fn path_edge_path_with_edge_payload() {
    // Edges carry payloads; edge_path IDs can be used to identify them.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("ep4://src"), b"{}".to_vec());
    tx.put_object(obj("ep4://dst"), b"{}".to_vec());
    tx.put_edge(
        eid("ep4_weighted"),
        obj("ep4://src"),
        obj("ep4://dst"),
        "W",
        Some(br#"{"weight":0.7}"#.to_vec()),
    );
    store.commit(tx, "test", "").unwrap();

    let q =
        GraphIrQuery::parse("MATCH (a)-[:W]->(b) WHERE id(a) = 'ep4://src' RETURN path, r.weight")
            .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].edge_path[0].0, "ep4_weighted");
    assert_eq!(
        rows[0].edge_fields.get("weight").and_then(|v| v.as_f64()),
        Some(0.7)
    );
    store.close().unwrap();
}
