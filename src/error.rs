use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("WAL checksum mismatch at entry {entry_id}")]
    ChecksumMismatch { entry_id: u64 },

    #[error("WAL entry {entry_id} is incomplete (truncated)")]
    IncompleteEntry { entry_id: u64 },

    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),

    #[error("transaction is empty — at least one change is required")]
    EmptyTransaction,

    #[error("store is closed")]
    Closed,

    #[error("namespace violation: actor '{actor}' does not own namespace '{namespace}' (owner: '{owner}')")]
    NamespaceViolation {
        namespace: String,
        actor: String,
        owner: String,
    },
}
