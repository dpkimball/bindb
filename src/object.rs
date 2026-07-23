use crate::provenance::Provenance;
use crate::types::{CommitId, ObjectId};
use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectVersionId(pub Ulid);

impl ObjectVersionId {
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl Default for ObjectVersionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ObjectVersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An immutable point-in-time snapshot of an object within a specific commit.
/// `payload = None` means this version is a tombstone (the object was deleted).
/// `valid_to = None` means this is the current live version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinderObjectVersion {
    pub version_id: ObjectVersionId,
    pub object_id: ObjectId,
    pub commit_id: CommitId,
    pub valid_from: CommitId,
    pub valid_to: Option<CommitId>,
    pub payload: Option<Vec<u8>>,
    pub provenance: Option<Provenance>,
}

impl BinderObjectVersion {
    pub fn is_tombstone(&self) -> bool {
        self.payload.is_none()
    }

    pub fn is_current(&self) -> bool {
        self.valid_to.is_none()
    }
}
