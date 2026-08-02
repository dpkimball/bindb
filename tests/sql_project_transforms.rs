#[path = "sql_agg_helpers.rs"]
mod helpers;
use helpers::*;

// ── ABS/ROUND/CEIL/FLOOR(PAYLOAD_FIELD) and LENGTH(PAYLOAD_FIELD) tests ──────

#[test]
fn select_abs_of_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sabn:a"), br#"{"delta":-7.5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ABS(PAYLOAD_FIELD('delta')) FROM 'obs://sabn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("delta").and_then(|v| v.as_f64()), Some(7.5));
    store.close().unwrap();
}

#[test]
fn select_round_of_decimal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srod:a"), br#"{"price":4.6}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ROUND(PAYLOAD_FIELD('price')) FROM 'obs://srod'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("price").and_then(|v| v.as_f64()), Some(5.0));
    store.close().unwrap();
}

#[test]
fn select_ceil_and_floor() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaf:a"), br#"{"val":3.2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let ceil_rows = store
        .sql_project("SELECT CEIL(PAYLOAD_FIELD('val')) FROM 'obs://scaf'")
        .unwrap();
    assert_eq!(ceil_rows[0].get("val").and_then(|v| v.as_f64()), Some(4.0));

    let floor_rows = store
        .sql_project("SELECT FLOOR(PAYLOAD_FIELD('val')) FROM 'obs://scaf'")
        .unwrap();
    assert_eq!(floor_rows[0].get("val").and_then(|v| v.as_f64()), Some(3.0));
    store.close().unwrap();
}

#[test]
fn select_numeric_transform_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sntsp:a"), br#"{"label":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ABS(PAYLOAD_FIELD('label')) FROM 'obs://sntsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("label").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_length_of_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slos:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('word')) FROM 'obs://slos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

#[test]
fn select_length_of_multiple_rows() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slmr:a"), br#"{"tag":"hi"}"#.to_vec());
    tx.put_object(oid("obs://slmr:b"), br#"{"tag":"world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('tag')) FROM 'obs://slmr' ORDER BY PAYLOAD_FIELD('tag') ASC")
        .unwrap();
    rows.sort_by_key(|r| r.get("tag").and_then(|v| v.as_u64()).unwrap_or(0));
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(rows[1].get("tag").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

#[test]
fn select_length_on_number_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slnp:a"), br#"{"count":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LENGTH(PAYLOAD_FIELD('count')) FROM 'obs://slnp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("count").and_then(|v| v.as_u64()), Some(42));
    store.close().unwrap();
}

#[test]
fn select_length_transform_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LENGTH(PAYLOAD_FIELD('name')) FROM 'obs://x'").unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("name"),
        Some(SqlFieldTransform::Length)
    ));
}

// ── COALESCE / REPLACE(PAYLOAD_FIELD) in SELECT tests ─────────────────────────

#[test]
fn select_coalesce_present_field_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scpf:a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('name'), 'unknown') FROM 'obs://scpf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("Alice"));
    store.close().unwrap();
}

#[test]
fn select_coalesce_missing_field_uses_fallback() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scmf:a"), br#"{"other":"x"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('name'), 'N/A') FROM 'obs://scmf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("N/A"));
    store.close().unwrap();
}

#[test]
fn select_coalesce_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT COALESCE(PAYLOAD_FIELD('label'), 'default') FROM 'obs://x'")
            .unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("label"),
        Some(SqlFieldTransform::Coalesce(s)) if s == "default"
    ));
}

#[test]
fn select_coalesce_with_alias() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwa:a"), br#"{}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COALESCE(PAYLOAD_FIELD('val'), 'none') AS v FROM 'obs://scwa'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("v").and_then(|v| v.as_str()), Some("none"));
    assert!(!rows[0].contains_key("val"));
    store.close().unwrap();
}

#[test]
fn select_replace_substitutes_substring() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srss:a"), br#"{"tag":"hello-world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('tag'), '-', '_') FROM 'obs://srss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("tag").and_then(|v| v.as_str()),
        Some("hello_world")
    );
    store.close().unwrap();
}

#[test]
fn select_replace_all_occurrences() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srao:a"), br#"{"path":"a/b/c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('path'), '/', '.') FROM 'obs://srao'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("path").and_then(|v| v.as_str()), Some("a.b.c"));
    store.close().unwrap();
}

#[test]
fn select_replace_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srnsp:a"), br#"{"n":99}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPLACE(PAYLOAD_FIELD('n'), '9', '0') FROM 'obs://srnsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(99));
    store.close().unwrap();
}

#[test]
fn select_replace_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse(
        "SELECT REPLACE(PAYLOAD_FIELD('text'), 'foo', 'bar') FROM 'obs://x'",
    )
    .unwrap();
    assert_eq!(q.field_transforms.len(), 1);
    assert!(matches!(
        q.field_transforms.get("text"),
        Some(SqlFieldTransform::Replace(f, t)) if f == "foo" && t == "bar"
    ));
}

// ── SUBSTRING / CAST(PAYLOAD_FIELD) in SELECT tests ───────────────────────────

#[test]
fn select_substring_extracts_chars() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssec:a"), br#"{"code":"ABCDE"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SUBSTRING(PAYLOAD_FIELD('code'), 1, 3) FROM 'obs://ssec'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("code").and_then(|v| v.as_str()), Some("BCD"));
    store.close().unwrap();
}

#[test]
fn select_substring_start_beyond_length_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sssbe:a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SUBSTRING(PAYLOAD_FIELD('tag'), 10, 3) FROM 'obs://sssbe'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_substring_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SUBSTRING(PAYLOAD_FIELD('name'), 2, 4) FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("name"),
        Some(SqlFieldTransform::Substring(2, 4))
    ));
}

#[test]
fn select_cast_as_int_from_float() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaif:a"), br#"{"price":9.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('price') AS INT) FROM 'obs://scaif'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("price").and_then(|v| v.as_i64()), Some(9));
    store.close().unwrap();
}

#[test]
fn select_cast_as_int_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaifs:a"), br#"{"count":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('count') AS INT) FROM 'obs://scaifs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("count").and_then(|v| v.as_i64()), Some(42));
    store.close().unwrap();
}

#[test]
fn select_cast_as_float_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scaff:a"), br#"{"temp":"2.5"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('temp') AS FLOAT) FROM 'obs://scaff'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("temp").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.5).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cast_as_string_from_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scasfn:a"), br#"{"id":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CAST(PAYLOAD_FIELD('id') AS STRING) FROM 'obs://scasfn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("id").and_then(|v| v.as_str()), Some("100"));
    store.close().unwrap();
}

#[test]
fn select_cast_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT CAST(PAYLOAD_FIELD('score') AS INT) FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("score"),
        Some(SqlFieldTransform::CastInt)
    ));
}

// ── NULLIF / LPAD / RPAD(PAYLOAD_FIELD) in SELECT tests ───────────────────────

#[test]
fn select_nullif_matching_produces_null() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://snmpn:a"), br#"{"status":"N/A"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT NULLIF(PAYLOAD_FIELD('status'), 'N/A') FROM 'obs://snmpn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("status"), Some(&serde_json::Value::Null));
    store.close().unwrap();
}

#[test]
fn select_nullif_non_matching_kept() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://snnmk:a"), br#"{"status":"active"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT NULLIF(PAYLOAD_FIELD('status'), 'N/A') FROM 'obs://snnmk'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("status").and_then(|v| v.as_str()),
        Some("active")
    );
    store.close().unwrap();
}

#[test]
fn select_nullif_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT NULLIF(PAYLOAD_FIELD('tag'), 'none') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("tag"),
        Some(SqlFieldTransform::NullIf(s)) if s == "none"
    ));
}

#[test]
fn select_lpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slpss:a"), br#"{"code":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LPAD(PAYLOAD_FIELD('code'), 5, '0') FROM 'obs://slpss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("code").and_then(|v| v.as_str()), Some("00042"));
    store.close().unwrap();
}

#[test]
fn select_lpad_already_wide_string_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slpawsu:a"), br#"{"tag":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LPAD(PAYLOAD_FIELD('tag'), 3, ' ') FROM 'obs://slpawsu'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_rpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srpss:a"), br#"{"name":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RPAD(PAYLOAD_FIELD('name'), 5, '.') FROM 'obs://srpss'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("hi..."));
    store.close().unwrap();
}

#[test]
fn select_lpad_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LPAD(PAYLOAD_FIELD('code'), 8, '0') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("code"),
        Some(SqlFieldTransform::Lpad(8, s)) if s == "0"
    ));
}

#[test]
fn select_rpad_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT RPAD(PAYLOAD_FIELD('label'), 10, ' ') FROM 'obs://x'")
        .unwrap();
    assert!(matches!(
        q.field_transforms.get("label"),
        Some(SqlFieldTransform::Rpad(10, s)) if s == " "
    ));
}

// ── REVERSE / INITCAP / REPEAT(PAYLOAD_FIELD) in SELECT tests ───────────────

#[test]
fn select_reverse_reverses_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srevrs:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REVERSE(PAYLOAD_FIELD('word')) FROM 'obs://srevrs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("olleh"));
    store.close().unwrap();
}

#[test]
fn select_reverse_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT REVERSE(PAYLOAD_FIELD('word')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Reverse)
    ));
}

#[test]
fn select_initcap_capitalises_words() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sicpw:a"), br#"{"title":"hello world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT INITCAP(PAYLOAD_FIELD('title')) FROM 'obs://sicpw'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("title").and_then(|v| v.as_str()),
        Some("Hello World")
    );
    store.close().unwrap();
}

#[test]
fn select_initcap_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT INITCAP(PAYLOAD_FIELD('title')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("title"),
        Some(SqlFieldTransform::InitCap)
    ));
}

#[test]
fn select_repeat_duplicates_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sreps:a"), br#"{"sep":"-"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('sep'), 3) FROM 'obs://sreps'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("sep").and_then(|v| v.as_str()), Some("---"));
    store.close().unwrap();
}

#[test]
fn select_repeat_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT REPEAT(PAYLOAD_FIELD('sep'), 4) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("sep"),
        Some(SqlFieldTransform::Repeat(4))
    ));
}

#[test]
fn select_repeat_zero_gives_empty_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srep0:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('tag'), 0) FROM 'obs://srep0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_repeat_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srepnsp:a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT REPEAT(PAYLOAD_FIELD('score'), 3) FROM 'obs://srepnsp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(42));
    store.close().unwrap();
}

// ── LEFT / RIGHT / SIGN(PAYLOAD_FIELD) in SELECT tests ──────────────────────

#[test]
fn select_left_extracts_prefix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sleft:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LEFT(PAYLOAD_FIELD('word'), 3) FROM 'obs://sleft'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("hel"));
    store.close().unwrap();
}

#[test]
fn select_left_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT LEFT(PAYLOAD_FIELD('word'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Left(3))
    ));
}

#[test]
fn select_right_extracts_suffix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srght:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RIGHT(PAYLOAD_FIELD('word'), 3) FROM 'obs://srght'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_str()), Some("llo"));
    store.close().unwrap();
}

#[test]
fn select_right_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT RIGHT(PAYLOAD_FIELD('word'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Right(3))
    ));
}

#[test]
fn select_sign_positive_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignp:a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('score')) FROM 'obs://ssignp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("score").and_then(|v| v.as_i64()), Some(1));
    store.close().unwrap();
}

#[test]
fn select_sign_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignn:a"), br#"{"val":-7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('val')) FROM 'obs://ssignn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("val").and_then(|v| v.as_i64()), Some(-1));
    store.close().unwrap();
}

#[test]
fn select_sign_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssignz:a"), br#"{"val":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIGN(PAYLOAD_FIELD('val')) FROM 'obs://ssignz'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("val").and_then(|v| v.as_i64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_sign_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SIGN(PAYLOAD_FIELD('score')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("score"),
        Some(SqlFieldTransform::Sign)
    ));
}

// ── SQRT / POWER(PAYLOAD_FIELD) in SELECT tests ──────────────────────────────

#[test]
fn select_sqrt_of_perfect_square() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssqrt:a"), br#"{"n":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SQRT(PAYLOAD_FIELD('n')) FROM 'obs://ssqrt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_sqrt_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT SQRT(PAYLOAD_FIELD('n')) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Sqrt)
    ));
}

#[test]
fn select_sqrt_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssqrts:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SQRT(PAYLOAD_FIELD('tag')) FROM 'obs://ssqrts'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_power_squares_a_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spow:a"), br#"{"n":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 2) FROM 'obs://spow'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 9.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_fractional_exponent_gives_sqrt() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spowf:a"), br#"{"n":4}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 0.5) FROM 'obs://spowf'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_parses_correctly() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT POWER(PAYLOAD_FIELD('n'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Power(e)) if (e - 3.0).abs() < 1e-9
    ));
}

#[test]
fn select_power_zero_exponent_gives_one() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spow0:a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('n'), 0) FROM 'obs://spow0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_power_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://spows:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT POWER(PAYLOAD_FIELD('tag'), 2) FROM 'obs://spows'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_mod_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://smod:a"), br#"{"n":10}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT MOD(PAYLOAD_FIELD('n'), 3) FROM 'obs://smod'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_mod_parse() {
    use bindb::SqlFieldTransform;
    let q = SqlProjectQuery::parse("SELECT MOD(PAYLOAD_FIELD('n'), 3) FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("n"),
        Some(SqlFieldTransform::Mod(d)) if (d - 3.0).abs() < 1e-9
    ));
}

#[test]
fn select_mod_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://smods:a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT MOD(PAYLOAD_FIELD('tag'), 3) FROM 'obs://smods'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("hi"));
    store.close().unwrap();
}

#[test]
fn select_strpos_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sstrpos:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT STRPOS(PAYLOAD_FIELD('word'), 'l') FROM 'obs://sstrpos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn select_strpos_not_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sstrpno:a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT STRPOS(PAYLOAD_FIELD('word'), 'z') FROM 'obs://sstrpno'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("word").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_strpos_parse() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT STRPOS(PAYLOAD_FIELD('word'), 'l') FROM 'obs://x'").unwrap();
    assert!(matches!(
        q.field_transforms.get("word"),
        Some(SqlFieldTransform::Strpos(n)) if n == "l"
    ));
}

#[test]
fn select_split_part_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssplitpt:a"), br#"{"csv":"a,b,c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 2) FROM 'obs://ssplitpt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("csv").and_then(|v| v.as_str()), Some("b"));
    store.close().unwrap();
}

#[test]
fn select_split_part_out_of_bounds() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssplitob:a"), br#"{"csv":"a,b"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 9) FROM 'obs://ssplitob'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("csv").and_then(|v| v.as_str()), Some(""));
    store.close().unwrap();
}

#[test]
fn select_split_part_parse() {
    use bindb::SqlFieldTransform;
    let q =
        SqlProjectQuery::parse("SELECT SPLIT_PART(PAYLOAD_FIELD('csv'), ',', 2) FROM 'obs://x'")
            .unwrap();
    assert!(matches!(
        q.field_transforms.get("csv"),
        Some(SqlFieldTransform::SplitPart(d, 2)) if d == ","
    ));
}

#[test]
fn select_ltrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sltrim:a"), br#"{"s":"  hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LTRIM(PAYLOAD_FIELD('s')) FROM 'obs://sltrim'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_rtrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srtrim:a"), br#"{"s":"hello  "}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RTRIM(PAYLOAD_FIELD('s')) FROM 'obs://srtrim'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("hello"));
    store.close().unwrap();
}

#[test]
fn select_log_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG(PAYLOAD_FIELD('n')) FROM 'obs://slog'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_log_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog0:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG(PAYLOAD_FIELD('n')) FROM 'obs://slog0'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_exp_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sexp:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT EXP(PAYLOAD_FIELD('n')) FROM 'obs://sexp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_exp_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sexps:a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT EXP(PAYLOAD_FIELD('tag')) FROM 'obs://sexps'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("tag").and_then(|v| v.as_str()), Some("abc"));
    store.close().unwrap();
}

#[test]
fn select_log2_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog2:a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG2(PAYLOAD_FIELD('n')) FROM 'obs://slog2'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_log2_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog2z:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG2(PAYLOAD_FIELD('n')) FROM 'obs://slog2z'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn select_log10_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://slog10:a"), br#"{"n":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT LOG10(PAYLOAD_FIELD('n')) FROM 'obs://slog10'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_radians_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srad:a"), br#"{"deg":180}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RADIANS(PAYLOAD_FIELD('deg')) FROM 'obs://srad'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("deg").and_then(|v| v.as_f64()).unwrap();
    assert!((v - std::f64::consts::PI).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_degrees_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://sdeg:a"),
        format!(r#"{{"rad":{}}}"#, std::f64::consts::PI).into_bytes(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DEGREES(PAYLOAD_FIELD('rad')) FROM 'obs://sdeg'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("rad").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 180.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_radians_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://srads:a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT RADIANS(PAYLOAD_FIELD('s')) FROM 'obs://srads'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("north"));
    store.close().unwrap();
}

#[test]
fn select_degrees_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sdegs:a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT DEGREES(PAYLOAD_FIELD('s')) FROM 'obs://sdegs'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("north"));
    store.close().unwrap();
}

#[test]
fn select_sin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssin:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIN(PAYLOAD_FIELD('n')) FROM 'obs://ssin'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_sin_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://ssins:a"), br#"{"s":"wave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT SIN(PAYLOAD_FIELD('s')) FROM 'obs://ssins'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("wave"));
    store.close().unwrap();
}

#[test]
fn select_cos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scos:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT COS(PAYLOAD_FIELD('n')) FROM 'obs://scos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_tan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://stan:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TAN(PAYLOAD_FIELD('n')) FROM 'obs://stan'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_atan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://satan:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ATAN(PAYLOAD_FIELD('n')) FROM 'obs://satan'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_atan_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://satans:a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ATAN(PAYLOAD_FIELD('s')) FROM 'obs://satans'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("arc"));
    store.close().unwrap();
}

#[test]
fn select_asin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sasin:a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ASIN(PAYLOAD_FIELD('n')) FROM 'obs://sasin'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_asin_out_of_domain_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sasinood:a"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ASIN(PAYLOAD_FIELD('n')) FROM 'obs://sasinood'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n").and_then(|v| v.as_i64()), Some(2));
    store.close().unwrap();
}

#[test]
fn select_acos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sacos:a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ACOS(PAYLOAD_FIELD('n')) FROM 'obs://sacos'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_acos_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sacosstr:a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT ACOS(PAYLOAD_FIELD('s')) FROM 'obs://sacosstr'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("arc"));
    store.close().unwrap();
}

#[test]
fn select_trunc_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://struncn:a"), br#"{"n":-1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TRUNC(PAYLOAD_FIELD('n')) FROM 'obs://struncn'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - (-1.0)).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_trunc_positive() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://struncp:a"), br#"{"n":1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT TRUNC(PAYLOAD_FIELD('n')) FROM 'obs://struncp'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cbrt_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scbrt:a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CBRT(PAYLOAD_FIELD('n')) FROM 'obs://scbrt'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].get("n").and_then(|v| v.as_f64()).unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn select_cbrt_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scbrtstr:a"), br#"{"s":"root"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT CBRT(PAYLOAD_FIELD('s')) FROM 'obs://scbrtstr'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("s").and_then(|v| v.as_str()), Some("root"));
    store.close().unwrap();
}

// ── Arithmetic computed columns ─────────────────────────────────────────────

#[test]
fn sql_project_arith_add_two_fields() {
    // SELECT PAYLOAD_FIELD('a') + PAYLOAD_FIELD('b') AS total FROM ...
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://arith:x"), br#"{"a":3,"b":4}"#.to_vec());
    tx.put_object(oid("obs://arith:y"), br#"{"a":10,"b":5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('a') + PAYLOAD_FIELD('b') AS total FROM 'obs://arith'")
        .unwrap();
    let eps = 1e-9;
    let mut totals: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("total").and_then(|v| v.as_f64()))
        .collect();
    totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((totals[0] - 7.0).abs() < eps);
    assert!((totals[1] - 15.0).abs() < eps);
    store.close().unwrap();
}

#[test]
fn sql_project_arith_multiply_field_literal() {
    // SELECT PAYLOAD_FIELD('price') * 2 AS doubled FROM ...
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://mul:a"), br#"{"price":5}"#.to_vec());
    tx.put_object(oid("obs://mul:b"), br#"{"price":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('price') * 2 AS doubled FROM 'obs://mul'")
        .unwrap();
    let eps = 1e-9;
    let mut vals: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("doubled").and_then(|v| v.as_f64()))
        .collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((vals[0] - 10.0).abs() < eps);
    assert!((vals[1] - 16.0).abs() < eps);
    store.close().unwrap();
}

#[test]
fn sql_project_arith_division_by_zero_null() {
    // Division by literal zero yields null (not a row).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://divz:a"), br#"{"v":10}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('v') / 0 AS q FROM 'obs://divz'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    // null means the key is absent from the row.
    assert!(!rows[0].contains_key("q"));
    store.close().unwrap();
}

#[test]
fn sql_project_arith_subtract() {
    // SELECT PAYLOAD_FIELD('a') - PAYLOAD_FIELD('b') AS diff FROM ...
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://sub:a"), br#"{"a":10,"b":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project("SELECT PAYLOAD_FIELD('a') - PAYLOAD_FIELD('b') AS diff FROM 'obs://sub'")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let diff = rows[0].get("diff").and_then(|v| v.as_f64()).unwrap_or(-1.0);
    assert!((diff - 7.0).abs() < 1e-9);
    store.close().unwrap();
}

// ── String concat (||) computed columns ────────────────────────────────────

#[test]
fn sql_project_concat_field_literal_field() {
    // SELECT PAYLOAD_FIELD('first') || ' ' || PAYLOAD_FIELD('last') AS full_name FROM ...
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://cat:a"),
        br#"{"first":"Alice","last":"Smith"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://cat:b"),
        br#"{"first":"Bob","last":"Jones"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('first') || ' ' || PAYLOAD_FIELD('last') AS full_name \
             FROM 'obs://cat'",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("full_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("full_name").and_then(|v| v.as_str()),
        Some("Alice Smith")
    );
    assert_eq!(
        rows[1].get("full_name").and_then(|v| v.as_str()),
        Some("Bob Jones")
    );
    store.close().unwrap();
}

#[test]
fn sql_project_concat_two_fields() {
    // PAYLOAD_FIELD('ns') || ':' || PAYLOAD_FIELD('key') AS id_like
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://cat2:a"), br#"{"ns":"user","key":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('ns') || ':' || PAYLOAD_FIELD('key') AS combined \
             FROM 'obs://cat2'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("combined").and_then(|v| v.as_str()),
        Some("user:42")
    );
    store.close().unwrap();
}

#[test]
fn sql_project_concat_missing_field_empty() {
    // Missing field in concat produces empty string for that part.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://catm:a"), br#"{"first":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('first') || ' ' || PAYLOAD_FIELD('last') AS name \
             FROM 'obs://catm'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name").and_then(|v| v.as_str()), Some("Alice "));
    store.close().unwrap();
}

// ── Searched CASE WHEN in SELECT ────────────────────────────────────────────

#[test]
fn sql_project_searched_case_when_basic() {
    // CASE WHEN PAYLOAD_FIELD('score') > 5 THEN 'hi' ELSE 'lo' END AS tier
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://scw:a"),
        br#"{"score":8,"name":"alice"}"#.to_vec(),
    );
    tx.put_object(oid("obs://scw:b"), br#"{"score":3,"name":"bob"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), \
             CASE WHEN PAYLOAD_FIELD('score') > 5 THEN 'hi' ELSE 'lo' END AS tier \
             FROM 'obs://scw'",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("tier").and_then(|v| v.as_str()), Some("hi"));
    assert_eq!(rows[1].get("tier").and_then(|v| v.as_str()), Some("lo"));
    store.close().unwrap();
}

#[test]
fn sql_project_searched_case_when_multi_branch() {
    // CASE WHEN … > 7 THEN 'a' WHEN … > 4 THEN 'b' ELSE 'c' END AS band
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    for (k, v) in [("x", 9), ("y", 6), ("z", 2)] {
        tx.put_object(
            oid(&format!("obs://scwm:{k}")),
            format!(r#"{{"v":{v}}}"#).into_bytes(),
        );
    }
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('v'), \
             CASE WHEN PAYLOAD_FIELD('v') > 7 THEN 'a' \
                  WHEN PAYLOAD_FIELD('v') > 4 THEN 'b' \
                  ELSE 'c' END AS band \
             FROM 'obs://scwm'",
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    let mut band_map: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for r in &rows {
        let v = r.get("v").and_then(|v| v.as_i64()).unwrap_or(-1);
        let band = r
            .get("band")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        band_map.insert(v, band);
    }
    assert_eq!(band_map.get(&9).map(|s| s.as_str()), Some("a"));
    assert_eq!(band_map.get(&6).map(|s| s.as_str()), Some("b"));
    assert_eq!(band_map.get(&2).map(|s| s.as_str()), Some("c"));
    store.close().unwrap();
}

#[test]
fn sql_project_searched_case_when_string_condition() {
    // CASE WHEN PAYLOAD_FIELD('role') = 'admin' THEN 'elevated' ELSE 'normal' END AS access
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        oid("obs://scws:a"),
        br#"{"role":"admin","name":"alice"}"#.to_vec(),
    );
    tx.put_object(
        oid("obs://scws:b"),
        br#"{"role":"user","name":"bob"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let mut rows = store
        .sql_project(
            "SELECT PAYLOAD_FIELD('name'), \
             CASE WHEN PAYLOAD_FIELD('role') = 'admin' THEN 'elevated' ELSE 'normal' END AS access \
             FROM 'obs://scws'",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("access").and_then(|v| v.as_str()),
        Some("elevated")
    );
    assert_eq!(
        rows[1].get("access").and_then(|v| v.as_str()),
        Some("normal")
    );
    store.close().unwrap();
}

#[test]
fn sql_project_searched_case_when_no_match_else_empty() {
    // No ELSE clause → unmatched rows get empty string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(oid("obs://scwne:a"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .sql_project(
            "SELECT CASE WHEN PAYLOAD_FIELD('v') > 5 THEN 'big' END AS label \
             FROM 'obs://scwne'",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    // empty string is falsy but present when not null
    let label = rows[0]
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("__absent__");
    // empty string means label key absent (null pruned) or present as ""
    assert!(label.is_empty() || label == "__absent__");
    store.close().unwrap();
}
