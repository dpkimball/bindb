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
//! SELECT MIN(VALUE) FROM 'obs://ns' WHERE id STARTS WITH '2024'
//! SELECT AVG(VALUE) FROM 'obs://ns' WHERE id STARTS WITH '2024-01' GROUP BY PATH_SEGMENT(2)
//! SELECT MAX(VALUE) FROM 'obs://ns'
//! SELECT P50(VALUE) FROM 'obs://ns'
//! SELECT P99(VALUE) FROM 'obs://ns'
//! SELECT COUNT(*) FROM 'obs://ns' GROUP BY PAYLOAD_FIELD('weekday')
//! SELECT AVG(VALUE) FROM 'obs://ns' GROUP BY PAYLOAD_FIELD('category')
//! ```
//!
//! `VALUE` extraction order: little-endian `f64` (8 bytes), then `bincode` `f64`,
//! then UTF-8 parse of payload as `f64`.
//! `PATH_SEGMENT(i)` splits `object_id` on `/` and `:` and takes 0-based index.
//! `PAYLOAD_FIELD('name')` groups by the named field in a UTF-8 JSON object payload.
//!   Observations whose payload is not JSON or lacks the field form a `None` group.
//! `WHERE id STARTS WITH 'prefix'` filters by the object_id suffix after the namespace.

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
    /// Group by the named field in a UTF-8 JSON object payload.
    /// Observations whose payload is non-JSON or lacks the field go into the `None` group.
    PayloadField(String),
    /// Group by multiple payload fields (`GROUP BY PAYLOAD_FIELD('f1'), PAYLOAD_FIELD('f2')`).
    /// Composite key = JSON array of the field values (e.g. `["red",1]`).
    /// Observations missing any field contribute a `null` for that position.
    MultiPayloadField(Vec<String>),
}

/// Filter applied to observation `object_id` in a `WHERE` clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlIdFilter {
    /// `WHERE id STARTS WITH 'prefix'`
    StartsWith(String),
    /// `WHERE id ENDS WITH 'suffix'`
    EndsWith(String),
    /// `WHERE id CONTAINS 'substr'`
    Contains(String),
    /// `WHERE id = 'exact'`
    Equals(String),
    /// `WHERE id IN ('a', 'b', 'c')` — matches any of the listed suffixes
    In(Vec<String>),
    /// `WHERE id NOT IN ('a', 'b', 'c')` — must not match any of the listed suffixes
    NotIn(Vec<String>),
}

/// Comparison operator for a HAVING clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HavingOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
}

/// Which aggregate value to compare in the HAVING clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HavingAggregate {
    /// `COUNT(*)` — compare against `AggregateResult::count`
    Count,
    /// Any `VALUE` aggregate — compare against `AggregateResult::value`
    Value,
}

/// Post-aggregation filter: `HAVING COUNT(*) > 5` or `HAVING AVG(VALUE) >= 3.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct HavingClause {
    pub agg: HavingAggregate,
    pub op: HavingOp,
    pub threshold: f64,
}

/// Column to sort aggregate results by in `ORDER BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggOrderByField {
    /// Sort by the aggregate result value (avg/count/sum/…).
    Value,
    /// Sort by the observation count within the group.
    Count,
    /// Sort by the group key string.
    GroupKey,
}

/// Sort direction for `ORDER BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggOrderDirection {
    Asc,
    Desc,
}

/// Post-aggregation sort: `ORDER BY value DESC`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggOrderBy {
    pub field: AggOrderByField,
    pub direction: AggOrderDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SqlAggregateQuery {
    pub aggregate: Aggregate,
    pub namespace: String,
    pub group_by: SqlGroupBy,
    /// True when the aggregate needs numeric `VALUE` extraction.
    pub needs_value: bool,
    /// Optional `WHERE id ...` filter applied to observation `object_id`.
    pub id_filter: Option<SqlIdFilter>,
    /// `WHERE PAYLOAD_FIELD('f') OP value` pre-aggregation filters.
    pub payload_filters: Vec<SqlPayloadFilter>,
    /// How `payload_filters` are combined (AND or OR). Defaults to `And`.
    pub payload_combinator: SqlFilterCombinator,
    /// Post-aggregation filters (AND/OR-combined by `having_combinator`).
    pub having: Vec<HavingClause>,
    /// How multiple `having` conditions are combined. Defaults to `And`.
    pub having_combinator: SqlFilterCombinator,
    /// Optional post-aggregation sort (applied after HAVING).
    pub order_by: Option<AggOrderBy>,
    /// Optional cap on returned aggregate rows (applied last).
    pub limit: Option<usize>,
    /// Number of aggregate rows to skip before applying LIMIT.
    pub offset: Option<usize>,
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

        // Optional WHERE clause: id filter, payload filter, or both joined with AND/OR.
        let (id_filter, payload_filters, payload_combinator, rest) =
            if starts_with_keyword(rest, "WHERE") {
                let after_where = skip_keyword(rest, "WHERE").trim_start();
                let su = after_where.to_ascii_uppercase();
                if su.starts_with("PAYLOAD_FIELD(") {
                    let (pfs, comb, remaining) = parse_payload_filter_chain(after_where)?;
                    (None, pfs, comb, remaining)
                } else {
                    let (filter, remaining) = parse_where_id_filter(after_where)?;
                    let remaining = remaining.trim_start();
                    // Optional AND PAYLOAD_FIELD(...) chain after the id filter
                    if starts_with_keyword(remaining, "AND") {
                        let after_and = skip_keyword(remaining, "AND").trim_start();
                        if after_and.to_ascii_uppercase().starts_with("PAYLOAD_FIELD(") {
                            let (pfs, comb, rem2) = parse_payload_filter_chain(after_and)?;
                            (Some(filter), pfs, comb, rem2)
                        } else {
                            (Some(filter), vec![], SqlFilterCombinator::And, remaining)
                        }
                    } else {
                        (Some(filter), vec![], SqlFilterCombinator::And, remaining)
                    }
                }
            } else {
                (None, vec![], SqlFilterCombinator::And, rest)
            };

        let mut rest = rest.trim_start();

        let group_by = if starts_with_keyword(rest, "GROUP") {
            let (gb, remaining) = parse_group_by(rest)?;
            rest = remaining;
            gb
        } else {
            SqlGroupBy::None
        };

        let mut having: Vec<HavingClause> = Vec::new();
        let mut having_combinator = SqlFilterCombinator::And;
        if starts_with_keyword(rest, "HAVING") {
            let after_having = skip_keyword(rest, "HAVING").trim_start();
            let (clause, remainder) = parse_having(after_having)?;
            having.push(clause);
            rest = remainder;
            loop {
                let is_and = starts_with_keyword(rest, "AND");
                let is_or = starts_with_keyword(rest, "OR");
                if !is_and && !is_or {
                    break;
                }
                let after_conn = if is_and {
                    skip_keyword(rest, "AND")
                } else {
                    skip_keyword(rest, "OR")
                }
                .trim_start();
                let su2 = after_conn.to_ascii_uppercase();
                let is_having_expr = su2.starts_with("COUNT(*)")
                    || su2.starts_with("AVG(")
                    || su2.starts_with("SUM(")
                    || su2.starts_with("MIN(")
                    || su2.starts_with("MAX(")
                    || su2.starts_with("P50(")
                    || su2.starts_with("P99(");
                if !is_having_expr {
                    break;
                }
                if !having.is_empty() {
                    having_combinator = if is_and {
                        SqlFilterCombinator::And
                    } else {
                        SqlFilterCombinator::Or
                    };
                }
                let (clause2, rem2) = parse_having(after_conn)?;
                having.push(clause2);
                rest = rem2;
            }
        }

        let order_by = if starts_with_keyword(rest, "ORDER") {
            let after_order = skip_keyword(rest, "ORDER").trim_start();
            if !starts_with_keyword(after_order, "BY") {
                return Err(SqlAggError::Unsupported("expected ORDER BY".into()));
            }
            let after_by = skip_keyword(after_order, "BY").trim_start();
            let (ob, remainder) = parse_agg_order_by(after_by)?;
            rest = remainder;
            Some(ob)
        } else {
            None
        };

        let offset = if starts_with_keyword(rest, "OFFSET") {
            let after_offset = skip_keyword(rest, "OFFSET").trim_start();
            let end = after_offset
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_offset.len());
            let n: usize = after_offset[..end].parse().map_err(|_| {
                SqlAggError::Unsupported("OFFSET must be a non-negative integer".into())
            })?;
            rest = after_offset[end..].trim_start();
            Some(n)
        } else {
            None
        };

        let limit = if starts_with_keyword(rest, "LIMIT") {
            let after_limit = skip_keyword(rest, "LIMIT").trim_start();
            let end = after_limit
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_limit.len());
            let n: usize = after_limit[..end]
                .parse()
                .map_err(|_| SqlAggError::Unsupported("LIMIT must be a positive integer".into()))?;
            rest = after_limit[end..].trim_start();
            Some(n)
        } else {
            None
        };

        if !rest.is_empty() {
            return Err(SqlAggError::Unsupported(format!(
                "trailing SQL not supported: {rest}"
            )));
        }

        Ok(SqlAggregateQuery {
            aggregate,
            namespace,
            group_by,
            needs_value,
            id_filter,
            payload_filters,
            payload_combinator,
            having,
            having_combinator,
            order_by,
            limit,
            offset,
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

fn parse_where_id_filter(s: &str) -> Result<(SqlIdFilter, &str), SqlAggError> {
    let s = s.trim_start();
    if !starts_with_keyword(s, "id") {
        return Err(SqlAggError::Unsupported(
            "WHERE clause must be: id STARTS WITH / ENDS WITH / CONTAINS / = / IN '…'".into(),
        ));
    }
    let after_id = skip_keyword(s, "id");
    if starts_with_keyword(after_id, "STARTS WITH") {
        let after_kw = skip_keyword(after_id, "STARTS WITH");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((SqlIdFilter::StartsWith(val), rest.trim_start()))
    } else if starts_with_keyword(after_id, "ENDS WITH") {
        let after_kw = skip_keyword(after_id, "ENDS WITH");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((SqlIdFilter::EndsWith(val), rest.trim_start()))
    } else if starts_with_keyword(after_id, "CONTAINS") {
        let after_kw = skip_keyword(after_id, "CONTAINS");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((SqlIdFilter::Contains(val), rest.trim_start()))
    } else if starts_with_keyword(after_id, "NOT IN") {
        let after_kw = skip_keyword(after_id, "NOT IN").trim_start();
        let (vals, rest) = parse_quoted_list(after_kw)?;
        Ok((SqlIdFilter::NotIn(vals), rest.trim_start()))
    } else if starts_with_keyword(after_id, "IN") {
        let after_kw = skip_keyword(after_id, "IN").trim_start();
        let (vals, rest) = parse_quoted_list(after_kw)?;
        Ok((SqlIdFilter::In(vals), rest.trim_start()))
    } else if let Some(after_eq) = after_id.trim_start().strip_prefix('=') {
        let (val, rest) = parse_quoted(after_eq)?;
        Ok((SqlIdFilter::Equals(val), rest.trim_start()))
    } else {
        Err(SqlAggError::Unsupported(
            "WHERE id must be followed by STARTS WITH, ENDS WITH, CONTAINS, IN, or =".into(),
        ))
    }
}

/// Parse a parenthesised list of quoted strings: `('a', 'b', 'c')`.
fn parse_quoted_list(s: &str) -> Result<(Vec<String>, &str), SqlAggError> {
    let s = s.trim_start();
    let s = s
        .strip_prefix('(')
        .ok_or_else(|| SqlAggError::Unsupported("expected '(' before id IN list".into()))?;
    let mut vals = Vec::new();
    let mut rest = s.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix(')') {
            return Ok((vals, after));
        }
        let (val, after) = parse_quoted(rest)?;
        vals.push(val);
        rest = after.trim_start();
        if rest.starts_with(',') {
            rest = rest[1..].trim_start();
        }
    }
}

/// Parse a parenthesised numeric list: `(1.0, 2.5, 3)`.
fn parse_paren_numeric_list(s: &str) -> Result<(Vec<f64>, &str), SqlAggError> {
    let s = s.trim_start();
    let inner = s
        .strip_prefix('(')
        .ok_or_else(|| SqlAggError::Unsupported("expected '(' before numeric IN list".into()))?;
    let mut nums = Vec::new();
    let mut rest = inner.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix(')') {
            return Ok((nums, after));
        }
        let end = rest
            .find(|c: char| {
                !c.is_ascii_digit() && c != '.' && c != '-' && c != '+' && c != 'e' && c != 'E'
            })
            .unwrap_or(rest.len());
        if end == 0 {
            return Err(SqlAggError::Unsupported(
                "expected number in numeric IN list".into(),
            ));
        }
        let n: f64 = rest[..end]
            .parse()
            .map_err(|_| SqlAggError::Unsupported("invalid number in numeric IN list".into()))?;
        nums.push(n);
        rest = rest[end..].trim_start();
        if rest.starts_with(',') {
            rest = rest[1..].trim_start();
        }
    }
}

/// Returns `(SqlGroupBy, remaining_text)`.
fn parse_group_by(s: &str) -> Result<(SqlGroupBy, &str), SqlAggError> {
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

    if au.starts_with("PATH_SEGMENT(") {
        let orig = &after["PATH_SEGMENT(".len()..];
        let close = orig.find(')').ok_or(SqlAggError::BadPathSegment)?;
        let idx: usize = orig[..close]
            .trim()
            .parse()
            .map_err(|_| SqlAggError::BadPathSegment)?;
        let rest = orig[close + 1..].trim_start();
        Ok((SqlGroupBy::PathSegment(idx), rest))
    } else if au.starts_with("PAYLOAD_FIELD(") {
        let orig = &after["PAYLOAD_FIELD(".len()..];
        let (field, rest) = parse_quoted_sql(orig)?;
        let rest = rest.trim_start();
        if !rest.starts_with(')') {
            return Err(SqlAggError::Unsupported(
                "expected ')' after PAYLOAD_FIELD argument".into(),
            ));
        }
        let mut fields = vec![field];
        let mut rest = rest[1..].trim_start();
        // Check for additional comma-separated PAYLOAD_FIELD('f2') entries
        while rest.starts_with(',') {
            let after_comma = rest[1..].trim_start();
            let au2 = after_comma.to_ascii_uppercase();
            if au2.starts_with("PAYLOAD_FIELD(") {
                let orig2 = &after_comma["PAYLOAD_FIELD(".len()..];
                let (f2, r2) = parse_quoted_sql(orig2)?;
                let r2 = r2.trim_start();
                if !r2.starts_with(')') {
                    break;
                }
                fields.push(f2);
                rest = r2[1..].trim_start();
            } else {
                break;
            }
        }
        if fields.len() == 1 {
            Ok((SqlGroupBy::PayloadField(fields.remove(0)), rest))
        } else {
            Ok((SqlGroupBy::MultiPayloadField(fields), rest))
        }
    } else {
        Err(SqlAggError::Unsupported(
            "GROUP BY must be PATH_SEGMENT(i) or PAYLOAD_FIELD('name')".into(),
        ))
    }
}

/// Parse a single-quoted or double-quoted string literal within a SQL GROUP BY argument.
fn parse_quoted_sql(s: &str) -> Result<(String, &str), SqlAggError> {
    let s = s.trim_start();
    let quote = s.chars().next().ok_or_else(|| {
        SqlAggError::Unsupported("expected quoted field name in PAYLOAD_FIELD".into())
    })?;
    if quote != '\'' && quote != '"' {
        return Err(SqlAggError::Unsupported(
            "PAYLOAD_FIELD argument must be a quoted string".into(),
        ));
    }
    let rest = &s[quote.len_utf8()..];
    let end = rest
        .find(quote)
        .ok_or_else(|| SqlAggError::Unsupported("unclosed string in PAYLOAD_FIELD".into()))?;
    Ok((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

/// Parse `COUNT(*) > 5` or `AVG(VALUE) >= 3.0` after the HAVING keyword.
/// Returns `(HavingClause, remaining_text)`.
fn parse_having(s: &str) -> Result<(HavingClause, &str), SqlAggError> {
    let su = s.to_ascii_uppercase();
    let (agg, after_agg) = if su.starts_with("COUNT(*)") {
        (HavingAggregate::Count, &s["COUNT(*)".len()..])
    } else {
        let agg_patterns: &[(&str, bool)] = &[
            ("AVG(VALUE)", true),
            ("SUM(VALUE)", true),
            ("MIN(VALUE)", true),
            ("MAX(VALUE)", true),
            ("P50(VALUE)", true),
            ("P99(VALUE)", true),
        ];
        let mut found = None;
        for (pat, _) in agg_patterns {
            if su.starts_with(pat) {
                found = Some(&s[pat.len()..]);
                break;
            }
        }
        match found {
            Some(rest) => (HavingAggregate::Value, rest),
            None => {
                return Err(SqlAggError::Unsupported(
                    "HAVING must use COUNT(*) or a VALUE aggregate".into(),
                ))
            }
        }
    };

    let after_agg = after_agg.trim_start();
    let (op, after_op) = if let Some(r) = after_agg.strip_prefix(">=") {
        (HavingOp::Gte, r)
    } else if let Some(r) = after_agg.strip_prefix("<=") {
        (HavingOp::Lte, r)
    } else if let Some(r) = after_agg.strip_prefix("!=") {
        (HavingOp::Ne, r)
    } else if let Some(r) = after_agg.strip_prefix('>') {
        (HavingOp::Gt, r)
    } else if let Some(r) = after_agg.strip_prefix('<') {
        (HavingOp::Lt, r)
    } else if let Some(r) = after_agg.strip_prefix('=') {
        (HavingOp::Eq, r)
    } else {
        return Err(SqlAggError::Unsupported(
            "expected comparison operator in HAVING clause".into(),
        ));
    };

    let after_op = after_op.trim_start();
    let num_end = after_op
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(after_op.len());
    if num_end == 0 {
        return Err(SqlAggError::Unsupported(
            "expected numeric threshold in HAVING clause".into(),
        ));
    }
    let threshold: f64 = after_op[..num_end]
        .parse()
        .map_err(|_| SqlAggError::Unsupported("invalid number in HAVING clause".into()))?;

    Ok((
        HavingClause { agg, op, threshold },
        after_op[num_end..].trim_start(),
    ))
}

/// Parse `value|count|group_key [ASC|DESC]`.
fn parse_agg_order_by(s: &str) -> Result<(AggOrderBy, &str), SqlAggError> {
    let (field, rest) = if starts_with_keyword(s, "value") {
        (AggOrderByField::Value, skip_keyword(s, "value"))
    } else if starts_with_keyword(s, "count") {
        (AggOrderByField::Count, skip_keyword(s, "count"))
    } else if starts_with_keyword(s, "group_key") {
        (AggOrderByField::GroupKey, skip_keyword(s, "group_key"))
    } else {
        return Err(SqlAggError::Unsupported(
            "ORDER BY must be value, count, or group_key".into(),
        ));
    };
    let rest = rest.trim_start();
    let (direction, rest) = if starts_with_keyword(rest, "DESC") {
        (AggOrderDirection::Desc, skip_keyword(rest, "DESC"))
    } else if starts_with_keyword(rest, "ASC") {
        (AggOrderDirection::Asc, skip_keyword(rest, "ASC"))
    } else {
        (AggOrderDirection::Asc, rest)
    };
    Ok((AggOrderBy { field, direction }, rest.trim_start()))
}

/// Parse `PAYLOAD_FIELD('field') OP value` after the WHERE keyword.
/// Returns `(SqlPayloadFilter, remaining_text)`.
fn parse_payload_filter(s: &str) -> Result<(SqlPayloadFilter, &str), SqlAggError> {
    let su = s.to_ascii_uppercase();
    if !su.starts_with("PAYLOAD_FIELD(") {
        return Err(SqlAggError::Unsupported(
            "WHERE payload filter must start with PAYLOAD_FIELD('field')".into(),
        ));
    }
    let after_pf = &s["PAYLOAD_FIELD(".len()..];
    let (field, rest) = parse_quoted_sql(after_pf)?;
    let rest = rest.trim_start();
    if !rest.starts_with(')') {
        return Err(SqlAggError::Unsupported(
            "expected ')' after PAYLOAD_FIELD argument".into(),
        ));
    }
    let after_close = rest[1..].trim_start();

    // Parse operator
    let (op, after_op) = if starts_with_keyword(after_close, "STARTS WITH") {
        (
            SqlPayloadOp::StartsWith,
            skip_keyword(after_close, "STARTS WITH"),
        )
    } else if starts_with_keyword(after_close, "ENDS WITH") {
        (
            SqlPayloadOp::EndsWith,
            skip_keyword(after_close, "ENDS WITH"),
        )
    } else if starts_with_keyword(after_close, "CONTAINS") {
        (
            SqlPayloadOp::Contains,
            skip_keyword(after_close, "CONTAINS"),
        )
    } else if starts_with_keyword(after_close, "NOT ILIKE") {
        let after_op = skip_keyword(after_close, "NOT ILIKE").trim_start();
        let (s, rest2) = parse_quoted(after_op)
            .map_err(|_| SqlAggError::Unsupported("NOT ILIKE requires a quoted pattern".into()))?;
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::NotILike,
                value: SqlPayloadValue::Str(s),
            },
            rest2.trim_start(),
        ));
    } else if starts_with_keyword(after_close, "NOT LIKE") {
        let after_op = skip_keyword(after_close, "NOT LIKE").trim_start();
        let (s, rest2) = parse_quoted(after_op)
            .map_err(|_| SqlAggError::Unsupported("NOT LIKE requires a quoted pattern".into()))?;
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::NotLike,
                value: SqlPayloadValue::Str(s),
            },
            rest2.trim_start(),
        ));
    } else if starts_with_keyword(after_close, "NOT IN") {
        let after_op = skip_keyword(after_close, "NOT IN").trim_start();
        // Auto-detect numeric vs string list by peeking at first non-paren char.
        let peek = after_op
            .trim_start()
            .strip_prefix('(')
            .map(|s| s.trim_start().chars().next().unwrap_or(' '))
            .unwrap_or(' ');
        if peek.is_ascii_digit() || peek == '-' || peek == '+' {
            let (nums, rest2) = parse_paren_numeric_list(after_op)?;
            return Ok((
                SqlPayloadFilter {
                    field,
                    op: SqlPayloadOp::NotIn,
                    value: SqlPayloadValue::NumList(nums),
                },
                rest2.trim_start(),
            ));
        }
        let (items, rest2) = parse_quoted_list(after_op)?;
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::NotIn,
                value: SqlPayloadValue::StrList(items),
            },
            rest2.trim_start(),
        ));
    } else if starts_with_keyword(after_close, "IN") {
        let after_op = skip_keyword(after_close, "IN").trim_start();
        // Auto-detect numeric vs string list.
        let peek = after_op
            .trim_start()
            .strip_prefix('(')
            .map(|s| s.trim_start().chars().next().unwrap_or(' '))
            .unwrap_or(' ');
        if peek.is_ascii_digit() || peek == '-' || peek == '+' {
            let (nums, rest2) = parse_paren_numeric_list(after_op)?;
            return Ok((
                SqlPayloadFilter {
                    field,
                    op: SqlPayloadOp::In,
                    value: SqlPayloadValue::NumList(nums),
                },
                rest2.trim_start(),
            ));
        }
        let (items, rest2) = parse_quoted_list(after_op)?;
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::In,
                value: SqlPayloadValue::StrList(items),
            },
            rest2.trim_start(),
        ));
    } else if starts_with_keyword(after_close, "ILIKE") {
        (SqlPayloadOp::ILike, skip_keyword(after_close, "ILIKE"))
    } else if starts_with_keyword(after_close, "LIKE") {
        (SqlPayloadOp::Like, skip_keyword(after_close, "LIKE"))
    } else if starts_with_keyword(after_close, "BETWEEN") {
        let after_between = skip_keyword(after_close, "BETWEEN").trim_start();
        let lo_end = after_between
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(after_between.len());
        if lo_end == 0 {
            return Err(SqlAggError::Unsupported(
                "BETWEEN requires a numeric lower bound".into(),
            ));
        }
        let lo: f64 = after_between[..lo_end]
            .parse()
            .map_err(|_| SqlAggError::Unsupported("invalid lower bound in BETWEEN".into()))?;
        let after_lo = after_between[lo_end..].trim_start();
        if !starts_with_keyword(after_lo, "AND") {
            return Err(SqlAggError::Unsupported(
                "BETWEEN requires AND between bounds".into(),
            ));
        }
        let after_and = skip_keyword(after_lo, "AND").trim_start();
        let hi_end = after_and
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(after_and.len());
        if hi_end == 0 {
            return Err(SqlAggError::Unsupported(
                "BETWEEN requires a numeric upper bound".into(),
            ));
        }
        let hi: f64 = after_and[..hi_end]
            .parse()
            .map_err(|_| SqlAggError::Unsupported("invalid upper bound in BETWEEN".into()))?;
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::Between,
                value: SqlPayloadValue::NumRange(lo, hi),
            },
            after_and[hi_end..].trim_start(),
        ));
    } else if starts_with_keyword(after_close, "IS NOT NULL") {
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::IsNotNull,
                value: SqlPayloadValue::Null,
            },
            skip_keyword(after_close, "IS NOT NULL").trim_start(),
        ));
    } else if starts_with_keyword(after_close, "IS NULL") {
        return Ok((
            SqlPayloadFilter {
                field,
                op: SqlPayloadOp::IsNull,
                value: SqlPayloadValue::Null,
            },
            skip_keyword(after_close, "IS NULL").trim_start(),
        ));
    } else if let Some(r) = after_close.strip_prefix(">=") {
        (SqlPayloadOp::Gte, r)
    } else if let Some(r) = after_close.strip_prefix("<=") {
        (SqlPayloadOp::Lte, r)
    } else if let Some(r) = after_close.strip_prefix("!=") {
        (SqlPayloadOp::Ne, r)
    } else if let Some(r) = after_close.strip_prefix('>') {
        (SqlPayloadOp::Gt, r)
    } else if let Some(r) = after_close.strip_prefix('<') {
        (SqlPayloadOp::Lt, r)
    } else if let Some(r) = after_close.strip_prefix('=') {
        (SqlPayloadOp::Eq, r)
    } else {
        return Err(SqlAggError::Unsupported(
            "unrecognised operator in WHERE PAYLOAD_FIELD clause".into(),
        ));
    };

    let after_op = after_op.trim_start();

    // Parse value: quoted string or numeric literal
    if after_op.starts_with('\'') || after_op.starts_with('"') {
        let (s, rest2) = parse_quoted(after_op)?;
        return Ok((
            SqlPayloadFilter {
                field,
                op,
                value: SqlPayloadValue::Str(s),
            },
            rest2.trim_start(),
        ));
    }
    let num_end = after_op
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(after_op.len());
    if num_end == 0 {
        return Err(SqlAggError::Unsupported(
            "expected quoted string or number in WHERE PAYLOAD_FIELD clause".into(),
        ));
    }
    let n: f64 = after_op[..num_end]
        .parse()
        .map_err(|_| SqlAggError::Unsupported("invalid number in WHERE PAYLOAD_FIELD".into()))?;
    Ok((
        SqlPayloadFilter {
            field,
            op,
            value: SqlPayloadValue::Num(n),
        },
        after_op[num_end..].trim_start(),
    ))
}

/// SQL LIKE pattern matching: `%` = any char sequence, `_` = exactly one char.
fn sql_like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (tn, pn) = (t.len(), p.len());
    let mut dp = vec![vec![false; pn + 1]; tn + 1];
    dp[0][0] = true;
    for j in 1..=pn {
        if p[j - 1] == '%' {
            dp[0][j] = dp[0][j - 1];
        }
    }
    for i in 1..=tn {
        for j in 1..=pn {
            dp[i][j] = match p[j - 1] {
                '%' => dp[i - 1][j] || dp[i][j - 1],
                '_' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && t[i - 1] == c,
            };
        }
    }
    dp[tn][pn]
}

/// Parse a chain of `PAYLOAD_FIELD(...) OP value [AND|OR PAYLOAD_FIELD(...) OP value ...]`.
/// Returns `(Vec<SqlPayloadFilter>, SqlFilterCombinator, remaining_text)`.
/// Connectors must be consistent within one chain (all AND or all OR); mixed connectors stop the chain.
fn parse_payload_filter_chain(
    s: &str,
) -> Result<(Vec<SqlPayloadFilter>, SqlFilterCombinator, &str), SqlAggError> {
    let mut filters = Vec::new();
    let (first, mut rest) = parse_payload_filter(s)?;
    filters.push(first);
    rest = rest.trim_start();
    let mut combinator = SqlFilterCombinator::And;
    let mut first_connector = true;
    loop {
        let is_and = starts_with_keyword(rest, "AND");
        let is_or = starts_with_keyword(rest, "OR");
        if !is_and && !is_or {
            break;
        }
        let this_conn = if is_and {
            SqlFilterCombinator::And
        } else {
            SqlFilterCombinator::Or
        };
        if first_connector {
            combinator = this_conn;
            first_connector = false;
        } else if this_conn != combinator {
            break; // mixed connectors — stop consuming
        }
        let kw = if is_and { "AND" } else { "OR" };
        let after_kw = skip_keyword(rest, kw).trim_start();
        if !after_kw.to_ascii_uppercase().starts_with("PAYLOAD_FIELD(") {
            break;
        }
        let (pf, rem) = parse_payload_filter(after_kw)?;
        filters.push(pf);
        rest = rem.trim_start();
    }
    Ok((filters, combinator, rest))
}

/// Apply a `SqlPayloadFilter` to a raw observation payload. Returns `true` if the
/// observation passes the filter, `false` if it should be excluded.
fn apply_payload_filter(filter: &SqlPayloadFilter, payload: Option<&[u8]>) -> bool {
    let get_json = || -> Option<serde_json::Value> {
        let bytes = payload?;
        let text = std::str::from_utf8(bytes).ok()?;
        serde_json::from_str(text).ok()
    };

    match filter.op {
        SqlPayloadOp::IsNull => {
            let v = get_json();
            v.as_ref()
                .and_then(|j| j.get(&filter.field))
                .map(|f| f.is_null())
                .unwrap_or(true)
        }
        SqlPayloadOp::IsNotNull => {
            let v = get_json();
            v.as_ref()
                .and_then(|j| j.get(&filter.field))
                .map(|f| !f.is_null())
                .unwrap_or(false)
        }
        _ => {
            let json = match get_json() {
                Some(j) => j,
                None => return false,
            };
            let field_val = match json.get(&filter.field) {
                Some(v) if !v.is_null() => v.clone(),
                _ => return false,
            };
            match &filter.value {
                SqlPayloadValue::Str(s) => {
                    let field_str = match &field_val {
                        serde_json::Value::String(fs) => fs.as_str(),
                        _ => return false,
                    };
                    match filter.op {
                        SqlPayloadOp::Eq => field_str == s.as_str(),
                        SqlPayloadOp::Ne => field_str != s.as_str(),
                        SqlPayloadOp::StartsWith => field_str.starts_with(s.as_str()),
                        SqlPayloadOp::EndsWith => field_str.ends_with(s.as_str()),
                        SqlPayloadOp::Contains => field_str.contains(s.as_str()),
                        SqlPayloadOp::Like => sql_like_match(field_str, s.as_str()),
                        SqlPayloadOp::NotLike => !sql_like_match(field_str, s.as_str()),
                        SqlPayloadOp::ILike => {
                            sql_like_match(&field_str.to_lowercase(), &s.to_lowercase())
                        }
                        SqlPayloadOp::NotILike => {
                            !sql_like_match(&field_str.to_lowercase(), &s.to_lowercase())
                        }
                        _ => false,
                    }
                }
                SqlPayloadValue::StrList(items) => {
                    let field_str = match &field_val {
                        serde_json::Value::String(fs) => fs.as_str(),
                        _ => return false,
                    };
                    match filter.op {
                        SqlPayloadOp::In => items.iter().any(|v| field_str == v.as_str()),
                        SqlPayloadOp::NotIn => items.iter().all(|v| field_str != v.as_str()),
                        _ => false,
                    }
                }
                SqlPayloadValue::Num(n) => {
                    let field_num = match &field_val {
                        serde_json::Value::Number(num) => {
                            if let Some(f) = num.as_f64() {
                                f
                            } else {
                                return false;
                            }
                        }
                        _ => return false,
                    };
                    match filter.op {
                        SqlPayloadOp::Eq => (field_num - n).abs() < f64::EPSILON,
                        SqlPayloadOp::Ne => (field_num - n).abs() >= f64::EPSILON,
                        SqlPayloadOp::Gt => field_num > *n,
                        SqlPayloadOp::Lt => field_num < *n,
                        SqlPayloadOp::Gte => field_num >= *n,
                        SqlPayloadOp::Lte => field_num <= *n,
                        _ => false,
                    }
                }
                SqlPayloadValue::NumRange(lo, hi) => {
                    let field_num = match &field_val {
                        serde_json::Value::Number(num) => match num.as_f64() {
                            Some(f) => f,
                            None => return false,
                        },
                        _ => return false,
                    };
                    match filter.op {
                        SqlPayloadOp::Between => field_num >= *lo && field_num <= *hi,
                        _ => false,
                    }
                }
                SqlPayloadValue::NumList(nums) => {
                    let field_num = match &field_val {
                        serde_json::Value::Number(num) => match num.as_f64() {
                            Some(f) => f,
                            None => return false,
                        },
                        _ => return false,
                    };
                    match filter.op {
                        SqlPayloadOp::In => {
                            nums.iter().any(|n| (field_num - n).abs() < f64::EPSILON)
                        }
                        SqlPayloadOp::NotIn => {
                            nums.iter().all(|n| (field_num - n).abs() >= f64::EPSILON)
                        }
                        _ => false,
                    }
                }
                SqlPayloadValue::Null => false,
            }
        }
    }
}

/// Extract a JSON payload field value as a group key string.
/// Returns `None` for non-JSON payloads or missing fields (those go into the None group).
fn payload_field_group_key(payload: Option<&[u8]>, field: &str) -> Option<String> {
    let bytes = payload?;
    let text = std::str::from_utf8(bytes).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let field_val = v.get(field)?;
    Some(match field_val {
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".into()),
    })
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

        // Combine id_filter and payload_filters into a single closure (filter() replaces).
        if q.id_filter.is_some() || !q.payload_filters.is_empty() {
            let id_filter = q.id_filter.clone();
            let payload_filters = q.payload_filters.clone();
            let payload_combinator = q.payload_combinator;
            let ns_prefix = format!("{}:", q.namespace);
            builder = builder.filter(move |v| {
                if let Some(ref f) = id_filter {
                    let id = v
                        .object_id
                        .0
                        .strip_prefix(ns_prefix.as_str())
                        .unwrap_or(&v.object_id.0);
                    let id_ok = match f {
                        SqlIdFilter::StartsWith(s) => id.starts_with(s.as_str()),
                        SqlIdFilter::EndsWith(s) => id.ends_with(s.as_str()),
                        SqlIdFilter::Contains(s) => id.contains(s.as_str()),
                        SqlIdFilter::Equals(s) => id == s.as_str(),
                        SqlIdFilter::In(vs) => vs.iter().any(|s| id == s.as_str()),
                        SqlIdFilter::NotIn(vs) => vs.iter().all(|s| id != s.as_str()),
                    };
                    if !id_ok {
                        return false;
                    }
                }
                let payload_ok = match payload_combinator {
                    SqlFilterCombinator::And => payload_filters
                        .iter()
                        .all(|pf| apply_payload_filter(pf, v.payload.as_deref())),
                    SqlFilterCombinator::Or => payload_filters
                        .iter()
                        .any(|pf| apply_payload_filter(pf, v.payload.as_deref())),
                };
                payload_ok
            });
        }

        builder = match &q.group_by {
            SqlGroupBy::None => builder,
            SqlGroupBy::PathSegment(idx) => {
                let idx = *idx;
                builder.group_by(move |v| path_segment(&v.object_id.0, idx))
            }
            SqlGroupBy::PayloadField(field) => {
                let field = field.clone();
                builder.group_by(move |v| payload_field_group_key(v.payload.as_deref(), &field))
            }
            SqlGroupBy::MultiPayloadField(fields) => {
                let fields = fields.clone();
                builder.group_by(move |v| {
                    let json = v
                        .payload
                        .as_deref()
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok());
                    match json {
                        None => None,
                        Some(j) => {
                            let parts: Vec<String> = fields
                                .iter()
                                .map(|f| match j.get(f) {
                                    Some(v) => v.to_string(),
                                    None => "null".to_string(),
                                })
                                .collect();
                            Some(format!("[{}]", parts.join(",")))
                        }
                    }
                })
            }
        };

        let results = builder.execute()?;

        // Apply HAVING filter post-aggregation
        let mut results = if !q.having.is_empty() {
            results
                .into_iter()
                .filter(|r| {
                    let check = |having: &HavingClause| {
                        let lhs = match having.agg {
                            HavingAggregate::Count => r.count as f64,
                            HavingAggregate::Value => r.value,
                        };
                        let rhs = having.threshold;
                        match having.op {
                            HavingOp::Eq => (lhs - rhs).abs() < f64::EPSILON,
                            HavingOp::Ne => (lhs - rhs).abs() >= f64::EPSILON,
                            HavingOp::Gt => lhs > rhs,
                            HavingOp::Lt => lhs < rhs,
                            HavingOp::Gte => lhs >= rhs,
                            HavingOp::Lte => lhs <= rhs,
                        }
                    };
                    match q.having_combinator {
                        SqlFilterCombinator::And => q.having.iter().all(check),
                        SqlFilterCombinator::Or => q.having.iter().any(check),
                    }
                })
                .collect::<Vec<_>>()
        } else {
            results
        };

        // Apply ORDER BY sort (overrides the default group_key sort from aggregation.rs)
        if let Some(ob) = &q.order_by {
            match ob.field {
                AggOrderByField::Value => {
                    results.sort_by(|a, b| {
                        a.value
                            .partial_cmp(&b.value)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                AggOrderByField::Count => {
                    results.sort_by_key(|r| r.count);
                }
                AggOrderByField::GroupKey => {
                    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
                }
            }
            if ob.direction == AggOrderDirection::Desc {
                results.reverse();
            }
        }

        // Apply OFFSET then LIMIT (OFFSET first so LIMIT picks from the right window)
        if let Some(n) = q.offset {
            if n >= results.len() {
                results.clear();
            } else {
                results.drain(..n);
            }
        }
        if let Some(n) = q.limit {
            results.truncate(n);
        }

        Ok(results)
    }
}

// ── Columnar projection (issue #5) ───────────────────────────────────────────

/// Comparison operator for a `WHERE PAYLOAD_FIELD` filter in a projection query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlPayloadOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    StartsWith,
    EndsWith,
    Contains,
    IsNull,
    IsNotNull,
    /// `LIKE 'pat%tern'` — SQL wildcard: `%` = any chars, `_` = one char
    Like,
    /// `NOT LIKE 'pat%tern'`
    NotLike,
    /// `ILIKE 'pat%tern'` — case-insensitive SQL wildcard
    ILike,
    /// `NOT ILIKE 'pat%tern'`
    NotILike,
    /// `BETWEEN lo AND hi` — numeric inclusive range
    Between,
    /// `IN ('a', 'b', 'c')` — matches any listed string value
    In,
    /// `NOT IN ('a', 'b', 'c')` — must not match any listed string value
    NotIn,
}

/// A value to compare against in a `WHERE PAYLOAD_FIELD` filter.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlPayloadValue {
    Str(String),
    Num(f64),
    Null,
    /// Inclusive numeric range for `BETWEEN lo AND hi`.
    NumRange(f64, f64),
    /// String list for `IN` / `NOT IN`.
    StrList(Vec<String>),
    /// Numeric list for `IN` / `NOT IN` with numbers.
    NumList(Vec<f64>),
}

/// How multiple `payload_filters` are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlFilterCombinator {
    /// All filters must match (AND chain — default).
    And,
    /// Any filter must match (OR chain).
    Or,
}

/// `WHERE PAYLOAD_FIELD('field') OP value` filter for a projection query.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlPayloadFilter {
    pub field: String,
    pub op: SqlPayloadOp,
    pub value: SqlPayloadValue,
}

/// Transform applied to a projected field in SELECT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlFieldTransform {
    Upper,
    Lower,
    Trim,
    /// `ABS(PAYLOAD_FIELD('f'))` — absolute value of a numeric field.
    Abs,
    /// `ROUND(PAYLOAD_FIELD('f'))` — round numeric field to nearest integer.
    Round,
    /// `CEIL(PAYLOAD_FIELD('f'))` — ceiling of a numeric field.
    Ceil,
    /// `FLOOR(PAYLOAD_FIELD('f'))` — floor of a numeric field.
    Floor,
    /// `LENGTH(PAYLOAD_FIELD('f'))` — character count of a string field (returned as a number).
    Length,
    /// `COALESCE(PAYLOAD_FIELD('f'), 'fallback')` — substitute fallback when field is null.
    Coalesce(String),
    /// `REPLACE(PAYLOAD_FIELD('f'), 'from', 'to')` — string replacement.
    Replace(String, String),
    /// `SUBSTRING(PAYLOAD_FIELD('f'), start, len)` — extract substring (zero-based, char-indexed).
    Substring(usize, usize),
    /// `CAST(PAYLOAD_FIELD('f') AS INT)` — convert field to integer (truncates floats, parses strings).
    CastInt,
    /// `CAST(PAYLOAD_FIELD('f') AS FLOAT)` — convert field to float (parses strings).
    CastFloat,
    /// `CAST(PAYLOAD_FIELD('f') AS STRING)` — convert numeric field to its string representation.
    CastString,
    /// `NULLIF(PAYLOAD_FIELD('f'), 'val')` — returns null when field equals `val`.
    NullIf(String),
    /// `LPAD(PAYLOAD_FIELD('f'), n, 'c')` — left-pad string to width `n` with char `c`.
    Lpad(usize, String),
    /// `RPAD(PAYLOAD_FIELD('f'), n, 'c')` — right-pad string to width `n` with char `c`.
    Rpad(usize, String),
}

/// Sort direction for `ORDER BY` in a projection query.
pub use AggOrderDirection as SqlProjectDirection;

/// `ORDER BY PAYLOAD_FIELD('field') [ASC|DESC]` sort clause for `SqlProjectQuery`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlProjectOrderBy {
    pub field: String,
    pub direction: AggOrderDirection,
}

/// A parsed `SELECT PAYLOAD_FIELD(…) [, …] FROM … [WHERE …] [ORDER BY …] [LIMIT n]` query.
///
/// Returns one row per matching observation; each row holds the projected field
/// values as JSON. Missing or non-JSON fields become `null`.
///
/// ```text
/// SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('score') FROM 'obs://activities'
/// SELECT PAYLOAD_FIELD('category') FROM 'obs://items' WHERE id STARTS WITH '2024' LIMIT 100
/// SELECT PAYLOAD_FIELD('name') FROM 'obs://users' WHERE PAYLOAD_FIELD('status') = 'active'
/// SELECT PAYLOAD_FIELD('name'), PAYLOAD_FIELD('score') FROM 'obs://items' ORDER BY PAYLOAD_FIELD('score') DESC LIMIT 5
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SqlProjectQuery {
    /// Field names to extract from each observation's JSON payload.
    pub fields: Vec<String>,
    /// Output-key aliases for projected fields (`SELECT PAYLOAD_FIELD('name') AS n`).
    /// Key = raw field name, value = alias used as output key in the result row.
    pub field_aliases: std::collections::HashMap<String, String>,
    /// Optional string transforms on projected fields (`SELECT UPPER(PAYLOAD_FIELD('f'))`).
    /// Key = raw field name, value = transform applied to string values before output.
    pub field_transforms: std::collections::HashMap<String, SqlFieldTransform>,
    pub namespace: String,
    /// When `true`, each row includes an `"id"` key holding the full object_id string.
    pub return_id: bool,
    /// When `true`, duplicate rows (identical projected field values) are removed.
    pub distinct: bool,
    /// Optional `WHERE id ...` filter applied to object_id suffix after namespace prefix.
    pub id_filter: Option<SqlIdFilter>,
    /// `WHERE PAYLOAD_FIELD('f') OP value` pre-filter.
    pub payload_filters: Vec<SqlPayloadFilter>,
    /// How `payload_filters` are combined (AND or OR). Defaults to `And`.
    pub payload_combinator: SqlFilterCombinator,
    /// `ORDER BY` sort keys (multi-key, evaluated left to right). Empty = no sort.
    pub order_by: Vec<SqlProjectOrderBy>,
    /// Number of rows to skip before applying LIMIT (applied after ORDER BY).
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

impl SqlProjectQuery {
    /// Parse the columnar projection dialect. Keywords are case-insensitive.
    pub fn parse(sql: &str) -> Result<Self, SqlAggError> {
        let raw = sql.trim();
        if raw.is_empty() {
            return Err(SqlAggError::Empty);
        }
        if !starts_with_keyword(raw, "SELECT") {
            return Err(SqlAggError::Unsupported("must start with SELECT".into()));
        }
        let mut s = skip_keyword(raw, "SELECT");

        // Optional DISTINCT keyword immediately after SELECT.
        let distinct = if starts_with_keyword(s, "DISTINCT") {
            s = skip_keyword(s, "DISTINCT");
            true
        } else {
            false
        };

        let mut fields = Vec::new();
        let mut field_aliases: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut field_transforms: std::collections::HashMap<String, SqlFieldTransform> =
            std::collections::HashMap::new();
        let mut return_id = false;
        loop {
            s = s.trim_start();
            if starts_with_keyword(s, "FROM") {
                break;
            }
            // `id` — return the full object_id as the `"id"` column
            let su = s.to_ascii_uppercase();
            let is_id = su.starts_with("ID,")
                || su.starts_with("ID ")
                || su == "ID"
                || su.starts_with("ID\t");
            if is_id {
                return_id = true;
                s = &s[2..];
                s = s.trim_start();
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            // Detect SUBSTRING(PAYLOAD_FIELD('f'), start, len)
            let su = s.to_ascii_uppercase();
            if su.starts_with("SUBSTRING(PAYLOAD_FIELD(") {
                let inner = &s["SUBSTRING(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in SUBSTRING".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected ',' after PAYLOAD_FIELD in SUBSTRING".into(),
                        )
                    })?
                    .trim_start();
                let comma2 = rest.find(',').ok_or_else(|| {
                    SqlAggError::Unsupported("SUBSTRING requires start and len arguments".into())
                })?;
                let start: usize = rest[..comma2].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported(
                        "SUBSTRING start must be a non-negative integer".into(),
                    )
                })?;
                let rest2 = rest[comma2 + 1..].trim_start();
                let close = rest2.find(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' to close SUBSTRING".into())
                })?;
                let len: usize = rest2[..close].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported("SUBSTRING len must be a non-negative integer".into())
                })?;
                s = rest2[close + 1..].trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Substring(start, len));
                if starts_with_keyword(s, "AS") {
                    let after_as = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                        s = after_as[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            // Detect CAST(PAYLOAD_FIELD('f') AS INT|FLOAT|STRING)
            if su.starts_with("CAST(PAYLOAD_FIELD(") {
                let inner = &s["CAST(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in CAST".into())
                })?;
                let rest = rest.trim_start();
                if !starts_with_keyword(rest, "AS") {
                    return Err(SqlAggError::Unsupported(
                        "CAST requires AS INT|FLOAT|STRING".into(),
                    ));
                }
                let after_as = skip_keyword(rest, "AS").trim_start();
                let su_type = after_as.to_ascii_uppercase();
                let (cast_transform, after_type) = if su_type.starts_with("INT") {
                    (SqlFieldTransform::CastInt, &after_as[3..])
                } else if su_type.starts_with("FLOAT") {
                    (SqlFieldTransform::CastFloat, &after_as[5..])
                } else if su_type.starts_with("STRING") {
                    (SqlFieldTransform::CastString, &after_as[6..])
                } else {
                    return Err(SqlAggError::Unsupported(
                        "CAST target type must be INT, FLOAT, or STRING".into(),
                    ));
                };
                let s_after = after_type
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| SqlAggError::Unsupported("expected ')' to close CAST".into()))?;
                s = s_after.trim_start();
                field_transforms.insert(field.clone(), cast_transform);
                if starts_with_keyword(s, "AS") {
                    let after_as2 = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as2
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as2.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as2[..alias_end].to_string());
                        s = after_as2[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            // Detect NULLIF(PAYLOAD_FIELD('f'), 'val'), LPAD(...), RPAD(...)
            let su = s.to_ascii_uppercase();
            if su.starts_with("NULLIF(PAYLOAD_FIELD(") {
                let inner = &s["NULLIF(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in NULLIF".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| SqlAggError::Unsupported("expected ',' in NULLIF".into()))?
                    .trim_start();
                let after_open = rest.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("NULLIF comparison value must be single-quoted".into())
                })?;
                let close = after_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in NULLIF value".into())
                })?;
                let cmp_val = after_open[..close].to_string();
                let s_after = after_open[close + 1..]
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' to close NULLIF".into())
                    })?;
                s = s_after.trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::NullIf(cmp_val));
                if starts_with_keyword(s, "AS") {
                    let after_as = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                        s = after_as[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            if su.starts_with("LPAD(PAYLOAD_FIELD(") || su.starts_with("RPAD(PAYLOAD_FIELD(") {
                let is_lpad = su.starts_with("LPAD(");
                let prefix = if is_lpad {
                    "LPAD(PAYLOAD_FIELD("
                } else {
                    "RPAD(PAYLOAD_FIELD("
                };
                let inner = &s[prefix.len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in LPAD/RPAD".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected width after PAYLOAD_FIELD in LPAD/RPAD".into(),
                        )
                    })?
                    .trim_start();
                let comma2 = rest.find(',').ok_or_else(|| {
                    SqlAggError::Unsupported("LPAD/RPAD requires width and pad arguments".into())
                })?;
                let width: usize = rest[..comma2].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported(
                        "LPAD/RPAD width must be a non-negative integer".into(),
                    )
                })?;
                let rest2 = rest[comma2 + 1..].trim_start();
                let after_pad_open = rest2.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("LPAD/RPAD pad char must be single-quoted".into())
                })?;
                let pad_end = after_pad_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in LPAD/RPAD pad char".into())
                })?;
                let pad_str = after_pad_open[..pad_end].to_string();
                let s_after = after_pad_open[pad_end + 1..]
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' to close LPAD/RPAD".into())
                    })?;
                s = s_after.trim_start();
                let transform = if is_lpad {
                    SqlFieldTransform::Lpad(width, pad_str)
                } else {
                    SqlFieldTransform::Rpad(width, pad_str)
                };
                field_transforms.insert(field.clone(), transform);
                if starts_with_keyword(s, "AS") {
                    let after_as = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                        s = after_as[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            // Detect COALESCE(PAYLOAD_FIELD('f'), 'fallback') and REPLACE(PAYLOAD_FIELD('f'), 'from', 'to')
            let su = s.to_ascii_uppercase();
            if su.starts_with("COALESCE(PAYLOAD_FIELD(") {
                let inner = &s["COALESCE(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let after_pf_close =
                    after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected ')' after PAYLOAD_FIELD in COALESCE".into(),
                        )
                    })?;
                let after_comma = after_pf_close
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected ',' in COALESCE after PAYLOAD_FIELD".into(),
                        )
                    })?
                    .trim_start();
                let after_quote_open = after_comma.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported(
                        "COALESCE fallback must be a single-quoted string".into(),
                    )
                })?;
                let quote_end = after_quote_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in COALESCE fallback".into())
                })?;
                let fallback = after_quote_open[..quote_end].to_string();
                let after_fallback = &after_quote_open[quote_end + 1..];
                let s_after = after_fallback
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' to close COALESCE".into())
                    })?;
                s = s_after.trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Coalesce(fallback));
                if starts_with_keyword(s, "AS") {
                    let after_as = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                        s = after_as[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            if su.starts_with("REPLACE(PAYLOAD_FIELD(") {
                let inner = &s["REPLACE(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in REPLACE".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected ',' after PAYLOAD_FIELD in REPLACE".into(),
                        )
                    })?
                    .trim_start();
                let after_from_open = rest.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("REPLACE 'from' must be single-quoted".into())
                })?;
                let from_end = after_from_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in REPLACE 'from'".into())
                })?;
                let from_str = after_from_open[..from_end].to_string();
                let rest = after_from_open[from_end + 1..].trim_start();
                let rest = rest
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ',' between REPLACE args".into())
                    })?
                    .trim_start();
                let after_to_open = rest.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("REPLACE 'to' must be single-quoted".into())
                })?;
                let to_end = after_to_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in REPLACE 'to'".into())
                })?;
                let to_str = after_to_open[..to_end].to_string();
                let s_after = after_to_open[to_end + 1..]
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' to close REPLACE".into())
                    })?;
                s = s_after.trim_start();
                field_transforms
                    .insert(field.clone(), SqlFieldTransform::Replace(from_str, to_str));
                if starts_with_keyword(s, "AS") {
                    let after_as = skip_keyword(s, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end > 0 {
                        field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                        s = after_as[alias_end..].trim_start();
                    }
                }
                fields.push(field);
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }
            // Detect UPPER/LOWER/TRIM/ABS/ROUND/CEIL/FLOOR/LENGTH(PAYLOAD_FIELD('f')) wrappers
            let su = s.to_ascii_uppercase();
            let transform_prefixes: &[(&str, SqlFieldTransform)] = &[
                ("UPPER(PAYLOAD_FIELD(", SqlFieldTransform::Upper),
                ("LOWER(PAYLOAD_FIELD(", SqlFieldTransform::Lower),
                ("TRIM(PAYLOAD_FIELD(", SqlFieldTransform::Trim),
                ("ABS(PAYLOAD_FIELD(", SqlFieldTransform::Abs),
                ("ROUND(PAYLOAD_FIELD(", SqlFieldTransform::Round),
                ("CEIL(PAYLOAD_FIELD(", SqlFieldTransform::Ceil),
                ("FLOOR(PAYLOAD_FIELD(", SqlFieldTransform::Floor),
                ("LENGTH(PAYLOAD_FIELD(", SqlFieldTransform::Length),
            ];
            let transform = transform_prefixes
                .iter()
                .find(|(pfx, _)| su.starts_with(pfx))
                .map(|(pfx, t)| {
                    // strip the outer function name including its opening '('
                    s = &s[pfx.len() - "PAYLOAD_FIELD(".len()..];
                    t.clone()
                });
            let su = s.to_ascii_uppercase();
            if !su.starts_with("PAYLOAD_FIELD(") {
                return Err(SqlAggError::Unsupported(
                    "SELECT list must contain id or PAYLOAD_FIELD('name') expressions".into(),
                ));
            }
            let after_pf = &s["PAYLOAD_FIELD(".len()..];
            let (field, rest) = parse_quoted_sql(after_pf)?;
            let rest = rest.trim_start();
            if !rest.starts_with(')') {
                return Err(SqlAggError::Unsupported(
                    "expected ')' after PAYLOAD_FIELD argument".into(),
                ));
            }
            s = rest[1..].trim_start();
            // Consume closing ')' of the outer UPPER/LOWER/TRIM wrapper if present
            if transform.is_some() {
                if let Some(after_outer) = s.strip_prefix(')') {
                    s = after_outer.trim_start();
                }
            }
            if let Some(t) = transform {
                field_transforms.insert(field.clone(), t);
            }
            // Optional `AS alias` after PAYLOAD_FIELD('f') or its wrapper
            if starts_with_keyword(s, "AS") {
                let after_as = skip_keyword(s, "AS").trim_start();
                let alias_end = after_as
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after_as.len());
                if alias_end > 0 {
                    field_aliases.insert(field.clone(), after_as[..alias_end].to_string());
                    s = after_as[alias_end..].trim_start();
                }
            }
            fields.push(field);
            if s.starts_with(',') {
                s = &s[1..];
            }
        }

        if fields.is_empty() && !return_id {
            return Err(SqlAggError::Unsupported(
                "SELECT list must have at least one id or PAYLOAD_FIELD expression".into(),
            ));
        }

        s = skip_keyword(s, "FROM");
        let (namespace, rest) = parse_quoted(s)?;
        s = rest.trim_start();

        let (id_filter, payload_filters, payload_combinator) = if starts_with_keyword(s, "WHERE") {
            let after_where = skip_keyword(s, "WHERE").trim_start();
            let su = after_where.to_ascii_uppercase();
            if su.starts_with("PAYLOAD_FIELD(") {
                let (pfs, comb, rest2) = parse_payload_filter_chain(after_where)?;
                s = rest2.trim_start();
                (None, pfs, comb)
            } else {
                let (filter, rest2) = parse_where_id_filter(after_where)?;
                let rest2 = rest2.trim_start();
                // Optional AND/OR PAYLOAD_FIELD(...) chain after the id filter
                if starts_with_keyword(rest2, "AND") {
                    let after_and = skip_keyword(rest2, "AND").trim_start();
                    if after_and.to_ascii_uppercase().starts_with("PAYLOAD_FIELD(") {
                        let (pfs, comb, rem2) = parse_payload_filter_chain(after_and)?;
                        s = rem2.trim_start();
                        (Some(filter), pfs, comb)
                    } else {
                        s = rest2;
                        (Some(filter), vec![], SqlFilterCombinator::And)
                    }
                } else {
                    s = rest2;
                    (Some(filter), vec![], SqlFilterCombinator::And)
                }
            }
        } else {
            (None, vec![], SqlFilterCombinator::And)
        };

        let mut order_by: Vec<SqlProjectOrderBy> = Vec::new();
        if starts_with_keyword(s, "ORDER") {
            let after_order = skip_keyword(s, "ORDER").trim_start();
            if !starts_with_keyword(after_order, "BY") {
                return Err(SqlAggError::Unsupported("expected ORDER BY".into()));
            }
            let mut cur = skip_keyword(after_order, "BY").trim_start();
            loop {
                let su = cur.to_ascii_uppercase();
                let (field, rest2) = if starts_with_keyword(cur, "id") {
                    let rest2 = skip_keyword(cur, "id").trim_start();
                    ("id".to_string(), rest2)
                } else if su.starts_with("PAYLOAD_FIELD(") {
                    let after_pf = &cur["PAYLOAD_FIELD(".len()..];
                    let (f, r) = parse_quoted_sql(after_pf)?;
                    let r = r.trim_start();
                    if !r.starts_with(')') {
                        return Err(SqlAggError::Unsupported(
                            "expected ')' after PAYLOAD_FIELD in ORDER BY".into(),
                        ));
                    }
                    (f, r[1..].trim_start())
                } else {
                    return Err(SqlAggError::Unsupported(
                        "ORDER BY must be id or PAYLOAD_FIELD('field')".into(),
                    ));
                };
                // Detect ASC/DESC even when followed by comma (not just whitespace).
                let rest2_u = rest2.to_ascii_uppercase();
                let (direction, rest2) = if rest2_u.starts_with("DESC")
                    && (rest2.len() == 4
                        || !rest2
                            .as_bytes()
                            .get(4)
                            .is_some_and(|b| b.is_ascii_alphanumeric()))
                    && rest2.len() >= 4
                {
                    (AggOrderDirection::Desc, rest2[4..].trim_start())
                } else if rest2_u.starts_with("ASC")
                    && (rest2.len() == 3
                        || !rest2
                            .as_bytes()
                            .get(3)
                            .is_some_and(|b| b.is_ascii_alphanumeric()))
                    && rest2.len() >= 3
                {
                    (AggOrderDirection::Asc, rest2[3..].trim_start())
                } else {
                    (AggOrderDirection::Asc, rest2)
                };
                order_by.push(SqlProjectOrderBy { field, direction });
                cur = rest2.trim_start();
                if cur.starts_with(',') {
                    cur = cur[1..].trim_start();
                } else {
                    break;
                }
            }
            s = cur;
        }

        let offset = if starts_with_keyword(s, "OFFSET") {
            let after_offset = skip_keyword(s, "OFFSET").trim_start();
            let end = after_offset
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_offset.len());
            let n: usize = after_offset[..end].parse().map_err(|_| {
                SqlAggError::Unsupported("OFFSET must be a non-negative integer".into())
            })?;
            s = after_offset[end..].trim_start();
            Some(n)
        } else {
            None
        };

        let limit = if starts_with_keyword(s, "LIMIT") {
            let after_limit = skip_keyword(s, "LIMIT").trim_start();
            let end = after_limit
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after_limit.len());
            let n: usize = after_limit[..end]
                .parse()
                .map_err(|_| SqlAggError::Unsupported("LIMIT must be a positive integer".into()))?;
            s = after_limit[end..].trim_start();
            Some(n)
        } else {
            None
        };

        if !s.is_empty() {
            return Err(SqlAggError::Unsupported(format!(
                "trailing SQL not supported: {s}"
            )));
        }

        Ok(SqlProjectQuery {
            fields,
            field_aliases,
            field_transforms,
            namespace,
            return_id,
            distinct,
            id_filter,
            payload_filters,
            payload_combinator,
            order_by,
            offset,
            limit,
        })
    }
}

fn apply_sql_field_transform(val: serde_json::Value, t: &SqlFieldTransform) -> serde_json::Value {
    match t {
        SqlFieldTransform::Upper | SqlFieldTransform::Lower | SqlFieldTransform::Trim => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(match t {
                    SqlFieldTransform::Upper => s.to_uppercase(),
                    SqlFieldTransform::Lower => s.to_lowercase(),
                    SqlFieldTransform::Trim => s.trim().to_string(),
                    _ => unreachable!(),
                })
            } else {
                val
            }
        }
        SqlFieldTransform::Length => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::Number((s.chars().count() as u64).into())
            } else {
                val
            }
        }
        SqlFieldTransform::Abs
        | SqlFieldTransform::Round
        | SqlFieldTransform::Ceil
        | SqlFieldTransform::Floor => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    let result = match t {
                        SqlFieldTransform::Abs => f.abs(),
                        SqlFieldTransform::Round => f.round(),
                        SqlFieldTransform::Ceil => f.ceil(),
                        SqlFieldTransform::Floor => f.floor(),
                        _ => unreachable!(),
                    };
                    if let Some(n) = serde_json::Number::from_f64(result) {
                        return serde_json::Value::Number(n);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Coalesce(fallback) => {
            if val.is_null() {
                serde_json::Value::String(fallback.clone())
            } else {
                val
            }
        }
        SqlFieldTransform::Replace(from, to) => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.replace(from.as_str(), to.as_str()))
            } else {
                val
            }
        }
        SqlFieldTransform::Substring(start, len) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let begin = (*start).min(chars.len());
                let end = (begin + len).min(chars.len());
                serde_json::Value::String(chars[begin..end].iter().collect())
            } else {
                val
            }
        }
        SqlFieldTransform::CastInt => match &val {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    serde_json::Value::Number(i.into())
                } else if let Some(f) = n.as_f64() {
                    serde_json::Value::Number((f as i64).into())
                } else {
                    val
                }
            }
            serde_json::Value::String(s) => {
                if let Ok(i) = s.parse::<i64>() {
                    serde_json::Value::Number(i.into())
                } else if let Ok(f) = s.parse::<f64>() {
                    serde_json::Value::Number((f as i64).into())
                } else {
                    val
                }
            }
            _ => val,
        },
        SqlFieldTransform::CastFloat => match &val {
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(val)
                } else {
                    val
                }
            }
            serde_json::Value::String(s) => {
                if let Ok(f) = s.parse::<f64>() {
                    serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(val)
                } else {
                    val
                }
            }
            _ => val,
        },
        SqlFieldTransform::CastString => match &val {
            serde_json::Value::Number(n) => serde_json::Value::String(n.to_string()),
            serde_json::Value::Bool(b) => serde_json::Value::String(b.to_string()),
            _ => val,
        },
        SqlFieldTransform::NullIf(cmp_val) => {
            if let serde_json::Value::String(s) = &val {
                if s == cmp_val {
                    return serde_json::Value::Null;
                }
            }
            val
        }
        SqlFieldTransform::Lpad(width, pad) => {
            if let serde_json::Value::String(s) = val {
                let pad_char = pad.chars().next().unwrap_or(' ');
                let char_count = s.chars().count();
                if char_count >= *width {
                    serde_json::Value::String(s)
                } else {
                    let needed = width - char_count;
                    let padding: String = std::iter::repeat_n(pad_char, needed).collect();
                    serde_json::Value::String(format!("{padding}{s}"))
                }
            } else {
                val
            }
        }
        SqlFieldTransform::Rpad(width, pad) => {
            if let serde_json::Value::String(s) = val {
                let pad_char = pad.chars().next().unwrap_or(' ');
                let char_count = s.chars().count();
                if char_count >= *width {
                    serde_json::Value::String(s)
                } else {
                    let needed = width - char_count;
                    let padding: String = std::iter::repeat_n(pad_char, needed).collect();
                    serde_json::Value::String(format!("{s}{padding}"))
                }
            } else {
                val
            }
        }
    }
}

fn project_payload_field(payload: Option<&[u8]>, field: &str) -> serde_json::Value {
    let Some(bytes) = payload else {
        return serde_json::Value::Null;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return serde_json::Value::Null;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return serde_json::Value::Null;
    };
    v.get(field).cloned().unwrap_or(serde_json::Value::Null)
}

impl BinderStore {
    /// Execute a columnar projection SQL string.
    ///
    /// Returns one `HashMap<field, value>` per matching observation. Fields
    /// absent from an observation's JSON payload appear as `null`.
    pub fn sql_project(
        &self,
        sql: &str,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>, StoreError> {
        let q = SqlProjectQuery::parse(sql).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.sql_project_parsed(&q)
    }

    /// Execute a pre-parsed columnar projection query.
    pub fn sql_project_parsed(
        &self,
        q: &SqlProjectQuery,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>, StoreError> {
        let versions = self.list(&q.namespace)?;

        let ns_prefix = format!("{}:", q.namespace);
        let iter = versions.iter().filter(|v| {
            if let Some(ref f) = q.id_filter {
                let id = v
                    .object_id
                    .0
                    .strip_prefix(ns_prefix.as_str())
                    .unwrap_or(&v.object_id.0);
                let ok = match f {
                    SqlIdFilter::StartsWith(s) => id.starts_with(s.as_str()),
                    SqlIdFilter::EndsWith(s) => id.ends_with(s.as_str()),
                    SqlIdFilter::Contains(s) => id.contains(s.as_str()),
                    SqlIdFilter::Equals(s) => id == s.as_str(),
                    SqlIdFilter::In(vs) => vs.iter().any(|s| id == s.as_str()),
                    SqlIdFilter::NotIn(vs) => vs.iter().all(|s| id != s.as_str()),
                };
                if !ok {
                    return false;
                }
            }
            let payload_ok = match q.payload_combinator {
                SqlFilterCombinator::And => q
                    .payload_filters
                    .iter()
                    .all(|pf| apply_payload_filter(pf, v.payload.as_deref())),
                SqlFilterCombinator::Or => q
                    .payload_filters
                    .iter()
                    .any(|pf| apply_payload_filter(pf, v.payload.as_deref())),
            };
            if !payload_ok {
                return false;
            }
            true
        });

        // When ORDER BY references a field not in SELECT, we need to project it
        // Collect sort-key fields that aren't in SELECT (need temporary projection).
        let sort_keys_extra: Vec<String> = q
            .order_by
            .iter()
            .filter(|ob| ob.field != "id" && !q.fields.contains(&ob.field))
            .map(|ob| ob.field.clone())
            .collect();

        let return_id = q.return_id;
        let field_aliases = q.field_aliases.clone();
        let field_transforms = q.field_transforms.clone();
        let mut rows: Vec<std::collections::HashMap<String, serde_json::Value>> = iter
            .map(|v| {
                let mut row: std::collections::HashMap<String, serde_json::Value> = q
                    .fields
                    .iter()
                    .map(|field| {
                        let raw = project_payload_field(v.payload.as_deref(), field);
                        let val = if let Some(t) = field_transforms.get(field) {
                            apply_sql_field_transform(raw, t)
                        } else {
                            raw
                        };
                        let out_key = field_aliases
                            .get(field)
                            .cloned()
                            .unwrap_or_else(|| field.clone());
                        (out_key, val)
                    })
                    .collect();
                if return_id {
                    row.insert(
                        "id".to_string(),
                        serde_json::Value::String(v.object_id.0.clone()),
                    );
                }
                for sk in &sort_keys_extra {
                    row.insert(sk.clone(), project_payload_field(v.payload.as_deref(), sk));
                }
                row
            })
            .collect();

        if !q.order_by.is_empty() {
            rows.sort_by(|a, b| {
                for ob in &q.order_by {
                    let av = a.get(&ob.field).cloned().unwrap_or(serde_json::Value::Null);
                    let bv = b.get(&ob.field).cloned().unwrap_or(serde_json::Value::Null);
                    let ord = compare_json_for_sort(&av, &bv);
                    let ord = if ob.direction == AggOrderDirection::Desc {
                        ord.reverse()
                    } else {
                        ord
                    };
                    if ord != std::cmp::Ordering::Equal {
                        return ord;
                    }
                }
                std::cmp::Ordering::Equal
            });
        }

        // Remove temporary sort keys not in original SELECT list.
        for sk in &sort_keys_extra {
            for row in &mut rows {
                row.remove(sk);
            }
        }

        // DISTINCT: remove duplicate rows (identical projected field values).
        if q.distinct {
            let mut seen = std::collections::HashSet::new();
            rows.retain(|row| {
                // Use sorted-key JSON serialisation as the dedup key.
                let mut pairs: Vec<_> = row.iter().collect();
                pairs.sort_by_key(|(k, _)| k.as_str());
                let key = pairs
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(",");
                seen.insert(key)
            });
        }

        if let Some(n) = q.offset {
            if n >= rows.len() {
                rows.clear();
            } else {
                rows.drain(..n);
            }
        }

        if let Some(limit) = q.limit {
            rows.truncate(limit);
        }

        Ok(rows)
    }
}

fn compare_json_for_sort(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    match (a, b) {
        (serde_json::Value::Number(na), serde_json::Value::Number(nb)) => {
            let fa = na.as_f64().unwrap_or(f64::NAN);
            let fb = nb.as_f64().unwrap_or(f64::NAN);
            fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
        }
        (serde_json::Value::String(sa), serde_json::Value::String(sb)) => sa.cmp(sb),
        (serde_json::Value::Null, serde_json::Value::Null) => std::cmp::Ordering::Equal,
        (serde_json::Value::Null, _) => std::cmp::Ordering::Greater,
        (_, serde_json::Value::Null) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
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
        assert_eq!(q.id_filter, None);
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
        assert_eq!(q.id_filter, None);
    }

    #[test]
    fn parses_where_id_filter() {
        let q = SqlAggregateQuery::parse(
            "SELECT AVG(VALUE) FROM 'obs://sleep' WHERE id STARTS WITH '2024'",
        )
        .unwrap();
        assert_eq!(q.aggregate, Aggregate::Avg);
        assert_eq!(q.namespace, "obs://sleep");
        assert_eq!(q.id_filter, Some(SqlIdFilter::StartsWith("2024".into())));
        assert_eq!(q.group_by, SqlGroupBy::None);
    }

    #[test]
    fn parses_where_then_group_by() {
        let q = SqlAggregateQuery::parse(
            "SELECT COUNT(*) FROM 'obs://sleep' WHERE id STARTS WITH '2024' GROUP BY PATH_SEGMENT(1)",
        )
        .unwrap();
        assert_eq!(q.id_filter, Some(SqlIdFilter::StartsWith("2024".into())));
        assert_eq!(q.group_by, SqlGroupBy::PathSegment(1));
    }

    #[test]
    fn rejects_join() {
        assert!(SqlAggregateQuery::parse("SELECT COUNT(*) FROM 'obs://a' JOIN 'obs://b'").is_err());
    }
}
