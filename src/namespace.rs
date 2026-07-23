use crate::error::StoreError;
use std::collections::HashMap;

/// Namespace ownership registry. Each namespace is owned by exactly one actor.
/// Enforced at commit time: a commit actor may only write objects in namespaces
/// they own (PRINCIPLES I-9). SAME_AS edges cross namespaces freely — edges
/// are never namespace-checked.
///
/// V1: If no namespaces are registered, all writes pass. The structural check
/// exists so Phase 2 enforcement requires no refactoring.
#[derive(Debug, Default)]
pub struct NamespaceRegistry {
    /// namespace → owner actor id
    owned: HashMap<String, String>,
}

impl NamespaceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a namespace as owned by actor. Overwrites any prior owner.
    pub fn register(&mut self, namespace: impl Into<String>, actor: impl Into<String>) {
        self.owned.insert(namespace.into(), actor.into());
    }

    /// Return the owner of a namespace, or None if not registered.
    pub fn owner(&self, namespace: &str) -> Option<&str> {
        self.owned.get(namespace).map(|s| s.as_str())
    }

    /// Check whether actor owns namespace. Returns true if the namespace is
    /// unregistered (open namespace — V1 permissive behaviour).
    pub fn actor_owns(&self, namespace: &str, actor: &str) -> bool {
        match self.owned.get(namespace) {
            Some(owner) => owner == actor,
            None => true, // unregistered namespace: permit (V1)
        }
    }

    /// Validate a transaction's namespace claim against the registry.
    /// `namespace` is the declared namespace for all object writes in this tx.
    /// Edge writes are exempt (they cross namespaces freely).
    pub fn check(&self, namespace: &str, actor: &str) -> Result<(), StoreError> {
        if self.actor_owns(namespace, actor) {
            Ok(())
        } else {
            Err(StoreError::NamespaceViolation {
                namespace: namespace.to_string(),
                actor: actor.to_string(),
                owner: self.owned[namespace].clone(),
            })
        }
    }

    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }
}
