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
    /// Lifecycle write reason (S1b knowledge lifecycle).
    #[serde(default)]
    pub lifecycle_reason: Option<String>,
    /// Normalized `(question, answer)` hash for import conflict detection.
    #[serde(default)]
    pub source_content_hash: Option<String>,
    /// Record `version` this import row was based on.
    #[serde(default)]
    pub import_base_version: Option<u64>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            source: ProvenanceSource::System,
            source_ref: String::new(),
            imported_at_ms: 0,
            lifecycle_reason: None,
            source_content_hash: None,
            import_base_version: None,
        }
    }
}
