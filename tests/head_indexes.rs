use bindb::{BinderStore, ObjectId, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ─── HEAD object index ────────────────────────────────────────────────────────

#[test]
fn get_returns_current_version() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = ObjectId::new("obj-a");

    assert!(store.get(&oid).unwrap().is_none(), "unknown object is None");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"v1".to_vec());
    let c1 = store.commit(tx, "actor", "first").unwrap();

    let v = store.get(&oid).unwrap().expect("must be Some after commit");
    assert_eq!(v.commit_id, c1);
    assert_eq!(v.payload.as_deref(), Some(b"v1".as_slice()));
    assert!(v.is_current());
    assert!(!v.is_tombstone());

    store.close().unwrap();
}

#[test]
fn get_tracks_latest_version() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = ObjectId::new("obj-track");

    for i in 0u8..3 {
        let mut tx = Transaction::new();
        tx.put_object(oid.clone(), vec![i]);
        store.commit(tx, "actor", format!("v{i}")).unwrap();
    }

    let v = store.get(&oid).unwrap().unwrap();
    assert_eq!(v.payload.as_deref(), Some(&[2u8][..]));
    store.close().unwrap();
}

#[test]
fn get_returns_none_after_delete() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = ObjectId::new("obj-del");

    let mut tx1 = Transaction::new();
    tx1.put_object(oid.clone(), b"data".to_vec());
    store.commit(tx1, "actor", "create").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(oid.clone());
    store.commit(tx2, "actor", "delete").unwrap();

    assert!(
        store.get(&oid).unwrap().is_none(),
        "tombstone must look like None to get()"
    );
    store.close().unwrap();
}

#[test]
fn head_index_rebuilt_identically_after_explicit_rebuild() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    for i in 0u8..5 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("o{i}")), vec![i]);
        store.commit(tx, "actor", format!("c{i}")).unwrap();
    }

    // Capture current state.
    let before: Vec<_> = (0u8..5)
        .map(|i| store.get(&ObjectId::new(format!("o{i}"))).unwrap())
        .collect();

    // Rebuild.
    store.rebuild_indexes().unwrap();

    // Must be identical.
    for (i, pre) in before.iter().enumerate() {
        let post = store.get(&ObjectId::new(format!("o{i}"))).unwrap();
        assert_eq!(
            pre.as_ref().map(|v| &v.commit_id),
            post.as_ref().map(|v| &v.commit_id)
        );
    }

    store.close().unwrap();
}

#[test]
fn head_index_intact_after_crash_recovery() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let ids: Vec<_> = {
        let store = BinderStore::open(&path).unwrap();
        (0u8..4)
            .map(|i| {
                let mut tx = Transaction::new();
                tx.put_object(ObjectId::new(format!("o{i}")), vec![i]);
                store.commit(tx, "actor", format!("{i}")).unwrap()
            })
            .collect()
        // drop = crash
    };

    let store2 = BinderStore::open(&path).unwrap();
    for (i, expected_cid) in ids.iter().enumerate() {
        let v = store2
            .get(&ObjectId::new(format!("o{i}")))
            .unwrap()
            .unwrap();
        assert_eq!(&v.commit_id, expected_cid);
    }
    store2.close().unwrap();
}

// ─── list ─────────────────────────────────────────────────────────────────────

#[test]
fn list_filters_by_namespace_prefix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(ObjectId::new("story://a"), b"a".to_vec());
    tx.put_object(ObjectId::new("story://b"), b"b".to_vec());
    tx.put_object(ObjectId::new("memory://x"), b"x".to_vec());
    store.commit(tx, "actor", "mixed").unwrap();

    let story_objs = store.list("story://").unwrap();
    assert_eq!(story_objs.len(), 2);
    assert!(story_objs
        .iter()
        .all(|v| v.object_id.0.starts_with("story://")));

    let memory_objs = store.list("memory://").unwrap();
    assert_eq!(memory_objs.len(), 1);

    let empty = store.list("other://").unwrap();
    assert!(empty.is_empty());

    store.close().unwrap();
}

#[test]
fn list_excludes_tombstones() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_object(ObjectId::new("story://live"), b"x".to_vec());
    tx1.put_object(ObjectId::new("story://dead"), b"y".to_vec());
    store.commit(tx1, "actor", "create").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(ObjectId::new("story://dead"));
    store.commit(tx2, "actor", "delete").unwrap();

    let objs = store.list("story://").unwrap();
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0].object_id.0, "story://live");

    store.close().unwrap();
}
