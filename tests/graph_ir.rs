use bindb::{
    BinderStore, EdgeId, GraphIrQuery, GraphIrWrite, GraphIrWriteError, ObjectId, OrderByField,
    OrderDirection, Transaction,
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
