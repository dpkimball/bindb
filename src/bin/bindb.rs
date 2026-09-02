//! Public `bindb` CLI (Phase 2: info + list).
//!
//! The executable is the public surface for binary distribution.
//! Internal Rust APIs remain private implementation details.

use bindb::BinderStore;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_SHA: &str = env!("BINDB_GIT_SHA");
const BUILD_DATE: &str = env!("BINDB_BUILD_DATE");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

fn run(args: &[String]) -> Result<(), ExitCode> {
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            print_help();
            Ok(())
        }
        "-V" | "--version" | "version" => {
            print_version();
            Ok(())
        }
        "info" => {
            let store_dir = args.get(1).ok_or_else(|| usage_exit("info <store-dir>"))?;
            cmd_info(Path::new(store_dir))
        }
        "list" => {
            let store_dir = args
                .get(1)
                .ok_or_else(|| usage_exit("list <store-dir> <namespace-prefix>"))?;
            let prefix = args
                .get(2)
                .ok_or_else(|| usage_exit("list <store-dir> <namespace-prefix>"))?;
            cmd_list(Path::new(store_dir), prefix)
        }
        other => {
            eprintln!("error: unknown argument '{other}'");
            eprintln!();
            print_help_to_stderr();
            Err(ExitCode::from(2))
        }
    }
}

fn usage_exit(hint: &str) -> ExitCode {
    eprintln!("error: usage: bindb {hint}");
    eprintln!();
    print_help_to_stderr();
    ExitCode::from(2)
}

fn require_existing_store(store_dir: &Path) -> Result<PathBuf, ExitCode> {
    let path = if store_dir.is_absolute() {
        store_dir.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|e| {
                eprintln!("error: {e}");
                ExitCode::from(1)
            })?
            .join(store_dir)
    };
    let wal = path.join("commits.wal");
    if !wal.is_file() {
        eprintln!(
            "error: not a bindb store (missing commits.wal): {}",
            path.display()
        );
        return Err(ExitCode::from(1));
    }
    Ok(path)
}

fn cmd_info(store_dir: &Path) -> Result<(), ExitCode> {
    let path = require_existing_store(store_dir)?;
    let wal_bytes = fs::metadata(path.join("commits.wal"))
        .map(|m| m.len())
        .map_err(|e| {
            eprintln!("error: reading commits.wal: {e}");
            ExitCode::from(1)
        })?;

    let store = BinderStore::open(&path).map_err(|e| {
        eprintln!("error: open store: {e}");
        ExitCode::from(1)
    })?;

    let commits = store.scan_commits().map_err(|e| {
        eprintln!("error: scan commits: {e}");
        ExitCode::from(1)
    })?;
    let head = store.head().map_err(|e| {
        eprintln!("error: head: {e}");
        ExitCode::from(1)
    })?;
    let live_objects = store.list_object_ids("").map_err(|e| {
        eprintln!("error: list objects: {e}");
        ExitCode::from(1)
    })?;

    println!("store: {}", path.display());
    println!("wal_bytes: {wal_bytes}");
    println!("commits: {}", commits.len());
    match head {
        Some(id) => println!("head: {id}"),
        None => println!("head: none"),
    }
    println!("live_objects: {}", live_objects.len());

    store.close().map_err(|e| {
        eprintln!("error: close store: {e}");
        ExitCode::from(1)
    })?;
    Ok(())
}

fn cmd_list(store_dir: &Path, prefix: &str) -> Result<(), ExitCode> {
    let path = require_existing_store(store_dir)?;
    let store = BinderStore::open(&path).map_err(|e| {
        eprintln!("error: open store: {e}");
        ExitCode::from(1)
    })?;

    let mut ids = store.list_object_ids(prefix).map_err(|e| {
        eprintln!("error: list: {e}");
        ExitCode::from(1)
    })?;
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    for id in &ids {
        println!("{id}");
    }

    store.close().map_err(|e| {
        eprintln!("error: close store: {e}");
        ExitCode::from(1)
    })?;
    Ok(())
}

fn print_version() {
    println!("bindb {VERSION}");
    println!("commit: {GIT_SHA}");
    println!("built: {BUILD_DATE}");
}

fn print_help() {
    println!(
        "\
bindb {VERSION}

bindb is a code intelligence and repository analysis storage engine CLI.

USAGE:
    bindb --version
    bindb --help
    bindb info <store-dir>
    bindb list <store-dir> <namespace-prefix>

COMMANDS:
    info <store-dir>
        Print store path, WAL size, commit count, head commit, live object count.
        <store-dir> must contain commits.wal.

    list <store-dir> <namespace-prefix>
        Print live object IDs matching the prefix (one per line, sorted).
        Use an empty prefix (\"\") to list all live object IDs.

OPTIONS:
    -h, --help       Print help
    -V, --version    Print version and build provenance

The bindb source code is privately maintained. This binary is distributed
separately; see the bindb-releases repository for installation and license.
"
    );
}

fn print_help_to_stderr() {
    eprintln!(
        "\
USAGE:
    bindb --version
    bindb --help
    bindb info <store-dir>
    bindb list <store-dir> <namespace-prefix>
"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bindb::{ObjectId, Transaction};

    #[test]
    fn provenance_constants_are_populated() {
        assert!(!VERSION.is_empty());
        assert_ne!(GIT_SHA, "unknown", "build.rs failed to inject git SHA");
        assert_ne!(
            BUILD_DATE, "unknown",
            "build.rs failed to inject build date"
        );
    }

    #[test]
    fn info_and_list_against_temp_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = BinderStore::open(dir.path()).unwrap();
        let mut tx = Transaction::new();
        tx.put_object(ObjectId::new("demo://a"), b"1".to_vec());
        tx.put_object(ObjectId::new("demo://b"), b"2".to_vec());
        tx.put_object(ObjectId::new("other://c"), b"3".to_vec());
        store.commit(tx, "test", "seed").unwrap();
        store.close().unwrap();

        cmd_info(dir.path()).expect("info");
        cmd_list(dir.path(), "demo://").expect("list demo");
        cmd_list(dir.path(), "").expect("list all");
    }

    #[test]
    fn missing_store_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(cmd_info(&missing).is_err());
    }
}
