//! Restricted graph query IR (issue #2, first slice).
//!
//! This is **not** Cypher. It parses a tiny MATCH dialect and maps it onto
//! [`GraphQuery`]. Variable-length min hops, edge predicates beyond type lists,
//! path collection, and RETURN projections remain future work.
//!
//! Supported forms:
//! ```text
//! MATCH (a)-[:KNOWS|RELATES_TO*1..3]->(b)
//! WHERE id(a) = 'person://alice' AND id(b) STARTS WITH 'memory://'
//! RETURN b
//!
//! MATCH (a)-[*2]->(b) WHERE id(a) = 'x' RETURN b
//! MATCH (a)<-[:KNOWS*1..2]-(b) WHERE id(a) = 'memory://m1' RETURN b
//! MATCH (a)-[:KNOWS]-(b) WHERE id(a) = 'x' RETURN b
//! ```
//!
//! Notes:
//! - `*min..max` / `*n` set **max_depth** only (the builder has no min-depth filter).
//! - Node filters apply to **results**, not traversal (same as `filter_node`).
//! - Undirected `-[]-` → [`Direction::Both`]; `<-[]-` → Incoming; `-[]->` → Outgoing.

use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::graph_query::{Direction, GraphQueryMatch};
use crate::types::ObjectId;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphIrQuery {
    pub start: ObjectId,
    pub edge_types: Vec<String>,
    /// Max hops from start (builder max_depth).
    pub max_depth: usize,
    pub direction: Direction,
    pub end_pred: NodeIdPred,
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

        let (direction, edge_types, max_depth, after_pattern) = parse_pattern(after_match)?;
        let after_pattern = after_pattern.trim_start();

        if !starts_with_keyword(after_pattern, "WHERE") {
            return Err(GraphIrError::Unsupported("expected WHERE".into()));
        }
        let after_where = skip_keyword(after_pattern, "WHERE");
        let (start, end_pred, after_preds) = parse_where(after_where)?;
        let after_preds = after_preds.trim_start();

        if !starts_with_keyword(after_preds, "RETURN") {
            return Err(GraphIrError::Unsupported("expected RETURN".into()));
        }
        // RETURN clause is accepted but ignored (always return matched nodes).
        let _ = skip_keyword(after_preds, "RETURN");

        Ok(GraphIrQuery {
            start,
            edge_types,
            max_depth,
            direction,
            end_pred,
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
fn parse_pattern(s: &str) -> Result<(Direction, Vec<String>, usize, &str), GraphIrError> {
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

    let (edge_types, max_depth) = parse_rel_body(rel_body)?;

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
    Ok((direction, edge_types, max_depth, rest))
}

fn parse_rel_body(body: &str) -> Result<(Vec<String>, usize), GraphIrError> {
    // Forms: "", "*2", "*1..3", ":KNOWS", ":KNOWS|TAGGED", ":KNOWS*1..3", ":A|B*2"
    let body = body.trim();
    if body.is_empty() {
        return Ok((Vec::new(), 6));
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

    let max_depth = match hops_part {
        None => 6,
        Some(h) => parse_hops(h.trim())?,
    };
    Ok((edge_types, max_depth))
}

fn parse_hops(h: &str) -> Result<usize, GraphIrError> {
    if let Some((a, b)) = h.split_once("..") {
        let _min: usize = a.trim().parse().map_err(|_| GraphIrError::BadHops)?;
        let max: usize = b.trim().parse().map_err(|_| GraphIrError::BadHops)?;
        if max == 0 {
            return Err(GraphIrError::BadHops);
        }
        // Min is accepted for Cypher familiarity but not enforced by the builder.
        Ok(max)
    } else {
        let n: usize = h.parse().map_err(|_| GraphIrError::BadHops)?;
        if n == 0 {
            return Err(GraphIrError::BadHops);
        }
        Ok(n)
    }
}

fn parse_where(s: &str) -> Result<(ObjectId, NodeIdPred, &str), GraphIrError> {
    let s = s.trim_start();
    // id(a) = '…'  [AND id(b) STARTS WITH '…' | AND id(b) = '…']
    let (start, rest) = parse_id_eq(s, "a")?;
    let rest = rest.trim_start();
    if starts_with_keyword(rest, "AND") {
        let after_and = skip_keyword(rest, "AND");
        let (end_pred, rest2) = parse_end_pred(after_and)?;
        Ok((start, end_pred, rest2))
    } else {
        Ok((start, NodeIdPred::Any, rest))
    }
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
        let mut builder = self
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

        builder.execute()
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
        assert_eq!(q.max_depth, 3);
        assert_eq!(q.direction, Direction::Outgoing);
        assert_eq!(q.end_pred, NodeIdPred::StartsWith("memory://".into()));
    }

    #[test]
    fn parses_undirected() {
        let q = GraphIrQuery::parse("MATCH (a)-[:KNOWS]-(b) WHERE id(a) = 'memory://m1' RETURN b")
            .unwrap();
        assert_eq!(q.direction, Direction::Both);
        assert_eq!(q.max_depth, 6);
    }

    #[test]
    fn parses_incoming() {
        let q = GraphIrQuery::parse(
            "MATCH (a)<-[:KNOWS*1..2]-(b) WHERE id(a) = 'memory://m1' RETURN b",
        )
        .unwrap();
        assert_eq!(q.direction, Direction::Incoming);
        assert_eq!(q.max_depth, 2);
    }

    #[test]
    fn rejects_cypher_extras() {
        assert!(GraphIrQuery::parse(
            "MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'x' AND b.sentiment > 0.8 RETURN b"
        )
        .is_err());
    }
}
