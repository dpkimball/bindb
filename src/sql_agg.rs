//! Restricted SQL surface for observation aggregates (issue #5, first slice).
//!
//! This is **not** a general SQL engine. It parses a tiny dialect and maps it
//! onto [`ObservationAggregation`]. Out-of-process / DuckDB projection remains
//! future work.
//!
//! Supported forms:
//! ```text
//! SELECT COUNT(*) FROM 'obs://health.sleep'
//! SELECT AVG(VALUE) FROM 'obs://health.sleep'
//! SELECT SUM(VALUE) FROM 'obs://ns' GROUP BY PATH_SEGMENT(1)
//! SELECT MIN(VALUE) FROM 'obs://ns' GROUP BY PATH_SEGMENT(2)
//! SELECT MAX(VALUE) FROM 'obs://ns'
//! SELECT P50(VALUE) FROM 'obs://ns'
//! SELECT P99(VALUE) FROM 'obs://ns'
//! ```
//!
//! `VALUE` extraction order: little-endian `f64` (8 bytes), then `bincode` `f64`,
//! then UTF-8 parse of payload as `f64`.
//! `PATH_SEGMENT(i)` splits `object_id` on `/` and `:` and takes 0-based index.

use crate::aggregation::{Aggregate, AggregateResult, ObservationAggregation};
use crate::engine::BinderStore;
use crate::error::StoreError;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SqlAggError {
    #[error("empty SQL")]
    Empty,
    #[error("unsupported SQL (restricted aggregate dialect only): {0}")]
    Unsupported(String),
    #[error("missing FROM namespace (quoted string required)")]
    MissingNamespace,
    #[error("invalid PATH_SEGMENT index")]
    BadPathSegment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlGroupBy {
    None,
    /// Split object id on `/` and `:`; take segment at `index` (0-based).
    PathSegment(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlAggregateQuery {
    pub aggregate: Aggregate,
    pub namespace: String,
    pub group_by: SqlGroupBy,
    /// True when the aggregate needs numeric `VALUE` extraction.
    pub needs_value: bool,
}

impl SqlAggregateQuery {
    /// Parse the restricted dialect. Keywords are case-insensitive.
    pub fn parse(sql: &str) -> Result<Self, SqlAggError> {
        let raw = sql.trim();
        if raw.is_empty() {
            return Err(SqlAggError::Empty);
        }
        if !starts_with_keyword(raw, "SELECT") {
            return Err(SqlAggError::Unsupported("must start with SELECT".into()));
        }
        let after_select = skip_keyword(raw, "SELECT");

        let (aggregate, needs_value, after_agg) = parse_aggregate(after_select)?;
        let after_agg = after_agg.trim_start();
        if !starts_with_keyword(after_agg, "FROM") {
            return Err(SqlAggError::Unsupported("expected FROM".into()));
        }
        let after_from = skip_keyword(after_agg, "FROM");

        let (namespace, rest) = parse_quoted(after_from)?;
        let rest = rest.trim_start();
        let group_by = if rest.is_empty() {
            SqlGroupBy::None
        } else {
            parse_group_by(rest)?
        };

        Ok(SqlAggregateQuery {
            aggregate,
            namespace,
            group_by,
            needs_value,
        })
    }
}

fn starts_with_keyword(s: &str, kw: &str) -> bool {
    let s = s.trim_start();
    s.len() >= kw.len()
        && s[..kw.len()].eq_ignore_ascii_case(kw)
        && (s.len() == kw.len() || s.as_bytes()[kw.len()].is_ascii_whitespace())
}

fn skip_keyword<'a>(s: &'a str, kw: &str) -> &'a str {
    let s = s.trim_start();
    debug_assert!(starts_with_keyword(s, kw));
    s[kw.len()..].trim_start()
}

fn parse_aggregate(s: &str) -> Result<(Aggregate, bool, &str), SqlAggError> {
    let u = s.to_ascii_uppercase();
    let patterns: &[(&str, Aggregate, bool)] = &[
        ("COUNT(*)", Aggregate::Count, false),
        ("AVG(VALUE)", Aggregate::Avg, true),
        ("SUM(VALUE)", Aggregate::Sum, true),
        ("MIN(VALUE)", Aggregate::Min, true),
        ("MAX(VALUE)", Aggregate::Max, true),
        ("P50(VALUE)", Aggregate::P50, true),
        ("P99(VALUE)", Aggregate::P99, true),
    ];
    for (pat, agg, needs) in patterns {
        if u.starts_with(pat) {
            return Ok((*agg, *needs, &s[pat.len()..]));
        }
    }
    Err(SqlAggError::Unsupported(
        "aggregate must be COUNT(*), AVG/SUM/MIN/MAX/P50/P99(VALUE)".into(),
    ))
}

fn parse_quoted(s: &str) -> Result<(String, &str), SqlAggError> {
    let s = s.trim_start();
    let quote = s.chars().next().ok_or(SqlAggError::MissingNamespace)?;
    if quote != '\'' && quote != '"' {
        return Err(SqlAggError::MissingNamespace);
    }
    let rest = &s[quote.len_utf8()..];
    let end = rest.find(quote).ok_or(SqlAggError::MissingNamespace)?;
    let ns = rest[..end].to_string();
    if ns.is_empty() {
        return Err(SqlAggError::MissingNamespace);
    }
    Ok((ns, &rest[end + quote.len_utf8()..]))
}

fn parse_group_by(s: &str) -> Result<SqlGroupBy, SqlAggError> {
    if !starts_with_keyword(s, "GROUP") {
        return Err(SqlAggError::Unsupported(format!(
            "trailing SQL not supported: {s}"
        )));
    }
    let after_group = skip_keyword(s, "GROUP");
    if !starts_with_keyword(after_group, "BY") {
        return Err(SqlAggError::Unsupported("expected GROUP BY".into()));
    }
    let after = skip_keyword(after_group, "BY");
    let au = after.to_ascii_uppercase();
    let prefix = "PATH_SEGMENT(";
    if !au.starts_with(prefix) {
        return Err(SqlAggError::Unsupported(
            "GROUP BY must be PATH_SEGMENT(i)".into(),
        ));
    }
    let orig = &after[prefix.len()..];
    let close = orig.find(')').ok_or(SqlAggError::BadPathSegment)?;
    let idx: usize = orig[..close]
        .trim()
        .parse()
        .map_err(|_| SqlAggError::BadPathSegment)?;
    let trailing = orig[close + 1..].trim();
    if !trailing.is_empty() {
        return Err(SqlAggError::Unsupported(format!(
            "trailing SQL: {trailing}"
        )));
    }
    Ok(SqlGroupBy::PathSegment(idx))
}

fn extract_value(payload: &[u8]) -> Option<f64> {
    if payload.len() == 8 {
        if let Ok(arr) = <[u8; 8]>::try_from(payload) {
            return Some(f64::from_le_bytes(arr));
        }
    }
    if let Ok(v) = bincode::deserialize::<f64>(payload) {
        return Some(v);
    }
    std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
}

fn path_segment(object_id: &str, index: usize) -> Option<String> {
    let parts: Vec<&str> = object_id
        .split(['/', ':'])
        .filter(|p| !p.is_empty())
        .collect();
    parts.get(index).map(|s| (*s).to_string())
}

impl BinderStore {
    /// Execute a restricted aggregate SQL string (issue #5 first slice).
    pub fn sql_aggregate(&self, sql: &str) -> Result<Vec<AggregateResult>, StoreError> {
        let q = SqlAggregateQuery::parse(sql).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.sql_aggregate_parsed(&q)
    }

    /// Execute a pre-parsed restricted aggregate query.
    pub fn sql_aggregate_parsed(
        &self,
        q: &SqlAggregateQuery,
    ) -> Result<Vec<AggregateResult>, StoreError> {
        let mut builder: ObservationAggregation<'_> = self
            .aggregate_observations()
            .namespace(q.namespace.clone())
            .aggregate(q.aggregate);

        if q.needs_value {
            builder = builder.extract_f64(extract_value);
        }

        builder = match q.group_by {
            SqlGroupBy::None => builder,
            SqlGroupBy::PathSegment(idx) => {
                builder.group_by(move |v| path_segment(&v.object_id.0, idx))
            }
        };

        builder.execute()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_count() {
        let q = SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://sleep'").unwrap();
        assert_eq!(q.aggregate, Aggregate::Count);
        assert_eq!(q.namespace, "obs://sleep");
        assert!(!q.needs_value);
        assert_eq!(q.group_by, SqlGroupBy::None);
    }

    #[test]
    fn parses_avg_group_by() {
        let q = SqlAggregateQuery::parse(
            "select avg(value) from \"obs://health.sleep\" group by path_segment(1)",
        )
        .unwrap();
        assert_eq!(q.aggregate, Aggregate::Avg);
        assert!(q.needs_value);
        assert_eq!(q.group_by, SqlGroupBy::PathSegment(1));
    }

    #[test]
    fn rejects_join() {
        assert!(SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://a' JOIN 'obs://b'").is_err());
    }
}
