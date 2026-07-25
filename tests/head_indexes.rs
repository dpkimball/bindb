use bindb::{BinderStore, EdgeId, ObjectId, Transaction};
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

// ─── LRU eviction + VersionStore fallback ─────────────────────────────────────

/// Insert more objects than the head cache cap; every get() must still return
/// the correct payload via the VersionStore fallback path.
#[test]
fn evicted_objects_still_accessible_via_version_store() {
    let dir = tmp();
    // Cap of 5: inserting 20 objects forces 15 evictions.
    let store = BinderStore::open_with_head_cap(dir.path(), 5).unwrap();

    for i in 0u8..20 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("obj:{i}")), vec![i]);
        store.commit(tx, "actor", format!("c{i}")).unwrap();
    }

    for i in 0u8..20 {
        let v = store
            .get(&ObjectId::new(format!("obj:{i}")))
            .unwrap()
            .expect("object must be accessible after eviction");
        assert_eq!(
            v.payload.as_deref(),
            Some(&[i][..]),
            "payload must match for obj:{i}"
        );
    }

    store.close().unwrap();
}

/// list() must return all live objects even when most head entries are evicted.
#[test]
fn list_returns_all_objects_with_evicted_cache() {
    let dir = tmp();
    let store = BinderStore::open_with_head_cap(dir.path(), 3).unwrap();

    for i in 0u8..10 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("ns://obj:{i}")), vec![i]);
        store.commit(tx, "actor", format!("{i}")).unwrap();
    }

    let results = store.list("ns://").unwrap();
    assert_eq!(results.len(), 10, "list must return all 10 objects");

    store.close().unwrap();
}

/// Updating an evicted object re-caches it and get() returns the new payload.
#[test]
fn update_evicted_object_recaches_it() {
    let dir = tmp();
    let store = BinderStore::open_with_head_cap(dir.path(), 2).unwrap();

    for i in 0u8..5 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("o{i}")), vec![i]);
        store.commit(tx, "actor", format!("{i}")).unwrap();
    }

    let mut tx_update = Transaction::new();
    tx_update.put_object(ObjectId::new("o0"), b"updated".to_vec());
    store.commit(tx_update, "actor", "update-o0").unwrap();

    let v = store
        .get(&ObjectId::new("o0"))
        .unwrap()
        .expect("o0 must be found");
    assert_eq!(v.payload.as_deref(), Some(b"updated".as_slice()));

    store.close().unwrap();
}

/// Tombstoned objects must not appear in get() even when evicted before deletion.
#[test]
fn tombstone_on_evicted_object_returns_none() {
    let dir = tmp();
    let store = BinderStore::open_with_head_cap(dir.path(), 1).unwrap();

    for i in 0u8..3 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("o{i}")), vec![i]);
        store.commit(tx, "actor", format!("{i}")).unwrap();
    }

    let mut tx_del = Transaction::new();
    tx_del.delete_object(ObjectId::new("o0"));
    store.commit(tx_del, "actor", "delete-o0").unwrap();

    assert!(
        store.get(&ObjectId::new("o0")).unwrap().is_none(),
        "tombstoned evicted object must return None"
    );
    assert!(
        store.get(&ObjectId::new("o1")).unwrap().is_some(),
        "evicted non-tombstoned object must still be readable"
    );

    store.close().unwrap();
}

/// edges_from returns all edges even when edge head entries are evicted.
#[test]
fn evicted_edges_still_accessible_via_version_store() {
    let dir = tmp();
    let store = BinderStore::open_with_head_cap(dir.path(), 1).unwrap();

    let mut tx = Transaction::new();
    for i in 0u8..3 {
        tx.put_object(ObjectId::new(format!("node:{i}")), vec![i]);
    }
    store.commit(tx, "actor", "nodes").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        EdgeId::new("edge:0"),
        ObjectId::new("node:0"),
        ObjectId::new("node:1"),
        "link",
        Some(b"e0".to_vec()),
    );
    store.commit(tx2, "actor", "edge0").unwrap();

    let mut tx3 = Transaction::new();
    tx3.put_edge(
        EdgeId::new("edge:1"),
        ObjectId::new("node:0"),
        ObjectId::new("node:2"),
        "link",
        Some(b"e1".to_vec()),
    );
    store.commit(tx3, "actor", "edge1").unwrap();

    let edges = store.edges_from(&ObjectId::new("node:0")).unwrap();
    assert_eq!(
        edges.len(),
        2,
        "both edges must be returned even if evicted"
    );

    store.close().unwrap();
}

/// rebuild_indexes must produce a complete namespace index even with a tiny cap.
#[test]
fn rebuild_indexes_correct_with_small_cap() {
    let dir = tmp();
    let store = BinderStore::open_with_head_cap(dir.path(), 2).unwrap();

    for i in 0u8..8 {
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new(format!("ns://o{i}")), vec![i]);
        store.commit(tx, "actor", format!("{i}")).unwrap();
    }

    store.rebuild_indexes().unwrap();

    let results = store.list("ns://").unwrap();
    assert_eq!(results.len(), 8);

    store.close().unwrap();
}
