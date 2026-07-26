use bindb::{BinderStore, ObjectId, Transaction};
use tempfile::TempDir;

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn obj(s: impl Into<String>) -> ObjectId {
    ObjectId::new(s)
}

fn write_f64(store: &BinderStore, oid: ObjectId, v: f64) {
    let mut tx = Transaction::new();
    tx.put_object(oid, v.to_le_bytes().to_vec());
    store.commit(tx, "test", "obs").unwrap();
}

// ── alias replacement unit test (no DuckDB needed) ───────────────────────────

/// query_sql_multi returns InvalidArgument for an empty tables slice.
#[test]
fn empty_tables_returns_error() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let err = store.query_sql_multi(&[], "SELECT 1").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("at least one"),
        "expected descriptive error, got: {msg}"
    );
    store.close().unwrap();
}

// ── DuckDB-backed join tests (skipped when duckdb is absent) ─────────────────

fn duckdb_available() -> bool {
    std::process::Command::new("duckdb")
        .arg("-c")
        .arg("SELECT 1")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Join two observation namespaces by object_id suffix and verify the result.
///
/// Schema: obs://health.sleep/YYYY-MM-DD → sleep hours (f64 LE)
///         obs://health.steps/YYYY-MM-DD → step count (f64 LE)
/// Query: for each date, return sleep hours and step count side-by-side.
#[test]
fn cross_namespace_join_by_object_id_suffix() {
    if !duckdb_available() {
        eprintln!("skipping: duckdb not on PATH");
        return;
    }

    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Write 3 sleep observations.
    for (date, hours) in [
        ("2024-01-01", 7.5f64),
        ("2024-01-02", 6.0),
        ("2024-01-03", 8.0),
    ] {
        write_f64(&store, obj(format!("obs://health.sleep/{date}")), hours);
    }
    // Write 3 step observations (same dates).
    for (date, steps) in [
        ("2024-01-01", 8000f64),
        ("2024-01-02", 12000.0),
        ("2024-01-03", 5500.0),
    ] {
        write_f64(&store, obj(format!("obs://health.steps/{date}")), steps);
    }

    let sql = "
        SELECT
            regexp_extract(s.object_id, '[^/]+$') AS date,
            s.value AS sleep_h,
            st.value AS steps
        FROM sleep s
        JOIN steps st
          ON regexp_extract(s.object_id, '[^/]+$') = regexp_extract(st.object_id, '[^/]+$')
        ORDER BY date
    ";

    let result = store
        .query_sql_multi(
            &[
                ("sleep", "obs://health.sleep"),
                ("steps", "obs://health.steps"),
            ],
            sql,
        )
        .unwrap();

    // CSV output: header + 3 data rows
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines.len(), 4, "expected header + 3 rows, got:\n{result}");
    assert!(lines[1].contains("2024-01-01"), "row 1 must be 2024-01-01");
    assert!(lines[1].contains("7.5"), "row 1 sleep must be 7.5");
    assert!(lines[1].contains("8000"), "row 1 steps must be 8000");

    store.close().unwrap();
}

/// A single-table query through query_sql_multi produces the same result as
/// query_sql_observations for the same namespace.
#[test]
fn single_table_multi_matches_single_api() {
    if !duckdb_available() {
        eprintln!("skipping: duckdb not on PATH");
        return;
    }

    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    for v in [1.0f64, 2.0, 3.0] {
        let date = format!("2024-01-0{}", v as u32);
        write_f64(&store, obj(format!("obs://health.hr/{date}")), v * 60.0);
    }

    let sql = "SELECT COUNT(*) AS n FROM hr";
    let multi = store
        .query_sql_multi(&[("hr", "obs://health.hr")], sql)
        .unwrap();
    // Equivalent single-table query
    let single = store
        .query_sql_observations("obs://health.hr", "SELECT COUNT(*) AS n FROM observations")
        .unwrap();

    let multi_count: i64 = multi.lines().nth(1).unwrap().trim().parse().unwrap();
    let single_count: i64 = single.lines().nth(1).unwrap().trim().parse().unwrap();
    assert_eq!(
        multi_count, single_count,
        "multi and single APIs must agree"
    );

    store.close().unwrap();
}

/// Alias that is a substring of another identifier is NOT replaced.
/// e.g. alias "hr" must not corrupt "heart_rate" column names.
#[test]
fn alias_substring_not_replaced() {
    if !duckdb_available() {
        eprintln!("skipping: duckdb not on PATH");
        return;
    }

    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    // Write two namespaces: "hr" and "heart_rate_variability"
    write_f64(&store, obj("obs://health.hr/2024-01-01"), 72.0);
    write_f64(&store, obj("obs://health.hrv/2024-01-01"), 45.0);

    // If alias replacement is naive, "hrv" would accidentally embed "hr" replacement.
    // The query uses both table names; if hrv is corrupted the query fails.
    let sql = "SELECT hr.value AS bpm, hrv.value AS ms FROM hr JOIN hrv ON 1=1";
    let result = store.query_sql_multi(
        &[("hr", "obs://health.hr"), ("hrv", "obs://health.hrv")],
        sql,
    );
    // May fail if DuckDB SQL doesn't like the exact join, but must NOT fail with
    // a path substitution error that garbles "hrv" into a double-expanded path.
    match result {
        Ok(out) => assert!(
            !out.contains("read_ndjson_auto"),
            "output must not contain raw DuckDB call: {out}"
        ),
        Err(e) => {
            let msg = format!("{e}");
            assert!(
                !msg.contains("read_ndjson_auto"),
                "error must not be a path substitution garble: {msg}"
            );
        }
    }

    store.close().unwrap();
}
