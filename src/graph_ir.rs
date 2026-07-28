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

use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::graph_query::{Direction, GraphQuery, GraphQueryMatch};
use crate::transaction::Transaction;
use crate::types::{CommitId, EdgeId, ObjectId};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphIrError {
    #[error("empty query")]
    Empty,
    #[error("unsupported graph IR: {0}")]
    Unsupported(String),
    #[error("missing start id(a) = '…' in WHERE")]
    MissingStart,
    #[error("invalid hop bound")]
    BadHops,
    #[error("no aggregating WITH clause; use graph_ir_projected instead")]
    NoAggregation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeIdPred {
    /// No predicate on matched end nodes.
    Any,
    StartsWith(String),
    Equals(String),
}

/// Comparison operator for payload field predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// Right-hand side of a payload predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum CmpValue {
    Number(f64),
    Text(String),
}

/// Predicate on a JSON field of the matched end node's payload.
/// Payload must be a UTF-8 JSON object; non-JSON payloads never match.
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadPred {
    pub field: String,
    pub op: CmpOp,
    pub value: CmpValue,
}

/// The `OPTIONAL MATCH` clause in a two-clause query.
///
/// Traversal starts from each matched `b` node in the mandatory clause and
/// tries to reach `c` nodes. When no `c` nodes are reachable, `GraphIrRow::optional`
/// is set to `None` so the mandatory row is still emitted.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrOptionalClause {
    pub edge_types: Vec<String>,
    /// Minimum hop depth from `b` included in optional results.
    pub min_depth: usize,
    /// Maximum hop depth from `b`.
    pub max_depth: usize,
    pub direction: Direction,
    pub end_pred: NodeIdPred,
    /// Payload field predicates applied to matched `c` nodes (AND-combined).
    pub payload_preds: Vec<PayloadPred>,
    /// Fields to project from each `c` node's JSON payload (`RETURN c.f1, c.f2`).
    pub return_fields: Vec<String>,
}

/// One result from an `OPTIONAL MATCH` clause.
///
/// Absent (`None` in [`GraphIrRow::optional`]) when no `c` node was reachable
/// from the mandatory `b` node under the optional clause's constraints.
#[derive(Debug, Clone)]
pub struct GraphIrOptionalMatch {
    pub object_id: ObjectId,
    /// Hop depth from the mandatory `b` node to this `c` node.
    pub depth: usize,
    /// Projected fields from `RETURN c.f1, c.f2`. Empty when no fields were requested.
    pub fields: HashMap<String, serde_json::Value>,
}

/// Aggregation function for a `WITH` aggregation clause.
///
/// Used in `WITH count(b) AS cnt` / `WITH sum(b.score) AS s` forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// One aggregation item in a `WITH count(b) AS alias` clause.
#[derive(Debug, Clone, PartialEq)]
pub struct WithAggregation {
    pub func: AggFunc,
    /// Payload field name (`b.field`) for sum/avg/min/max; `None` for `count(b)`.
    pub field: Option<String>,
    pub alias: String,
}

/// The `WITH b` clause between a mandatory MATCH and a subsequent OPTIONAL MATCH
/// or RETURN.
///
/// Two forms:
/// - **Pass-through** (`WITH b [WHERE …] [ORDER BY …] [SKIP n] [LIMIT n]`): filters,
///   sorts, and pages mandatory `b` results before optional expansion. When present,
///   `RETURN ORDER BY / LIMIT / SKIP` apply to the final expanded rows.
/// - **Aggregating** (`WITH count(b) AS cnt [, sum(b.score) AS s …]`): collapses all
///   matched `b` nodes into scalar values via `graph_ir_aggregated`. When
///   `aggregations` is non-empty the other fields are unused.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrWithClause {
    /// Additional id predicate on matched `b` nodes, applied after the MATCH.
    pub end_pred: NodeIdPred,
    /// Additional payload predicates on `b` nodes (AND-combined), applied after the MATCH.
    pub payload_preds: Vec<PayloadPred>,
    /// Sort mandatory `b` results before optional expansion.
    pub order_by: Option<GraphIrOrderBy>,
    /// Limit applied to mandatory results after ordering (before optional expansion).
    pub limit: Option<usize>,
    /// Skip applied to mandatory results after ordering (before optional expansion).
    pub skip: Option<usize>,
    /// Aggregation items. Non-empty → aggregating WITH; use `graph_ir_aggregated`.
    pub aggregations: Vec<WithAggregation>,
    /// Fields to group by (`b.f1, b.f2` → `["f1", "f2"]`). Non-empty → grouped
    /// aggregation; each unique combination of values produces a separate row.
    /// Single-field case is `["field"]`; replaces the former `Option<String>`.
    pub group_by_fields: Vec<String>,
}

/// One row returned by `graph_ir_projected`.
///
/// `fields` contains only the keys requested in `RETURN b.field1, b.field2`.
/// Missing or non-JSON payloads produce no entries for that key.
///
/// `path` is the sequence of node IDs from the start node to `object_id`
/// (inclusive of both endpoints) when `RETURN path` was requested; otherwise empty.
///
/// `optional` is populated when the query contains an `OPTIONAL MATCH` clause.
/// It is `None` when the optional pattern found no matches for this mandatory row,
/// and `Some` for each individually matched optional node.
#[derive(Debug, Clone)]
pub struct GraphIrRow {
    pub object_id: ObjectId,
    pub depth: usize,
    pub fields: HashMap<String, serde_json::Value>,
    pub path: Vec<ObjectId>,
    /// Result of the `OPTIONAL MATCH` clause, if present in the query.
    pub optional: Option<GraphIrOptionalMatch>,
}

/// Field to sort graph query results by.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderByField {
    /// Sort by the matched node's object ID string (lexicographic).
    ObjectId,
    /// Sort by the traversal depth (number of hops from start).
    Depth,
    /// Sort by a named field extracted from the matched node's JSON payload.
    PayloadField(String),
}

/// Sort direction for ORDER BY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderDirection {
    Asc,
    Desc,
}

/// An ORDER BY clause in a graph IR query.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrOrderBy {
    pub field: OrderByField,
    pub direction: OrderDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrQuery {
    pub start: ObjectId,
    pub edge_types: Vec<String>,
    /// Minimum hop depth included in results (1 = include immediate neighbours).
    pub min_depth: usize,
    /// Max hops from start (builder max_depth).
    pub max_depth: usize,
    pub direction: Direction,
    pub end_pred: NodeIdPred,
    /// Payload field predicates applied to matched end nodes (AND-combined).
    /// All predicates must hold for a node to be included in results.
    /// Empty = no payload filtering.
    pub payload_preds: Vec<PayloadPred>,
    /// Fields to project from each matched node's JSON payload (`RETURN b.f1, b.f2`).
    /// Empty = plain node return via `graph_ir` / `graph_ir_parsed`.
    pub return_fields: Vec<String>,
    /// When true, `graph_ir_projected` populates `GraphIrRow::path` with the
    /// sequence of node IDs from the start node to each matched node.
    pub return_path: bool,
    /// Optional ORDER BY clause applied after min-depth filtering.
    pub order_by: Option<GraphIrOrderBy>,
    /// Maximum number of results to return (applied after ORDER BY and SKIP).
    pub limit: Option<usize>,
    /// Number of results to skip before applying LIMIT (applied after ORDER BY).
    pub skip: Option<usize>,
    /// Optional second traversal clause. When present, each mandatory result is
    /// used as the start node for this clause. Results with no optional match
    /// are emitted with `GraphIrRow::optional = None`.
    ///
    /// When `with_clause` is absent: ORDER BY / LIMIT / SKIP are applied to
    /// mandatory results before optional expansion (backward compatible).
    /// When `with_clause` is present: ORDER BY / LIMIT / SKIP are applied to
    /// the final expanded rows after optional expansion.
    pub optional_clause: Option<GraphIrOptionalClause>,
    /// Optional WITH clause. Filters and pages mandatory `b` results before
    /// they enter optional expansion or RETURN. See [`GraphIrWithClause`].
    pub with_clause: Option<GraphIrWithClause>,
}

impl GraphIrQuery {
    /// Parse the restricted MATCH dialect. Keywords are case-insensitive.
    pub fn parse(src: &str) -> Result<Self, GraphIrError> {
        let raw = collapse_ws(src.trim());
        if raw.is_empty() {
            return Err(GraphIrError::Empty);
        }
        if !starts_with_keyword(&raw, "MATCH") {
            return Err(GraphIrError::Unsupported("must start with MATCH".into()));
        }
        let after_match = skip_keyword(&raw, "MATCH");

        let (direction, edge_types, min_depth, max_depth, after_pattern) =
            parse_pattern(after_match)?;
        let after_pattern = after_pattern.trim_start();

        if !starts_with_keyword(after_pattern, "WHERE") {
            return Err(GraphIrError::Unsupported("expected WHERE".into()));
        }
        let after_where = skip_keyword(after_pattern, "WHERE");
        let (start, end_pred, after_preds) = parse_where(after_where)?;
        let after_preds = after_preds.trim_start();

        // Optional payload predicates: AND b.FIELD OP VALUE (may repeat)
        let mut payload_preds: Vec<PayloadPred> = Vec::new();
        let mut after_preds2 = after_preds;
        loop {
            if starts_with_keyword(after_preds2, "AND") {
                let after_and = skip_keyword(after_preds2, "AND").trim_start();
                if after_and.starts_with("b.") || after_and.starts_with("B.") {
                    let (pred, rest) = parse_payload_pred(after_and)?;
                    payload_preds.push(pred);
                    after_preds2 = rest.trim_start();
                    continue;
                }
            }
            break;
        }

        // Optional WITH clause: aggregating form or pass-through form.
        let (with_clause, after_with) = if starts_with_keyword(after_preds2, "WITH") {
            let after_with_kw = skip_keyword(after_preds2, "WITH");
            if is_agg_func_start(after_with_kw) {
                // Aggregating: WITH count(b) AS cnt [, …] [GROUP BY b.f1 [, b.f2 …]]
                let (aggs, group_by_fields, after_aggs) = parse_with_aggregations(after_with_kw)?;
                let wc = GraphIrWithClause {
                    end_pred: NodeIdPred::Any,
                    payload_preds: vec![],
                    order_by: None,
                    limit: None,
                    skip: None,
                    aggregations: aggs,
                    group_by_fields,
                };
                (Some(wc), after_aggs.trim_start())
            } else {
                // Pass-through: WITH b [WHERE …] [ORDER BY …] [SKIP n] [LIMIT n]
                if !starts_with_keyword(after_with_kw, "b") {
                    return Err(GraphIrError::Unsupported(
                        "WITH must project variable 'b' or start with an aggregation function"
                            .into(),
                    ));
                }
                let after_b = skip_keyword(after_with_kw, "b").trim_start();
                let (wc_end_pred, wc_preds, after_wc_where) = parse_with_where(after_b)?;
                let (wc_ob, wc_limit, wc_skip, after_wc_ordering) =
                    parse_ordering_tail_rest(after_wc_where)?;
                let wc = GraphIrWithClause {
                    end_pred: wc_end_pred,
                    payload_preds: wc_preds,
                    order_by: wc_ob,
                    limit: wc_limit,
                    skip: wc_skip,
                    aggregations: vec![],
                    group_by_fields: vec![],
                };
                (Some(wc), after_wc_ordering.trim_start())
            }
        } else {
            (None, after_preds2)
        };

        // Optional MATCH clause (OPTIONAL MATCH (b)-[…]->(c) [WHERE …])
        let (optional_clause_raw, after_preds3) = if starts_with_keyword(after_with, "OPTIONAL") {
            let after_opt = skip_keyword(after_with, "OPTIONAL");
            if !starts_with_keyword(after_opt, "MATCH") {
                return Err(GraphIrError::Unsupported(
                    "expected MATCH after OPTIONAL".into(),
                ));
            }
            let after_opt_match = skip_keyword(after_opt, "MATCH");
            let (opt_dir, opt_types, opt_min, opt_max, after_opt_pat) =
                parse_pattern(after_opt_match)?;
            let after_opt_pat = after_opt_pat.trim_start();
            let (opt_end_pred, opt_preds, after_opt_where) = parse_optional_where(after_opt_pat)?;
            let oc = GraphIrOptionalClause {
                edge_types: opt_types,
                min_depth: opt_min,
                max_depth: opt_max,
                direction: opt_dir,
                end_pred: opt_end_pred,
                payload_preds: opt_preds,
                return_fields: vec![], // filled in after RETURN is parsed
            };
            (Some(oc), after_opt_where.trim_start())
        } else {
            (None, after_with)
        };

        if !starts_with_keyword(after_preds3, "RETURN") {
            return Err(GraphIrError::Unsupported("expected RETURN".into()));
        }
        let after_return = skip_keyword(after_preds3, "RETURN");
        let (ret_spec, ordering_tail) = split_at_ordering_clause(after_return);
        let (return_fields, c_return_fields, return_path) = parse_return_spec(ret_spec);
        let (order_by, limit, skip) = parse_ordering_tail(ordering_tail)?;

        let optional_clause = optional_clause_raw.map(|mut oc| {
            oc.return_fields = c_return_fields;
            oc
        });

        Ok(GraphIrQuery {
            start,
            edge_types,
            min_depth,
            max_depth,
            direction,
            end_pred,
            payload_preds,
            return_fields,
            return_path,
            order_by,
            limit,
            skip,
            optional_clause,
            with_clause,
        })
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

fn starts_with_keyword(s: &str, kw: &str) -> bool {
    let s = s.trim_start();
    if s.len() < kw.len() || !s[..kw.len()].eq_ignore_ascii_case(kw) {
        return false;
    }
    if s.len() == kw.len() {
        return true;
    }
    let next = s.as_bytes()[kw.len()];
    !next.is_ascii_alphanumeric() && next != b'_'
}

fn skip_keyword<'a>(s: &'a str, kw: &str) -> &'a str {
    let s = s.trim_start();
    debug_assert!(starts_with_keyword(s, kw));
    s[kw.len()..].trim_start()
}

/// Parse `(a)-[:TYPES*hops]->(b)` / undirected / incoming variants.
fn parse_pattern(s: &str) -> Result<(Direction, Vec<String>, usize, usize, &str), GraphIrError> {
    let s = s.trim_start();
    // Require `(…)` start node binding (name ignored).
    if !s.starts_with('(') {
        return Err(GraphIrError::Unsupported(
            "expected (start)-[…]- pattern".into(),
        ));
    }
    let after_open = &s[1..];
    let close = after_open
        .find(')')
        .ok_or_else(|| GraphIrError::Unsupported("unclosed start node".into()))?;
    let after_start = after_open[close + 1..].trim_start();

    // Direction + relationship bracket.
    let (direction, after_arrow) = if let Some(rest) = after_start.strip_prefix("<-[") {
        (Direction::Incoming, rest)
    } else if let Some(rest) = after_start.strip_prefix("-[") {
        // May be `-[…]→` or `-[…]-`
        (Direction::Outgoing, rest) // provisional; fix after ]
    } else {
        return Err(GraphIrError::Unsupported(
            "expected -[ or <-[ relationship".into(),
        ));
    };

    let end_bracket = after_arrow
        .find(']')
        .ok_or_else(|| GraphIrError::Unsupported("unclosed relationship".into()))?;
    let rel_body = after_arrow[..end_bracket].trim();
    let after_rel = after_arrow[end_bracket + 1..].trim_start();

    let (edge_types, min_depth, max_depth) = parse_rel_body(rel_body)?;

    let (direction, after_dir) = match direction {
        Direction::Incoming => {
            if let Some(rest) = after_rel.strip_prefix('-') {
                (Direction::Incoming, rest.trim_start())
            } else {
                return Err(GraphIrError::Unsupported(
                    "incoming pattern must end with -(".into(),
                ));
            }
        }
        Direction::Outgoing => {
            if let Some(rest) = after_rel.strip_prefix("->") {
                (Direction::Outgoing, rest.trim_start())
            } else if let Some(rest) = after_rel.strip_prefix('-') {
                (Direction::Both, rest.trim_start())
            } else {
                return Err(GraphIrError::Unsupported(
                    "expected -> or - after relationship".into(),
                ));
            }
        }
        Direction::Both => unreachable!(),
    };

    if !after_dir.starts_with('(') {
        return Err(GraphIrError::Unsupported("expected (end) node".into()));
    }
    let after_end_open = &after_dir[1..];
    let end_close = after_end_open
        .find(')')
        .ok_or_else(|| GraphIrError::Unsupported("unclosed end node".into()))?;
    let rest = after_end_open[end_close + 1..].trim_start();
    Ok((direction, edge_types, min_depth, max_depth, rest))
}

fn parse_rel_body(body: &str) -> Result<(Vec<String>, usize, usize), GraphIrError> {
    // Forms: "", "*2", "*1..3", ":KNOWS", ":KNOWS|TAGGED", ":KNOWS*1..3", ":A|B*2"
    let body = body.trim();
    if body.is_empty() {
        return Ok((Vec::new(), 1, 6));
    }

    let (types_part, hops_part) = if let Some(star) = body.find('*') {
        (&body[..star], Some(&body[star + 1..]))
    } else {
        (body, None)
    };

    let types_part = types_part.trim();
    let mut edge_types = Vec::new();
    if !types_part.is_empty() {
        let types_part = types_part
            .strip_prefix(':')
            .ok_or_else(|| GraphIrError::Unsupported("edge types must start with :".into()))?;
        for t in types_part.split('|') {
            let t = t.trim();
            if t.is_empty() {
                return Err(GraphIrError::Unsupported("empty edge type".into()));
            }
            if !t
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(GraphIrError::Unsupported(format!(
                    "invalid edge type token: {t}"
                )));
            }
            edge_types.push(t.to_string());
        }
    }

    let (min_depth, max_depth) = match hops_part {
        None => (1, 6),
        Some(h) => parse_hops(h.trim())?,
    };
    Ok((edge_types, min_depth, max_depth))
}

fn parse_hops(h: &str) -> Result<(usize, usize), GraphIrError> {
    if let Some((a, b)) = h.split_once("..") {
        let min: usize = a.trim().parse().map_err(|_| GraphIrError::BadHops)?;
        let max: usize = b.trim().parse().map_err(|_| GraphIrError::BadHops)?;
        if min == 0 || max == 0 || min > max {
            return Err(GraphIrError::BadHops);
        }
        Ok((min, max))
    } else {
        let n: usize = h.parse().map_err(|_| GraphIrError::BadHops)?;
        if n == 0 {
            return Err(GraphIrError::BadHops);
        }
        Ok((n, n))
    }
}

fn parse_where(s: &str) -> Result<(ObjectId, NodeIdPred, &str), GraphIrError> {
    let s = s.trim_start();
    // id(a) = '…'  [AND id(b) STARTS WITH '…' | AND id(b) = '…']
    let (start, rest) = parse_id_eq(s, "a")?;
    let rest = rest.trim_start();
    if starts_with_keyword(rest, "AND") {
        let after_and = skip_keyword(rest, "AND").trim_start();
        // Peek: if it starts with id(b) parse as end predicate; if b. it's a payload pred.
        if after_and.starts_with("id(") || after_and.starts_with("ID(") {
            let (end_pred, rest2) = parse_end_pred(after_and)?;
            Ok((start, end_pred, rest2))
        } else {
            // Payload pred comes after; leave rest pointing at AND so the
            // caller's payload-pred parser sees it.
            Ok((start, NodeIdPred::Any, rest))
        }
    } else {
        Ok((start, NodeIdPred::Any, rest))
    }
}

/// Parse `VAR.field OP value` for an arbitrary lowercase variable name.
/// `s` must start with `VAR.` (case-insensitive prefix already verified by caller).
fn parse_payload_pred_for_var<'a>(
    s: &'a str,
    var: &str,
) -> Result<(PayloadPred, &'a str), GraphIrError> {
    let prefix_len = var.len() + 1; // "c."
    parse_payload_pred_inner(&s[prefix_len..], var)
}

fn parse_payload_pred(s: &str) -> Result<(PayloadPred, &str), GraphIrError> {
    // s starts with "b." or "B."
    parse_payload_pred_inner(&s[2..], "b")
}

fn parse_payload_pred_inner<'a>(
    s: &'a str,
    var: &str,
) -> Result<(PayloadPred, &'a str), GraphIrError> {
    // s starts after "VAR." — field name: alphanumeric + underscore
    let field_end = s
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if field_end == 0 {
        return Err(GraphIrError::Unsupported(format!(
            "expected field name after {var}."
        )));
    }
    let field = s[..field_end].to_string();
    let rest = s[field_end..].trim_start();

    let (op, rest) = if let Some(r) = rest.strip_prefix(">=") {
        (CmpOp::Gte, r.trim_start())
    } else if let Some(r) = rest.strip_prefix("<=") {
        (CmpOp::Lte, r.trim_start())
    } else if let Some(r) = rest.strip_prefix("!=") {
        (CmpOp::Ne, r.trim_start())
    } else if let Some(r) = rest.strip_prefix('>') {
        (CmpOp::Gt, r.trim_start())
    } else if let Some(r) = rest.strip_prefix('<') {
        (CmpOp::Lt, r.trim_start())
    } else if let Some(r) = rest.strip_prefix('=') {
        (CmpOp::Eq, r.trim_start())
    } else {
        return Err(GraphIrError::Unsupported(format!(
            "expected comparison operator after {var}.{field}"
        )));
    };

    // RHS: quoted string or numeric literal
    let (value, rest) = if rest.starts_with('\'') || rest.starts_with('"') {
        let (s, r) = parse_quoted(rest)?;
        (CmpValue::Text(s), r)
    } else {
        let num_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(rest.len());
        if num_end == 0 {
            return Err(GraphIrError::Unsupported(
                "expected number or string on RHS of payload predicate".into(),
            ));
        }
        let n: f64 = rest[..num_end]
            .parse()
            .map_err(|_| GraphIrError::Unsupported("invalid number literal".into()))?;
        (CmpValue::Number(n), &rest[num_end..])
    };

    Ok((PayloadPred { field, op, value }, rest))
}

fn parse_id_eq<'a>(s: &'a str, var: &str) -> Result<(ObjectId, &'a str), GraphIrError> {
    let s = s.trim_start();
    let prefix = format!("id({var})");
    if s.len() < prefix.len() || !s[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return Err(GraphIrError::MissingStart);
    }
    let after = s[prefix.len()..].trim_start();
    let Some(after_eq) = after.strip_prefix('=') else {
        return Err(GraphIrError::Unsupported(format!("expected id({var}) =")));
    };
    let after_eq = after_eq.trim_start();
    let (val, rest) = parse_quoted(after_eq)?;
    Ok((ObjectId::new(val), rest))
}

fn parse_end_pred(s: &str) -> Result<(NodeIdPred, &str), GraphIrError> {
    let s = s.trim_start();
    let prefix = "id(b)";
    if s.len() < prefix.len() || !s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return Err(GraphIrError::Unsupported(
            "AND clause must be id(b) STARTS WITH or id(b) =".into(),
        ));
    }
    let after = s[prefix.len()..].trim_start();
    if starts_with_keyword(after, "STARTS") {
        let after_starts = skip_keyword(after, "STARTS");
        if !starts_with_keyword(after_starts, "WITH") {
            return Err(GraphIrError::Unsupported("expected STARTS WITH".into()));
        }
        let after_with = skip_keyword(after_starts, "WITH");
        let (val, rest) = parse_quoted(after_with)?;
        Ok((NodeIdPred::StartsWith(val), rest))
    } else if let Some(after_eq) = after.strip_prefix('=') {
        let (val, rest) = parse_quoted(after_eq.trim_start())?;
        Ok((NodeIdPred::Equals(val), rest))
    } else {
        Err(GraphIrError::Unsupported(
            "expected id(b) STARTS WITH or id(b) =".into(),
        ))
    }
}

/// Parse `id(VAR) STARTS WITH '…'` or `id(VAR) = '…'` for a named variable.
fn parse_end_pred_for_var<'a>(
    s: &'a str,
    var: &str,
) -> Result<(NodeIdPred, &'a str), GraphIrError> {
    let prefix = format!("id({var})");
    if s.len() < prefix.len() || !s[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return Err(GraphIrError::Unsupported(format!(
            "expected id({var}) STARTS WITH or id({var}) ="
        )));
    }
    let after = s[prefix.len()..].trim_start();
    if starts_with_keyword(after, "STARTS") {
        let after_starts = skip_keyword(after, "STARTS");
        if !starts_with_keyword(after_starts, "WITH") {
            return Err(GraphIrError::Unsupported("expected STARTS WITH".into()));
        }
        let after_with = skip_keyword(after_starts, "WITH");
        let (val, rest) = parse_quoted(after_with)?;
        Ok((NodeIdPred::StartsWith(val), rest))
    } else if let Some(after_eq) = after.strip_prefix('=') {
        let (val, rest) = parse_quoted(after_eq.trim_start())?;
        Ok((NodeIdPred::Equals(val), rest))
    } else {
        Err(GraphIrError::Unsupported(format!(
            "expected id({var}) STARTS WITH or id({var}) ="
        )))
    }
}

/// Parse the optional WHERE clause that follows `OPTIONAL MATCH (b)-[…]->(c)`.
///
/// Supported predicates (all on variable `c`):
/// - `id(c) STARTS WITH '…'` / `id(c) = '…'`  — end-node ID constraint
/// - `c.field OP value`                         — JSON payload predicates
/// - Combinations connected by AND
///
/// Returns `(end_pred, payload_preds, remaining)`.
fn parse_optional_where(s: &str) -> Result<(NodeIdPred, Vec<PayloadPred>, &str), GraphIrError> {
    let s = s.trim_start();
    if !starts_with_keyword(s, "WHERE") {
        return Ok((NodeIdPred::Any, vec![], s));
    }
    let mut rest = skip_keyword(s, "WHERE").trim_start();

    let mut end_pred = NodeIdPred::Any;
    let mut payload_preds: Vec<PayloadPred> = Vec::new();

    // Parse predicates until we hit something that isn't id(c) or c.field
    loop {
        let t = rest.trim_start();
        if (t.len() >= 5 && t[..5].eq_ignore_ascii_case("id(c)"))
            || (t.len() >= 5 && t[..5].eq_ignore_ascii_case("id(C)"))
        {
            let (pred, next) = parse_end_pred_for_var(t, "c")?;
            end_pred = pred;
            rest = next.trim_start();
        } else if t.starts_with("c.") || t.starts_with("C.") {
            let (pred, next) = parse_payload_pred_for_var(t, "c")?;
            payload_preds.push(pred);
            rest = next.trim_start();
        } else {
            break;
        }
        // Consume AND before the next predicate (if present)
        if starts_with_keyword(rest, "AND") {
            rest = skip_keyword(rest, "AND").trim_start();
        } else {
            break;
        }
    }

    Ok((end_pred, payload_preds, rest))
}

/// Parse the optional WHERE predicates that follow `WITH b`.
///
/// Supports: `id(b) STARTS WITH '…'`, `id(b) = '…'`, `b.field OP value`, `AND`-connected.
/// Returns `(end_pred, payload_preds, remaining)`.
fn parse_with_where(s: &str) -> Result<(NodeIdPred, Vec<PayloadPred>, &str), GraphIrError> {
    let s = s.trim_start();
    if !starts_with_keyword(s, "WHERE") {
        return Ok((NodeIdPred::Any, vec![], s));
    }
    let mut rest = skip_keyword(s, "WHERE").trim_start();
    let mut end_pred = NodeIdPred::Any;
    let mut payload_preds: Vec<PayloadPred> = Vec::new();

    loop {
        let t = rest.trim_start();
        if t.len() >= 5 && t[..5].eq_ignore_ascii_case("id(b)") {
            let (pred, next) = parse_end_pred_for_var(t, "b")?;
            end_pred = pred;
            rest = next.trim_start();
        } else if t.starts_with("b.") || t.starts_with("B.") {
            let (pred, next) = parse_payload_pred_for_var(t, "b")?;
            payload_preds.push(pred);
            rest = next.trim_start();
        } else {
            break;
        }
        if starts_with_keyword(rest, "AND") {
            rest = skip_keyword(rest, "AND").trim_start();
        } else {
            break;
        }
    }

    Ok((end_pred, payload_preds, rest))
}

/// Return true if `s` starts with a known aggregation function name followed by `(`.
fn is_agg_func_start(s: &str) -> bool {
    let s = s.trim_start();
    for prefix in &["count(", "sum(", "avg(", "min(", "max("] {
        if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
            return true;
        }
    }
    false
}

/// Parse a comma-separated list of aggregation items plus an optional GROUP BY clause:
/// `count(b) AS cnt [, sum(b.field) AS s …] [GROUP BY b.f1 [, b.f2 …]]`
///
/// Returns `(aggregations, group_by_fields, remaining)`.
fn parse_with_aggregations(
    s: &str,
) -> Result<(Vec<WithAggregation>, Vec<String>, &str), GraphIrError> {
    let mut aggs = Vec::new();
    let mut s = s.trim_start();
    loop {
        let (agg, rest) = parse_one_aggregation(s)?;
        aggs.push(agg);
        s = rest.trim_start();
        if s.starts_with(',') {
            s = s[1..].trim_start();
        } else {
            break;
        }
    }

    // Optional GROUP BY b.f1 [, b.f2 …]
    let (group_by_fields, s) = if starts_with_keyword(s, "GROUP") {
        let after_group = skip_keyword(s, "GROUP");
        if !starts_with_keyword(after_group, "BY") {
            return Err(GraphIrError::Unsupported("expected GROUP BY".into()));
        }
        let mut rest = skip_keyword(after_group, "BY").trim_start();
        let mut fields = Vec::new();
        loop {
            if !rest.starts_with("b.") && !rest.starts_with("B.") {
                return Err(GraphIrError::Unsupported(
                    "GROUP BY fields must be b.field".into(),
                ));
            }
            let after_dot = &rest[2..];
            let field_end = after_dot
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(after_dot.len());
            if field_end == 0 {
                return Err(GraphIrError::Unsupported(
                    "expected field name after GROUP BY b.".into(),
                ));
            }
            fields.push(after_dot[..field_end].to_string());
            rest = after_dot[field_end..].trim_start();
            if rest.starts_with(',') {
                rest = rest[1..].trim_start();
            } else {
                break;
            }
        }
        (fields, rest)
    } else {
        (vec![], s)
    };

    Ok((aggs, group_by_fields, s))
}

/// Parse one aggregation item: `aggfunc(b[.field]) AS alias`.
fn parse_one_aggregation(s: &str) -> Result<(WithAggregation, &str), GraphIrError> {
    let s = s.trim_start();
    // Match function name + opening paren.
    let (func, after_paren) = if s.len() >= 6 && s[..6].eq_ignore_ascii_case("count(") {
        (AggFunc::Count, &s[6..])
    } else if s.len() >= 4 && s[..4].eq_ignore_ascii_case("sum(") {
        (AggFunc::Sum, &s[4..])
    } else if s.len() >= 4 && s[..4].eq_ignore_ascii_case("avg(") {
        (AggFunc::Avg, &s[4..])
    } else if s.len() >= 4 && s[..4].eq_ignore_ascii_case("min(") {
        (AggFunc::Min, &s[4..])
    } else if s.len() >= 4 && s[..4].eq_ignore_ascii_case("max(") {
        (AggFunc::Max, &s[4..])
    } else {
        return Err(GraphIrError::Unsupported(format!(
            "expected aggregation function (count/sum/avg/min/max) near: {}",
            &s[..s.len().min(20)]
        )));
    };

    let after_paren = after_paren.trim_start();

    // Parse argument: b.field (for sum/avg/min/max) or b (for count).
    let (field, after_arg) = if after_paren.starts_with("b.") || after_paren.starts_with("B.") {
        let rest = &after_paren[2..];
        let field_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if field_end == 0 {
            return Err(GraphIrError::Unsupported(
                "expected field name after b. in aggregation argument".into(),
            ));
        }
        (Some(rest[..field_end].to_string()), &rest[field_end..])
    } else if after_paren.starts_with('b') || after_paren.starts_with('B') {
        (None, &after_paren[1..])
    } else {
        return Err(GraphIrError::Unsupported(
            "aggregation argument must be 'b' or 'b.field'".into(),
        ));
    };

    let after_arg = after_arg.trim_start();
    if !after_arg.starts_with(')') {
        return Err(GraphIrError::Unsupported(
            "expected ')' after aggregation argument".into(),
        ));
    }
    let after_close = after_arg[1..].trim_start();

    // Require field for sum/avg/min/max.
    if matches!(
        func,
        AggFunc::Sum | AggFunc::Avg | AggFunc::Min | AggFunc::Max
    ) && field.is_none()
    {
        return Err(GraphIrError::Unsupported(
            "sum/avg/min/max require a field argument, e.g. sum(b.score)".into(),
        ));
    }

    if !starts_with_keyword(after_close, "AS") {
        return Err(GraphIrError::Unsupported(
            "expected AS after aggregation function".into(),
        ));
    }
    let after_as = skip_keyword(after_close, "AS").trim_start();

    let alias_end = after_as
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(after_as.len());
    if alias_end == 0 {
        return Err(GraphIrError::Unsupported(
            "expected alias name after AS".into(),
        ));
    }
    let alias = after_as[..alias_end].to_string();
    let rest = &after_as[alias_end..];

    Ok((WithAggregation { func, field, alias }, rest))
}

/// Like `parse_ordering_tail` but returns the remaining unparsed string as the
/// fourth element.  Used when ORDER BY / SKIP / LIMIT must be followed by more
/// clauses (e.g. WITH followed by OPTIONAL MATCH or RETURN).
#[allow(clippy::type_complexity)]
fn parse_ordering_tail_rest(
    s: &str,
) -> Result<(Option<GraphIrOrderBy>, Option<usize>, Option<usize>, &str), GraphIrError> {
    let mut s = s.trim_start();
    let mut order_by = None;
    let mut limit = None;
    let mut skip = None;

    if starts_with_keyword(s, "ORDER") {
        let after_order = skip_keyword(s, "ORDER");
        if !starts_with_keyword(after_order, "BY") {
            return Err(GraphIrError::Unsupported("expected ORDER BY".into()));
        }
        let after_by = skip_keyword(after_order, "BY");
        let (field, rest) = parse_order_field(after_by)?;
        let rest = rest.trim_start();
        let (direction, rest) = if starts_with_keyword(rest, "DESC") {
            (OrderDirection::Desc, skip_keyword(rest, "DESC"))
        } else if starts_with_keyword(rest, "ASC") {
            (OrderDirection::Asc, skip_keyword(rest, "ASC"))
        } else {
            (OrderDirection::Asc, rest)
        };
        order_by = Some(GraphIrOrderBy { field, direction });
        s = rest.trim_start();
    }

    if starts_with_keyword(s, "SKIP") {
        let after_skip = skip_keyword(s, "SKIP");
        let (n, rest) = parse_usize_literal(after_skip)?;
        skip = Some(n);
        s = rest.trim_start();
    }

    if starts_with_keyword(s, "LIMIT") {
        let after_limit = skip_keyword(s, "LIMIT");
        let (n, rest) = parse_usize_literal(after_limit)?;
        limit = Some(n);
        s = rest.trim_start();
    }

    Ok((order_by, limit, skip, s))
}

fn parse_quoted(s: &str) -> Result<(String, &str), GraphIrError> {
    let s = s.trim_start();
    let quote = s
        .chars()
        .next()
        .ok_or_else(|| GraphIrError::Unsupported("expected quoted string".into()))?;
    if quote != '\'' && quote != '"' {
        return Err(GraphIrError::Unsupported("expected quoted string".into()));
    }
    let rest = &s[quote.len_utf8()..];
    let end = rest
        .find(quote)
        .ok_or_else(|| GraphIrError::Unsupported("unclosed string".into()))?;
    Ok((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

/// Split the RETURN spec from any trailing ORDER BY / SKIP / LIMIT clause.
/// Advances char by char and stops at the first keyword boundary for those tokens.
fn split_at_ordering_clause(s: &str) -> (&str, &str) {
    let mut i = 0;
    loop {
        let rest = &s[i..];
        if starts_with_keyword(rest, "ORDER")
            || starts_with_keyword(rest, "SKIP")
            || starts_with_keyword(rest, "LIMIT")
        {
            return (&s[..i], rest);
        }
        if i == s.len() {
            break;
        }
        i += s[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    (s, "")
}

#[allow(clippy::type_complexity)]
fn parse_ordering_tail(
    s: &str,
) -> Result<(Option<GraphIrOrderBy>, Option<usize>, Option<usize>), GraphIrError> {
    let (ob, limit, skip, _rest) = parse_ordering_tail_rest(s)?;
    Ok((ob, limit, skip))
}

fn parse_order_field(s: &str) -> Result<(OrderByField, &str), GraphIrError> {
    let s = s.trim_start();
    if s.len() >= 5 && s[..5].eq_ignore_ascii_case("id(b)") {
        return Ok((OrderByField::ObjectId, &s[5..]));
    }
    if starts_with_keyword(s, "depth") {
        return Ok((OrderByField::Depth, &s[5..]));
    }
    if s.starts_with("b.") || s.starts_with("B.") {
        let rest = &s[2..];
        let field_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if field_end == 0 {
            return Err(GraphIrError::Unsupported(
                "expected field name after ORDER BY b.".into(),
            ));
        }
        return Ok((
            OrderByField::PayloadField(rest[..field_end].to_string()),
            &rest[field_end..],
        ));
    }
    Err(GraphIrError::Unsupported(format!(
        "unknown ORDER BY target near: {}",
        &s[..s.len().min(20)]
    )))
}

fn parse_usize_literal(s: &str) -> Result<(usize, &str), GraphIrError> {
    let s = s.trim_start();
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        return Err(GraphIrError::Unsupported(
            "expected integer after SKIP/LIMIT".into(),
        ));
    }
    let n: usize = s[..end]
        .parse()
        .map_err(|_| GraphIrError::Unsupported("invalid integer after SKIP/LIMIT".into()))?;
    Ok((n, &s[end..]))
}

/// Evaluate a payload predicate against raw payload bytes.
/// Returns false if the payload is not valid UTF-8 JSON or lacks the field.
fn apply_payload_pred(payload: &[u8], pred: &PayloadPred) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let field_val = match v.get(&pred.field) {
        Some(fv) => fv,
        None => return false,
    };
    match &pred.value {
        CmpValue::Number(rhs) => {
            let lhs = match field_val.as_f64() {
                Some(n) => n,
                None => return false,
            };
            match pred.op {
                CmpOp::Eq => (lhs - rhs).abs() < f64::EPSILON,
                CmpOp::Ne => (lhs - rhs).abs() >= f64::EPSILON,
                CmpOp::Gt => lhs > *rhs,
                CmpOp::Lt => lhs < *rhs,
                CmpOp::Gte => lhs >= *rhs,
                CmpOp::Lte => lhs <= *rhs,
            }
        }
        CmpValue::Text(rhs) => {
            let lhs = match field_val.as_str() {
                Some(s) => s,
                None => return false,
            };
            match pred.op {
                CmpOp::Eq => lhs == rhs,
                CmpOp::Ne => lhs != rhs,
                CmpOp::Gt => lhs > rhs.as_str(),
                CmpOp::Lt => lhs < rhs.as_str(),
                CmpOp::Gte => lhs >= rhs.as_str(),
                CmpOp::Lte => lhs <= rhs.as_str(),
            }
        }
    }
}

/// Reconstruct the shortest-path sequence from `start` to `node` using the
/// parent map produced by BFS. Both endpoints are included.
fn reconstruct_path(
    start: &ObjectId,
    node: &ObjectId,
    parent_map: &HashMap<ObjectId, ObjectId>,
) -> Vec<ObjectId> {
    let mut path = vec![node.clone()];
    let mut current = node;
    while let Some(parent) = parent_map.get(current) {
        path.push(parent.clone());
        current = parent;
        if current == start {
            break;
        }
    }
    path.reverse();
    path
}

/// Build a `GraphQuery` from a `GraphIrQuery` (shared by `graph_ir_parsed` and
/// `graph_ir_projected`), factoring out the builder setup so it isn't duplicated.
fn build_graph_query<'a>(store: &'a BinderStore, q: &GraphIrQuery) -> GraphQuery<'a> {
    let mut builder = store
        .graph_query()
        .from(&q.start)
        .max_depth(q.max_depth)
        .direction(q.direction);

    if !q.edge_types.is_empty() {
        builder = builder.edge_types(q.edge_types.clone());
    }

    builder = match &q.end_pred {
        NodeIdPred::Any => builder,
        NodeIdPred::StartsWith(pfx) => {
            let pfx = pfx.clone();
            builder.filter_node(move |obj| obj.object_id.0.starts_with(&pfx))
        }
        NodeIdPred::Equals(id) => {
            let id = id.clone();
            builder.filter_node(move |obj| obj.object_id.0 == id)
        }
    };

    if !q.payload_preds.is_empty() {
        let preds = q.payload_preds.clone();
        builder = builder.filter_node(move |obj| {
            let payload = obj.payload.as_deref().unwrap_or(&[]);
            preds.iter().all(|pred| apply_payload_pred(payload, pred))
        });
    }

    builder
}

/// Parse the RETURN spec.
///
/// Returns `(b_fields, c_fields, return_path)` where:
/// - `b_fields`: fields projected from the mandatory `b` node (`RETURN b.f1, b.f2`)
/// - `c_fields`: fields projected from the optional `c` node (`RETURN c.f1`)
/// - `return_path`: true when `path` appears in the RETURN list
///
/// Plain `RETURN b` and `RETURN c` (variable name only) are valid but produce no fields.
fn parse_return_spec(s: &str) -> (Vec<String>, Vec<String>, bool) {
    let s = s.trim();
    if s.is_empty() {
        return (vec![], vec![], false);
    }
    let mut b_fields = Vec::new();
    let mut c_fields = Vec::new();
    let mut return_path = false;
    for part in s.split(',') {
        let p = part.trim();
        if p.eq_ignore_ascii_case("path") {
            return_path = true;
        } else if let Some(field) = p.strip_prefix("b.").or_else(|| p.strip_prefix("B.")) {
            let f = field.trim().to_string();
            if !f.is_empty() {
                b_fields.push(f);
            }
        } else if let Some(field) = p.strip_prefix("c.").or_else(|| p.strip_prefix("C.")) {
            let f = field.trim().to_string();
            if !f.is_empty() {
                c_fields.push(f);
            }
        }
        // "b", "c" alone → plain return; ignore
    }
    (b_fields, c_fields, return_path)
}

enum SortKey {
    Num(f64),
    Str(String),
    Missing,
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other) == Some(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use SortKey::*;
        match (self, other) {
            (Num(a), Num(b)) => a.partial_cmp(b),
            (Str(a), Str(b)) => Some(a.cmp(b)),
            (Num(_), Str(_)) => Some(std::cmp::Ordering::Less),
            (Str(_), Num(_)) => Some(std::cmp::Ordering::Greater),
            (Missing, Missing) => Some(std::cmp::Ordering::Equal),
            (_, Missing) => Some(std::cmp::Ordering::Less),
            (Missing, _) => Some(std::cmp::Ordering::Greater),
        }
    }
}

fn extract_sort_key(payload: Option<&[u8]>, field: &str) -> SortKey {
    let Some(bytes) = payload else {
        return SortKey::Missing;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return SortKey::Missing;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return SortKey::Missing;
    };
    match v.get(field) {
        Some(serde_json::Value::Number(n)) => {
            n.as_f64().map(SortKey::Num).unwrap_or(SortKey::Missing)
        }
        Some(serde_json::Value::String(s)) => SortKey::Str(s.clone()),
        _ => SortKey::Missing,
    }
}

fn apply_order_limit_matches(matches: &mut Vec<GraphQueryMatch>, q: &GraphIrQuery) {
    if let Some(ob) = &q.order_by {
        matches.sort_by(|a, b| {
            let ord = match &ob.field {
                OrderByField::ObjectId => a.object.object_id.0.cmp(&b.object.object_id.0),
                OrderByField::Depth => a.depth.cmp(&b.depth),
                OrderByField::PayloadField(field) => {
                    let ak = extract_sort_key(a.object.payload.as_deref(), field);
                    let bk = extract_sort_key(b.object.payload.as_deref(), field);
                    ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                }
            };
            if ob.direction == OrderDirection::Desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
    if let Some(n) = q.skip.filter(|&n| n > 0) {
        let drain_end = n.min(matches.len());
        matches.drain(..drain_end);
    }
    if let Some(limit) = q.limit {
        matches.truncate(limit);
    }
}

impl BinderStore {
    /// Execute a restricted MATCH / WHERE / RETURN graph IR string (issue #2).
    pub fn graph_ir(&self, query: &str) -> Result<Vec<GraphQueryMatch>, StoreError> {
        let q = GraphIrQuery::parse(query).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.graph_ir_parsed(&q)
    }

    /// Execute a pre-parsed restricted graph IR query.
    pub fn graph_ir_parsed(&self, q: &GraphIrQuery) -> Result<Vec<GraphQueryMatch>, StoreError> {
        let mut matches = build_graph_query(self, q).execute()?;
        if q.min_depth > 1 {
            matches.retain(|m| m.depth >= q.min_depth);
        }
        apply_order_limit_matches(&mut matches, q);
        Ok(matches)
    }

    /// Execute a pre-parsed query with field projections and/or path collection.
    ///
    /// - `RETURN b.f1, b.f2` → `GraphIrRow::fields` contains the projected JSON fields.
    /// - `RETURN path` → `GraphIrRow::path` contains the node-id sequence from start to match.
    /// - Both may be combined: `RETURN b.sentiment, path`.
    /// - `OPTIONAL MATCH` → each mandatory result is expanded with optional results; rows with
    ///   no optional match have `GraphIrRow::optional = None`.
    ///
    /// Fields absent from the payload or non-JSON payloads produce no entry for that key.
    /// For plain `RETURN b` with no optional clause, prefer `graph_ir_parsed`.
    pub fn graph_ir_projected(&self, q: &GraphIrQuery) -> Result<Vec<GraphIrRow>, StoreError> {
        let (mut matches, parent_map) = if q.return_path {
            let (mut ms, pm) = self.execute_graph_query_with_paths(build_graph_query(self, q))?;
            if q.min_depth > 1 {
                ms.retain(|m| m.depth >= q.min_depth);
            }
            (ms, pm)
        } else {
            let mut ms = build_graph_query(self, q).execute()?;
            if q.min_depth > 1 {
                ms.retain(|m| m.depth >= q.min_depth);
            }
            (ms, HashMap::new())
        };

        // Pre-expansion ordering: WITH clause controls mandatory results.
        // When no WITH clause, q.order_by/limit/skip apply pre-expansion (backward compat).
        if let Some(wc) = &q.with_clause {
            apply_with_clause(&mut matches, wc);
        } else {
            apply_order_limit_matches(&mut matches, q);
        }

        let start = &q.start;
        let mut rows: Vec<GraphIrRow> = Vec::with_capacity(matches.len());

        for m in matches {
            let mut b_fields = HashMap::new();
            if !q.return_fields.is_empty() {
                if let Some(payload) = m.object.payload.as_deref() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                            for key in &q.return_fields {
                                if let Some(val) = v.get(key) {
                                    b_fields.insert(key.clone(), val.clone());
                                }
                            }
                        }
                    }
                }
            }
            let path = if q.return_path {
                reconstruct_path(start, &m.object.object_id, &parent_map)
            } else {
                vec![]
            };

            if let Some(opt_clause) = &q.optional_clause {
                let opt_results = execute_optional_clause(self, &m.object.object_id, opt_clause)?;
                if opt_results.is_empty() {
                    rows.push(GraphIrRow {
                        object_id: m.object.object_id,
                        depth: m.depth,
                        fields: b_fields,
                        path,
                        optional: None,
                    });
                } else {
                    for opt in opt_results {
                        rows.push(GraphIrRow {
                            object_id: m.object.object_id.clone(),
                            depth: m.depth,
                            fields: b_fields.clone(),
                            path: path.clone(),
                            optional: Some(opt),
                        });
                    }
                }
            } else {
                rows.push(GraphIrRow {
                    object_id: m.object.object_id,
                    depth: m.depth,
                    fields: b_fields,
                    path,
                    optional: None,
                });
            }
        }

        // Post-expansion ordering: applied to final rows only when a WITH clause
        // was present (WITH owns the pre-expansion pass; RETURN owns the post-expansion pass).
        if q.with_clause.is_some() {
            apply_order_limit_rows(&mut rows, q);
        }

        Ok(rows)
    }
}

/// Apply WITH clause predicates, ordering, and paging to mandatory `b` results.
fn apply_with_clause(matches: &mut Vec<GraphQueryMatch>, wc: &GraphIrWithClause) {
    match &wc.end_pred {
        NodeIdPred::Any => {}
        NodeIdPred::StartsWith(pfx) => {
            matches.retain(|m| m.object.object_id.0.starts_with(pfx.as_str()));
        }
        NodeIdPred::Equals(id) => {
            matches.retain(|m| m.object.object_id.0 == id.as_str());
        }
    }
    if !wc.payload_preds.is_empty() {
        matches.retain(|m| {
            let payload = m.object.payload.as_deref().unwrap_or(&[]);
            wc.payload_preds
                .iter()
                .all(|pred| apply_payload_pred(payload, pred))
        });
    }
    if let Some(ob) = &wc.order_by {
        matches.sort_by(|a, b| {
            let ord = match &ob.field {
                OrderByField::ObjectId => a.object.object_id.0.cmp(&b.object.object_id.0),
                OrderByField::Depth => a.depth.cmp(&b.depth),
                OrderByField::PayloadField(field) => {
                    let ak = extract_sort_key(a.object.payload.as_deref(), field);
                    let bk = extract_sort_key(b.object.payload.as_deref(), field);
                    ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                }
            };
            if ob.direction == OrderDirection::Desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
    if let Some(n) = wc.skip.filter(|&n| n > 0) {
        matches.drain(..n.min(matches.len()));
    }
    if let Some(limit) = wc.limit {
        matches.truncate(limit);
    }
}

fn sort_key_from_value(v: Option<&serde_json::Value>) -> SortKey {
    match v {
        None => SortKey::Missing,
        Some(serde_json::Value::Number(n)) => {
            n.as_f64().map(SortKey::Num).unwrap_or(SortKey::Missing)
        }
        Some(serde_json::Value::String(s)) => SortKey::Str(s.clone()),
        _ => SortKey::Missing,
    }
}

/// Apply ORDER BY / SKIP / LIMIT to final expanded `GraphIrRow` results.
/// Used post-optional-expansion when a WITH clause controls pre-expansion ordering.
fn apply_order_limit_rows(rows: &mut Vec<GraphIrRow>, q: &GraphIrQuery) {
    if let Some(ob) = &q.order_by {
        rows.sort_by(|a, b| {
            let ord = match &ob.field {
                OrderByField::ObjectId => a.object_id.0.cmp(&b.object_id.0),
                OrderByField::Depth => a.depth.cmp(&b.depth),
                OrderByField::PayloadField(field) => {
                    let ak = sort_key_from_value(a.fields.get(field));
                    let bk = sort_key_from_value(b.fields.get(field));
                    ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                }
            };
            if ob.direction == OrderDirection::Desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }
    if let Some(n) = q.skip.filter(|&n| n > 0) {
        rows.drain(..n.min(rows.len()));
    }
    if let Some(limit) = q.limit {
        rows.truncate(limit);
    }
}

/// Execute an OPTIONAL MATCH clause starting from `start`, returning all matched
/// `c` nodes with projected fields. Returns an empty Vec when no c nodes are
/// reachable — the caller emits a null-padded row in that case.
fn execute_optional_clause(
    store: &BinderStore,
    start: &ObjectId,
    clause: &GraphIrOptionalClause,
) -> Result<Vec<GraphIrOptionalMatch>, StoreError> {
    let mut builder = store
        .graph_query()
        .from(start)
        .max_depth(clause.max_depth)
        .direction(clause.direction);

    if !clause.edge_types.is_empty() {
        builder = builder.edge_types(clause.edge_types.clone());
    }

    builder = match &clause.end_pred {
        NodeIdPred::Any => builder,
        NodeIdPred::StartsWith(pfx) => {
            let pfx = pfx.clone();
            builder.filter_node(move |obj| obj.object_id.0.starts_with(&pfx))
        }
        NodeIdPred::Equals(id) => {
            let id = id.clone();
            builder.filter_node(move |obj| obj.object_id.0 == id)
        }
    };

    if !clause.payload_preds.is_empty() {
        let preds = clause.payload_preds.clone();
        builder = builder.filter_node(move |obj| {
            let payload = obj.payload.as_deref().unwrap_or(&[]);
            preds.iter().all(|pred| apply_payload_pred(payload, pred))
        });
    }

    let mut matches = builder.execute()?;
    if clause.min_depth > 1 {
        matches.retain(|m| m.depth >= clause.min_depth);
    }

    let results = matches
        .into_iter()
        .map(|m| {
            let mut fields = HashMap::new();
            if !clause.return_fields.is_empty() {
                if let Some(payload) = m.object.payload.as_deref() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                            for key in &clause.return_fields {
                                if let Some(val) = v.get(key) {
                                    fields.insert(key.clone(), val.clone());
                                }
                            }
                        }
                    }
                }
            }
            GraphIrOptionalMatch {
                object_id: m.object.object_id,
                depth: m.depth,
                fields,
            }
        })
        .collect();

    Ok(results)
}

// ── aggregation helpers ───────────────────────────────────────────────────────

/// Apply only the filter portion of a WITH clause (id predicate + payload preds).
/// Used by `graph_ir_aggregated` before computing aggregate values.
fn apply_with_clause_filter(matches: &mut Vec<GraphQueryMatch>, wc: &GraphIrWithClause) {
    match &wc.end_pred {
        NodeIdPred::Any => {}
        NodeIdPred::StartsWith(pfx) => {
            matches.retain(|m| m.object.object_id.0.starts_with(pfx.as_str()));
        }
        NodeIdPred::Equals(id) => {
            matches.retain(|m| m.object.object_id.0 == id.as_str());
        }
    }
    if !wc.payload_preds.is_empty() {
        matches.retain(|m| {
            let payload = m.object.payload.as_deref().unwrap_or(&[]);
            wc.payload_preds
                .iter()
                .all(|pred| apply_payload_pred(payload, pred))
        });
    }
}

/// Extract a numeric field value from a JSON payload.
fn extract_numeric_field(payload: Option<&[u8]>, field: &str) -> Option<f64> {
    let bytes = payload?;
    let text = std::str::from_utf8(bytes).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get(field)?.as_f64()
}

/// Compute a single aggregation over the matched nodes.
fn compute_aggregation(matches: &[GraphQueryMatch], agg: &WithAggregation) -> serde_json::Value {
    match &agg.func {
        AggFunc::Count => serde_json::Value::Number(serde_json::Number::from(matches.len() as u64)),
        AggFunc::Sum => {
            let field = agg.field.as_deref().unwrap_or("");
            let sum: f64 = matches
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .sum();
            serde_json::json!(sum)
        }
        AggFunc::Avg => {
            let field = agg.field.as_deref().unwrap_or("");
            let values: Vec<f64> = matches
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .collect();
            if values.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(values.iter().sum::<f64>() / values.len() as f64)
            }
        }
        AggFunc::Min => {
            let field = agg.field.as_deref().unwrap_or("");
            matches
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .reduce(f64::min)
                .map(|v| serde_json::json!(v))
                .unwrap_or(serde_json::Value::Null)
        }
        AggFunc::Max => {
            let field = agg.field.as_deref().unwrap_or("");
            matches
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .reduce(f64::max)
                .map(|v| serde_json::json!(v))
                .unwrap_or(serde_json::Value::Null)
        }
    }
}

/// Extract a single JSON field as a `serde_json::Value` (Null when absent/non-JSON).
fn extract_json_field_value(payload: Option<&[u8]>, field: &str) -> serde_json::Value {
    let key_str = json_field_as_group_key(payload, field);
    serde_json::from_str(&key_str).unwrap_or(serde_json::Value::Null)
}

/// Serialize a node's JSON payload field to a stable string key for grouping.
/// Returns `"null"` when the payload is missing, non-JSON, or lacks the field.
fn json_field_as_group_key(payload: Option<&[u8]>, field: &str) -> String {
    let Some(bytes) = payload else {
        return "null".into();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return "null".into();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return "null".into();
    };
    match v.get(field) {
        Some(val) => serde_json::to_string(val).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    }
}

/// Build a stable composite group key for all `fields` from a JSON payload.
/// Single field: raw JSON-serialized value. Multiple fields: JSON array `["v1","v2"]`.
fn composite_group_key(payload: Option<&[u8]>, fields: &[String]) -> String {
    if fields.len() == 1 {
        return json_field_as_group_key(payload, &fields[0]);
    }
    let vals: Vec<serde_json::Value> = fields
        .iter()
        .map(|f| extract_json_field_value(payload, f))
        .collect();
    serde_json::to_string(&vals).unwrap_or_else(|_| "null".into())
}

/// Like `compute_aggregation` but operates on a borrowed slice of match references.
fn compute_aggregation_slice(
    group: &[&GraphQueryMatch],
    agg: &WithAggregation,
) -> serde_json::Value {
    match &agg.func {
        AggFunc::Count => serde_json::Value::Number(serde_json::Number::from(group.len() as u64)),
        AggFunc::Sum => {
            let field = agg.field.as_deref().unwrap_or("");
            let sum: f64 = group
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .sum();
            serde_json::json!(sum)
        }
        AggFunc::Avg => {
            let field = agg.field.as_deref().unwrap_or("");
            let values: Vec<f64> = group
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .collect();
            if values.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::json!(values.iter().sum::<f64>() / values.len() as f64)
            }
        }
        AggFunc::Min => {
            let field = agg.field.as_deref().unwrap_or("");
            group
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .reduce(f64::min)
                .map(|v| serde_json::json!(v))
                .unwrap_or(serde_json::Value::Null)
        }
        AggFunc::Max => {
            let field = agg.field.as_deref().unwrap_or("");
            group
                .iter()
                .filter_map(|m| extract_numeric_field(m.object.payload.as_deref(), field))
                .reduce(f64::max)
                .map(|v| serde_json::json!(v))
                .unwrap_or(serde_json::Value::Null)
        }
    }
}

// ── write IR (issue #2 — write clauses) ──────────────────────────────────────

/// A single graph mutation in the write IR.
#[derive(Debug, Clone)]
pub enum WriteOp {
    /// Unconditionally upsert a node (put_object).
    CreateNode { id: ObjectId, payload: Vec<u8> },
    /// Upsert a node only if it does not already exist.
    MergeNode { id: ObjectId, payload: Vec<u8> },
    /// Unconditionally create or update an edge.
    CreateEdge {
        id: EdgeId,
        from: ObjectId,
        to: ObjectId,
        label: String,
        payload: Option<Vec<u8>>,
    },
    /// Create an edge only if it does not already exist (keyed by edge id).
    MergeEdge {
        id: EdgeId,
        from: ObjectId,
        to: ObjectId,
        label: String,
        payload: Option<Vec<u8>>,
    },
}

/// Parse error for the write IR.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphIrWriteError {
    #[error("unknown verb on line {line}: expected CREATE or MERGE")]
    UnknownVerb { line: usize },
    #[error("unknown target on line {line}: expected NODE or EDGE")]
    UnknownTarget { line: usize },
    #[error("missing field '{field}' on line {line}")]
    MissingField { field: &'static str, line: usize },
    #[error("invalid JSON payload on line {line}: {reason}")]
    InvalidPayload { line: usize, reason: String },
}

/// Parsed write IR — one or more graph mutation operations.
///
/// Grammar (one operation per non-empty line):
/// ```text
/// CREATE NODE id="<object-id>" payload=<json-object>
/// MERGE  NODE id="<object-id>" payload=<json-object>
/// CREATE EDGE id="<edge-id>" from="<oid>" to="<oid>" label="<str>" [payload=<json>]
/// MERGE  EDGE id="<edge-id>" from="<oid>" to="<oid>" label="<str>" [payload=<json>]
/// ```
///
/// Lines starting with `//` or `#` are treated as comments and ignored.
/// All operations in a single `GraphIrWrite` are committed as one transaction.
#[derive(Debug, Clone)]
pub struct GraphIrWrite {
    pub operations: Vec<WriteOp>,
}

/// Extract the value of a `key="value"` pair from a KV string.
fn extract_quoted_value(s: &str, key: &str) -> Option<String> {
    let pattern = format!("{}=\"", key);
    let start = s.find(pattern.as_str())? + pattern.len();
    let end = s[start..].find('"')?;
    Some(s[start..start + end].to_string())
}

/// Extract a JSON value starting at `key=<json>`, consuming just the JSON token.
/// Uses serde_json's streaming deserializer so trailing KV pairs are ignored.
fn extract_json_value(s: &str, key: &str) -> Option<serde_json::Value> {
    let pattern = format!("{}=", key);
    let start = s.find(pattern.as_str())? + pattern.len();
    let mut de = serde_json::Deserializer::from_str(&s[start..]);
    serde::Deserialize::deserialize(&mut de).ok()
}

impl GraphIrWrite {
    /// Parse a write IR string into a list of operations.
    pub fn parse(s: &str) -> Result<Self, GraphIrWriteError> {
        let mut operations = Vec::new();
        for (line_idx, raw) in s.lines().enumerate() {
            let line = line_idx + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            // Split on up to 3 whitespace-delimited tokens: VERB TARGET rest...
            let mut parts = trimmed.splitn(3, ' ');
            let verb = parts.next().unwrap_or("").to_uppercase();
            let target = parts.next().unwrap_or("").to_uppercase();
            let rest = parts.next().unwrap_or("").trim();

            let is_create = match verb.as_str() {
                "CREATE" => true,
                "MERGE" => false,
                _ => return Err(GraphIrWriteError::UnknownVerb { line }),
            };

            match target.as_str() {
                "NODE" => {
                    let id_str = extract_quoted_value(rest, "id")
                        .ok_or(GraphIrWriteError::MissingField { field: "id", line })?;
                    let json = extract_json_value(rest, "payload").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "payload",
                            line,
                        },
                    )?;
                    let payload = serde_json::to_vec(&json).map_err(|e| {
                        GraphIrWriteError::InvalidPayload {
                            line,
                            reason: e.to_string(),
                        }
                    })?;
                    let op = if is_create {
                        WriteOp::CreateNode {
                            id: ObjectId::new(id_str),
                            payload,
                        }
                    } else {
                        WriteOp::MergeNode {
                            id: ObjectId::new(id_str),
                            payload,
                        }
                    };
                    operations.push(op);
                }
                "EDGE" => {
                    let id_str = extract_quoted_value(rest, "id")
                        .ok_or(GraphIrWriteError::MissingField { field: "id", line })?;
                    let from_str = extract_quoted_value(rest, "from").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "from",
                            line,
                        },
                    )?;
                    let to_str = extract_quoted_value(rest, "to")
                        .ok_or(GraphIrWriteError::MissingField { field: "to", line })?;
                    let label = extract_quoted_value(rest, "label").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "label",
                            line,
                        },
                    )?;
                    let payload = extract_json_value(rest, "payload")
                        .and_then(|v| serde_json::to_vec(&v).ok());
                    let op = if is_create {
                        WriteOp::CreateEdge {
                            id: EdgeId::new(id_str),
                            from: ObjectId::new(from_str),
                            to: ObjectId::new(to_str),
                            label,
                            payload,
                        }
                    } else {
                        WriteOp::MergeEdge {
                            id: EdgeId::new(id_str),
                            from: ObjectId::new(from_str),
                            to: ObjectId::new(to_str),
                            label,
                            payload,
                        }
                    };
                    operations.push(op);
                }
                _ => return Err(GraphIrWriteError::UnknownTarget { line }),
            }
        }
        Ok(GraphIrWrite { operations })
    }
}

impl BinderStore {
    /// Parse and execute a write IR string as a single committed transaction.
    ///
    /// Returns `None` if all MERGE operations found existing objects/edges and
    /// nothing was written. Returns `Some(commit_id)` on a successful write.
    pub fn graph_ir_write(&self, s: &str) -> Result<Option<CommitId>, StoreError> {
        let write = GraphIrWrite::parse(s).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.graph_ir_write_parsed(&write)
    }

    /// Execute a pre-parsed write IR.
    pub fn graph_ir_write_parsed(
        &self,
        write: &GraphIrWrite,
    ) -> Result<Option<CommitId>, StoreError> {
        let mut tx = Transaction::new();
        let mut any = false;

        for op in &write.operations {
            match op {
                WriteOp::CreateNode { id, payload } => {
                    tx.put_object(id.clone(), payload.clone());
                    any = true;
                }
                WriteOp::MergeNode { id, payload } => {
                    if self.get(id)?.is_none() {
                        tx.put_object(id.clone(), payload.clone());
                        any = true;
                    }
                }
                WriteOp::CreateEdge {
                    id,
                    from,
                    to,
                    label,
                    payload,
                } => {
                    tx.put_edge(id.clone(), from.clone(), to.clone(), label, payload.clone());
                    any = true;
                }
                WriteOp::MergeEdge {
                    id,
                    from,
                    to,
                    label,
                    payload,
                } => {
                    if self.edge_history(id)?.is_empty() {
                        tx.put_edge(id.clone(), from.clone(), to.clone(), label, payload.clone());
                        any = true;
                    }
                }
            }
        }

        if !any {
            return Ok(None);
        }
        Ok(Some(self.commit(tx, "graph_ir_write", "")?))
    }

    /// Execute a graph IR query that has an aggregating WITH clause.
    ///
    /// Returns one row per group (or one row total when no GROUP BY is present).
    /// Each row is a `HashMap<alias, value>`. When a `GROUP BY b.field` clause is
    /// present the group field value is included under its bare field name.
    ///
    /// ```text
    /// -- Ungrouped (returns Vec with one row):
    /// MATCH (a)-[:KNOWS*1..3]->(b)
    /// WHERE id(a) = 'person://alice'
    /// WITH count(b) AS total, avg(b.score) AS avg_score
    /// RETURN total, avg_score
    ///
    /// -- Grouped (returns Vec with one row per unique value of b.type):
    /// MATCH (a)-[:KNOWS*1..3]->(b)
    /// WHERE id(a) = 'person://alice'
    /// WITH count(b) AS cnt GROUP BY b.type
    /// RETURN b.type, cnt
    /// ```
    ///
    /// - `count(b)` always returns a JSON integer.
    /// - `sum(b.field)` returns 0.0 when no nodes contribute.
    /// - `avg/min/max(b.field)` return JSON null when no nodes contribute.
    /// - Nodes missing the GROUP BY field form a group whose key is JSON null.
    ///
    /// Use [`graph_ir_projected`] for non-aggregating queries.
    pub fn graph_ir_aggregated(
        &self,
        query: &str,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>, StoreError> {
        let q = GraphIrQuery::parse(query).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;

        let with_clause = q
            .with_clause
            .as_ref()
            .ok_or_else(|| StoreError::InvalidArgument {
                message: GraphIrError::NoAggregation.to_string(),
            })?;

        if with_clause.aggregations.is_empty() {
            return Err(StoreError::InvalidArgument {
                message: GraphIrError::NoAggregation.to_string(),
            });
        }

        let mut matches = build_graph_query(self, &q).execute()?;
        if q.min_depth > 1 {
            matches.retain(|m| m.depth >= q.min_depth);
        }
        apply_with_clause_filter(&mut matches, with_clause);

        if !with_clause.group_by_fields.is_empty() {
            // Grouped: partition matches by the composite key of all GROUP BY fields,
            // preserving insertion order of unique key combinations.
            let fields = &with_clause.group_by_fields;
            let mut group_keys: Vec<String> = Vec::new();
            let mut group_indices: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();

            for (i, m) in matches.iter().enumerate() {
                let key = composite_group_key(m.object.payload.as_deref(), fields);
                if !group_indices.contains_key(&key) {
                    group_keys.push(key.clone());
                }
                group_indices.entry(key).or_default().push(i);
            }

            let mut rows = Vec::with_capacity(group_keys.len());
            for key in &group_keys {
                let indices = &group_indices[key];
                let group: Vec<&GraphQueryMatch> = indices.iter().map(|&i| &matches[i]).collect();

                let mut row = std::collections::HashMap::new();
                // Re-extract each individual field value from the first member of
                // the group and include it under the bare field name.
                if let Some(&first_idx) = indices.first() {
                    let payload = matches[first_idx].object.payload.as_deref();
                    for field in fields {
                        let val = extract_json_field_value(payload, field);
                        row.insert(field.clone(), val);
                    }
                }
                for agg in &with_clause.aggregations {
                    row.insert(agg.alias.clone(), compute_aggregation_slice(&group, agg));
                }
                rows.push(row);
            }
            Ok(rows)
        } else {
            // Ungrouped: single output row.
            let mut row = std::collections::HashMap::new();
            for agg in &with_clause.aggregations {
                row.insert(agg.alias.clone(), compute_aggregation(&matches, agg));
            }
            Ok(vec![row])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
