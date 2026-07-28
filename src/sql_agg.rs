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
    /// Optional `WHERE PAYLOAD_FIELD('f') OP value` pre-aggregation filter.
    pub payload_filter: Option<SqlPayloadFilter>,
    /// Optional post-aggregation filter.
    pub having: Option<HavingClause>,
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

        // Optional WHERE clause: id filter, payload filter, or both joined with AND.
        let (id_filter, payload_filter, rest) = if starts_with_keyword(rest, "WHERE") {
            let after_where = skip_keyword(rest, "WHERE").trim_start();
            let su = after_where.to_ascii_uppercase();
            if su.starts_with("PAYLOAD_FIELD(") {
                let (pf, remaining) = parse_payload_filter(after_where)?;
                (None, Some(pf), remaining)
            } else {
                let (filter, remaining) = parse_where_id_filter(after_where)?;
                let remaining = remaining.trim_start();
                // Optional AND PAYLOAD_FIELD(...) after the id filter
                if starts_with_keyword(remaining, "AND") {
                    let after_and = skip_keyword(remaining, "AND").trim_start();
                    let su2 = after_and.to_ascii_uppercase();
                    if su2.starts_with("PAYLOAD_FIELD(") {
                        let (pf, rem2) = parse_payload_filter(after_and)?;
                        (Some(filter), Some(pf), rem2)
                    } else {
                        (Some(filter), None, remaining)
                    }
                } else {
                    (Some(filter), None, remaining)
                }
            }
        } else {
            (None, None, rest)
        };

        let mut rest = rest.trim_start();

        let group_by = if starts_with_keyword(rest, "GROUP") {
            let (gb, remaining) = parse_group_by(rest)?;
            rest = remaining;
            gb
        } else {
            SqlGroupBy::None
        };

        let having = if starts_with_keyword(rest, "HAVING") {
            let after_having = skip_keyword(rest, "HAVING").trim_start();
            let (having_clause, remainder) = parse_having(after_having)?;
            rest = remainder;
            Some(having_clause)
        } else {
            None
        };

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
            payload_filter,
            having,
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
            "WHERE clause must be: id STARTS WITH / ENDS WITH / CONTAINS / = '…'".into(),
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
    } else if let Some(after_eq) = after_id.trim_start().strip_prefix('=') {
        let (val, rest) = parse_quoted(after_eq)?;
        Ok((SqlIdFilter::Equals(val), rest.trim_start()))
    } else {
        Err(SqlAggError::Unsupported(
            "WHERE id must be followed by STARTS WITH, ENDS WITH, CONTAINS, or =".into(),
        ))
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
        Ok((SqlGroupBy::PayloadField(field), rest[1..].trim_start()))
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

        // Combine id_filter and payload_filter into a single closure (filter() replaces).
        if q.id_filter.is_some() || q.payload_filter.is_some() {
            let id_filter = q.id_filter.clone();
            let payload_filter = q.payload_filter.clone();
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
                    };
                    if !id_ok {
                        return false;
                    }
                }
                if let Some(ref pf) = payload_filter {
                    if !apply_payload_filter(pf, v.payload.as_deref()) {
                        return false;
                    }
                }
                true
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
        };

        let results = builder.execute()?;

        // Apply HAVING filter post-aggregation
        let mut results = if let Some(having) = &q.having {
            results
                .into_iter()
                .filter(|r| {
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
}

/// A value to compare against in a `WHERE PAYLOAD_FIELD` filter.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlPayloadValue {
    Str(String),
    Num(f64),
    Null,
}

/// `WHERE PAYLOAD_FIELD('field') OP value` filter for a projection query.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlPayloadFilter {
    pub field: String,
    pub op: SqlPayloadOp,
    pub value: SqlPayloadValue,
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
    pub namespace: String,
    /// Optional `WHERE id ...` filter applied to object_id suffix after namespace prefix.
    pub id_filter: Option<SqlIdFilter>,
    /// Optional `WHERE PAYLOAD_FIELD('f') OP value` filter.
    pub payload_filter: Option<SqlPayloadFilter>,
    /// Optional `ORDER BY PAYLOAD_FIELD('f') [ASC|DESC]` sort.
    pub order_by: Option<SqlProjectOrderBy>,
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

        let mut fields = Vec::new();
        loop {
            s = s.trim_start();
            if starts_with_keyword(s, "FROM") {
                break;
            }
            let su = s.to_ascii_uppercase();
            if !su.starts_with("PAYLOAD_FIELD(") {
                return Err(SqlAggError::Unsupported(
                    "SELECT list must contain only PAYLOAD_FIELD('name') expressions".into(),
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
            fields.push(field);
            if s.starts_with(',') {
                s = &s[1..];
            }
        }

        if fields.is_empty() {
            return Err(SqlAggError::Unsupported(
                "SELECT list must have at least one PAYLOAD_FIELD".into(),
            ));
        }

        s = skip_keyword(s, "FROM");
        let (namespace, rest) = parse_quoted(s)?;
        s = rest.trim_start();

        let (id_filter, payload_filter) = if starts_with_keyword(s, "WHERE") {
            let after_where = skip_keyword(s, "WHERE").trim_start();
            let su = after_where.to_ascii_uppercase();
            if su.starts_with("PAYLOAD_FIELD(") {
                let (pf, rest2) = parse_payload_filter(after_where)?;
                s = rest2.trim_start();
                (None, Some(pf))
            } else {
                let (filter, rest2) = parse_where_id_filter(after_where)?;
                let rest2 = rest2.trim_start();
                // Optional AND PAYLOAD_FIELD(...) after the id filter
                if starts_with_keyword(rest2, "AND") {
                    let after_and = skip_keyword(rest2, "AND").trim_start();
                    let su2 = after_and.to_ascii_uppercase();
                    if su2.starts_with("PAYLOAD_FIELD(") {
                        let (pf, rem2) = parse_payload_filter(after_and)?;
                        s = rem2.trim_start();
                        (Some(filter), Some(pf))
                    } else {
                        s = rest2;
                        (Some(filter), None)
                    }
                } else {
                    s = rest2;
                    (Some(filter), None)
                }
            }
        } else {
            (None, None)
        };

        let order_by = if starts_with_keyword(s, "ORDER") {
            let after_order = skip_keyword(s, "ORDER").trim_start();
            if !starts_with_keyword(after_order, "BY") {
                return Err(SqlAggError::Unsupported("expected ORDER BY".into()));
            }
            let after_by = skip_keyword(after_order, "BY").trim_start();
            let su = after_by.to_ascii_uppercase();
            if !su.starts_with("PAYLOAD_FIELD(") {
                return Err(SqlAggError::Unsupported(
                    "ORDER BY must be PAYLOAD_FIELD('field')".into(),
                ));
            }
            let after_pf = &after_by["PAYLOAD_FIELD(".len()..];
            let (field, rest2) = parse_quoted_sql(after_pf)?;
            let rest2 = rest2.trim_start();
            if !rest2.starts_with(')') {
                return Err(SqlAggError::Unsupported(
                    "expected ')' after PAYLOAD_FIELD in ORDER BY".into(),
                ));
            }
            let rest2 = rest2[1..].trim_start();
            let (direction, rest2) = if starts_with_keyword(rest2, "DESC") {
                (AggOrderDirection::Desc, skip_keyword(rest2, "DESC"))
            } else if starts_with_keyword(rest2, "ASC") {
                (AggOrderDirection::Asc, skip_keyword(rest2, "ASC"))
            } else {
                (AggOrderDirection::Asc, rest2)
            };
            s = rest2.trim_start();
            Some(SqlProjectOrderBy { field, direction })
        } else {
            None
        };

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
            namespace,
            id_filter,
            payload_filter,
            order_by,
            offset,
            limit,
        })
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
                };
                if !ok {
                    return false;
                }
            }
            if let Some(pf) = &q.payload_filter {
                if !apply_payload_filter(pf, v.payload.as_deref()) {
                    return false;
                }
            }
            true
        });

        // When ORDER BY references a field not in SELECT, we need to project it
        // temporarily for sorting, then strip it from the output rows.
        let sort_key_extra = q
            .order_by
            .as_ref()
            .filter(|ob| !q.fields.contains(&ob.field))
            .map(|ob| ob.field.clone());

        let mut rows: Vec<std::collections::HashMap<String, serde_json::Value>> = iter
            .map(|v| {
                let mut row: std::collections::HashMap<String, serde_json::Value> = q
                    .fields
                    .iter()
                    .map(|field| {
                        let val = project_payload_field(v.payload.as_deref(), field);
                        (field.clone(), val)
                    })
                    .collect();
                if let Some(ref sk) = sort_key_extra {
                    row.insert(sk.clone(), project_payload_field(v.payload.as_deref(), sk));
                }
                row
            })
            .collect();

        if let Some(ob) = &q.order_by {
            let field = ob.field.clone();
            let desc = ob.direction == AggOrderDirection::Desc;
            rows.sort_by(|a, b| {
                let av = a.get(&field).cloned().unwrap_or(serde_json::Value::Null);
                let bv = b.get(&field).cloned().unwrap_or(serde_json::Value::Null);
                let ord = compare_json_for_sort(&av, &bv);
                if desc {
                    ord.reverse()
                } else {
                    ord
                }
            });
        }

        // Remove the temporary sort key if it wasn't in the original SELECT list.
        if let Some(ref sk) = sort_key_extra {
            for row in &mut rows {
                row.remove(sk);
            }
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
