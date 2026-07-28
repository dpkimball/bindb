use bindb::{
    AggFunc, BinderStore, CmpOp, CmpValue, EdgeId, GraphIrQuery, GraphIrWrite, GraphIrWriteError,
    NodeIdPred, ObjectId, OrderByField, OrderDirection, Transaction, WithAggregation,
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
    let ob = q.order_by.unwrap();
    assert_eq!(ob.field, OrderByField::ObjectId);
    assert_eq!(ob.direction, OrderDirection::Desc);
    assert_eq!(q.limit, Some(5));
    assert_eq!(q.skip, None);

    let q2 = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY depth ASC SKIP 2 LIMIT 10",
    )
    .unwrap();
    let ob2 = q2.order_by.unwrap();
    assert_eq!(ob2.field, OrderByField::Depth);
    assert_eq!(ob2.direction, OrderDirection::Asc);
    assert_eq!(q2.skip, Some(2));
    assert_eq!(q2.limit, Some(10));

    let q3 = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY b.sentiment DESC",
    )
    .unwrap();
    let ob3 = q3.order_by.unwrap();
    assert_eq!(ob3.field, OrderByField::PayloadField("sentiment".into()));
    assert_eq!(ob3.direction, OrderDirection::Desc);
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
    assert!(wc.order_by.is_some());
    assert_eq!(
        wc.order_by.as_ref().unwrap().direction,
        OrderDirection::Desc
    );
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
    assert!(wc.order_by.is_none());
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
