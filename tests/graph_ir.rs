use bindb::{
    AggFunc, BinderStore, CmpOp, CmpValue, EdgeId, GraphIrQuery, GraphIrUnionQuery, GraphIrWrite,
    GraphIrWriteError, NodeIdPred, ObjectId, OrderByField, OrderDirection, Transaction,
    WithAggregation,
};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
fn obj(s: &str) -> ObjectId {
    ObjectId::new(s)
}
fn eid(s: &str) -> EdgeId {
    EdgeId::new(s)
}

fn build_social_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("person://alice"), b"alice".to_vec());
    tx.put_object(obj("memory://m1"), b"memory1".to_vec());
    tx.put_object(obj("memory://m2"), b"memory2".to_vec());
    tx.put_object(obj("memory://m3"), b"memory3".to_vec());
    tx.put_object(obj("tag://t1"), b"tag1".to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e1"),
        obj("person://alice"),
        obj("memory://m1"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e2"),
        obj("person://alice"),
        obj("memory://m2"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e3"),
        obj("person://alice"),
        obj("tag://t1"),
        "TAGGED",
        None,
    );
    tx2.put_edge(
        eid("e4"),
        obj("memory://m1"),
        obj("memory://m3"),
        "RELATES_TO",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();
}

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
fn build_sortable_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("root://alice"), b"alice".to_vec());
    // IDs chosen so alphabetical order is c < a < b (reverse of name order).
    tx.put_object(obj("node://charlie"), b"c".to_vec());
    tx.put_object(obj("node://alpha"), b"a".to_vec());
    tx.put_object(obj("node://bravo"), b"b".to_vec());
    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ea"),
        obj("root://alice"),
        obj("node://charlie"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("eb"),
        obj("root://alice"),
        obj("node://alpha"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("ec"),
        obj("root://alice"),
        obj("node://bravo"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "edges").unwrap();
}

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

// ── OPTIONAL MATCH (issue #2) ─────────────────────────────────────────────────

/// Build a graph for OPTIONAL MATCH tests:
///
/// ```text
/// person://alice --KNOWS--> person://bob   --HAS_TAG--> tag://rust
///                                          --HAS_TAG--> tag://databases
///                --KNOWS--> person://carol              (no tags)
///                --KNOWS--> person://dave  --HAS_TAG--> tag://rust
///                                          (dave also has a numeric payload)
/// ```
///
/// Payloads:
/// - person://alice: `{"name":"Alice"}`
/// - person://bob:   `{"name":"Bob","score":10}`
/// - person://carol: `{"name":"Carol","score":5}`
/// - person://dave:  `{"name":"Dave","score":20}`
/// - tag://rust:     `{"label":"rust","weight":0.9}`
/// - tag://databases:`{"label":"databases","weight":0.4}`
fn build_optional_graph(store: &BinderStore) {
    let mut tx = Transaction::new();

    // People
    tx.put_object(obj("person://alice"), br#"{"name":"Alice"}"#.to_vec());
    tx.put_object(
        obj("person://bob"),
        br#"{"name":"Bob","score":10}"#.to_vec(),
    );
    tx.put_object(
        obj("person://carol"),
        br#"{"name":"Carol","score":5}"#.to_vec(),
    );
    tx.put_object(
        obj("person://dave"),
        br#"{"name":"Dave","score":20}"#.to_vec(),
    );

    // Tags
    tx.put_object(
        obj("tag://rust"),
        br#"{"label":"rust","weight":0.9}"#.to_vec(),
    );
    tx.put_object(
        obj("tag://databases"),
        br#"{"label":"databases","weight":0.4}"#.to_vec(),
    );

    store.commit(tx, "test", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    // alice KNOWS bob, carol, dave
    tx2.put_edge(
        eid("e-a-bob"),
        obj("person://alice"),
        obj("person://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e-a-carol"),
        obj("person://alice"),
        obj("person://carol"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e-a-dave"),
        obj("person://alice"),
        obj("person://dave"),
        "KNOWS",
        None,
    );
    // bob HAS_TAG rust, databases
    tx2.put_edge(
        eid("e-bob-rust"),
        obj("person://bob"),
        obj("tag://rust"),
        "HAS_TAG",
        None,
    );
    tx2.put_edge(
        eid("e-bob-db"),
        obj("person://bob"),
        obj("tag://databases"),
        "HAS_TAG",
        None,
    );
    // dave HAS_TAG rust
    tx2.put_edge(
        eid("e-dave-rust"),
        obj("person://dave"),
        obj("tag://rust"),
        "HAS_TAG",
        None,
    );
    // carol has no HAS_TAG edges
    store.commit(tx2, "test", "edges").unwrap();
}

#[test]
fn optional_match_no_match_emits_null_row() {
    // carol has no HAS_TAG edges; OPTIONAL MATCH must still emit carol with optional=None.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "one mandatory result (carol), no expansion");
    assert_eq!(rows[0].object_id.0, "person://carol");
    assert!(
        rows[0].optional.is_none(),
        "carol has no tags → optional=None"
    );

    store.close().unwrap();
}

#[test]
fn optional_match_single_c_produces_one_row() {
    // dave has exactly one tag: rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://dave' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "dave has one tag → one expanded row");
    assert_eq!(rows[0].object_id.0, "person://dave");
    let opt = rows[0].optional.as_ref().expect("optional must be Some");
    assert_eq!(opt.object_id.0, "tag://rust");
    assert_eq!(opt.depth, 1);

    store.close().unwrap();
}

#[test]
fn optional_match_multiple_c_expands_rows() {
    // bob has two tags: rust + databases → two output rows for bob
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 2, "bob has two tags → two rows");
    // Both rows have the same b (bob)
    assert!(rows.iter().all(|r| r.object_id.0 == "person://bob"));
    // Both rows have a non-None optional
    assert!(rows.iter().all(|r| r.optional.is_some()));

    let c_ids: Vec<&str> = rows
        .iter()
        .map(|r| r.optional.as_ref().unwrap().object_id.0.as_str())
        .collect();
    assert!(c_ids.contains(&"tag://rust"), "expected rust tag");
    assert!(c_ids.contains(&"tag://databases"), "expected databases tag");

    store.close().unwrap();
}

#[test]
fn optional_match_mixed_mandatory_results() {
    // alice KNOWS: bob (2 tags), carol (0 tags), dave (1 tag)
    // → total rows: 2 + 1 + 1 = 4 (carol emits one null-padded row)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    let bob_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://bob")
        .collect();
    let carol_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://carol")
        .collect();
    let dave_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://dave")
        .collect();

    assert_eq!(bob_rows.len(), 2, "bob has 2 tags");
    assert_eq!(carol_rows.len(), 1, "carol has 0 tags → one null row");
    assert_eq!(dave_rows.len(), 1, "dave has 1 tag");

    assert!(
        carol_rows[0].optional.is_none(),
        "carol optional must be None"
    );
    assert!(bob_rows.iter().all(|r| r.optional.is_some()));
    assert!(dave_rows[0].optional.is_some());

    store.close().unwrap();
}

#[test]
fn optional_match_c_payload_predicate_filters() {
    // bob has rust (weight=0.9) and databases (weight=0.4)
    // WHERE c.weight > 0.5 should keep only rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE c.weight > 0.5 \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // Only rust matches c.weight > 0.5; databases doesn't.
    // But bob IS matched by mandatory → bob is always emitted.
    // Since rust matches optional: one row with optional=Some(rust).
    // databases doesn't match optional: that sub-result is filtered out.
    // Result: one row (bob + rust) — databases sub-match excluded.
    assert_eq!(rows.len(), 1, "only rust passes weight>0.5 filter");
    assert_eq!(rows[0].object_id.0, "person://bob");
    let opt = rows[0].optional.as_ref().expect("rust must match");
    assert_eq!(opt.object_id.0, "tag://rust");

    store.close().unwrap();
}

#[test]
fn optional_match_c_payload_pred_excludes_all_leaves_null_row() {
    // carol has no tags, so OPTIONAL MATCH with a predicate that would never match
    // should still emit carol with optional=None
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE c.weight > 0.99 \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "person://carol");
    assert!(rows[0].optional.is_none());

    store.close().unwrap();
}

#[test]
fn optional_match_field_projection_b_and_c() {
    // Project b.name and c.label
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://dave' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Dave")
    );
    let opt = rows[0].optional.as_ref().expect("dave has rust tag");
    assert_eq!(
        opt.fields.get("label").and_then(|v| v.as_str()),
        Some("rust")
    );

    store.close().unwrap();
}

#[test]
fn optional_match_c_field_projection_absent_for_null_row() {
    // carol has no tags → optional=None → c fields inaccessible, no panic
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Carol")
    );
    assert!(
        rows[0].optional.is_none(),
        "no tags → null optional, no field panic"
    );

    store.close().unwrap();
}

#[test]
fn optional_match_c_id_predicate_filters_by_prefix() {
    // bob has rust + databases; WHERE id(c) STARTS WITH 'tag://r' should keep only rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://r' \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "only rust starts with tag://r");
    assert_eq!(rows[0].optional.as_ref().unwrap().object_id.0, "tag://rust");

    store.close().unwrap();
}

#[test]
fn optional_match_c_id_and_payload_pred_combined() {
    // WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5 \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // rust passes both; databases fails weight>0.5; one row with rust
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]
            .optional
            .as_ref()
            .unwrap()
            .fields
            .get("label")
            .and_then(|v| v.as_str()),
        Some("rust")
    );

    store.close().unwrap();
}

#[test]
fn optional_match_with_order_by_and_limit_on_mandatory() {
    // ORDER BY b.score DESC LIMIT 2 → dave(20), bob(10); then expand with optional
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label \
         ORDER BY b.score DESC LIMIT 2",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // LIMIT 2 mandatory results: dave(score=20), bob(score=10)
    // dave has 1 tag → 1 row; bob has 2 tags → 2 rows; total 3 rows
    assert_eq!(rows.len(), 3, "dave×1 + bob×2");

    // First row should be dave (highest score, first after ORDER BY DESC)
    // Dave has one tag (rust)
    let dave_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.fields.get("name").and_then(|v| v.as_str()) == Some("Dave"))
        .collect();
    let bob_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.fields.get("name").and_then(|v| v.as_str()) == Some("Bob"))
        .collect();

    assert_eq!(dave_rows.len(), 1);
    assert_eq!(bob_rows.len(), 2);

    store.close().unwrap();
}

#[test]
fn optional_match_parse_captures_clause() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..2]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5 \
         RETURN b.name, c.label",
    )
    .unwrap();

    let opt = q
        .optional_clause
        .as_ref()
        .expect("optional clause must be parsed");
    assert_eq!(opt.edge_types, vec!["HAS_TAG"]);
    assert_eq!(opt.min_depth, 1);
    assert_eq!(opt.max_depth, 1);
    assert!(matches!(opt.end_pred, bindb::NodeIdPred::StartsWith(ref s) if s == "tag://"));
    assert_eq!(opt.payload_preds.len(), 1);
    assert_eq!(opt.payload_preds[0].field, "weight");
    assert_eq!(opt.return_fields, vec!["label"]);
    assert_eq!(q.return_fields, vec!["name"]);
}

#[test]
fn optional_match_query_without_optional_clause_unchanged() {
    // Existing non-optional queries must still work identically — no regression
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q =
        GraphIrQuery::parse("MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' RETURN b")
            .unwrap();
    assert!(q.optional_clause.is_none());

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3, "alice KNOWS 3 people");
    assert!(
        rows.iter().all(|r| r.optional.is_none()),
        "no optional clause → all None"
    );

    store.close().unwrap();
}

// ── WITH clause tests (issue #2) ──────────────────────────────────────────────
//
// Graph: alice -[KNOWS]-> bob (score:10), carol (score:5), dave (score:20)
//        bob -[HAS_TAG]-> tag://rust (weight:0.9), tag://databases (weight:0.4)
//        dave -[HAS_TAG]-> tag://rust
//        carol has no HAS_TAG edges

#[test]
fn with_no_where_passes_all_mandatory_through() {
    // WITH b with no predicate or ordering is a no-op; all mandatory results pass through.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3, "all three KNOWS targets pass through");
    store.close().unwrap();
}

#[test]
fn with_where_payload_pred_filters_mandatory() {
    // WITH b WHERE b.score > 8 keeps only bob (10) and dave (20); carol (5) is excluded.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE b.score > 8 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(names.contains(&"Bob"), "bob (score:10) must pass");
    assert!(names.contains(&"Dave"), "dave (score:20) must pass");
    assert!(
        !names.contains(&"Carol"),
        "carol (score:5) must be excluded"
    );
    store.close().unwrap();
}

#[test]
fn with_where_id_pred_filters_by_exact_id() {
    // WITH b WHERE id(b) = 'person://dave' keeps only dave.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE id(b) = 'person://dave' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Dave")
    );
    store.close().unwrap();
}

#[test]
fn with_order_by_desc_sorts_mandatory_results() {
    // WITH b ORDER BY b.score DESC sorts highest-score first (dave=20, bob=10, carol=5).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3);
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert_eq!(names[0], "Dave", "highest score first");
    assert_eq!(names[1], "Bob");
    store.close().unwrap();
}

#[test]
fn with_limit_caps_mandatory_set() {
    // WITH b LIMIT 2 emits at most 2 rows regardless of match count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b LIMIT 2 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "WITH LIMIT 2 caps at 2");
    store.close().unwrap();
}

#[test]
fn with_skip_offsets_mandatory_results() {
    // WITH b ORDER BY b.score ASC SKIP 1 skips carol (lowest score=5).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score ASC SKIP 1 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "skip 1 of 3 → 2 remaining");
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(!names.contains(&"Carol"), "carol (lowest) was skipped");
    store.close().unwrap();
}

#[test]
fn with_order_limit_then_optional_expands_only_top_n() {
    // WITH b ORDER BY b.score DESC LIMIT 2 → top-2 are dave (20) and bob (10).
    // OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c):
    //   dave has 1 tag (rust), bob has 2 tags (rust, databases) → 3 expanded rows.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC LIMIT 2 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label",
            )
            .unwrap(),
        )
        .unwrap();

    let b_names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !b_names.contains(&"Carol"),
        "carol was excluded by WITH LIMIT 2"
    );
    // dave has 1 tag, bob has 2 → 3 rows total
    assert_eq!(rows.len(), 3, "dave(1 tag) + bob(2 tags) = 3 rows");
    store.close().unwrap();
}

#[test]
fn with_where_combined_with_optional_and_c_pred() {
    // WITH b WHERE b.score >= 10: keeps bob (10) and dave (20).
    // OPTIONAL MATCH WHERE c.weight > 0.5: keeps tag://rust (0.9), excludes databases (0.4).
    // bob has rust → 1 row; dave has rust → 1 row.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE b.score >= 10 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) WHERE c.weight > 0.5 \
                 RETURN b.name, c.label",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "bob→rust + dave→rust; databases excluded");
    for row in &rows {
        assert_eq!(
            row.optional
                .as_ref()
                .and_then(|o| o.fields.get("label"))
                .and_then(|v| v.as_str()),
            Some("rust"),
            "only the rust tag passes the weight > 0.5 predicate"
        );
    }
    store.close().unwrap();
}

#[test]
fn with_return_order_by_sorts_post_expansion() {
    // WITH b ORDER BY b.score DESC LIMIT 2 → dave, bob (pre-expansion)
    // OPTIONAL MATCH → dave→rust, bob→rust, bob→databases (3 expanded rows)
    // RETURN ORDER BY id(b) ASC → sort by b.object_id lexicographically:
    //   person://bob < person://dave, so bob's 2 rows come first.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC LIMIT 2 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label ORDER BY id(b) ASC",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3);
    // First two rows must be bob (id "person://bob" < "person://dave").
    assert_eq!(rows[0].object_id.0, "person://bob");
    assert_eq!(rows[1].object_id.0, "person://bob");
    assert_eq!(rows[2].object_id.0, "person://dave");
    store.close().unwrap();
}

#[test]
fn with_parse_captures_clause_fields() {
    // Parser test: with_clause populated with WHERE pred, ORDER BY DESC, SKIP, LIMIT.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE b.score > 5 ORDER BY b.score DESC SKIP 1 LIMIT 3 \
         RETURN b.name",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().expect("with_clause must be Some");
    assert_eq!(wc.payload_preds.len(), 1);
    assert_eq!(wc.payload_preds[0].field, "score");
    assert_eq!(wc.payload_preds[0].op, CmpOp::Gt);
    assert_eq!(wc.payload_preds[0].value, CmpValue::Number(5.0));
    assert_eq!(wc.order_by.len(), 1);
    assert_eq!(wc.order_by[0].direction, OrderDirection::Desc);
    assert_eq!(wc.skip, Some(1));
    assert_eq!(wc.limit, Some(3));
    assert!(q.optional_clause.is_none());
}

#[test]
fn with_parse_id_pred_in_where() {
    // WITH b WHERE id(b) STARTS WITH 'person://d' populates wc.end_pred.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE id(b) STARTS WITH 'person://d' RETURN b",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.end_pred, NodeIdPred::StartsWith("person://d".into()));
    assert!(wc.payload_preds.is_empty());
    assert!(wc.order_by.is_empty());
}

#[test]
fn without_with_optional_match_order_by_still_pre_expansion() {
    // Backward compat: when no WITH clause, ORDER BY/LIMIT applies to mandatory
    // results before optional expansion.  LIMIT 1 with ORDER BY b.score DESC
    // keeps only dave (score=20); OPTIONAL MATCH gives dave→rust (1 row).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label ORDER BY b.score DESC LIMIT 1",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(
        rows.len(),
        1,
        "LIMIT 1 pre-expansion → 1 mandatory → 1 tag row"
    );
    assert_eq!(rows[0].object_id.0, "person://dave");
    store.close().unwrap();
}

// ── WITH aggregation (issue #2) ───────────────────────────────────────────────

#[test]
fn agg_parse_count_single() {
    // Parser: WITH count(b) AS total → one WithAggregation, func=Count, field=None.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH count(b) AS total RETURN total",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().expect("with_clause must be Some");
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(
        wc.aggregations[0],
        WithAggregation {
            func: AggFunc::Count,
            field: None,
            alias: "total".into(),
        }
    );
}

#[test]
fn agg_parse_multi_aggregations() {
    // Parser: WITH count(b) AS cnt, sum(b.score) AS s, avg(b.score) AS a.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH count(b) AS cnt, sum(b.score) AS s, avg(b.score) AS a RETURN cnt",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.aggregations.len(), 3);
    assert_eq!(wc.aggregations[0].func, AggFunc::Count);
    assert_eq!(wc.aggregations[0].alias, "cnt");
    assert_eq!(wc.aggregations[1].func, AggFunc::Sum);
    assert_eq!(wc.aggregations[1].field.as_deref(), Some("score"));
    assert_eq!(wc.aggregations[1].alias, "s");
    assert_eq!(wc.aggregations[2].func, AggFunc::Avg);
    assert_eq!(wc.aggregations[2].alias, "a");
}

#[test]
fn agg_parse_min_max() {
    // Parser: WITH min(b.score) AS lo, max(b.score) AS hi.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH min(b.score) AS lo, max(b.score) AS hi RETURN lo, hi",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.aggregations.len(), 2);
    assert_eq!(wc.aggregations[0].func, AggFunc::Min);
    assert_eq!(wc.aggregations[0].field.as_deref(), Some("score"));
    assert_eq!(wc.aggregations[1].func, AggFunc::Max);
    assert_eq!(wc.aggregations[1].field.as_deref(), Some("score"));
}

#[test]
fn agg_parse_existing_with_still_works() {
    // Regression: pass-through WITH b still parses correctly after aggregation support added.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE b.score > 5 ORDER BY b.score DESC LIMIT 2 RETURN b.name",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert!(
        wc.aggregations.is_empty(),
        "pass-through WITH has no aggregations"
    );
    assert_eq!(wc.payload_preds.len(), 1);
    assert_eq!(wc.limit, Some(2));
}

#[test]
fn agg_count_returns_total_matched() {
    // count(b) returns the number of matched b nodes.
    // build_optional_graph: alice KNOWS bob, carol, dave → 3 matches.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH count(b) AS total RETURN total",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("total").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn agg_count_zero_when_no_match() {
    // count(b) = 0 when the MATCH predicate filters everything out.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'nobody://' \
             WITH count(b) AS n RETURN n",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn agg_sum_numeric_field() {
    // bob score=10, carol score=5, dave score=20 → sum = 35.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH sum(b.score) AS total_score RETURN total_score",
        )
        .unwrap();
    let row = &result[0];
    let v = row
        .get("total_score")
        .and_then(|v| v.as_f64())
        .expect("total_score must be a number");
    assert!((v - 35.0).abs() < 1e-9, "expected 35.0, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_avg_numeric_field() {
    // (10 + 5 + 20) / 3 ≈ 11.667.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH avg(b.score) AS avg_score RETURN avg_score",
        )
        .unwrap();
    let row = &result[0];
    let v = row
        .get("avg_score")
        .and_then(|v| v.as_f64())
        .expect("avg_score must be a number");
    assert!((v - 35.0 / 3.0).abs() < 1e-9, "expected ≈11.667, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_min_max_numeric_field() {
    // min(b.score)=5 (carol), max(b.score)=20 (dave).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH min(b.score) AS lo, max(b.score) AS hi RETURN lo, hi",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("lo").and_then(|v| v.as_f64()), Some(5.0));
    assert_eq!(row.get("hi").and_then(|v| v.as_f64()), Some(20.0));
    store.close().unwrap();
}

#[test]
fn agg_multiple_in_one_query() {
    // count + sum + min + max in a single call.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH count(b) AS n, sum(b.score) AS s, min(b.score) AS lo, max(b.score) AS hi \
             RETURN n, s, lo, hi",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(3));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 35.0).abs() < 1e-9);
    assert_eq!(row.get("lo").and_then(|v| v.as_f64()), Some(5.0));
    assert_eq!(row.get("hi").and_then(|v| v.as_f64()), Some(20.0));
    store.close().unwrap();
}

#[test]
fn agg_with_pre_match_filter() {
    // WHERE b.score >= 10 in MATCH filters before aggregation: bob (10) + dave (20) → 2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' AND b.score >= 10 \
             WITH count(b) AS n, sum(b.score) AS s RETURN n, s",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 30.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_missing_field_excluded_from_sum_avg() {
    // Nodes without the aggregated field are skipped (not treated as 0).
    // hub → node_a (score=10), node_b (no score field)
    // sum(b.score) = 10, avg = 10.0, count = 2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://h"), b"{}".to_vec());
    tx.put_object(obj("n://a"), br#"{"score":10}"#.to_vec());
    tx.put_object(obj("n://b"), br#"{"label":"no-score"}"#.to_vec());
    tx.put_edge(eid("ea"), obj("hub://h"), obj("n://a"), "LINKS", None);
    tx.put_edge(eid("eb"), obj("hub://h"), obj("n://b"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'hub://h' \
             WITH count(b) AS n, sum(b.score) AS s, avg(b.score) AS a \
             RETURN n, s, a",
        )
        .unwrap();

    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 10.0).abs() < 1e-9);
    assert!((row.get("a").and_then(|v| v.as_f64()).unwrap() - 10.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_avg_min_max_null_when_no_valid_field() {
    // avg/min/max return JSON null when no nodes have the requested field.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://h"), b"{}".to_vec());
    tx.put_object(obj("n://x"), br#"{"label":"x"}"#.to_vec());
    tx.put_edge(eid("e1"), obj("hub://h"), obj("n://x"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'hub://h' \
             WITH avg(b.score) AS a, min(b.score) AS lo, max(b.score) AS hi \
             RETURN a, lo, hi",
        )
        .unwrap();

    let row = &result[0];
    assert!(
        row.get("a").map(|v| v.is_null()).unwrap_or(false),
        "avg of missing field is null"
    );
    assert!(
        row.get("lo").map(|v| v.is_null()).unwrap_or(false),
        "min of missing field is null"
    );
    assert!(
        row.get("hi").map(|v| v.is_null()).unwrap_or(false),
        "max of missing field is null"
    );
    store.close().unwrap();
}

#[test]
fn agg_no_aggregating_with_returns_error() {
    // Calling graph_ir_aggregated on a non-aggregating query is an error.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let err = store
        .graph_ir_aggregated("MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' RETURN b")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("aggregat"),
        "error should mention aggregation, got: {msg}"
    );
    store.close().unwrap();
}

// ── GROUP BY aggregation (issue #2) ──────────────────────────────────────────

#[test]
fn agg_grouped_parse_captures_group_by_field() {
    // Parser: WITH count(b) AS cnt GROUP BY b.type → group_by_fields = ["type"].
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH count(b) AS cnt GROUP BY b.type RETURN b.type, cnt",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["type"]);
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(wc.aggregations[0].func, AggFunc::Count);
}

#[test]
fn agg_grouped_parse_multi_agg_with_group_by() {
    // count + sum with GROUP BY in one clause.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH count(b) AS n, sum(b.score) AS s GROUP BY b.category RETURN b.category, n, s",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["category"]);
    assert_eq!(wc.aggregations.len(), 2);
}

fn build_grouped_graph(store: &BinderStore) {
    // hub → 5 nodes across 3 categories:
    //   animal: cat (score=2), dog (score=4)
    //   plant:  rose (score=6), fern (score=8)
    //   other:  rock (score=1)
    let mut tx = Transaction::new();
    tx.put_object(obj("h://root"), b"{}".to_vec());
    tx.put_object(obj("n://cat"), br#"{"type":"animal","score":2}"#.to_vec());
    tx.put_object(obj("n://dog"), br#"{"type":"animal","score":4}"#.to_vec());
    tx.put_object(obj("n://rose"), br#"{"type":"plant","score":6}"#.to_vec());
    tx.put_object(obj("n://fern"), br#"{"type":"plant","score":8}"#.to_vec());
    tx.put_object(obj("n://rock"), br#"{"type":"other","score":1}"#.to_vec());
    tx.put_edge(eid("e1"), obj("h://root"), obj("n://cat"), "LINKS", None);
    tx.put_edge(eid("e2"), obj("h://root"), obj("n://dog"), "LINKS", None);
    tx.put_edge(eid("e3"), obj("h://root"), obj("n://rose"), "LINKS", None);
    tx.put_edge(eid("e4"), obj("h://root"), obj("n://fern"), "LINKS", None);
    tx.put_edge(eid("e5"), obj("h://root"), obj("n://rock"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();
}

#[test]
fn agg_grouped_count_per_category() {
    // count(b) GROUP BY b.type → animal:2, plant:2, other:1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n GROUP BY b.type RETURN b.type, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 3, "three distinct type values");
    let find = |t: &str| {
        rows.iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some(t))
            .expect(t)
    };
    assert_eq!(find("animal").get("n").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(find("plant").get("n").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(find("other").get("n").and_then(|v| v.as_u64()), Some(1));
    store.close().unwrap();
}

#[test]
fn agg_grouped_sum_per_category() {
    // sum(b.score) GROUP BY b.type → animal:6, plant:14, other:1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH sum(b.score) AS s GROUP BY b.type RETURN b.type, s",
        )
        .unwrap();

    let find = |t: &str| {
        rows.iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some(t))
            .expect(t)
    };
    assert!((find("animal").get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    assert!((find("plant").get("s").and_then(|v| v.as_f64()).unwrap() - 14.0).abs() < 1e-9);
    assert!((find("other").get("s").and_then(|v| v.as_f64()).unwrap() - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_grouped_multi_agg_per_category() {
    // count + sum + avg per group, all in one query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n, sum(b.score) AS s, avg(b.score) AS a GROUP BY b.type \
             RETURN b.type, n, s, a",
        )
        .unwrap();

    let animal = rows
        .iter()
        .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("animal"))
        .unwrap();
    assert_eq!(animal.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((animal.get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    assert!((animal.get("a").and_then(|v| v.as_f64()).unwrap() - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_grouped_null_group_for_missing_field() {
    // Nodes that lack the GROUP BY field go into a null group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("h://root"), b"{}".to_vec());
    tx.put_object(obj("n://a"), br#"{"type":"X","score":1}"#.to_vec());
    tx.put_object(obj("n://b"), br#"{"score":2}"#.to_vec()); // no type field
    tx.put_edge(eid("e1"), obj("h://root"), obj("n://a"), "LINKS", None);
    tx.put_edge(eid("e2"), obj("h://root"), obj("n://b"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n GROUP BY b.type RETURN b.type, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "X group + null group");
    let null_row = rows
        .iter()
        .find(|r| r.get("type").map(|v| v.is_null()).unwrap_or(false));
    assert!(
        null_row.is_some(),
        "null group exists for nodes without type field"
    );
    assert_eq!(null_row.unwrap().get("n").and_then(|v| v.as_u64()), Some(1));
    store.close().unwrap();
}

#[test]
fn agg_ungrouped_returns_single_row_vec() {
    // Without GROUP BY, result is always a 1-element Vec.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n RETURN n",
        )
        .unwrap();

    assert_eq!(rows.len(), 1, "ungrouped always returns exactly one row");
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

// ── Multi-key GROUP BY (issue #2) ────────────────────────────────────────────

fn build_multikey_graph(store: &BinderStore) {
    // hub → 6 nodes, two grouping fields: type + status
    //   (animal, active): cat (score=2), dog (score=4)
    //   (animal, retired): wolf (score=3)
    //   (plant, active): rose (score=6)
    //   (plant, retired): fern (score=8)
    //   (null, active): rock (score=1) — missing type field
    let mut tx = Transaction::new();
    tx.put_object(obj("h://mk_root"), b"{}".to_vec());
    tx.put_object(
        obj("n://mk/cat"),
        br#"{"type":"animal","status":"active","score":2}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/dog"),
        br#"{"type":"animal","status":"active","score":4}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/wolf"),
        br#"{"type":"animal","status":"retired","score":3}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/rose"),
        br#"{"type":"plant","status":"active","score":6}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/fern"),
        br#"{"type":"plant","status":"retired","score":8}"#.to_vec(),
    );
    tx.put_object(
        obj("n://mk/rock"),
        br#"{"status":"active","score":1}"#.to_vec(),
    );
    tx.put_edge(
        eid("ek1"),
        obj("h://mk_root"),
        obj("n://mk/cat"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek2"),
        obj("h://mk_root"),
        obj("n://mk/dog"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek3"),
        obj("h://mk_root"),
        obj("n://mk/wolf"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek4"),
        obj("h://mk_root"),
        obj("n://mk/rose"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek5"),
        obj("h://mk_root"),
        obj("n://mk/fern"),
        "MK",
        None,
    );
    tx.put_edge(
        eid("ek6"),
        obj("h://mk_root"),
        obj("n://mk/rock"),
        "MK",
        None,
    );
    store.commit(tx, "test", "").unwrap();
}

#[test]
fn agg_multikey_parse_two_fields() {
    // Parser: WITH count(b) AS cnt GROUP BY b.type, b.status → group_by_fields = ["type","status"].
    use bindb::GraphIrQuery;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
         WITH count(b) AS cnt GROUP BY b.type, b.status RETURN b.type, b.status, cnt",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["type", "status"]);
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(wc.aggregations[0].alias, "cnt");
}

#[test]
fn agg_multikey_count_per_type_status() {
    // count(b) GROUP BY b.type, b.status should yield 5 groups.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH count(b) AS n GROUP BY b.type, b.status \
             RETURN b.type, b.status, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 5, "5 distinct (type,status) combinations");

    let find = |t: Option<&str>, s: &str| {
        rows.iter().find(|r| {
            r.get("type").and_then(|v| v.as_str()) == t
                && r.get("status").and_then(|v| v.as_str()) == Some(s)
        })
    };
    assert_eq!(
        find(Some("animal"), "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        find(Some("animal"), "retired")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        find(Some("plant"), "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        find(Some("plant"), "retired")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    // rock has no type → null group
    assert_eq!(
        find(None, "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    store.close().unwrap();
}

#[test]
fn agg_multikey_sum_per_type_status() {
    // sum(b.score) GROUP BY b.type, b.status.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH sum(b.score) AS s GROUP BY b.type, b.status \
             RETURN b.type, b.status, s",
        )
        .unwrap();

    let get_sum = |t: Option<&str>, st: &str| -> Option<f64> {
        rows.iter()
            .find(|r| {
                r.get("type").and_then(|v| v.as_str()) == t
                    && r.get("status").and_then(|v| v.as_str()) == Some(st)
            })
            .and_then(|r| r.get("s"))
            .and_then(|v| v.as_f64())
    };
    assert!(
        (get_sum(Some("animal"), "active").unwrap() - 6.0).abs() < 1e-9,
        "cat+dog"
    );
    assert!(
        (get_sum(Some("animal"), "retired").unwrap() - 3.0).abs() < 1e-9,
        "wolf"
    );
    assert!(
        (get_sum(Some("plant"), "active").unwrap() - 6.0).abs() < 1e-9,
        "rose"
    );
    assert!(
        (get_sum(Some("plant"), "retired").unwrap() - 8.0).abs() < 1e-9,
        "fern"
    );
    assert!(
        (get_sum(None, "active").unwrap() - 1.0).abs() < 1e-9,
        "rock"
    );
    store.close().unwrap();
}

#[test]
fn agg_multikey_multi_agg_same_query() {
    // count + sum in the same multi-key GROUP BY query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH count(b) AS n, sum(b.score) AS s GROUP BY b.type, b.status \
             RETURN b.type, b.status, n, s",
        )
        .unwrap();

    let animal_active = rows
        .iter()
        .find(|r| {
            r.get("type").and_then(|v| v.as_str()) == Some("animal")
                && r.get("status").and_then(|v| v.as_str()) == Some("active")
        })
        .expect("animal/active group");
    assert_eq!(animal_active.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((animal_active.get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_multikey_both_fields_null_group() {
    // Nodes missing both GROUP BY fields form a (null, null) group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("h://mk2"), b"{}".to_vec());
    tx.put_object(
        obj("n://mk2/a"),
        br#"{"type":"X","status":"Y","score":10}"#.to_vec(),
    );
    tx.put_object(obj("n://mk2/b"), br#"{"score":5}"#.to_vec()); // missing both
    tx.put_edge(eid("ek_a"), obj("h://mk2"), obj("n://mk2/a"), "MK2", None);
    tx.put_edge(eid("ek_b"), obj("h://mk2"), obj("n://mk2/b"), "MK2", None);
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK2]->(b) WHERE id(a) = 'h://mk2' \
             WITH count(b) AS n GROUP BY b.type, b.status \
             RETURN b.type, b.status, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "(X,Y) + (null,null)");
    let null_group = rows
        .iter()
        .find(|r| r.get("type").map(|v| v.is_null()).unwrap_or(false))
        .expect("null,null group");
    assert_eq!(null_group.get("n").and_then(|v| v.as_u64()), Some(1));
    assert!(null_group
        .get("status")
        .map(|v| v.is_null())
        .unwrap_or(false));
    store.close().unwrap();
}

// ── Edge payload predicates / AND r.field OP value (issue #2) ────────────────

fn build_weighted_graph(store: &BinderStore) {
    // root → 4 nodes via edges that carry a JSON payload with a "weight" field.
    //   edge to n://strong1: {"weight": 0.9, "kind": "strong"}
    //   edge to n://strong2: {"weight": 0.8, "kind": "strong"}
    //   edge to n://weak1:   {"weight": 0.3, "kind": "weak"}
    //   edge to n://untyped: no payload (None)
    let mut tx = Transaction::new();
    tx.put_object(obj("h://wroot"), b"{}".to_vec());
    tx.put_object(obj("n://strong1"), br#"{"label":"s1"}"#.to_vec());
    tx.put_object(obj("n://strong2"), br#"{"label":"s2"}"#.to_vec());
    tx.put_object(obj("n://weak1"), br#"{"label":"w1"}"#.to_vec());
    tx.put_object(obj("n://untyped"), br#"{"label":"u1"}"#.to_vec());
    tx.put_edge(
        eid("we1"),
        obj("h://wroot"),
        obj("n://strong1"),
        "REL",
        Some(br#"{"weight":0.9,"kind":"strong"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we2"),
        obj("h://wroot"),
        obj("n://strong2"),
        "REL",
        Some(br#"{"weight":0.8,"kind":"strong"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we3"),
        obj("h://wroot"),
        obj("n://weak1"),
        "REL",
        Some(br#"{"weight":0.3,"kind":"weak"}"#.to_vec()),
    );
    tx.put_edge(
        eid("we4"),
        obj("h://wroot"),
        obj("n://untyped"),
        "REL",
        None,
    );
    store.commit(tx, "test", "").unwrap();
}

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

// ── string predicate tests (STARTS WITH / ENDS WITH / CONTAINS) ──────────────

fn build_string_pred_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // Nodes with string payload fields
    tx.put_object(
        obj("sp://alice"),
        br#"{"name":"Alice Smith","role":"admin","tag":"alpha-user"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://bob"),
        br#"{"name":"Bobby Jones","role":"editor","tag":"beta-user"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://carol"),
        br#"{"name":"Carol White","role":"admin","tag":"alpha-mod"}"#.to_vec(),
    );
    tx.put_object(
        obj("sp://dave"),
        br#"{"name":"Dave Brown","role":"viewer","tag":"gamma"}"#.to_vec(),
    );
    tx.put_object(obj("sp://root"), br#"{"name":"Root"}"#.to_vec());
    // Edges from root to everyone
    tx.put_edge(
        eid("sp_e1"),
        obj("sp://root"),
        obj("sp://alice"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e2"),
        obj("sp://root"),
        obj("sp://bob"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e3"),
        obj("sp://root"),
        obj("sp://carol"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sp_e4"),
        obj("sp://root"),
        obj("sp://dave"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

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

fn build_null_check_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    // "optional_field" present on some nodes, absent on others
    tx.put_object(obj("nc://a"), br#"{"name":"alpha","score":10}"#.to_vec());
    tx.put_object(
        obj("nc://b"),
        br#"{"name":"beta"}"#.to_vec(), // no "score"
    );
    tx.put_object(obj("nc://c"), br#"{"name":"gamma","score":0}"#.to_vec());
    tx.put_object(
        obj("nc://d"),
        br#"{"name":"delta"}"#.to_vec(), // no "score"
    );
    tx.put_object(obj("nc://root"), b"{}".to_vec());
    tx.put_edge(eid("nc_e1"), obj("nc://root"), obj("nc://a"), "LINK", None);
    tx.put_edge(eid("nc_e2"), obj("nc://root"), obj("nc://b"), "LINK", None);
    tx.put_edge(eid("nc_e3"), obj("nc://root"), obj("nc://c"), "LINK", None);
    tx.put_edge(eid("nc_e4"), obj("nc://root"), obj("nc://d"), "LINK", None);
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

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

fn build_in_pred_graph() -> (BinderStore, TempDir) {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("in://alice"),
        br#"{"role":"admin","dept":"eng"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://bob"),
        br#"{"role":"editor","dept":"eng"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://carol"),
        br#"{"role":"viewer","dept":"hr"}"#.to_vec(),
    );
    tx.put_object(
        obj("in://dave"),
        br#"{"role":"admin","dept":"hr"}"#.to_vec(),
    );
    tx.put_object(obj("in://root"), b"{}".to_vec());
    tx.put_edge(
        eid("in_e1"),
        obj("in://root"),
        obj("in://alice"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e2"),
        obj("in://root"),
        obj("in://bob"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e3"),
        obj("in://root"),
        obj("in://carol"),
        "MEMBER",
        None,
    );
    tx.put_edge(
        eid("in_e4"),
        obj("in://root"),
        obj("in://dave"),
        "MEMBER",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

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

// ── RETURN id(b) tests (issue #2) ────────────────────────────────────────────

fn build_return_id_graph() -> (BinderStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rid://root"), b"{}".to_vec());
    tx.put_object(
        obj("rid://alice"),
        br#"{"name":"alice","score":10}"#.to_vec(),
    );
    tx.put_object(obj("rid://bob"), br#"{"name":"bob","score":20}"#.to_vec());
    tx.put_object(
        obj("rid://carol"),
        br#"{"name":"carol","score":15}"#.to_vec(),
    );
    tx.put_edge(
        eid("rid://e1"),
        obj("rid://root"),
        obj("rid://alice"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("rid://e2"),
        obj("rid://root"),
        obj("rid://bob"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("rid://e3"),
        obj("rid://root"),
        obj("rid://carol"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    (store, dir)
}

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

// ── BETWEEN predicate (issue #2) ─────────────────────────────────────────────

fn build_between_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("hub://root"), b"{}".to_vec());
    tx.put_object(obj("item://low"), br#"{"price":5.0}"#.to_vec());
    tx.put_object(obj("item://mid"), br#"{"price":50.0}"#.to_vec());
    tx.put_object(obj("item://high"), br#"{"price":150.0}"#.to_vec());
    tx.put_object(obj("item://no-price"), b"{}".to_vec());
    tx.put_edge(eid("e1"), obj("hub://root"), obj("item://low"), "HAS", None);
    tx.put_edge(eid("e2"), obj("hub://root"), obj("item://mid"), "HAS", None);
    tx.put_edge(
        eid("e3"),
        obj("hub://root"),
        obj("item://high"),
        "HAS",
        None,
    );
    tx.put_edge(
        eid("e4"),
        obj("hub://root"),
        obj("item://no-price"),
        "HAS",
        None,
    );
    store.commit(tx, "test", "seed").unwrap();
}

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

fn build_like_graph(store: &BinderStore) {
    let mut tx = Transaction::new();
    tx.put_object(obj("hub://root"), b"{}".to_vec());
    tx.put_object(
        obj("node://alice"),
        br#"{"name":"alice@corp.com","code":"US01"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://bob"),
        br#"{"name":"bob@corp.com","code":"US02"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://carol"),
        br#"{"name":"carol@other.com","code":"EU01"}"#.to_vec(),
    );
    tx.put_object(
        obj("node://dave"),
        br#"{"name":"dave","code":"EU02"}"#.to_vec(), // no @ — won't match email pattern
    );
    tx.put_edge(
        eid("e1"),
        obj("hub://root"),
        obj("node://alice"),
        "HAS",
        None,
    );
    tx.put_edge(eid("e2"), obj("hub://root"), obj("node://bob"), "HAS", None);
    tx.put_edge(
        eid("e3"),
        obj("hub://root"),
        obj("node://carol"),
        "HAS",
        None,
    );
    tx.put_edge(
        eid("e4"),
        obj("hub://root"),
        obj("node://dave"),
        "HAS",
        None,
    );
    store.commit(tx, "test", "seed").unwrap();
}

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

// ── RETURN UPPER/LOWER/TRIM tests ─────────────────────────────────────────────

#[test]
fn return_upper_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rut://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rut://a"), br#"{"name":"alice"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rut_e1"), obj("rut://hub"), obj("rut://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rut://hub' RETURN UPPER(b.name)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name"),
        Some(&serde_json::Value::String("ALICE".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_lower_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlt://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rlt://a"), br#"{"tag":"HELLO"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rlt_e1"), obj("rlt://hub"), obj("rlt://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlt://hub' RETURN LOWER(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tag"),
        Some(&serde_json::Value::String("hello".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_trim_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rtt://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rtt://a"), br#"{"val":"  hi  "}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rtt_e1"), obj("rtt://hub"), obj("rtt://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rtt://hub' RETURN TRIM(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("val"),
        Some(&serde_json::Value::String("hi".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_transform_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rtns://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rtns://a"), br#"{"score":42}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rtns_e1"),
        obj("rtns://hub"),
        obj("rtns://a"),
        "X",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rtns://hub' RETURN UPPER(b.score)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    // numeric values pass through unchanged
    assert_eq!(
        rows[0].fields.get("score"),
        Some(&serde_json::Value::Number(42.into()))
    );
    store.close().unwrap();
}

// ── WITH DISTINCT b ──────────────────────────────────────────────────────────

#[test]
fn with_distinct_parse_sets_flag() {
    let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH DISTINCT b RETURN b")
        .unwrap();
    assert!(q.with_clause.as_ref().unwrap().distinct);
}

#[test]
fn with_no_distinct_flag_false() {
    let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b RETURN b").unwrap();
    assert!(!q.with_clause.as_ref().unwrap().distinct);
}

#[test]
fn with_distinct_returns_correct_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wdist://hub"), b"{}".to_vec());
    tx.put_object(obj("wdist://a"), br#"{"val":1}"#.to_vec());
    tx.put_object(obj("wdist://b"), br#"{"val":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wdist_e1"),
        obj("wdist://hub"),
        obj("wdist://a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("wdist_e2"),
        obj("wdist://hub"),
        obj("wdist://b"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'wdist://hub' WITH DISTINCT b RETURN b.val",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn with_distinct_then_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wdl://hub"), b"{}".to_vec());
    tx.put_object(obj("wdl://a"), br#"{"n":1}"#.to_vec());
    tx.put_object(obj("wdl://b"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("wdl_e1"), obj("wdl://hub"), obj("wdl://a"), "L", None);
    tx2.put_edge(eid("wdl_e2"), obj("wdl://hub"), obj("wdl://b"), "L", None);
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wdl://hub' WITH DISTINCT b LIMIT 1 RETURN b.n",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    store.close().unwrap();
}

// ── multi-field ORDER BY in RETURN ───────────────────────────────────────────

#[test]
fn multi_order_by_parse_roundtrip() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY b.dept ASC, b.rank DESC",
    )
    .unwrap();
    assert_eq!(q.order_by.len(), 2);
    assert_eq!(
        q.order_by[0].field,
        OrderByField::PayloadField("dept".into())
    );
    assert_eq!(q.order_by[0].direction, OrderDirection::Asc);
    assert_eq!(
        q.order_by[1].field,
        OrderByField::PayloadField("rank".into())
    );
    assert_eq!(q.order_by[1].direction, OrderDirection::Desc);
}

#[test]
fn multi_order_by_sorts_by_two_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob://hub"), b"{}".to_vec());
    tx.put_object(obj("mob://a"), br#"{"dept":"eng","rank":2}"#.to_vec());
    tx.put_object(obj("mob://b"), br#"{"dept":"eng","rank":1}"#.to_vec());
    tx.put_object(obj("mob://c"), br#"{"dept":"art","rank":5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob_e1"),
        obj("mob://hub"),
        obj("mob://a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("mob_e2"),
        obj("mob://hub"),
        obj("mob://b"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("mob_e3"),
        obj("mob://hub"),
        obj("mob://c"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'mob://hub' RETURN b.dept, b.rank ORDER BY b.dept ASC, b.rank ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // art first (dept ASC), then eng sorted by rank ASC
    assert_eq!(rows[0].fields["dept"], serde_json::json!("art"));
    assert_eq!(rows[1].fields["rank"], serde_json::json!(1));
    assert_eq!(rows[2].fields["rank"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn multi_order_by_primary_desc_secondary_asc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob2://hub"), b"{}".to_vec());
    tx.put_object(obj("mob2://a"), br#"{"grp":"x","pos":2}"#.to_vec());
    tx.put_object(obj("mob2://b"), br#"{"grp":"x","pos":1}"#.to_vec());
    tx.put_object(obj("mob2://c"), br#"{"grp":"y","pos":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob2_e1"),
        obj("mob2://hub"),
        obj("mob2://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob2_e2"),
        obj("mob2://hub"),
        obj("mob2://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob2_e3"),
        obj("mob2://hub"),
        obj("mob2://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'mob2://hub' RETURN b.grp, b.pos ORDER BY b.grp DESC, b.pos ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // grp DESC: y first, then x; within x, pos ASC: 1 before 2
    assert_eq!(rows[0].fields["grp"], serde_json::json!("y"));
    assert_eq!(rows[1].fields["pos"], serde_json::json!(1));
    assert_eq!(rows[2].fields["pos"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn multi_order_by_single_field_unchanged() {
    // Single-field ORDER BY still works with the Vec-based API
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob3://hub"), b"{}".to_vec());
    tx.put_object(obj("mob3://a"), br#"{"v":3}"#.to_vec());
    tx.put_object(obj("mob3://b"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("mob3://c"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob3_e1"),
        obj("mob3://hub"),
        obj("mob3://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob3_e2"),
        obj("mob3://hub"),
        obj("mob3://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob3_e3"),
        obj("mob3://hub"),
        obj("mob3://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'mob3://hub' RETURN b.v ORDER BY b.v ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].fields["v"], serde_json::json!(1));
    assert_eq!(rows[1].fields["v"], serde_json::json!(2));
    assert_eq!(rows[2].fields["v"], serde_json::json!(3));
    store.close().unwrap();
}

// ── WITH multi-field ORDER BY ─────────────────────────────────────────────────

#[test]
fn with_multi_order_by_parse_roundtrip() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b ORDER BY b.dept ASC, b.rank DESC RETURN b",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.order_by.len(), 2);
    assert_eq!(
        wc.order_by[0].field,
        OrderByField::PayloadField("dept".into())
    );
    assert_eq!(wc.order_by[0].direction, OrderDirection::Asc);
    assert_eq!(
        wc.order_by[1].field,
        OrderByField::PayloadField("rank".into())
    );
    assert_eq!(wc.order_by[1].direction, OrderDirection::Desc);
}

#[test]
fn with_multi_order_by_sorts_pre_expansion() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wmob://hub"), b"{}".to_vec());
    tx.put_object(obj("wmob://a"), br#"{"grp":"x","pos":2}"#.to_vec());
    tx.put_object(obj("wmob://b"), br#"{"grp":"x","pos":1}"#.to_vec());
    tx.put_object(obj("wmob://c"), br#"{"grp":"y","pos":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wmob_e1"),
        obj("wmob://hub"),
        obj("wmob://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmob_e2"),
        obj("wmob://hub"),
        obj("wmob://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmob_e3"),
        obj("wmob://hub"),
        obj("wmob://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wmob://hub' WITH b ORDER BY b.grp ASC, b.pos ASC RETURN b.grp, b.pos",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // grp ASC: x first, then y; within x, pos ASC: 1 before 2
    assert_eq!(rows[0].fields["grp"], serde_json::json!("x"));
    assert_eq!(rows[0].fields["pos"], serde_json::json!(1));
    assert_eq!(rows[1].fields["pos"], serde_json::json!(2));
    assert_eq!(rows[2].fields["grp"], serde_json::json!("y"));
    store.close().unwrap();
}

#[test]
fn with_multi_order_by_with_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wmobl://hub"), b"{}".to_vec());
    tx.put_object(obj("wmobl://a"), br#"{"tier":1,"rank":3}"#.to_vec());
    tx.put_object(obj("wmobl://b"), br#"{"tier":1,"rank":1}"#.to_vec());
    tx.put_object(obj("wmobl://c"), br#"{"tier":2,"rank":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wmobl_e1"),
        obj("wmobl://hub"),
        obj("wmobl://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmobl_e2"),
        obj("wmobl://hub"),
        obj("wmobl://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmobl_e3"),
        obj("wmobl://hub"),
        obj("wmobl://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wmobl://hub' WITH b ORDER BY b.tier ASC, b.rank ASC LIMIT 2 RETURN b.tier, b.rank",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].fields["tier"], serde_json::json!(1));
    assert_eq!(rows[0].fields["rank"], serde_json::json!(1));
    assert_eq!(rows[1].fields["tier"], serde_json::json!(1));
    assert_eq!(rows[1].fields["rank"], serde_json::json!(3));
    store.close().unwrap();
}

#[test]
fn with_single_order_by_still_works_as_vec() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b ORDER BY b.score DESC RETURN b",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.order_by.len(), 1);
    assert_eq!(wc.order_by[0].direction, OrderDirection::Desc);
}

// ── Numeric RETURN transforms ─────────────────────────────────────────────────

#[test]
fn return_abs_of_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnabs://hub"), b"{}".to_vec());
    tx.put_object(obj("rnabs://a"), br#"{"val":-4.5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnabs_e1"),
        obj("rnabs://hub"),
        obj("rnabs://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnabs://hub' RETURN ABS(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].fields.get("val").and_then(|v| v.as_f64());
    assert!((v.unwrap() - 4.5).abs() < f64::EPSILON);
    store.close().unwrap();
}

#[test]
fn return_round_of_decimal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnrd://hub"), b"{}".to_vec());
    tx.put_object(obj("rnrd://a"), br#"{"val":3.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnrd_e1"),
        obj("rnrd://hub"),
        obj("rnrd://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnrd://hub' RETURN ROUND(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(4.0));
    store.close().unwrap();
}

#[test]
fn return_ceil_and_floor() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rncf://hub"), b"{}".to_vec());
    tx.put_object(obj("rncf://a"), br#"{"hi":2.1,"lo":2.9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rncf_e1"),
        obj("rncf://hub"),
        obj("rncf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let ceil_rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rncf://hub' RETURN CEIL(b.hi)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(ceil_rows[0].fields["hi"], serde_json::json!(3.0));

    let floor_rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rncf://hub' RETURN FLOOR(b.lo)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(floor_rows[0].fields["lo"], serde_json::json!(2.0));
    store.close().unwrap();
}

#[test]
fn return_numeric_transform_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rntsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rntsp://a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rntsp_e1"),
        obj("rntsp://hub"),
        obj("rntsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    // ABS on a string should pass the value through unchanged
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rntsp://hub' RETURN ABS(b.name)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!("Alice"));
    store.close().unwrap();
}

// ── RETURN COALESCE / RETURN REPLACE tests ────────────────────────────────────

#[test]
fn return_coalesce_present_field_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcpf://hub"), b"{}".to_vec());
    tx.put_object(obj("rcpf://a"), br#"{"label":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcpf_e1"),
        obj("rcpf://hub"),
        obj("rcpf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcpf://hub' RETURN COALESCE(b.label, 'default')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["label"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_coalesce_missing_field_uses_fallback() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcmf://hub"), b"{}".to_vec());
    tx.put_object(obj("rcmf://a"), br#"{"other":"x"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcmf_e1"),
        obj("rcmf://hub"),
        obj("rcmf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcmf://hub' RETURN COALESCE(b.label, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["label"], serde_json::json!("N/A"));
    store.close().unwrap();
}

#[test]
fn return_coalesce_parses_transform_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN COALESCE(b.name, 'unknown')",
    )
    .unwrap();
    assert_eq!(q.return_field_transforms.len(), 1);
    assert!(matches!(
        q.return_field_transforms.get("name"),
        Some(ReturnTransform::Coalesce(s)) if s == "unknown"
    ));
}

#[test]
fn return_replace_substitutes_substring() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrss://hub"), b"{}".to_vec());
    tx.put_object(obj("rrss://a"), br#"{"tag":"hello-world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrss_e1"),
        obj("rrss://hub"),
        obj("rrss://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrss://hub' RETURN REPLACE(b.tag, '-', '_')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hello_world"));
    store.close().unwrap();
}

#[test]
fn return_replace_no_match_returns_original() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrnm://hub"), b"{}".to_vec());
    tx.put_object(obj("rrnm://a"), br#"{"val":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrnm_e1"),
        obj("rrnm://hub"),
        obj("rrnm://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrnm://hub' RETURN REPLACE(b.val, 'x', 'y')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_replace_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrnsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rrnsp://a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrnsp_e1"),
        obj("rrnsp://hub"),
        obj("rrnsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrnsp://hub' RETURN REPLACE(b.n, '4', '5')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(42));
    store.close().unwrap();
}

#[test]
fn return_replace_parses_transform_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REPLACE(b.text, 'foo', 'bar')",
    )
    .unwrap();
    assert_eq!(q.return_field_transforms.len(), 1);
    assert!(matches!(
        q.return_field_transforms.get("text"),
        Some(ReturnTransform::Replace(f, t)) if f == "foo" && t == "bar"
    ));
}

#[test]
fn return_replace_all_occurrences() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrao://hub"), b"{}".to_vec());
    tx.put_object(obj("rrao://a"), br#"{"msg":"a-b-c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrao_e1"),
        obj("rrao://hub"),
        obj("rrao://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrao://hub' RETURN REPLACE(b.msg, '-', '/')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["msg"], serde_json::json!("a/b/c"));
    store.close().unwrap();
}

// ── RETURN LENGTH / RETURN SUBSTRING tests ────────────────────────────────────

#[test]
fn return_length_of_string_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlosf://hub"), b"{}".to_vec());
    tx.put_object(obj("rlosf://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlosf_e1"),
        obj("rlosf://hub"),
        obj("rlosf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlosf://hub' RETURN LENGTH(b.word)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(5));
    store.close().unwrap();
}

#[test]
fn return_length_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlonsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rlonsp://a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlonsp_e1"),
        obj("rlonsp://hub"),
        obj("rlonsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlonsp://hub' RETURN LENGTH(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(42));
    store.close().unwrap();
}

#[test]
fn return_length_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LENGTH(b.name)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("name"),
        Some(ReturnTransform::Length)
    ));
}

#[test]
fn return_substring_extracts_chars() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rsec://hub"), b"{}".to_vec());
    tx.put_object(obj("rsec://a"), br#"{"code":"ABCDE"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rsec_e1"),
        obj("rsec://hub"),
        obj("rsec://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rsec://hub' RETURN SUBSTRING(b.code, 1, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["code"], serde_json::json!("BCD"));
    store.close().unwrap();
}

#[test]
fn return_substring_start_beyond_end_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rssbe://hub"), b"{}".to_vec());
    tx.put_object(obj("rssbe://a"), br#"{"s":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rssbe_e1"),
        obj("rssbe://hub"),
        obj("rssbe://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rssbe://hub' RETURN SUBSTRING(b.s, 10, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!(""));
    store.close().unwrap();
}

#[test]
fn return_substring_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rsonsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rsonsp://a"), br#"{"val":99}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rsonsp_e1"),
        obj("rsonsp://hub"),
        obj("rsonsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rsonsp://hub' RETURN SUBSTRING(b.val, 0, 2)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(99));
    store.close().unwrap();
}

#[test]
fn return_substring_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SUBSTRING(b.text, 2, 5)")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("text"),
        Some(ReturnTransform::Substring(2, 5))
    ));
}

#[test]
fn return_substring_zero_len_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rszlge://hub"), b"{}".to_vec());
    tx.put_object(obj("rszlge://a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rszlge_e1"),
        obj("rszlge://hub"),
        obj("rszlge://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rszlge://hub' RETURN SUBSTRING(b.name, 0, 0)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!(""));
    store.close().unwrap();
}

// ── RETURN CAST / RETURN NULLIF tests ─────────────────────────────────────────

#[test]
fn return_cast_as_int_from_float() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcaif://hub"), b"{}".to_vec());
    tx.put_object(obj("rcaif://a"), br#"{"score":9.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcaif_e1"),
        obj("rcaif://hub"),
        obj("rcaif://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcaif://hub' RETURN CAST(b.score AS INT)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["score"], serde_json::json!(9i64));
    store.close().unwrap();
}

#[test]
fn return_cast_as_float_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcafs://hub"), b"{}".to_vec());
    tx.put_object(obj("rcafs://a"), br#"{"val":"2.5"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcafs_e1"),
        obj("rcafs://hub"),
        obj("rcafs://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcafs://hub' RETURN CAST(b.val AS FLOAT)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 2.5).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cast_as_string_from_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcasfn://hub"), b"{}".to_vec());
    tx.put_object(obj("rcasfn://a"), br#"{"id":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcasfn_e1"),
        obj("rcasfn://hub"),
        obj("rcasfn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcasfn://hub' RETURN CAST(b.id AS STRING)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["id"], serde_json::json!("100"));
    store.close().unwrap();
}

#[test]
fn return_cast_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN CAST(b.n AS INT)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("n"),
        Some(ReturnTransform::CastInt)
    ));
}

#[test]
fn return_nullif_matching_value_omits_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnimvof://hub"), b"{}".to_vec());
    tx.put_object(obj("rnimvof://a"), br#"{"status":"N/A"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnimvof_e1"),
        obj("rnimvof://hub"),
        obj("rnimvof://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnimvof://hub' RETURN NULLIF(b.status, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(!rows[0].fields.contains_key("status"));
    store.close().unwrap();
}

#[test]
fn return_nullif_non_matching_value_kept() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnnmvk://hub"), b"{}".to_vec());
    tx.put_object(obj("rnnmvk://a"), br#"{"status":"active"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnnmvk_e1"),
        obj("rnnmvk://hub"),
        obj("rnnmvk://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnnmvk://hub' RETURN NULLIF(b.status, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["status"], serde_json::json!("active"));
    store.close().unwrap();
}

#[test]
fn return_nullif_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnonsp2://hub"), b"{}".to_vec());
    tx.put_object(obj("rnonsp2://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnonsp2_e1"),
        obj("rnonsp2://hub"),
        obj("rnonsp2://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnonsp2://hub' RETURN NULLIF(b.n, '0')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_nullif_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN NULLIF(b.status, 'none')",
    )
    .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("status"),
        Some(ReturnTransform::NullIf(s)) if s == "none"
    ));
}

// ── REVERSE / INITCAP / LPAD / RPAD transform tests ─────────────────────────

#[test]
fn return_reverse_reverses_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrev://hub"), b"{}".to_vec());
    tx.put_object(obj("rrev://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrev_e1"),
        obj("rrev://hub"),
        obj("rrev://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrev://hub' RETURN REVERSE(b.word)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("olleh"));
    store.close().unwrap();
}

#[test]
fn return_reverse_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REVERSE(b.word)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Reverse)
    ));
}

#[test]
fn return_initcap_capitalises_words() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ricp://hub"), b"{}".to_vec());
    tx.put_object(obj("ricp://a"), br#"{"title":"hello world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ricp_e1"),
        obj("ricp://hub"),
        obj("ricp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'ricp://hub' RETURN INITCAP(b.title)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["title"], serde_json::json!("Hello World"));
    store.close().unwrap();
}

#[test]
fn return_initcap_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN INITCAP(b.title)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("title"),
        Some(ReturnTransform::InitCap)
    ));
}

#[test]
fn return_lpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlpad://hub"), b"{}".to_vec());
    tx.put_object(obj("rlpad://a"), br#"{"code":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlpad_e1"),
        obj("rlpad://hub"),
        obj("rlpad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlpad://hub' RETURN LPAD(b.code, 5, '0')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["code"], serde_json::json!("00042"));
    store.close().unwrap();
}

#[test]
fn return_lpad_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LPAD(b.code, 8, '0')")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("code"),
        Some(ReturnTransform::Lpad(8, s)) if s == "0"
    ));
}

#[test]
fn return_rpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrpad://hub"), b"{}".to_vec());
    tx.put_object(obj("rrpad://a"), br#"{"name":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrpad_e1"),
        obj("rrpad://hub"),
        obj("rrpad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrpad://hub' RETURN RPAD(b.name, 5, '.')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!("hi..."));
    store.close().unwrap();
}

#[test]
fn return_rpad_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN RPAD(b.label, 10, ' ')")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("label"),
        Some(ReturnTransform::Rpad(10, s)) if s == " "
    ));
}

// ── REPEAT / LEFT / RIGHT transform tests ────────────────────────────────────

#[test]
fn return_repeat_duplicates_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grep://hub"), b"{}".to_vec());
    tx.put_object(obj("grep://a"), br#"{"sep":"-"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grep_e1"),
        obj("grep://hub"),
        obj("grep://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grep://hub' RETURN REPEAT(b.sep, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["sep"], serde_json::json!("---"));
    store.close().unwrap();
}

#[test]
fn return_repeat_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REPEAT(b.sep, 4)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("sep"),
        Some(ReturnTransform::Repeat(4))
    ));
}

#[test]
fn return_left_extracts_prefix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gleft://hub"), b"{}".to_vec());
    tx.put_object(obj("gleft://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gleft_e1"),
        obj("gleft://hub"),
        obj("gleft://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gleft://hub' RETURN LEFT(b.word, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("hel"));
    store.close().unwrap();
}

#[test]
fn return_left_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LEFT(b.word, 3)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Left(3))
    ));
}

#[test]
fn return_right_extracts_suffix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grght://hub"), b"{}".to_vec());
    tx.put_object(obj("grght://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grght_e1"),
        obj("grght://hub"),
        obj("grght://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grght://hub' RETURN RIGHT(b.word, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("llo"));
    store.close().unwrap();
}

#[test]
fn return_right_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN RIGHT(b.word, 3)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Right(3))
    ));
}

#[test]
fn return_left_beyond_length_returns_full_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gleftbl://hub"), b"{}".to_vec());
    tx.put_object(obj("gleftbl://a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gleftbl_e1"),
        obj("gleftbl://hub"),
        obj("gleftbl://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gleftbl://hub' RETURN LEFT(b.tag, 100)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hi"));
    store.close().unwrap();
}

#[test]
fn return_right_zero_gives_empty_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grght0://hub"), b"{}".to_vec());
    tx.put_object(obj("grght0://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grght0_e1"),
        obj("grght0://hub"),
        obj("grght0://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grght0://hub' RETURN RIGHT(b.tag, 0)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!(""));
    store.close().unwrap();
}

// ── SIGN / SQRT transform tests ───────────────────────────────────────────────

#[test]
fn return_sign_positive_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignp://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignp://a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignp_e1"),
        obj("gsignp://hub"),
        obj("gsignp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignp://hub' RETURN SIGN(b.score)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["score"], serde_json::json!(1));
    store.close().unwrap();
}

#[test]
fn return_sign_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignn://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignn://a"), br#"{"val":-7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignn_e1"),
        obj("gsignn://hub"),
        obj("gsignn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignn://hub' RETURN SIGN(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(-1));
    store.close().unwrap();
}

#[test]
fn return_sign_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SIGN(b.score)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("score"),
        Some(ReturnTransform::Sign)
    ));
}

#[test]
fn return_sqrt_of_perfect_square() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsqrt://hub"), b"{}".to_vec());
    tx.put_object(obj("gsqrt://a"), br#"{"n":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsqrt_e1"),
        obj("gsqrt://hub"),
        obj("gsqrt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsqrt://hub' RETURN SQRT(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_sqrt_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SQRT(b.n)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("n"),
        Some(ReturnTransform::Sqrt)
    ));
}

#[test]
fn return_sqrt_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsqrts://hub"), b"{}".to_vec());
    tx.put_object(obj("gsqrts://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsqrts_e1"),
        obj("gsqrts://hub"),
        obj("gsqrts://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsqrts://hub' RETURN SQRT(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_sign_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignz://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignz://a"), br#"{"val":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignz_e1"),
        obj("gsignz://hub"),
        obj("gsignz://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignz://hub' RETURN SIGN(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_sign_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignstr://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignstr_e1"),
        obj("gsignstr://hub"),
        obj("gsignstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignstr://hub' RETURN SIGN(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_power_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gpow://hub"), b"{}".to_vec());
    tx.put_object(obj("gpow://a"), br#"{"val":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gpow_e1"),
        obj("gpow://hub"),
        obj("gpow://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gpow://hub' RETURN POWER(b.val, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 8.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_power_parse() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN POWER(b.val, 3)")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("val"),
        Some(ReturnTransform::Power(e)) if (e - 3.0).abs() < 1e-9
    ));
}

#[test]
fn return_mod_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gmod://hub"), b"{}".to_vec());
    tx.put_object(obj("gmod://a"), br#"{"val":10}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gmod_e1"),
        obj("gmod://hub"),
        obj("gmod://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gmod://hub' RETURN MOD(b.val, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_mod_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gmodstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gmodstr://a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gmodstr_e1"),
        obj("gmodstr://hub"),
        obj("gmodstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gmodstr://hub' RETURN MOD(b.tag, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hi"));
    store.close().unwrap();
}

#[test]
fn return_strpos_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gstrpos://hub"), b"{}".to_vec());
    tx.put_object(obj("gstrpos://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gstrpos_e1"),
        obj("gstrpos://hub"),
        obj("gstrpos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gstrpos://hub' RETURN STRPOS(b.word, 'l')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(3));
    store.close().unwrap();
}

#[test]
fn return_strpos_not_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gstrpno://hub"), b"{}".to_vec());
    tx.put_object(obj("gstrpno://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gstrpno_e1"),
        obj("gstrpno://hub"),
        obj("gstrpno://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gstrpno://hub' RETURN STRPOS(b.word, 'z')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_split_part_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsplitpt://hub"), b"{}".to_vec());
    tx.put_object(obj("gsplitpt://a"), br#"{"csv":"a,b,c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsplitpt_e1"),
        obj("gsplitpt://hub"),
        obj("gsplitpt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsplitpt://hub' RETURN SPLIT_PART(b.csv, ',', 2)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["csv"], serde_json::json!("b"));
    store.close().unwrap();
}

#[test]
fn return_split_part_parse() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN SPLIT_PART(b.csv, ',', 2)",
    )
    .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("csv"),
        Some(ReturnTransform::SplitPart(d, 2)) if d == ","
    ));
}

#[test]
fn return_ltrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gltrim://hub"), b"{}".to_vec());
    tx.put_object(obj("gltrim://a"), br#"{"s":"  hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gltrim_e1"),
        obj("gltrim://hub"),
        obj("gltrim://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gltrim://hub' RETURN LTRIM(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_rtrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grtrim://hub"), b"{}".to_vec());
    tx.put_object(obj("grtrim://a"), br#"{"s":"hello  "}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grtrim_e1"),
        obj("grtrim://hub"),
        obj("grtrim://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grtrim://hub' RETURN RTRIM(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_log_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog://hub"), b"{}".to_vec());
    tx.put_object(obj("glog://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog_e1"),
        obj("glog://hub"),
        obj("glog://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'glog://hub' RETURN LOG(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_log_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog0://hub"), b"{}".to_vec());
    tx.put_object(obj("glog0://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog0_e1"),
        obj("glog0://hub"),
        obj("glog0://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'glog0://hub' RETURN LOG(b.n)")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_exp_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gexp://hub"), b"{}".to_vec());
    tx.put_object(obj("gexp://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gexp_e1"),
        obj("gexp://hub"),
        obj("gexp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gexp://hub' RETURN EXP(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_exp_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gexpstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gexpstr://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gexpstr_e1"),
        obj("gexpstr://hub"),
        obj("gexpstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gexpstr://hub' RETURN EXP(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_log2_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog2://hub"), b"{}".to_vec());
    tx.put_object(obj("glog2://a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog2_e1"),
        obj("glog2://hub"),
        obj("glog2://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog2://hub' RETURN LOG2(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_log2_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog2z://hub"), b"{}".to_vec());
    tx.put_object(obj("glog2z://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog2z_e1"),
        obj("glog2z://hub"),
        obj("glog2z://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog2z://hub' RETURN LOG2(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_log10_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog10://hub"), b"{}".to_vec());
    tx.put_object(obj("glog10://a"), br#"{"n":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog10_e1"),
        obj("glog10://hub"),
        obj("glog10://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog10://hub' RETURN LOG10(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_radians_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grad://hub"), b"{}".to_vec());
    tx.put_object(obj("grad://a"), br#"{"deg":180}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grad_e1"),
        obj("grad://hub"),
        obj("grad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grad://hub' RETURN RADIANS(b.deg)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["deg"].as_f64().unwrap();
    assert!((v - std::f64::consts::PI).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_degrees_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gdeg://hub"), b"{}".to_vec());
    tx.put_object(
        obj("gdeg://a"),
        format!(r#"{{"rad":{}}}"#, std::f64::consts::PI).into_bytes(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gdeg_e1"),
        obj("gdeg://hub"),
        obj("gdeg://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gdeg://hub' RETURN DEGREES(b.rad)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["rad"].as_f64().unwrap();
    assert!((v - 180.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_radians_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gradstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gradstr://a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gradstr_e1"),
        obj("gradstr://hub"),
        obj("gradstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gradstr://hub' RETURN RADIANS(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("north"));
    store.close().unwrap();
}

#[test]
fn return_degrees_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gdegstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gdegstr://a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gdegstr_e1"),
        obj("gdegstr://hub"),
        obj("gdegstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gdegstr://hub' RETURN DEGREES(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("north"));
    store.close().unwrap();
}

#[test]
fn return_sin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsin://hub"), b"{}".to_vec());
    tx.put_object(obj("gsin://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsin_e1"),
        obj("gsin://hub"),
        obj("gsin://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gsin://hub' RETURN SIN(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_sin_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsinstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gsinstr://a"), br#"{"s":"wave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsinstr_e1"),
        obj("gsinstr://hub"),
        obj("gsinstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsinstr://hub' RETURN SIN(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("wave"));
    store.close().unwrap();
}

#[test]
fn return_cos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcos://hub"), b"{}".to_vec());
    tx.put_object(obj("gcos://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcos_e1"),
        obj("gcos://hub"),
        obj("gcos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gcos://hub' RETURN COS(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_tan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtan://hub"), b"{}".to_vec());
    tx.put_object(obj("gtan://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtan_e1"),
        obj("gtan://hub"),
        obj("gtan://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gtan://hub' RETURN TAN(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_atan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gatan://hub"), b"{}".to_vec());
    tx.put_object(obj("gatan://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gatan_e1"),
        obj("gatan://hub"),
        obj("gatan://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gatan://hub' RETURN ATAN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_atan_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gatanstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gatanstr://a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gatanstr_e1"),
        obj("gatanstr://hub"),
        obj("gatanstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gatanstr://hub' RETURN ATAN(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("arc"));
    store.close().unwrap();
}

#[test]
fn return_asin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gasin://hub"), b"{}".to_vec());
    tx.put_object(obj("gasin://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gasin_e1"),
        obj("gasin://hub"),
        obj("gasin://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gasin://hub' RETURN ASIN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_asin_out_of_domain_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gasinood://hub"), b"{}".to_vec());
    tx.put_object(obj("gasinood://a"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gasinood_e1"),
        obj("gasinood://hub"),
        obj("gasinood://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gasinood://hub' RETURN ASIN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn return_acos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gacos://hub"), b"{}".to_vec());
    tx.put_object(obj("gacos://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gacos_e1"),
        obj("gacos://hub"),
        obj("gacos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gacos://hub' RETURN ACOS(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_acos_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gacosstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gacosstr://a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gacosstr_e1"),
        obj("gacosstr://hub"),
        obj("gacosstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gacosstr://hub' RETURN ACOS(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("arc"));
    store.close().unwrap();
}

#[test]
fn return_trunc_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtruncn://hub"), b"{}".to_vec());
    tx.put_object(obj("gtruncn://a"), br#"{"n":-1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtruncn_e1"),
        obj("gtruncn://hub"),
        obj("gtruncn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gtruncn://hub' RETURN TRUNC(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - (-1.0)).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_trunc_positive() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtruncp://hub"), b"{}".to_vec());
    tx.put_object(obj("gtruncp://a"), br#"{"n":1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtruncp_e1"),
        obj("gtruncp://hub"),
        obj("gtruncp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gtruncp://hub' RETURN TRUNC(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cbrt_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcbrt://hub"), b"{}".to_vec());
    tx.put_object(obj("gcbrt://a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcbrt_e1"),
        obj("gcbrt://hub"),
        obj("gcbrt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gcbrt://hub' RETURN CBRT(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cbrt_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcbrtstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gcbrtstr://a"), br#"{"s":"root"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcbrtstr_e1"),
        obj("gcbrtstr://hub"),
        obj("gcbrtstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gcbrtstr://hub' RETURN CBRT(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("root"));
    store.close().unwrap();
}

// --- UNION ---

#[test]
fn union_merges_two_edge_types() {
    // Alice -FOLLOWS-> Bob and Alice -LIKED_BY-> Carol; UNION should return both.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn://alice"), b"{}".to_vec());
    tx.put_object(obj("sn://bob"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("sn://carol"), br#"{"name":"carol"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("uf_e1"),
        obj("sn://alice"),
        obj("sn://bob"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("uf_e2"),
        obj("sn://alice"),
        obj("sn://carol"),
        "LIKED_BY",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:FOLLOWS]->(b) WHERE id(a) = 'sn://alice' RETURN b.name \
             UNION \
             MATCH (a)-[:LIKED_BY]->(b) WHERE id(a) = 'sn://alice' RETURN b.name",
        )
        .unwrap();
    let names: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| {
            r.fields
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(names.contains("bob"), "expected bob in union");
    assert!(names.contains("carol"), "expected carol in union");
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn union_deduplicates_same_node() {
    // Same node reachable via two edge types; UNION deduplicates by object_id.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn2://hub"), b"{}".to_vec());
    tx.put_object(obj("sn2://shared"), br#"{"name":"shared"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ud_e1"),
        obj("sn2://hub"),
        obj("sn2://shared"),
        "EDGE_A",
        None,
    );
    tx2.put_edge(
        eid("ud_e2"),
        obj("sn2://hub"),
        obj("sn2://shared"),
        "EDGE_B",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:EDGE_A]->(b) WHERE id(a) = 'sn2://hub' RETURN b.name \
             UNION \
             MATCH (a)-[:EDGE_B]->(b) WHERE id(a) = 'sn2://hub' RETURN b.name",
        )
        .unwrap();
    // UNION should deduplicate: shared node appears once.
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("shared")
    );
    store.close().unwrap();
}

#[test]
fn union_all_preserves_duplicates() {
    // Same node reachable via two edge types; UNION ALL keeps both rows.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn3://hub"), b"{}".to_vec());
    tx.put_object(obj("sn3://shared"), br#"{"name":"shared"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ua_e1"),
        obj("sn3://hub"),
        obj("sn3://shared"),
        "EDGE_A",
        None,
    );
    tx2.put_edge(
        eid("ua_e2"),
        obj("sn3://hub"),
        obj("sn3://shared"),
        "EDGE_B",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:EDGE_A]->(b) WHERE id(a) = 'sn3://hub' RETURN b.name \
             UNION ALL \
             MATCH (a)-[:EDGE_B]->(b) WHERE id(a) = 'sn3://hub' RETURN b.name",
        )
        .unwrap();
    // UNION ALL: both rows preserved.
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn union_three_branches() {
    // Three independent hubs; UNION of three MATCH clauses merges all results.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn4://h1"), b"{}".to_vec());
    tx.put_object(obj("sn4://h2"), b"{}".to_vec());
    tx.put_object(obj("sn4://h3"), b"{}".to_vec());
    tx.put_object(obj("sn4://n1"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("sn4://n2"), br#"{"v":2}"#.to_vec());
    tx.put_object(obj("sn4://n3"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("u3_e1"), obj("sn4://h1"), obj("sn4://n1"), "X", None);
    tx2.put_edge(eid("u3_e2"), obj("sn4://h2"), obj("sn4://n2"), "X", None);
    tx2.put_edge(eid("u3_e3"), obj("sn4://h3"), obj("sn4://n3"), "X", None);
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h1' RETURN b.v \
             UNION \
             MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h2' RETURN b.v \
             UNION \
             MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h3' RETURN b.v",
        )
        .unwrap();
    let vals: std::collections::HashSet<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("v").and_then(|v| v.as_i64()))
        .collect();
    assert_eq!(vals, [1, 2, 3].into_iter().collect());
    store.close().unwrap();
}

#[test]
fn union_parse_error_on_single_match() {
    // Parsing a single MATCH as a UNION query must fail.
    let err = GraphIrUnionQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN b.name");
    assert!(err.is_err());
}

#[test]
fn union_projected_api() {
    // Test graph_ir_union_projected with pre-parsed query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn5://hub"), b"{}".to_vec());
    tx.put_object(obj("sn5://a"), br#"{"n":1}"#.to_vec());
    tx.put_object(obj("sn5://b"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("up_e1"), obj("sn5://hub"), obj("sn5://a"), "A", None);
    tx2.put_edge(eid("up_e2"), obj("sn5://hub"), obj("sn5://b"), "B", None);
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrUnionQuery::parse(
        "MATCH (a)-[:A]->(b) WHERE id(a) = 'sn5://hub' RETURN b.n \
         UNION \
         MATCH (a)-[:B]->(b) WHERE id(a) = 'sn5://hub' RETURN b.n",
    )
    .unwrap();
    assert_eq!(q.branches.len(), 2);
    assert!(!q.all);

    let rows = store.graph_ir_union_projected(&q).unwrap();
    let ns: std::collections::HashSet<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("n").and_then(|v| v.as_i64()))
        .collect();
    assert!(ns.contains(&1));
    assert!(ns.contains(&2));
    store.close().unwrap();
}
