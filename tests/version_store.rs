/// Tests for the disk-backed VersionStore.
///
/// These tests focus on the properties that differ from the in-memory path:
/// - persisted valid_to via RetireObject/RetireEdge records
/// - correct as_of semantics through the index
/// - history returns all versions in insertion order
/// - rebuild_from_versions resets the file and index correctly
/// - high-version-count stores don't exhaust RAM (sanity smoke)
use bindb::{BinderStore, CommitId, EdgeId, ObjectId, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}
fn oid(s: &str) -> ObjectId {
    ObjectId::new(s)
}
fn eid(s: &str) -> EdgeId {
    EdgeId::new(s)
}

// ── object history via VersionStore ──────────────────────────────────────────

#[test]
fn version_store_returns_full_object_history() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("mem://a"), b"v1".to_vec());
    store.commit(tx, "test", "c1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid("mem://a"), b"v2".to_vec());
    store.commit(tx2, "test", "c2").unwrap();

    let mut tx3 = Transaction::new();
    tx3.put_object(oid("mem://a"), b"v3".to_vec());
    store.commit(tx3, "test", "c3").unwrap();

    let history = store.object_history(&oid("mem://a")).unwrap();
    assert_eq!(history.len(), 3, "three versions expected");
    assert_eq!(history[0].payload.as_deref(), Some(b"v1".as_ref()));
    assert_eq!(history[1].payload.as_deref(), Some(b"v2".as_ref()));
    assert_eq!(history[2].payload.as_deref(), Some(b"v3".as_ref()));
    store.close().unwrap();
}

#[test]
fn version_store_valid_to_is_set_on_historical_versions() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("mem://b"), b"first".to_vec());
    let c1 = store.commit(tx, "test", "c1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid("mem://b"), b"second".to_vec());
    let c2 = store.commit(tx2, "test", "c2").unwrap();

    let history = store.object_history(&oid("mem://b")).unwrap();
    assert_eq!(history.len(), 2);
    // First version must be retired at c2.
    assert_eq!(history[0].valid_to, Some(c2), "v1.valid_to must equal c2");
    // Second version must be live.
    assert_eq!(history[1].valid_to, None, "v2 must be current");
    // valid_from of v2 must equal valid_to of v1.
    assert_eq!(history[1].valid_from, c2);
    // valid_from of v1 == c1.
    assert_eq!(history[0].valid_from, c1);
    store.close().unwrap();
}

// ── object_as_of via VersionStore ─────────────────────────────────────────────

#[test]
fn version_store_as_of_returns_correct_snapshot() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("mem://c"), b"alpha".to_vec());
    let c1 = store.commit(tx, "test", "c1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid("mem://c"), b"beta".to_vec());
    let c2 = store.commit(tx2, "test", "c2").unwrap();

    let at_c1 = store.object_as_of(&oid("mem://c"), c1).unwrap();
    assert_eq!(at_c1.unwrap().payload.as_deref(), Some(b"alpha".as_ref()));

    let at_c2 = store.object_as_of(&oid("mem://c"), c2).unwrap();
    assert_eq!(at_c2.unwrap().payload.as_deref(), Some(b"beta".as_ref()));
    store.close().unwrap();
}

#[test]
fn version_store_as_of_before_creation_returns_none() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let before = CommitId::new();

    let mut tx = Transaction::new();
    tx.put_object(oid("mem://d"), b"payload".to_vec());
    store.commit(tx, "test", "c1").unwrap();

    let result = store.object_as_of(&oid("mem://d"), before).unwrap();
    assert!(
        result.is_none(),
        "object didn't exist before its first commit"
    );
    store.close().unwrap();
}

#[test]
fn version_store_tombstone_returns_none_from_as_of() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(oid("mem://e"), b"alive".to_vec());
    let c1 = store.commit(tx, "test", "c1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(oid("mem://e"));
    let c2 = store.commit(tx2, "test", "c2").unwrap();

    // At c1 the object existed.
    let at_c1 = store.object_as_of(&oid("mem://e"), c1).unwrap();
    assert!(at_c1.is_some());

    // At c2 it's tombstoned — should return None.
    let at_c2 = store.object_as_of(&oid("mem://e"), c2).unwrap();
    assert!(
        at_c2.is_none(),
        "tombstoned object must return None from as_of"
    );
    store.close().unwrap();
}

// ── edge history via VersionStore ─────────────────────────────────────────────

#[test]
fn version_store_returns_edge_history() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut setup = Transaction::new();
    setup.put_object(oid("a://1"), b"a".to_vec());
    setup.put_object(oid("b://1"), b"b".to_vec());
    store.commit(setup, "test", "nodes").unwrap();

    let mut tx = Transaction::new();
    tx.put_edge(eid("e1"), oid("a://1"), oid("b://1"), "LINK", None);
    let c1 = store.commit(tx, "test", "e1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_edge(eid("e1"));
    let c2 = store.commit(tx2, "test", "del").unwrap();

    let history = store.edge_history(&eid("e1")).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].valid_from, c1);
    assert_eq!(history[0].valid_to, Some(c2));
    assert!(history[1].is_tombstone());
    store.close().unwrap();
}

#[test]
fn version_store_edge_as_of_returns_correct_snapshot() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut setup = Transaction::new();
    setup.put_object(oid("x://1"), b"x".to_vec());
    setup.put_object(oid("y://1"), b"y".to_vec());
    store.commit(setup, "test", "nodes").unwrap();

    let mut tx = Transaction::new();
    tx.put_edge(eid("e2"), oid("x://1"), oid("y://1"), "KNOWS", None);
    let c1 = store.commit(tx, "test", "c1").unwrap();

    let before = CommitId::new();
    let _ = before; // earlier than c1

    let at_c1 = store.edge_as_of(&eid("e2"), c1).unwrap();
    assert!(at_c1.is_some());
    assert_eq!(at_c1.unwrap().relationship_type.as_deref(), Some("KNOWS"));
    store.close().unwrap();
}

// ── rebuild survives reopen ────────────────────────────────────────────────────

#[test]
fn version_store_rebuilt_correctly_on_reopen() {
    let dir = tmp();

    // First session: write three versions.
    {
        let store = BinderStore::open(dir.path()).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(oid("mem://f"), b"v1".to_vec());
        store.commit(tx, "test", "c1").unwrap();

        let mut tx2 = Transaction::new();
        tx2.put_object(oid("mem://f"), b"v2".to_vec());
        store.commit(tx2, "test", "c2").unwrap();
        store.close().unwrap();
    }

    // Second session: history must still be correct.
    {
        let store = BinderStore::open(dir.path()).unwrap();
        let history = store.object_history(&oid("mem://f")).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[0].valid_to.is_some(), "v1 must be retired");
        assert!(history[1].valid_to.is_none(), "v2 must be current");
        store.close().unwrap();
    }
}

// ── many-version smoke test ───────────────────────────────────────────────────

#[test]
fn version_store_handles_many_versions_without_error() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Write 500 versions of the same object — exercises retire/append cycling.
    for i in 0u32..500 {
        let mut tx = Transaction::new();
        tx.put_object(oid("mem://g"), i.to_le_bytes().to_vec());
        store.commit(tx, "test", "v").unwrap();
    }

    let history = store.object_history(&oid("mem://g")).unwrap();
    assert_eq!(history.len(), 500);

    // All but the last version must be retired.
    let retired = history.iter().filter(|v| v.valid_to.is_some()).count();
    assert_eq!(retired, 499);
    assert!(history.last().unwrap().valid_to.is_none());
    store.close().unwrap();
}
