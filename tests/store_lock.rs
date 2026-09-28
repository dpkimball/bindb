//! Cross-process exclusive write flock on BinderStore::open (#111).

use bindb::{BinderStore, StoreError};
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn child_mode() -> bool {
    env::var_os("BINDB_STORE_LOCK_CHILD").is_some()
}

#[test]
fn second_process_gets_store_locked() {
    if child_mode() {
        let path = PathBuf::from(env::var("BINDB_STORE_LOCK_PATH").expect("path"));
        match BinderStore::open(&path) {
            Err(StoreError::StoreLocked) => std::process::exit(0),
            Ok(_) => std::process::exit(10),
            Err(_) => std::process::exit(11),
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let _holder = BinderStore::open(&path).expect("first open");

    let exe = env::current_exe().expect("current_exe");
    let status = Command::new(&exe)
        .arg("--exact")
        .arg("second_process_gets_store_locked")
        .env("BINDB_STORE_LOCK_CHILD", "1")
        .env("BINDB_STORE_LOCK_PATH", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawn child");

    assert!(
        status.success(),
        "child should exit 0 on StoreLocked, got {status:?}"
    );
}

#[test]
fn open_succeeds_after_holder_dropped() {
    if child_mode() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    {
        let _holder = BinderStore::open(&path).expect("first open");
    }
    // Brief pause so OS releases flock after drop on some platforms.
    thread::sleep(Duration::from_millis(50));
    let _again = BinderStore::open(&path).expect("reopen after drop");
}
