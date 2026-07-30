use super::types::*;
use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::graph_query::{GraphQuery, GraphQueryMatch};
use crate::types::ObjectId;
use std::collections::HashMap;

/// Apply a `ReturnTransform` to a JSON value. Only `String` values are affected.
fn apply_return_transform(val: serde_json::Value, t: &ReturnTransform) -> serde_json::Value {
    match t {
        ReturnTransform::Upper | ReturnTransform::Lower | ReturnTransform::Trim => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(match t {
                    ReturnTransform::Upper => s.to_uppercase(),
                    ReturnTransform::Lower => s.to_lowercase(),
                    ReturnTransform::Trim => s.trim().to_string(),
                    _ => unreachable!(),
                })
            } else {
                val
            }
        }
        ReturnTransform::Abs
        | ReturnTransform::Round
        | ReturnTransform::Ceil
        | ReturnTransform::Floor => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    let result = match t {
                        ReturnTransform::Abs => f.abs(),
                        ReturnTransform::Round => f.round(),
                        ReturnTransform::Ceil => f.ceil(),
                        ReturnTransform::Floor => f.floor(),
                        _ => unreachable!(),
                    };
                    if let Some(n) = serde_json::Number::from_f64(result) {
                        return serde_json::Value::Number(n);
                    }
                }
            }
            val
        }
        ReturnTransform::Coalesce(fallback) => {
            if val.is_null() {
                serde_json::Value::String(fallback.clone())
            } else {
                val
            }
        }
        ReturnTransform::Replace(from, to) => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.replace(from.as_str(), to.as_str()))
            } else {
                val
            }
        }
        ReturnTransform::Length => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::Number((s.chars().count() as u64).into())
            } else {
                val
            }
        }
        ReturnTransform::Substring(start, len) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let begin = (*start).min(chars.len());
                let end = (begin + len).min(chars.len());
                serde_json::Value::String(chars[begin..end].iter().collect())
            } else {
                val
            }
        }
        ReturnTransform::CastInt => match &val {
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
        ReturnTransform::CastFloat => match &val {
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
        ReturnTransform::CastString => match &val {
            serde_json::Value::Number(n) => serde_json::Value::String(n.to_string()),
            serde_json::Value::Bool(b) => serde_json::Value::String(b.to_string()),
            _ => val,
        },
        ReturnTransform::NullIf(cmp_val) => {
            if let serde_json::Value::String(s) = &val {
                if s == cmp_val {
                    return serde_json::Value::Null;
                }
            }
            val
        }
        ReturnTransform::Reverse => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.chars().rev().collect())
            } else {
                val
            }
        }
        ReturnTransform::InitCap => {
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
        ReturnTransform::Lpad(width, pad) => {
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
        ReturnTransform::Rpad(width, pad) => {
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
        ReturnTransform::Repeat(n) => {
            if let serde_json::Value::String(s) = val {
                serde_json::Value::String(s.repeat(*n))
            } else {
                val
            }
        }
        ReturnTransform::Left(n) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let end = (*n).min(chars.len());
                serde_json::Value::String(chars[..end].iter().collect())
            } else {
                val
            }
        }
        ReturnTransform::Right(n) => {
            if let serde_json::Value::String(s) = val {
                let chars: Vec<char> = s.chars().collect();
                let start = chars.len().saturating_sub(*n);
                serde_json::Value::String(chars[start..].iter().collect())
            } else {
                val
            }
        }
        ReturnTransform::Sign => {
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
        ReturnTransform::Sqrt => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.sqrt()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Power(exp) => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.powf(*exp)) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Mod(divisor) => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f % divisor) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Strpos(needle) => {
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
        ReturnTransform::SplitPart(delim, n) => {
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
        ReturnTransform::Ltrim => {
            if let serde_json::Value::String(s) = val {
                return serde_json::Value::String(s.trim_start().to_string());
            }
            val
        }
        ReturnTransform::Rtrim => {
            if let serde_json::Value::String(s) = val {
                return serde_json::Value::String(s.trim_end().to_string());
            }
            val
        }
        ReturnTransform::Log => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.ln()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Exp => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.exp()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Log2 => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.log2()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Log10 => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.log10()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Radians => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.to_radians()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Degrees => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.to_degrees()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Sin => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.sin()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Cos => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.cos()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Tan => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.tan()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Atan => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.atan()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Asin => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.asin()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Acos => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.acos()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Trunc => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.trunc()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::Cbrt => {
            if let serde_json::Value::Number(n) = &val {
                if let Some(f) = n.as_f64() {
                    if let Some(result) = serde_json::Number::from_f64(f.cbrt()) {
                        return serde_json::Value::Number(result);
                    }
                }
            }
            val
        }
        ReturnTransform::CaseWhen { branches, else_val } => {
            let field_str = match &val {
                serde_json::Value::String(s) => s.as_str().to_string(),
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

/// Match `text` against a SQL LIKE pattern. `%` matches any sequence; `_` matches one char.
fn like_match(text: &str, pattern: &str) -> bool {
    // dp[i][j] = text[..i] matches pattern[..j]
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

pub(super) fn apply_payload_pred(payload: &[u8], pred: &PayloadPred) -> bool {
    let result = apply_payload_pred_inner(payload, pred);
    if pred.negated {
        !result
    } else {
        result
    }
}

fn apply_payload_pred_inner(payload: &[u8], pred: &PayloadPred) -> bool {
    let Ok(text) = std::str::from_utf8(payload) else {
        // Non-UTF-8: treat as no JSON fields present
        return pred.op == CmpOp::IsNull;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return pred.op == CmpOp::IsNull;
    };
    let field_val = v.get(&pred.field);

    // IS NULL / IS NOT NULL don't need a value on the RHS
    match pred.op {
        CmpOp::IsNull => return field_val.is_none(),
        CmpOp::IsNotNull => return field_val.is_some(),
        _ => {}
    }

    let field_val = match field_val {
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
                // String ops on a numeric field never match
                CmpOp::StartsWith | CmpOp::EndsWith | CmpOp::Contains => false,
                // Already handled above
                CmpOp::IsNull | CmpOp::IsNotNull => unreachable!(),
                // IN/NOT IN handled via StringList arm; Between via NumberRange; Like/ILike on text only
                CmpOp::In | CmpOp::NotIn | CmpOp::Between | CmpOp::Like | CmpOp::ILike => false,
            }
        }
        CmpValue::NumberRange(lo, hi) => {
            let lhs = match field_val.as_f64() {
                Some(n) => n,
                None => return false,
            };
            match pred.op {
                CmpOp::Between => lhs >= *lo && lhs <= *hi,
                _ => false,
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
                CmpOp::StartsWith => lhs.starts_with(rhs.as_str()),
                CmpOp::EndsWith => lhs.ends_with(rhs.as_str()),
                CmpOp::Contains => lhs.contains(rhs.as_str()),
                // Already handled above
                CmpOp::IsNull | CmpOp::IsNotNull => unreachable!(),
                // IN/NOT IN with a single Text value: treat as 1-element list
                CmpOp::In => lhs == rhs.as_str(),
                CmpOp::NotIn => lhs != rhs.as_str(),
                // BETWEEN on a text field never matches
                CmpOp::Between => false,
                CmpOp::Like => like_match(lhs, rhs.as_str()),
                CmpOp::ILike => like_match(&lhs.to_lowercase(), &rhs.to_lowercase()),
            }
        }
        CmpValue::Null => false,
        CmpValue::StringList(list) => {
            let lhs = match field_val.as_str() {
                Some(s) => s,
                None => return false,
            };
            match pred.op {
                CmpOp::In => list.iter().any(|v| v == lhs),
                CmpOp::NotIn => list.iter().all(|v| v != lhs),
                _ => false, // StringList only valid for In/NotIn
            }
        }
        CmpValue::NumberList(list) => {
            let lhs = match field_val.as_f64() {
                Some(n) => n,
                None => return false,
            };
            match pred.op {
                CmpOp::In => list.iter().any(|v| (lhs - v).abs() < f64::EPSILON),
                CmpOp::NotIn => list.iter().all(|v| (lhs - v).abs() >= f64::EPSILON),
                _ => false,
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
    parent_edge_map: &HashMap<ObjectId, crate::types::EdgeId>,
) -> (Vec<ObjectId>, Vec<crate::types::EdgeId>) {
    let mut nodes = vec![node.clone()];
    let mut edges = Vec::new();
    let mut current = node;
    while let Some(parent) = parent_map.get(current) {
        if let Some(eid) = parent_edge_map.get(current) {
            edges.push(eid.clone());
        }
        nodes.push(parent.clone());
        current = parent;
        if current == start {
            break;
        }
    }
    nodes.reverse();
    edges.reverse();
    (nodes, edges)
}

/// Build a `GraphQuery` from a `GraphIrQuery` (shared by `graph_ir_parsed` and
/// `graph_ir_projected`), factoring out the builder setup so it isn't duplicated.
pub(super) fn build_graph_query<'a>(store: &'a BinderStore, q: &GraphIrQuery) -> GraphQuery<'a> {
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
        NodeIdPred::EndsWith(sfx) => {
            let sfx = sfx.clone();
            builder.filter_node(move |obj| obj.object_id.0.ends_with(sfx.as_str()))
        }
        NodeIdPred::Contains(sub) => {
            let sub = sub.clone();
            builder.filter_node(move |obj| obj.object_id.0.contains(sub.as_str()))
        }
        NodeIdPred::Equals(id) => {
            let id = id.clone();
            builder.filter_node(move |obj| obj.object_id.0 == id)
        }
        NodeIdPred::InList(ids) => {
            let ids = ids.clone();
            builder.filter_node(move |obj| ids.contains(&obj.object_id.0))
        }
        NodeIdPred::NotInList(ids) => {
            let ids = ids.clone();
            builder.filter_node(move |obj| !ids.contains(&obj.object_id.0))
        }
    };

    if !q.payload_preds.is_empty() {
        let preds = q.payload_preds.clone();
        let combinator = q.pred_combinator;
        builder = builder.filter_node(move |obj| {
            let payload = obj.payload.as_deref().unwrap_or(&[]);
            match combinator {
                PayloadPredCombinator::And => {
                    preds.iter().all(|pred| apply_payload_pred(payload, pred))
                }
                PayloadPredCombinator::Or => {
                    preds.iter().any(|pred| apply_payload_pred(payload, pred))
                }
            }
        });
    }

    if !q.edge_preds.is_empty() {
        let preds = q.edge_preds.clone();
        builder = builder.filter_edge(move |edge| {
            let payload = edge.payload.as_deref().unwrap_or(&[]);
            preds.iter().all(|pred| apply_payload_pred(payload, pred))
        });
    }

    builder
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
    if !q.order_by.is_empty() {
        matches.sort_by(|a, b| {
            for ob in &q.order_by {
                let ord = match &ob.field {
                    OrderByField::ObjectId => a.object.object_id.0.cmp(&b.object.object_id.0),
                    OrderByField::Depth => a.depth.cmp(&b.depth),
                    OrderByField::PayloadField(field) => {
                        let ak = extract_sort_key(a.object.payload.as_deref(), field);
                        let bk = extract_sort_key(b.object.payload.as_deref(), field);
                        ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                    }
                };
                let ord = if ob.direction == OrderDirection::Desc {
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

    /// Execute a `MATCH … RETURN … UNION [ALL] MATCH … RETURN …` query string.
    ///
    /// Branches are executed independently and their rows merged. Plain UNION
    /// deduplicates by `object_id`; UNION ALL preserves duplicates.
    pub fn graph_ir_union(&self, query: &str) -> Result<Vec<GraphIrRow>, StoreError> {
        let q = GraphIrUnionQuery::parse(query).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.graph_ir_union_projected(&q)
    }

    /// Execute a pre-parsed UNION query with field projections.
    pub fn graph_ir_union_projected(
        &self,
        q: &GraphIrUnionQuery,
    ) -> Result<Vec<GraphIrRow>, StoreError> {
        let mut all_rows: Vec<GraphIrRow> = Vec::new();
        for branch in &q.branches {
            let rows = self.graph_ir_projected(branch)?;
            all_rows.extend(rows);
        }
        if !q.all {
            // UNION: deduplicate by object_id — first occurrence wins.
            let mut seen = std::collections::HashSet::new();
            all_rows.retain(|row| seen.insert(row.object_id.clone()));
        }
        Ok(all_rows)
    }

    /// Parse and execute a chained MATCH query (multi-hop traversal).
    pub fn graph_ir_chained(&self, query: &str) -> Result<Vec<GraphIrRow>, StoreError> {
        let q = GraphIrChainedQuery::parse(query).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.graph_ir_chained_projected(&q)
    }

    /// Execute a pre-parsed chained MATCH query.
    ///
    /// Runs `steps[0]` normally. For each subsequent step, iterates over the
    /// `object_id`s produced by the previous step and runs that step with the
    /// bound start ID, collecting all results. Returns the projected rows from
    /// the final step.
    pub fn graph_ir_chained_projected(
        &self,
        q: &GraphIrChainedQuery,
    ) -> Result<Vec<GraphIrRow>, StoreError> {
        if q.steps.is_empty() {
            return Ok(vec![]);
        }
        if q.steps.len() == 1 {
            return self.graph_ir_projected(&q.steps[0]);
        }

        // Step 0: run normally, collect end-node IDs.
        let step0_rows = self.graph_ir_projected(&q.steps[0])?;
        let mut bound_ids: Vec<crate::types::ObjectId> =
            step0_rows.into_iter().map(|r| r.object_id).collect();

        // Intermediate steps: pipeline bound IDs through each step.
        for step in &q.steps[1..q.steps.len() - 1] {
            let mut next_ids = Vec::new();
            for start in &bound_ids {
                let mut s = step.clone();
                s.start = start.clone();
                let rows = self.graph_ir_projected(&s)?;
                next_ids.extend(rows.into_iter().map(|r| r.object_id));
            }
            bound_ids = next_ids;
        }

        // Last step: return full projected rows.
        let last = &q.steps[q.steps.len() - 1];
        let mut all_rows = Vec::new();
        for start in &bound_ids {
            let mut s = last.clone();
            s.start = start.clone();
            let rows = self.graph_ir_projected(&s)?;
            all_rows.extend(rows);
        }
        Ok(all_rows)
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
        let (mut matches, parent_map, parent_edge_map) = if q.return_path {
            let (mut ms, pm, pem) =
                self.execute_graph_query_with_paths(build_graph_query(self, q))?;
            if q.min_depth > 1 {
                ms.retain(|m| m.depth >= q.min_depth);
            }
            (ms, pm, pem)
        } else {
            let mut ms = build_graph_query(self, q).execute()?;
            if q.min_depth > 1 {
                ms.retain(|m| m.depth >= q.min_depth);
            }
            (ms, HashMap::new(), HashMap::new())
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
            if q.return_all_fields {
                if let Some(payload) = m.object.payload.as_deref() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                            if let Some(obj) = v.as_object() {
                                for (key, val) in obj {
                                    b_fields.insert(key.clone(), val.clone());
                                }
                            }
                        }
                    }
                }
            } else if !q.return_fields.is_empty() {
                if let Some(payload) = m.object.payload.as_deref() {
                    if let Ok(text) = std::str::from_utf8(payload) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                            for key in &q.return_fields {
                                let raw_val =
                                    v.get(key).cloned().unwrap_or(serde_json::Value::Null);
                                let has_transform = q.return_field_transforms.contains_key(key);
                                if raw_val.is_null() && !has_transform {
                                    continue;
                                }
                                let out_key = q
                                    .return_field_aliases
                                    .get(key)
                                    .cloned()
                                    .unwrap_or_else(|| key.clone());
                                let val = if let Some(t) = q.return_field_transforms.get(key) {
                                    apply_return_transform(raw_val, t)
                                } else {
                                    raw_val
                                };
                                if !val.is_null() {
                                    b_fields.insert(out_key, val);
                                }
                            }
                        }
                    }
                }
            }
            if q.return_id {
                b_fields.insert(
                    "id(b)".to_string(),
                    serde_json::Value::String(m.object.object_id.0.clone()),
                );
            }

            let mut r_fields = HashMap::new();
            if !q.edge_return_fields.is_empty() {
                let edge_json = m
                    .via_edge
                    .as_ref()
                    .and_then(|e| e.payload.as_deref())
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok());
                for key in &q.edge_return_fields {
                    let val = edge_json
                        .as_ref()
                        .and_then(|v| v.get(key))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    r_fields.insert(key.clone(), val);
                }
            }

            let (path, edge_path) = if q.return_path {
                reconstruct_path(start, &m.object.object_id, &parent_map, &parent_edge_map)
            } else {
                (vec![], vec![])
            };

            if let Some(opt_clause) = &q.optional_clause {
                let opt_results = execute_optional_clause(self, &m.object.object_id, opt_clause)?;
                if opt_results.is_empty() {
                    rows.push(GraphIrRow {
                        object_id: m.object.object_id,
                        depth: m.depth,
                        fields: b_fields,
                        edge_fields: r_fields,
                        path,
                        edge_path,
                        optional: None,
                    });
                } else {
                    for opt in opt_results {
                        rows.push(GraphIrRow {
                            object_id: m.object.object_id.clone(),
                            depth: m.depth,
                            fields: b_fields.clone(),
                            edge_fields: r_fields.clone(),
                            path: path.clone(),
                            edge_path: edge_path.clone(),
                            optional: Some(opt),
                        });
                    }
                }
            } else {
                rows.push(GraphIrRow {
                    object_id: m.object.object_id,
                    depth: m.depth,
                    fields: b_fields,
                    edge_fields: r_fields,
                    path,
                    edge_path,
                    optional: None,
                });
            }
        }

        // DISTINCT: keep first occurrence of each object_id
        if q.distinct {
            let mut seen = std::collections::HashSet::new();
            rows.retain(|r| seen.insert(r.object_id.clone()));
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
    if wc.distinct {
        let mut seen = std::collections::HashSet::new();
        matches.retain(|m| seen.insert(m.object.object_id.clone()));
    }
    match &wc.end_pred {
        NodeIdPred::Any => {}
        NodeIdPred::StartsWith(pfx) => {
            matches.retain(|m| m.object.object_id.0.starts_with(pfx.as_str()));
        }
        NodeIdPred::EndsWith(sfx) => {
            matches.retain(|m| m.object.object_id.0.ends_with(sfx.as_str()));
        }
        NodeIdPred::Contains(sub) => {
            matches.retain(|m| m.object.object_id.0.contains(sub.as_str()));
        }
        NodeIdPred::Equals(id) => {
            matches.retain(|m| m.object.object_id.0 == id.as_str());
        }
        NodeIdPred::InList(ids) => {
            matches.retain(|m| ids.iter().any(|id| m.object.object_id.0 == id.as_str()));
        }
        NodeIdPred::NotInList(ids) => {
            matches.retain(|m| ids.iter().all(|id| m.object.object_id.0 != id.as_str()));
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
    if !wc.order_by.is_empty() {
        matches.sort_by(|a, b| {
            for ob in &wc.order_by {
                let ord = match &ob.field {
                    OrderByField::ObjectId => a.object.object_id.0.cmp(&b.object.object_id.0),
                    OrderByField::Depth => a.depth.cmp(&b.depth),
                    OrderByField::PayloadField(field) => {
                        let ak = extract_sort_key(a.object.payload.as_deref(), field);
                        let bk = extract_sort_key(b.object.payload.as_deref(), field);
                        ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                    }
                };
                let ord = if ob.direction == OrderDirection::Desc {
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
    if !q.order_by.is_empty() {
        rows.sort_by(|a, b| {
            for ob in &q.order_by {
                let ord = match &ob.field {
                    OrderByField::ObjectId => a.object_id.0.cmp(&b.object_id.0),
                    OrderByField::Depth => a.depth.cmp(&b.depth),
                    OrderByField::PayloadField(field) => {
                        let ak = sort_key_from_value(a.fields.get(field));
                        let bk = sort_key_from_value(b.fields.get(field));
                        ak.partial_cmp(&bk).unwrap_or(std::cmp::Ordering::Equal)
                    }
                };
                let ord = if ob.direction == OrderDirection::Desc {
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
        NodeIdPred::EndsWith(sfx) => {
            let sfx = sfx.clone();
            builder.filter_node(move |obj| obj.object_id.0.ends_with(sfx.as_str()))
        }
        NodeIdPred::Contains(sub) => {
            let sub = sub.clone();
            builder.filter_node(move |obj| obj.object_id.0.contains(sub.as_str()))
        }
        NodeIdPred::Equals(id) => {
            let id = id.clone();
            builder.filter_node(move |obj| obj.object_id.0 == id)
        }
        NodeIdPred::InList(ids) => {
            let ids = ids.clone();
            builder.filter_node(move |obj| ids.contains(&obj.object_id.0))
        }
        NodeIdPred::NotInList(ids) => {
            let ids = ids.clone();
            builder.filter_node(move |obj| !ids.contains(&obj.object_id.0))
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
        NodeIdPred::EndsWith(sfx) => {
            matches.retain(|m| m.object.object_id.0.ends_with(sfx.as_str()));
        }
        NodeIdPred::Contains(sub) => {
            matches.retain(|m| m.object.object_id.0.contains(sub.as_str()));
        }
        NodeIdPred::Equals(id) => {
            matches.retain(|m| m.object.object_id.0 == id.as_str());
        }
        NodeIdPred::InList(ids) => {
            matches.retain(|m| ids.iter().any(|id| m.object.object_id.0 == id.as_str()));
        }
        NodeIdPred::NotInList(ids) => {
            matches.retain(|m| ids.iter().all(|id| m.object.object_id.0 != id.as_str()));
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
        AggFunc::Collect => {
            let vals: Vec<serde_json::Value> = if let Some(field) = &agg.field {
                matches
                    .iter()
                    .map(|m| {
                        let bytes = m.object.payload.as_deref().unwrap_or(b"");
                        let text = std::str::from_utf8(bytes).unwrap_or("");
                        serde_json::from_str::<serde_json::Value>(text)
                            .ok()
                            .and_then(|v| v.get(field).cloned())
                            .unwrap_or(serde_json::Value::Null)
                    })
                    .collect()
            } else {
                matches
                    .iter()
                    .map(|m| serde_json::Value::String(m.object.object_id.0.clone()))
                    .collect()
            };
            serde_json::Value::Array(vals)
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
        AggFunc::Collect => {
            let vals: Vec<serde_json::Value> = if let Some(field) = &agg.field {
                group
                    .iter()
                    .map(|m| {
                        let bytes = m.object.payload.as_deref().unwrap_or(b"");
                        let text = std::str::from_utf8(bytes).unwrap_or("");
                        serde_json::from_str::<serde_json::Value>(text)
                            .ok()
                            .and_then(|v| v.get(field).cloned())
                            .unwrap_or(serde_json::Value::Null)
                    })
                    .collect()
            } else {
                group
                    .iter()
                    .map(|m| serde_json::Value::String(m.object.object_id.0.clone()))
                    .collect()
            };
            serde_json::Value::Array(vals)
        }
    }
}

impl BinderStore {
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
