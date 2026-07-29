use crate::engine::BinderStore;
use crate::error::StoreError;
use crate::transaction::Transaction;
use crate::types::{CommitId, EdgeId, ObjectId};
use thiserror::Error;

// ── write IR (issue #2 — write clauses) ──────────────────────────────────────

/// A single graph mutation in the write IR.
#[derive(Debug, Clone)]
pub enum WriteOp {
    /// Unconditionally upsert a node (put_object).
    CreateNode { id: ObjectId, payload: Vec<u8> },
    /// Upsert a node only if it does not already exist.
    MergeNode { id: ObjectId, payload: Vec<u8> },
    /// Unconditionally create or update an edge.
    CreateEdge {
        id: EdgeId,
        from: ObjectId,
        to: ObjectId,
        label: String,
        payload: Option<Vec<u8>>,
    },
    /// Create an edge only if it does not already exist (keyed by edge id).
    MergeEdge {
        id: EdgeId,
        from: ObjectId,
        to: ObjectId,
        label: String,
        payload: Option<Vec<u8>>,
    },
}

/// Parse error for the write IR.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphIrWriteError {
    #[error("unknown verb on line {line}: expected CREATE or MERGE")]
    UnknownVerb { line: usize },
    #[error("unknown target on line {line}: expected NODE or EDGE")]
    UnknownTarget { line: usize },
    #[error("missing field '{field}' on line {line}")]
    MissingField { field: &'static str, line: usize },
    #[error("invalid JSON payload on line {line}: {reason}")]
    InvalidPayload { line: usize, reason: String },
}

/// Parsed write IR — one or more graph mutation operations.
///
/// Grammar (one operation per non-empty line):
/// ```text
/// CREATE NODE id="<object-id>" payload=<json-object>
/// MERGE  NODE id="<object-id>" payload=<json-object>
/// CREATE EDGE id="<edge-id>" from="<oid>" to="<oid>" label="<str>" [payload=<json>]
/// MERGE  EDGE id="<edge-id>" from="<oid>" to="<oid>" label="<str>" [payload=<json>]
/// ```
///
/// Lines starting with `//` or `#` are treated as comments and ignored.
/// All operations in a single `GraphIrWrite` are committed as one transaction.
#[derive(Debug, Clone)]
pub struct GraphIrWrite {
    pub operations: Vec<WriteOp>,
}

/// Extract the value of a `key="value"` pair from a KV string.
fn extract_quoted_value(s: &str, key: &str) -> Option<String> {
    let pattern = format!("{}=\"", key);
    let start = s.find(pattern.as_str())? + pattern.len();
    let end = s[start..].find('"')?;
    Some(s[start..start + end].to_string())
}

/// Extract a JSON value starting at `key=<json>`, consuming just the JSON token.
/// Uses serde_json's streaming deserializer so trailing KV pairs are ignored.
fn extract_json_value(s: &str, key: &str) -> Option<serde_json::Value> {
    let pattern = format!("{}=", key);
    let start = s.find(pattern.as_str())? + pattern.len();
    let mut de = serde_json::Deserializer::from_str(&s[start..]);
    serde::Deserialize::deserialize(&mut de).ok()
}

impl GraphIrWrite {
    /// Parse a write IR string into a list of operations.
    pub fn parse(s: &str) -> Result<Self, GraphIrWriteError> {
        let mut operations = Vec::new();
        for (line_idx, raw) in s.lines().enumerate() {
            let line = line_idx + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
                continue;
            }
            // Split on up to 3 whitespace-delimited tokens: VERB TARGET rest...
            let mut parts = trimmed.splitn(3, ' ');
            let verb = parts.next().unwrap_or("").to_uppercase();
            let target = parts.next().unwrap_or("").to_uppercase();
            let rest = parts.next().unwrap_or("").trim();

            let is_create = match verb.as_str() {
                "CREATE" => true,
                "MERGE" => false,
                _ => return Err(GraphIrWriteError::UnknownVerb { line }),
            };

            match target.as_str() {
                "NODE" => {
                    let id_str = extract_quoted_value(rest, "id")
                        .ok_or(GraphIrWriteError::MissingField { field: "id", line })?;
                    let json = extract_json_value(rest, "payload").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "payload",
                            line,
                        },
                    )?;
                    let payload = serde_json::to_vec(&json).map_err(|e| {
                        GraphIrWriteError::InvalidPayload {
                            line,
                            reason: e.to_string(),
                        }
                    })?;
                    let op = if is_create {
                        WriteOp::CreateNode {
                            id: ObjectId::new(id_str),
                            payload,
                        }
                    } else {
                        WriteOp::MergeNode {
                            id: ObjectId::new(id_str),
                            payload,
                        }
                    };
                    operations.push(op);
                }
                "EDGE" => {
                    let id_str = extract_quoted_value(rest, "id")
                        .ok_or(GraphIrWriteError::MissingField { field: "id", line })?;
                    let from_str = extract_quoted_value(rest, "from").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "from",
                            line,
                        },
                    )?;
                    let to_str = extract_quoted_value(rest, "to")
                        .ok_or(GraphIrWriteError::MissingField { field: "to", line })?;
                    let label = extract_quoted_value(rest, "label").ok_or(
                        GraphIrWriteError::MissingField {
                            field: "label",
                            line,
                        },
                    )?;
                    let payload = extract_json_value(rest, "payload")
                        .and_then(|v| serde_json::to_vec(&v).ok());
                    let op = if is_create {
                        WriteOp::CreateEdge {
                            id: EdgeId::new(id_str),
                            from: ObjectId::new(from_str),
                            to: ObjectId::new(to_str),
                            label,
                            payload,
                        }
                    } else {
                        WriteOp::MergeEdge {
                            id: EdgeId::new(id_str),
                            from: ObjectId::new(from_str),
                            to: ObjectId::new(to_str),
                            label,
                            payload,
                        }
                    };
                    operations.push(op);
                }
                _ => return Err(GraphIrWriteError::UnknownTarget { line }),
            }
        }
        Ok(GraphIrWrite { operations })
    }
}

impl BinderStore {
    /// Parse and execute a write IR string as a single committed transaction.
    ///
    /// Returns `None` if all MERGE operations found existing objects/edges and
    /// nothing was written. Returns `Some(commit_id)` on a successful write.
    pub fn graph_ir_write(&self, s: &str) -> Result<Option<CommitId>, StoreError> {
        let write = GraphIrWrite::parse(s).map_err(|e| StoreError::InvalidArgument {
            message: e.to_string(),
        })?;
        self.graph_ir_write_parsed(&write)
    }

    /// Execute a pre-parsed write IR.
    pub fn graph_ir_write_parsed(
        &self,
        write: &GraphIrWrite,
    ) -> Result<Option<CommitId>, StoreError> {
        let mut tx = Transaction::new();
        let mut any = false;

        for op in &write.operations {
            match op {
                WriteOp::CreateNode { id, payload } => {
                    tx.put_object(id.clone(), payload.clone());
                    any = true;
                }
                WriteOp::MergeNode { id, payload } => {
                    if self.get(id)?.is_none() {
                        tx.put_object(id.clone(), payload.clone());
                        any = true;
                    }
                }
                WriteOp::CreateEdge {
                    id,
                    from,
                    to,
                    label,
                    payload,
                } => {
                    tx.put_edge(id.clone(), from.clone(), to.clone(), label, payload.clone());
                    any = true;
                }
                WriteOp::MergeEdge {
                    id,
                    from,
                    to,
                    label,
                    payload,
                } => {
                    if self.edge_history(id)?.is_empty() {
                        tx.put_edge(id.clone(), from.clone(), to.clone(), label, payload.clone());
                        any = true;
                    }
                }
            }
        }

        if !any {
            return Ok(None);
        }
        Ok(Some(self.commit(tx, "graph_ir_write", "")?))
    }
}
