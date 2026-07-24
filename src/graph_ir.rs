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
//! -- Field projections (payload must be a UTF-8 JSON object):
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! RETURN b.sentiment
//!
//! MATCH (a)-[:RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice'
//! RETURN b.sentiment, b.label
//! ```
//!
//! Payload predicate operators: `>`, `<`, `>=`, `<=`, `=`, `!=`.
//! The right-hand side may be a number literal or a quoted string.
//! String comparison uses lexicographic ordering.
//!
//! `RETURN b` returns matched node ids (`graph_ir` / `graph_ir_parsed`).
//! `RETURN b.field …` returns projected rows via `graph_ir_projected`.
//!
//! Notes:
//! - `*min..max` / `*n` set max_depth and filter results to `depth >= min`.
//! - Node filters apply to **results**, not traversal.
//! - Undirected `-[]-` → [`Direction::Both`]; `<-[]-` → Incoming; `-[]->` → Outgoing.

use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::graph_query::{Direction, GraphQuery, GraphQueryMatch};
use crate::types::ObjectId;
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

/// One row returned by `graph_ir_projected`.
///
/// `fields` contains only the keys requested in `RETURN b.field1, b.field2`.
/// Missing or non-JSON payloads produce no entries for that key.
///
/// `path` is the sequence of node IDs from the start node to `object_id`
/// (inclusive of both endpoints) when `RETURN path` was requested; otherwise empty.
#[derive(Debug, Clone)]
pub struct GraphIrRow {
    pub object_id: ObjectId,
    pub depth: usize,
    pub fields: HashMap<String, serde_json::Value>,
    pub path: Vec<ObjectId>,
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
    /// Optional payload field predicate applied to matched end nodes.
    pub payload_pred: Option<PayloadPred>,
    /// Fields to project from each matched node's JSON payload (`RETURN b.f1, b.f2`).
    /// Empty = plain node return via `graph_ir` / `graph_ir_parsed`.
    pub return_fields: Vec<String>,
    /// When true, `graph_ir_projected` populates `GraphIrRow::path` with the
    /// sequence of node IDs from the start node to each matched node.
    pub return_path: bool,
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

        // Optional payload predicate: AND b.FIELD OP VALUE
        let (payload_pred, after_preds2) = if starts_with_keyword(after_preds, "AND") {
            let after_and = skip_keyword(after_preds, "AND");
            if after_and.trim_start().starts_with("b.") || after_and.trim_start().starts_with("B.")
            {
                let (pred, rest) = parse_payload_pred(after_and.trim_start())?;
                (Some(pred), rest.trim_start())
            } else {
                (None, after_preds)
            }
        } else {
            (None, after_preds)
        };

        if !starts_with_keyword(after_preds2, "RETURN") {
            return Err(GraphIrError::Unsupported("expected RETURN".into()));
        }
        let after_return = skip_keyword(after_preds2, "RETURN");
        let (return_fields, return_path) = parse_return_spec(after_return);

        Ok(GraphIrQuery {
            start,
            edge_types,
            min_depth,
            max_depth,
            direction,
            end_pred,
            payload_pred,
            return_fields,
            return_path,
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

fn parse_payload_pred(s: &str) -> Result<(PayloadPred, &str), GraphIrError> {
    // s starts with "b." or "B."
    let s = &s[2..]; // skip "b."
                     // field name: alphanumeric + underscore
    let field_end = s
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if field_end == 0 {
        return Err(GraphIrError::Unsupported(
            "expected field name after b.".into(),
        ));
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
            "expected comparison operator after b.{field}"
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

    if let Some(pred) = &q.payload_pred {
        let pred = pred.clone();
        builder = builder.filter_node(move |obj| {
            apply_payload_pred(obj.payload.as_deref().unwrap_or(&[]), &pred)
        });
    }

    builder
}

/// Parse `RETURN b` → `([], false)`,
/// `RETURN b.f1, b.f2` → `(["f1", "f2"], false)`,
/// `RETURN path` → `([], true)`,
/// `RETURN b.sentiment, path` → `(["sentiment"], true)`.
fn parse_return_spec(s: &str) -> (Vec<String>, bool) {
    let s = s.trim();
    if s.is_empty() {
        return (vec![], false);
    }
    let mut fields = Vec::new();
    let mut return_path = false;
    for part in s.split(',') {
        let p = part.trim();
        if p.eq_ignore_ascii_case("path") {
            return_path = true;
        } else if let Some(field) = p.strip_prefix("b.").or_else(|| p.strip_prefix("B.")) {
            let f = field.trim().to_string();
            if !f.is_empty() {
                fields.push(f);
            }
        }
        // "b" alone → plain return; ignore
    }
    (fields, return_path)
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
        Ok(matches)
    }

    /// Execute a pre-parsed query with field projections and/or path collection.
    ///
    /// - `RETURN b.f1, b.f2` → `GraphIrRow::fields` contains the projected JSON fields.
    /// - `RETURN path` → `GraphIrRow::path` contains the node-id sequence from start to match.
    /// - Both may be combined: `RETURN b.sentiment, path`.
    ///
    /// Fields absent from the payload or non-JSON payloads produce no entry for that key.
    /// For plain `RETURN b`, prefer `graph_ir_parsed` which avoids the overhead of path tracking.
    pub fn graph_ir_projected(&self, q: &GraphIrQuery) -> Result<Vec<GraphIrRow>, StoreError> {
        let (matches, parent_map) = if q.return_path {
            self.execute_graph_query_with_paths(build_graph_query(self, q))?
        } else {
            (self.graph_ir_parsed(q)?, HashMap::new())
        };

        let start = &q.start;
        let rows = matches
            .into_iter()
            .map(|m| {
                let mut fields = HashMap::new();
                if !q.return_fields.is_empty() {
                    if let Some(payload) = m.object.payload.as_deref() {
                        if let Ok(text) = std::str::from_utf8(payload) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                for key in &q.return_fields {
                                    if let Some(val) = v.get(key) {
                                        fields.insert(key.clone(), val.clone());
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
                GraphIrRow {
                    object_id: m.object.object_id.clone(),
                    depth: m.depth,
                    fields,
                    path,
                }
            })
            .collect();
        Ok(rows)
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
        let pred = q.payload_pred.unwrap();
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
        let pred = q.payload_pred.unwrap();
        assert_eq!(pred.field, "status");
        assert_eq!(pred.op, CmpOp::Eq);
        assert_eq!(pred.value, CmpValue::Text("active".into()));
    }

    #[test]
    fn no_payload_pred_when_absent() {
        let q = GraphIrQuery::parse("MATCH (a)-[]->(b) WHERE id(a) = 'x' RETURN b").unwrap();
        assert!(q.payload_pred.is_none());
        assert!(q.return_fields.is_empty());
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
