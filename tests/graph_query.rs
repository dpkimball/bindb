use bindb::{BinderStore, Direction, EdgeId, ObjectId, StoreError, Transaction};
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

// Build a small social graph:
//   person → (KNOWS) → memory1
//   person → (KNOWS) → memory2
//   person → (TAGGED) → tag1
//   memory1 → (RELATES_TO) → memory3
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

// ── basic traversal ───────────────────────────────────────────────────────────

#[test]
fn query_returns_all_reachable_within_depth() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(1)
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"memory://m1"));
    assert!(ids.contains(&"memory://m2"));
    assert!(ids.contains(&"tag://t1"));
    // depth-2 node must NOT appear at max_depth=1
    assert!(!ids.contains(&"memory://m3"));
    store.close().unwrap();
}

#[test]
fn query_depth_2_reaches_transitive_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(2)
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"memory://m3"), "m3 is 2 hops away");
    store.close().unwrap();
}

#[test]
fn depth_field_is_correct() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(2)
        .execute()
        .unwrap();

    for m in &results {
        if m.object.object_id.0 == "memory://m3" {
            assert_eq!(m.depth, 2, "m3 must report depth 2");
        } else {
            assert_eq!(m.depth, 1, "{} should be depth 1", m.object.object_id.0);
        }
    }
    store.close().unwrap();
}

// ── edge type filter ─────────────────────────────────────────────────────────

#[test]
fn edge_type_filter_restricts_traversal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .edge_types(["KNOWS"])
        .max_depth(1)
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"memory://m1"));
    assert!(ids.contains(&"memory://m2"));
    assert!(!ids.contains(&"tag://t1"), "TAGGED edge must be excluded");
    store.close().unwrap();
}

#[test]
fn multiple_edge_types_are_a_union() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .edge_types(["KNOWS", "TAGGED"])
        .max_depth(1)
        .execute()
        .unwrap();

    assert_eq!(results.len(), 3, "KNOWS(2) + TAGGED(1) = 3 nodes");
    store.close().unwrap();
}

// ── node filter ───────────────────────────────────────────────────────────────

#[test]
fn filter_node_restricts_results_but_not_traversal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    // Only want memory:// nodes.  Traversal still passes through alice's tags.
    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(2)
        .filter_node(|obj| obj.object_id.0.starts_with("memory://"))
        .execute()
        .unwrap();

    assert!(results
        .iter()
        .all(|m| m.object.object_id.0.starts_with("memory://")));
    // m3 is reachable via m1 even though tag isn't a memory.
    assert!(results
        .iter()
        .any(|m| m.object.object_id.0 == "memory://m3"));
    store.close().unwrap();
}

// ── direction ─────────────────────────────────────────────────────────────────

#[test]
fn incoming_direction_traverses_reverse_edges() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    // Start from m1, traverse incoming: alice should be found.
    let results = store
        .graph_query()
        .from(&obj("memory://m1"))
        .direction(Direction::Incoming)
        .max_depth(1)
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(
        ids.contains(&"person://alice"),
        "incoming edge from alice must be found"
    );
    store.close().unwrap();
}

#[test]
fn both_direction_traverses_in_and_out() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    // Start from m1: outgoing → m3, incoming → alice.
    let results = store
        .graph_query()
        .from(&obj("memory://m1"))
        .direction(Direction::Both)
        .max_depth(1)
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(ids.contains(&"person://alice"));
    assert!(ids.contains(&"memory://m3"));
    store.close().unwrap();
}

// ── edge filter ───────────────────────────────────────────────────────────────

#[test]
fn filter_edge_blocks_traversal_along_excluded_edges() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    // Block TAGGED edges via edge filter.
    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(1)
        .filter_edge(|e| e.relationship_type.as_deref() != Some("TAGGED"))
        .execute()
        .unwrap();

    let ids: Vec<_> = results
        .iter()
        .map(|m| m.object.object_id.0.as_str())
        .collect();
    assert!(
        !ids.contains(&"tag://t1"),
        "TAGGED edge must be blocked by edge filter"
    );
    assert!(ids.contains(&"memory://m1"));
    store.close().unwrap();
}

// ── error cases ───────────────────────────────────────────────────────────────

#[test]
fn execute_without_from_returns_error() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let result = store.graph_query().max_depth(3).execute();
    assert!(
        matches!(result, Err(StoreError::QueryMissingStart)),
        "missing start must return QueryMissingStart"
    );
    store.close().unwrap();
}

#[test]
fn query_on_unknown_start_returns_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let results = store
        .graph_query()
        .from(&obj("nobody://x"))
        .max_depth(3)
        .execute()
        .unwrap();

    assert!(results.is_empty(), "unknown start must yield no results");
    store.close().unwrap();
}

#[test]
fn start_node_not_included_in_results() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_social_graph(&store);

    let results = store
        .graph_query()
        .from(&obj("person://alice"))
        .max_depth(3)
        .execute()
        .unwrap();

    assert!(
        results
            .iter()
            .all(|m| m.object.object_id.0 != "person://alice"),
        "start node must not appear in results"
    );
    store.close().unwrap();
}
