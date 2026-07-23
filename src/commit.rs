use crate::provenance::Provenance;
use crate::types::{CommitId, EdgeId, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectChange {
    pub object_id: ObjectId,
    pub payload: Option<Vec<u8>>,
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeChange {
    pub edge_id: EdgeId,
    pub from_object: Option<ObjectId>,
    pub to_object: Option<ObjectId>,
    pub relationship_type: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinderCommit {
    pub commit_id: CommitId,
    pub parent_commit: Option<CommitId>,
    pub timestamp_ms: u64,
    pub actor: String,
    pub message: String,
    pub object_changes: Vec<ObjectChange>,
    pub relationship_changes: Vec<EdgeChange>,
}

impl BinderCommit {
    pub fn is_empty(&self) -> bool {
        self.object_changes.is_empty() && self.relationship_changes.is_empty()
    }
}
