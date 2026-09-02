//! Public `bindb` CLI (init / put / get / info / list).
//!
//! The executable is the public surface for binary distribution.
//! Internal Rust APIs remain private implementation details.

use bindb::{BinderStore, ObjectId, Transaction};
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
        "init" => {
            let store_dir = args.get(1).ok_or_else(|| usage_exit("init <store-dir>"))?;
            cmd_init(Path::new(store_dir))
        }
        "put" => {
            let store_dir = args
                .get(1)
                .ok_or_else(|| usage_exit("put <store-dir> <object-id> <payload>"))?;
            let object_id = args
                .get(2)
                .ok_or_else(|| usage_exit("put <store-dir> <object-id> <payload>"))?;
            let payload = args
                .get(3)
                .ok_or_else(|| usage_exit("put <store-dir> <object-id> <payload>"))?;
            cmd_put(Path::new(store_dir), object_id, payload.as_bytes())
        }
        "get" => {
            let store_dir = args
                .get(1)
                .ok_or_else(|| usage_exit("get <store-dir> <object-id>"))?;
            let object_id = args
                .get(2)
                .ok_or_else(|| usage_exit("get <store-dir> <object-id>"))?;
            cmd_get(Path::new(store_dir), object_id)
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

fn resolve_path(store_dir: &Path) -> Result<PathBuf, ExitCode> {
    if store_dir.is_absolute() {
        Ok(store_dir.to_path_buf())
    } else {
        Ok(env::current_dir()
            .map_err(|e| {
                eprintln!("error: {e}");
                ExitCode::from(1)
            })?
            .join(store_dir))
    }
}

fn require_existing_store(store_dir: &Path) -> Result<PathBuf, ExitCode> {
    let path = resolve_path(store_dir)?;
    let wal = path.join("commits.wal");
    if !wal.is_file() {
        eprintln!(
            "error: not a bindb store (missing commits.wal): {}",
            path.display()
        );
        eprintln!("hint: create one with: bindb init {}", store_dir.display());
        return Err(ExitCode::from(1));
    }
    Ok(path)
}

fn cmd_init(store_dir: &Path) -> Result<(), ExitCode> {
    let path = resolve_path(store_dir)?;
    if path.join("commits.wal").is_file() {
        eprintln!(
            "error: store already exists (commits.wal present): {}",
            path.display()
        );
        return Err(ExitCode::from(1));
    }
    if path.exists()
        && path
            .read_dir()
            .map_err(|e| {
                eprintln!("error: {e}");
                ExitCode::from(1)
            })?
            .next()
            .is_some()
    {
        // Allow empty dir; refuse non-empty non-store dirs to avoid clobbering.
        eprintln!(
            "error: directory is not empty and is not a bindb store: {}",
            path.display()
        );
        return Err(ExitCode::from(1));
    }

    let store = BinderStore::open(&path).map_err(|e| {
        eprintln!("error: init store: {e}");
        ExitCode::from(1)
    })?;
    store.close().map_err(|e| {
        eprintln!("error: close store: {e}");
        ExitCode::from(1)
    })?;

    println!("initialized: {}", path.display());
    Ok(())
}

fn cmd_put(store_dir: &Path, object_id: &str, payload: &[u8]) -> Result<(), ExitCode> {
    let path = require_existing_store(store_dir)?;
    let store = BinderStore::open(&path).map_err(|e| {
        eprintln!("error: open store: {e}");
        ExitCode::from(1)
    })?;

    let mut tx = Transaction::new();
    tx.put_object(ObjectId::new(object_id), payload.to_vec());
    let commit_id = store
        .commit(tx, "bindb-cli", format!("put {object_id}"))
        .map_err(|e| {
            eprintln!("error: commit: {e}");
            ExitCode::from(1)
        })?;

    println!("ok: {object_id}");
    println!("commit: {commit_id}");

    store.close().map_err(|e| {
        eprintln!("error: close store: {e}");
        ExitCode::from(1)
    })?;
    Ok(())
}

fn cmd_get(store_dir: &Path, object_id: &str) -> Result<(), ExitCode> {
    let path = require_existing_store(store_dir)?;
    let store = BinderStore::open(&path).map_err(|e| {
        eprintln!("error: open store: {e}");
        ExitCode::from(1)
    })?;

    let oid = ObjectId::new(object_id);
    let version = store.get(&oid).map_err(|e| {
        eprintln!("error: get: {e}");
        ExitCode::from(1)
    })?;

    match version {
        None => {
            eprintln!("error: object not found: {object_id}");
            let _ = store.close();
            Err(ExitCode::from(1))
        }
        Some(v) if v.is_tombstone() => {
            eprintln!("error: object deleted (tombstone): {object_id}");
            let _ = store.close();
            Err(ExitCode::from(1))
        }
        Some(v) => {
            match v.payload.as_deref() {
                Some(bytes) => match std::str::from_utf8(bytes) {
                    Ok(s) => print!("{s}"),
                    Err(_) => {
                        // Binary payloads: print hex so agents can still inspect.
                        for b in bytes {
                            print!("{b:02x}");
                        }
                        println!();
                    }
                },
                None => {
                    eprintln!("error: object has no payload in memory: {object_id}");
                    let _ = store.close();
                    return Err(ExitCode::from(1));
                }
            }
            if !std::io::Write::flush(&mut std::io::stdout()).is_ok() {
                // ignore flush errors
            }
            store.close().map_err(|e| {
                eprintln!("error: close store: {e}");
                ExitCode::from(1)
            })?;
            Ok(())
        }
    }
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

There are no SQL tables. A store is a directory containing commits.wal.
Object IDs are URI-like strings (e.g. demo://notes/1); prefixes group them.

USAGE:
    bindb --version
    bindb --help
    bindb init <store-dir>
    bindb put  <store-dir> <object-id> <payload>
    bindb get  <store-dir> <object-id>
    bindb info <store-dir>
    bindb list <store-dir> <namespace-prefix>

COMMANDS:
    init <store-dir>
        Create a new empty store directory (writes commits.wal).

    put <store-dir> <object-id> <payload>
        Commit one object. Payload is a UTF-8 string argument.

    get <store-dir> <object-id>
        Print the live object payload (UTF-8, or hex if binary).

    info <store-dir>
        Print WAL size, commit count, head commit, live object count.

    list <store-dir> <namespace-prefix>
        Print live object IDs matching the prefix (sorted).
        Use \"\" to list all live object IDs.

OPTIONS:
    -h, --help       Print help
    -V, --version    Print version and build provenance

See https://github.com/dpkimball/bindb-releases (GETTING_STARTED.md).
"
    );
}

fn print_help_to_stderr() {
    eprintln!(
        "\
USAGE:
    bindb --version
    bindb --help
    bindb init <store-dir>
    bindb put  <store-dir> <object-id> <payload>
    bindb get  <store-dir> <object-id>
    bindb info <store-dir>
    bindb list <store-dir> <namespace-prefix>
"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn init_put_get_list_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mydb");
        cmd_init(&store).expect("init");
        assert!(store.join("commits.wal").is_file());
        assert!(cmd_init(&store).is_err(), "second init must fail");

        cmd_put(&store, "demo://hello", b"world").expect("put");
        cmd_info(&store).expect("info");
        cmd_list(&store, "demo://").expect("list");
        cmd_get(&store, "demo://hello").expect("get");
        assert!(cmd_get(&store, "demo://missing").is_err());
    }

    #[test]
    fn missing_store_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(cmd_info(&missing).is_err());
    }
}
