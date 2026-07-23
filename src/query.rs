use crate::edge::BinderEdgeVersion;
use crate::object::BinderObjectVersion;
use crate::types::{CommitId, EdgeId, ObjectId};

/// Point-in-time query: find the version of object_id that was current at
/// `at_commit`. "Current at commit C" means:
///   valid_from <= C  AND  (valid_to is None OR C < valid_to)
///
/// Versions must be in ascending commit order (valid_from ascending).
pub fn object_as_of<'a>(
    versions: &'a [BinderObjectVersion],
    object_id: &ObjectId,
    at_commit: CommitId,
) -> Option<&'a BinderObjectVersion> {
    versions
        .iter()
        .filter(|v| &v.object_id == object_id)
        .rfind(|v| v.valid_from <= at_commit && v.valid_to.is_none_or(|to| at_commit < to))
}

/// Point-in-time query for an edge.
pub fn edge_as_of<'a>(
    versions: &'a [BinderEdgeVersion],
    edge_id: &EdgeId,
    at_commit: CommitId,
) -> Option<&'a BinderEdgeVersion> {
    versions
        .iter()
        .filter(|v| &v.edge_id == edge_id)
        .rfind(|v| v.valid_from <= at_commit && v.valid_to.is_none_or(|to| at_commit < to))
}
