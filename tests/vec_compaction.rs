use bindb::{BinderStore, ObjectId, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

/// After writing 5 objects and overwriting 3 of them, the live Vec should
/// hold exactly 5 entries (all live) and historical queries should work.
#[test]
fn live_only_vec_after_overwrites() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Write 5 distinct objects.
    let mut commit_ids = Vec::new();
    for i in 0..5usize {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("ns/obj{i}")), vec![i as u8]);
        commit_ids.push(store.commit(tx, "actor", format!("create {i}")).unwrap());
    }

    // Overwrite 3 of them (obj0, obj1, obj2).
    let mut overwrite_ids = Vec::new();
    for i in 0..3usize {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("ns/obj{i}")), vec![100 + i as u8]);
        overwrite_ids.push(store.commit(tx, "actor", format!("update {i}")).unwrap());
    }

    // list() must return exactly 5 live objects.
    let live = store.list("ns").unwrap();
    assert_eq!(live.len(), 5, "list must return exactly 5 live objects");

    // Each live object has the correct current payload.
    for i in 0..3usize {
        let v = store.get(&obj(format!("ns/obj{i}"))).unwrap().expect("live");
        assert_eq!(
            v.payload.as_deref(),
            Some([100 + i as u8].as_slice()),
            "obj{i} must show updated payload"
        );
    }
    for i in 3..5usize {
        let v = store.get(&obj(format!("ns/obj{i}"))).unwrap().expect("live");
        assert_eq!(
            v.payload.as_deref(),
            Some([i as u8].as_slice()),
            "obj{i} must show original payload"
        );
    }

    // Historical queries via object_as_of must still return old values.
    for i in 0..3usize {
        let old = store
            .object_as_of(&obj(format!("ns/obj{i}")), commit_ids[i])
            .unwrap()
            .expect("historical version must exist");
        assert_eq!(
            old.payload.as_deref(),
            Some([i as u8].as_slice()),
            "historical payload must be original for obj{i}"
        );
    }

    store.close().unwrap();
}

/// After writing an object and deleting it, then writing another object,
/// list() must return only the alive object.
#[test]
fn tombstone_removed_from_list() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let oid_del = obj("ns/deleted");
    let oid_alive = obj("ns/alive");

    // Write and then delete an object.
    let mut tx1 = Transaction::new();
    tx1.put_object(oid_del.clone(), b"will-be-deleted".to_vec());
    store.commit(tx1, "actor", "create deleted").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(oid_del.clone());
    store.commit(tx2, "actor", "delete").unwrap();

    // Write a second object that stays alive.
    let mut tx3 = Transaction::new();
    tx3.put_object(oid_alive.clone(), b"alive-data".to_vec());
    store.commit(tx3, "actor", "create alive").unwrap();

    // get() on the deleted object must return None.
    let result = store.get(&oid_del).unwrap();
    assert!(result.is_none(), "deleted object must not be returned by get");

    // list() must return only the alive object.
    let live = store.list("ns").unwrap();
    assert_eq!(live.len(), 1, "list must return exactly one live object");
    assert_eq!(live[0].object_id, oid_alive);
    assert_eq!(
        live[0].payload.as_deref(),
        Some(b"alive-data".as_slice())
    );

    store.close().unwrap();
}

/// Write an object, overwrite it, close the store, reopen it, then verify
/// both the live get() and historical object_as_of() work correctly.
#[test]
fn reopen_after_vec_compaction() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let (c1, _c2) = {
        let store = BinderStore::open(&path).unwrap();
        let oid = obj("ns/reopen-obj");

        let mut tx1 = Transaction::new();
        tx1.put_object(oid.clone(), b"original".to_vec());
        let c1 = store.commit(tx1, "actor", "v1").unwrap();

        let mut tx2 = Transaction::new();
        tx2.put_object(oid.clone(), b"updated".to_vec());
        let c2 = store.commit(tx2, "actor", "v2").unwrap();

        store.close().unwrap();
        (c1, c2)
    };

    // Reopen: live-only Vec is rebuilt from the snapshot.
    let store2 = BinderStore::open(&path).unwrap();
    let oid = obj("ns/reopen-obj");

    // Live get() must return the updated version.
    let live = store2.get(&oid).unwrap().expect("live after reopen");
    assert_eq!(
        live.payload.as_deref(),
        Some(b"updated".as_slice()),
        "live payload must be updated after reopen"
    );
    assert!(live.valid_to.is_none(), "live version must have no valid_to");

    // Historical object_as_of() at c1 must return the original.
    let historical = store2
        .object_as_of(&oid, c1)
        .unwrap()
        .expect("historical version must exist after reopen");
    assert_eq!(
        historical.payload.as_deref(),
        Some(b"original".as_slice()),
        "historical payload must be original after reopen"
    );
    assert!(
        historical.valid_to.is_some(),
        "historical version must have valid_to set"
    );

    // Full history must have both versions.
    let history = store2.object_history(&oid).unwrap();
    assert_eq!(history.len(), 2, "must have 2 versions in history after reopen");
    assert_eq!(history[0].payload.as_deref(), Some(b"original".as_slice()));
    assert_eq!(history[1].payload.as_deref(), Some(b"updated".as_slice()));

    store2.close().unwrap();
}
