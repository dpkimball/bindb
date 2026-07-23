use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProvenanceSource {
    Plugin(String),
    UserEdit,
    Import(String),
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub source: ProvenanceSource,
    pub source_ref: String,
    pub imported_at_ms: u64,
}
