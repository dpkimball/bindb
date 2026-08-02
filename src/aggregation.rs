/// Builder-pattern aggregation DSL over namespace observations.
///
/// Lets callers compute GROUP BY aggregates over any set of objects without
/// pulling payloads into application memory for ad-hoc iteration.
///
/// # Example
/// ```ignore
/// let results = store
///     .aggregate_observations()
///     .namespace("obs://health.sleep_duration_h")
///     .extract_f64(|payload| bincode::deserialize::<MySleepObs>(payload).ok().map(|o| o.hours))
///     .group_by(|v| v.object_id.0.split(':').nth(1).map(str::to_owned)) // source
///     .aggregate(Aggregate::Avg)
///     .execute()?;
/// ```
use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::object::BinderObjectVersion;
use std::collections::HashMap;

// ── public types ──────────────────────────────────────────────────────────────

/// A pair of matched observations from two namespaces, produced by
/// [`BinderStore::join_observations`].
#[derive(Debug, Clone)]
pub struct JoinRow {
    /// The key that matched across both namespaces.
    pub key: String,
    /// The matching observation from the left namespace.
    pub left: BinderObjectVersion,
    /// The matching observation from the right namespace.
    pub right: BinderObjectVersion,
}

/// Aggregate function applied across each group of extracted values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Aggregate {
    /// Number of observations in each group.
    Count,
    /// Sum of extracted values.
    Sum,
    /// Arithmetic mean of extracted values.
    Avg,
    /// Minimum extracted value.
    Min,
    /// Maximum extracted value.
    Max,
    /// 50th-percentile (median) of extracted values.
    P50,
    /// 99th-percentile of extracted values.
    P99,
    /// Population standard deviation of extracted values.
    Stddev,
    /// Population variance of extracted values.
    Variance,
    /// JSON array of a named payload field value from each observation in the group.
    /// `COLLECT(PAYLOAD_FIELD('fieldname'))` — result in `AggregateResult::collected`.
    Collect(String),
    /// Concatenate string values of a named payload field across the group, separated by `sep`.
    /// `STRING_AGG(PAYLOAD_FIELD('fieldname'), 'sep')` — result in `AggregateResult::collected`
    /// as a `Value::String`. Non-string field values are skipped.
    StringAgg { field: String, sep: String },
}

/// A single row returned by an aggregation query.
#[derive(Debug, Clone)]
pub struct AggregateResult {
    /// The group key, or `None` when no `.group_by()` was specified.
    pub group_key: Option<String>,
    /// Computed aggregate value (e.g. average hours, total count).
    /// For `COLLECT` queries this is always `0.0`; use `collected` instead.
    pub value: f64,
    /// Number of observations that contributed to this result.
    pub count: usize,
    /// Populated for `Aggregate::Collect` (JSON array) and `Aggregate::StringAgg` (JSON string).
    pub collected: Option<serde_json::Value>,
}

// ── type aliases ──────────────────────────────────────────────────────────────

type ExtractFn<'a> = Box<dyn Fn(&[u8]) -> Option<f64> + 'a>;
type GroupFn<'a> = Box<dyn Fn(&BinderObjectVersion) -> Option<String> + 'a>;
type FilterFn<'a> = Box<dyn Fn(&BinderObjectVersion) -> bool + 'a>;

// ── builder ───────────────────────────────────────────────────────────────────

pub struct ObservationAggregation<'a> {
    pub(crate) store: &'a BinderStore,
    pub(crate) namespace: Option<String>,
    pub(crate) extract_fn: Option<ExtractFn<'a>>,
    pub(crate) group_fn: Option<GroupFn<'a>>,
    pub(crate) aggregate: Aggregate,
    pub(crate) filter_fn: Option<FilterFn<'a>>,
}

impl<'a> ObservationAggregation<'a> {
    pub(crate) fn new(store: &'a BinderStore) -> Self {
        Self {
            store,
            namespace: None,
            extract_fn: None,
            group_fn: None,
            aggregate: Aggregate::Count,
            filter_fn: None,
        }
    }

    /// Namespace prefix to scan (e.g. `"obs://health.sleep_duration_h"`).
    /// Required before `execute()`.
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = Some(ns.into());
        self
    }

    /// Provide a function that extracts a numeric value from each object's
    /// payload bytes.  Returning `None` from this function excludes that object
    /// from aggregation (but not from the `Count` aggregate — `Count` counts
    /// objects regardless).
    pub fn extract_f64(mut self, f: impl Fn(&[u8]) -> Option<f64> + 'a) -> Self {
        self.extract_fn = Some(Box::new(f));
        self
    }

    /// Group results by the string key returned by `f`.  Objects for which `f`
    /// returns `None` are placed into a `None` group.
    pub fn group_by(mut self, f: impl Fn(&BinderObjectVersion) -> Option<String> + 'a) -> Self {
        self.group_fn = Some(Box::new(f));
        self
    }

    /// Aggregate function to apply within each group (default: `Count`).
    pub fn aggregate(mut self, agg: Aggregate) -> Self {
        self.aggregate = agg;
        self
    }

    /// Only include objects where `predicate` returns `true`.
    pub fn filter(mut self, predicate: impl Fn(&BinderObjectVersion) -> bool + 'a) -> Self {
        self.filter_fn = Some(Box::new(predicate));
        self
    }

    /// Execute the aggregation query.
    pub fn execute(self) -> Result<Vec<AggregateResult>, StoreError> {
        self.store.execute_aggregation(self)
    }
}

// ── internal execution (called by engine) ────────────────────────────────────

pub(crate) fn run_aggregation(
    versions: Vec<BinderObjectVersion>,
    q: ObservationAggregation<'_>,
) -> Vec<AggregateResult> {
    let mut groups: HashMap<Option<String>, Vec<f64>> = HashMap::new();
    let mut counts: HashMap<Option<String>, usize> = HashMap::new();
    let mut collect_groups: HashMap<Option<String>, Vec<serde_json::Value>> = HashMap::new();
    let mut string_agg_groups: HashMap<Option<String>, Vec<String>> = HashMap::new();

    let collect_field = if let Aggregate::Collect(ref f) = q.aggregate {
        Some(f.clone())
    } else {
        None
    };
    let string_agg_params = if let Aggregate::StringAgg { ref field, ref sep } = q.aggregate {
        Some((field.clone(), sep.clone()))
    } else {
        None
    };

    for v in versions {
        if let Some(ref f) = q.filter_fn {
            if !f(&v) {
                continue;
            }
        }

        let key = q.group_fn.as_ref().and_then(|f| f(&v));
        *counts.entry(key.clone()).or_insert(0) += 1;

        if let Some(ref field) = collect_field {
            let json_val = v
                .payload
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|v| v.get(field.as_str()).cloned())
                .unwrap_or(serde_json::Value::Null);
            collect_groups
                .entry(key.clone())
                .or_default()
                .push(json_val);
            groups.entry(key).or_default();
        } else if let Some((ref field, _)) = string_agg_params {
            if let Some(s) = v
                .payload
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
                .and_then(|jv| {
                    jv.get(field.as_str())
                        .and_then(|f| f.as_str())
                        .map(str::to_owned)
                })
            {
                string_agg_groups.entry(key.clone()).or_default().push(s);
            } else {
                string_agg_groups.entry(key.clone()).or_default();
            }
            groups.entry(key).or_default();
        } else {
            let numeric = v
                .payload
                .as_deref()
                .and_then(|p| q.extract_fn.as_ref().and_then(|f| f(p)));

            if let Some(val) = numeric {
                groups.entry(key).or_default().push(val);
            } else {
                groups.entry(key).or_default();
            }
        }
    }

    let agg = q.aggregate;
    let is_collect = collect_field.is_some();
    let is_string_agg = string_agg_params.is_some();
    let string_agg_sep = string_agg_params.map(|(_, sep)| sep).unwrap_or_default();
    let mut results: Vec<AggregateResult> = groups
        .into_iter()
        .map(|(key, mut values)| {
            let count = counts.get(&key).copied().unwrap_or(0);
            let (value, collected) = if is_collect {
                let arr = collect_groups.remove(&key).map(serde_json::Value::Array);
                (0.0, arr)
            } else if is_string_agg {
                let joined = string_agg_groups
                    .remove(&key)
                    .unwrap_or_default()
                    .join(&string_agg_sep);
                (0.0, Some(serde_json::Value::String(joined)))
            } else {
                let v = match &agg {
                    Aggregate::Count => count as f64,
                    Aggregate::Sum => values.iter().sum(),
                    Aggregate::Avg => {
                        if values.is_empty() {
                            0.0
                        } else {
                            values.iter().sum::<f64>() / values.len() as f64
                        }
                    }
                    Aggregate::Min => values.iter().cloned().fold(f64::INFINITY, f64::min),
                    Aggregate::Max => values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                    Aggregate::P50 => percentile(&mut values, 50.0),
                    Aggregate::P99 => percentile(&mut values, 99.0),
                    Aggregate::Stddev => stddev_of(&values, false),
                    Aggregate::Variance => stddev_of(&values, true),
                    Aggregate::Collect(_) | Aggregate::StringAgg { .. } => unreachable!(),
                };
                (v, None)
            };
            AggregateResult {
                group_key: key,
                value,
                count,
                collected,
            }
        })
        .collect();

    results.sort_by(|a, b| a.group_key.cmp(&b.group_key));
    results
}

fn stddev_of(values: &[f64], variance: bool) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    if variance {
        var
    } else {
        var.sqrt()
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((p / 100.0) * (values.len() - 1) as f64).round() as usize;
    values[idx.min(values.len() - 1)]
}
