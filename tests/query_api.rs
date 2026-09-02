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

// ─── full-text search projection ─────────────────────────────────────────────

#[test]
fn text_search_finds_indexed_object() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid = obj("doc://a");
    store
        .index_text(&oid, "Rust is a systems programming language")
        .unwrap();

    let results = store.text_search("systems").unwrap();
    assert!(
        results.contains(&oid),
        "indexed object must appear in results"
    );
    store.close().unwrap();
}

#[test]
fn text_search_is_case_insensitive() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid = obj("doc://b");
    store.index_text(&oid, "Hello World").unwrap();

    assert!(!store.text_search("hello world").unwrap().is_empty());
    assert!(!store.text_search("HELLO").unwrap().is_empty());
    assert!(!store.text_search("WoRlD").unwrap().is_empty());
    store.close().unwrap();
}

#[test]
fn text_search_returns_empty_when_no_match() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    store
        .index_text(&obj("doc://c"), "The quick brown fox")
        .unwrap();

    let results = store.text_search("elephant").unwrap();
    assert!(results.is_empty(), "non-matching query must return empty");
    store.close().unwrap();
}

#[test]
fn text_search_returns_multiple_matches() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let a = obj("doc://d1");
    let b = obj("doc://d2");
    let c = obj("doc://d3");
    store.index_text(&a, "apple pie recipe").unwrap();
    store.index_text(&b, "baked apple crumble").unwrap();
    store.index_text(&c, "banana smoothie").unwrap();

    let results = store.text_search("apple").unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.contains(&a));
    assert!(results.contains(&b));
    assert!(!results.contains(&c));
    store.close().unwrap();
}

#[test]
fn index_text_replaces_previous_entry() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid = obj("doc://e");
    store.index_text(&oid, "original text").unwrap();
    store.index_text(&oid, "replacement content").unwrap();

    assert!(store.text_search("original").unwrap().is_empty());
    assert_eq!(store.text_search("replacement").unwrap(), vec![oid]);
    store.close().unwrap();
}

#[test]
fn clear_text_index_removes_entry() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid = obj("doc://f");
    store.index_text(&oid, "to be removed").unwrap();
    store.clear_text_index(&oid).unwrap();

    assert!(store.text_search("removed").unwrap().is_empty());
    store.close().unwrap();
}

#[test]
fn index_text_empty_string_clears_entry() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid = obj("doc://g");
    store.index_text(&oid, "some content").unwrap();
    store.index_text(&oid, "").unwrap(); // empty string should clear

    assert!(store.text_search("content").unwrap().is_empty());
    store.close().unwrap();
}

#[test]
fn text_search_on_empty_index_returns_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let results = store.text_search("anything").unwrap();
    assert!(results.is_empty());
    store.close().unwrap();
}

#[test]
fn list_object_ids_matches_list_without_payloads() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ns://one"), b"a".to_vec());
    tx.put_object(obj("ns://two"), b"b".to_vec());
    tx.put_object(obj("other://z"), b"c".to_vec());
    store.commit(tx, "actor", "seed").unwrap();

    let mut ids = store.list_object_ids("ns://").unwrap();
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].0, "ns://one");
    assert_eq!(ids[1].0, "ns://two");

    let versions = store.list("ns://").unwrap();
    assert_eq!(versions.len(), 2);

    store.close().unwrap();
}
