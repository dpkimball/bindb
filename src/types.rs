use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;
use ulid::{Generator, Ulid};

/// Process-wide monotonic ULID source so successive commits always compare
/// in creation order (plain `Ulid::new()` can invert within the same ms).
static COMMIT_IDS: Mutex<Generator> = Mutex::new(Generator::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommitId(pub Ulid);

impl CommitId {
    pub fn new() -> Self {
        let mut gen = COMMIT_IDS.lock().unwrap_or_else(|e| e.into_inner());
        // Never fall back to Ulid::new() — that can invert within the same ms and
        // break as_of ordering. Advance the generator until it succeeds.
        loop {
            match gen.generate() {
                Ok(id) => return Self(id),
                Err(_) => {
                    // Generator overflow within this millisecond; spin until clock advances.
                    std::thread::yield_now();
                }
            }
        }
    }
}

impl Default for CommitId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub String);

impl ObjectId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeId(pub String);

impl EdgeId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
