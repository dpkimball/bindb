#[path = "graph_ir_helpers.rs"]
mod helpers;
use helpers::*;

// ── COLLECT aggregation ──────────────────────────────────────────────────────

#[test]
fn collect_field_values() {
    // collect(b.name) returns a JSON array of all name values.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://col:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://col:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("obs://col:b"), br#"{"name":"bob"}"#.to_vec());
    tx.put_object(obj("obs://col:c"), br#"{"name":"carol"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://col:1"),
        obj("obs://col:hub"),
        obj("obs://col:a"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e://col:2"),
        obj("obs://col:hub"),
        obj("obs://col:b"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e://col:3"),
        obj("obs://col:hub"),
        obj("obs://col:c"),
        "KNOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'obs://col:hub' \
             WITH collect(b.name) AS names RETURN names",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let arr = rows[0].get("names").and_then(|v| v.as_array()).unwrap();
    let mut names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["alice", "bob", "carol"]);
    store.close().unwrap();
}

#[test]
fn collect_node_ids() {
    // collect(b) returns a JSON array of object_id strings.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cid:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cid:x"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cid:y"), br#"{}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://cid:1"),
        obj("obs://cid:hub"),
        obj("obs://cid:x"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("e://cid:2"),
        obj("obs://cid:hub"),
        obj("obs://cid:y"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cid:hub' \
             WITH collect(b) AS ids RETURN ids",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let arr = rows[0].get("ids").and_then(|v| v.as_array()).unwrap();
    let mut ids: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    ids.sort();
    assert_eq!(ids, vec!["obs://cid:x", "obs://cid:y"]);
    store.close().unwrap();
}

#[test]
fn collect_grouped_by_field() {
    // collect(b.name) GROUP BY b.role — each role gets its own list of names.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cgf:hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("obs://cgf:a"),
        br#"{"role":"admin","name":"alice"}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://cgf:b"),
        br#"{"role":"admin","name":"bob"}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://cgf:c"),
        br#"{"role":"user","name":"carol"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://cgf:1"),
        obj("obs://cgf:hub"),
        obj("obs://cgf:a"),
        "HAS",
        None,
    );
    tx2.put_edge(
        eid("e://cgf:2"),
        obj("obs://cgf:hub"),
        obj("obs://cgf:b"),
        "HAS",
        None,
    );
    tx2.put_edge(
        eid("e://cgf:3"),
        obj("obs://cgf:hub"),
        obj("obs://cgf:c"),
        "HAS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let mut rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:HAS*1..1]->(b) WHERE id(a) = 'obs://cgf:hub' \
             WITH collect(b.name) AS members GROUP BY b.role RETURN members",
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);

    // admin group has alice and bob
    let admin_arr = rows[0].get("members").and_then(|v| v.as_array()).unwrap();
    let mut admin_names: Vec<&str> = admin_arr.iter().filter_map(|v| v.as_str()).collect();
    admin_names.sort();
    assert_eq!(admin_names, vec!["alice", "bob"]);

    // user group has only carol
    let user_arr = rows[1].get("members").and_then(|v| v.as_array()).unwrap();
    let user_names: Vec<&str> = user_arr.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(user_names, vec!["carol"]);
    store.close().unwrap();
}

#[test]
fn collect_combined_with_count() {
    // collect(b.name) and count(b) in the same WITH clause.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://ccc:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://ccc:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("obs://ccc:b"), br#"{"name":"bob"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://ccc:1"),
        obj("obs://ccc:hub"),
        obj("obs://ccc:a"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e://ccc:2"),
        obj("obs://ccc:hub"),
        obj("obs://ccc:b"),
        "KNOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'obs://ccc:hub' \
             WITH count(b) AS cnt, collect(b.name) AS names RETURN cnt, names",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("cnt").and_then(|v| v.as_i64()), Some(2));
    let arr = rows[0].get("names").and_then(|v| v.as_array()).unwrap();
    assert_eq!(arr.len(), 2);
    store.close().unwrap();
}

#[test]
fn collect_empty_result() {
    // collect on a MATCH that finds nothing returns an empty array.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cer:hub"), br#"{}"#.to_vec());
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'obs://cer:hub' \
             WITH collect(b.name) AS names RETURN names",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    let arr = rows[0].get("names").and_then(|v| v.as_array()).unwrap();
    assert!(arr.is_empty());
    store.close().unwrap();
}

#[test]
fn collect_missing_field_gives_null_entries() {
    // Nodes that lack the collected field contribute JSON null to the array.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cmf:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cmf:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("obs://cmf:b"), br#"{}"#.to_vec()); // no name field
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://cmf:1"),
        obj("obs://cmf:hub"),
        obj("obs://cmf:a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("e://cmf:2"),
        obj("obs://cmf:hub"),
        obj("obs://cmf:b"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cmf:hub' \
             WITH collect(b.name) AS names RETURN names",
        )
        .unwrap();
    let arr = rows[0].get("names").and_then(|v| v.as_array()).unwrap();
    assert_eq!(arr.len(), 2);
    let nulls = arr.iter().filter(|v| v.is_null()).count();
    let strings = arr.iter().filter(|v| v.is_string()).count();
    assert_eq!(nulls, 1);
    assert_eq!(strings, 1);
    store.close().unwrap();
}

#[test]
fn case_when_basic() {
    // CASE b.role WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cwb:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cwb:a"), br#"{"role":"admin"}"#.to_vec());
    tx.put_object(obj("obs://cwb:b"), br#"{"role":"user"}"#.to_vec());
    tx.put_object(obj("obs://cwb:c"), br#"{"role":"guest"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b", "c"] {
        tx2.put_edge(
            eid(&format!("e://cwb:{k}")),
            obj("obs://cwb:hub"),
            obj(&format!("obs://cwb:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cwb:hub' \
         RETURN CASE b.role WHEN 'admin' THEN 'A' WHEN 'user' THEN 'U' ELSE 'X' END AS tier",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3);
    let tiers: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| {
            r.fields
                .get("tier")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(tiers.contains("A"), "missing A: {tiers:?}");
    assert!(tiers.contains("U"), "missing U: {tiers:?}");
    assert!(tiers.contains("X"), "missing X: {tiers:?}");
    store.close().unwrap();
}

#[test]
fn case_when_no_else_uses_empty_string() {
    // Without ELSE, unmatched nodes get an empty string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cwne:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cwne:a"), br#"{"status":"active"}"#.to_vec());
    tx.put_object(obj("obs://cwne:b"), br#"{"status":"inactive"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b"] {
        tx2.put_edge(
            eid(&format!("e://cwne:{k}")),
            obj("obs://cwne:hub"),
            obj(&format!("obs://cwne:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cwne:hub' \
         RETURN CASE b.status WHEN 'active' THEN 'yes' END AS flag",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    let yes_count = rows
        .iter()
        .filter(|r| r.fields.get("flag").and_then(|v| v.as_str()) == Some("yes"))
        .count();
    assert_eq!(yes_count, 1);
    store.close().unwrap();
}

#[test]
fn case_when_combined_with_other_fields() {
    // CASE alongside regular field projection.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cwcf:hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("obs://cwcf:a"),
        br#"{"name":"alice","role":"admin"}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://cwcf:b"),
        br#"{"name":"bob","role":"user"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b"] {
        tx2.put_edge(
            eid(&format!("e://cwcf:{k}")),
            obj("obs://cwcf:hub"),
            obj(&format!("obs://cwcf:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cwcf:hub' \
         RETURN b.name, CASE b.role WHEN 'admin' THEN 'elevated' ELSE 'standard' END AS access",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    for r in &rows {
        let name = r.fields.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let access = r
            .fields
            .get("access")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name == "alice" {
            assert_eq!(access, "elevated");
        } else if name == "bob" {
            assert_eq!(access, "standard");
        }
    }
    store.close().unwrap();
}

#[test]
fn case_when_numeric_field_match() {
    // CASE on a numeric field; match values are compared as their JSON string representation.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cwnm:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://cwnm:a"), br#"{"score":1}"#.to_vec());
    tx.put_object(obj("obs://cwnm:b"), br#"{"score":2}"#.to_vec());
    tx.put_object(obj("obs://cwnm:c"), br#"{"score":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b", "c"] {
        tx2.put_edge(
            eid(&format!("e://cwnm:{k}")),
            obj("obs://cwnm:hub"),
            obj(&format!("obs://cwnm:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cwnm:hub' \
         RETURN CASE b.score WHEN '1' THEN 'low' WHEN '2' THEN 'mid' ELSE 'high' END AS band",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3);
    let mut bands: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.fields
                .get("band")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    bands.sort();
    assert_eq!(bands, vec!["high", "low", "mid"]);
    store.close().unwrap();
}

// ── Feature 1: Zero-hop paths [*0..n] ────────────────────────────────────────

#[test]
fn zero_hop_includes_start_node() {
    // [*0..1] should return both the start node (depth=0) and its immediate neighbours.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://zh:hub"), br#"{"name":"hub"}"#.to_vec());
    tx.put_object(obj("obs://zh:n1"), br#"{"name":"n1"}"#.to_vec());
    tx.put_object(obj("obs://zh:n2"), br#"{"name":"n2"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://zh:1"),
        obj("obs://zh:hub"),
        obj("obs://zh:n1"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("e://zh:2"),
        obj("obs://zh:hub"),
        obj("obs://zh:n2"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*0..1]->(b) WHERE id(a) = 'obs://zh:hub' RETURN b.name",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();

    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    // start node (hub) at depth 0 + 2 neighbours
    assert_eq!(
        rows.len(),
        3,
        "expected 3 rows (start + 2 neighbours), got {:?}",
        names
    );
    assert!(names.contains(&"hub"), "start node should be included");
    assert!(names.contains(&"n1"));
    assert!(names.contains(&"n2"));
    // depth-0 row must be present
    assert!(rows.iter().any(|r| r.depth == 0));
    store.close().unwrap();
}

#[test]
fn zero_hop_only() {
    // [*0..0] returns just the start node at depth 0.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://zho:hub"), br#"{"name":"hub"}"#.to_vec());
    tx.put_object(obj("obs://zho:n1"), br#"{"name":"n1"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://zho:1"),
        obj("obs://zho:hub"),
        obj("obs://zho:n1"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    // [*0..0] is only valid for the range case — exact *0 still errors per spec.
    // We test [*0..0] here which means max=0 → should fail parse, but [*0..1] with
    // depth filter. Actually spec says keep max==0 as error. So we use a query that only
    // returns depth-0 by using a predicate that nothing at depth>0 passes.
    // Instead test parse of [*0..1] and verify depth-0 row.
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*0..1]->(b) WHERE id(a) = 'obs://zho:hub' \
         AND id(b) = 'obs://zho:hub' RETURN b.name",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    // Only the start node matches id(b) = 'obs://zho:hub'
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].depth, 0);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("hub")
    );
    store.close().unwrap();
}

#[test]
fn zero_hop_with_payload_pred() {
    // AND b.role = 'admin' should filter zero-hop result correctly.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("obs://zhp:hub"),
        br#"{"role":"admin","name":"hub"}"#.to_vec(),
    );
    tx.put_object(
        obj("obs://zhp:n1"),
        br#"{"role":"user","name":"n1"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://zhp:1"),
        obj("obs://zhp:hub"),
        obj("obs://zhp:n1"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    // Predicate b.role = 'admin': only hub passes
    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*0..1]->(b) WHERE id(a) = 'obs://zhp:hub' \
         AND b.role = 'admin' RETURN b.name",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1, "only the admin node should pass");
    assert_eq!(rows[0].depth, 0);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("hub")
    );
    store.close().unwrap();
}

#[test]
fn zero_hop_with_return_field() {
    // RETURN b.name works on the zero-hop start node.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("obs://zhrf:hub"),
        br#"{"name":"start","val":42}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[*0..1]->(b) WHERE id(a) = 'obs://zhrf:hub' RETURN b.name, b.val",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    // No outgoing edges from hub, but depth-0 row should still appear
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].depth, 0);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("start")
    );
    assert_eq!(
        rows[0].fields.get("val").and_then(|v| v.as_f64()),
        Some(42.0)
    );
    store.close().unwrap();
}

// ── Feature 2: Arithmetic in RETURN ──────────────────────────────────────────

#[test]
fn return_add_two_fields() {
    // RETURN b.x + b.y AS total should sum two numeric fields.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://arith:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://arith:a"), br#"{"x":3,"y":7}"#.to_vec());
    tx.put_object(obj("obs://arith:b"), br#"{"x":10,"y":20}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://arith:a"),
        obj("obs://arith:hub"),
        obj("obs://arith:a"),
        "LINK",
        None,
    );
    tx2.put_edge(
        eid("e://arith:b"),
        obj("obs://arith:hub"),
        obj("obs://arith:b"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://arith:hub' \
         RETURN b.x + b.y AS total",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    let mut totals: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.fields.get("total").and_then(|v| v.as_f64()))
        .collect();
    totals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(totals, vec![10.0, 30.0]);
    store.close().unwrap();
}

#[test]
fn return_mul_field_by_literal() {
    // RETURN b.score * 2 AS doubled should double the score field.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://mul:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://mul:a"), br#"{"score":5}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://mul:a"),
        obj("obs://mul:hub"),
        obj("obs://mul:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://mul:hub' \
         RETURN b.score * 2 AS doubled",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("doubled").and_then(|v| v.as_f64()),
        Some(10.0)
    );
    store.close().unwrap();
}

#[test]
fn return_sub_fields() {
    // RETURN b.high - b.low AS range.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://sub:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://sub:a"), br#"{"high":100,"low":40}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://sub:a"),
        obj("obs://sub:hub"),
        obj("obs://sub:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://sub:hub' \
         RETURN b.high - b.low AS range",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("range").and_then(|v| v.as_f64()),
        Some(60.0)
    );
    store.close().unwrap();
}

#[test]
fn return_div_with_zero_denominator() {
    // Division by zero should produce null, not panic.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://divz:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://divz:a"), br#"{"x":10,"zero":0}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://divz:a"),
        obj("obs://divz:hub"),
        obj("obs://divz:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://divz:hub' \
         RETURN b.x / b.zero AS r",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    // Division by zero → null
    let val = rows[0].fields.get("r");
    assert!(
        val.is_none_or(|v| v.is_null()),
        "expected null for division by zero, got {:?}",
        val
    );
    store.close().unwrap();
}

// ── Feature 3: String concatenation in RETURN ─────────────────────────────────

#[test]
fn return_concat_two_fields() {
    // RETURN b.first || ' ' || b.last AS full should concatenate with a space separator.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://cat:hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("obs://cat:a"),
        br#"{"first":"Alice","last":"Smith"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://cat:a"),
        obj("obs://cat:hub"),
        obj("obs://cat:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://cat:hub' \
         RETURN b.first || ' ' || b.last AS full",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("full").and_then(|v| v.as_str()),
        Some("Alice Smith")
    );
    store.close().unwrap();
}

#[test]
fn return_concat_field_and_literal() {
    // RETURN b.city || ', ' || b.country AS place
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://catfl:hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("obs://catfl:a"),
        br#"{"city":"Paris","country":"France"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://catfl:a"),
        obj("obs://catfl:hub"),
        obj("obs://catfl:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://catfl:hub' \
         RETURN b.city || ', ' || b.country AS place",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("place").and_then(|v| v.as_str()),
        Some("Paris, France")
    );
    store.close().unwrap();
}

#[test]
fn return_concat_missing_field() {
    // Missing field should contribute empty string, not cause an error.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://catmf:hub"), br#"{}"#.to_vec());
    // 'last' field is absent
    tx.put_object(obj("obs://catmf:a"), br#"{"first":"Bob"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://catmf:a"),
        obj("obs://catmf:hub"),
        obj("obs://catmf:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://catmf:hub' \
         RETURN b.first || ' ' || b.last AS full",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    // Missing 'last' → empty string contribution
    assert_eq!(
        rows[0].fields.get("full").and_then(|v| v.as_str()),
        Some("Bob ")
    );
    store.close().unwrap();
}

#[test]
fn return_concat_single_field() {
    // b.name || '' AS name is a trivial concat (just the field value).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://catsf:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://catsf:a"), br#"{"name":"Charlie"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://catsf:a"),
        obj("obs://catsf:hub"),
        obj("obs://catsf:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://catsf:hub' \
         RETURN b.name || '' AS name",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("name").and_then(|v| v.as_str()),
        Some("Charlie")
    );
    store.close().unwrap();
}

// ── Feature 4: Searched CASE WHEN in RETURN ──────────────────────────────────

#[test]
fn searched_case_gt_threshold() {
    // CASE WHEN b.score > 5 THEN 'high' ELSE 'low' END AS tier
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://scgt:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://scgt:a"), br#"{"score":8}"#.to_vec());
    tx.put_object(obj("obs://scgt:b"), br#"{"score":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b"] {
        tx2.put_edge(
            eid(&format!("e://scgt:{k}")),
            obj("obs://scgt:hub"),
            obj(&format!("obs://scgt:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://scgt:hub' \
         RETURN CASE WHEN b.score > 5 THEN 'high' ELSE 'low' END AS tier",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    let mut tiers: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("tier").and_then(|v| v.as_str()))
        .collect();
    tiers.sort();
    assert_eq!(tiers, vec!["high", "low"]);
    store.close().unwrap();
}

#[test]
fn searched_case_multiple_branches() {
    // Three WHEN branches: score > 8 → 'A', score > 4 → 'B', else 'C'
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://scmb:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://scmb:a"), br#"{"score":9}"#.to_vec());
    tx.put_object(obj("obs://scmb:b"), br#"{"score":6}"#.to_vec());
    tx.put_object(obj("obs://scmb:c"), br#"{"score":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b", "c"] {
        tx2.put_edge(
            eid(&format!("e://scmb:{k}")),
            obj("obs://scmb:hub"),
            obj(&format!("obs://scmb:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://scmb:hub' \
         RETURN CASE WHEN b.score > 8 THEN 'A' WHEN b.score > 4 THEN 'B' ELSE 'C' END AS grade",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3);
    let mut grades: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("grade").and_then(|v| v.as_str()))
        .collect();
    grades.sort();
    assert_eq!(grades, vec!["A", "B", "C"]);
    store.close().unwrap();
}

#[test]
fn searched_case_string_pred() {
    // CASE WHEN b.status = 'active' THEN 'yes' ELSE 'no' END AS flag
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://scsp:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://scsp:a"), br#"{"status":"active"}"#.to_vec());
    tx.put_object(obj("obs://scsp:b"), br#"{"status":"inactive"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b"] {
        tx2.put_edge(
            eid(&format!("e://scsp:{k}")),
            obj("obs://scsp:hub"),
            obj(&format!("obs://scsp:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://scsp:hub' \
         RETURN CASE WHEN b.status = 'active' THEN 'yes' ELSE 'no' END AS flag",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    let yes_count = rows
        .iter()
        .filter(|r| r.fields.get("flag").and_then(|v| v.as_str()) == Some("yes"))
        .count();
    let no_count = rows
        .iter()
        .filter(|r| r.fields.get("flag").and_then(|v| v.as_str()) == Some("no"))
        .count();
    assert_eq!(yes_count, 1);
    assert_eq!(no_count, 1);
    store.close().unwrap();
}

#[test]
fn searched_case_no_else() {
    // When no ELSE clause and no branch matches, result should be empty string.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://scne:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://scne:a"), br#"{"score":10}"#.to_vec());
    tx.put_object(obj("obs://scne:b"), br#"{"score":3}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    for k in ["a", "b"] {
        tx2.put_edge(
            eid(&format!("e://scne:{k}")),
            obj("obs://scne:hub"),
            obj(&format!("obs://scne:{k}")),
            "LINK",
            None,
        );
    }
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://scne:hub' \
         RETURN CASE WHEN b.score > 8 THEN 'high' END AS tier",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 2);
    // score=10 matches → 'high'; score=3 does not → empty string
    let high_count = rows
        .iter()
        .filter(|r| r.fields.get("tier").and_then(|v| v.as_str()) == Some("high"))
        .count();
    let empty_count = rows
        .iter()
        .filter(|r| r.fields.get("tier").and_then(|v| v.as_str()) == Some(""))
        .count();
    assert_eq!(high_count, 1, "expected one 'high' result");
    assert_eq!(
        empty_count, 1,
        "expected one empty-string result for no match"
    );
    store.close().unwrap();
}
// ── RETURN type(r) ──────────────────────────────────────────────────────────

#[test]
fn return_type_r_basic() {
    // type(r) returns the edge relationship label.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rtr:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rtr:a"), br#"{"name":"alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rtr:a"),
        obj("obs://rtr:hub"),
        obj("obs://rtr:a"),
        "KNOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'obs://rtr:hub' RETURN type(r)",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].edge_fields.get("type(r)").and_then(|v| v.as_str()),
        Some("KNOWS")
    );
    store.close().unwrap();
}

#[test]
fn return_type_r_multi_type_edges() {
    // With [:T1|T2] the type(r) field reflects the actual type of each matched edge.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rtm:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rtm:a"), br#"{"name":"alice"}"#.to_vec());
    tx.put_object(obj("obs://rtm:b"), br#"{"name":"bob"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rtm:a"),
        obj("obs://rtm:hub"),
        obj("obs://rtm:a"),
        "KNOWS",
        None,
    );
    tx2.put_edge(
        eid("e://rtm:b"),
        obj("obs://rtm:hub"),
        obj("obs://rtm:b"),
        "FOLLOWS",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:KNOWS|FOLLOWS*1..1]->(b) WHERE id(a) = 'obs://rtm:hub' \
         RETURN b.name, type(r)",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by_key(|r| {
        r.fields
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].edge_fields.get("type(r)").and_then(|v| v.as_str()),
        Some("KNOWS")
    );
    assert_eq!(
        rows[1].edge_fields.get("type(r)").and_then(|v| v.as_str()),
        Some("FOLLOWS")
    );
    store.close().unwrap();
}

#[test]
fn return_type_r_combined_with_fields() {
    // type(r) alongside b.field projections.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rtc:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rtc:a"), br#"{"score":42}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rtc:a"),
        obj("obs://rtc:hub"),
        obj("obs://rtc:a"),
        "RATED",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:RATED*1..1]->(b) WHERE id(a) = 'obs://rtc:hub' \
         RETURN b.score, type(r)",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("score").and_then(|v| v.as_i64()),
        Some(42)
    );
    assert_eq!(
        rows[0].edge_fields.get("type(r)").and_then(|v| v.as_str()),
        Some("RATED")
    );
    store.close().unwrap();
}

#[test]
fn return_type_r_no_type_on_edge_empty_string() {
    // An edge committed without a relationship type yields an empty string for type(r).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rtne:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rtne:a"), br#"{"v":1}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    // put_edge with empty string label
    tx2.put_edge(
        eid("e://rtne:a"),
        obj("obs://rtne:hub"),
        obj("obs://rtne:a"),
        "",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q =
        GraphIrQuery::parse("MATCH (a)-[*1..1]->(b) WHERE id(a) = 'obs://rtne:hub' RETURN type(r)")
            .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    let label = rows[0]
        .edge_fields
        .get("type(r)")
        .and_then(|v| v.as_str())
        .unwrap_or("__missing__");
    // Empty string or the stored empty type is acceptable.
    assert!(label.is_empty() || label == "__missing__");
    store.close().unwrap();
}

// ── RETURN depth ────────────────────────────────────────────────────────────

#[test]
fn return_depth_single_hop() {
    // RETURN depth for 1-hop traversal should be 1.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rds:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rds:a"), br#"{"name":"alice"}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rds:a"),
        obj("obs://rds:hub"),
        obj("obs://rds:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://rds:hub' RETURN b.name, depth",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("depth").and_then(|v| v.as_u64()),
        Some(1)
    );
    store.close().unwrap();
}

#[test]
fn return_depth_multi_hop_values() {
    // depth values distinguish 1-hop vs 2-hop matches.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rdm:root"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rdm:mid"), br#"{"level":1}"#.to_vec());
    tx.put_object(obj("obs://rdm:leaf"), br#"{"level":2}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rdm:1"),
        obj("obs://rdm:root"),
        obj("obs://rdm:mid"),
        "NEXT",
        None,
    );
    tx2.put_edge(
        eid("e://rdm:2"),
        obj("obs://rdm:mid"),
        obj("obs://rdm:leaf"),
        "NEXT",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:NEXT*1..2]->(b) WHERE id(a) = 'obs://rdm:root' RETURN b.level, depth",
    )
    .unwrap();
    let mut rows = store.graph_ir_projected(&q).unwrap();
    rows.sort_by_key(|r| r.fields.get("depth").and_then(|v| v.as_u64()).unwrap_or(0));
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].fields.get("depth").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        rows[1].fields.get("depth").and_then(|v| v.as_u64()),
        Some(2)
    );
    store.close().unwrap();
}

#[test]
fn return_depth_with_order_by() {
    // RETURN depth combined with ORDER BY depth DESC.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rdd:root"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rdd:a"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rdd:b"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rdd:c"), br#"{}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rdd:1"),
        obj("obs://rdd:root"),
        obj("obs://rdd:a"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("e://rdd:2"),
        obj("obs://rdd:a"),
        obj("obs://rdd:b"),
        "L",
        None,
    );
    tx2.put_edge(
        eid("e://rdd:3"),
        obj("obs://rdd:b"),
        obj("obs://rdd:c"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:L*1..3]->(b) WHERE id(a) = 'obs://rdd:root' \
         RETURN depth ORDER BY depth DESC",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 3);
    let depths: Vec<u64> = rows
        .iter()
        .map(|r| r.fields.get("depth").and_then(|v| v.as_u64()).unwrap_or(0))
        .collect();
    assert_eq!(depths, vec![3, 2, 1]);
    store.close().unwrap();
}

// ── RETURN b.field[n] list indexing ─────────────────────────────────────────

#[test]
fn return_field_index_basic() {
    // b.tags[0] extracts the first element of a JSON array field.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rfi:hub"), br#"{}"#.to_vec());
    tx.put_object(
        obj("obs://rfi:a"),
        br#"{"tags":["rust","graph","fast"]}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rfi:a"),
        obj("obs://rfi:hub"),
        obj("obs://rfi:a"),
        "LINK",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:LINK*1..1]->(b) WHERE id(a) = 'obs://rfi:hub' RETURN b.tags[0]",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tags").and_then(|v| v.as_str()),
        Some("rust")
    );
    store.close().unwrap();
}

#[test]
fn return_field_index_second_element() {
    // b.tags[1] extracts the second element.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rfis:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rfis:a"), br#"{"scores":[10,20,30]}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rfis:a"),
        obj("obs://rfis:hub"),
        obj("obs://rfis:a"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:L*1..1]->(b) WHERE id(a) = 'obs://rfis:hub' RETURN b.scores[1]",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("scores").and_then(|v| v.as_i64()),
        Some(20)
    );
    store.close().unwrap();
}

#[test]
fn return_field_index_out_of_bounds() {
    // Index beyond array length returns null (field absent from result row).
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rfio:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rfio:a"), br#"{"items":["x"]}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rfio:a"),
        obj("obs://rfio:hub"),
        obj("obs://rfio:a"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:L*1..1]->(b) WHERE id(a) = 'obs://rfio:hub' RETURN b.items[5]",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    // null value means the key is absent from the row (projection loop omits null).
    assert!(!rows[0].fields.contains_key("items"));
    store.close().unwrap();
}

#[test]
fn return_field_index_with_alias() {
    // b.tags[0] AS first_tag uses the alias as the output key.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("obs://rfa:hub"), br#"{}"#.to_vec());
    tx.put_object(obj("obs://rfa:a"), br#"{"cats":["alpha","beta"]}"#.to_vec());
    store.commit(tx, "test", "").unwrap();
    let mut tx2 = Transaction::new();
    tx2.put_edge(
        eid("e://rfa:a"),
        obj("obs://rfa:hub"),
        obj("obs://rfa:a"),
        "L",
        None,
    );
    store.commit(tx2, "test", "").unwrap();

    let q = GraphIrQuery::parse(
        "MATCH (a)-[:L*1..1]->(b) WHERE id(a) = 'obs://rfa:hub' RETURN b.cats[0] AS first_cat",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("first_cat").and_then(|v| v.as_str()),
        Some("alpha")
    );
    store.close().unwrap();
}

// ── GraphIrScanQuery ────────────────────────────────────────────────────────

#[test]
fn scan_return_all_fields() {
    let dir = tmp();
    let store = build_scan_store(dir.path());
    let rows = store
        .graph_ir_scan("MATCH (a) WHERE id(a) STARTS WITH 'people://' RETURN a.*")
        .unwrap();
    assert_eq!(rows.len(), 4);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Bob"));
    assert!(names.contains(&"Carol"));
    assert!(names.contains(&"Dave"));
    store.close().unwrap();
}

#[test]
fn scan_payload_filter() {
    let dir = tmp();
    let store = build_scan_store(dir.path());
    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'people://' AND a.role = 'admin' RETURN a.name",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Carol"));
    store.close().unwrap();
}

#[test]
fn scan_order_by_and_limit() {
    let dir = tmp();
    let store = build_scan_store(dir.path());
    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'people://' RETURN a.name, a.age ORDER BY a.age ASC LIMIT 2",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].fields.get("age").and_then(|v| v.as_f64()),
        Some(22.0)
    );
    assert_eq!(
        rows[1].fields.get("age").and_then(|v| v.as_f64()),
        Some(25.0)
    );
    store.close().unwrap();
}

#[test]
fn scan_return_id() {
    let dir = tmp();
    let store = build_scan_store(dir.path());
    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'people://' AND a.role = 'user' RETURN id(a)",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("id(b)").and_then(|v| v.as_str()))
        .collect();
    assert!(ids.contains(&"people://bob"));
    assert!(ids.contains(&"people://dave"));
    store.close().unwrap();
}

#[test]
fn scan_parse_error_on_edge_pattern() {
    let err = GraphIrScanQuery::parse("MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'x' RETURN b.*");
    assert!(err.is_err());
}

#[test]
fn scan_numeric_gt_filter() {
    let dir = tmp();
    let store = build_scan_store(dir.path());
    let rows = store
        .graph_ir_scan("MATCH (a) WHERE id(a) STARTS WITH 'people://' AND a.age > 28 RETURN a.name")
        .unwrap();
    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"));
    assert!(names.contains(&"Carol"));
    store.close().unwrap();
}

// ── Multi-start fan-out ──────────────────────────────────────────────────────

#[test]
fn multi_start_fan_out_merges_results() {
    let dir = tmp();
    let store = build_multistart_store(dir.path());
    // alice → bob, carol, shared; dave → eve, shared (5 rows total without dedup)
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) IN ['sn://alice', 'sn://dave'] RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 5);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Bob"));
    assert!(names.contains(&"Carol"));
    assert!(names.contains(&"Eve"));
    assert!(names.contains(&"Shared"));
    store.close().unwrap();
}

#[test]
fn multi_start_distinct_deduplicates_shared_neighbor() {
    let dir = tmp();
    let store = build_multistart_store(dir.path());
    // Both alice and dave follow sn://shared — DISTINCT collapses it to one row.
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) IN ['sn://alice', 'sn://dave'] \
                 RETURN DISTINCT id(b)",
            )
            .unwrap(),
        )
        .unwrap();
    // unique reachable: bob, carol, shared, eve = 4
    assert_eq!(rows.len(), 4);
    store.close().unwrap();
}

#[test]
fn multi_start_single_element_matches_eq() {
    let dir = tmp();
    let store = build_multistart_store(dir.path());
    let rows_in = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) IN ['sn://alice'] RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    let rows_eq = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) = 'sn://alice' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows_in.len(), rows_eq.len());
    store.close().unwrap();
}

#[test]
fn multi_start_limit_applied_globally() {
    let dir = tmp();
    let store = build_multistart_store(dir.path());
    // Without LIMIT we'd get 5 rows; LIMIT 2 should give exactly 2.
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) IN ['sn://alice', 'sn://dave'] \
                 RETURN b.name ORDER BY b.name ASC LIMIT 2",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    // Global sort: Bob < Carol < Eve < Shared (alphabetically)
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(names[0], "Bob");
    assert_eq!(names[1], "Carol");
    store.close().unwrap();
}

#[test]
fn multi_start_parse_error_empty_list() {
    let err = GraphIrQuery::parse("MATCH (a)-[:FOLLOWS*1]->(b) WHERE id(a) IN [] RETURN b.name");
    assert!(err.is_err());
}

// ── HAS element predicate ────────────────────────────────────────────────────

#[test]
fn has_element_filters_to_matching_nodes() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_tagged_graph(&store);
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' AND b.tags HAS 'performance' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    // 'rust' and 'perf' have 'performance' tag
    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"rust"));
    assert!(names.contains(&"perf"));
    store.close().unwrap();
}

#[test]
fn has_element_negated_excludes_matching() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_tagged_graph(&store);
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' AND NOT b.tags HAS 'performance' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    // 'graph' and 'empty' do NOT have 'performance'
    assert_eq!(rows.len(), 2);
    let names: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.fields.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"graph"));
    assert!(names.contains(&"empty"));
    store.close().unwrap();
}

#[test]
fn has_element_on_empty_array_never_matches() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_tagged_graph(&store);
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' AND b.tags HAS 'anything' RETURN b.name",
            )
            .unwrap(),
        )
        .unwrap();
    // only 'rust' (systems/performance/safe), 'graph' (data/graph), 'perf' (performance/latency)
    // 'empty' never matches HAS 'anything' regardless
    for r in &rows {
        assert_ne!(r.fields.get("name").and_then(|v| v.as_str()), Some("empty"));
    }
    store.close().unwrap();
}

#[test]
fn has_element_parse_error_unquoted_value() {
    let err = GraphIrQuery::parse(
        "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' AND b.tags HAS performance RETURN b.name",
    );
    assert!(err.is_err());
}

// ── RETURN ARRAY_LENGTH(b.field) ─────────────────────────────────────────────

#[test]
fn return_array_length_projected() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_tagged_graph(&store);
    let mut rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' RETURN ARRAY_LENGTH(b.tags) AS tag_count",
            )
            .unwrap(),
        )
        .unwrap();
    rows.sort_by_key(|r| {
        r.fields
            .get("tag_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX)
    });
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].fields["tag_count"].as_u64(), Some(0)); // empty
    assert_eq!(rows[1].fields["tag_count"].as_u64(), Some(2)); // graph or perf
    assert_eq!(rows[2].fields["tag_count"].as_u64(), Some(2)); // graph or perf
    assert_eq!(rows[3].fields["tag_count"].as_u64(), Some(3)); // rust
    store.close().unwrap();
}

#[test]
fn return_array_length_non_array_passes_through() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_tagged_graph(&store);
    // 'name' is a string, not an array — pass through unchanged
    let rows = store
        .graph_ir_projected(
            &GraphIrQuery::parse(
                "MATCH (a)-[:LINKS*1]->(b) WHERE id(a) = 'hub://root' RETURN ARRAY_LENGTH(b.name) AS n",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert!(
            r.fields["n"].is_string(),
            "string field should pass through"
        );
    }
    store.close().unwrap();
}

// ── P50 / P99 / STDDEV / VARIANCE WITH aggregations ──────────────────────────

#[test]
fn agg_p50_returns_median() {
    // scores: 5, 10, 20 → sorted [5, 10, 20] → idx = round(0.50 * 2) = 1 → 10
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH p50(b.score) AS med RETURN med",
        )
        .unwrap();
    let v = result[0]
        .get("med")
        .and_then(|v| v.as_f64())
        .expect("med must be a number");
    assert!((v - 10.0).abs() < 1e-9, "expected 10.0 (median), got {v}");
    store.close().unwrap();
}

#[test]
fn agg_p99_returns_near_max() {
    // scores: 5, 10, 20 → sorted [5, 10, 20] → idx = round(0.99 * 2) = 2 → 20
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH p99(b.score) AS hi RETURN hi",
        )
        .unwrap();
    let v = result[0]
        .get("hi")
        .and_then(|v| v.as_f64())
        .expect("hi must be a number");
    assert!((v - 20.0).abs() < 1e-9, "expected 20.0 (p99), got {v}");
    store.close().unwrap();
}

#[test]
fn agg_stddev_population() {
    // scores: [5, 10, 20], mean=35/3, variance=350/9, stddev=sqrt(350/9)
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH stddev(b.score) AS sd RETURN sd",
        )
        .unwrap();
    let v = result[0]
        .get("sd")
        .and_then(|v| v.as_f64())
        .expect("sd must be a number");
    let expected = (350.0f64 / 9.0).sqrt();
    assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_variance_population() {
    // scores: [5, 10, 20], variance = 350/9
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH variance(b.score) AS vr RETURN vr",
        )
        .unwrap();
    let v = result[0]
        .get("vr")
        .and_then(|v| v.as_f64())
        .expect("vr must be a number");
    let expected = 350.0f64 / 9.0;
    assert!((v - expected).abs() < 1e-9, "expected {expected}, got {v}");
    store.close().unwrap();
}

#[test]
fn agg_stddev_empty_group_returns_zero() {
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'nobody://x' \
             WITH stddev(b.score) AS sd RETURN sd",
        )
        .unwrap();
    let v = result[0].get("sd").and_then(|v| v.as_f64()).unwrap_or(0.0);
    assert!((v - 0.0).abs() < 1e-9, "empty group stddev should be 0");
    store.close().unwrap();
}

#[test]
fn agg_parse_case_insensitive() {
    // P50/STDDEV/VARIANCE parsing must be case-insensitive.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_optional_graph(&store);
    let result = store
        .graph_ir_aggregated(
            "MATCH (a)-[:KNOWS*1..1]->(b) WHERE id(a) = 'person://alice' \
             WITH P50(b.score) AS med, STDDEV(b.score) AS sd, VARIANCE(b.score) AS vr \
             RETURN med, sd, vr",
        )
        .unwrap();
    assert!(result[0].get("med").and_then(|v| v.as_f64()).is_some());
    assert!(result[0].get("sd").and_then(|v| v.as_f64()).is_some());
    assert!(result[0].get("vr").and_then(|v| v.as_f64()).is_some());
    store.close().unwrap();
}

// ── UNWIND tests (Feature 1) ─────────────────────────────────────────────────

#[test]
fn unwind_array_field_produces_one_row_per_element() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    let q = GraphIrQuery::parse(
        "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' UNWIND b.tags AS tag RETURN tag",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    // obs://arr:one → 3 rows ("rust","perf","db")
    // obs://arr:two → 1 row ("graph")
    // obs://arr:three → 0 rows (empty array)
    assert_eq!(rows.len(), 4);
    let tags: std::collections::HashSet<String> = rows
        .iter()
        .map(|r| r.fields["tag"].as_str().unwrap().to_string())
        .collect();
    assert!(tags.contains("rust"));
    assert!(tags.contains("perf"));
    assert!(tags.contains("db"));
    assert!(tags.contains("graph"));
    store.close().unwrap();
}

#[test]
fn unwind_empty_array_produces_zero_rows() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    let q = GraphIrQuery::parse(
        "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' AND b.score = 0 UNWIND b.tags AS tag RETURN tag",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    // Only obs://arr:three matches (score=0), has empty tags → 0 rows
    assert_eq!(rows.len(), 0);
    store.close().unwrap();
}

#[test]
fn unwind_combined_with_other_return_fields() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    let q = GraphIrQuery::parse(
        "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' AND b.score = 1 UNWIND b.tags AS tag RETURN b.score, tag",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields["tag"].as_str().unwrap(), "graph");
    assert_eq!(rows[0].fields["score"].as_f64().unwrap(), 1.0);
    store.close().unwrap();
}

#[test]
fn unwind_non_array_field_produces_single_row_with_value() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    let q = GraphIrQuery::parse(
        "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' AND b.score = 1 UNWIND b.score AS val RETURN val",
    )
    .unwrap();
    let rows = store.graph_ir_projected(&q).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].fields["val"].as_f64().unwrap(), 1.0);
    store.close().unwrap();
}

// ── HAVING tests (Feature 2) ──────────────────────────────────────────────────

#[test]
fn with_aggregation_having_filters_groups() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    // 3 nodes match (score 3, 1, 0 from hub) — ungrouped count = 3
    // HAVING cnt > 2 → 3 > 2 → passes
    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' WITH count(b) AS cnt HAVING cnt > 2 RETURN cnt",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"].as_f64().unwrap(), 3.0);
    store.close().unwrap();
}

#[test]
fn with_aggregation_having_excludes_all_when_threshold_too_high() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' WITH count(b) AS cnt HAVING cnt > 10 RETURN cnt",
        )
        .unwrap();
    assert_eq!(rows.len(), 0);
    store.close().unwrap();
}

#[test]
fn with_aggregation_having_lte() {
    let dir = tmp();
    let store = build_arr_graph(dir.path());
    // count = 3; HAVING cnt <= 2 → 3 <= 2 → false → 0 rows
    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'obs://arr:hub' WITH count(b) AS cnt HAVING cnt <= 2 RETURN cnt",
        )
        .unwrap();
    assert_eq!(rows.len(), 0);
    store.close().unwrap();
}

#[test]
fn with_aggregation_having_grouped() {
    // Use build_grouped_graph: hub → 5 nodes across 3 categories
    // animal: 2 nodes, plant: 2 nodes, other: 1 node
    // WITH count(b) AS cnt GROUP BY b.type HAVING cnt >= 2
    // → animal (2) and plant (2) survive; other (1) is filtered out
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_grouped_graph(&store);
    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'h://root' WITH count(b) AS cnt GROUP BY b.type HAVING cnt >= 2 RETURN b.type, cnt",
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
    let types: std::collections::HashSet<String> = rows
        .iter()
        .map(|r| r["type"].as_str().unwrap().to_string())
        .collect();
    assert!(types.contains("animal"));
    assert!(types.contains("plant"));
    assert!(!types.contains("other"));
    store.close().unwrap();
}

// ── COUNT(DISTINCT …) aggregation ────────────────────────────────────────────

#[test]
fn count_distinct_field_values() {
    // hub → 5 nodes: 3 with type="animal", 2 with type="plant"
    // COUNT(DISTINCT b.type) should return 2.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(obj("cd://hub"), b"{}".to_vec());
    tx.put_object(obj("cd://n1"), br#"{"type":"animal"}"#.to_vec());
    tx.put_object(obj("cd://n2"), br#"{"type":"animal"}"#.to_vec());
    tx.put_object(obj("cd://n3"), br#"{"type":"animal"}"#.to_vec());
    tx.put_object(obj("cd://n4"), br#"{"type":"plant"}"#.to_vec());
    tx.put_object(obj("cd://n5"), br#"{"type":"plant"}"#.to_vec());
    tx.put_edge(eid("cd:e1"), obj("cd://hub"), obj("cd://n1"), "HAS", None);
    tx.put_edge(eid("cd:e2"), obj("cd://hub"), obj("cd://n2"), "HAS", None);
    tx.put_edge(eid("cd:e3"), obj("cd://hub"), obj("cd://n3"), "HAS", None);
    tx.put_edge(eid("cd:e4"), obj("cd://hub"), obj("cd://n4"), "HAS", None);
    tx.put_edge(eid("cd:e5"), obj("cd://hub"), obj("cd://n5"), "HAS", None);
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'cd://hub' \
             WITH count(distinct b.type) AS dc RETURN dc",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dc"].as_u64(), Some(2));
    store.close().unwrap();
}

#[test]
fn count_distinct_with_group_by() {
    // build_multikey_graph: hub → 6 nodes
    //   animal: cat (active), dog (active), wolf (retired) → 2 distinct statuses
    //   plant:  rose (active), fern (retired)              → 2 distinct statuses
    //   null:   rock (active)                              → 1 distinct status
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    build_multikey_graph(&store);

    let rows = store
        .graph_ir_aggregated(
            "MATCH (a)-[r]->(b) WHERE id(a) = 'h://mk_root' \
             WITH count(distinct b.status) AS dc GROUP BY b.type RETURN b.type, dc",
        )
        .unwrap();
    // 3 groups: animal, plant, null
    assert_eq!(rows.len(), 3);

    // Find the animal group and verify 2 distinct statuses.
    let animal_row = rows
        .iter()
        .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("animal"))
        .expect("animal group missing");
    assert_eq!(animal_row["dc"].as_u64(), Some(2));

    // Find the plant group and verify 2 distinct statuses.
    let plant_row = rows
        .iter()
        .find(|r| r.get("type").and_then(|v| v.as_str()) == Some("plant"))
        .expect("plant group missing");
    assert_eq!(plant_row["dc"].as_u64(), Some(2));

    store.close().unwrap();
}

// ── Scan UNWIND ──────────────────────────────────────────────────────────────

#[test]
fn scan_unwind_array_field() {
    // Objects with a `tags` array field; UNWIND a.tags AS tag should produce one row per tag.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("scan://alpha"),
        br#"{"tags":["rust","embedded","db"]}"#.to_vec(),
    );
    tx.put_object(
        obj("scan://beta"),
        br#"{"tags":["graph","query"]}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'scan://' UNWIND a.tags AS tag RETURN tag",
        )
        .unwrap();
    // alpha contributes 3 rows, beta contributes 2 rows → 5 total
    assert_eq!(rows.len(), 5);
    let tags: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| r.fields.get("tag").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .collect();
    assert!(tags.contains("rust"));
    assert!(tags.contains("embedded"));
    assert!(tags.contains("db"));
    assert!(tags.contains("graph"));
    assert!(tags.contains("query"));
    store.close().unwrap();
}

#[test]
fn scan_unwind_empty_array() {
    // Object with empty `tags: []`; UNWIND produces zero rows for that object.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("scan://empty"),
        br#"{"tags":[],"name":"empty"}"#.to_vec(),
    );
    tx.put_object(
        obj("scan://nonempty"),
        br#"{"tags":["x"],"name":"nonempty"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'scan://' UNWIND a.tags AS tag RETURN tag",
        )
        .unwrap();
    // empty object → 0 rows; nonempty → 1 row
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tag").and_then(|v| v.as_str()),
        Some("x")
    );
    store.close().unwrap();
}

#[test]
fn scan_unwind_non_array() {
    // Object where the UNWIND field is a string (not an array); should produce one row
    // with the string as the alias value.
    let dir = tmp();
    let store = BinderStore::open(dir.path()).unwrap();
    let mut tx = Transaction::new();
    tx.put_object(
        obj("scan://scalar"),
        br#"{"tags":"single","name":"scalar"}"#.to_vec(),
    );
    store.commit(tx, "test", "").unwrap();

    let rows = store
        .graph_ir_scan(
            "MATCH (a) WHERE id(a) STARTS WITH 'scan://' UNWIND a.tags AS tag RETURN tag",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].fields.get("tag").and_then(|v| v.as_str()),
        Some("single")
    );
    store.close().unwrap();
}
