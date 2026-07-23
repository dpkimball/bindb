use bindb::{BinderStore, EdgeId, ObjectId, Transaction};
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

fn create_edge(store: &BinderStore, edge_id: &str, from: &str, to: &str, rel: &str) {
    let mut tx = Transaction::new();
    tx.put_edge(eid(edge_id), obj(from), obj(to), rel, None);
    store
        .commit(tx, "actor", format!("edge {edge_id}"))
        .unwrap();
}

// ─── edges_from ───────────────────────────────────────────────────────────────

#[test]
fn edges_from_empty_for_unknown_object() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let result = store.edges_from(&obj("nobody")).unwrap();
    assert!(result.is_empty());
    store.close().unwrap();
}

#[test]
fn edges_from_returns_live_outgoing_edges() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    create_edge(&store, "e1", "a", "b", "LINK");
    create_edge(&store, "e2", "a", "c", "LINK");
    create_edge(&store, "e3", "b", "c", "LINK"); // unrelated

    let edges = store.edges_from(&obj("a")).unwrap();
    assert_eq!(edges.len(), 2);
    let edge_ids: Vec<_> = edges.iter().map(|e| e.edge_id.0.as_str()).collect();
    assert!(edge_ids.contains(&"e1"));
    assert!(edge_ids.contains(&"e2"));
    assert!(!edge_ids.contains(&"e3"));

    store.close().unwrap();
}

// ─── edges_to ─────────────────────────────────────────────────────────────────

#[test]
fn edges_to_returns_incoming_edges() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    create_edge(&store, "e1", "a", "c", "LINK");
    create_edge(&store, "e2", "b", "c", "LINK");
    create_edge(&store, "e3", "c", "d", "LINK"); // outgoing from c

    let edges = store.edges_to(&obj("c")).unwrap();
    assert_eq!(edges.len(), 2);
    let edge_ids: Vec<_> = edges.iter().map(|e| e.edge_id.0.as_str()).collect();
    assert!(edge_ids.contains(&"e1"));
    assert!(edge_ids.contains(&"e2"));

    store.close().unwrap();
}

// ─── edges_from_with_type ─────────────────────────────────────────────────────

#[test]
fn edges_from_with_type_filters_correctly() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    create_edge(&store, "same1", "x", "y", "SAME_AS");
    create_edge(&store, "link1", "x", "z", "LINK");

    let same_as_edges = store.edges_from_with_type(&obj("x"), "SAME_AS").unwrap();
    assert_eq!(same_as_edges.len(), 1);
    assert_eq!(same_as_edges[0].edge_id.0, "same1");

    let link_edges = store.edges_from_with_type(&obj("x"), "LINK").unwrap();
    assert_eq!(link_edges.len(), 1);
    assert_eq!(link_edges[0].edge_id.0, "link1");

    let none_edges = store.edges_from_with_type(&obj("x"), "CONTAINS").unwrap();
    assert!(none_edges.is_empty());

    store.close().unwrap();
}

// ─── tombstone exclusion ─────────────────────────────────────────────────────

#[test]
fn deleted_edge_excluded_from_graph_index() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    create_edge(&store, "e-del", "a", "b", "LINK");
    assert_eq!(store.edges_from(&obj("a")).unwrap().len(), 1);

    let mut tx = Transaction::new();
    tx.delete_edge(eid("e-del"));
    store.commit(tx, "actor", "delete edge").unwrap();

    assert!(
        store.edges_from(&obj("a")).unwrap().is_empty(),
        "deleted edge must leave graph index"
    );
    assert!(store.edges_to(&obj("b")).unwrap().is_empty());

    store.close().unwrap();
}

#[test]
fn re_add_edge_after_deletion_appears_in_index() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    create_edge(&store, "e-toggle", "a", "b", "LINK");

    let mut del = Transaction::new();
    del.delete_edge(eid("e-toggle"));
    store.commit(del, "actor", "delete").unwrap();
    assert!(store.edges_from(&obj("a")).unwrap().is_empty());

    create_edge(&store, "e-toggle", "a", "b", "LINK");
    let edges = store.edges_from(&obj("a")).unwrap();
    assert_eq!(edges.len(), 1);
    assert!(!edges[0].is_tombstone());

    store.close().unwrap();
}

// ─── BFS traversal ───────────────────────────────────────────────────────────

#[test]
fn traverse_empty_from_isolated_node() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("lone"), b"x".to_vec());
    store.commit(tx, "actor", "lone").unwrap();

    let reachable = store.traverse(&obj("lone"), 3).unwrap();
    assert!(reachable.is_empty());
    store.close().unwrap();
}

#[test]
fn traverse_one_hop() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Create objects so HEAD index knows they're alive.
    let mut tx = Transaction::new();
    tx.put_object(obj("a"), b"a".to_vec());
    tx.put_object(obj("b"), b"b".to_vec());
    tx.put_edge(eid("ab"), obj("a"), obj("b"), "LINK", None);
    store.commit(tx, "actor", "init").unwrap();

    let reachable = store.traverse(&obj("a"), 1).unwrap();
    assert_eq!(reachable.len(), 1);
    assert_eq!(reachable[0], obj("b"));
    store.close().unwrap();
}

#[test]
fn traverse_multi_hop() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    for id in ["a", "b", "c", "d"] {
        tx.put_object(obj(id), id.as_bytes().to_vec());
    }
    tx.put_edge(eid("ab"), obj("a"), obj("b"), "L", None);
    tx.put_edge(eid("bc"), obj("b"), obj("c"), "L", None);
    tx.put_edge(eid("cd"), obj("c"), obj("d"), "L", None);
    store.commit(tx, "actor", "chain").unwrap();

    let depth2 = store.traverse(&obj("a"), 2).unwrap();
    assert_eq!(depth2.len(), 2);
    assert!(depth2.contains(&obj("b")));
    assert!(depth2.contains(&obj("c")));

    let depth3 = store.traverse(&obj("a"), 3).unwrap();
    assert_eq!(depth3.len(), 3);
    assert!(depth3.contains(&obj("d")));
    store.close().unwrap();
}

#[test]
fn traverse_skips_tombstoned_objects() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("a"), b"a".to_vec());
    tx.put_object(obj("b"), b"b".to_vec()); // will be deleted
    tx.put_object(obj("c"), b"c".to_vec());
    tx.put_edge(eid("ab"), obj("a"), obj("b"), "L", None);
    tx.put_edge(eid("bc"), obj("b"), obj("c"), "L", None);
    store.commit(tx, "actor", "init").unwrap();

    // Delete object b — traversal should skip it and not reach c through it.
    let mut del = Transaction::new();
    del.delete_object(obj("b"));
    store.commit(del, "actor", "delete b").unwrap();

    let reachable = store.traverse(&obj("a"), 3).unwrap();
    assert!(
        !reachable.contains(&obj("b")),
        "tombstoned b must be pruned"
    );
    assert!(
        !reachable.contains(&obj("c")),
        "c unreachable because path through b is pruned"
    );
    store.close().unwrap();
}

#[test]
fn graph_index_rebuilt_correctly_after_crash() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(obj("a"), b"a".to_vec());
        tx.put_object(obj("b"), b"b".to_vec());
        tx.put_edge(eid("ab"), obj("a"), obj("b"), "LINK", None);
        store.commit(tx, "actor", "init").unwrap();
        drop(store); // crash
    }

    let store2 = BinderStore::open(&path).unwrap();
    let edges = store2.edges_from(&obj("a")).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].edge_id.0, "ab");
    store2.close().unwrap();
}
