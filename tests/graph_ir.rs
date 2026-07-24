use bindb::{BinderStore, EdgeId, GraphIrQuery, ObjectId, Transaction};
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
