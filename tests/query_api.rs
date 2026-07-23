use bindb::{BinderStore, EdgeId, ObjectId, StoreError, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn obj(s: &str) -> ObjectId {
    ObjectId::new(s)
}

// ─── as_of queries ────────────────────────────────────────────────────────────

#[test]
fn object_as_of_at_first_commit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_object(obj("x"), b"v1".to_vec());
    let c1 = store.commit(tx1, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(obj("x"), b"v2".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    // as_of c1 must return the v1 payload.
    let at_c1 = store.object_as_of(&obj("x"), c1).unwrap();
    assert!(at_c1.is_some());
    assert_eq!(at_c1.unwrap().payload.as_deref(), Some(b"v1".as_slice()));

    store.close().unwrap();
}

#[test]
fn object_as_of_at_second_commit_returns_v2() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_object(obj("y"), b"v1".to_vec());
    store.commit(tx1, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(obj("y"), b"v2".to_vec());
    let c2 = store.commit(tx2, "actor", "v2").unwrap();

    let at_c2 = store.object_as_of(&obj("y"), c2).unwrap();
    assert_eq!(at_c2.unwrap().payload.as_deref(), Some(b"v2".as_slice()));

    store.close().unwrap();
}

#[test]
fn object_as_of_returns_none_before_object_existed() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Commit an unrelated object first to get a CommitId.
    let mut tx0 = Transaction::new();
    tx0.put_object(obj("unrelated"), b"x".to_vec());
    let c0 = store.commit(tx0, "actor", "unrelated").unwrap();

    // Now create our object.
    let mut tx1 = Transaction::new();
    tx1.put_object(obj("late"), b"data".to_vec());
    store.commit(tx1, "actor", "late").unwrap();

    // as_of c0 — object didn't exist yet.
    let result = store.object_as_of(&obj("late"), c0).unwrap();
    assert!(result.is_none(), "object did not exist at c0");

    store.close().unwrap();
}

#[test]
fn object_as_of_after_tombstone_returns_none() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_object(obj("z"), b"alive".to_vec());
    store.commit(tx1, "actor", "create").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(obj("z"));
    let c2 = store.commit(tx2, "actor", "delete").unwrap();

    // as_of c2 — tombstone exists at c2, so result should be None (deleted).
    let result = store.object_as_of(&obj("z"), c2).unwrap();
    assert!(result.is_none(), "tombstone should appear as None");

    store.close().unwrap();
}

#[test]
fn edge_as_of_historical_query() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_edge(
        EdgeId::new("e1"),
        obj("from"),
        obj("to"),
        "SAME_AS",
        Some(b"original".to_vec()),
    );
    let c1 = store.commit(tx1, "actor", "create edge").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        EdgeId::new("e1"),
        obj("from"),
        obj("to"),
        "SAME_AS",
        Some(b"updated".to_vec()),
    );
    store.commit(tx2, "actor", "update edge").unwrap();

    let at_c1 = store.edge_as_of(&EdgeId::new("e1"), c1).unwrap();
    assert!(at_c1.is_some());
    assert_eq!(
        at_c1.unwrap().payload.as_deref(),
        Some(b"original".as_slice())
    );

    store.close().unwrap();
}

// ─── namespace enforcement ────────────────────────────────────────────────────

#[test]
fn commit_passes_when_namespace_registered_for_actor() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store.register_namespace("story", "story-binder").unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("story://x"), b"data".to_vec());
    let result = store.commit_with_namespace(tx, "story-binder", "ok", "story");
    assert!(result.is_ok());
    store.close().unwrap();
}

#[test]
fn commit_fails_when_actor_does_not_own_namespace() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store.register_namespace("story", "story-binder").unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("story://x"), b"data".to_vec());
    let result = store.commit_with_namespace(tx, "memory-binder", "steal", "story");
    assert!(
        matches!(result, Err(StoreError::NamespaceViolation { .. })),
        "wrong actor must be rejected"
    );
    store.close().unwrap();
}

#[test]
fn namespace_violation_leaves_store_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store.register_namespace("ns", "owner").unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("ns://x"), b"data".to_vec());
    store
        .commit_with_namespace(tx, "intruder", "bad", "ns")
        .unwrap_err();

    // Store must be empty.
    assert!(store.scan_commits().unwrap().is_empty());
    assert!(store.get(&obj("ns://x")).unwrap().is_none());
    store.close().unwrap();
}

#[test]
fn unregistered_namespace_is_permitted() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    // No namespace registered — open namespace; any actor may write.
    let mut tx = Transaction::new();
    tx.put_object(obj("free://x"), b"data".to_vec());
    let result = store.commit_with_namespace(tx, "anybody", "ok", "free");
    assert!(result.is_ok());
    store.close().unwrap();
}

#[test]
fn edge_writes_bypass_namespace_check() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store.register_namespace("locked", "owner").unwrap();

    // Edge-only transaction with a namespace that doesn't own "locked" — must pass
    // because edges cross namespaces freely (PRINCIPLES I-9).
    let mut tx = Transaction::new();
    tx.put_edge(
        EdgeId::new("cross-edge"),
        obj("locked://a"),
        obj("other://b"),
        "SAME_AS",
        None,
    );
    // namespace check is only on object writes; edge-only tx goes through plain commit.
    let result = store.commit(tx, "intruder", "edge only");
    assert!(
        result.is_ok(),
        "edge-only commits bypass namespace enforcement"
    );
    store.close().unwrap();
}

// ─── checkpoint ──────────────────────────────────────────────────────────────

#[test]
fn checkpoint_written_on_clean_close() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("cp-obj"), b"x".to_vec());
    store.commit(tx, "actor", "commit before close").unwrap();
    store.close().unwrap();

    // checkpoint.log must exist and be non-empty after clean close.
    let cp_path = dir.path().join("checkpoint.log");
    assert!(cp_path.exists(), "checkpoint.log must be written on close");
    let cp_len = std::fs::metadata(&cp_path).unwrap().len();
    assert!(
        cp_len >= 20,
        "at least one 20-byte checkpoint record expected"
    );
}

#[test]
fn store_reopens_correctly_after_clean_close_with_checkpoint() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let commit_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(obj("persist"), b"y".to_vec());
        let id = store.commit(tx, "actor", "persist").unwrap();
        store.close().unwrap(); // writes checkpoint
        id
    };

    let store2 = BinderStore::open(&path).unwrap();
    let v = store2.get(&obj("persist")).unwrap().unwrap();
    assert_eq!(v.commit_id, commit_id);
    store2.close().unwrap();
}
