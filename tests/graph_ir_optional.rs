#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── OPTIONAL MATCH (issue #2) ─────────────────────────────────────────────────

/// Build a graph for OPTIONAL MATCH tests:
///
/// ```text
/// person://alice --KNOWS--> person://bob   --HAS_TAG--> tag://rust
///                                          --HAS_TAG--> tag://databases
///                --KNOWS--> person://carol              (no tags)
///                --KNOWS--> person://dave  --HAS_TAG--> tag://rust
///                                          (dave also has a numeric payload)
/// ```
///
/// Payloads:
/// - person://alice: `{"name":"Alice"}`
/// - person://bob:   `{"name":"Bob","score":10}`
/// - person://carol: `{"name":"Carol","score":5}`
/// - person://dave:  `{"name":"Dave","score":20}`
/// - tag://rust:     `{"label":"rust","weight":0.9}`
/// - tag://databases:`{"label":"databases","weight":0.4}`

#[test]
fn optional_match_no_match_emits_null_row() {
    // carol has no HAS_TAG edges; OPTIONAL MATCH must still emit carol with optional=None.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "one mandatory result (carol), no expansion");
    assert_eq!(rows[0].object_id.0, "person://carol");
    assert!(
        rows[0].optional.is_none(),
        "carol has no tags → optional=None"
    );

    store.close().unwrap();
}

#[test]
fn optional_match_single_c_produces_one_row() {
    // dave has exactly one tag: rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://dave' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "dave has one tag → one expanded row");
    assert_eq!(rows[0].object_id.0, "person://dave");
    let opt = rows[0].optional.as_ref().expect("optional must be Some");
    assert_eq!(opt.object_id.0, "tag://rust");
    assert_eq!(opt.depth, 1);

    store.close().unwrap();
}

#[test]
fn optional_match_multiple_c_expands_rows() {
    // bob has two tags: rust + databases → two output rows for bob
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 2, "bob has two tags → two rows");
    // Both rows have the same b (bob)
    assert!(rows.iter().all(|r| r.object_id.0 == "person://bob"));
    // Both rows have a non-None optional
    assert!(rows.iter().all(|r| r.optional.is_some()));

    let c_ids: Vec<&str> = rows
        .iter()
        .map(|r| r.optional.as_ref().unwrap().object_id.0.as_str())
        .collect();
    assert!(c_ids.contains(&"tag://rust"), "expected rust tag");
    assert!(c_ids.contains(&"tag://databases"), "expected databases tag");

    store.close().unwrap();
}

#[test]
fn optional_match_mixed_mandatory_results() {
    // alice KNOWS: bob (2 tags), carol (0 tags), dave (1 tag)
    // → total rows: 2 + 1 + 1 = 4 (carol emits one null-padded row)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    let bob_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://bob")
        .collect();
    let carol_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://carol")
        .collect();
    let dave_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.object_id.0 == "person://dave")
        .collect();

    assert_eq!(bob_rows.len(), 2, "bob has 2 tags");
    assert_eq!(carol_rows.len(), 1, "carol has 0 tags → one null row");
    assert_eq!(dave_rows.len(), 1, "dave has 1 tag");

    assert!(
        carol_rows[0].optional.is_none(),
        "carol optional must be None"
    );
    assert!(bob_rows.iter().all(|r| r.optional.is_some()));
    assert!(dave_rows[0].optional.is_some());

    store.close().unwrap();
}

#[test]
fn optional_match_c_payload_predicate_filters() {
    // bob has rust (weight=0.9) and databases (weight=0.4)
    // WHERE c.weight > 0.5 should keep only rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE c.weight > 0.5 \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // Only rust matches c.weight > 0.5; databases doesn't.
    // But bob IS matched by mandatory → bob is always emitted.
    // Since rust matches optional: one row with optional=Some(rust).
    // databases doesn't match optional: that sub-result is filtered out.
    // Result: one row (bob + rust) — databases sub-match excluded.
    assert_eq!(rows.len(), 1, "only rust passes weight>0.5 filter");
    assert_eq!(rows[0].object_id.0, "person://bob");
    let opt = rows[0].optional.as_ref().expect("rust must match");
    assert_eq!(opt.object_id.0, "tag://rust");

    store.close().unwrap();
}

#[test]
fn optional_match_c_payload_pred_excludes_all_leaves_null_row() {
    // carol has no tags, so OPTIONAL MATCH with a predicate that would never match
    // should still emit carol with optional=None
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE c.weight > 0.99 \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].object_id.0, "person://carol");
    assert!(rows[0].optional.is_none());

    store.close().unwrap();
}

#[test]
fn optional_match_field_projection_b_and_c() {
    // Project b.name and c.label
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://dave' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Dave")
    );
    let opt = rows[0].optional.as_ref().expect("dave has rust tag");
    assert_eq!(
        opt.fields.get("label").and_then(|v| v.as_str()),
        Some("rust")
    );

    store.close().unwrap();
}

#[test]
fn optional_match_c_field_projection_absent_for_null_row() {
    // carol has no tags → optional=None → c fields inaccessible, no panic
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://carol' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Carol")
    );
    assert!(
        rows[0].optional.is_none(),
        "no tags → null optional, no field panic"
    );

    store.close().unwrap();
}

#[test]
fn optional_match_c_id_predicate_filters_by_prefix() {
    // bob has rust + databases; WHERE id(c) STARTS WITH 'tag://r' should keep only rust
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://r' \
         RETURN b, c",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    assert_eq!(rows.len(), 1, "only rust starts with tag://r");
    assert_eq!(rows[0].optional.as_ref().unwrap().object_id.0, "tag://rust");

    store.close().unwrap();
}

#[test]
fn optional_match_c_id_and_payload_pred_combined() {
    // WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' AND id(b) = 'person://bob' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5 \
         RETURN b.name, c.label",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // rust passes both; databases fails weight>0.5; one row with rust
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]
            .optional
            .as_ref()
            .unwrap()
            .fields
            .get("label")
            .and_then(|v| v.as_str()),
        Some("rust")
    );

    store.close().unwrap();
}

#[test]
fn optional_match_with_order_by_and_limit_on_mandatory() {
    // ORDER BY b.score DESC LIMIT 2 → dave(20), bob(10); then expand with optional
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         RETURN b.name, c.label \
         ORDER BY b.score DESC LIMIT 2",
    )
    .unwrap();

    let rows = store.graph_ir_projected(&q).unwrap();

    // LIMIT 2 mandatory results: dave(score=20), bob(score=10)
    // dave has 1 tag → 1 row; bob has 2 tags → 2 rows; total 3 rows
    assert_eq!(rows.len(), 3, "dave×1 + bob×2");

    // First row should be dave (highest score, first after ORDER BY DESC)
    // Dave has one tag (rust)
    let dave_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.fields.get("name").and_then(|v| v.as_str()) == Some("Dave"))
        .collect();
    let bob_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.fields.get("name").and_then(|v| v.as_str()) == Some("Bob"))
        .collect();

    assert_eq!(dave_rows.len(), 1);
    assert_eq!(bob_rows.len(), 2);

    store.close().unwrap();
}

#[test]
fn optional_match_parse_captures_clause() {
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..2]->(b) \
         WHERE id(a) = 'person://alice' \
         OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
         WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5 \
         RETURN b.name, c.label",
    )
    .unwrap();

    let opt = q
        .optional_clause
        .as_ref()
        .expect("optional clause must be parsed");
    assert_eq!(opt.edge_types, vec!["HAS_TAG"]);
    assert_eq!(opt.min_depth, 1);
    assert_eq!(opt.max_depth, 1);
    assert!(matches!(opt.end_pred, bindb::NodeIdPred::StartsWith(ref s) if s == "tag://"));
    assert_eq!(opt.payload_preds.len(), 1);
    assert_eq!(opt.payload_preds[0].field, "weight");
    assert_eq!(opt.return_fields, vec!["label"]);
    assert_eq!(q.return_fields, vec!["name"]);
}

#[test]
fn optional_match_query_without_optional_clause_unchanged() {
    // Existing non-optional queries must still work identically — no regression
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let q =
        GraphIrQuery::parse("MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' RETURN b")
            .unwrap();
    assert!(q.optional_clause.is_none());

    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3, "alice KNOWS 3 people");
    assert!(
        rows.iter().all(|r| r.optional.is_none()),
        "no optional clause → all None"
    );

    store.close().unwrap();
}

// ── WITH clause tests (issue #2) ──────────────────────────────────────────────
//
// Graph: alice -[KNOWS]-> bob (score:10), carol (score:5), dave (score:20)
//        bob -[HAS_TAG]-> tag://rust (weight:0.9), tag://databases (weight:0.4)
//        dave -[HAS_TAG]-> tag://rust
//        carol has no HAS_TAG edges

#[test]
fn with_no_where_passes_all_mandatory_through() {
    // WITH b with no predicate or ordering is a no-op; all mandatory results pass through.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3, "all three KNOWS targets pass through");
    store.close().unwrap();
}

#[test]
fn with_where_payload_pred_filters_mandatory() {
    // WITH b WHERE b.score > 8 keeps only bob (10) and dave (20); carol (5) is excluded.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE b.score > 8 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(names.contains(&"Bob"), "bob (score:10) must pass");
    assert!(names.contains(&"Dave"), "dave (score:20) must pass");
    assert!(
        !names.contains(&"Carol"),
        "carol (score:5) must be excluded"
    );
    store.close().unwrap();
}

#[test]
fn with_where_id_pred_filters_by_exact_id() {
    // WITH b WHERE id(b) = 'person://dave' keeps only dave.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE id(b) = 'person://dave' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Dave")
    );
    store.close().unwrap();
}

#[test]
fn with_order_by_desc_sorts_mandatory_results() {
    // WITH b ORDER BY b.score DESC sorts highest-score first (dave=20, bob=10, carol=5).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3);
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert_eq!(names[0], "Dave", "highest score first");
    assert_eq!(names[1], "Bob");
    store.close().unwrap();
}

#[test]
fn with_limit_caps_mandatory_set() {
    // WITH b LIMIT 2 emits at most 2 rows regardless of match count.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b LIMIT 2 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "WITH LIMIT 2 caps at 2");
    store.close().unwrap();
}

#[test]
fn with_skip_offsets_mandatory_results() {
    // WITH b ORDER BY b.score ASC SKIP 1 skips carol (lowest score=5).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score ASC SKIP 1 RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "skip 1 of 3 → 2 remaining");
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(!names.contains(&"Carol"), "carol (lowest) was skipped");
    store.close().unwrap();
}

#[test]
fn with_order_limit_then_optional_expands_only_top_n() {
    // WITH b ORDER BY b.score DESC LIMIT 2 → top-2 are dave (20) and bob (10).
    // OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c):
    //   dave has 1 tag (rust), bob has 2 tags (rust, databases) → 3 expanded rows.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC LIMIT 2 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label",
            )
            .unwrap(),
        )
        .unwrap();

    let b_names: Vec<&str> = rows
        .iter()
        .map(|r| r.fields.get("name").and_then(|v| v.as_str()).unwrap_or(""))
        .collect();
    assert!(
        !b_names.contains(&"Carol"),
        "carol was excluded by WITH LIMIT 2"
    );
    // dave has 1 tag, bob has 2 → 3 rows total
    assert_eq!(rows.len(), 3, "dave(1 tag) + bob(2 tags) = 3 rows");
    store.close().unwrap();
}

#[test]
fn with_where_combined_with_optional_and_c_pred() {
    // WITH b WHERE b.score >= 10: keeps bob (10) and dave (20).
    // OPTIONAL MATCH WHERE c.weight > 0.5: keeps tag://rust (0.9), excludes databases (0.4).
    // bob has rust → 1 row; dave has rust → 1 row.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b WHERE b.score >= 10 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) WHERE c.weight > 0.5 \
                 RETURN b.name, c.label",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "bob→rust + dave→rust; databases excluded");
    for row in &rows {
        assert_eq!(
            row.optional
                .as_ref()
                .and_then(|o| o.fields.get("label"))
                .and_then(|v| v.as_str()),
            Some("rust"),
            "only the rust tag passes the weight > 0.5 predicate"
        );
    }
    store.close().unwrap();
}

#[test]
fn with_return_order_by_sorts_post_expansion() {
    // WITH b ORDER BY b.score DESC LIMIT 2 → dave, bob (pre-expansion)
    // OPTIONAL MATCH → dave→rust, bob→rust, bob→databases (3 expanded rows)
    // RETURN ORDER BY id(b) ASC → sort by b.object_id lexicographically:
    //   person://bob < person://dave, so bob's 2 rows come first.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 WITH b ORDER BY b.score DESC LIMIT 2 \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label ORDER BY id(b) ASC",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(rows.len(), 3);
    // First two rows must be bob (id "person://bob" < "person://dave").
    assert_eq!(rows[0].object_id.0, "person://bob");
    assert_eq!(rows[1].object_id.0, "person://bob");
    assert_eq!(rows[2].object_id.0, "person://dave");
    store.close().unwrap();
}

#[test]
fn with_parse_captures_clause_fields() {
    // Parser test: with_clause populated with WHERE pred, ORDER BY DESC, SKIP, LIMIT.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE b.score > 5 ORDER BY b.score DESC SKIP 1 LIMIT 3 \
         RETURN b.name",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().expect("with_clause must be Some");
    assert_eq!(wc.payload_preds.len(), 1);
    assert_eq!(wc.payload_preds[0].field, "score");
    assert_eq!(wc.payload_preds[0].op, CmpOp::Gt);
    assert_eq!(wc.payload_preds[0].value, CmpValue::Number(5.0));
    assert_eq!(wc.order_by.len(), 1);
    assert_eq!(wc.order_by[0].direction, OrderDirection::Desc);
    assert_eq!(wc.skip, Some(1));
    assert_eq!(wc.limit, Some(3));
    assert!(q.optional_clause.is_none());
}

#[test]
fn with_parse_id_pred_in_where() {
    // WITH b WHERE id(b) STARTS WITH 'person://d' populates wc.end_pred.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
         WITH b WHERE id(b) STARTS WITH 'person://d' RETURN b",
    )
    .unwrap();

    let wc = q.with_clause.as_ref().unwrap();
    assert_eq!(wc.end_pred, NodeIdPred::StartsWith("person://d".into()));
    assert!(wc.payload_preds.is_empty());
    assert!(wc.order_by.is_empty());
}

#[test]
fn without_with_optional_match_order_by_still_pre_expansion() {
    // Backward compat: when no WITH clause, ORDER BY/LIMIT applies to mandatory
    // results before optional expansion.  LIMIT 1 with ORDER BY b.score DESC
    // keeps only dave (score=20); OPTIONAL MATCH gives dave→rust (1 row).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);

    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
                 OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c) \
                 RETURN b.name, c.label ORDER BY b.score DESC LIMIT 1",
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(
        rows.len(),
        1,
        "LIMIT 1 pre-expansion → 1 mandatory → 1 tag row"
    );
    assert_eq!(rows[0].object_id.0, "person://dave");
    store.close().unwrap();
}
