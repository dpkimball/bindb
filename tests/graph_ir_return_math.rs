#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── SIGN / SQRT transform tests ───────────────────────────────────────────────

#[test]
fn return_sign_positive_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignp://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignp://a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignp_e1"),
        obj("gsignp://hub"),
        obj("gsignp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignp://hub' RETURN SIGN(b.score)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["score"], serde_json::json!(1));
    store.close().unwrap();
}

#[test]
fn return_sign_negative_number() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignn://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignn://a"), br#"{"val":-7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignn_e1"),
        obj("gsignn://hub"),
        obj("gsignn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignn://hub' RETURN SIGN(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(-1));
    store.close().unwrap();
}

#[test]
fn return_sign_parses_correctly() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SIGN(b.score)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("score"),
        Some(ReturnTransform::Sign)
    ));
}

#[test]
fn return_sqrt_of_perfect_square() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsqrt://hub"), b"{}".to_vec());
    tx.put_object(obj("gsqrt://a"), br#"{"n":9}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsqrt_e1"),
        obj("gsqrt://hub"),
        obj("gsqrt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsqrt://hub' RETURN SQRT(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_sqrt_parses_correctly() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x' RETURN SQRT(b.n)").unwrap();
    assert!(matches!(
        q.return_field_transforms.get("n"),
        Some(ReturnTransform::Sqrt)
    ));
}

#[test]
fn return_sqrt_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsqrts://hub"), b"{}".to_vec());
    tx.put_object(obj("gsqrts://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsqrts_e1"),
        obj("gsqrts://hub"),
        obj("gsqrts://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsqrts://hub' RETURN SQRT(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_sign_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignz://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignz://a"), br#"{"val":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignz_e1"),
        obj("gsignz://hub"),
        obj("gsignz://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignz://hub' RETURN SIGN(b.val)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["val"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_sign_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsignstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gsignstr://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsignstr_e1"),
        obj("gsignstr://hub"),
        obj("gsignstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsignstr://hub' RETURN SIGN(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_power_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gpow://hub"), b"{}".to_vec());
    tx.put_object(obj("gpow://a"), br#"{"val":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gpow_e1"),
        obj("gpow://hub"),
        obj("gpow://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gpow://hub' RETURN POWER(b.val, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 8.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_power_parse() {
    use bindb::ReturnTransform;
    let q =
        GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN POWER(b.val, 3)")
            .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("val"),
        Some(ReturnTransform::Power(e)) if (e - 3.0).abs() < 1e-9
    ));
}

#[test]
fn return_mod_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gmod://hub"), b"{}".to_vec());
    tx.put_object(obj("gmod://a"), br#"{"val":10}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gmod_e1"),
        obj("gmod://hub"),
        obj("gmod://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gmod://hub' RETURN MOD(b.val, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["val"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_mod_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gmodstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gmodstr://a"), br#"{"tag":"hi"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gmodstr_e1"),
        obj("gmodstr://hub"),
        obj("gmodstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gmodstr://hub' RETURN MOD(b.tag, 3)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("hi"));
    store.close().unwrap();
}

#[test]
fn return_strpos_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gstrpos://hub"), b"{}".to_vec());
    tx.put_object(obj("gstrpos://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gstrpos_e1"),
        obj("gstrpos://hub"),
        obj("gstrpos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gstrpos://hub' RETURN STRPOS(b.word, 'l')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(3));
    store.close().unwrap();
}

#[test]
fn return_strpos_not_found() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gstrpno://hub"), b"{}".to_vec());
    tx.put_object(obj("gstrpno://a"), br#"{"word":"hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gstrpno_e1"),
        obj("gstrpno://hub"),
        obj("gstrpno://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gstrpno://hub' RETURN STRPOS(b.word, 'z')",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["word"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_split_part_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsplitpt://hub"), b"{}".to_vec());
    tx.put_object(obj("gsplitpt://a"), br#"{"csv":"a,b,c"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsplitpt_e1"),
        obj("gsplitpt://hub"),
        obj("gsplitpt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsplitpt://hub' RETURN SPLIT_PART(b.csv, ',', 2)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["csv"], serde_json::json!("b"));
    store.close().unwrap();
}

#[test]
fn return_split_part_parse() {
    use bindb::ReturnTransform;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN SPLIT_PART(b.csv, ',', 2)",
    )
    .unwrap();
    assert!(matches!(
        q.return_field_transforms.get("csv"),
        Some(ReturnTransform::SplitPart(d, 2)) if d == ","
    ));
}

#[test]
fn return_ltrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gltrim://hub"), b"{}".to_vec());
    tx.put_object(obj("gltrim://a"), br#"{"s":"  hello"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gltrim_e1"),
        obj("gltrim://hub"),
        obj("gltrim://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gltrim://hub' RETURN LTRIM(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_rtrim_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grtrim://hub"), b"{}".to_vec());
    tx.put_object(obj("grtrim://a"), br#"{"s":"hello  "}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grtrim_e1"),
        obj("grtrim://hub"),
        obj("grtrim://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grtrim://hub' RETURN RTRIM(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("hello"));
    store.close().unwrap();
}

#[test]
fn return_log_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog://hub"), b"{}".to_vec());
    tx.put_object(obj("glog://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog_e1"),
        obj("glog://hub"),
        obj("glog://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'glog://hub' RETURN LOG(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_log_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog0://hub"), b"{}".to_vec());
    tx.put_object(obj("glog0://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog0_e1"),
        obj("glog0://hub"),
        obj("glog0://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'glog0://hub' RETURN LOG(b.n)")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_exp_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gexp://hub"), b"{}".to_vec());
    tx.put_object(obj("gexp://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gexp_e1"),
        obj("gexp://hub"),
        obj("gexp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gexp://hub' RETURN EXP(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_exp_on_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gexpstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gexpstr://a"), br#"{"tag":"abc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gexpstr_e1"),
        obj("gexpstr://hub"),
        obj("gexpstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gexpstr://hub' RETURN EXP(b.tag)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["tag"], serde_json::json!("abc"));
    store.close().unwrap();
}

#[test]
fn return_log2_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog2://hub"), b"{}".to_vec());
    tx.put_object(obj("glog2://a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog2_e1"),
        obj("glog2://hub"),
        obj("glog2://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog2://hub' RETURN LOG2(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_log2_non_positive_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog2z://hub"), b"{}".to_vec());
    tx.put_object(obj("glog2z://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog2z_e1"),
        obj("glog2z://hub"),
        obj("glog2z://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog2z://hub' RETURN LOG2(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(0));
    store.close().unwrap();
}

#[test]
fn return_log10_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("glog10://hub"), b"{}".to_vec());
    tx.put_object(obj("glog10://a"), br#"{"n":100}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("glog10_e1"),
        obj("glog10://hub"),
        obj("glog10://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'glog10://hub' RETURN LOG10(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_radians_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("grad://hub"), b"{}".to_vec());
    tx.put_object(obj("grad://a"), br#"{"deg":180}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("grad_e1"),
        obj("grad://hub"),
        obj("grad://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'grad://hub' RETURN RADIANS(b.deg)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["deg"].as_f64().unwrap();
    assert!((v - std::f64::consts::PI).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_degrees_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gdeg://hub"), b"{}".to_vec());
    tx.put_object(
        obj("gdeg://a"),
        format!(r#"{{"rad":{}}}"#, std::f64::consts::PI).into_bytes(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gdeg_e1"),
        obj("gdeg://hub"),
        obj("gdeg://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gdeg://hub' RETURN DEGREES(b.rad)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["rad"].as_f64().unwrap();
    assert!((v - 180.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_radians_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gradstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gradstr://a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gradstr_e1"),
        obj("gradstr://hub"),
        obj("gradstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gradstr://hub' RETURN RADIANS(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("north"));
    store.close().unwrap();
}

#[test]
fn return_degrees_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gdegstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gdegstr://a"), br#"{"s":"north"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gdegstr_e1"),
        obj("gdegstr://hub"),
        obj("gdegstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gdegstr://hub' RETURN DEGREES(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("north"));
    store.close().unwrap();
}

#[test]
fn return_sin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsin://hub"), b"{}".to_vec());
    tx.put_object(obj("gsin://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsin_e1"),
        obj("gsin://hub"),
        obj("gsin://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gsin://hub' RETURN SIN(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_sin_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gsinstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gsinstr://a"), br#"{"s":"wave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gsinstr_e1"),
        obj("gsinstr://hub"),
        obj("gsinstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gsinstr://hub' RETURN SIN(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("wave"));
    store.close().unwrap();
}

#[test]
fn return_cos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcos://hub"), b"{}".to_vec());
    tx.put_object(obj("gcos://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcos_e1"),
        obj("gcos://hub"),
        obj("gcos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gcos://hub' RETURN COS(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_tan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtan://hub"), b"{}".to_vec());
    tx.put_object(obj("gtan://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtan_e1"),
        obj("gtan://hub"),
        obj("gtan://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'gtan://hub' RETURN TAN(b.n)")
                .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_atan_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gatan://hub"), b"{}".to_vec());
    tx.put_object(obj("gatan://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gatan_e1"),
        obj("gatan://hub"),
        obj("gatan://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gatan://hub' RETURN ATAN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - std::f64::consts::FRAC_PI_4).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_atan_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gatanstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gatanstr://a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gatanstr_e1"),
        obj("gatanstr://hub"),
        obj("gatanstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gatanstr://hub' RETURN ATAN(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("arc"));
    store.close().unwrap();
}

#[test]
fn return_asin_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gasin://hub"), b"{}".to_vec());
    tx.put_object(obj("gasin://a"), br#"{"n":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gasin_e1"),
        obj("gasin://hub"),
        obj("gasin://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gasin://hub' RETURN ASIN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_asin_out_of_domain_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gasinood://hub"), b"{}".to_vec());
    tx.put_object(obj("gasinood://a"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gasinood_e1"),
        obj("gasinood://hub"),
        obj("gasinood://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gasinood://hub' RETURN ASIN(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["n"], serde_json::json!(2));
    store.close().unwrap();
}

#[test]
fn return_acos_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gacos://hub"), b"{}".to_vec());
    tx.put_object(obj("gacos://a"), br#"{"n":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gacos_e1"),
        obj("gacos://hub"),
        obj("gacos://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gacos://hub' RETURN ACOS(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!(v.abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_acos_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gacosstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gacosstr://a"), br#"{"s":"arc"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gacosstr_e1"),
        obj("gacosstr://hub"),
        obj("gacosstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gacosstr://hub' RETURN ACOS(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("arc"));
    store.close().unwrap();
}

#[test]
fn return_trunc_negative() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtruncn://hub"), b"{}".to_vec());
    tx.put_object(obj("gtruncn://a"), br#"{"n":-1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtruncn_e1"),
        obj("gtruncn://hub"),
        obj("gtruncn://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gtruncn://hub' RETURN TRUNC(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - (-1.0)).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_trunc_positive() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gtruncp://hub"), b"{}".to_vec());
    tx.put_object(obj("gtruncp://a"), br#"{"n":1.7}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gtruncp_e1"),
        obj("gtruncp://hub"),
        obj("gtruncp://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gtruncp://hub' RETURN TRUNC(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cbrt_exec() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcbrt://hub"), b"{}".to_vec());
    tx.put_object(obj("gcbrt://a"), br#"{"n":8}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcbrt_e1"),
        obj("gcbrt://hub"),
        obj("gcbrt://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gcbrt://hub' RETURN CBRT(b.n)",
            )
            .unwrap(),
        )
        .unwrap();
    let v = rows[0].fields["n"].as_f64().unwrap();
    assert!((v - 2.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn return_cbrt_string_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("gcbrtstr://hub"), b"{}".to_vec());
    tx.put_object(obj("gcbrtstr://a"), br#"{"s":"root"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("gcbrtstr_e1"),
        obj("gcbrtstr://hub"),
        obj("gcbrtstr://a"),
        "X",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:X]->(b) WHERE id(a) = 'gcbrtstr://hub' RETURN CBRT(b.s)",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows[0].fields["s"], serde_json::json!("root"));
    store.close().unwrap();
}

// --- UNION ---

#[test]
fn union_merges_two_edge_types() {
    // Alice -FOLLOWS-> Bob and Alice -LIKED_BY-> Carol; UNION should return both.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn://alice"), b"{}".to_vec());
    tx.put_object(obj("sn://bob"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("sn://carol"), br#"{"name":"carol"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("uf_e1"),
        obj("sn://alice"),
        obj("sn://bob"),
        "FOLLOWS",
        None,
    );
    tx2.put_edge(
        eid("uf_e2"),
        obj("sn://alice"),
        obj("sn://carol"),
        "LIKED_BY",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:FOLLOWS]->(b) WHERE id(a) = 'sn://alice' RETURN b.name \
             UNION \
             MATCH (a)-[:LIKED_BY]->(b) WHERE id(a) = 'sn://alice' RETURN b.name",
        )
        .unwrap();
    let names: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| {
            r.fields
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(names.contains("bob"), "expected bob in union");
    assert!(names.contains("carol"), "expected carol in union");
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn union_deduplicates_same_node() {
    // Same node reachable via two edge types; UNION deduplicates by object_id.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn2://hub"), b"{}".to_vec());
    tx.put_object(obj("sn2://shared"), br#"{"name":"shared"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ud_e1"),
        obj("sn2://hub"),
        obj("sn2://shared"),
        "EDGE_A",
        None,
    );
    tx2.put_edge(
        eid("ud_e2"),
        obj("sn2://hub"),
        obj("sn2://shared"),
        "EDGE_B",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:EDGE_A]->(b) WHERE id(a) = 'sn2://hub' RETURN b.name \
             UNION \
             MATCH (a)-[:EDGE_B]->(b) WHERE id(a) = 'sn2://hub' RETURN b.name",
        )
        .unwrap();
    // UNION should deduplicate: shared node appears once.
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("shared")
    );
    store.close().unwrap();
}

#[test]
fn union_all_preserves_duplicates() {
    // Same node reachable via two edge types; UNION ALL keeps both rows.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn3://hub"), b"{}".to_vec());
    tx.put_object(obj("sn3://shared"), br#"{"name":"shared"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ua_e1"),
        obj("sn3://hub"),
        obj("sn3://shared"),
        "EDGE_A",
        None,
    );
    tx2.put_edge(
        eid("ua_e2"),
        obj("sn3://hub"),
        obj("sn3://shared"),
        "EDGE_B",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:EDGE_A]->(b) WHERE id(a) = 'sn3://hub' RETURN b.name \
             UNION ALL \
             MATCH (a)-[:EDGE_B]->(b) WHERE id(a) = 'sn3://hub' RETURN b.name",
        )
        .unwrap();
    // UNION ALL: both rows preserved.
    assert_eq!(rows.len(), 2);
    store.close().unwrap();
}

#[test]
fn union_three_branches() {
    // Three independent hubs; UNION of three MATCH clauses merges all results.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn4://h1"), b"{}".to_vec());
    tx.put_object(obj("sn4://h2"), b"{}".to_vec());
    tx.put_object(obj("sn4://h3"), b"{}".to_vec());
    tx.put_object(obj("sn4://n1"), br#"{"v":1}"#.to_vec());
    tx.put_object(obj("sn4://n2"), br#"{"v":2}"#.to_vec());
    tx.put_object(obj("sn4://n3"), br#"{"v":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("u3_e1"), obj("sn4://h1"), obj("sn4://n1"), "X", None);
    tx2.put_edge(eid("u3_e2"), obj("sn4://h2"), obj("sn4://n2"), "X", None);
    tx2.put_edge(eid("u3_e3"), obj("sn4://h3"), obj("sn4://n3"), "X", None);
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_union(
            "MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h1' RETURN b.v \
             UNION \
             MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h2' RETURN b.v \
             UNION \
             MATCH (a)-[:X]->(b) WHERE id(a) = 'sn4://h3' RETURN b.v",
        )
        .unwrap();
    let vals: std::collections::HashSet<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("v").and_then(|v| v.as_i64()))
        .collect();
    assert_eq!(vals, [1, 2, 3].into_iter().collect());
    store.close().unwrap();
}

#[test]
fn union_parse_error_on_single_match() {
    // Parsing a single MATCH as a UNION query must fail.
    let err = GraphIrUnionQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://hub' RETURN b.name");
    assert!(err.is_err());
}

#[test]
fn union_projected_api() {
    // Test graph_ir_union_projected with pre-parsed query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("sn5://hub"), b"{}".to_vec());
    tx.put_object(obj("sn5://a"), br#"{"n":1}"#.to_vec());
    tx.put_object(obj("sn5://b"), br#"{"n":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(eid("up_e1"), obj("sn5://hub"), obj("sn5://a"), "A", None);
    tx2.put_edge(eid("up_e2"), obj("sn5://hub"), obj("sn5://b"), "B", None);
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrUnionQuery::parse(
        "MATCH (a)-[:A]->(b) WHERE id(a) = 'sn5://hub' RETURN b.n \
         UNION \
         MATCH (a)-[:B]->(b) WHERE id(a) = 'sn5://hub' RETURN b.n",
    )
    .unwrap();
    assert_eq!(q.branches.len(), 2);
    assert!(!q.all);

    let rows = store.graph_ir_union_projected(&q).unwrap();
    let ns: std::collections::HashSet<i64> = rows
        .iter()
        .filter_map(|r| r.fields.get("n").and_then(|v| v.as_i64()))
        .collect();
    assert!(ns.contains(&1));
    assert!(ns.contains(&2));
    store.close().unwrap();
}

// --- SET NODE / DELETE NODE / DELETE EDGE write IR ---

#[test]
fn write_ir_set_node_replaces_payload() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store
        .graph_ir_write(r#"CREATE NODE id="upd://n1" payload={"v":1}"#)
        .unwrap();
    store
        .graph_ir_write(r#"SET NODE id="upd://n1" payload={"v":99}"#)
        .unwrap();
    let raw = store.get(&obj("upd://n1")).unwrap().unwrap();
    let val: serde_json::Value = serde_json::from_slice(raw.payload.unwrap().as_slice()).unwrap();
    assert_eq!(val["v"], serde_json::json!(99));
    store.close().unwrap();
}

#[test]
fn write_ir_set_node_merge_payload_merges_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store
        .graph_ir_write(r#"CREATE NODE id="mrg://n1" payload={"a":1,"b":2}"#)
        .unwrap();
    store
        .graph_ir_write(r#"SET NODE id="mrg://n1" merge_payload={"b":20,"c":30}"#)
        .unwrap();
    let raw = store.get(&obj("mrg://n1")).unwrap().unwrap();
    let val: serde_json::Value = serde_json::from_slice(raw.payload.unwrap().as_slice()).unwrap();
    // 'a' preserved, 'b' overwritten, 'c' added.
    assert_eq!(val["a"], serde_json::json!(1));
    assert_eq!(val["b"], serde_json::json!(20));
    assert_eq!(val["c"], serde_json::json!(30));
    store.close().unwrap();
}

#[test]
fn write_ir_set_node_merge_creates_if_absent() {
    // merge_payload on a non-existent node creates it.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store
        .graph_ir_write(r#"SET NODE id="new://n1" merge_payload={"x":7}"#)
        .unwrap();
    let raw = store.get(&obj("new://n1")).unwrap().unwrap();
    let val: serde_json::Value = serde_json::from_slice(raw.payload.unwrap().as_slice()).unwrap();
    assert_eq!(val["x"], serde_json::json!(7));
    store.close().unwrap();
}

#[test]
fn write_ir_delete_node_removes_it() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store
        .graph_ir_write(r#"CREATE NODE id="del://n1" payload={"v":1}"#)
        .unwrap();
    assert!(store.get(&obj("del://n1")).unwrap().is_some());
    store
        .graph_ir_write(r#"DELETE NODE id="del://n1""#)
        .unwrap();
    assert!(store.get(&obj("del://n1")).unwrap().is_none());
    store.close().unwrap();
}

#[test]
fn write_ir_delete_edge_removes_it() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    store
        .graph_ir_write(
            "CREATE NODE id=\"de://a\" payload={}\nCREATE NODE id=\"de://b\" payload={}",
        )
        .unwrap();
    store
        .graph_ir_write(r#"CREATE EDGE id="de://e1" from="de://a" to="de://b" label="LINK""#)
        .unwrap();
    // Verify edge exists via traversal.
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:LINK]->(b) WHERE id(a) = 'de://a' RETURN id(b)")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 1);

    store.graph_ir_write(r#"DELETE EDGE id="de://e1""#).unwrap();

    // Edge gone — no traversal results.
    let rows2 = store
        .graph_ir_projected(
            &GraphIrQuery::parse("MATCH (a)-[:LINK]->(b) WHERE id(a) = 'de://a' RETURN id(b)")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(rows2.len(), 0);
    store.close().unwrap();
}

#[test]
fn write_ir_set_delete_parse_error_unknown_verb() {
    let err = GraphIrWrite::parse("UPSERT NODE id=\"x://n\" payload={}");
    assert!(err.is_err());
}

// --- Chained MATCH (multi-hop traversal) ---

#[test]
fn chained_match_two_hops() {
    // Alice -KNOWS-> Bob -LIKES-> Carol; chained MATCH finds Carol from Alice.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ch://alice"), b"{}".to_vec());
    tx.put_object(obj("ch://bob"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("ch://carol"), br#"{"name":"carol"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ch_e1"),
        obj("ch://alice"),
        obj("ch://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("ch_e2"),
        obj("ch://bob"),
        obj("ch://carol"),
        "LIKES",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_chained(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'ch://alice' \
             MATCH (a)-[:LIKES]->(b) RETURN b.name",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("carol")
    );
    store.close().unwrap();
}

#[test]
fn chained_match_three_hops() {
    // Alice -KNOWS-> Bob -LIKES-> Carol -FOLLOWS-> Dave
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("ch3://alice"), b"{}".to_vec());
    tx.put_object(obj("ch3://bob"), b"{}".to_vec());
    tx.put_object(obj("ch3://carol"), b"{}".to_vec());
    tx.put_object(obj("ch3://dave"), br#"{"name":"dave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("ch3_e1"),
        obj("ch3://alice"),
        obj("ch3://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("ch3_e2"),
        obj("ch3://bob"),
        obj("ch3://carol"),
        "LIKES",
        None,
    );
    tx2.put_edge(
        eid("ch3_e3"),
        obj("ch3://carol"),
        obj("ch3://dave"),
        "FOLLOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_chained(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'ch3://alice' \
             MATCH (a)-[:LIKES]->(b) \
             MATCH (a)-[:FOLLOWS]->(b) RETURN b.name",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("dave")
    );
    store.close().unwrap();
}

#[test]
fn chained_match_fan_out() {
    // Alice -KNOWS-> Bob and Alice -KNOWS-> Carol; Bob and Carol both -LIKES-> Dave.
    // Two-hop chain returns Dave twice (once per path).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("chf://alice"), b"{}".to_vec());
    tx.put_object(obj("chf://bob"), b"{}".to_vec());
    tx.put_object(obj("chf://carol"), b"{}".to_vec());
    tx.put_object(obj("chf://dave"), br#"{"name":"dave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("chf_e1"),
        obj("chf://alice"),
        obj("chf://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("chf_e2"),
        obj("chf://alice"),
        obj("chf://carol"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("chf_e3"),
        obj("chf://bob"),
        obj("chf://dave"),
        "LIKES",
        None,
    );
    tx2.put_edge(
        eid("chf_e4"),
        obj("chf://carol"),
        obj("chf://dave"),
        "LIKES",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_chained(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'chf://alice' \
             MATCH (a)-[:LIKES]->(b) RETURN b.name",
        )
        .unwrap();
    // Two paths both reach dave.
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r.fields.get("name").and_then(|v| v.as_str()) == Some("dave")));
    store.close().unwrap();
}

#[test]
fn chained_match_no_results_when_hop_missing() {
    // Alice -KNOWS-> Bob; no LIKES edge from Bob — chain returns empty.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("chn://alice"), b"{}".to_vec());
    tx.put_object(obj("chn://bob"), b"{}".to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("chn_e1"),
        obj("chn://alice"),
        obj("chn://bob"),
        "KNOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_chained(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'chn://alice' \
             MATCH (a)-[:LIKES]->(b) RETURN b.name",
        )
        .unwrap();
    assert!(rows.is_empty());
    store.close().unwrap();
}

#[test]
fn chained_match_with_end_predicate() {
    // Alice -KNOWS-> Bob (score=5) and Alice -KNOWS-> Carol (score=10).
    // Both -LIKES-> Dave; second step filters b.score > 8 so only Carol's path continues.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("chp://alice"), b"{}".to_vec());
    tx.put_object(obj("chp://bob"), br#"{"score":5}"#.to_vec());
    tx.put_object(obj("chp://carol"), br#"{"score":10}"#.to_vec());
    tx.put_object(obj("chp://dave"), br#"{"name":"dave"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("chp_e1"),
        obj("chp://alice"),
        obj("chp://bob"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("chp_e2"),
        obj("chp://alice"),
        obj("chp://carol"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("chp_e3"),
        obj("chp://bob"),
        obj("chp://dave"),
        "LIKES",
        None,
    );
    tx2.put_edge(
        eid("chp_e4"),
        obj("chp://carol"),
        obj("chp://dave"),
        "LIKES",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    // First step: KNOWS from alice, filter b.score > 8 → only carol
    let rows = store
        .graph_ir_chained(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'chp://alice' AND b.score > 8 \
             MATCH (a)-[:LIKES]->(b) RETURN b.name",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("dave")
    );
    store.close().unwrap();
}

#[test]
fn chained_match_parse_error_on_single_match() {
    let err = GraphIrChainedQuery::parse("MATCH (a)-[:X]->(b) WHERE id(a) = 'x://a' RETURN b.name");
    assert!(err.is_err());
}
