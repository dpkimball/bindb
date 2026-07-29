use super::types::*;
use crate::types::ObjectId;
use std::collections::HashMap;

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

        // Optional named path variable: `p = (a)-[r]->(b)` — capture name before pattern.
        let (path_var, after_match) = {
            let s = after_match.trim_start();
            // Identifier chars before ` = ` or `=`
            let var_end = s
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(s.len());
            let candidate = &s[..var_end];
            let after_var = s[var_end..].trim_start();
            if !candidate.is_empty()
                && !candidate.eq_ignore_ascii_case("optional")
                && after_var.starts_with('=')
                && !after_var.starts_with("=>")
            {
                let after_eq = after_var[1..].trim_start();
                (Some(candidate.to_string()), after_eq)
            } else {
                (None, s)
            }
        };

        let (direction, edge_types, min_depth, max_depth, after_pattern) =
            parse_pattern(after_match)?;
        let after_pattern = after_pattern.trim_start();

        if !starts_with_keyword(after_pattern, "WHERE") {
            return Err(GraphIrError::Unsupported("expected WHERE".into()));
        }
        let after_where = skip_keyword(after_pattern, "WHERE");
        let (start, end_pred, after_preds) = parse_where(after_where)?;
        let after_preds = after_preds.trim_start();

        // Optional payload predicates: AND/OR [NOT] b.FIELD OP VALUE or AND/OR [NOT] r.FIELD OP VALUE
        let mut payload_preds: Vec<PayloadPred> = Vec::new();
        let mut edge_preds: Vec<PayloadPred> = Vec::new();
        let mut pred_combinator = PayloadPredCombinator::And;
        let mut after_preds2 = after_preds;
        loop {
            let is_and = starts_with_keyword(after_preds2, "AND");
            let is_or = starts_with_keyword(after_preds2, "OR");
            if !is_and && !is_or {
                break;
            }
            let this_conn = if is_and {
                PayloadPredCombinator::And
            } else {
                PayloadPredCombinator::Or
            };
            let kw = if is_and { "AND" } else { "OR" };
            let after_kw = skip_keyword(after_preds2, kw).trim_start();
            // Detect optional NOT modifier (only meaningful with AND)
            let (negated, after_not) = if starts_with_keyword(after_kw, "NOT") {
                (true, skip_keyword(after_kw, "NOT").trim_start())
            } else {
                (false, after_kw)
            };
            if after_not.starts_with("b.") || after_not.starts_with("B.") {
                // The combinator is determined by the connector between the predicates
                // (second and subsequent), not the one that introduces the first predicate.
                if !payload_preds.is_empty() {
                    pred_combinator = this_conn;
                }
                let (mut pred, rest) = parse_payload_pred(after_not)?;
                // XOR: outer NOT flips whatever the inner parse set (e.g. NOT LIKE sets true).
                pred.negated ^= negated;
                payload_preds.push(pred);
                after_preds2 = rest.trim_start();
                continue;
            } else if after_not.starts_with("r.") || after_not.starts_with("R.") {
                let (mut pred, rest) = parse_payload_pred_for_var(after_not, "r")?;
                pred.negated ^= negated;
                edge_preds.push(pred);
                after_preds2 = rest.trim_start();
                continue;
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
                    order_by: vec![],
                    limit: None,
                    skip: None,
                    aggregations: aggs,
                    group_by_fields,
                    distinct: false,
                };
                (Some(wc), after_aggs.trim_start())
            } else {
                // Pass-through: WITH [DISTINCT] b [WHERE …] [ORDER BY …] [SKIP n] [LIMIT n]
                let (wc_distinct, after_with_kw) = if starts_with_keyword(after_with_kw, "DISTINCT")
                {
                    (true, skip_keyword(after_with_kw, "DISTINCT"))
                } else {
                    (false, after_with_kw)
                };
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
                    distinct: wc_distinct,
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
        let (distinct, after_return) = if starts_with_keyword(after_return, "DISTINCT") {
            (true, skip_keyword(after_return, "DISTINCT"))
        } else {
            (false, after_return)
        };
        let (ret_spec, ordering_tail) = split_at_ordering_clause(after_return);
        let (
            return_fields,
            return_field_aliases,
            return_field_transforms,
            edge_return_fields,
            c_return_fields,
            return_path,
            return_id,
            return_all_fields,
        ) = parse_return_spec(ret_spec, path_var.as_deref());
        let (order_by, limit, skip) = parse_multi_ordering_tail(ordering_tail)?;

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
            pred_combinator,
            edge_preds,
            return_fields,
            return_field_aliases,
            return_field_transforms,
            return_all_fields,
            edge_return_fields,
            return_path,
            return_id,
            distinct,
            order_by,
            limit,
            skip,
            optional_clause,
            with_clause,
            path_var,
        })
    }
}

pub(super) fn collapse_ws(s: &str) -> String {
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

pub(super) fn starts_with_keyword(s: &str, kw: &str) -> bool {
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

pub(super) fn skip_keyword<'a>(s: &'a str, kw: &str) -> &'a str {
    let s = s.trim_start();
    debug_assert!(starts_with_keyword(s, kw));
    s[kw.len()..].trim_start()
}

/// Parse `(a)-[:TYPES*hops]->(b)` / undirected / incoming variants.
fn parse_pattern(
    s: &str,
) -> Result<
    (
        crate::graph_query::Direction,
        Vec<String>,
        usize,
        usize,
        &str,
    ),
    GraphIrError,
> {
    use crate::graph_query::Direction;
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
    // Also accepts an optional relationship variable prefix: "r:KNOWS*1..3", "rel*1..2"
    let body = body.trim();
    if body.is_empty() {
        return Ok((Vec::new(), 1, 6));
    }
    // Strip optional leading variable name (identifier chars before ':' or '*').
    let body = {
        let var_end = body.find([':', '*']).unwrap_or(body.len());
        if var_end > 0
            && body[..var_end]
                .chars()
                .all(|c| c.is_ascii_alphabetic() || c == '_')
        {
            &body[var_end..]
        } else {
            body
        }
    };
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
pub(super) fn parse_payload_pred_for_var<'a>(
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

    // BETWEEN lo AND hi — numeric inclusive range
    if starts_with_keyword(rest, "BETWEEN") {
        let after_kw = skip_keyword(rest, "BETWEEN").trim_start();
        let lo_end = after_kw
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(after_kw.len());
        if lo_end == 0 {
            return Err(GraphIrError::Unsupported(
                "BETWEEN requires a numeric lower bound".into(),
            ));
        }
        let lo: f64 = after_kw[..lo_end]
            .parse()
            .map_err(|_| GraphIrError::Unsupported("invalid lower bound in BETWEEN".into()))?;
        let after_lo = after_kw[lo_end..].trim_start();
        if !starts_with_keyword(after_lo, "AND") {
            return Err(GraphIrError::Unsupported(
                "BETWEEN requires AND between bounds".into(),
            ));
        }
        let after_and = skip_keyword(after_lo, "AND").trim_start();
        let hi_end = after_and
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(after_and.len());
        if hi_end == 0 {
            return Err(GraphIrError::Unsupported(
                "BETWEEN requires a numeric upper bound".into(),
            ));
        }
        let hi: f64 = after_and[..hi_end]
            .parse()
            .map_err(|_| GraphIrError::Unsupported("invalid upper bound in BETWEEN".into()))?;
        return Ok((
            PayloadPred {
                field,
                op: CmpOp::Between,
                value: CmpValue::NumberRange(lo, hi),
                negated: false,
            },
            after_and[hi_end..].trim_start(),
        ));
    }

    // IS NOT NULL and IS NULL must be checked before other operators (no RHS)
    if starts_with_keyword(rest, "IS NOT NULL") {
        let rest = skip_keyword(rest, "IS NOT NULL");
        return Ok((
            PayloadPred {
                field,
                op: CmpOp::IsNotNull,
                value: CmpValue::Null,
                negated: false,
            },
            rest,
        ));
    }
    if starts_with_keyword(rest, "IS NULL") {
        let rest = skip_keyword(rest, "IS NULL");
        return Ok((
            PayloadPred {
                field,
                op: CmpOp::IsNull,
                value: CmpValue::Null,
                negated: false,
            },
            rest,
        ));
    }

    // NOT IN [...] before IN [...] to avoid ambiguity
    if starts_with_keyword(rest, "NOT IN") {
        let after_kw = skip_keyword(rest, "NOT IN").trim_start();
        let (value, rest) = parse_value_list(after_kw)?;
        return Ok((
            PayloadPred {
                field,
                op: CmpOp::NotIn,
                value,
                negated: false,
            },
            rest,
        ));
    }
    if starts_with_keyword(rest, "IN") {
        let after_kw = skip_keyword(rest, "IN").trim_start();
        let (value, rest) = parse_value_list(after_kw)?;
        return Ok((
            PayloadPred {
                field,
                op: CmpOp::In,
                value,
                negated: false,
            },
            rest,
        ));
    }

    // Detect keyword operators (STARTS WITH, ENDS WITH, CONTAINS, LIKE, ILIKE) before symbols.
    // NOT ILIKE / NOT LIKE must come before ILIKE / LIKE to avoid ambiguity.
    let (op, rest) = if starts_with_keyword(rest, "NOT ILIKE") {
        return {
            let r = skip_keyword(rest, "NOT ILIKE").trim_start();
            let (value, rest) = if r.starts_with('\'') || r.starts_with('"') {
                let (s, r2) = parse_quoted(r)?;
                (CmpValue::Text(s), r2)
            } else {
                return Err(GraphIrError::Unsupported(
                    "NOT ILIKE requires a quoted string pattern".into(),
                ));
            };
            Ok((
                PayloadPred {
                    field,
                    op: CmpOp::ILike,
                    value,
                    negated: true,
                },
                rest.trim_start(),
            ))
        };
    } else if starts_with_keyword(rest, "NOT LIKE") {
        return {
            let r = skip_keyword(rest, "NOT LIKE").trim_start();
            let (value, rest) = if r.starts_with('\'') || r.starts_with('"') {
                let (s, r2) = parse_quoted(r)?;
                (CmpValue::Text(s), r2)
            } else {
                return Err(GraphIrError::Unsupported(
                    "NOT LIKE requires a quoted string pattern".into(),
                ));
            };
            Ok((
                PayloadPred {
                    field,
                    op: CmpOp::Like,
                    value,
                    negated: true,
                },
                rest.trim_start(),
            ))
        };
    } else if starts_with_keyword(rest, "ILIKE") {
        (CmpOp::ILike, skip_keyword(rest, "ILIKE").trim_start())
    } else if starts_with_keyword(rest, "LIKE") {
        (CmpOp::Like, skip_keyword(rest, "LIKE").trim_start())
    } else if starts_with_keyword(rest, "STARTS WITH") {
        (
            CmpOp::StartsWith,
            skip_keyword(rest, "STARTS WITH").trim_start(),
        )
    } else if starts_with_keyword(rest, "ENDS WITH") {
        (
            CmpOp::EndsWith,
            skip_keyword(rest, "ENDS WITH").trim_start(),
        )
    } else if starts_with_keyword(rest, "CONTAINS") {
        (CmpOp::Contains, skip_keyword(rest, "CONTAINS").trim_start())
    } else if let Some(r) = rest.strip_prefix(">=") {
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

    Ok((
        PayloadPred {
            field,
            op,
            value,
            negated: false,
        },
        rest,
    ))
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
    parse_end_pred_for_var(s, "b")
}

/// Parse `id(VAR) STARTS WITH / ENDS WITH / CONTAINS / = '…'` for a named variable.
fn parse_end_pred_for_var<'a>(
    s: &'a str,
    var: &str,
) -> Result<(NodeIdPred, &'a str), GraphIrError> {
    let prefix = format!("id({var})");
    let s = s.trim_start();
    if s.len() < prefix.len() || !s[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return Err(GraphIrError::Unsupported(format!(
            "expected id({var}) STARTS WITH, ENDS WITH, CONTAINS, or id({var}) ="
        )));
    }
    let after = s[prefix.len()..].trim_start();
    if starts_with_keyword(after, "STARTS WITH") {
        let after_kw = skip_keyword(after, "STARTS WITH");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((NodeIdPred::StartsWith(val), rest))
    } else if starts_with_keyword(after, "ENDS WITH") {
        let after_kw = skip_keyword(after, "ENDS WITH");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((NodeIdPred::EndsWith(val), rest))
    } else if starts_with_keyword(after, "CONTAINS") {
        let after_kw = skip_keyword(after, "CONTAINS");
        let (val, rest) = parse_quoted(after_kw)?;
        Ok((NodeIdPred::Contains(val), rest))
    } else if let Some(after_eq) = after.strip_prefix('=') {
        let (val, rest) = parse_quoted(after_eq.trim_start())?;
        Ok((NodeIdPred::Equals(val), rest))
    } else if starts_with_keyword(after, "NOT IN") {
        let after_kw = skip_keyword(after, "NOT IN").trim_start();
        let (items, rest) = parse_string_list(after_kw)?;
        Ok((NodeIdPred::NotInList(items), rest))
    } else if starts_with_keyword(after, "IN") {
        let after_kw = skip_keyword(after, "IN").trim_start();
        let (items, rest) = parse_string_list(after_kw)?;
        Ok((NodeIdPred::InList(items), rest))
    } else {
        Err(GraphIrError::Unsupported(format!(
            "expected id({var}) STARTS WITH, ENDS WITH, CONTAINS, =, IN or NOT IN"
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
pub(super) fn is_agg_func_start(s: &str) -> bool {
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

type OrderingTailRest<'a> =
    Result<(Vec<GraphIrOrderBy>, Option<usize>, Option<usize>, &'a str), GraphIrError>;

/// Like `parse_multi_ordering_tail` but also returns the remaining unparsed string.
/// Used when ORDER BY / SKIP / LIMIT must be followed by more clauses (WITH → OPTIONAL MATCH or RETURN).
fn parse_ordering_tail_rest(s: &str) -> OrderingTailRest<'_> {
    let mut s = s.trim_start();
    let mut order_by: Vec<GraphIrOrderBy> = Vec::new();
    let mut limit = None;
    let mut skip = None;

    if starts_with_keyword(s, "ORDER") {
        let after_order = skip_keyword(s, "ORDER");
        if !starts_with_keyword(after_order, "BY") {
            return Err(GraphIrError::Unsupported("expected ORDER BY".into()));
        }
        let mut rest = skip_keyword(after_order, "BY");
        loop {
            let (field, after_field) = parse_order_field(rest)?;
            let after_field = after_field.trim_start();
            let au = after_field.to_ascii_uppercase();
            let (direction, after_dir) = if au.starts_with("DESC")
                && (after_field.len() == 4
                    || !after_field
                        .as_bytes()
                        .get(4)
                        .is_some_and(|b| b.is_ascii_alphanumeric()))
            {
                (OrderDirection::Desc, after_field[4..].trim_start())
            } else if au.starts_with("ASC")
                && (after_field.len() == 3
                    || !after_field
                        .as_bytes()
                        .get(3)
                        .is_some_and(|b| b.is_ascii_alphanumeric()))
            {
                (OrderDirection::Asc, after_field[3..].trim_start())
            } else {
                (OrderDirection::Asc, after_field)
            };
            order_by.push(GraphIrOrderBy { field, direction });
            if let Some(after_comma) = after_dir.strip_prefix(',') {
                rest = after_comma.trim_start();
                if starts_with_keyword(rest, "SKIP")
                    || starts_with_keyword(rest, "LIMIT")
                    || rest.is_empty()
                {
                    s = rest;
                    break;
                }
            } else {
                s = after_dir;
                break;
            }
        }
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

/// Parse `[item, ...]` where items are either all quoted strings or all bare numbers.
/// Returns `CmpValue::StringList` or `CmpValue::NumberList` accordingly.
fn parse_value_list(s: &str) -> Result<(CmpValue, &str), GraphIrError> {
    let s = s.trim_start();
    let inner = s
        .strip_prefix('[')
        .ok_or_else(|| GraphIrError::Unsupported("expected '[' for IN list".into()))?
        .trim_start();
    // Peek: if first non-ws char is a digit, '-', or '+' → numeric list
    let first = inner.chars().next().unwrap_or(' ');
    if first.is_ascii_digit() || first == '-' || first == '+' {
        let mut nums = Vec::new();
        let mut rest = inner;
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                return Ok((CmpValue::NumberList(nums), after.trim_start()));
            }
            let end = rest
                .find(|c: char| {
                    !c.is_ascii_digit() && c != '.' && c != '-' && c != '+' && c != 'e' && c != 'E'
                })
                .unwrap_or(rest.len());
            let num: f64 = rest[..end].parse().map_err(|_| {
                GraphIrError::Unsupported(format!("invalid number in IN list: {}", &rest[..end]))
            })?;
            nums.push(num);
            rest = rest[end..].trim_start();
            if let Some(after_comma) = rest.strip_prefix(',') {
                rest = after_comma;
            }
        }
    } else {
        let (items, rest) = parse_string_list(s)?;
        Ok((CmpValue::StringList(items), rest))
    }
}

/// Parse a `['v1', 'v2', ...]` string list literal.
fn parse_string_list(s: &str) -> Result<(Vec<String>, &str), GraphIrError> {
    let s = s.trim_start();
    let Some(s) = s.strip_prefix('[') else {
        return Err(GraphIrError::Unsupported("expected '[' for IN list".into()));
    };
    let mut items = Vec::new();
    let mut rest = s.trim_start();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            return Ok((items, after.trim_start()));
        }
        let (item, after_item) = parse_quoted(rest)?;
        items.push(item);
        rest = after_item.trim_start();
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma;
        }
    }
}

/// Split the RETURN spec from any trailing ORDER BY / SKIP / LIMIT clause.
/// Advances char by char and stops at the first keyword boundary for those tokens.
pub(super) fn split_at_ordering_clause(s: &str) -> (&str, &str) {
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

type MultiOrderingResult =
    Result<(Vec<GraphIrOrderBy>, Option<usize>, Option<usize>), GraphIrError>;

/// Parse multi-field ORDER BY for the RETURN clause.
/// Supports `ORDER BY b.f1 ASC, b.f2 DESC` with comma-separated fields.
pub(super) fn parse_multi_ordering_tail(s: &str) -> MultiOrderingResult {
    let mut s = s.trim_start();
    let mut order_by: Vec<GraphIrOrderBy> = Vec::new();
    let mut limit = None;
    let mut skip = None;

    if starts_with_keyword(s, "ORDER") {
        let after_order = skip_keyword(s, "ORDER");
        if !starts_with_keyword(after_order, "BY") {
            return Err(GraphIrError::Unsupported("expected ORDER BY".into()));
        }
        let mut rest = skip_keyword(after_order, "BY");
        loop {
            let (field, after_field) = parse_order_field(rest)?;
            let after_field = after_field.trim_start();
            // Detect direction: allow non-alphanumeric boundary (e.g. comma) not just whitespace.
            let au = after_field.to_ascii_uppercase();
            let (direction, after_dir) = if au.starts_with("DESC")
                && (after_field.len() == 4
                    || !after_field
                        .as_bytes()
                        .get(4)
                        .is_some_and(|b| b.is_ascii_alphanumeric()))
            {
                (OrderDirection::Desc, after_field[4..].trim_start())
            } else if au.starts_with("ASC")
                && (after_field.len() == 3
                    || !after_field
                        .as_bytes()
                        .get(3)
                        .is_some_and(|b| b.is_ascii_alphanumeric()))
            {
                (OrderDirection::Asc, after_field[3..].trim_start())
            } else {
                (OrderDirection::Asc, after_field)
            };
            order_by.push(GraphIrOrderBy { field, direction });
            // Consume comma for next sort key
            if let Some(after_comma) = after_dir.strip_prefix(',') {
                rest = after_comma.trim_start();
                if starts_with_keyword(rest, "SKIP")
                    || starts_with_keyword(rest, "LIMIT")
                    || rest.is_empty()
                {
                    break;
                }
            } else {
                s = after_dir;
                break;
            }
        }
        if order_by.len() == 1 {
            // single-field: s was already set inside the loop by the `break`
        }
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
    let _ = s;

    Ok((order_by, limit, skip))
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

pub(super) type ReturnSpec = (
    Vec<String>,
    HashMap<String, String>,
    HashMap<String, ReturnTransform>,
    Vec<String>,
    Vec<String>,
    bool,
    bool,
    bool,
);

/// Detect `UPPER/LOWER/TRIM/ABS/ROUND/CEIL/FLOOR(b.field)` in a RETURN selector.
/// Returns `(field_name, transform)` if recognized, otherwise `None`.
fn parse_single_quoted_graph(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('\'')?;
    let end = s.find('\'')?;
    Some((s[..end].to_string(), &s[end + 1..]))
}

fn parse_return_transform(s: &str) -> Option<(String, ReturnTransform)> {
    let su = s.to_ascii_uppercase();

    // COALESCE(b.field, 'fallback')
    if su.starts_with("COALESCE(") && s.ends_with(')') {
        let inner = &s["COALESCE(".len()..s.len() - 1];
        let comma = inner.find(',')?;
        let field_part = inner[..comma].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let fallback_raw = inner[comma + 1..].trim();
        let fallback = fallback_raw
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))?
            .to_string();
        return Some((field, ReturnTransform::Coalesce(fallback)));
    }

    // REPLACE(b.field, 'from', 'to')
    if su.starts_with("REPLACE(") && s.ends_with(')') {
        let inner = &s["REPLACE(".len()..s.len() - 1];
        let comma1 = inner.find(',')?;
        let field_part = inner[..comma1].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let rest = inner[comma1 + 1..].trim_start();
        let (from_str, after_from) = parse_single_quoted_graph(rest)?;
        let rest2 = after_from.trim_start().strip_prefix(',')?.trim_start();
        let (to_str, _) = parse_single_quoted_graph(rest2)?;
        return Some((field, ReturnTransform::Replace(from_str, to_str)));
    }

    // CAST(b.field AS INT|FLOAT|STRING)
    if su.starts_with("CAST(") && s.ends_with(')') {
        let inner = &s["CAST(".len()..s.len() - 1];
        let su_inner = inner.to_ascii_uppercase();
        let as_pos = su_inner.find(" AS ")?;
        let field_part = inner[..as_pos].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let type_str = su_inner[as_pos + 4..].trim();
        let transform = match type_str {
            "INT" => ReturnTransform::CastInt,
            "FLOAT" => ReturnTransform::CastFloat,
            "STRING" => ReturnTransform::CastString,
            _ => return None,
        };
        return Some((field, transform));
    }

    // NULLIF(b.field, 'val')
    if su.starts_with("NULLIF(") && s.ends_with(')') {
        let inner = &s["NULLIF(".len()..s.len() - 1];
        let comma = inner.find(',')?;
        let field_part = inner[..comma].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let val_raw = inner[comma + 1..].trim();
        let cmp = val_raw
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))?
            .to_string();
        return Some((field, ReturnTransform::NullIf(cmp)));
    }

    // SUBSTRING(b.field, start, len)
    if su.starts_with("SUBSTRING(") && s.ends_with(')') {
        let inner = &s["SUBSTRING(".len()..s.len() - 1];
        let comma1 = inner.find(',')?;
        let field_part = inner[..comma1].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let rest = inner[comma1 + 1..].trim_start();
        let comma2 = rest.find(',')?;
        let start: usize = rest[..comma2].trim().parse().ok()?;
        let len: usize = rest[comma2 + 1..].trim().parse().ok()?;
        return Some((field, ReturnTransform::Substring(start, len)));
    }

    // REPEAT(b.field, n) / LEFT(b.field, n) / RIGHT(b.field, n)
    for (kw, mk) in [("REPEAT(", 0usize), ("LEFT(", 1usize), ("RIGHT(", 2usize)] {
        if su.starts_with(kw) && s.ends_with(')') {
            let inner = &s[kw.len()..s.len() - 1];
            let comma = inner.find(',')?;
            let field_part = inner[..comma].trim();
            let field = field_part
                .strip_prefix("b.")
                .or_else(|| field_part.strip_prefix("B."))?
                .trim()
                .to_string();
            if field.is_empty() {
                return None;
            }
            let n: usize = inner[comma + 1..].trim().parse().ok()?;
            let transform = match mk {
                0 => ReturnTransform::Repeat(n),
                1 => ReturnTransform::Left(n),
                _ => ReturnTransform::Right(n),
            };
            return Some((field, transform));
        }
    }

    // POWER(b.field, exp)
    if su.starts_with("POWER(") && s.ends_with(')') {
        let inner = &s["POWER(".len()..s.len() - 1];
        let comma = inner.find(',')?;
        let field_part = inner[..comma].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let exp: f64 = inner[comma + 1..].trim().parse().ok()?;
        return Some((field, ReturnTransform::Power(exp)));
    }

    // MOD(b.field, n)
    if su.starts_with("MOD(") && s.ends_with(')') {
        let inner = &s["MOD(".len()..s.len() - 1];
        let comma = inner.find(',')?;
        let field_part = inner[..comma].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let n: f64 = inner[comma + 1..].trim().parse().ok()?;
        return Some((field, ReturnTransform::Mod(n)));
    }

    // STRPOS(b.field, 'needle')
    if su.starts_with("STRPOS(") && s.ends_with(')') {
        let inner = &s["STRPOS(".len()..s.len() - 1];
        let comma = inner.find(',')?;
        let field_part = inner[..comma].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let needle_raw = inner[comma + 1..].trim();
        let needle = needle_raw
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))?
            .to_string();
        return Some((field, ReturnTransform::Strpos(needle)));
    }

    // SPLIT_PART(b.field, 'delim', n)
    if su.starts_with("SPLIT_PART(") && s.ends_with(')') {
        let inner = &s["SPLIT_PART(".len()..s.len() - 1];
        let comma1 = inner.find(',')?;
        let field_part = inner[..comma1].trim();
        let field = field_part
            .strip_prefix("b.")
            .or_else(|| field_part.strip_prefix("B."))?
            .trim()
            .to_string();
        if field.is_empty() {
            return None;
        }
        let rest = inner[comma1 + 1..].trim_start();
        let (delim, after_delim) = parse_single_quoted_graph(rest)?;
        let rest2 = after_delim.trim_start().strip_prefix(',')?.trim_start();
        let n: usize = rest2.parse().ok()?;
        return Some((field, ReturnTransform::SplitPart(delim, n)));
    }

    // LPAD(b.field, n, 'c') / RPAD(b.field, n, 'c')
    for (kw, is_lpad) in [("LPAD(", true), ("RPAD(", false)] {
        if su.starts_with(kw) && s.ends_with(')') {
            let inner = &s[kw.len()..s.len() - 1];
            let comma1 = inner.find(',')?;
            let field_part = inner[..comma1].trim();
            let field = field_part
                .strip_prefix("b.")
                .or_else(|| field_part.strip_prefix("B."))?
                .trim()
                .to_string();
            if field.is_empty() {
                return None;
            }
            let rest = inner[comma1 + 1..].trim_start();
            let comma2 = rest.find(',')?;
            let width: usize = rest[..comma2].trim().parse().ok()?;
            let rest2 = rest[comma2 + 1..].trim_start();
            let (pad_str, _) = parse_single_quoted_graph(rest2)?;
            let transform = if is_lpad {
                ReturnTransform::Lpad(width, pad_str)
            } else {
                ReturnTransform::Rpad(width, pad_str)
            };
            return Some((field, transform));
        }
    }

    let prefixes: &[(&str, ReturnTransform)] = &[
        ("UPPER(", ReturnTransform::Upper),
        ("LOWER(", ReturnTransform::Lower),
        ("TRIM(", ReturnTransform::Trim),
        ("ABS(", ReturnTransform::Abs),
        ("ROUND(", ReturnTransform::Round),
        ("CEIL(", ReturnTransform::Ceil),
        ("FLOOR(", ReturnTransform::Floor),
        ("LENGTH(", ReturnTransform::Length),
        ("REVERSE(", ReturnTransform::Reverse),
        ("INITCAP(", ReturnTransform::InitCap),
        ("SIGN(", ReturnTransform::Sign),
        ("SQRT(", ReturnTransform::Sqrt),
        ("LTRIM(", ReturnTransform::Ltrim),
        ("RTRIM(", ReturnTransform::Rtrim),
        ("LOG2(", ReturnTransform::Log2),
        ("LOG10(", ReturnTransform::Log10),
        ("LOG(", ReturnTransform::Log),
        ("EXP(", ReturnTransform::Exp),
        ("RADIANS(", ReturnTransform::Radians),
        ("DEGREES(", ReturnTransform::Degrees),
        ("SIN(", ReturnTransform::Sin),
        ("COS(", ReturnTransform::Cos),
        ("TAN(", ReturnTransform::Tan),
        ("ATAN(", ReturnTransform::Atan),
        ("ASIN(", ReturnTransform::Asin),
        ("ACOS(", ReturnTransform::Acos),
        ("TRUNC(", ReturnTransform::Trunc),
        ("CBRT(", ReturnTransform::Cbrt),
    ];
    let found = prefixes
        .iter()
        .find(|(pfx, _)| su.starts_with(pfx) && s.ends_with(')'))?;
    let pfx: &str = found.0;
    let t: ReturnTransform = found.1.clone();
    let inner = &s[pfx.len()..s.len() - 1];
    let field = inner
        .strip_prefix("b.")
        .or_else(|| inner.strip_prefix("B."))?;
    let field = field.trim().to_string();
    if field.is_empty() {
        None
    } else {
        Some((field, t))
    }
}

/// Split `EXPR [AS alias]` where `AS` is only an alias marker when it appears after
/// all parentheses in `EXPR` are closed.  Handles `CAST(b.f AS INT)` correctly.
fn split_selector_alias(p: &str) -> (&str, Option<String>) {
    // Find the position after all parens close (depth returns to 0).
    let mut depth: usize = 0;
    let mut close_end: Option<usize> = None;
    for (i, c) in p.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_end = Some(i + c.len_utf8());
                }
            }
            _ => {}
        }
    }
    let search_from = close_end.unwrap_or(0);
    let remainder = p[search_from..].trim_start();
    let ru = remainder.to_ascii_uppercase();
    if ru.starts_with("AS ") || ru == "AS" {
        let sel = p[..search_from + (p[search_from..].len() - remainder.len())].trim();
        let after_as = remainder[2..].trim_start();
        let end = after_as
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(after_as.len());
        let alias = &after_as[..end];
        return (
            sel,
            if alias.is_empty() {
                None
            } else {
                Some(alias.to_string())
            },
        );
    }
    if close_end.is_none() {
        // No parens at all — simple `b.field [AS alias]` case.
        let toks: Vec<&str> = p.split_whitespace().collect();
        if toks.len() >= 3 && toks[1].eq_ignore_ascii_case("AS") {
            return (toks[0], Some(toks[2].to_string()));
        }
    }
    (p, None)
}

/// Split a RETURN selector list on commas that are not inside parentheses.
fn split_return_parts(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Plain `RETURN b`, `RETURN r`, and `RETURN c` are valid but produce no fields.
pub(super) fn parse_return_spec(s: &str, path_var: Option<&str>) -> ReturnSpec {
    let s = s.trim();
    if s.is_empty() {
        return (
            vec![],
            HashMap::new(),
            HashMap::new(),
            vec![],
            vec![],
            false,
            false,
            false,
        );
    }
    let mut b_fields = Vec::new();
    let mut b_aliases: HashMap<String, String> = HashMap::new();
    let mut b_transforms: HashMap<String, ReturnTransform> = HashMap::new();
    let mut r_fields = Vec::new();
    let mut c_fields = Vec::new();
    let mut return_path = false;
    let mut return_id = false;
    let mut return_all_fields = false;
    for part in split_return_parts(s) {
        let p = part.trim();
        // Detect optional `AS alias` suffix, respecting parenthesis depth.
        let (selector, alias) = split_selector_alias(p);
        let is_path_var = path_var.is_some_and(|v| selector.eq_ignore_ascii_case(v));
        if selector.eq_ignore_ascii_case("path") || is_path_var {
            return_path = true;
        } else if selector.eq_ignore_ascii_case("id(b)") || selector.eq_ignore_ascii_case("id(a)") {
            return_id = true;
        } else if selector.eq_ignore_ascii_case("b.*") {
            return_all_fields = true;
        } else if let Some((field, transform)) = parse_return_transform(selector) {
            // UPPER(b.field) / LOWER(b.field) / TRIM(b.field)
            b_transforms.insert(field.clone(), transform);
            if let Some(a) = alias {
                b_aliases.insert(field.clone(), a);
            }
            b_fields.push(field);
        } else if let Some(field) = selector
            .strip_prefix("b.")
            .or_else(|| selector.strip_prefix("B."))
        {
            let f = field.trim().to_string();
            if !f.is_empty() {
                if let Some(a) = alias {
                    b_aliases.insert(f.clone(), a);
                }
                b_fields.push(f);
            }
        } else if let Some(field) = selector
            .strip_prefix("r.")
            .or_else(|| selector.strip_prefix("R."))
        {
            let f = field.trim().to_string();
            if !f.is_empty() {
                r_fields.push(f);
            }
        } else if let Some(field) = selector
            .strip_prefix("c.")
            .or_else(|| selector.strip_prefix("C."))
        {
            let f = field.trim().to_string();
            if !f.is_empty() {
                c_fields.push(f);
            }
        }
        // "b", "r", "c" alone → plain return; ignore
    }
    (
        b_fields,
        b_aliases,
        b_transforms,
        r_fields,
        c_fields,
        return_path,
        return_id,
        return_all_fields,
    )
}

impl GraphIrUnionQuery {
    /// Parse a `MATCH … RETURN … UNION [ALL] MATCH … RETURN …` query.
    ///
    /// Splits on ` UNION MATCH ` or ` UNION ALL MATCH ` (case-insensitive,
    /// after whitespace collapsing). Each branch is parsed independently with
    /// `GraphIrQuery::parse`. Requires at least two branches.
    pub fn parse(src: &str) -> Result<Self, GraphIrError> {
        let collapsed = collapse_ws(src.trim());
        let upper = collapsed.to_ascii_uppercase();

        let mut raw_branches: Vec<String> = Vec::new();
        let mut union_all = false;
        let mut scan = 0usize;
        let mut seg_start = 0usize;

        while scan < upper.len() {
            if upper[scan..].starts_with(" UNION ALL MATCH ") {
                raw_branches.push(collapsed[seg_start..scan].trim().to_string());
                union_all = true;
                scan += " UNION ALL ".len();
                seg_start = scan;
            } else if upper[scan..].starts_with(" UNION MATCH ") {
                raw_branches.push(collapsed[seg_start..scan].trim().to_string());
                scan += " UNION ".len();
                seg_start = scan;
            } else {
                scan += 1;
            }
        }
        raw_branches.push(collapsed[seg_start..].trim().to_string());

        if raw_branches.len() < 2 {
            return Err(GraphIrError::Unsupported(
                "UNION query requires at least two MATCH clauses separated by UNION".into(),
            ));
        }

        let branches = raw_branches
            .iter()
            .map(|s| GraphIrQuery::parse(s))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            branches,
            all: union_all,
        })
    }
}
