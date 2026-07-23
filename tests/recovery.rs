use bindb::{BinderStore, EdgeId, ObjectId, StoreError, Transaction};
use std::fs;
use std::io::Write;
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ─── clean open ────────────────────────────────────────────────────────────────

#[test]
fn clean_open_has_no_commits() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    assert!(store.scan_commits().unwrap().is_empty());
    assert!(store.head().unwrap().is_none());
    store.close().unwrap();
}

// ─── dirty marker ─────────────────────────────────────────────────────────────

#[test]
fn dirty_marker_exists_while_open() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    assert!(
        dir.path().join(".dirty").exists(),
        ".dirty must exist while store is open"
    );
    store.close().unwrap();
}

#[test]
fn clean_close_removes_dirty_marker() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store.close().unwrap();
    assert!(
        !dir.path().join(".dirty").exists(),
        ".dirty must be removed on clean close"
    );
}

// ─── crash recovery ───────────────────────────────────────────────────────────

#[test]
fn commit_survives_simulated_crash() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    // Write a commit then drop the store without calling close() (simulate crash).
    let commit_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("obj-crash"), b"value".to_vec());
        let id = store.commit(tx, "test-actor", "crash test commit").unwrap();
        // Drop without close — dirty marker remains, simulates crash.
        id
    };

    // Dirty marker must still be present.
    assert!(
        path.join(".dirty").exists(),
        ".dirty must persist after crash"
    );

    // Reopen and verify the commit is recovered.
    let store2 = BinderStore::open(&path).unwrap();
    let commits = store2.scan_commits().unwrap();
    assert_eq!(commits.len(), 1, "recovered commit count must be 1");
    assert_eq!(
        commits[0].commit_id, commit_id,
        "recovered commit_id must match"
    );
    assert_eq!(store2.head().unwrap(), Some(commit_id));
    store2.close().unwrap();
}

#[test]
fn multiple_commits_all_recovered_after_crash() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let ids: Vec<_> = {
        let store = BinderStore::open(&path).unwrap();
        (0..5)
            .map(|i| {
                let mut tx = Transaction::new();
                tx.put_object(
                    ObjectId::new(format!("obj-{i}")),
                    format!("val-{i}").into_bytes(),
                );
                store.commit(tx, "actor", format!("commit {i}")).unwrap()
            })
            .collect()
    };
    // Drop without close.

    let store2 = BinderStore::open(&path).unwrap();
    let commits = store2.scan_commits().unwrap();
    assert_eq!(commits.len(), 5);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(&commits[i].commit_id, id);
    }
    assert_eq!(store2.head().unwrap(), Some(*ids.last().unwrap()));
    store2.close().unwrap();
}

#[test]
fn reopened_store_continues_from_correct_head() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let first_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("first"), b"a".to_vec());
        let id = store.commit(tx, "actor", "first").unwrap();
        store.close().unwrap();
        id
    };

    let store2 = BinderStore::open(&path).unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_object(ObjectId::new("second"), b"b".to_vec());
    let second_id = store2.commit(tx2, "actor", "second").unwrap();

    let commits = store2.scan_commits().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(
        commits[1].parent_commit,
        Some(first_id),
        "second commit must point at first"
    );
    assert_eq!(store2.head().unwrap(), Some(second_id));
    store2.close().unwrap();
}

// ─── partial / corrupted WAL entries ─────────────────────────────────────────

#[test]
fn partial_wal_entry_not_visible_after_crash() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    // Write one valid commit.
    let good_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("good"), b"ok".to_vec());
        let id = store.commit(tx, "actor", "good commit").unwrap();
        drop(store); // crash
        id
    };

    // Append a partial frame: magic + entry_id only (no data_len, no data, no checksum).
    {
        let wal_path = path.join("commits.wal");
        let mut f = fs::OpenOptions::new().append(true).open(&wal_path).unwrap();
        f.write_all(b"BNDR").unwrap();
        f.write_all(&99u64.to_le_bytes()).unwrap();
        // Intentionally stop here — truncated entry.
        f.flush().unwrap();
    }

    // Reopen: the partial entry must be dropped; only the good commit survives.
    let store2 = BinderStore::open(&path).unwrap();
    let commits = store2.scan_commits().unwrap();
    assert_eq!(commits.len(), 1, "partial entry must not be visible");
    assert_eq!(commits[0].commit_id, good_id);
    store2.close().unwrap();
}

#[test]
fn corrupted_checksum_stops_replay() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let good_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("before-corrupt"), b"data".to_vec());
        let id = store.commit(tx, "actor", "before corrupt").unwrap();
        drop(store);
        id
    };

    // Write a second commit then corrupt its checksum.
    {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("will-corrupt"), b"bad".to_vec());
        store.commit(tx, "actor", "will corrupt").unwrap();
        drop(store);
    }

    // Flip the last 4 bytes of the WAL (the checksum of the second commit).
    {
        let wal_path = path.join("commits.wal");
        let mut bytes = fs::read(&wal_path).unwrap();
        let len = bytes.len();
        for b in &mut bytes[len - 4..] {
            *b ^= 0xFF;
        }
        fs::write(&wal_path, &bytes).unwrap();
    }

    // Reopen: corrupted entry is dropped; only the first commit survives.
    let store3 = BinderStore::open(&path).unwrap();
    let commits = store3.scan_commits().unwrap();
    assert_eq!(commits.len(), 1, "corrupted commit must not be visible");
    assert_eq!(commits[0].commit_id, good_id);
    store3.close().unwrap();
}

// ─── empty transaction ────────────────────────────────────────────────────────

#[test]
fn empty_transaction_is_rejected() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let tx = Transaction::new();
    let result = store.commit(tx, "actor", "empty");
    assert!(
        matches!(result, Err(StoreError::EmptyTransaction)),
        "empty transaction must be rejected"
    );
    assert!(
        store.scan_commits().unwrap().is_empty(),
        "no commit must be written"
    );
    store.close().unwrap();
}

// ─── edge round-trip ──────────────────────────────────────────────────────────

#[test]
fn edge_change_survives_crash_recovery() {
    let dir = tmp();
    let path = dir.path().to_path_buf();

    let commit_id = {
        let store = BinderStore::open(&path).unwrap();
        let mut tx = Transaction::new();
        tx.put_edge(
            EdgeId::new("edge-1"),
            ObjectId::new("from-obj"),
            ObjectId::new("to-obj"),
            "SAME_AS",
            None,
        );
        let id = store.commit(tx, "actor", "edge commit").unwrap();
        drop(store); // crash
        id
    };

    let store2 = BinderStore::open(&path).unwrap();
    let commits = store2.scan_commits().unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].commit_id, commit_id);
    assert_eq!(commits[0].relationship_changes.len(), 1);
    assert_eq!(
        commits[0].relationship_changes[0]
            .relationship_type
            .as_deref(),
        Some("SAME_AS")
    );
    store2.close().unwrap();
}
