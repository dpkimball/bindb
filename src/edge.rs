use crate::provenance::Provenance;
use crate::types::{CommitId, EdgeId, ObjectId};
use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeVersionId(pub Ulid);

impl EdgeVersionId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for EdgeVersionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EdgeVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An immutable point-in-time snapshot of a relationship edge within a specific commit.
/// Object and relationship version chains are independent (I-2 of PRINCIPLES).
/// `from_object = None` means this version is a tombstone (the edge was deleted).
/// `valid_to = None` means this is the current live version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinderEdgeVersion {
    pub version_id: EdgeVersionId,
    pub edge_id: EdgeId,
    pub commit_id: CommitId,
    pub valid_from: CommitId,
    pub valid_to: Option<CommitId>,
    pub from_object: Option<ObjectId>,
    pub to_object: Option<ObjectId>,
    pub relationship_type: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub provenance: Option<Provenance>,
}

impl BinderEdgeVersion {
    pub fn is_tombstone(&self) -> bool {
        self.from_object.is_none()
    }

    pub fn is_current(&self) -> bool {
        self.valid_to.is_none()
    }
}
