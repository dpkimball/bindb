use crate::graph_query::Direction;
use crate::types::{EdgeId, ObjectId};
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
    EndsWith(String),
    Contains(String),
    Equals(String),
    /// `id(b) IN ['x://a', 'x://b']` — matches any of the listed full object IDs.
    InList(Vec<String>),
    /// `id(b) NOT IN ['x://a', 'x://b']` — excludes any of the listed full object IDs.
    NotInList(Vec<String>),
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
    /// `b.field STARTS WITH 'prefix'`
    StartsWith,
    /// `b.field ENDS WITH 'suffix'`
    EndsWith,
    /// `b.field CONTAINS 'substr'`
    Contains,
    /// `b.field IS NULL` — field absent from the JSON payload
    IsNull,
    /// `b.field IS NOT NULL` — field present in the JSON payload
    IsNotNull,
    /// `b.field IN ['v1', 'v2']` — field value is in the list
    In,
    /// `b.field NOT IN ['v1', 'v2']` — field value is not in the list
    NotIn,
    /// `b.field BETWEEN lo AND hi` — numeric range, inclusive on both ends
    Between,
    /// `b.field LIKE 'pat%tern'` — SQL wildcard: `%` = any chars, `_` = one char
    Like,
    /// `b.field ILIKE 'pat%tern'` — case-insensitive SQL wildcard
    ILike,
    /// `b.field HAS 'val'` — JSON array field contains a string element equal to `val`
    HasElement,
}

/// Right-hand side of a payload predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum CmpValue {
    Number(f64),
    Text(String),
    /// Placeholder for IS NULL / IS NOT NULL (no RHS value).
    Null,
    /// List of string values for IN / NOT IN.
    StringList(Vec<String>),
    /// List of numeric values for IN / NOT IN with numbers.
    NumberList(Vec<f64>),
    /// Inclusive numeric range for BETWEEN.
    NumberRange(f64, f64),
}

/// String transform applied to a projected node field in `RETURN UPPER(b.field)` etc.
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnTransform {
    Upper,
    Lower,
    Trim,
    /// `ABS(b.field)` — absolute value of a numeric field.
    Abs,
    /// `ROUND(b.field)` — round numeric field to nearest integer.
    Round,
    /// `CEIL(b.field)` — ceiling of a numeric field.
    Ceil,
    /// `FLOOR(b.field)` — floor of a numeric field.
    Floor,
    /// `COALESCE(b.field, 'fallback')` — substitute fallback when field is null.
    Coalesce(String),
    /// `REPLACE(b.field, 'from', 'to')` — string replacement.
    Replace(String, String),
    /// `LENGTH(b.field)` — character count of a string field (returned as a number).
    Length,
    /// `SUBSTRING(b.field, start, len)` — extract a substring (zero-based, char-indexed).
    Substring(usize, usize),
    /// `CAST(b.field AS INT)` — convert field to integer.
    CastInt,
    /// `CAST(b.field AS FLOAT)` — convert field to float.
    CastFloat,
    /// `CAST(b.field AS STRING)` — convert numeric field to string.
    CastString,
    /// `NULLIF(b.field, 'val')` — returns null (field omitted) when field equals `val`.
    NullIf(String),
    /// `REVERSE(b.field)` — reverses a string field character by character.
    Reverse,
    /// `INITCAP(b.field)` — capitalizes the first letter of each whitespace-separated word.
    InitCap,
    /// `LPAD(b.field, n, 'c')` — left-pad string to width `n` with char `c`.
    Lpad(usize, String),
    /// `RPAD(b.field, n, 'c')` — right-pad string to width `n` with char `c`.
    Rpad(usize, String),
    /// `REPEAT(b.field, n)` — repeat the string value `n` times.
    Repeat(usize),
    /// `LEFT(b.field, n)` — first `n` characters of a string field.
    Left(usize),
    /// `RIGHT(b.field, n)` — last `n` characters of a string field.
    Right(usize),
    /// `SIGN(b.field)` — sign of a numeric field: -1, 0, or 1.
    Sign,
    /// `SQRT(b.field)` — square root of a non-negative numeric field.
    Sqrt,
    /// `POWER(b.field, exp)` — raise numeric field to power `exp`.
    Power(f64),
    /// `MOD(b.field, n)` — numeric field modulo `n`.
    Mod(f64),
    /// `STRPOS(b.field, 'needle')` — 1-based char position of needle in string field; 0 if not found.
    Strpos(String),
    /// `SPLIT_PART(b.field, 'delim', n)` — nth (1-based) token after splitting string by delimiter.
    SplitPart(String, usize),
    /// `LTRIM(b.field)` — remove leading whitespace from a string field.
    Ltrim,
    /// `RTRIM(b.field)` — remove trailing whitespace from a string field.
    Rtrim,
    /// `LOG(b.field)` — natural logarithm of a positive numeric field; non-positive passes through.
    Log,
    /// `EXP(b.field)` — e raised to the power of a numeric field.
    Exp,
    /// `LOG2(b.field)` — base-2 logarithm of a positive numeric field.
    Log2,
    /// `LOG10(b.field)` — base-10 logarithm of a positive numeric field.
    Log10,
    /// `RADIANS(b.field)` — convert a numeric field from degrees to radians.
    Radians,
    /// `DEGREES(b.field)` — convert a numeric field from radians to degrees.
    Degrees,
    /// `SIN(b.field)` — sine of a numeric field (radians).
    Sin,
    /// `COS(b.field)` — cosine of a numeric field (radians).
    Cos,
    /// `TAN(b.field)` — tangent of a numeric field (radians).
    Tan,
    /// `ATAN(b.field)` — arc tangent of a numeric field; result in [-π/2, π/2].
    Atan,
    /// `ASIN(b.field)` — arc sine of a numeric field in [-1, 1]; out-of-domain passes through.
    Asin,
    /// `ACOS(b.field)` — arc cosine of a numeric field in [-1, 1]; out-of-domain passes through.
    Acos,
    /// `TRUNC(b.field)` — truncate a numeric field toward zero (differs from FLOOR for negatives).
    Trunc,
    /// `CBRT(b.field)` — cube root of a numeric field.
    Cbrt,
    /// `CASE b.field WHEN 'v1' THEN 'out1' [WHEN 'v2' THEN 'out2'] [ELSE 'default'] END`
    /// — value-match conditional; returns first matching output or `else_val`.
    CaseWhen {
        branches: Vec<(String, String)>,
        else_val: String,
    },
    /// `RETURN b.field[n]` — extract the nth element (0-based) from a JSON array field.
    /// Non-array values pass through unchanged; out-of-bounds returns null.
    Index(usize),
    /// `RETURN ARRAY_LENGTH(b.field)` — number of elements in a JSON array field.
    /// Non-array values pass through unchanged.
    ArrayLength,
}

/// How `payload_preds` are combined during BFS node filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadPredCombinator {
    /// All predicates must hold (AND — default).
    And,
    /// Any predicate must hold (OR).
    Or,
}

/// Predicate on a JSON field of the matched end node's payload.
/// Payload must be a UTF-8 JSON object; non-JSON payloads never match.
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadPred {
    pub field: String,
    pub op: CmpOp,
    pub value: CmpValue,
    /// When `true`, the predicate result is inverted (`NOT`).
    pub negated: bool,
}

// ── Computed RETURN expressions ───────────────────────────────────────────────

/// One operand in a binary numeric expression in RETURN.
#[derive(Debug, Clone, PartialEq)]
pub enum NumArg {
    /// A field from the node payload: `b.fieldname`.
    Field(String),
    /// A numeric literal.
    Literal(f64),
}

/// Binary arithmetic operator for computed return fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// A binary numeric expression `lhs OP rhs` used in RETURN computed fields.
#[derive(Debug, Clone, PartialEq)]
pub struct NumBinExpr {
    pub lhs: NumArg,
    pub op: NumBinOp,
    pub rhs: NumArg,
}

/// One part of a string concatenation: either a field reference or a literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrPart {
    /// A field from the node payload: `b.fieldname`.
    Field(String),
    /// A single-quoted string literal.
    Literal(String),
}

/// A string concatenation expression `part1 || part2 || …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrConcatExpr {
    pub parts: Vec<StrPart>,
}

/// One branch of a searched CASE WHEN expression.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchedCaseBranch {
    pub pred: PayloadPred,
    pub output: String,
}

/// The expression for a computed RETURN field.
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnComputedExpr {
    /// `b.lhs OP b.rhs AS alias` or `b.lhs OP literal AS alias`.
    NumBin(NumBinExpr),
    /// `b.f1 || 'sep' || b.f2 AS alias`.
    StrConcat(StrConcatExpr),
    /// `CASE WHEN b.field OP val THEN 'out' … ELSE 'default' END AS alias`.
    SearchedCase {
        branches: Vec<SearchedCaseBranch>,
        else_val: String,
    },
}

/// A computed field in a RETURN clause (arithmetic, concat, or CASE WHEN).
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedReturnField {
    pub alias: String,
    pub expr: ReturnComputedExpr,
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
    /// `p50(b.field)` — 50th-percentile (median) of the field across matched nodes.
    P50,
    /// `p99(b.field)` — 99th-percentile of the field across matched nodes.
    P99,
    /// `stddev(b.field)` — population standard deviation.
    Stddev,
    /// `variance(b.field)` — population variance.
    Variance,
    /// `collect(b)` / `collect(b.field)` — builds a JSON array of node IDs or field values.
    Collect,
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
    /// Sort mandatory `b` results before optional expansion (multi-key, left-to-right priority).
    pub order_by: Vec<GraphIrOrderBy>,
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
    /// When true, duplicate `b` nodes (same `object_id`) are removed before
    /// ordering and paging.  Parsed from `WITH DISTINCT b …`.
    pub distinct: bool,
}

/// One row returned by `graph_ir_projected`.
///
/// `fields` contains only the keys requested in `RETURN b.field1, b.field2`.
/// `edge_fields` contains keys requested in `RETURN r.field1, r.field2`, from
/// the edge traversed to reach this node; missing fields are `null`.
/// `path` is populated when `RETURN path` is requested; otherwise empty.
/// `optional` is populated by an `OPTIONAL MATCH` clause if present.
#[derive(Debug, Clone)]
pub struct GraphIrRow {
    pub object_id: ObjectId,
    pub depth: usize,
    pub fields: HashMap<String, serde_json::Value>,
    /// Fields projected from the via-edge (`RETURN r.field`). Empty when no
    /// `r.` fields were requested.
    pub edge_fields: HashMap<String, serde_json::Value>,
    /// Node IDs from start to this node when `RETURN path` was requested; otherwise empty.
    /// Length N means N nodes (start inclusive). Parallel to `edge_path`.
    pub path: Vec<ObjectId>,
    /// Edge IDs traversed along `path` when `RETURN path` was requested; otherwise empty.
    /// Length N-1 for a path of N nodes: `edge_path[i]` is the edge from `path[i]` to `path[i+1]`.
    pub edge_path: Vec<EdgeId>,
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
    /// Additional start nodes for multi-start fan-out (`WHERE id(a) IN ['x', 'y']`).
    /// When non-empty, BFS is run from each of `[start] + extra_starts` and results merged.
    pub extra_starts: Vec<ObjectId>,
    pub edge_types: Vec<String>,
    /// Minimum hop depth included in results (1 = include immediate neighbours).
    pub min_depth: usize,
    /// Max hops from start (builder max_depth).
    pub max_depth: usize,
    pub direction: Direction,
    pub end_pred: NodeIdPred,
    /// Payload field predicates applied to matched end nodes.
    /// Combined according to `pred_combinator`. Empty = no payload filtering.
    pub payload_preds: Vec<PayloadPred>,
    /// How `payload_preds` are combined. Defaults to `And`.
    pub pred_combinator: PayloadPredCombinator,
    /// Payload field predicates applied to each traversal edge (AND-combined).
    /// Edges whose payload does not match all predicates are skipped; the BFS
    /// does not follow them. Empty = no edge filtering.
    pub edge_preds: Vec<PayloadPred>,
    /// Fields to project from each matched node's JSON payload (`RETURN b.f1, b.f2`).
    /// Empty = plain node return via `graph_ir` / `graph_ir_parsed`.
    pub return_fields: Vec<String>,
    /// Optional output-key aliases for projected node fields (`RETURN b.name AS n`).
    /// Key = raw field name, value = alias used in `GraphIrRow::fields`.
    pub return_field_aliases: HashMap<String, String>,
    /// Optional string transforms on projected node fields (`RETURN UPPER(b.name)`).
    /// Key = raw field name, value = transform applied to string values before output.
    pub return_field_transforms: HashMap<String, ReturnTransform>,
    /// Fields to project from the via-edge's JSON payload (`RETURN r.f1, r.f2`).
    /// Empty = no edge projection. Populated into `GraphIrRow::edge_fields`.
    pub edge_return_fields: Vec<String>,
    /// When true, `graph_ir_projected` populates `GraphIrRow::path` with the
    /// sequence of node IDs from the start node to each matched node.
    pub return_path: bool,
    /// When true, `graph_ir_projected` adds `"id(b)"` → object_id to each row's `fields`.
    pub return_id: bool,
    /// When true, all JSON fields from matched node payload are returned (`RETURN b.*`).
    pub return_all_fields: bool,
    /// When true, the edge type/label of the via-edge is included in `GraphIrRow::edge_fields`
    /// under the key `"type(r)"`.
    pub return_edge_type: bool,
    /// When true, the traversal hop depth is included in `GraphIrRow::fields` under the key
    /// `"depth"`.
    pub return_depth: bool,
    /// When true, duplicate rows (same `object_id`) are removed before ORDER BY.
    pub distinct: bool,
    /// ORDER BY sort keys (multi-key, evaluated left to right). Empty = no sort.
    pub order_by: Vec<GraphIrOrderBy>,
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
    /// Named path variable from `MATCH p = (a)-[r]->(b)`. When set, `RETURN p`
    /// is equivalent to `RETURN path` and `GraphIrRow::path` is populated.
    pub path_var: Option<String>,
    /// Computed fields generated from arithmetic expressions, string concatenations,
    /// or searched CASE WHEN clauses in the RETURN clause.
    pub return_computed: Vec<ComputedReturnField>,
}

/// A `MATCH … RETURN … UNION [ALL] MATCH … RETURN …` query.
///
/// Each branch is an independent `GraphIrQuery`. Results are merged and,
/// for plain `UNION`, deduplicated by `object_id` (first occurrence wins).
/// `UNION ALL` preserves duplicates.
///
/// ```text
/// MATCH (a)-[:FOLLOWS]->(b) WHERE id(a) = 'sn://alice' RETURN b.name
/// UNION
/// MATCH (a)-[:LIKED_BY]->(b) WHERE id(a) = 'sn://alice' RETURN b.name
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrUnionQuery {
    pub branches: Vec<GraphIrQuery>,
    /// When `false` (plain UNION), duplicate rows are deduplicated by `object_id`.
    /// When `true` (UNION ALL), duplicates are preserved.
    pub all: bool,
}

/// A scan-based MATCH query — iterates all live objects under a namespace prefix and
/// filters them by payload predicates, without a fixed start node or graph traversal.
///
/// ```text
/// MATCH (a) WHERE id(a) STARTS WITH 'ns://'
///   [AND a.field OP val]
///   RETURN a.f1, a.f2
///   [ORDER BY a.field [ASC|DESC]] [SKIP n] [LIMIT n]
/// ```
///
/// Use `BinderStore::graph_ir_scan_projected` to execute.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrScanQuery {
    /// Namespace prefix to scan; all live objects with this prefix are candidates.
    pub namespace_prefix: String,
    /// Additional id-level predicate beyond STARTS WITH (ENDS WITH, CONTAINS, =, IN, NOT IN).
    pub end_pred: NodeIdPred,
    /// Payload field predicates on scanned nodes. Empty = no payload filtering.
    pub payload_preds: Vec<PayloadPred>,
    pub pred_combinator: PayloadPredCombinator,
    pub return_fields: Vec<String>,
    pub return_field_aliases: HashMap<String, String>,
    pub return_field_transforms: HashMap<String, ReturnTransform>,
    pub return_id: bool,
    pub return_all_fields: bool,
    pub distinct: bool,
    pub order_by: Vec<GraphIrOrderBy>,
    pub limit: Option<usize>,
    pub skip: Option<usize>,
    pub return_computed: Vec<ComputedReturnField>,
}

/// A multi-hop chained MATCH query where each step's matched end node becomes the
/// start node for the next step.
///
/// Steps share variable names `(a)` and `(b)` across the chain. The first step
/// must provide `WHERE id(a) = '…'`; subsequent steps omit the start ID — it is
/// bound at execution time from the previous step's results.
///
/// ```text
/// MATCH (a)-[:KNOWS]->(b) WHERE id(a) = 'sn://alice'
/// MATCH (a)-[:LIKES]->(b) RETURN b.title
/// ```
///
/// Predicates in steps 2+ (`WHERE b.field OP val`, `RETURN b.field`, etc.) apply
/// to the end node `b` of that step. The start node `a` is always the bound result
/// from the preceding step.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphIrChainedQuery {
    pub steps: Vec<GraphIrQuery>,
}
