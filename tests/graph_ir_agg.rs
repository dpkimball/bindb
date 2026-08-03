#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── WITH aggregation (issue #2) ───────────────────────────────────────────────

#[test]
fn agg_parse_count_single() {
    // Parser: WITH count(b) AS total → one WithAggregation, func=Count, field=None.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH count(b) AS total RETURN total",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().expect("with_clause must be Some");
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(
        wc.aggregations[0],
        WithAggregation {
            func: AggFunc::Count,
            field: None,
            alias: "total".into(),
        }
    );
}

#[test]
fn agg_parse_multi_aggregations() {
    // Parser: WITH count(b) AS cnt, sum(b.score) AS s, avg(b.score) AS a.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH count(b) AS cnt, sum(b.score) AS s, avg(b.score) AS a RETURN cnt",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.aggregations.len(), 3);
    assert_eq!(wc.aggregations[0].func, AggFunc::Count);
    assert_eq!(wc.aggregations[0].alias, "cnt");
    assert_eq!(wc.aggregations[1].func, AggFunc::Sum);
    assert_eq!(wc.aggregations[1].field.as_deref(), Some("score"));
    assert_eq!(wc.aggregations[1].alias, "s");
    assert_eq!(wc.aggregations[2].func, AggFunc::Avg);
    assert_eq!(wc.aggregations[2].alias, "a");
}

#[test]
fn agg_parse_min_max() {
    // Parser: WITH min(b.score) AS lo, max(b.score) AS hi.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH min(b.score) AS lo, max(b.score) AS hi RETURN lo, hi",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.aggregations.len(), 2);
    assert_eq!(wc.aggregations[0].func, AggFunc::Min);
    assert_eq!(wc.aggregations[0].field.as_deref(), Some("score"));
    assert_eq!(wc.aggregations[1].func, AggFunc::Max);
    assert_eq!(wc.aggregations[1].field.as_deref(), Some("score"));
}

#[test]
fn agg_parse_existing_with_still_works() {
    // Regression: pass-through WITH b still parses correctly after aggregation support added.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE b.score > 5 ORDER BY b.score DESC LIMIT 2 RETURN b.name",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert!(
        wc.aggregations.is_empty(),
        "pass-through WITH has no aggregations"
    );
    assert_eq!(wc.payload_preds.len(), 1);
    assert_eq!(wc.limit, Some(2));
}

#[test]
fn agg_count_returns_total_matched() {
    // count(b) returns the number of matched b nodes.
    // build_optional_graph: alice KNOWS bob, carol, dave → 3 matches.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH count(b) AS total RETURN total",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("total").and_then(|v| v.as_u64()), Some(3));
    store.close().unwrap();
}

#[test]
fn agg_count_zero_when_no_match() {
    // count(b) = 0 when the MATCH predicate filters everything out.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'nobody://' \
             WITH count(b) AS n RETURN n",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(0));
    store.close().unwrap();
}

#[test]
fn agg_sum_numeric_field() {
    // bob score=10, carol score=5, dave score=20 → sum = 35.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH sum(b.score) AS total_score RETURN total_score",
        )
        .unwrap();
    let row = &result[0];
    let v = row
        .get("total_score")
        .and_then(|v| v.as_f64())
        .expect("total_score must be a number");
    assert!((v - 35.0).abs() < 1e-9, "expected 35.0, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_avg_numeric_field() {
    // (10 + 5 + 20) / 3 ≈ 11.667.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH avg(b.score) AS avg_score RETURN avg_score",
        )
        .unwrap();
    let row = &result[0];
    let v = row
        .get("avg_score")
        .and_then(|v| v.as_f64())
        .expect("avg_score must be a number");
    assert!((v - 35.0 / 3.0).abs() < 1e-9, "expected ≈11.667, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_min_max_numeric_field() {
    // min(b.score)=5 (carol), max(b.score)=20 (dave).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH min(b.score) AS lo, max(b.score) AS hi RETURN lo, hi",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("lo").and_then(|v| v.as_f64()), Some(5.0));
    assert_eq!(row.get("hi").and_then(|v| v.as_f64()), Some(20.0));
    store.close().unwrap();
}

#[test]
fn agg_multiple_in_one_query() {
    // count + sum + min + max in a single call.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH count(b) AS n, sum(b.score) AS s, min(b.score) AS lo, max(b.score) AS hi \
             RETURN n, s, lo, hi",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(3));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 35.0).abs() < 1e-9);
    assert_eq!(row.get("lo").and_then(|v| v.as_f64()), Some(5.0));
    assert_eq!(row.get("hi").and_then(|v| v.as_f64()), Some(20.0));
    store.close().unwrap();
}

#[test]
fn agg_with_pre_match_filter() {
    // WHERE b.score >= 10 in MATCH filters before aggregation: bob (10) + dave (20) → 2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' AND b.score >= 10 \
             WITH count(b) AS n, sum(b.score) AS s RETURN n, s",
        )
        .unwrap();
    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 30.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_missing_field_excluded_from_sum_avg() {
    // Nodes without the aggregated field are skipped (not treated as 0).
    // hub → node_a (score=10), node_b (no score field)
    // sum(b.score) = 10, avg = 10.0, count = 2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://h"), b"{}".to_vec());
    tx.put_object(obj("n://a"), br#"{"score":10}"#.to_vec());
    tx.put_object(obj("n://b"), br#"{"label":"no-score"}"#.to_vec());
    tx.put_edge(eid("ea"), obj("hub://h"), obj("n://a"), "LINKS", None);
    tx.put_edge(eid("eb"), obj("hub://h"), obj("n://b"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'hub://h' \
             WITH count(b) AS n, sum(b.score) AS s, avg(b.score) AS a \
             RETURN n, s, a",
        )
        .unwrap();

    let row = &result[0];
    assert_eq!(row.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((row.get("s").and_then(|v| v.as_f64()).unwrap() - 10.0).abs() < 1e-9);
    assert!((row.get("a").and_then(|v| v.as_f64()).unwrap() - 10.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_avg_min_max_null_when_no_valid_field() {
    // avg/min/max return JSON null when no nodes have the requested field.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("hub://h"), b"{}".to_vec());
    tx.put_object(obj("n://x"), br#"{"label":"x"}"#.to_vec());
    tx.put_edge(eid("e1"), obj("hub://h"), obj("n://x"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'hub://h' \
             WITH avg(b.score) AS a, min(b.score) AS lo, max(b.score) AS hi \
             RETURN a, lo, hi",
        )
        .unwrap();

    let row = &result[0];
    assert!(
        row.get("a").map(|v| v.is_null()).unwrap_or(false),
        "avg of missing field is null"
    );
    assert!(
        row.get("lo").map(|v| v.is_null()).unwrap_or(false),
        "min of missing field is null"
    );
    assert!(
        row.get("hi").map(|v| v.is_null()).unwrap_or(false),
        "max of missing field is null"
    );
    store.close().unwrap();
}

#[test]
fn agg_no_aggregating_with_returns_error() {
    // Calling graph_ir_aggregated on a non-aggregating query is an error.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let err = store
        .graph_ir_aggregated("MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' RETURN b")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("aggregat"),
        "error should mention aggregation, got: {msg}"
    );
    store.close().unwrap();
}

// ── GROUP BY aggregation (issue #2) ──────────────────────────────────────────

#[test]
fn agg_grouped_parse_captures_group_by_field() {
    // Parser: WITH count(b) AS cnt GROUP BY b.type → group_by_fields = ["type"].
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH count(b) AS cnt GROUP BY b.type RETURN b.type, cnt",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["type"]);
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(wc.aggregations[0].func, AggFunc::Count);
}

#[test]
fn agg_grouped_parse_multi_agg_with_group_by() {
    // count + sum with GROUP BY in one clause.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[]->(b) WHERE id(a) = 'x' \
         WITH count(b) AS n, sum(b.score) AS s GROUP BY b.category RETURN b.category, n, s",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["category"]);
    assert_eq!(wc.aggregations.len(), 2);
}

#[test]
fn agg_grouped_count_per_category() {
    // count(b) GROUP BY b.type → animal:2, plant:2, other:1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n GROUP BY b.type RETURN b.type, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 3, "three distinct type values");
    let find = |t: &str| {
        rows.iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some(t))
            .expect(t)
    };
    assert_eq!(find("animal").get("n").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(find("plant").get("n").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(find("other").get("n").and_then(|v| v.as_u64()), Some(1));
    store.close().unwrap();
}

#[test]
fn agg_grouped_sum_per_category() {
    // sum(b.score) GROUP BY b.type → animal:6, plant:14, other:1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH sum(b.score) AS s GROUP BY b.type RETURN b.type, s",
        )
        .unwrap();

    let find = |t: &str| {
        rows.iter()
            .find(|r| r.get("type").and_then(|v| v.as_str()) == Some(t))
            .expect(t)
    };
    assert!((find("animal").get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    assert!((find("plant").get("s").and_then(|v| v.as_f64()).unwrap() - 14.0).abs() < 1e-9);
    assert!((find("other").get("s").and_then(|v| v.as_f64()).unwrap() - 1.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_grouped_multi_agg_per_category() {
    // count + sum + avg per group, all in one query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n, sum(b.score) AS s, avg(b.score) AS a GROUP BY b.type \
             RETURN b.type, n, s, a",
        )
        .unwrap();

    let animal = rows
        .iter()
        .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("animal"))
        .unwrap();
    assert_eq!(animal.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((animal.get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    assert!((animal.get("a").and_then(|v| v.as_f64()).unwrap() - 3.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_grouped_null_group_for_missing_field() {
    // Nodes that lack the GROUP BY field go into a null group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("h://root"), b"{}".to_vec());
    tx.put_object(obj("n://a"), br#"{"type":"X","score":1}"#.to_vec());
    tx.put_object(obj("n://b"), br#"{"score":2}"#.to_vec()); // no type field
    tx.put_edge(eid("e1"), obj("h://root"), obj("n://a"), "LINKS", None);
    tx.put_edge(eid("e2"), obj("h://root"), obj("n://b"), "LINKS", None);
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n GROUP BY b.type RETURN b.type, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "X group + null group");
    let null_row = rows
        .iter()
        .find(|r| r.get("type").map(|v| v.is_null()).unwrap_or(false));
    assert!(
        null_row.is_some(),
        "null group exists for nodes without type field"
    );
    assert_eq!(null_row.unwrap().get("n").and_then(|v| v.as_u64()), Some(1));
    store.close().unwrap();
}

#[test]
fn agg_ungrouped_returns_single_row_vec() {
    // Without GROUP BY, result is always a 1-element Vec.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS]->(b) WHERE id(a) = 'h://root' \
             WITH count(b) AS n RETURN n",
        )
        .unwrap();

    assert_eq!(rows.len(), 1, "ungrouped always returns exactly one row");
    assert_eq!(rows[0].get("n").and_then(|v| v.as_u64()), Some(5));
    store.close().unwrap();
}

// ── Multi-key GROUP BY (issue #2) ────────────────────────────────────────────

#[test]
fn agg_multikey_parse_two_fields() {
    // Parser: WITH count(b) AS cnt GROUP BY b.type, b.status → group_by_fields = ["type","status"].
    use bindb::GraphIrQuery;
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
         WITH count(b) AS cnt GROUP BY b.type, b.status RETURN b.type, b.status, cnt",
    )
    .unwrap();
    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.group_by_fields, vec!["type", "status"]);
    assert_eq!(wc.aggregations.len(), 1);
    assert_eq!(wc.aggregations[0].alias, "cnt");
}

#[test]
fn agg_multikey_count_per_type_status() {
    // count(b) GROUP BY b.type, b.status should yield 5 groups.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH count(b) AS n GROUP BY b.type, b.status \
             RETURN b.type, b.status, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 5, "5 distinct (type,status) combinations");

    let find = |t: Option<&str>, s: &str| {
        rows.iter().find(|r| {
            r.get("type").and_then(|v| v.as_str()) == t
                && r.get("status").and_then(|v| v.as_str()) == Some(s)
        })
    };
    assert_eq!(
        find(Some("animal"), "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );
    assert_eq!(
        find(Some("animal"), "retired")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        find(Some("plant"), "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        find(Some("plant"), "retired")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    // rock has no type → null group
    assert_eq!(
        find(None, "active")
            .and_then(|r| r.get("n"))
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    store.close().unwrap();
}

#[test]
fn agg_multikey_sum_per_type_status() {
    // sum(b.score) GROUP BY b.type, b.status.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH sum(b.score) AS s GROUP BY b.type, b.status \
             RETURN b.type, b.status, s",
        )
        .unwrap();

    let get_sum = |t: Option<&str>, st: &str| -> Option<f64> {
        rows.iter()
            .find(|r| {
                r.get("type").and_then(|v| v.as_str()) == t
                    && r.get("status").and_then(|v| v.as_str()) == Some(st)
            })
            .and_then(|r| r.get("s"))
            .and_then(|v| v.as_f64())
    };
    assert!(
        (get_sum(Some("animal"), "active").unwrap() - 6.0).abs() < 1e-9,
        "cat+dog"
    );
    assert!(
        (get_sum(Some("animal"), "retired").unwrap() - 3.0).abs() < 1e-9,
        "wolf"
    );
    assert!(
        (get_sum(Some("plant"), "active").unwrap() - 6.0).abs() < 1e-9,
        "rose"
    );
    assert!(
        (get_sum(Some("plant"), "retired").unwrap() - 8.0).abs() < 1e-9,
        "fern"
    );
    assert!(
        (get_sum(None, "active").unwrap() - 1.0).abs() < 1e-9,
        "rock"
    );
    store.close().unwrap();
}

#[test]
fn agg_multikey_multi_agg_same_query() {
    // count + sum in the same multi-key GROUP BY query.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK]->(b) WHERE id(a) = 'h://mk_root' \
             WITH count(b) AS n, sum(b.score) AS s GROUP BY b.type, b.status \
             RETURN b.type, b.status, n, s",
        )
        .unwrap();

    let animal_active = rows
        .iter()
        .find(|r| {
            r.get("type").and_then(|v| v.as_str()) == Some("animal")
                && r.get("status").and_then(|v| v.as_str()) == Some("active")
        })
        .expect("animal/active group");
    assert_eq!(animal_active.get("n").and_then(|v| v.as_u64()), Some(2));
    assert!((animal_active.get("s").and_then(|v| v.as_f64()).unwrap() - 6.0).abs() < 1e-9);
    store.close().unwrap();
}

#[test]
fn agg_multikey_both_fields_null_group() {
    // Nodes missing both GROUP BY fields form a (null, null) group.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();

    let mut tx = Transaction::new();
    tx.put_object(obj("h://mk2"), b"{}".to_vec());
    tx.put_object(
        obj("n://mk2/a"),
        br#"{"type":"X","status":"Y","score":10}"#.to_vec(),
    );
    tx.put_object(obj("n://mk2/b"), br#"{"score":5}"#.to_vec()); // missing both
    tx.put_edge(eid("ek_a"), obj("h://mk2"), obj("n://mk2/a"), "MK2", None);
    tx.put_edge(eid("ek_b"), obj("h://mk2"), obj("n://mk2/b"), "MK2", None);
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:MK2]->(b) WHERE id(a) = 'h://mk2' \
             WITH count(b) AS n GROUP BY b.type, b.status \
             RETURN b.type, b.status, n",
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "(X,Y) + (null,null)");
    let null_group = rows
        .iter()
        .find(|r| r.get("type").map(|v| v.is_null()).unwrap_or(false))
        .expect("null,null group");
    assert_eq!(null_group.get("n").and_then(|v| v.as_u64()), Some(1));
    assert!(null_group
        .get("status")
        .map(|v| v.is_null())
        .unwrap_or(false));
    store.close().unwrap();
}

// ── STRING_AGG ────────────────────────────────────────────────────────────────

#[test]
fn string_agg_joins_field_values() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("h://sa:root"), b"{}".to_vec());
    tx.put_object(obj("h://sa:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("h://sa:b"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("h://sa:c"), br#"{"name":"carol"}"#.to_vec());
    tx.put_edge(
        eid("sa:e1"),
        obj("h://sa:root"),
        obj("h://sa:a"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sa:e2"),
        obj("h://sa:root"),
        obj("h://sa:b"),
        "KNOWS",
        None,
    );
    tx.put_edge(
        eid("sa:e3"),
        obj("h://sa:root"),
        obj("h://sa:c"),
        "KNOWS",
        None,
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'h://sa:root' \
             WITH string_agg(b.name, ', ') AS names RETURN names",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let names = rows[0].get("names").and_then(|v| v.as_str()).unwrap();
    let mut parts: Vec<&str> = names.split(", ").collect();
    parts.sort();
    assert_eq!(parts, vec!["alice", "bob", "carol"]);
    store.close().unwrap();
}

#[test]
fn string_agg_skips_null_fields() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("h://san:root"), b"{}".to_vec());
    tx.put_object(obj("h://san:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("h://san:b"), b"{}".to_vec()); // no name field
    tx.put_object(obj("h://san:c"), br#"{"name":"carol"}"#.to_vec());
    tx.put_edge(
        eid("san:e1"),
        obj("h://san:root"),
        obj("h://san:a"),
        "L",
        None,
    );
    tx.put_edge(
        eid("san:e2"),
        obj("h://san:root"),
        obj("h://san:b"),
        "L",
        None,
    );
    tx.put_edge(
        eid("san:e3"),
        obj("h://san:root"),
        obj("h://san:c"),
        "L",
        None,
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:L*1..1]->(b) WHERE id(a) = 'h://san:root' \
             WITH string_agg(b.name, '|') AS names RETURN names",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let names = rows[0].get("names").and_then(|v| v.as_str()).unwrap();
    let mut parts: Vec<&str> = names.split('|').collect();
    parts.sort();
    assert_eq!(parts, vec!["alice", "carol"]);
    store.close().unwrap();
}

#[test]
fn string_agg_with_group_by() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINKS*1..1]->(b) WHERE id(a) = 'h://root' \
             WITH string_agg(b.type, ',') AS types GROUP BY b.type \
             RETURN b.type, types",
        )
        .unwrap();
    // Each group's string_agg should just be the type value repeated once per member,
    // but since all members in a group share the same type, count by group size.
    assert_eq!(rows.len(), 3); // animal, plant, other
    for row in &rows {
        let group_type = row.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let types_str = row.get("types").and_then(|v| v.as_str()).unwrap_or("");
        let count = types_str.split(',').count();
        match group_type {
            "animal" => assert_eq!(count, 2),
            "plant" => assert_eq!(count, 2),
            "other" => assert_eq!(count, 1),
            _ => panic!("unexpected group: {group_type}"),
        }
    }
    store.close().unwrap();
}

#[test]
fn string_agg_parse_error_missing_sep() {
    let result = GraphIrQuery::parse(
        "MATCH (a)-[:L]->(b) WHERE id(a) = 'x' \
         WITH string_agg(b.name) AS s RETURN s",
    );
    assert!(result.is_err());
}

#[test]
fn string_agg_parse_error_bare_b() {
    let result = GraphIrQuery::parse(
        "MATCH (a)-[:L]->(b) WHERE id(a) = 'x' \
         WITH string_agg(b, ',') AS s RETURN s",
    );
    assert!(result.is_err());
}
