//! Restricted graph query IR (issue #2).
//!
//! This is **not** Cypher. It parses a MATCH dialect and maps it onto
//! [`GraphQuery`]. Full Cypher (path collection, arbitrary RETURN projections,
//! write clauses) remains future work.
//!
//! Supported forms:
//! ```text
//! MATCH (a)-[:KNOWS|RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'memory://'
//! RETURN b
//!
//! -- JSON payload predicates (payload must be a UTF-8 JSON object):
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//!   AND id(b) STARTS WITH 'memory://'
//!   AND b.sentiment > 0.8
//! RETURN b
//!
//! -- Multiple payload predicates (AND-combined):
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//!   AND b.sentiment > 0.5
//!   AND b.score < 10
//! RETURN b
//!
//! -- Field projections (payload must be a UTF-8 JSON object):
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! RETURN b.sentiment
//!
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! RETURN b.sentiment, b.label
//!
//! -- WITH clause: filter / order / page mandatory results before optional expansion:
//! MATCH (a)-[:KNOWS*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! WITH b WHERE b.score > 0.5 ORDER BY b.score DESC LIMIT 5
//! OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c)
//! RETURN b.name, c.label
//!
//! -- WITH without OPTIONAL MATCH (standalone filter + page):
//! MATCH (a)-[:KNOWS*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! WITH b ORDER BY b.score DESC SKIP 10 LIMIT 5
//! RETURN b.name, b.score
//!
//! -- OPTIONAL MATCH: emit b even when no c is reachable (left-outer-join):
//! MATCH (a)-[:KNOWS*1..2]->(b)
//! WHERE id(a) = 'person://alice'
//! OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c)
//! RETURN b, c
//!
//! -- OPTIONAL MATCH with predicate on c and field projection:
//! MATCH (a)-[:KNOWS*1..2]->(b)
//! WHERE id(a) = 'person://alice'
//! OPTIONAL MATCH (b)-[:HAS_TAG*1..1]->(c)
//! WHERE id(c) STARTS WITH 'tag://' AND c.weight > 0.5
//! RETURN b.name, c.label
//! ```
//!
//! Payload predicate operators: `>`, `<`, `>=`, `<=`, `=`, `!=`.
//! The right-hand side may be a number literal or a quoted string.
//! String comparison uses lexicographic ordering.
//!
//! `RETURN b` returns matched node ids (`graph_ir` / `graph_ir_parsed`).
//! `RETURN b.field …` returns projected rows via `graph_ir_projected`.
//! OPTIONAL MATCH results are only available through `graph_ir_projected`.
//!
//! ## Ordering semantics
//!
//! - **Without WITH**: `ORDER BY / LIMIT / SKIP` in RETURN apply to mandatory `b`
//!   results before optional expansion (backward compatible).
//! - **With WITH**: `WITH … ORDER BY / LIMIT / SKIP` apply to mandatory `b` results
//!   before optional expansion; `RETURN … ORDER BY / LIMIT / SKIP` apply to the
//!   final expanded rows after optional expansion.
//!
//! Other notes:
//! - `*min..max` / `*n` set max_depth and filter results to `depth >= min`.
//! - Node filters apply to **results**, not traversal.
//! - Undirected `-[]-` → [`Direction::Both`]; `<-[]-` → Incoming; `-[]->` → Outgoing.

mod types;
pub use types::*;

mod exec;
mod parse;

mod write;
pub use write::{GraphIrWrite, GraphIrWriteError, WriteOp};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_query::Direction;

    #[test]
    fn parses_typed_hops_and_prefix() {
        let q = GraphIrQuery::parse(
            "MATCH (a)-[:KNOWS|RELATES_TO*1..3]->(b) WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'memory://' RETURN b",
        )
        .unwrap();
        assert_eq!(q.start.0, "person://alice");
        assert_eq!(q.edge_types, vec!["KNOWS", "RELATES_TO"]);
        assert_eq!(q.min_depth, 1);
        assert_eq!(q.max_depth, 3);
        assert_eq!(q.direction, Direction::Outgoing);
        assert_eq!(q.end_pred, NodeIdPred::StartsWith("memory://".into()));
    }

    #[test]
    fn parses_undirected() {
        let q = GraphIrQuery::parse("MATCH (a)-[:KNOWS]-(b) WHERE id(a) = 'memory://m1' RETURN b")
            .unwrap();
        assert_eq!(q.direction, Direction::Both);
        assert_eq!(q.min_depth, 1);
        assert_eq!(q.max_depth, 6);
    }

    #[test]
    fn parses_incoming() {
        let q = GraphIrQuery::parse(
            "MATCH (a)<-[:KNOWS*1..2]-(b) WHERE id(a) = 'memory://m1' RETURN b",
        )
        .unwrap();
        assert_eq!(q.direction, Direction::Incoming);
        assert_eq!(q.min_depth, 1);
        assert_eq!(q.max_depth, 2);
    }

    #[test]
    fn parses_exact_hops() {
        let q = GraphIrQuery::parse("MATCH (a)-[*2]->(b) WHERE id(a) = 'x' RETURN b").unwrap();
        assert_eq!(q.min_depth, 2);
        assert_eq!(q.max_depth, 2);
    }

    #[test]
    fn parses_payload_pred_gt() {
        let q = GraphIrQuery::parse(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'x' AND b.sentiment > 0.8 RETURN b",
        )
        .unwrap();
        assert_eq!(q.payload_preds.len(), 1);
        let pred = &q.payload_preds[0];
        assert_eq!(pred.field, "sentiment");
        assert_eq!(pred.op, CmpOp::Gt);
        assert_eq!(pred.value, CmpValue::Number(0.8));
    }

    #[test]
    fn parses_payload_pred_string_eq() {
        let q = GraphIrQuery::parse(
            "MATCH (a)-[]->(b) WHERE id(a) = 'x' AND b.status = 'active' RETURN b",
        )
        .unwrap();
        assert_eq!(q.payload_preds.len(), 1);
        let pred = &q.payload_preds[0];
        assert_eq!(pred.field, "status");
        assert_eq!(pred.op, CmpOp::Eq);
        assert_eq!(pred.value, CmpValue::Text("active".into()));
    }

    #[test]
    fn no_payload_pred_when_absent() {
        let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b").unwrap();
        assert!(q.payload_preds.is_empty());
        assert!(q.return_fields.is_empty());
    }

    #[test]
    fn parses_multiple_payload_preds() {
        let q = GraphIrQuery::parse(
            "MATCH (a)-[]->(b) WHERE id(a) = 'x' AND b.sentiment > 0.5 AND b.score < 10 RETURN b",
        )
        .unwrap();
        assert_eq!(q.payload_preds.len(), 2);
        assert_eq!(q.payload_preds[0].field, "sentiment");
        assert_eq!(q.payload_preds[0].op, CmpOp::Gt);
        assert_eq!(q.payload_preds[1].field, "score");
        assert_eq!(q.payload_preds[1].op, CmpOp::Lt);
    }

    #[test]
    fn parses_return_single_field() {
        let q =
            GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b.sentiment").unwrap();
        assert_eq!(q.return_fields, vec!["sentiment"]);
    }

    #[test]
    fn parses_return_multiple_fields() {
        let q =
            GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b.sentiment, b.label")
                .unwrap();
        assert_eq!(q.return_fields, vec!["sentiment", "label"]);
    }
}
