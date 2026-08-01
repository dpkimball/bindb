#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

#[test]
fn ir_knows_depth1() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_ir("MATCH (a)-[:KNOWS*1]->(b) WHERE id(a) = 'person://alice' RETURN b")
        .unwrap();
    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"memory://m1"));
    assert!(ids.contains(&"memory://m2"));
    assert!(!ids.contains(&"tag://t1"));
    assert!(!ids.contains(&"memory://m3"));
    store.close().unwrap();
}

#[test]
fn ir_prefix_and_multi_type() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:KNOWS|RELATES_TO*1..2]->(b) \
             WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'memory://' \
             RETURN b",
        )
        .unwrap();
    assert!(results
        .iter()
        .all(|m| m.object.object_id.0.starts_with("memory://")));
    assert!(results
        .iter()
        .any(|m| m.object.object_id.0 == "memory://m3"));
    store.close().unwrap();
}

#[test]
fn ir_incoming() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_ir("MATCH (a)<-[:KNOWS*1]-(b) WHERE id(a) = 'memory://m1' RETURN b")
        .unwrap();
    assert!(results
        .iter()
        .any(|m| m.object.object_id.0 == "person://alice"));
    store.close().unwrap();
}

#[test]
fn ir_exact_depth_excludes_shallower() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    // m3 is 2 hops; m1/m2 are 1 hop via KNOWS|RELATES_TO.
    let results = store
        .graph_ir("MATCH (a)-[:KNOWS|RELATES_TO*2..2]->(b) WHERE id(a) = 'person://alice' RETURN b")
        .unwrap();
    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"memory://m3"));
    assert!(!ids.contains(&"memory://m1"));
    assert!(!ids.contains(&"memory://m2"));
    store.close().unwrap();
}

#[test]
fn ir_payload_predicate_filters_by_json_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // hub → two spoke nodes; one has sentiment > 0.8, one doesn't.
    let mut tx = Transaction::new();
    tx.put_object(obj("a://hub"), b"{}".to_vec());
    tx.put_object(obj("a://high"), br#"{"sentiment":0.9}"#.to_vec());
    tx.put_object(obj("a://low"), br#"{"sentiment":0.2}"#.to_vec());
    tx.put_edge(eid("e1"), obj("a://hub"), obj("a://high"), "KNOWS", None);
    tx.put_edge(eid("e2"), obj("a://hub"), obj("a://low"), "KNOWS", None);
    store.commit(tx, "test", "").unwrap();

    let results = store
        .graph_ir("MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'a://hub' AND b.sentiment > 0.8 RETURN b")
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "a://high");
    store.close().unwrap();
}

#[test]
fn ir_projected_extracts_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://root"), b"{}".to_vec());
    tx.put_object(
        obj("note://a"),
        br#"{"sentiment":0.9,"label":"positive"}"#.to_vec(),
    );
    tx.put_object(
        obj("note://b"),
        br#"{"sentiment":0.3,"label":"negative"}"#.to_vec(),
    );
    tx.put_edge(eid("e1"), obj("hub://root"), obj("note://a"), "LINKS", None);
    tx.put_edge(eid("e2"), obj("hub://root"), obj("note://b"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'hub://root' RETURN b.sentiment, b.label",
    )
    .unwrap();
    assert_eq!(q.return_fields, vec!["sentiment", "label"]);

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);

    // Find the row for note://a and verify both fields are projected.
    let row_a = rows.iter().find(|r| r.object_id.0 == "note://a").unwrap();
    assert_eq!(
        row_a.fields.get("sentiment").and_then(|v| v.as_f64()),
        Some(0.9)
    );
    assert_eq!(
        row_a.fields.get("label").and_then(|v| v.as_str()),
        Some("positive")
    );

    // note://b should have sentiment 0.3 and label "negative".
    let row_b = rows.iter().find(|r| r.object_id.0 == "note://b").unwrap();
    assert_eq!(
        row_b.fields.get("sentiment").and_then(|v| v.as_f64()),
        Some(0.3)
    );
    assert_eq!(
        row_b.fields.get("label").and_then(|v| v.as_str()),
        Some("negative")
    );

    store.close().unwrap();
}

#[test]
fn ir_return_path_collects_shortest_path() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Chain: a → b → c
    let mut tx = Transaction::new();
    tx.put_object(obj("n://a"), b"{}".to_vec());
    tx.put_object(obj("n://b"), b"{}".to_vec());
    tx.put_object(obj("n://c"), b"{}".to_vec());
    tx.put_edge(eid("e1"), obj("n://a"), obj("n://b"), "STEP", None);
    tx.put_edge(eid("e2"), obj("n://b"), obj("n://c"), "STEP", None);
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse("MATCH (a)-[:STEP*1..3]->(b) WHERE id(a) = 'n://a' RETURN path")
        .unwrap();
    assert!(q.return_path);
    assert!(q.return_fields.is_empty());

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2); // b and c

    let row_b = rows.iter().find(|r| r.object_id.0 == "n://b").unwrap();
    assert_eq!(
        row_b
            .path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["n://a", "n://b"]
    );

    let row_c = rows.iter().find(|r| r.object_id.0 == "n://c").unwrap();
    assert_eq!(
        row_c
            .path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["n://a", "n://b", "n://c"]
    );

    store.close().unwrap();
}

#[test]
fn ir_return_path_and_field_combined() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("p://root"), b"{}".to_vec());
    tx.put_object(obj("p://leaf"), br#"{"score":42}"#.to_vec());
    tx.put_edge(eid("e1"), obj("p://root"), obj("p://leaf"), "HAS", None);
    store.commit(tx, "test", "").unwrap();

    let q =
        GraphIrQuery::parse("MATCH (a)-[:HAS]->(b) WHERE id(a) = 'p://root' RETURN b.score, path")
            .unwrap();
    assert!(q.return_path);
    assert_eq!(q.return_fields, vec!["score"]);

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("score").and_then(|v| v.as_f64()),
        Some(42.0)
    );
    assert_eq!(
        rows[0]
            .path
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["p://root", "p://leaf"]
    );

    store.close().unwrap();
}

// ── write IR tests ────────────────────────────────────────────────────────────

#[test]
fn write_ir_create_node_and_read_back() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let cid = store
        .graph_ir_write(r#"CREATE NODE id="note://foo" payload={"text":"hello"}"#)
        .unwrap();
    assert!(cid.is_some());

    let obj = store.get(&ObjectId::new("note://foo")).unwrap();
    assert!(obj.is_some());
    let payload: serde_json::Value =
        serde_json::from_slice(obj.unwrap().payload.as_deref().unwrap()).unwrap();
    assert_eq!(payload["text"], "hello");

    store.close().unwrap();
}

#[test]
fn write_ir_merge_node_skips_existing() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // First write creates the node.
    store
        .graph_ir_write(r#"CREATE NODE id="note://bar" payload={"v":1}"#)
        .unwrap();

    // MERGE should not overwrite an existing node — returns None (no commit).
    let cid = store
        .graph_ir_write(r#"MERGE NODE id="note://bar" payload={"v":2}"#)
        .unwrap();
    assert!(cid.is_none());

    // Value should still be 1.
    let obj = store.get(&ObjectId::new("note://bar")).unwrap().unwrap();
    let payload: serde_json::Value =
        serde_json::from_slice(obj.payload.as_deref().unwrap()).unwrap();
    assert_eq!(payload["v"], 1);

    store.close().unwrap();
}

#[test]
fn write_ir_create_edge_and_traverse() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    store
        .graph_ir_write(
            r#"
CREATE NODE id="note://a" payload={"label":"alpha"}
CREATE NODE id="note://b" payload={"label":"beta"}
CREATE EDGE id="e_ab" from="note://a" to="note://b" label="LINKS_TO"
"#,
        )
        .unwrap();

    let edges = store.edges_from(&ObjectId::new("note://a")).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to_object.as_ref().unwrap().0, "note://b");
    assert_eq!(edges[0].relationship_type.as_deref(), Some("LINKS_TO"));

    store.close().unwrap();
}

#[test]
fn write_ir_merge_edge_skips_existing() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    store
        .graph_ir_write(
            r#"
CREATE NODE id="p://x" payload={}
CREATE NODE id="p://y" payload={}
CREATE EDGE id="e_xy" from="p://x" to="p://y" label="KNOWS"
"#,
        )
        .unwrap();

    // MERGE EDGE for existing id should return None.
    let cid = store
        .graph_ir_write(r#"MERGE EDGE id="e_xy" from="p://x" to="p://y" label="KNOWS""#)
        .unwrap();
    assert!(cid.is_none());

    // Still only one edge.
    let edges = store.edges_from(&ObjectId::new("p://x")).unwrap();
    assert_eq!(edges.len(), 1);

    store.close().unwrap();
}

#[test]
fn write_ir_parse_error_unknown_verb() {
    let err = GraphIrWrite::parse("UPSERT NODE id=\"x\" payload={}").unwrap_err();
    assert!(matches!(err, GraphIrWriteError::UnknownVerb { line: 1 }));
}

#[test]
fn write_ir_parse_error_missing_field() {
    let err = GraphIrWrite::parse("CREATE NODE payload={}").unwrap_err();
    assert!(matches!(
        err,
        GraphIrWriteError::MissingField {
            field: "id",
            line: 1
        }
    ));
}

#[test]
fn write_ir_comments_and_blank_lines_ignored() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    store
        .graph_ir_write(
            r#"
// create two nodes
CREATE NODE id="n://1" payload={"x":1}

# another comment
CREATE NODE id="n://2" payload={"x":2}
"#,
        )
        .unwrap();

    assert!(store.get(&ObjectId::new("n://1")).unwrap().is_some());
    assert!(store.get(&ObjectId::new("n://2")).unwrap().is_some());

    store.close().unwrap();
}

// ── ORDER BY / LIMIT / SKIP ───────────────────────────────────────────────────

/// Build a small graph where alice connects to three nodes with known IDs so
/// that ORDER BY id(b) has a deterministic expected sequence.

#[test]
fn order_by_id_asc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sortable_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://alice' RETURN b ORDER BY id(b) ASC",
        )
        .unwrap();

    let ids: Vec<&str> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert_eq!(ids, vec!["node://alpha", "node://bravo", "node://charlie"]);
    store.close().unwrap();
}

#[test]
fn order_by_id_desc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sortable_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://alice' RETURN b ORDER BY id(b) DESC",
        )
        .unwrap();

    let ids: Vec<&str> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert_eq!(ids, vec!["node://charlie", "node://bravo", "node://alpha"]);
    store.close().unwrap();
}

#[test]
fn limit_truncates_results() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sortable_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://alice' RETURN b ORDER BY id(b) ASC LIMIT 2",
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].object.object_id.0, "node://alpha");
    assert_eq!(results[1].object.object_id.0, "node://bravo");
    store.close().unwrap();
}

#[test]
fn skip_and_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sortable_graph(&store);

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://alice' RETURN b ORDER BY id(b) ASC SKIP 1 LIMIT 1",
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object.object_id.0, "node://bravo");
    store.close().unwrap();
}

#[test]
fn order_by_depth_asc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("root://a"), b"a".to_vec());
    tx.put_object(obj("node://near"), b"near".to_vec());
    tx.put_object(obj("node://far"), b"far".to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("e1"), obj("root://a"), obj("node://near"), "LINK", None);
    tx2.put_edge(
        eid("e2"),
        obj("node://near"),
        obj("node://far"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1..2]->(b) WHERE id(a) = 'root://a' RETURN b ORDER BY depth ASC",
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].depth, 1);
    assert_eq!(results[1].depth, 2);
    assert_eq!(results[0].object.object_id.0, "node://near");
    assert_eq!(results[1].object.object_id.0, "node://far");
    store.close().unwrap();
}

#[test]
fn order_by_payload_field_asc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("root://a"), b"a".to_vec());
    tx.put_object(obj("node://x"), br#"{"score":3.0}"#.to_vec());
    tx.put_object(obj("node://y"), br#"{"score":1.0}"#.to_vec());
    tx.put_object(obj("node://z"), br#"{"score":2.0}"#.to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("ex"), obj("root://a"), obj("node://x"), "LINK", None);
    tx2.put_edge(eid("ey"), obj("root://a"), obj("node://y"), "LINK", None);
    tx2.put_edge(eid("ez"), obj("root://a"), obj("node://z"), "LINK", None);
    store.commit(tx2, "test", "edges").unwrap();

    let results = store
        .graph_ir("MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://a' RETURN b ORDER BY b.score ASC")
        .unwrap();

    assert_eq!(results.len(), 3);
    // Expect ascending order: 1.0 (y), 2.0 (z), 3.0 (x)
    assert_eq!(results[0].object.object_id.0, "node://y");
    assert_eq!(results[1].object.object_id.0, "node://z");
    assert_eq!(results[2].object.object_id.0, "node://x");
    store.close().unwrap();
}

#[test]
fn order_by_payload_field_desc_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("root://a"), b"a".to_vec());
    tx.put_object(obj("node://x"), br#"{"score":3.0}"#.to_vec());
    tx.put_object(obj("node://y"), br#"{"score":1.0}"#.to_vec());
    tx.put_object(obj("node://z"), br#"{"score":2.0}"#.to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("ex"), obj("root://a"), obj("node://x"), "LINK", None);
    tx2.put_edge(eid("ey"), obj("root://a"), obj("node://y"), "LINK", None);
    tx2.put_edge(eid("ez"), obj("root://a"), obj("node://z"), "LINK", None);
    store.commit(tx2, "test", "edges").unwrap();

    let results = store
        .graph_ir(
            "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://a' RETURN b ORDER BY b.score DESC LIMIT 2",
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    // Top 2 descending: 3.0 (x), 2.0 (z)
    assert_eq!(results[0].object.object_id.0, "node://x");
    assert_eq!(results[1].object.object_id.0, "node://z");
    store.close().unwrap();
}

#[test]
fn parse_order_by_fields() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY id(b) DESC LIMIT 5",
    )
    .unwrap();
    assert_eq!(q.order_by.len(), 1);
    assert_eq!(q.order_by[0].field, OrderByField::ObjectId);
    assert_eq!(q.order_by[0].direction, OrderDirection::Desc);
    assert_eq!(q.limit, Some(5));
    assert_eq!(q.skip, None);

    let q2 = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY depth ASC SKIP 2 LIMIT 10",
    )
    .unwrap();
    assert_eq!(q2.order_by.len(), 1);
    assert_eq!(q2.order_by[0].field, OrderByField::Depth);
    assert_eq!(q2.order_by[0].direction, OrderDirection::Asc);
    assert_eq!(q2.skip, Some(2));
    assert_eq!(q2.limit, Some(10));

    let q3 = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY b.sentiment DESC",
    )
    .unwrap();
    assert_eq!(q3.order_by.len(), 1);
    assert_eq!(
        q3.order_by[0].field,
        OrderByField::PayloadField("sentiment".into())
    );
    assert_eq!(q3.order_by[0].direction, OrderDirection::Desc);
    assert_eq!(q3.limit, None);
}

#[test]
fn limit_without_order_by() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_sortable_graph(&store);

    let results = store
        .graph_ir("MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'root://alice' RETURN b LIMIT 2")
        .unwrap();

    assert_eq!(results.len(), 2, "LIMIT 2 must return at most 2 results");
    store.close().unwrap();
}
