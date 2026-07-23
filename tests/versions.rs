use bindb::{BinderStore, EdgeId, ObjectId, Provenance, ProvenanceSource, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ─── object version history ───────────────────────────────────────────────────

#[test]
fn single_object_has_one_version() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(ObjectId::new("obj-a"), b"v1".to_vec());
    let commit_id = store.commit(tx, "actor", "first").unwrap();

    let history = store.object_history(&ObjectId::new("obj-a")).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].commit_id, commit_id);
    assert_eq!(history[0].valid_from, commit_id);
    assert!(
        history[0].valid_to.is_none(),
        "only version must be current"
    );
    assert_eq!(history[0].payload.as_deref(), Some(b"v1".as_slice()));
    assert!(!history[0].is_tombstone());
    assert!(history[0].is_current());

    store.close().unwrap();
}

#[test]
fn second_write_retires_first_version() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx1 = Transaction::new();
    tx1.put_object(ObjectId::new("obj-b"), b"v1".to_vec());
    let c1 = store.commit(tx1, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(ObjectId::new("obj-b"), b"v2".to_vec());
    let c2 = store.commit(tx2, "actor", "v2").unwrap();

    let history = store.object_history(&ObjectId::new("obj-b")).unwrap();
    assert_eq!(history.len(), 2);

    // v1 must now have valid_to = c2
    assert_eq!(history[0].valid_from, c1);
    assert_eq!(history[0].valid_to, Some(c2), "v1 must be retired by c2");
    assert!(!history[0].is_current());

    // v2 is current
    assert_eq!(history[1].valid_from, c2);
    assert!(history[1].valid_to.is_none());
    assert!(history[1].is_current());
    assert_eq!(history[1].payload.as_deref(), Some(b"v2".as_slice()));

    store.close().unwrap();
}

#[test]
fn three_versions_chain_correctly() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = ObjectId::new("obj-chain");

    let mut ids = Vec::new();
    for i in 0..3u8 {
        let mut tx = Transaction::new();
        tx.put_object(oid.clone(), vec![i]);
        ids.push(store.commit(tx, "actor", format!("v{i}")).unwrap());
    }

    let history = store.object_history(&oid).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].valid_to, Some(ids[1]));
    assert_eq!(history[1].valid_to, Some(ids[2]));
    assert!(history[2].valid_to.is_none());

    store.close().unwrap();
}

#[test]
fn unrelated_objects_histories_are_independent() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(ObjectId::new("x"), b"x".to_vec());
    tx.put_object(ObjectId::new("y"), b"y".to_vec());
    store.commit(tx, "actor", "both").unwrap();

    let hist_x = store.object_history(&ObjectId::new("x")).unwrap();
    let hist_y = store.object_history(&ObjectId::new("y")).unwrap();
    assert_eq!(hist_x.len(), 1);
    assert_eq!(hist_y.len(), 1);
    assert!(hist_x[0].is_current());
    assert!(hist_y[0].is_current());

    store.close().unwrap();
}

// ─── tombstone ────────────────────────────────────────────────────────────────

#[test]
fn delete_object_creates_tombstone() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = ObjectId::new("to-delete");

    let mut tx1 = Transaction::new();
    tx1.put_object(oid.clone(), b"alive".to_vec());
    let c1 = store.commit(tx1, "actor", "create").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(oid.clone());
    let c2 = store.commit(tx2, "actor", "delete").unwrap();

    let history = store.object_history(&oid).unwrap();
    assert_eq!(history.len(), 2);

    // original version retired
    assert_eq!(history[0].valid_from, c1);
    assert_eq!(history[0].valid_to, Some(c2));

    // tombstone is current
    assert_eq!(history[1].valid_from, c2);
    assert!(history[1].valid_to.is_none(), "tombstone must be current");
    assert!(history[1].is_tombstone(), "payload must be None");
    assert!(history[1].is_current());

    store.close().unwrap();
}

#[test]
fn tombstone_history_survives_crash_recovery() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let (c1, c2) = {
        let store = BinderStore::open(&path).unwrap();
        let oid = ObjectId::new("crash-delete");

        let mut tx1 = Transaction::new();
        tx1.put_object(oid.clone(), b"data".to_vec());
        let c1 = store.commit(tx1, "actor", "create").unwrap();

        let mut tx2 = Transaction::new();
        tx2.delete_object(oid.clone());
        let c2 = store.commit(tx2, "actor", "delete").unwrap();

        drop(store); // crash
        (c1, c2)
    };

    let store2 = BinderStore::open(&path).unwrap();
    let history = store2
        .object_history(&ObjectId::new("crash-delete"))
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].valid_from, c1);
    assert_eq!(history[0].valid_to, Some(c2));
    assert!(history[1].is_tombstone());
    assert!(history[1].is_current());
    store2.close().unwrap();
}

// ─── provenance ───────────────────────────────────────────────────────────────

#[test]
fn provenance_stored_and_recovered() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let prov = Provenance {
        source: ProvenanceSource::Plugin("story-binder".into()),
        source_ref: "github://org/repo".into(),
        imported_at_ms: 1_700_000_000_000,
    };

    let commit_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object_with_provenance(ObjectId::new("obj-prov"), b"payload".to_vec(), Some(prov));
        let id = store.commit(tx, "actor", "with provenance").unwrap();
        drop(store); // crash
        id
    };

    let store2 = BinderStore::open(&path).unwrap();
    let history = store2.object_history(&ObjectId::new("obj-prov")).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].commit_id, commit_id);
    let p = history[0]
        .provenance
        .as_ref()
        .expect("provenance must survive recovery");
    assert_eq!(p.source_ref, "github://org/repo");
    assert_eq!(p.imported_at_ms, 1_700_000_000_000);
    store2.close().unwrap();
}

// ─── edge version history ─────────────────────────────────────────────────────

#[test]
fn edge_versions_independent_from_object_versions() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Write object and edge in same commit.
    let mut tx1 = Transaction::new();
    tx1.put_object(ObjectId::new("node-a"), b"a".to_vec());
    tx1.put_edge(
        EdgeId::new("edge-ab"),
        ObjectId::new("node-a"),
        ObjectId::new("node-b"),
        "REFERENCES",
        None,
    );
    let c1 = store.commit(tx1, "actor", "first").unwrap();

    // Update only the object — edge must remain current independently.
    let mut tx2 = Transaction::new();
    tx2.put_object(ObjectId::new("node-a"), b"a-v2".to_vec());
    let c2 = store.commit(tx2, "actor", "update object only").unwrap();

    let obj_history = store.object_history(&ObjectId::new("node-a")).unwrap();
    assert_eq!(obj_history.len(), 2);
    assert_eq!(obj_history[0].valid_to, Some(c2));
    assert!(obj_history[1].is_current());

    let edge_history = store.edge_history(&EdgeId::new("edge-ab")).unwrap();
    assert_eq!(
        edge_history.len(),
        1,
        "edge was not touched; must have exactly one version"
    );
    assert_eq!(edge_history[0].valid_from, c1);
    assert!(
        edge_history[0].valid_to.is_none(),
        "edge must still be current"
    );

    store.close().unwrap();
}

#[test]
fn delete_edge_creates_tombstone() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let eid = EdgeId::new("edge-to-drop");

    let mut tx1 = Transaction::new();
    tx1.put_edge(
        eid.clone(),
        ObjectId::new("from"),
        ObjectId::new("to"),
        "SAME_AS",
        None,
    );
    let c1 = store.commit(tx1, "actor", "create edge").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_edge(eid.clone());
    let c2 = store.commit(tx2, "actor", "delete edge").unwrap();

    let history = store.edge_history(&eid).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].valid_to, Some(c2));
    assert!(history[1].is_tombstone());
    assert!(history[1].is_current());

    // Verify valid_from chain
    assert_eq!(history[0].valid_from, c1);
    assert_eq!(history[1].valid_from, c2);

    store.close().unwrap();
}

#[test]
fn unknown_object_has_empty_history() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let history = store
        .object_history(&ObjectId::new("does-not-exist"))
        .unwrap();
    assert!(history.is_empty());
    store.close().unwrap();
}
