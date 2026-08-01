#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── RETURN UPPER/LOWER/TRIM tests ─────────────────────────────────────────────

#[test]
fn return_upper_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rut://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rut://a"), br#"{"name":"alice"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rut_e1"), obj("rut://hub"), obj("rut://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rut://hub' RETURN UPPER(b.name)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name"),
        Some(&serde_json::Value::String("ALICE".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_lower_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlt://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rlt://a"), br#"{"tag":"HELLO"}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rlt_e1"), obj("rlt://hub"), obj("rlt://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlt://hub' RETURN LOWER(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tag"),
        Some(&serde_json::Value::String("hello".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_trim_transform() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rtt://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rtt://a"), br#"{"val":"  hi  "}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("rtt_e1"), obj("rtt://hub"), obj("rtt://a"), "X", None);
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rtt://hub' RETURN TRIM(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("val"),
        Some(&serde_json::Value::String("hi".into()))
    );
    store.close().unwrap();
}

#[test]
fn return_transform_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rtns://hub"), br#"{}"#.to_vec());
    tx.put_object(obj("rtns://a"), br#"{"score":42}"#.to_vec());
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rtns_e1"),
        obj("rtns://hub"),
        obj("rtns://a"),
        "X",
        None,
    );
    store.commit(tx, "test", "").unwrap();
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rtns://hub' RETURN UPPER(b.score)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    // numeric values pass through unchanged
    assert_eq!(
        rows[0].fields.get("score"),
        Some(&serde_json::Value::Number(42.into()))
    );
    store.close().unwrap();
}

// ── WITH DISTINCT b ──────────────────────────────────────────────────────────

#[test]
fn with_distinct_parse_sets_flag() {
    let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH DISTINCT b RETURN b")
        .unwrap();
    assert!(q.with_clause.as_ref().unwrap().distinct);
}

#[test]
fn with_no_distinct_flag_false() {
    let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b RETURN b").unwrap();
    assert!(!q.with_clause.as_ref().unwrap().distinct);
}

#[test]
fn with_distinct_returns_correct_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wdist://hub"), b"{}".to_vec());
    tx.put_object(obj("wdist://a"), br#"{"val":1}"#.to_vec());
    tx.put_object(obj("wdist://b"), br#"{"val":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wdist_e1"),
        obj("wdist://hub"),
        obj("wdist://a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("wdist_e2"),
        obj("wdist://hub"),
        obj("wdist://b"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'wdist://hub' WITH DISTINCT b RETURN b.val",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn with_distinct_then_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wdl://hub"), b"{}".to_vec());
    tx.put_object(obj("wdl://a"), br#"{"n":1}"#.to_vec());
    tx.put_object(obj("wdl://b"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("wdl_e1"), obj("wdl://hub"), obj("wdl://a"), "L", None);
    tx2.put_edge(eid("wdl_e2"), obj("wdl://hub"), obj("wdl://b"), "L", None);
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wdl://hub' WITH DISTINCT b LIMIT 1 RETURN b.n",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    store.close().unwrap();
}

// ── multi-field ORDER BY in RETURN ───────────────────────────────────────────

#[test]
fn multi_order_by_parse_roundtrip() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b ORDER BY b.dept ASC, b.rank DESC",
    )
    .unwrap();
    assert_eq!(q.order_by.len(), 2);
    assert_eq!(
        q.order_by[0].field,
        OrderByField::PayloadField("dept".into())
    );
    assert_eq!(q.order_by[0].direction, OrderDirection::Asc);
    assert_eq!(
        q.order_by[1].field,
        OrderByField::PayloadField("rank".into())
    );
    assert_eq!(q.order_by[1].direction, OrderDirection::Desc);
}

#[test]
fn multi_order_by_sorts_by_two_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob://hub"), b"{}".to_vec());
    tx.put_object(obj("mob://a"), br#"{"dept":"eng","rank":2}"#.to_vec());
    tx.put_object(obj("mob://b"), br#"{"dept":"eng","rank":1}"#.to_vec());
    tx.put_object(obj("mob://c"), br#"{"dept":"art","rank":5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob_e1"),
        obj("mob://hub"),
        obj("mob://a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("mob_e2"),
        obj("mob://hub"),
        obj("mob://b"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("mob_e3"),
        obj("mob://hub"),
        obj("mob://c"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINK*1]->(b) WHERE id(a) = 'mob://hub' RETURN b.dept, b.rank ORDER BY b.dept ASC, b.rank ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // art first (dept ASC), then eng sorted by rank ASC
    assert_eq!(rows[0].fields["dept"], serde_json::json!("art"));
    assert_eq!(rows[1].fields["rank"], serde_json::json!(1));
    assert_eq!(rows[2].fields["rank"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn multi_order_by_primary_desc_secondary_asc() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob2://hub"), b"{}".to_vec());
    tx.put_object(obj("mob2://a"), br#"{"grp":"x","pos":2}"#.to_vec());
    tx.put_object(obj("mob2://b"), br#"{"grp":"x","pos":1}"#.to_vec());
    tx.put_object(obj("mob2://c"), br#"{"grp":"y","pos":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob2_e1"),
        obj("mob2://hub"),
        obj("mob2://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob2_e2"),
        obj("mob2://hub"),
        obj("mob2://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob2_e3"),
        obj("mob2://hub"),
        obj("mob2://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'mob2://hub' RETURN b.grp, b.pos ORDER BY b.grp DESC, b.pos ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // grp DESC: y first, then x; within x, pos ASC: 1 before 2
    assert_eq!(rows[0].fields["grp"], serde_json::json!("y"));
    assert_eq!(rows[1].fields["pos"], serde_json::json!(1));
    assert_eq!(rows[2].fields["pos"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn multi_order_by_single_field_unchanged() {
    // Single-field ORDER BY still works with the Vec-based API
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("mob3://hub"), b"{}".to_vec());
    tx.put_object(obj("mob3://a"), br#"{"v":3}"#.to_vec());
    tx.put_object(obj("mob3://b"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("mob3://c"), br#"{"v":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("mob3_e1"),
        obj("mob3://hub"),
        obj("mob3://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob3_e2"),
        obj("mob3://hub"),
        obj("mob3://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("mob3_e3"),
        obj("mob3://hub"),
        obj("mob3://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'mob3://hub' RETURN b.v ORDER BY b.v ASC",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].fields["v"], serde_json::json!(1));
    assert_eq!(rows[1].fields["v"], serde_json::json!(2));
    assert_eq!(rows[2].fields["v"], serde_json::json!(3));
    store.close().unwrap();
}

// ── WITH multi-field ORDER BY ─────────────────────────────────────────────────

#[test]
fn with_multi_order_by_parse_roundtrip() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b ORDER BY b.dept ASC, b.rank DESC RETURN b",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.order_by.len(), 2);
    assert_eq!(
        wc.order_by[0].field,
        OrderByField::PayloadField("dept".into())
    );
    assert_eq!(wc.order_by[0].direction, OrderDirection::Asc);
    assert_eq!(
        wc.order_by[1].field,
        OrderByField::PayloadField("rank".into())
    );
    assert_eq!(wc.order_by[1].direction, OrderDirection::Desc);
}

#[test]
fn with_multi_order_by_sorts_pre_expansion() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wmob://hub"), b"{}".to_vec());
    tx.put_object(obj("wmob://a"), br#"{"grp":"x","pos":2}"#.to_vec());
    tx.put_object(obj("wmob://b"), br#"{"grp":"x","pos":1}"#.to_vec());
    tx.put_object(obj("wmob://c"), br#"{"grp":"y","pos":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wmob_e1"),
        obj("wmob://hub"),
        obj("wmob://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmob_e2"),
        obj("wmob://hub"),
        obj("wmob://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmob_e3"),
        obj("wmob://hub"),
        obj("wmob://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wmob://hub' WITH b ORDER BY b.grp ASC, b.pos ASC RETURN b.grp, b.pos",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    // grp ASC: x first, then y; within x, pos ASC: 1 before 2
    assert_eq!(rows[0].fields["grp"], serde_json::json!("x"));
    assert_eq!(rows[0].fields["pos"], serde_json::json!(1));
    assert_eq!(rows[1].fields["pos"], serde_json::json!(2));
    assert_eq!(rows[2].fields["grp"], serde_json::json!("y"));
    store.close().unwrap();
}

#[test]
fn with_multi_order_by_with_limit() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("wmobl://hub"), b"{}".to_vec());
    tx.put_object(obj("wmobl://a"), br#"{"tier":1,"rank":3}"#.to_vec());
    tx.put_object(obj("wmobl://b"), br#"{"tier":1,"rank":1}"#.to_vec());
    tx.put_object(obj("wmobl://c"), br#"{"tier":2,"rank":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("wmobl_e1"),
        obj("wmobl://hub"),
        obj("wmobl://a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmobl_e2"),
        obj("wmobl://hub"),
        obj("wmobl://b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("wmobl_e3"),
        obj("wmobl://hub"),
        obj("wmobl://c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:L*1]->(b) WHERE id(a) = 'wmobl://hub' WITH b ORDER BY b.tier ASC, b.rank ASC LIMIT 2 RETURN b.tier, b.rank",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].fields["tier"], serde_json::json!(1));
    assert_eq!(rows[0].fields["rank"], serde_json::json!(1));
    assert_eq!(rows[1].fields["tier"], serde_json::json!(1));
    assert_eq!(rows[1].fields["rank"], serde_json::json!(3));
    store.close().unwrap();
}

#[test]
fn with_single_order_by_still_works_as_vec() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' WITH b ORDER BY b.score DESC RETURN b",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.order_by.len(), 1);
    assert_eq!(wc.order_by[0].direction, OrderDirection::Desc);
}

// ── Numeric RETURN transforms ─────────────────────────────────────────────────

#[test]
fn return_abs_of_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnabs://hub"), b"{}".to_vec());
    tx.put_object(obj("rnabs://a"), br#"{"val":-4.5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnabs_e1"),
        obj("rnabs://hub"),
        obj("rnabs://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnabs://hub' RETURN ABS(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let v = rows[0].fields.get("val").and_then(|v| v.as_f64());
    assert!((v.unwrap() - 4.5).abs() < f64::EPSILON);
    store.close().unwrap();
}

#[test]
fn return_round_of_decimal() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnrd://hub"), b"{}".to_vec());
    tx.put_object(obj("rnrd://a"), br#"{"val":3.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnrd_e1"),
        obj("rnrd://hub"),
        obj("rnrd://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnrd://hub' RETURN ROUND(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(4.0));
    store.close().unwrap();
}

#[test]
fn return_ceil_and_floor() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rncf://hub"), b"{}".to_vec());
    tx.put_object(obj("rncf://a"), br#"{"hi":2.1,"lo":2.9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rncf_e1"),
        obj("rncf://hub"),
        obj("rncf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let ceil_rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rncf://hub' RETURN CEIL(b.hi)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(ceil_rows[0].fields["hi"], serde_json::json!(3.0));

    let floor_rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rncf://hub' RETURN FLOOR(b.lo)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(floor_rows[0].fields["lo"], serde_json::json!(2.0));
    store.close().unwrap();
}

#[test]
fn return_numeric_transform_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rntsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rntsp://a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rntsp_e1"),
        obj("rntsp://hub"),
        obj("rntsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    // ABS on a string should pass the value through unchanged
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rntsp://hub' RETURN ABS(b.name)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!("Alice"));
    store.close().unwrap();
}

// ── RETURN COALESCE / RETURN REPLACE tests ────────────────────────────────────

#[test]
fn return_coalesce_present_field_unchanged() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcpf://hub"), b"{}".to_vec());
    tx.put_object(obj("rcpf://a"), br#"{"label":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcpf_e1"),
        obj("rcpf://hub"),
        obj("rcpf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcpf://hub' RETURN COALESCE(b.label, 'default')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["label"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_coalesce_missing_field_uses_fallback() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcmf://hub"), b"{}".to_vec());
    tx.put_object(obj("rcmf://a"), br#"{"other":"x"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcmf_e1"),
        obj("rcmf://hub"),
        obj("rcmf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcmf://hub' RETURN COALESCE(b.label, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["label"], serde_json::json!("N/A"));
    store.close().unwrap();
}

#[test]
fn return_coalesce_parses_transform_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN COALESCE(b.name, 'unknown')",
    )
    .unwrap();
    assert_eq!(q.return_field_transforms.len(), 1);
    assert!(matches!(
        q.return_field_transforms.get("name"),
        Some(ReturnTransform::Coalesce(s)) if s == "unknown"
    ));
}

#[test]
fn return_replace_substitutes_substring() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrss://hub"), b"{}".to_vec());
    tx.put_object(obj("rrss://a"), br#"{"tag":"hello-world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrss_e1"),
        obj("rrss://hub"),
        obj("rrss://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrss://hub' RETURN REPLACE(b.tag, '-', '_')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hello_world"));
    store.close().unwrap();
}

#[test]
fn return_replace_no_match_returns_original() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrnm://hub"), b"{}".to_vec());
    tx.put_object(obj("rrnm://a"), br#"{"val":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrnm_e1"),
        obj("rrnm://hub"),
        obj("rrnm://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrnm://hub' RETURN REPLACE(b.val, 'x', 'y')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_replace_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrnsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rrnsp://a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrnsp_e1"),
        obj("rrnsp://hub"),
        obj("rrnsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrnsp://hub' RETURN REPLACE(b.n, '4', '5')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(42));
    store.close().unwrap();
}

#[test]
fn return_replace_parses_transform_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REPLACE(b.text, 'foo', 'bar')",
    )
    .unwrap();
    assert_eq!(q.return_field_transforms.len(), 1);
    assert!(matches!(
        q.return_field_transforms.get("text"),
        Some(ReturnTransform::Replace(f, t)) if f == "foo" && t == "bar"
    ));
}

#[test]
fn return_replace_all_occurrences() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrao://hub"), b"{}".to_vec());
    tx.put_object(obj("rrao://a"), br#"{"msg":"a-b-c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrao_e1"),
        obj("rrao://hub"),
        obj("rrao://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrao://hub' RETURN REPLACE(b.msg, '-', '/')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["msg"], serde_json::json!("a/b/c"));
    store.close().unwrap();
}

// ── RETURN LENGTH / RETURN SUBSTRING tests ────────────────────────────────────

#[test]
fn return_length_of_string_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlosf://hub"), b"{}".to_vec());
    tx.put_object(obj("rlosf://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlosf_e1"),
        obj("rlosf://hub"),
        obj("rlosf://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlosf://hub' RETURN LENGTH(b.word)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(5));
    store.close().unwrap();
}

#[test]
fn return_length_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlonsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rlonsp://a"), br#"{"n":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlonsp_e1"),
        obj("rlonsp://hub"),
        obj("rlonsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlonsp://hub' RETURN LENGTH(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(42));
    store.close().unwrap();
}

#[test]
fn return_length_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LENGTH(b.name)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("name"),
        Some(ReturnTransform::Length)
    ));
}

#[test]
fn return_substring_extracts_chars() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rsec://hub"), b"{}".to_vec());
    tx.put_object(obj("rsec://a"), br#"{"code":"ABCDE"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rsec_e1"),
        obj("rsec://hub"),
        obj("rsec://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rsec://hub' RETURN SUBSTRING(b.code, 1, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["code"], serde_json::json!("BCD"));
    store.close().unwrap();
}

#[test]
fn return_substring_start_beyond_end_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rssbe://hub"), b"{}".to_vec());
    tx.put_object(obj("rssbe://a"), br#"{"s":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rssbe_e1"),
        obj("rssbe://hub"),
        obj("rssbe://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rssbe://hub' RETURN SUBSTRING(b.s, 10, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!(""));
    store.close().unwrap();
}

#[test]
fn return_substring_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rsonsp://hub"), b"{}".to_vec());
    tx.put_object(obj("rsonsp://a"), br#"{"val":99}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rsonsp_e1"),
        obj("rsonsp://hub"),
        obj("rsonsp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rsonsp://hub' RETURN SUBSTRING(b.val, 0, 2)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(99));
    store.close().unwrap();
}

#[test]
fn return_substring_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SUBSTRING(b.text, 2, 5)")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("text"),
        Some(ReturnTransform::Substring(2, 5))
    ));
}

#[test]
fn return_substring_zero_len_gives_empty() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rszlge://hub"), b"{}".to_vec());
    tx.put_object(obj("rszlge://a"), br#"{"name":"Alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rszlge_e1"),
        obj("rszlge://hub"),
        obj("rszlge://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rszlge://hub' RETURN SUBSTRING(b.name, 0, 0)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!(""));
    store.close().unwrap();
}

// ── RETURN CAST / RETURN NULLIF tests ─────────────────────────────────────────

#[test]
fn return_cast_as_int_from_float() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcaif://hub"), b"{}".to_vec());
    tx.put_object(obj("rcaif://a"), br#"{"score":9.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcaif_e1"),
        obj("rcaif://hub"),
        obj("rcaif://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcaif://hub' RETURN CAST(b.score AS INT)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["score"], serde_json::json!(9i64));
    store.close().unwrap();
}

#[test]
fn return_cast_as_float_from_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcafs://hub"), b"{}".to_vec());
    tx.put_object(obj("rcafs://a"), br#"{"val":"2.5"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcafs_e1"),
        obj("rcafs://hub"),
        obj("rcafs://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcafs://hub' RETURN CAST(b.val AS FLOAT)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 2.5).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cast_as_string_from_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rcasfn://hub"), b"{}".to_vec());
    tx.put_object(obj("rcasfn://a"), br#"{"id":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rcasfn_e1"),
        obj("rcasfn://hub"),
        obj("rcasfn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rcasfn://hub' RETURN CAST(b.id AS STRING)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["id"], serde_json::json!("100"));
    store.close().unwrap();
}

#[test]
fn return_cast_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN CAST(b.n AS INT)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("n"),
        Some(ReturnTransform::CastInt)
    ));
}

#[test]
fn return_nullif_matching_value_omits_field() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnimvof://hub"), b"{}".to_vec());
    tx.put_object(obj("rnimvof://a"), br#"{"status":"N/A"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnimvof_e1"),
        obj("rnimvof://hub"),
        obj("rnimvof://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnimvof://hub' RETURN NULLIF(b.status, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert!(!rows[0].fields.contains_key("status"));
    store.close().unwrap();
}

#[test]
fn return_nullif_non_matching_value_kept() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnnmvk://hub"), b"{}".to_vec());
    tx.put_object(obj("rnnmvk://a"), br#"{"status":"active"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnnmvk_e1"),
        obj("rnnmvk://hub"),
        obj("rnnmvk://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnnmvk://hub' RETURN NULLIF(b.status, 'N/A')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["status"], serde_json::json!("active"));
    store.close().unwrap();
}

#[test]
fn return_nullif_on_non_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rnonsp2://hub"), b"{}".to_vec());
    tx.put_object(obj("rnonsp2://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rnonsp2_e1"),
        obj("rnonsp2://hub"),
        obj("rnonsp2://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rnonsp2://hub' RETURN NULLIF(b.n, '0')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_nullif_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN NULLIF(b.status, 'none')",
    )
    .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("status"),
        Some(ReturnTransform::NullIf(s)) if s == "none"
    ));
}

// ── REVERSE / INITCAP / LPAD / RPAD transform tests ─────────────────────────

#[test]
fn return_reverse_reverses_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrev://hub"), b"{}".to_vec());
    tx.put_object(obj("rrev://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrev_e1"),
        obj("rrev://hub"),
        obj("rrev://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrev://hub' RETURN REVERSE(b.word)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("olleh"));
    store.close().unwrap();
}

#[test]
fn return_reverse_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REVERSE(b.word)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Reverse)
    ));
}

#[test]
fn return_initcap_capitalises_words() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ricp://hub"), b"{}".to_vec());
    tx.put_object(obj("ricp://a"), br#"{"title":"hello world"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ricp_e1"),
        obj("ricp://hub"),
        obj("ricp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'ricp://hub' RETURN INITCAP(b.title)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["title"], serde_json::json!("Hello World"));
    store.close().unwrap();
}

#[test]
fn return_initcap_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN INITCAP(b.title)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("title"),
        Some(ReturnTransform::InitCap)
    ));
}

#[test]
fn return_lpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rlpad://hub"), b"{}".to_vec());
    tx.put_object(obj("rlpad://a"), br#"{"code":"42"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rlpad_e1"),
        obj("rlpad://hub"),
        obj("rlpad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rlpad://hub' RETURN LPAD(b.code, 5, '0')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["code"], serde_json::json!("00042"));
    store.close().unwrap();
}

#[test]
fn return_lpad_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LPAD(b.code, 8, '0')")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("code"),
        Some(ReturnTransform::Lpad(8, s)) if s == "0"
    ));
}

#[test]
fn return_rpad_pads_short_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("rrpad://hub"), b"{}".to_vec());
    tx.put_object(obj("rrpad://a"), br#"{"name":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("rrpad_e1"),
        obj("rrpad://hub"),
        obj("rrpad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'rrpad://hub' RETURN RPAD(b.name, 5, '.')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["name"], serde_json::json!("hi..."));
    store.close().unwrap();
}

#[test]
fn return_rpad_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN RPAD(b.label, 10, ' ')")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("label"),
        Some(ReturnTransform::Rpad(10, s)) if s == " "
    ));
}

// ── REPEAT / LEFT / RIGHT transform tests ────────────────────────────────────

#[test]
fn return_repeat_duplicates_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grep://hub"), b"{}".to_vec());
    tx.put_object(obj("grep://a"), br#"{"sep":"-"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grep_e1"),
        obj("grep://hub"),
        obj("grep://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grep://hub' RETURN REPEAT(b.sep, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["sep"], serde_json::json!("---"));
    store.close().unwrap();
}

#[test]
fn return_repeat_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN REPEAT(b.sep, 4)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("sep"),
        Some(ReturnTransform::Repeat(4))
    ));
}

#[test]
fn return_left_extracts_prefix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gleft://hub"), b"{}".to_vec());
    tx.put_object(obj("gleft://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gleft_e1"),
        obj("gleft://hub"),
        obj("gleft://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gleft://hub' RETURN LEFT(b.word, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("hel"));
    store.close().unwrap();
}

#[test]
fn return_left_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN LEFT(b.word, 3)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Left(3))
    ));
}

#[test]
fn return_right_extracts_suffix() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grght://hub"), b"{}".to_vec());
    tx.put_object(obj("grght://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grght_e1"),
        obj("grght://hub"),
        obj("grght://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grght://hub' RETURN RIGHT(b.word, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!("llo"));
    store.close().unwrap();
}

#[test]
fn return_right_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN RIGHT(b.word, 3)")
        .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("word"),
        Some(ReturnTransform::Right(3))
    ));
}

#[test]
fn return_left_beyond_length_returns_full_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gleftbl://hub"), b"{}".to_vec());
    tx.put_object(obj("gleftbl://a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gleftbl_e1"),
        obj("gleftbl://hub"),
        obj("gleftbl://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gleftbl://hub' RETURN LEFT(b.tag, 100)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hi"));
    store.close().unwrap();
}

#[test]
fn return_right_zero_gives_empty_string() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grght0://hub"), b"{}".to_vec());
    tx.put_object(obj("grght0://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grght0_e1"),
        obj("grght0://hub"),
        obj("grght0://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grght0://hub' RETURN RIGHT(b.tag, 0)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!(""));
    store.close().unwrap();
}
