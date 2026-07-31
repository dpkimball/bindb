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
    // COLLECT(PAYLOAD_FIELD('fieldname')) — variable field name, handled before the table
    if u.starts_with("COLLECT(PAYLOAD_FIELD(") {
        let after_pf = &s["COLLECT(PAYLOAD_FIELD(".len()..];
        let (field, rest) = parse_quoted_sql(after_pf)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(')').ok_or_else(|| {
            SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in COLLECT".into())
        })?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix(')')
            .ok_or_else(|| SqlAggError::Unsupported("expected ')' closing COLLECT(...)".into()))?;
        return Ok((Aggregate::Collect(field), false, rest));
    }
    // STRING_AGG(PAYLOAD_FIELD('fieldname'), 'sep')
    if u.starts_with("STRING_AGG(PAYLOAD_FIELD(") {
        let after_pf = &s["STRING_AGG(PAYLOAD_FIELD(".len()..];
        let (field, rest) = parse_quoted_sql(after_pf)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(')').ok_or_else(|| {
            SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in STRING_AGG".into())
        })?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(',').ok_or_else(|| {
            SqlAggError::Unsupported("expected ',' before separator in STRING_AGG".into())
        })?;
        let rest = rest.trim_start();
        let (sep, rest) = parse_quoted_sql(rest).map_err(|_| {
            SqlAggError::Unsupported("STRING_AGG separator must be a quoted string".into())
        })?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(')').ok_or_else(|| {
            SqlAggError::Unsupported("expected ')' closing STRING_AGG(...)".into())
        })?;
        return Ok((Aggregate::StringAgg { field, sep }, false, rest));
    }
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
            return Ok((agg.clone(), *needs, &s[pat.len()..]));
        }
    }
    Err(SqlAggError::Unsupported(
        "aggregate must be COUNT(*), AVG/SUM/MIN/MAX/P50/P99(VALUE), COLLECT(PAYLOAD_FIELD('field')), or STRING_AGG(PAYLOAD_FIELD('field'), 'sep')".into(),
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
            .aggregate(q.aggregate.clone());

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
#[derive(Debug, Clone, PartialEq)]
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
    /// `REVERSE(PAYLOAD_FIELD('f'))` — reverse the characters of a string field.
    Reverse,
    /// `INITCAP(PAYLOAD_FIELD('f'))` — capitalise the first letter of each whitespace-delimited word.
    InitCap,
    /// `REPEAT(PAYLOAD_FIELD('f'), n)` — repeat the string value `n` times.
    Repeat(usize),
    /// `LEFT(PAYLOAD_FIELD('f'), n)` — first `n` characters of a string field.
    Left(usize),
    /// `RIGHT(PAYLOAD_FIELD('f'), n)` — last `n` characters of a string field.
    Right(usize),
    /// `SIGN(PAYLOAD_FIELD('f'))` — sign of a numeric field: -1, 0, or 1.
    Sign,
    /// `SQRT(PAYLOAD_FIELD('f'))` — square root of a non-negative numeric field.
    Sqrt,
    /// `POWER(PAYLOAD_FIELD('f'), exp)` — raise numeric field to power `exp`.
    Power(f64),
    /// `MOD(PAYLOAD_FIELD('f'), n)` — numeric field modulo `n`.
    Mod(f64),
    /// `STRPOS(PAYLOAD_FIELD('f'), 'needle')` — 1-based char position of needle; 0 if not found.
    Strpos(String),
    /// `SPLIT_PART(PAYLOAD_FIELD('f'), 'delim', n)` — nth (1-based) token split by delimiter.
    SplitPart(String, usize),
    /// `LTRIM(PAYLOAD_FIELD('f'))` — remove leading whitespace from a string field.
    Ltrim,
    /// `RTRIM(PAYLOAD_FIELD('f'))` — remove trailing whitespace from a string field.
    Rtrim,
    /// `LOG(PAYLOAD_FIELD('f'))` — natural logarithm of a positive numeric field.
    Log,
    /// `EXP(PAYLOAD_FIELD('f'))` — e raised to the power of a numeric field.
    Exp,
    /// `LOG2(PAYLOAD_FIELD('f'))` — base-2 logarithm of a positive numeric field.
    Log2,
    /// `LOG10(PAYLOAD_FIELD('f'))` — base-10 logarithm of a positive numeric field.
    Log10,
    /// `RADIANS(PAYLOAD_FIELD('f'))` — convert a numeric field from degrees to radians.
    Radians,
    /// `DEGREES(PAYLOAD_FIELD('f'))` — convert a numeric field from radians to degrees.
    Degrees,
    /// `SIN(PAYLOAD_FIELD('f'))` — sine of a numeric field (radians).
    Sin,
    /// `COS(PAYLOAD_FIELD('f'))` — cosine of a numeric field (radians).
    Cos,
    /// `TAN(PAYLOAD_FIELD('f'))` — tangent of a numeric field (radians).
    Tan,
    /// `ATAN(PAYLOAD_FIELD('f'))` — arc tangent of a numeric field; result in [-π/2, π/2].
    Atan,
    /// `ASIN(PAYLOAD_FIELD('f'))` — arc sine of a numeric field in [-1, 1]; out-of-domain passes through.
    Asin,
    /// `ACOS(PAYLOAD_FIELD('f'))` — arc cosine of a numeric field in [-1, 1]; out-of-domain passes through.
    Acos,
    /// `TRUNC(PAYLOAD_FIELD('f'))` — truncate toward zero (differs from FLOOR for negatives).
    Trunc,
    /// `CBRT(PAYLOAD_FIELD('f'))` — cube root of a numeric field.
    Cbrt,
    /// `ARRAY_LENGTH(PAYLOAD_FIELD('f'))` — number of elements in a JSON array field.
    /// Non-array values pass through unchanged.
    ArrayLength,
    /// `CASE PAYLOAD_FIELD('f') WHEN 'v1' THEN 'out1' [WHEN …] [ELSE 'default'] END`
    /// — value-match conditional; returns first matching output or `else_val` (empty string if no ELSE).
    CaseWhen {
        branches: Vec<(String, String)>,
        else_val: String,
    },
}

/// Sort direction for `ORDER BY` in a projection query.
pub use AggOrderDirection as SqlProjectDirection;

/// `ORDER BY PAYLOAD_FIELD('field') [ASC|DESC]` sort clause for `SqlProjectQuery`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlProjectOrderBy {
    pub field: String,
    pub direction: AggOrderDirection,
}

/// Window function kind for `OVER (...)` clauses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlWindowFunc {
    /// `ROW_NUMBER()` — sequential 1-based row number within the partition.
    RowNumber,
    /// `RANK()` — rank with gaps for ties (1, 2, 2, 4).
    Rank,
    /// `DENSE_RANK()` — rank without gaps for ties (1, 2, 2, 3).
    DenseRank,
    /// `LAG(PAYLOAD_FIELD('f'), n)` — payload field value from n rows before in the partition.
    Lag { field: String, offset: usize },
    /// `LEAD(PAYLOAD_FIELD('f'), n)` — payload field value from n rows after in the partition.
    Lead { field: String, offset: usize },
    /// `SUM(PAYLOAD_FIELD('f'))` — sum of field; running if ORDER BY present, full-partition otherwise.
    Sum { field: String },
    /// `AVG(PAYLOAD_FIELD('f'))` — average of field; running if ORDER BY present.
    Avg { field: String },
    /// `COUNT(*)` — row count; running if ORDER BY present, full-partition otherwise.
    Count,
    /// `MIN(PAYLOAD_FIELD('f'))` — minimum of field; running if ORDER BY present.
    Min { field: String },
    /// `MAX(PAYLOAD_FIELD('f'))` — maximum of field; running if ORDER BY present.
    Max { field: String },
    /// `NTILE(n)` — divides the partition into n equal-sized buckets; assigns 1-based bucket number.
    Ntile { buckets: usize },
    /// `FIRST_VALUE(PAYLOAD_FIELD('f'))` — value of field from the first row in the partition window.
    FirstValue { field: String },
    /// `LAST_VALUE(PAYLOAD_FIELD('f'))` — value of field from the last row in the partition window.
    LastValue { field: String },
    /// `PERCENT_RANK()` — relative rank: (rank−1)/(n−1); 0.0 for the first row, 1.0 for the last.
    PercentRank,
    /// `CUME_DIST()` — cumulative distribution: fraction of rows with ORDER BY key ≤ current row.
    CumeDist,
    /// `NTH_VALUE(PAYLOAD_FIELD('f'), n)` — value of field from the nth row (1-based) in the partition.
    NthValue { field: String, n: usize },
}

/// A window column: `FUNC() OVER (PARTITION BY … ORDER BY …) AS alias`.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlWindowColumn {
    pub func: SqlWindowFunc,
    /// Payload field names to partition by (empty = single global partition).
    pub partition_by: Vec<String>,
    /// Sort order within each partition for rank/numbering.
    pub order_by: Vec<SqlProjectOrderBy>,
    /// Output column name in each result row.
    pub alias: String,
}

/// Operand for an arithmetic computed column in SELECT.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlNumArg {
    Field(String),
    Literal(f64),
}

/// Binary arithmetic operator for a computed SELECT column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlNumBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// One part of a string-concatenation computed column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlStrPart {
    /// Value comes from a JSON payload field.
    Field(String),
    /// A string literal.
    Lit(String),
}

/// One `WHEN … THEN …` branch of a searched CASE WHEN expression.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlSearchedCaseBranch {
    pub filter: SqlPayloadFilter,
    pub output: String,
}

/// A computed SELECT column whose value is derived from an expression.
///
/// Computed columns are evaluated after regular `PAYLOAD_FIELD` projections and written
/// into the row under the `alias` key.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlComputedColumn {
    /// `PAYLOAD_FIELD('a') + PAYLOAD_FIELD('b') AS total` — binary arithmetic on two operands.
    NumBin {
        op: SqlNumBinOp,
        left: SqlNumArg,
        right: SqlNumArg,
        alias: String,
    },
    /// `PAYLOAD_FIELD('f1') || 'sep' || PAYLOAD_FIELD('f2') AS name` — string concat chain.
    StrConcat {
        parts: Vec<SqlStrPart>,
        alias: String,
    },
    /// `CASE WHEN PAYLOAD_FIELD('f') > 5 THEN 'hi' ELSE 'lo' END AS tier` — searched CASE WHEN.
    SearchedCase {
        branches: Vec<SqlSearchedCaseBranch>,
        else_val: String,
        alias: String,
    },
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
    /// Window function columns (`ROW_NUMBER()`, `RANK()`, `DENSE_RANK()` OVER clauses).
    /// Computed against the full filtered result set before OFFSET/LIMIT.
    pub window_columns: Vec<SqlWindowColumn>,
    /// Arithmetic, string-concat, and searched-CASE-WHEN columns computed after regular projection.
    pub computed_cols: Vec<SqlComputedColumn>,
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
        let (raw, named_windows) = pre_parse_window_clause(raw)?;
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
        let mut window_columns: Vec<SqlWindowColumn> = Vec::new();
        let mut computed_cols: Vec<SqlComputedColumn> = Vec::new();
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
            // Detect window functions: ROW_NUMBER(), RANK(), DENSE_RANK(), LAG(), LEAD() OVER (...)
            let su = s.to_ascii_uppercase();
            let wf_pair: Option<(SqlWindowFunc, &str)> = if su.starts_with("LAG(PAYLOAD_FIELD(") {
                parse_lag_lead_func(true, s)
            } else if su.starts_with("LEAD(PAYLOAD_FIELD(") {
                parse_lag_lead_func(false, s)
            } else if su.starts_with("SUM(PAYLOAD_FIELD(") {
                parse_single_field_window_func("SUM", s)
                    .map(|(f, r)| (SqlWindowFunc::Sum { field: f }, r))
            } else if su.starts_with("AVG(PAYLOAD_FIELD(") {
                parse_single_field_window_func("AVG", s)
                    .map(|(f, r)| (SqlWindowFunc::Avg { field: f }, r))
            } else if su.starts_with("MIN(PAYLOAD_FIELD(") {
                parse_single_field_window_func("MIN", s)
                    .map(|(f, r)| (SqlWindowFunc::Min { field: f }, r))
            } else if su.starts_with("MAX(PAYLOAD_FIELD(") {
                parse_single_field_window_func("MAX", s)
                    .map(|(f, r)| (SqlWindowFunc::Max { field: f }, r))
            } else if su.starts_with("COUNT(*)") {
                Some((SqlWindowFunc::Count, &s["COUNT(*)".len()..]))
            } else if su.starts_with("ROW_NUMBER()") {
                Some((SqlWindowFunc::RowNumber, &s["ROW_NUMBER()".len()..]))
            } else if su.starts_with("DENSE_RANK()") {
                Some((SqlWindowFunc::DenseRank, &s["DENSE_RANK()".len()..]))
            } else if su.starts_with("RANK()") {
                Some((SqlWindowFunc::Rank, &s["RANK()".len()..]))
            } else if su.starts_with("NTILE(") {
                parse_ntile_func(s)
            } else if su.starts_with("FIRST_VALUE(PAYLOAD_FIELD(") {
                parse_single_field_window_func("FIRST_VALUE", s)
                    .map(|(f, r)| (SqlWindowFunc::FirstValue { field: f }, r))
            } else if su.starts_with("LAST_VALUE(PAYLOAD_FIELD(") {
                parse_single_field_window_func("LAST_VALUE", s)
                    .map(|(f, r)| (SqlWindowFunc::LastValue { field: f }, r))
            } else if su.starts_with("PERCENT_RANK()") {
                Some((SqlWindowFunc::PercentRank, &s["PERCENT_RANK()".len()..]))
            } else if su.starts_with("CUME_DIST()") {
                Some((SqlWindowFunc::CumeDist, &s["CUME_DIST()".len()..]))
            } else if su.starts_with("NTH_VALUE(PAYLOAD_FIELD(") {
                parse_nth_value_func(s)
            } else {
                None
            };
            if let Some((func, after_func)) = wf_pair {
                let after_func = after_func.trim_start();
                if !starts_with_keyword(after_func, "OVER") {
                    return Err(SqlAggError::Unsupported(
                        "expected OVER after window function".into(),
                    ));
                }
                let after_over = skip_keyword(after_func, "OVER").trim_start();
                let (partition_by, win_order_by, after_close) = if after_over.starts_with('(') {
                    parse_over_clause(after_over)?
                } else {
                    // Named window reference: OVER window_name
                    let id_end = after_over
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_over.len());
                    if id_end == 0 {
                        return Err(SqlAggError::Unsupported(
                            "expected '(' or window name after OVER".into(),
                        ));
                    }
                    let wname = &after_over[..id_end];
                    let (pb, ob) = named_windows.get(wname).ok_or_else(|| {
                        SqlAggError::Unsupported(format!("unknown window name: {wname}"))
                    })?;
                    (pb.clone(), ob.clone(), &after_over[id_end..])
                };
                let after_close = after_close.trim_start();
                let (alias, rest_after_alias) = if starts_with_keyword(after_close, "AS") {
                    let after_as = skip_keyword(after_close, "AS").trim_start();
                    let alias_end = after_as
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(after_as.len());
                    if alias_end == 0 {
                        return Err(SqlAggError::Unsupported(
                            "expected alias name after AS in window function".into(),
                        ));
                    }
                    (after_as[..alias_end].to_string(), &after_as[alias_end..])
                } else {
                    let default = match &func {
                        SqlWindowFunc::RowNumber => "row_number",
                        SqlWindowFunc::Rank => "rank",
                        SqlWindowFunc::DenseRank => "dense_rank",
                        SqlWindowFunc::Lag { .. } => "lag",
                        SqlWindowFunc::Lead { .. } => "lead",
                        SqlWindowFunc::Sum { .. } => "sum",
                        SqlWindowFunc::Avg { .. } => "avg",
                        SqlWindowFunc::Count => "count",
                        SqlWindowFunc::Min { .. } => "min",
                        SqlWindowFunc::Max { .. } => "max",
                        SqlWindowFunc::Ntile { .. } => "ntile",
                        SqlWindowFunc::FirstValue { .. } => "first_value",
                        SqlWindowFunc::LastValue { .. } => "last_value",
                        SqlWindowFunc::PercentRank => "percent_rank",
                        SqlWindowFunc::CumeDist => "cume_dist",
                        SqlWindowFunc::NthValue { .. } => "nth_value",
                    };
                    (default.to_string(), after_close)
                };
                window_columns.push(SqlWindowColumn {
                    func,
                    partition_by,
                    order_by: win_order_by,
                    alias,
                });
                s = rest_after_alias.trim_start();
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
            // Detect REPEAT(PAYLOAD_FIELD('f'), n)
            let su = s.to_ascii_uppercase();
            if su.starts_with("REPEAT(PAYLOAD_FIELD(") {
                let inner = &s["REPEAT(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in REPEAT".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected count after PAYLOAD_FIELD in REPEAT".into(),
                        )
                    })?
                    .trim_start();
                let paren_end = rest.find(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' to close REPEAT".into())
                })?;
                let count: usize = rest[..paren_end].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported("REPEAT count must be a non-negative integer".into())
                })?;
                s = rest[paren_end + 1..].trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Repeat(count));
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
            // Detect POWER(PAYLOAD_FIELD('f'), exp)
            let su = s.to_ascii_uppercase();
            if su.starts_with("POWER(PAYLOAD_FIELD(") {
                let inner = &s["POWER(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in POWER".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected exponent after PAYLOAD_FIELD in POWER".into(),
                        )
                    })?
                    .trim_start();
                let paren_end = rest.find(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' to close POWER".into())
                })?;
                let exp: f64 = rest[..paren_end].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported("POWER exponent must be a number".into())
                })?;
                s = rest[paren_end + 1..].trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Power(exp));
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
            // Detect MOD(PAYLOAD_FIELD('f'), n)
            let su = s.to_ascii_uppercase();
            if su.starts_with("MOD(PAYLOAD_FIELD(") {
                let inner = &s["MOD(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in MOD".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected divisor after PAYLOAD_FIELD in MOD".into(),
                        )
                    })?
                    .trim_start();
                let paren_end = rest
                    .find(')')
                    .ok_or_else(|| SqlAggError::Unsupported("expected ')' to close MOD".into()))?;
                let n: f64 = rest[..paren_end]
                    .trim()
                    .parse()
                    .map_err(|_| SqlAggError::Unsupported("MOD divisor must be a number".into()))?;
                s = rest[paren_end + 1..].trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Mod(n));
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
            // Detect STRPOS(PAYLOAD_FIELD('f'), 'needle')
            let su = s.to_ascii_uppercase();
            if su.starts_with("STRPOS(PAYLOAD_FIELD(") {
                let inner = &s["STRPOS(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in STRPOS".into())
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected needle after PAYLOAD_FIELD in STRPOS".into(),
                        )
                    })?
                    .trim_start();
                let after_quote_open = rest.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("STRPOS needle must be single-quoted".into())
                })?;
                let quote_end = after_quote_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in STRPOS needle".into())
                })?;
                let needle = after_quote_open[..quote_end].to_string();
                let s_after = after_quote_open[quote_end + 1..]
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' to close STRPOS".into())
                    })?;
                s = s_after.trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::Strpos(needle));
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
            // Detect SPLIT_PART(PAYLOAD_FIELD('f'), 'delim', n)
            let su = s.to_ascii_uppercase();
            if su.starts_with("SPLIT_PART(PAYLOAD_FIELD(") {
                let inner = &s["SPLIT_PART(PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported(
                        "expected ')' after PAYLOAD_FIELD in SPLIT_PART".into(),
                    )
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected delimiter after PAYLOAD_FIELD in SPLIT_PART".into(),
                        )
                    })?
                    .trim_start();
                let after_delim_open = rest.strip_prefix('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("SPLIT_PART delimiter must be single-quoted".into())
                })?;
                let delim_end = after_delim_open.find('\'').ok_or_else(|| {
                    SqlAggError::Unsupported("unclosed quote in SPLIT_PART delimiter".into())
                })?;
                let delim = after_delim_open[..delim_end].to_string();
                let rest2 = after_delim_open[delim_end + 1..]
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ',' after SPLIT_PART delimiter".into())
                    })?
                    .trim_start();
                let paren_end = rest2.find(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' to close SPLIT_PART".into())
                })?;
                let n: usize = rest2[..paren_end].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported(
                        "SPLIT_PART index must be a non-negative integer".into(),
                    )
                })?;
                s = rest2[paren_end + 1..].trim_start();
                field_transforms.insert(field.clone(), SqlFieldTransform::SplitPart(delim, n));
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
            // Detect LEFT(PAYLOAD_FIELD('f'), n) / RIGHT(PAYLOAD_FIELD('f'), n)
            let su = s.to_ascii_uppercase();
            if su.starts_with("LEFT(PAYLOAD_FIELD(") || su.starts_with("RIGHT(PAYLOAD_FIELD(") {
                let is_left = su.starts_with("LEFT(");
                let prefix = if is_left {
                    "LEFT(PAYLOAD_FIELD("
                } else {
                    "RIGHT(PAYLOAD_FIELD("
                };
                let inner = &s[prefix.len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let rest = after_field.trim_start().strip_prefix(')').ok_or_else(|| {
                    SqlAggError::Unsupported(
                        "expected ')' after PAYLOAD_FIELD in LEFT/RIGHT".into(),
                    )
                })?;
                let rest = rest
                    .trim_start()
                    .strip_prefix(',')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported(
                            "expected count after PAYLOAD_FIELD in LEFT/RIGHT".into(),
                        )
                    })?
                    .trim_start();
                let paren_end = rest.find(')').ok_or_else(|| {
                    SqlAggError::Unsupported("expected ')' to close LEFT/RIGHT".into())
                })?;
                let n: usize = rest[..paren_end].trim().parse().map_err(|_| {
                    SqlAggError::Unsupported(
                        "LEFT/RIGHT count must be a non-negative integer".into(),
                    )
                })?;
                s = rest[paren_end + 1..].trim_start();
                let transform = if is_left {
                    SqlFieldTransform::Left(n)
                } else {
                    SqlFieldTransform::Right(n)
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
            // Detect CASE PAYLOAD_FIELD('f') WHEN 'v1' THEN 'out1' ... [ELSE 'default'] END [AS alias]
            let su = s.to_ascii_uppercase();
            if su.starts_with("CASE PAYLOAD_FIELD(") {
                let inner = &s["CASE PAYLOAD_FIELD(".len()..];
                let (field, after_field) = parse_quoted_sql(inner)?;
                let mut rest = after_field
                    .trim_start()
                    .strip_prefix(')')
                    .ok_or_else(|| {
                        SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in CASE".into())
                    })?
                    .trim_start();
                let mut branches: Vec<(String, String)> = Vec::new();
                loop {
                    if !rest.to_ascii_uppercase().starts_with("WHEN ") {
                        break;
                    }
                    rest = rest["WHEN ".len()..].trim_start();
                    let (mv, after_mv) = parse_quoted_sql(rest)?;
                    rest = after_mv.trim_start();
                    if !rest.to_ascii_uppercase().starts_with("THEN ") {
                        return Err(SqlAggError::Unsupported(
                            "expected THEN after WHEN value in CASE".into(),
                        ));
                    }
                    rest = rest["THEN ".len()..].trim_start();
                    let (ov, after_ov) = parse_quoted_sql(rest)?;
                    rest = after_ov.trim_start();
                    branches.push((mv, ov));
                }
                if branches.is_empty() {
                    return Err(SqlAggError::Unsupported(
                        "CASE requires at least one WHEN branch".into(),
                    ));
                }
                let else_val = if rest.to_ascii_uppercase().starts_with("ELSE ") {
                    rest = rest["ELSE ".len()..].trim_start();
                    let (ev, after_ev) = parse_quoted_sql(rest)?;
                    rest = after_ev.trim_start();
                    ev
                } else {
                    String::new()
                };
                if !rest.to_ascii_uppercase().starts_with("END") {
                    return Err(SqlAggError::Unsupported(
                        "expected END to close CASE".into(),
                    ));
                }
                s = rest["END".len()..].trim_start();
                field_transforms.insert(
                    field.clone(),
                    SqlFieldTransform::CaseWhen { branches, else_val },
                );
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
            // ── Computed columns ─────────────────────────────────────────────
            let su = s.to_ascii_uppercase();

            // Searched CASE WHEN PAYLOAD_FIELD('f') OP value THEN 'out' … END AS alias
            if su.starts_with("CASE WHEN PAYLOAD_FIELD(") {
                let mut case_rest = skip_keyword(s, "CASE").trim_start();
                let mut branches: Vec<SqlSearchedCaseBranch> = Vec::new();
                let mut else_val = String::new();
                loop {
                    case_rest = case_rest.trim_start();
                    let cu = case_rest.to_ascii_uppercase();
                    if starts_with_keyword(case_rest, "WHEN") {
                        case_rest = skip_keyword(case_rest, "WHEN").trim_start();
                        let (filter, after_filter) = parse_payload_filter(case_rest)?;
                        case_rest = after_filter.trim_start();
                        if !starts_with_keyword(case_rest, "THEN") {
                            return Err(SqlAggError::Unsupported(
                                "expected THEN after WHEN condition in CASE WHEN".into(),
                            ));
                        }
                        case_rest = skip_keyword(case_rest, "THEN").trim_start();
                        let (output, after_out) = parse_quoted_sql(case_rest)?;
                        case_rest = after_out.trim_start();
                        branches.push(SqlSearchedCaseBranch { filter, output });
                    } else if starts_with_keyword(case_rest, "ELSE") {
                        case_rest = skip_keyword(case_rest, "ELSE").trim_start();
                        let (ev, after_ev) = parse_quoted_sql(case_rest)?;
                        else_val = ev;
                        case_rest = after_ev.trim_start();
                    } else if cu.starts_with("END") {
                        case_rest = case_rest[3..].trim_start();
                        break;
                    } else {
                        return Err(SqlAggError::Unsupported(
                            "expected WHEN, ELSE, or END in CASE WHEN".into(),
                        ));
                    }
                }
                if branches.is_empty() {
                    return Err(SqlAggError::Unsupported(
                        "CASE WHEN requires at least one WHEN branch".into(),
                    ));
                }
                if !starts_with_keyword(case_rest, "AS") {
                    return Err(SqlAggError::Unsupported(
                        "CASE WHEN expression requires AS alias".into(),
                    ));
                }
                let after_as = skip_keyword(case_rest, "AS").trim_start();
                let alias_end = after_as
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after_as.len());
                if alias_end == 0 {
                    return Err(SqlAggError::Unsupported(
                        "empty alias after CASE WHEN … AS".into(),
                    ));
                }
                let alias = after_as[..alias_end].to_string();
                s = after_as[alias_end..].trim_start();
                computed_cols.push(SqlComputedColumn::SearchedCase {
                    branches,
                    else_val,
                    alias,
                });
                if s.starts_with(',') {
                    s = &s[1..];
                }
                continue;
            }

            // PAYLOAD_FIELD('f') op … AS alias — arithmetic (+,-,*,/,%) or concat (||)
            if su.starts_with("PAYLOAD_FIELD(") {
                let after_pf0 = &s["PAYLOAD_FIELD(".len()..];
                if let Ok((first_field, after_close0)) = parse_quoted_sql(after_pf0) {
                    if let Some(after_rparen0) = after_close0.trim_start().strip_prefix(')') {
                        let peek = after_rparen0.trim_start();

                        // String concat: PAYLOAD_FIELD('f') || ...
                        if peek.starts_with("||") {
                            let mut parts = vec![SqlStrPart::Field(first_field)];
                            let mut cur = peek.strip_prefix("||").unwrap_or_default().trim_start();
                            loop {
                                let cu = cur.to_ascii_uppercase();
                                if cu.starts_with("PAYLOAD_FIELD(") {
                                    let inner = &cur["PAYLOAD_FIELD(".len()..];
                                    let (f, rest2) = parse_quoted_sql(inner)?;
                                    let rest2 =
                                        rest2.trim_start().strip_prefix(')').ok_or_else(|| {
                                            SqlAggError::Unsupported(
                                                "expected ')' after PAYLOAD_FIELD in ||".into(),
                                            )
                                        })?;
                                    parts.push(SqlStrPart::Field(f));
                                    cur = rest2.trim_start();
                                } else if cur.starts_with('\'') {
                                    let end = cur[1..].find('\'').ok_or_else(|| {
                                        SqlAggError::Unsupported(
                                            "unclosed string literal in || concat".into(),
                                        )
                                    })?;
                                    let lit = cur[1..1 + end].to_string();
                                    parts.push(SqlStrPart::Lit(lit));
                                    cur = cur[1 + end + 1..].trim_start();
                                } else {
                                    break;
                                }
                                if cur.starts_with("||") {
                                    cur = cur[2..].trim_start();
                                } else {
                                    break;
                                }
                            }
                            if !starts_with_keyword(cur, "AS") {
                                return Err(SqlAggError::Unsupported(
                                    "|| string concat requires AS alias".into(),
                                ));
                            }
                            let after_as = skip_keyword(cur, "AS").trim_start();
                            let alias_end = after_as
                                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .unwrap_or(after_as.len());
                            if alias_end == 0 {
                                return Err(SqlAggError::Unsupported(
                                    "empty alias after || concat AS".into(),
                                ));
                            }
                            let alias = after_as[..alias_end].to_string();
                            s = after_as[alias_end..].trim_start();
                            computed_cols.push(SqlComputedColumn::StrConcat { parts, alias });
                            if s.starts_with(',') {
                                s = &s[1..];
                            }
                            continue;
                        }

                        // Arithmetic: PAYLOAD_FIELD('f') +/-/*//%  rhs AS alias
                        let op_opt: Option<(SqlNumBinOp, usize)> =
                            if peek.starts_with('+') && !peek.starts_with("||") {
                                Some((SqlNumBinOp::Add, 1))
                            } else if peek.starts_with('-') {
                                Some((SqlNumBinOp::Sub, 1))
                            } else if peek.starts_with('*') {
                                Some((SqlNumBinOp::Mul, 1))
                            } else if peek.starts_with('/') {
                                Some((SqlNumBinOp::Div, 1))
                            } else if peek.starts_with('%') {
                                Some((SqlNumBinOp::Rem, 1))
                            } else {
                                None
                            };

                        if let Some((op, op_len)) = op_opt {
                            let left = SqlNumArg::Field(first_field);
                            let after_op = peek[op_len..].trim_start();
                            let aou = after_op.to_ascii_uppercase();
                            let (right, after_right): (SqlNumArg, &str) = if aou
                                .starts_with("PAYLOAD_FIELD(")
                            {
                                let inner = &after_op["PAYLOAD_FIELD(".len()..];
                                let (f, rest2) = parse_quoted_sql(inner)?;
                                let rest2 =
                                    rest2.trim_start().strip_prefix(')').ok_or_else(|| {
                                        SqlAggError::Unsupported(
                                            "expected ')' after PAYLOAD_FIELD in arithmetic".into(),
                                        )
                                    })?;
                                (SqlNumArg::Field(f), rest2.trim_start())
                            } else {
                                let lit_end = after_op
                                    .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                                    .unwrap_or(after_op.len());
                                if lit_end == 0 {
                                    return Err(SqlAggError::Unsupported(
                                        "expected PAYLOAD_FIELD or number literal in arithmetic"
                                            .into(),
                                    ));
                                }
                                let n: f64 = after_op[..lit_end].parse().map_err(|_| {
                                    SqlAggError::Unsupported(
                                        "invalid number in arithmetic expression".into(),
                                    )
                                })?;
                                (SqlNumArg::Literal(n), after_op[lit_end..].trim_start())
                            };
                            if !starts_with_keyword(after_right, "AS") {
                                return Err(SqlAggError::Unsupported(
                                    "arithmetic expression requires AS alias".into(),
                                ));
                            }
                            let after_as = skip_keyword(after_right, "AS").trim_start();
                            let alias_end = after_as
                                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .unwrap_or(after_as.len());
                            if alias_end == 0 {
                                return Err(SqlAggError::Unsupported(
                                    "empty alias after arithmetic AS".into(),
                                ));
                            }
                            let alias = after_as[..alias_end].to_string();
                            s = after_as[alias_end..].trim_start();
                            computed_cols.push(SqlComputedColumn::NumBin {
                                op,
                                left,
                                right,
                                alias,
                            });
                            if s.starts_with(',') {
                                s = &s[1..];
                            }
                            continue;
                        }
                    }
                }
                // Fall through to regular PAYLOAD_FIELD handling below.
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
                ("REVERSE(PAYLOAD_FIELD(", SqlFieldTransform::Reverse),
                ("INITCAP(PAYLOAD_FIELD(", SqlFieldTransform::InitCap),
                ("SIGN(PAYLOAD_FIELD(", SqlFieldTransform::Sign),
                ("SQRT(PAYLOAD_FIELD(", SqlFieldTransform::Sqrt),
                ("LTRIM(PAYLOAD_FIELD(", SqlFieldTransform::Ltrim),
                ("RTRIM(PAYLOAD_FIELD(", SqlFieldTransform::Rtrim),
                ("LOG2(PAYLOAD_FIELD(", SqlFieldTransform::Log2),
                ("LOG10(PAYLOAD_FIELD(", SqlFieldTransform::Log10),
                ("LOG(PAYLOAD_FIELD(", SqlFieldTransform::Log),
                ("EXP(PAYLOAD_FIELD(", SqlFieldTransform::Exp),
                ("RADIANS(PAYLOAD_FIELD(", SqlFieldTransform::Radians),
                ("DEGREES(PAYLOAD_FIELD(", SqlFieldTransform::Degrees),
                ("SIN(PAYLOAD_FIELD(", SqlFieldTransform::Sin),
                ("COS(PAYLOAD_FIELD(", SqlFieldTransform::Cos),
                ("TAN(PAYLOAD_FIELD(", SqlFieldTransform::Tan),
                ("ATAN(PAYLOAD_FIELD(", SqlFieldTransform::Atan),
                ("ASIN(PAYLOAD_FIELD(", SqlFieldTransform::Asin),
                ("ACOS(PAYLOAD_FIELD(", SqlFieldTransform::Acos),
                ("TRUNC(PAYLOAD_FIELD(", SqlFieldTransform::Trunc),
                ("CBRT(PAYLOAD_FIELD(", SqlFieldTransform::Cbrt),
                (
                    "ARRAY_LENGTH(PAYLOAD_FIELD(",
                    SqlFieldTransform::ArrayLength,
                ),
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

        if fields.is_empty() && !return_id && window_columns.is_empty() && computed_cols.is_empty()
        {
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
            window_columns,
            computed_cols,
        })
    }
}

/// A `SELECT … FROM … UNION [ALL] SELECT … FROM …` multi-branch projection query.
///
/// Each branch is an independent [`SqlProjectQuery`]. Results are merged in branch order.
/// Plain UNION deduplicates rows by their serialized key-value content (sorted key order).
/// UNION ALL preserves all rows including duplicates.
///
/// ```text
/// SELECT PAYLOAD_FIELD('name') FROM 'obs://users'
/// UNION ALL
/// SELECT PAYLOAD_FIELD('name') FROM 'obs://guests'
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SqlProjectUnionQuery {
    pub branches: Vec<SqlProjectQuery>,
    /// When `false` (plain UNION), duplicate rows are content-deduplicated.
    /// When `true` (UNION ALL), all rows are preserved.
    pub all: bool,
}

impl SqlProjectUnionQuery {
    /// Parse a `SELECT … UNION [ALL] SELECT …` query.
    ///
    /// Splits on ` UNION ALL SELECT ` or ` UNION SELECT ` (case-insensitive after whitespace
    /// collapsing). Each segment is parsed as a [`SqlProjectQuery`]. Requires at least two branches.
    pub fn parse(sql: &str) -> Result<Self, SqlAggError> {
        let collapsed: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        let upper = collapsed.to_ascii_uppercase();

        let mut raw_branches: Vec<String> = Vec::new();
        let mut union_all = false;
        let mut scan = 0usize;
        let mut seg_start = 0usize;

        while scan < upper.len() {
            if upper[scan..].starts_with(" UNION ALL SELECT ") {
                raw_branches.push(collapsed[seg_start..scan].trim().to_string());
                union_all = true;
                scan += " UNION ALL ".len();
                seg_start = scan;
            } else if upper[scan..].starts_with(" UNION SELECT ") {
                raw_branches.push(collapsed[seg_start..scan].trim().to_string());
                scan += " UNION ".len();
                seg_start = scan;
            } else {
                scan += 1;
            }
        }
        raw_branches.push(collapsed[seg_start..].trim().to_string());

        if raw_branches.len() < 2 {
            return Err(SqlAggError::Unsupported(
                "UNION query requires at least two SELECT branches separated by UNION".into(),
            ));
        }

        let branches = raw_branches
            .iter()
            .map(|s| SqlProjectQuery::parse(s))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            branches,
            all: union_all,
        })
    }
}

/// Parse `LAG(PAYLOAD_FIELD('f'), n)` or `LEAD(PAYLOAD_FIELD('f'), n)` at the start of `s`.
/// Returns `Some((func, rest_after_closing_paren))` or `None` on any parse failure.
fn parse_lag_lead_func(is_lag: bool, s: &str) -> Option<(SqlWindowFunc, &str)> {
    let after_open = if is_lag {
        &s["LAG(".len()..]
    } else {
        &s["LEAD(".len()..]
    };
    let pf_upper = after_open.to_ascii_uppercase();
    if !pf_upper.starts_with("PAYLOAD_FIELD('") {
        return None;
    }
    let inner = &after_open["PAYLOAD_FIELD('".len()..];
    let quote_end = inner.find('\'')?;
    let field = inner[..quote_end].to_string();
    let after_pf_close = inner[quote_end + 1..].trim_start_matches(')').trim_start();
    let (offset, after_offset) = if after_pf_close.starts_with(',') {
        let rest = after_pf_close
            .strip_prefix(',')
            .unwrap_or(after_pf_close)
            .trim_start();
        let n_end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: usize = rest[..n_end].parse().unwrap_or(1);
        (n, rest[n_end..].trim_start())
    } else {
        (1usize, after_pf_close)
    };
    let after_lag_close = after_offset.trim_start_matches(')');
    let func = if is_lag {
        SqlWindowFunc::Lag { field, offset }
    } else {
        SqlWindowFunc::Lead { field, offset }
    };
    Some((func, after_lag_close))
}

/// Parse `FUNC_NAME(PAYLOAD_FIELD('fieldname'))` at the start of `s`.
/// `func_name` is the uppercase function name (e.g. `"SUM"`).
/// Returns `(field_name, rest_after_closing_paren)` or `None`.
fn parse_single_field_window_func<'a>(func_name: &str, s: &'a str) -> Option<(String, &'a str)> {
    let after_open = &s[func_name.len() + 1..]; // skip "FUNC_NAME("
    let upper = after_open.to_ascii_uppercase();
    if !upper.starts_with("PAYLOAD_FIELD('") {
        return None;
    }
    let inner = &after_open["PAYLOAD_FIELD('".len()..];
    let quote_end = inner.find('\'')?;
    let field = inner[..quote_end].to_string();
    let after_pf_close = inner[quote_end + 1..].trim_start_matches(')').trim_start();
    let after_close = after_pf_close.trim_start_matches(')');
    Some((field, after_close))
}

/// Parse `NTH_VALUE(PAYLOAD_FIELD('f'), n)` at the start of `s`.
fn parse_nth_value_func(s: &str) -> Option<(SqlWindowFunc, &str)> {
    // s starts with "NTH_VALUE(PAYLOAD_FIELD(" (case-insensitive already checked by caller)
    let after_pf_open = &s["NTH_VALUE(PAYLOAD_FIELD(".len()..];
    // Parse 'fieldname')
    let sq = after_pf_open.strip_prefix('\'')?;
    let quote_end = sq.find('\'')?;
    let field = sq[..quote_end].to_string();
    let after_quote = sq[quote_end + 1..].trim_start();
    // closing ) for PAYLOAD_FIELD
    let after_pf_close = after_quote.strip_prefix(')')?;
    let rest = after_pf_close.trim_start().strip_prefix(',')?;
    let rest = rest.trim_start();
    let n_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if n_end == 0 {
        return None;
    }
    let n: usize = rest[..n_end].parse().ok()?;
    let after_n = rest[n_end..].trim_start();
    // closing ) for NTH_VALUE
    let after_close = after_n.strip_prefix(')')?;
    Some((SqlWindowFunc::NthValue { field, n }, after_close))
}

/// Parse `NTILE(n)` at the start of `s`, returning `(SqlWindowFunc::Ntile{buckets:n}, rest)`.
fn parse_ntile_func(s: &str) -> Option<(SqlWindowFunc, &str)> {
    // s starts with "NTILE(" (case-insensitive already checked)
    let after_open = &s["NTILE(".len()..];
    let n_end = after_open
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_open.len());
    if n_end == 0 {
        return None;
    }
    let buckets: usize = after_open[..n_end].parse().ok()?;
    let after_n = after_open[n_end..].trim_start();
    let after_close = after_n.strip_prefix(')')?;
    Some((SqlWindowFunc::Ntile { buckets }, after_close))
}

type NamedWindowMap = std::collections::HashMap<String, (Vec<String>, Vec<SqlProjectOrderBy>)>;

/// Pre-scan `sql` for an optional `WINDOW name AS (...)` clause at depth=0.
/// Returns `(main_sql, named_windows)` where `main_sql` is everything before the WINDOW keyword.
/// Multiple named windows may be separated by commas: `WINDOW w1 AS (...), w2 AS (...)`.
fn pre_parse_window_clause(sql: &str) -> Result<(&str, NamedWindowMap), SqlAggError> {
    let upper = sql.to_ascii_uppercase();
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut pos = 0;
    let mut window_start = None;
    while pos < bytes.len() {
        match bytes[pos] {
            b'(' => {
                depth += 1;
                pos += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                pos += 1;
            }
            b'\'' => {
                pos += 1;
                while pos < bytes.len() && bytes[pos] != b'\'' {
                    pos += 1;
                }
                pos += 1;
            }
            _ => {
                if depth == 0 && upper[pos..].starts_with("WINDOW") {
                    let before_ok = pos == 0 || {
                        let b = bytes[pos - 1];
                        !b.is_ascii_alphanumeric() && b != b'_'
                    };
                    let after_pos = pos + 6;
                    let after_ok = after_pos >= bytes.len() || {
                        let b = bytes[after_pos];
                        !b.is_ascii_alphanumeric() && b != b'_'
                    };
                    if before_ok && after_ok {
                        window_start = Some(pos);
                        break;
                    }
                }
                pos += 1;
            }
        }
    }

    let Some(ws) = window_start else {
        return Ok((sql, NamedWindowMap::new()));
    };

    let main = sql[..ws].trim_end();
    let mut rest = sql[ws + "WINDOW".len()..].trim_start();
    let mut map = NamedWindowMap::new();
    loop {
        let id_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        if id_end == 0 {
            break;
        }
        let name = rest[..id_end].to_string();
        rest = rest[id_end..].trim_start();
        if !starts_with_keyword(rest, "AS") {
            return Err(SqlAggError::Unsupported(format!(
                "expected AS after window name '{name}'"
            )));
        }
        rest = skip_keyword(rest, "AS").trim_start();
        let (pb, ob, after) = parse_over_clause(rest)?;
        map.insert(name, (pb, ob));
        rest = after.trim_start();
        if rest.starts_with(',') {
            rest = rest[1..].trim_start();
        } else {
            break;
        }
    }
    Ok((main, map))
}

/// Parse `(PARTITION BY PAYLOAD_FIELD('f'), … ORDER BY PAYLOAD_FIELD('g') ASC|DESC, …)`.
/// `s` must start with `(`. Returns `(partition_by, order_by, rest_after_close_paren)`.
fn parse_over_clause(s: &str) -> Result<(Vec<String>, Vec<SqlProjectOrderBy>, &str), SqlAggError> {
    let s = s
        .strip_prefix('(')
        .ok_or_else(|| SqlAggError::Unsupported("expected '(' after OVER".into()))?;
    // Find matching ')' with depth counting.
    let mut depth = 1usize;
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(SqlAggError::Unsupported(
            "unclosed '(' in OVER clause".into(),
        ));
    }
    let inner = s[..end].trim();
    let after = &s[end + 1..];

    let mut cur = inner;

    // Optional PARTITION BY clause.
    let mut partition_by = Vec::new();
    let cu = cur.to_ascii_uppercase();
    if cu.starts_with("PARTITION") {
        cur = skip_keyword(cur, "PARTITION").trim_start();
        if !starts_with_keyword(cur, "BY") {
            return Err(SqlAggError::Unsupported(
                "expected BY after PARTITION".into(),
            ));
        }
        cur = skip_keyword(cur, "BY").trim_start();
        loop {
            let cu2 = cur.to_ascii_uppercase();
            if !cu2.starts_with("PAYLOAD_FIELD(") {
                break;
            }
            let after_pf = &cur["PAYLOAD_FIELD(".len()..];
            let (f, r) = parse_quoted_sql(after_pf)?;
            let r = r.trim_start().strip_prefix(')').ok_or_else(|| {
                SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in PARTITION BY".into())
            })?;
            partition_by.push(f);
            cur = r.trim_start();
            if cur.starts_with(',') {
                cur = cur[1..].trim_start();
            } else {
                break;
            }
        }
    }

    // Optional ORDER BY clause.
    let mut order_by = Vec::new();
    let cu = cur.to_ascii_uppercase();
    if cu.starts_with("ORDER") {
        cur = skip_keyword(cur, "ORDER").trim_start();
        if !starts_with_keyword(cur, "BY") {
            return Err(SqlAggError::Unsupported(
                "expected BY after ORDER in OVER".into(),
            ));
        }
        cur = skip_keyword(cur, "BY").trim_start();
        loop {
            let cu2 = cur.to_ascii_uppercase();
            if !cu2.starts_with("PAYLOAD_FIELD(") {
                break;
            }
            let after_pf = &cur["PAYLOAD_FIELD(".len()..];
            let (f, r) = parse_quoted_sql(after_pf)?;
            let r = r.trim_start().strip_prefix(')').ok_or_else(|| {
                SqlAggError::Unsupported("expected ')' after PAYLOAD_FIELD in OVER ORDER BY".into())
            })?;
            let r = r.trim_start();
            let ru = r.to_ascii_uppercase();
            let (direction, r) = if ru.starts_with("DESC")
                && !r
                    .as_bytes()
                    .get(4)
                    .is_some_and(|b| b.is_ascii_alphanumeric())
            {
                (AggOrderDirection::Desc, &r[4..])
            } else if ru.starts_with("ASC")
                && !r
                    .as_bytes()
                    .get(3)
                    .is_some_and(|b| b.is_ascii_alphanumeric())
            {
                (AggOrderDirection::Asc, &r[3..])
            } else {
                (AggOrderDirection::Asc, r)
            };
            order_by.push(SqlProjectOrderBy {
                field: f,
                direction,
            });
            cur = r.trim_start();
            if cur.starts_with(',') {
                cur = cur[1..].trim_start();
            } else {
                break;
            }
        }
    }

    Ok((partition_by, order_by, after))
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
        SqlFieldTransform::Reverse => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.chars().rev().collect())
            } else {
                val
            }
        }
        SqlFieldTransform::InitCap => {
            if let serde_json::Value::String(s) = val {
                let result = s
                    .split_whitespace()
                    .map(|word| {
                        let mut chars = word.chars();
                        match chars.next() {
                            None => String::new(),
                            Some(first) => {
                                first.to_uppercase().collect::<String>()
                                    + &chars.as_str().to_lowercase()
                            }
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                serde_json::Value::String(result)
            } else {
                val
            }
        }
        SqlFieldTransform::Repeat(n) => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.repeat(*n))
            } else {
                val
            }
        }
        SqlFieldTransform::Left(n) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let end = (*n).min(chars.len());
                serde_json::Value::String(chars[..end].iter().collect())
            } else {
                val
            }
        }
        SqlFieldTransform::Right(n) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let start = chars.len().saturating_sub(*n);
                serde_json::Value::String(chars[start..].iter().collect())
            } else {
                val
            }
        }
        SqlFieldTransform::Sign => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    let sign: i64 = if f > 0.0 {
                        1
                    } else if f < 0.0 {
                        -1
                    } else {
                        0
                    };
                    return serde_json::Value::Number(sign.into());
                }
            }
            val
        }
        SqlFieldTransform::Sqrt => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.sqrt()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Power(exp) => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.powf(*exp)) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Mod(divisor) => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f % divisor) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Strpos(needle) => {
            if let serde_json::Value::String(s) = &val {
                let pos = if let Some(byte_pos) = s.find(needle.as_str()) {
                    s[..byte_pos].chars().count() + 1
                } else {
                    0usize
                };
                return serde_json::Value::Number(pos.into());
            }
            val
        }
        SqlFieldTransform::SplitPart(delim, n) => {
            if let serde_json::Value::String(s) = &val {
                let part = s
                    .split(delim.as_str())
                    .nth(n.saturating_sub(1))
                    .unwrap_or("")
                    .to_string();
                return serde_json::Value::String(part);
            }
            val
        }
        SqlFieldTransform::Ltrim => {
            if let serde_json::Value::String(s) = val {
                return serde_json::Value::String(s.trim_start().to_string());
            }
            val
        }
        SqlFieldTransform::Rtrim => {
            if let serde_json::Value::String(s) = val {
                return serde_json::Value::String(s.trim_end().to_string());
            }
            val
        }
        SqlFieldTransform::Log => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.ln()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Exp => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.exp()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Log2 => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.log2()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Log10 => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.log10()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Radians => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.to_radians()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Degrees => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.to_degrees()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Sin => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.sin()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Cos => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.cos()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Tan => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.tan()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Atan => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.atan()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Asin => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.asin()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Acos => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.acos()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Trunc => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.trunc()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::Cbrt => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.cbrt()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        SqlFieldTransform::ArrayLength => {
            if let serde_json::Value::Array(arr) = &val {
                serde_json::Value::Number((arr.len() as u64).into())
            } else {
                val
            }
        }
        SqlFieldTransform::CaseWhen { branches, else_val } => {
            let field_str = match &val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => String::new(),
            };
            for (match_val, output) in branches {
                if field_str == *match_val {
                    return serde_json::Value::String(output.clone());
                }
            }
            serde_json::Value::String(else_val.clone())
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

        // Extra fields needed by window PARTITION BY / ORDER BY / LAG/LEAD source but not in SELECT.
        let window_extra_fields: Vec<String> = q
            .window_columns
            .iter()
            .flat_map(|wc| {
                let mut extras: Vec<String> = wc
                    .partition_by
                    .iter()
                    .chain(wc.order_by.iter().map(|ob| &ob.field))
                    .cloned()
                    .collect();
                match &wc.func {
                    SqlWindowFunc::Lag { field, .. }
                    | SqlWindowFunc::Lead { field, .. }
                    | SqlWindowFunc::Sum { field }
                    | SqlWindowFunc::Avg { field }
                    | SqlWindowFunc::Min { field }
                    | SqlWindowFunc::Max { field }
                    | SqlWindowFunc::FirstValue { field }
                    | SqlWindowFunc::LastValue { field }
                    | SqlWindowFunc::NthValue { field, .. } => {
                        extras.push(field.clone());
                    }
                    SqlWindowFunc::Count
                    | SqlWindowFunc::RowNumber
                    | SqlWindowFunc::Rank
                    | SqlWindowFunc::DenseRank
                    | SqlWindowFunc::Ntile { .. }
                    | SqlWindowFunc::PercentRank
                    | SqlWindowFunc::CumeDist => {}
                }
                extras
            })
            .filter(|f| !q.fields.contains(f) && !sort_keys_extra.contains(f))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
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
                for wf in &window_extra_fields {
                    row.insert(wf.clone(), project_payload_field(v.payload.as_deref(), wf));
                }
                // Computed columns: arithmetic, string concat, searched CASE WHEN.
                for cc in &q.computed_cols {
                    let val = match cc {
                        SqlComputedColumn::NumBin {
                            op,
                            left,
                            right,
                            alias: _,
                        } => {
                            let lv = match left {
                                SqlNumArg::Field(f) => {
                                    project_payload_field(v.payload.as_deref(), f)
                                }
                                SqlNumArg::Literal(n) => serde_json::Number::from_f64(*n)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null),
                            };
                            let rv = match right {
                                SqlNumArg::Field(f) => {
                                    project_payload_field(v.payload.as_deref(), f)
                                }
                                SqlNumArg::Literal(n) => serde_json::Number::from_f64(*n)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null),
                            };
                            if let (Some(l), Some(r)) = (
                                lv.as_f64().or_else(|| lv.as_i64().map(|i| i as f64)),
                                rv.as_f64().or_else(|| rv.as_i64().map(|i| i as f64)),
                            ) {
                                let result = match op {
                                    SqlNumBinOp::Add => l + r,
                                    SqlNumBinOp::Sub => l - r,
                                    SqlNumBinOp::Mul => l * r,
                                    SqlNumBinOp::Div => {
                                        if r == 0.0 {
                                            f64::NAN
                                        } else {
                                            l / r
                                        }
                                    }
                                    SqlNumBinOp::Rem => {
                                        if r == 0.0 {
                                            f64::NAN
                                        } else {
                                            l % r
                                        }
                                    }
                                };
                                serde_json::Number::from_f64(result)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null)
                            } else {
                                serde_json::Value::Null
                            }
                        }
                        SqlComputedColumn::StrConcat { parts, alias: _ } => {
                            let mut out = String::new();
                            for part in parts {
                                match part {
                                    SqlStrPart::Field(f) => {
                                        let fv = project_payload_field(v.payload.as_deref(), f);
                                        match &fv {
                                            serde_json::Value::String(s) => out.push_str(s),
                                            serde_json::Value::Number(n) => {
                                                out.push_str(&n.to_string())
                                            }
                                            serde_json::Value::Bool(b) => {
                                                out.push_str(&b.to_string())
                                            }
                                            serde_json::Value::Null => {}
                                            _ => {}
                                        }
                                    }
                                    SqlStrPart::Lit(lit) => out.push_str(lit),
                                }
                            }
                            serde_json::Value::String(out)
                        }
                        SqlComputedColumn::SearchedCase {
                            branches,
                            else_val,
                            alias: _,
                        } => {
                            let mut found: Option<String> = None;
                            for branch in branches {
                                if apply_payload_filter(&branch.filter, v.payload.as_deref()) {
                                    found = Some(branch.output.clone());
                                    break;
                                }
                            }
                            serde_json::Value::String(found.unwrap_or_else(|| else_val.clone()))
                        }
                    };
                    let alias = match cc {
                        SqlComputedColumn::NumBin { alias, .. }
                        | SqlComputedColumn::StrConcat { alias, .. }
                        | SqlComputedColumn::SearchedCase { alias, .. } => alias.clone(),
                    };
                    if !val.is_null() {
                        row.insert(alias, val);
                    }
                }
                row
            })
            .collect();

        // Window function computation on the full filtered result set.
        for wc in &q.window_columns {
            // Group row indices by partition key.
            let mut partitions: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, row) in rows.iter().enumerate() {
                let pk = wc
                    .partition_by
                    .iter()
                    .map(|f| {
                        row.get(f)
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "null".to_string())
                    })
                    .collect::<Vec<_>>()
                    .join("\x00");
                partitions.entry(pk).or_default().push(i);
            }
            for indices in partitions.values() {
                // Sort partition indices by window ORDER BY.
                let mut sorted: Vec<usize> = indices.clone();
                sorted.sort_by(|&ai, &bi| {
                    for ob in &wc.order_by {
                        let av = rows[ai]
                            .get(&ob.field)
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let bv = rows[bi]
                            .get(&ob.field)
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
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
                // Pre-compute order keys for tie detection in RANK/DENSE_RANK.
                let order_keys: Vec<String> = sorted
                    .iter()
                    .map(|&idx| {
                        wc.order_by
                            .iter()
                            .map(|ob| {
                                rows[idx]
                                    .get(&ob.field)
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "null".to_string())
                            })
                            .collect::<Vec<_>>()
                            .join("\x00")
                    })
                    .collect();
                match &wc.func {
                    SqlWindowFunc::RowNumber => {
                        for (n, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(
                                wc.alias.clone(),
                                serde_json::Value::Number((n + 1).into()),
                            );
                        }
                    }
                    SqlWindowFunc::Rank => {
                        let mut cur_rank = 1usize;
                        let mut gap = 0usize;
                        for (pos, (&idx, key)) in sorted.iter().zip(order_keys.iter()).enumerate() {
                            if pos == 0 {
                                cur_rank = 1;
                                gap = 0;
                            } else if key == &order_keys[pos - 1] {
                                gap += 1;
                            } else {
                                cur_rank += 1 + gap;
                                gap = 0;
                            }
                            rows[idx].insert(
                                wc.alias.clone(),
                                serde_json::Value::Number(cur_rank.into()),
                            );
                        }
                    }
                    SqlWindowFunc::DenseRank => {
                        let mut cur_rank = 1usize;
                        for (pos, (&idx, key)) in sorted.iter().zip(order_keys.iter()).enumerate() {
                            if pos > 0 && key != &order_keys[pos - 1] {
                                cur_rank += 1;
                            }
                            rows[idx].insert(
                                wc.alias.clone(),
                                serde_json::Value::Number(cur_rank.into()),
                            );
                        }
                    }
                    SqlWindowFunc::Lag { field, offset } => {
                        let values: Vec<serde_json::Value> = (0..sorted.len())
                            .map(|pos| {
                                if pos >= *offset {
                                    rows[sorted[pos - *offset]]
                                        .get(field)
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null)
                                } else {
                                    serde_json::Value::Null
                                }
                            })
                            .collect();
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Lead { field, offset } => {
                        let n = sorted.len();
                        let values: Vec<serde_json::Value> = (0..n)
                            .map(|pos| {
                                if pos + *offset < n {
                                    rows[sorted[pos + *offset]]
                                        .get(field)
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null)
                                } else {
                                    serde_json::Value::Null
                                }
                            })
                            .collect();
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Sum { field } => {
                        let running = !wc.order_by.is_empty();
                        let values: Vec<serde_json::Value> = if running {
                            let mut acc = 0.0f64;
                            sorted
                                .iter()
                                .map(|&idx| {
                                    if let Some(f) = rows[idx].get(field).and_then(|v| v.as_f64()) {
                                        acc += f;
                                    }
                                    serde_json::Number::from_f64(acc)
                                        .map(serde_json::Value::Number)
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect()
                        } else {
                            let total: f64 = sorted
                                .iter()
                                .filter_map(|&idx| rows[idx].get(field).and_then(|v| v.as_f64()))
                                .sum();
                            let v = serde_json::Number::from_f64(total)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null);
                            vec![v; sorted.len()]
                        };
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Avg { field } => {
                        let running = !wc.order_by.is_empty();
                        let values: Vec<serde_json::Value> = if running {
                            let mut sum = 0.0f64;
                            let mut cnt = 0usize;
                            sorted
                                .iter()
                                .map(|&idx| {
                                    if let Some(f) = rows[idx].get(field).and_then(|v| v.as_f64()) {
                                        sum += f;
                                        cnt += 1;
                                    }
                                    if cnt == 0 {
                                        serde_json::Value::Null
                                    } else {
                                        serde_json::Number::from_f64(sum / cnt as f64)
                                            .map(serde_json::Value::Number)
                                            .unwrap_or(serde_json::Value::Null)
                                    }
                                })
                                .collect()
                        } else {
                            let nums: Vec<f64> = sorted
                                .iter()
                                .filter_map(|&idx| rows[idx].get(field).and_then(|v| v.as_f64()))
                                .collect();
                            if nums.is_empty() {
                                vec![serde_json::Value::Null; sorted.len()]
                            } else {
                                let avg = nums.iter().sum::<f64>() / nums.len() as f64;
                                let v = serde_json::Number::from_f64(avg)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null);
                                vec![v; sorted.len()]
                            }
                        };
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Count => {
                        let running = !wc.order_by.is_empty();
                        let values: Vec<serde_json::Value> = if running {
                            (1..=sorted.len())
                                .map(|n| serde_json::Value::Number(n.into()))
                                .collect()
                        } else {
                            vec![serde_json::Value::Number(sorted.len().into()); sorted.len()]
                        };
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Min { field } => {
                        let running = !wc.order_by.is_empty();
                        let values: Vec<serde_json::Value> = if running {
                            let mut cur: Option<f64> = None;
                            sorted
                                .iter()
                                .map(|&idx| {
                                    if let Some(f) = rows[idx].get(field).and_then(|v| v.as_f64()) {
                                        cur = Some(cur.map_or(f, |m: f64| m.min(f)));
                                    }
                                    cur.and_then(serde_json::Number::from_f64)
                                        .map(serde_json::Value::Number)
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect()
                        } else {
                            let min = sorted
                                .iter()
                                .filter_map(|&idx| rows[idx].get(field).and_then(|v| v.as_f64()))
                                .reduce(f64::min);
                            let v = min
                                .and_then(serde_json::Number::from_f64)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null);
                            vec![v; sorted.len()]
                        };
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Max { field } => {
                        let running = !wc.order_by.is_empty();
                        let values: Vec<serde_json::Value> = if running {
                            let mut cur: Option<f64> = None;
                            sorted
                                .iter()
                                .map(|&idx| {
                                    if let Some(f) = rows[idx].get(field).and_then(|v| v.as_f64()) {
                                        cur = Some(cur.map_or(f, |m: f64| m.max(f)));
                                    }
                                    cur.and_then(serde_json::Number::from_f64)
                                        .map(serde_json::Value::Number)
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect()
                        } else {
                            let max = sorted
                                .iter()
                                .filter_map(|&idx| rows[idx].get(field).and_then(|v| v.as_f64()))
                                .reduce(f64::max);
                            let v = max
                                .and_then(serde_json::Number::from_f64)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null);
                            vec![v; sorted.len()]
                        };
                        for (pos, &idx) in sorted.iter().enumerate() {
                            rows[idx].insert(wc.alias.clone(), values[pos].clone());
                        }
                    }
                    SqlWindowFunc::Ntile { buckets } => {
                        let n = sorted.len();
                        for (pos, &idx) in sorted.iter().enumerate() {
                            // bucket = floor(pos * buckets / n) + 1, clamped to buckets
                            let bucket = if n == 0 || *buckets == 0 {
                                1usize
                            } else {
                                (pos * buckets / n + 1).min(*buckets)
                            };
                            rows[idx]
                                .insert(wc.alias.clone(), serde_json::Value::Number(bucket.into()));
                        }
                    }
                    SqlWindowFunc::FirstValue { field } => {
                        let first_val = sorted
                            .first()
                            .and_then(|&idx| rows[idx].get(field).cloned())
                            .unwrap_or(serde_json::Value::Null);
                        for &idx in &sorted {
                            rows[idx].insert(wc.alias.clone(), first_val.clone());
                        }
                    }
                    SqlWindowFunc::LastValue { field } => {
                        let last_val = sorted
                            .last()
                            .and_then(|&idx| rows[idx].get(field).cloned())
                            .unwrap_or(serde_json::Value::Null);
                        for &idx in &sorted {
                            rows[idx].insert(wc.alias.clone(), last_val.clone());
                        }
                    }
                    SqlWindowFunc::PercentRank => {
                        let n = sorted.len();
                        // Compute rank (with gaps, same as RANK) then (rank-1)/(n-1).
                        let mut cur_rank = 1usize;
                        let mut gap = 0usize;
                        for (pos, (&idx, key)) in sorted.iter().zip(order_keys.iter()).enumerate() {
                            if pos == 0 {
                                cur_rank = 1;
                                gap = 0;
                            } else if key == &order_keys[pos - 1] {
                                gap += 1;
                            } else {
                                cur_rank += 1 + gap;
                                gap = 0;
                            }
                            let pct = if n <= 1 {
                                0.0f64
                            } else {
                                (cur_rank - 1) as f64 / (n - 1) as f64
                            };
                            rows[idx].insert(
                                wc.alias.clone(),
                                serde_json::Number::from_f64(pct)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null),
                            );
                        }
                    }
                    SqlWindowFunc::CumeDist => {
                        let n = sorted.len();
                        // For each position, cume_dist = (number of rows with key <= current key) / n.
                        // Rows with the same ORDER BY key share the same cume_dist value.
                        let mut pos = 0usize;
                        while pos < n {
                            let key = &order_keys[pos];
                            let mut end = pos + 1;
                            while end < n && &order_keys[end] == key {
                                end += 1;
                            }
                            let cd = end as f64 / n as f64;
                            let v = serde_json::Number::from_f64(cd)
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null);
                            for &idx in &sorted[pos..end] {
                                rows[idx].insert(wc.alias.clone(), v.clone());
                            }
                            pos = end;
                        }
                    }
                    SqlWindowFunc::NthValue { field, n } => {
                        // n is 1-based; null when n > partition size.
                        let nth_val = if *n == 0 || *n > sorted.len() {
                            serde_json::Value::Null
                        } else {
                            rows[sorted[n - 1]]
                                .get(field)
                                .cloned()
                                .unwrap_or(serde_json::Value::Null)
                        };
                        for &idx in &sorted {
                            rows[idx].insert(wc.alias.clone(), nth_val.clone());
                        }
                    }
                }
            }
        }

        // Remove temporary window extra fields.
        for wf in &window_extra_fields {
            for row in &mut rows {
                row.remove(wf);
            }
        }

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

    /// Parse and execute a `SELECT … UNION [ALL] SELECT …` projection query.
    pub fn sql_project_union(
        &self,
        sql: &str,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>, StoreError> {
        let q = SqlProjectUnionQuery::parse(sql).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.sql_project_union_parsed(&q)
    }

    /// Execute a pre-parsed UNION projection query.
    pub fn sql_project_union_parsed(
        &self,
        q: &SqlProjectUnionQuery,
    ) -> Result<Vec<std::collections::HashMap<String, serde_json::Value>>, StoreError> {
        let mut all_rows: Vec<std::collections::HashMap<String, serde_json::Value>> = Vec::new();
        for branch in &q.branches {
            let rows = self.sql_project_parsed(branch)?;
            all_rows.extend(rows);
        }
        if !q.all {
            // UNION: deduplicate by serialized row content (sorted key order, first wins).
            let mut seen = std::collections::HashSet::new();
            all_rows.retain(|row| {
                let mut pairs: Vec<_> = row.iter().collect();
                pairs.sort_by_key(|(k, _)| k.as_str());
                let key: String = pairs
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("|");
                seen.insert(key)
            });
        }
        Ok(all_rows)
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
