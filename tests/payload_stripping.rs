use bindb::{BinderStore, EdgeId, ObjectId, Provenance, ProvenanceSource, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

fn eid(s: impl Into<String>) -> EdgeId {
    EdgeId::new(s)
}

// ─── payload stripping ───────────────────────────────────────────────────────

/// After overwriting an object, the retired version's payload is None in RAM
/// (payload_stripped=true), but the live version's payload is intact.
#[test]
fn retired_version_payload_stripped_from_ram() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-a");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"v1-payload".to_vec());
    store.commit(tx, "actor", "v1").unwrap();

    // Overwrite — this should retire v1 and strip its payload from the Vec.
    let mut tx2 = Transaction::new();
    tx2.put_object(oid.clone(), b"v2-payload".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    // Live version is intact.
    let live = store.get(&oid).unwrap().expect("live");
    assert_eq!(live.payload.as_deref(), Some(b"v2-payload".as_slice()));
    assert!(!live.payload_stripped);
    assert!(!live.is_tombstone());
}

/// object_as_of still works after payload stripping: history query reads from
/// version_log.bin not the in-memory Vec.
#[test]
fn object_as_of_works_after_payload_stripping() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-b");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"original".to_vec());
    let c1 = store.commit(tx, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid.clone(), b"updated".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    // as_of the first commit must still return the original payload.
    let historical = store.object_as_of(&oid, c1).unwrap().expect("must exist");
    assert_eq!(historical.payload.as_deref(), Some(b"original".as_slice()));
}

/// object_history returns all versions with correct payloads after stripping.
#[test]
fn object_history_works_after_payload_stripping() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-c");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"v1".to_vec());
    store.commit(tx, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid.clone(), b"v2".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    let mut tx3 = Transaction::new();
    tx3.put_object(oid.clone(), b"v3".to_vec());
    store.commit(tx3, "actor", "v3").unwrap();

    let history = store.object_history(&oid).unwrap();
    assert_eq!(history.len(), 3);
    let payloads: Vec<&[u8]> = history
        .iter()
        .map(|v| v.payload.as_deref().unwrap())
        .collect();
    assert_eq!(payloads, [b"v1".as_slice(), b"v2", b"v3"]);
}

/// Tombstones are NOT treated as stripped: deleting an object leaves
/// is_tombstone() == true on the live version.
#[test]
fn tombstone_not_confused_with_stripped() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-d");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"data".to_vec());
    store.commit(tx, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.delete_object(oid.clone());
    store.commit(tx2, "actor", "del").unwrap();

    let result = store.get(&oid).unwrap();
    assert!(result.is_none(), "deleted object returns None from get()");
}

/// rebuild_indexes succeeds when the Vec contains stripped payloads.
/// All queries remain correct afterward.
#[test]
fn rebuild_indexes_correct_with_stripped_payloads() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-e");

    let mut tx = Transaction::new();
    tx.put_object(oid.clone(), b"v1".to_vec());
    let c1 = store.commit(tx, "actor", "v1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_object(oid.clone(), b"v2".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    // rebuild_indexes must not corrupt version_log.bin even though v1 payload
    // is stripped from the in-memory Vec.
    store.rebuild_indexes().unwrap();

    // Live version still correct.
    let live = store.get(&oid).unwrap().expect("live");
    assert_eq!(live.payload.as_deref(), Some(b"v2".as_slice()));

    // Historical query still correct.
    let hist = store.object_as_of(&oid, c1).unwrap().expect("hist");
    assert_eq!(hist.payload.as_deref(), Some(b"v1".as_slice()));
}

/// compact() + reopen: after compacting a store with stripped payloads,
/// reopening it produces correct results for both live and historical queries.
#[test]
fn compact_and_reopen_with_stripped_payloads() {
    let dir = tmp();
    {
        let store = BinderStore::open(dir.path()).unwrap();
        let oid = obj("ns/obj-f");

        let mut tx = Transaction::new();
        tx.put_object(oid.clone(), b"v1-data".to_vec());
        store.commit(tx, "actor", "v1").unwrap();

        let mut tx2 = Transaction::new();
        tx2.put_object(oid.clone(), b"v2-data".to_vec());
        store.commit(tx2, "actor", "v2").unwrap();

        store.compact().unwrap();
        store.close().unwrap();
    }

    // Reopen.
    let store2 = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/obj-f");

    let live = store2.get(&oid).unwrap().expect("live after reopen");
    assert_eq!(live.payload.as_deref(), Some(b"v2-data".as_slice()));

    let history = store2.object_history(&oid).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].payload.as_deref(), Some(b"v1-data".as_slice()));
    assert_eq!(history[1].payload.as_deref(), Some(b"v2-data".as_slice()));
}

/// Edge payloads are also stripped from retired versions.
#[test]
fn edge_payload_stripped_after_retirement() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let eid = eid("e1");
    let from = obj("ns/from");
    let to = obj("ns/to");

    // Create the endpoints first.
    let mut tx0 = Transaction::new();
    tx0.put_object(from.clone(), b"from".to_vec());
    tx0.put_object(to.clone(), b"to".to_vec());
    store.commit(tx0, "actor", "nodes").unwrap();

    let mut tx = Transaction::new();
    tx.put_edge(
        eid.clone(),
        from.clone(),
        to.clone(),
        "rel",
        Some(b"e-v1".to_vec()),
    );
    let c1 = store.commit(tx, "actor", "e1").unwrap();

    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid.clone(),
        from.clone(),
        to.clone(),
        "rel",
        Some(b"e-v2".to_vec()),
    );
    store.commit(tx2, "actor", "e2").unwrap();

    // Historical edge query still works.
    let hist = store.edge_as_of(&eid, c1).unwrap().expect("hist edge");
    assert_eq!(hist.payload.as_deref(), Some(b"e-v1".as_slice()));
}

/// Many rewrites accumulate stripped entries; list() returns all live objects.
#[test]
fn list_correct_after_many_rewrites() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let n = 50usize;

    // Write initial versions.
    for i in 0..n {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("ns/o{i}")), vec![i as u8; 4]);
        store.commit(tx, "actor", "v1").unwrap();
    }

    // Overwrite half of them (strips payloads).
    for i in 0..(n / 2) {
        let mut tx = Transaction::new();
        tx.put_object(obj(format!("ns/o{i}")), vec![100 + i as u8; 4]);
        store.commit(tx, "actor", "v2").unwrap();
    }

    let ids = store.list("ns").unwrap();
    assert_eq!(ids.len(), n);
}

/// Provenance heap strings are cleared from retired entries but recovered
/// correctly when object_history / rebuild_indexes / compact+reopen is used.
#[test]
fn provenance_recovered_after_stripping() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/prov-obj");

    let prov = Provenance {
        source: ProvenanceSource::Import("health-kit".to_string()),
        source_ref: "HKQuantityTypeIdentifierHeartRate".to_string(),
        imported_at_ms: 1_700_000_000_000,
    };

    let mut tx = Transaction::new();
    tx.put_object_with_provenance(oid.clone(), b"v1".to_vec(), Some(prov.clone()));
    let c1 = store.commit(tx, "actor", "v1").unwrap();

    // Overwrite — retires v1, stripping its provenance from RAM.
    let mut tx2 = Transaction::new();
    tx2.put_object(oid.clone(), b"v2".to_vec());
    store.commit(tx2, "actor", "v2").unwrap();

    // Historical query reads provenance from VersionStore (disk), not RAM.
    let hist = store.object_as_of(&oid, c1).unwrap().expect("hist");
    assert_eq!(
        hist.provenance.as_ref().map(|p| p.source_ref.as_str()),
        Some("HKQuantityTypeIdentifierHeartRate")
    );

    // rebuild_indexes must recover provenance before rewriting version_log.bin.
    store.rebuild_indexes().unwrap();
    let hist2 = store
        .object_as_of(&oid, c1)
        .unwrap()
        .expect("hist after rebuild");
    assert_eq!(
        hist2.provenance.as_ref().map(|p| p.source_ref.as_str()),
        Some("HKQuantityTypeIdentifierHeartRate")
    );
}

/// compact() + reopen: provenance on retired versions survives the snapshot cycle.
#[test]
fn provenance_survives_compact_reopen() {
    let dir = tmp();
    let prov = Provenance {
        source: ProvenanceSource::Plugin("binder-health".to_string()),
        source_ref: "steps-2026-01-01".to_string(),
        imported_at_ms: 1_750_000_000_000,
    };

    let c1 = {
        let store = BinderStore::open(dir.path()).unwrap();
        let oid = obj("ns/prov-compact");

        let mut tx = Transaction::new();
        tx.put_object_with_provenance(oid.clone(), b"original".to_vec(), Some(prov.clone()));
        let c1 = store.commit(tx, "actor", "v1").unwrap();

        let mut tx2 = Transaction::new();
        tx2.put_object(oid.clone(), b"updated".to_vec());
        store.commit(tx2, "actor", "v2").unwrap();

        store.compact().unwrap();
        store.close().unwrap();
        c1
    };

    let store2 = BinderStore::open(dir.path()).unwrap();
    let oid = obj("ns/prov-compact");
    let hist = store2.object_as_of(&oid, c1).unwrap().expect("hist");
    assert_eq!(
        hist.provenance.as_ref().map(|p| p.source_ref.as_str()),
        Some("steps-2026-01-01")
    );
}
