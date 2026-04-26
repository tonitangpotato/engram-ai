use uuid::Uuid;

#[derive(thiserror::Error, Debug)]
pub enum GraphError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("entity not found: {0}")]
    EntityNotFound(Uuid),

    #[error("edge not found: {0}")]
    EdgeNotFound(Uuid),

    #[error("invariant violation: {0}")]
    Invariant(&'static str),

    /// Attempted to mutate an invalidated edge (GUARD-3 / GOAL-1.6).
    #[error("edge {0} is invalidated and cannot be modified")]
    EdgeFrozen(Uuid),

    /// Attempted to delete an entity with live edges.
    #[error("entity {0} has live edges; merge or supersede instead")]
    EntityHasLiveEdges(Uuid),

    /// Predicate classify failed (should be infallible, but kept explicit).
    #[error("malformed predicate: {0}")]
    MalformedPredicate(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("transaction was rolled back: {0}")]
    Rolledback(String),

    /// Schema migration failure (e.g. ALTER TABLE failed for a non-idempotent
    /// reason). String carries human-readable context including the column /
    /// table being migrated, so operators can pinpoint the failing step.
    #[error("schema migration failed: {0}")]
    Migration(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn entity_not_found_displays() {
        let err = GraphError::EntityNotFound(Uuid::nil());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn edge_not_found_displays() {
        let err = GraphError::EdgeNotFound(Uuid::nil());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn invariant_displays() {
        let err = GraphError::Invariant("bad state");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn edge_frozen_displays() {
        let err = GraphError::EdgeFrozen(Uuid::nil());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn entity_has_live_edges_displays() {
        let err = GraphError::EntityHasLiveEdges(Uuid::nil());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn malformed_predicate_displays() {
        let err = GraphError::MalformedPredicate("oops".to_string());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn rolledback_displays() {
        let err = GraphError::Rolledback("conflict".to_string());
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn sqlite_preserves_source() {
        let err: GraphError = rusqlite::Error::QueryReturnedNoRows.into();
        assert!(err.source().is_some());
    }

    #[test]
    fn serde_preserves_source() {
        let underlying = serde_json::from_str::<i32>("not a number").unwrap_err();
        let err: GraphError = underlying.into();
        assert!(err.source().is_some());
    }
}
