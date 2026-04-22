//! Write-path API types.
//!
//! Defines the public return types for `store_enriched` / `store_raw`
//! (to be implemented in Step 4) and the structured error hierarchy
//! that replaces the current "Option<serde_json::Value> metadata" API.
//!
//! See design §3.1, §3.2, §4.2 of ISS-019.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::MemoryType;

// ---------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------

/// Memory id, identical to the `String` used everywhere in engram today.
///
/// Kept as a type alias for now to avoid churn; promoting to a newtype
/// is a separate refactor.
pub type MemoryId = String;

/// Quarantine row id. Newtype so it cannot be confused with `MemoryId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuarantineId(pub String);

impl QuarantineId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Content hash for deduplication of skipped / quarantined content.
///
/// Hex-encoded SHA-256 is the canonical format; constructor caller is
/// responsible for the choice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub String);

impl ContentHash {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------
// StorageMeta — caller-supplied write context
// ---------------------------------------------------------------------

/// Metadata the caller provides to `store_raw`.
///
/// Replaces the previous `Option<serde_json::Value>` catch-all —
/// every meaningful field is explicit, user-supplied extras live in
/// `user_metadata` (namespaced away from engram internals).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageMeta {
    /// Caller's hint at importance. Merged with the extractor's
    /// inferred importance at store time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance_hint: Option<f64>,

    /// Logical source (e.g., "telegram", "rebuild-pilot", "agent-loop").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Namespace isolation (multi-agent / per-project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Arbitrary caller-supplied JSON. Stored under `user.*` namespace
    /// on disk so it cannot collide with engram internals.
    #[serde(default)]
    pub user_metadata: serde_json::Value,

    /// Legacy callers (`add()`) pass an explicit `MemoryType`. When
    /// no extractor is configured, this hint lets the minimal path
    /// preserve the old explicit-type behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_type_hint: Option<MemoryType>,
}

// ---------------------------------------------------------------------
// Outcomes — enriched / raw paths
// ---------------------------------------------------------------------

/// Outcome of `store_enriched`: the row was either newly inserted or
/// merged into an existing similar one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum StoreOutcome {
    Inserted {
        id: MemoryId,
    },
    Merged {
        id: MemoryId,
        similarity: f32,
    },
}

impl StoreOutcome {
    /// The resulting memory id in either branch.
    pub fn id(&self) -> &MemoryId {
        match self {
            StoreOutcome::Inserted { id } | StoreOutcome::Merged { id, .. } => id,
        }
    }
}

/// Why a raw write produced no row.
///
/// Extractor returned empty facts — nothing was memory-worthy — is
/// the primary case. Distinct from "extractor failed", which is
/// `QuarantineReason` + `Quarantined` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Extractor ran successfully but produced zero facts.
    NoFactsExtracted,
    /// Content-hash deduplication matched an already-stored memory.
    DuplicateContent,
    /// Content was below the minimum length threshold (if configured).
    TooShort,
}

/// Why content was quarantined instead of stored.
///
/// All of these represent extractor-runtime problems — distinct from
/// `StoreError`, which is programmer error (DB unreachable, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum QuarantineReason {
    /// Extractor exceeded its deadline.
    ExtractorTimeout,
    /// Extractor returned a typed error (API error, malformed JSON, etc.).
    ExtractorError(String),
    /// Extractor panicked — caught by the wrapper.
    ExtractorPanic,
    /// Extractor returned facts but every one failed validation
    /// (e.g., all `core_fact` were empty).
    AllFactsInvalid(String),
    /// Non-extractor pipeline failure before the row could be written.
    PipelineError(String),
}

/// Outcome of `store_raw`: the content either produced one or more
/// stored rows, was intentionally skipped, or was quarantined.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum RawStoreOutcome {
    /// Extractor produced facts (or minimal fallback used); each one
    /// stored or merged.
    Stored(Vec<StoreOutcome>),

    /// Nothing stored — content wasn't memory-worthy.
    Skipped {
        reason: SkipReason,
        content_hash: ContentHash,
    },

    /// Extractor failed; content preserved in quarantine for retry.
    Quarantined {
        id: QuarantineId,
        reason: QuarantineReason,
    },
}

// ---------------------------------------------------------------------
// StoreError — programmer error hierarchy
// ---------------------------------------------------------------------

/// Errors that prevent the store operation from even reaching a
/// success/skip/quarantine decision.
///
/// Per design §3.2: extractor failure is **not** a `StoreError` —
/// it's a legitimate outcome modeled by `RawStoreOutcome::Quarantined`.
/// The `Quarantined` variant here exists only for the legacy shim
/// path (§3.3) so legacy callers are not silently ignored when the
/// new pipeline quarantines content.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Underlying storage (SQLite) failure.
    #[error("storage error: {0}")]
    DbError(#[from] rusqlite::Error),

    /// Construction of EnrichedMemory / Dimensions failed.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Invariant violation that should have been caught earlier.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// Embedding pipeline failed (model unavailable, etc.).
    #[error("embedding failed: {0}")]
    EmbeddingError(String),

    /// Surfaced by the legacy shim (`Memory::add` / etc.) when the new
    /// pipeline quarantined the content. Converts a non-error outcome
    /// into an error for old callers that had no concept of quarantine.
    #[error("content quarantined: {reason:?} (id={id:?})")]
    Quarantined {
        id: QuarantineId,
        reason: QuarantineReason,
    },
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_outcome_id_accessor() {
        let inserted = StoreOutcome::Inserted {
            id: "m-1".to_string(),
        };
        let merged = StoreOutcome::Merged {
            id: "m-2".to_string(),
            similarity: 0.9,
        };
        assert_eq!(inserted.id(), "m-1");
        assert_eq!(merged.id(), "m-2");
    }

    #[test]
    fn storage_meta_default() {
        let m = StorageMeta::default();
        assert!(m.importance_hint.is_none());
        assert!(m.source.is_none());
        assert!(m.namespace.is_none());
        assert!(m.memory_type_hint.is_none());
        assert_eq!(m.user_metadata, serde_json::Value::Null);
    }

    #[test]
    fn quarantine_id_roundtrip() {
        let q = QuarantineId::new("q-abc");
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, "\"q-abc\"");
        let decoded: QuarantineId = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, q);
    }

    #[test]
    fn skip_reason_serde() {
        let json = serde_json::to_string(&SkipReason::NoFactsExtracted).unwrap();
        assert_eq!(json, "\"no_facts_extracted\"");
        let decoded: SkipReason = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, SkipReason::NoFactsExtracted);
    }

    #[test]
    fn quarantine_reason_serde_roundtrip() {
        for r in [
            QuarantineReason::ExtractorTimeout,
            QuarantineReason::ExtractorError("503".to_string()),
            QuarantineReason::ExtractorPanic,
            QuarantineReason::AllFactsInvalid("empty core_fact".to_string()),
            QuarantineReason::PipelineError("bus closed".to_string()),
        ] {
            let j = serde_json::to_string(&r).unwrap();
            let back: QuarantineReason = serde_json::from_str(&j).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn raw_store_outcome_stored_variant_serde() {
        let o = RawStoreOutcome::Stored(vec![StoreOutcome::Inserted {
            id: "x".to_string(),
        }]);
        let j = serde_json::to_string(&o).unwrap();
        // Ensure tag present.
        assert!(j.contains("\"kind\":\"stored\""));
    }

    #[test]
    fn store_error_from_rusqlite_compiles() {
        // Smoke-check the #[from] conversion compiles and Display works.
        let e: StoreError = rusqlite::Error::QueryReturnedNoRows.into();
        let s = format!("{e}");
        assert!(s.contains("storage error"));
    }
}
