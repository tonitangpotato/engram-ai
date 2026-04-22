//! `EnrichedMemory` — the validated, typed counterpart of
//! `ExtractedFact`.
//!
//! `ExtractedFact` is the loose JSON-friendly wire format the LLM
//! emits (see `extractor.rs`). `EnrichedMemory` is its validated
//! form: every instance is guaranteed to carry a non-empty
//! `Dimensions`, a clamped `Importance`, and content that is kept
//! in sync with `dimensions.core_fact`.
//!
//! This is the **only** type accepted by the primary write path
//! (`store_enriched` in Step 4 of the ISS-019 plan). See design
//! §2.3 of ISS-019.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dimensions::{
    Confidence, Dimensions, Domain, EmptyCoreFactError, Importance, NonEmptyString, TemporalMark,
    Valence,
};
use crate::embeddings::{EmbeddingError, EmbeddingProvider};
use crate::extractor::ExtractedFact;
use crate::type_weights::infer_type_weights;

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// Reasons `EnrichedMemory::from_extracted` / `::minimal` can refuse
/// to construct a value.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConstructionError {
    /// `core_fact` (or `content` for `minimal`) was empty or
    /// whitespace-only.
    #[error("core_fact is empty or whitespace-only")]
    EmptyCoreFact,
}

impl From<EmptyCoreFactError> for ConstructionError {
    fn from(_: EmptyCoreFactError) -> Self {
        ConstructionError::EmptyCoreFact
    }
}

// ---------------------------------------------------------------------
// EnrichedMemory
// ---------------------------------------------------------------------

/// A validated memory ready for the write path.
///
/// Invariants (enforced by constructors):
/// - `dimensions.core_fact` is non-empty.
/// - `content` equals `dimensions.core_fact.as_str()` byte-for-byte.
///   (Kept redundant to preserve ergonomics for FTS / search index
///    code that already reads `.content`; constructors sync them.)
/// - `importance` is in `[0.0, 1.0]` (by construction of `Importance`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichedMemory {
    /// Plain-text content. Always equal to `dimensions.core_fact`.
    pub content: String,

    /// Typed dimensional signature.
    pub dimensions: Dimensions,

    /// Pre-computed embedding, if the caller has one. `None` means
    /// the write path will embed inline. `Some(v)` lets batching
    /// callers (the rebuild pilot) bypass the per-row embedding
    /// call — see `precompute_embeddings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,

    /// Caller-supplied or extractor-derived importance.
    pub importance: Importance,

    /// Logical source label (e.g. "telegram", "rebuild-pilot").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Namespace isolation (multi-agent / per-project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Caller-supplied JSON. Stored under the `user.*` metadata
    /// namespace on disk, never colliding with engram internals.
    #[serde(default)]
    pub user_metadata: serde_json::Value,
}

impl EnrichedMemory {
    // -----------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------

    /// Build an `EnrichedMemory` from an extractor-produced
    /// `ExtractedFact`.
    ///
    /// Fails if `fact.core_fact` is empty or whitespace-only.
    ///
    /// - `participants` / `location` / `context` / `causation` /
    ///   `outcome` / `method` / `relations` / `sentiment` / `stance`
    ///   are carried over as-is (narrative fields, `Option<String>`).
    /// - `temporal` is parsed via `parse_temporal_mark` — strings
    ///   that match ISO-8601 datetime or `YYYY-MM-DD` become
    ///   `TemporalMark::Exact` / `Day`; everything else becomes
    ///   `TemporalMark::Vague` (preserves the string losslessly).
    /// - `valence` is clamped via `Valence::new`.
    /// - `domain` is parsed via `Domain::from_loose_str`.
    /// - `confidence` is parsed via `Confidence::from_loose_str`.
    /// - `tags` become a `BTreeSet` (dedup + stable ordering).
    /// - `type_weights` is computed via `infer_type_weights` — the
    ///   same inference the legacy `add_to_namespace` path used.
    /// - `importance` is clamped to `[0.0, 1.0]`.
    pub fn from_extracted(
        fact: ExtractedFact,
        source: Option<String>,
        namespace: Option<String>,
        user_metadata: serde_json::Value,
    ) -> Result<Self, ConstructionError> {
        let core_fact = NonEmptyString::new(fact.core_fact.clone())?;
        let type_weights = infer_type_weights(&fact);

        let mut tags = std::collections::BTreeSet::new();
        for t in fact.tags.iter() {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                tags.insert(trimmed.to_string());
            }
        }

        let dimensions = Dimensions {
            core_fact,
            participants: fact.participants.clone(),
            temporal: fact.temporal.as_deref().map(parse_temporal_mark),
            location: fact.location.clone(),
            context: fact.context.clone(),
            causation: fact.causation.clone(),
            outcome: fact.outcome.clone(),
            method: fact.method.clone(),
            relations: fact.relations.clone(),
            sentiment: fact.sentiment.clone(),
            stance: fact.stance.clone(),
            valence: Valence::new(fact.valence),
            domain: Domain::from_loose_str(&fact.domain),
            confidence: Confidence::from_loose_str(&fact.confidence),
            tags,
            type_weights,
        };

        let content = dimensions.core_fact.as_str().to_string();

        Ok(Self {
            content,
            dimensions,
            embedding: None,
            importance: Importance::new(fact.importance),
            source,
            namespace,
            user_metadata,
        })
    }

    /// Build an `EnrichedMemory` from raw content with minimal
    /// `Dimensions`.
    ///
    /// Used when no extractor is configured (FINDING-4) or during
    /// legacy row migration (FINDING-5). Not the same as extractor
    /// failure — failure is routed to the quarantine table instead.
    pub fn minimal(
        content: &str,
        importance: Importance,
        source: Option<String>,
        namespace: Option<String>,
    ) -> Result<Self, ConstructionError> {
        let dimensions = Dimensions::minimal(content)?;
        let content_str = dimensions.core_fact.as_str().to_string();
        Ok(Self {
            content: content_str,
            dimensions,
            embedding: None,
            importance,
            source,
            namespace,
            user_metadata: serde_json::Value::Null,
        })
    }

    /// Build an `EnrichedMemory` from an already-validated
    /// `Dimensions`. For internal callers that already hold the
    /// typed value (e.g., merge path, backfill).
    pub fn from_dimensions(
        dimensions: Dimensions,
        importance: Importance,
        source: Option<String>,
        namespace: Option<String>,
        user_metadata: serde_json::Value,
    ) -> Self {
        let content = dimensions.core_fact.as_str().to_string();
        Self {
            content,
            dimensions,
            embedding: None,
            importance,
            source,
            namespace,
            user_metadata,
        }
    }

    // -----------------------------------------------------------------
    // Accessors / mutators
    // -----------------------------------------------------------------

    /// Attach a pre-computed embedding. Returns `self` so batch
    /// helpers can chain.
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// True iff `content` matches `dimensions.core_fact`. Used by
    /// debug assertions; normal callers never see this false.
    pub fn invariants_hold(&self) -> bool {
        self.content.as_str() == self.dimensions.core_fact.as_str()
    }
}

// ---------------------------------------------------------------------
// Temporal parsing — ExtractedFact.temporal : String → TemporalMark
// ---------------------------------------------------------------------

/// Parse an extractor-emitted temporal string into a typed mark.
///
/// Accepts:
/// - RFC-3339 / ISO-8601 datetime (`2026-04-22T01:56:24Z`) → `Exact`.
/// - Calendar date (`YYYY-MM-DD`) → `Day`.
/// - Anything else → `Vague(original)` (never lossy).
///
/// Range parsing is intentionally conservative — extractor prompts
/// today produce dates and datetimes, not ranges. Ranges enter the
/// system via `merge_memory_into` and the backfill job, not via
/// fresh extraction. A future enhancement can widen this parser.
pub fn parse_temporal_mark(s: &str) -> TemporalMark {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return TemporalMark::Vague(s.to_string());
    }

    // RFC 3339 (with offset) — the common case for structured timestamps.
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return TemporalMark::Exact(dt.with_timezone(&Utc));
    }

    // Naive datetime `YYYY-MM-DDTHH:MM:SS` (assume UTC).
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        if let chrono::LocalResult::Single(dt) = Utc.from_local_datetime(&ndt) {
            return TemporalMark::Exact(dt);
        }
    }

    // Calendar day.
    if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return TemporalMark::Day(d);
    }

    TemporalMark::Vague(s.to_string())
}

// ---------------------------------------------------------------------
// Batch helper: precompute embeddings
// ---------------------------------------------------------------------

/// Pre-compute embeddings for every `EnrichedMemory` in the slice
/// that has `embedding == None`.
///
/// Aborts on the first failure and returns the error — callers that
/// want best-effort batching should filter the slice and retry.
///
/// This is the seam the rebuild pilot uses to batch embedding calls:
/// a single HTTP round-trip per N records instead of N per-row calls
/// from inside the write path.
pub fn precompute_embeddings(
    provider: &EmbeddingProvider,
    items: &mut [EnrichedMemory],
) -> Result<usize, EmbeddingError> {
    let mut filled = 0usize;
    for item in items.iter_mut() {
        if item.embedding.is_some() {
            continue;
        }
        let emb = provider.embed(&item.content)?;
        item.embedding = Some(emb);
        filled += 1;
    }
    Ok(filled)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimensions::TemporalMark;

    fn sample_fact(core: &str) -> ExtractedFact {
        ExtractedFact {
            core_fact: core.to_string(),
            participants: Some("alice, bob".to_string()),
            temporal: Some("2026-04-22".to_string()),
            location: Some("lab".to_string()),
            context: Some("team meeting".to_string()),
            causation: Some("kickoff".to_string()),
            outcome: Some("agreed".to_string()),
            method: Some("discussion".to_string()),
            relations: Some("project-x; rfc-42".to_string()),
            sentiment: Some("optimistic".to_string()),
            stance: Some("supportive".to_string()),
            importance: 0.7,
            tags: vec!["rust".to_string(), " rust ".to_string(), "".to_string()],
            confidence: "likely".to_string(),
            valence: 0.4,
            domain: "coding".to_string(),
        }
    }

    // -- from_extracted --

    #[test]
    fn from_extracted_round_trip() {
        let f = sample_fact("kickoff meeting happened");
        let em = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap();
        assert!(em.invariants_hold());
        assert_eq!(em.content, "kickoff meeting happened");
        assert_eq!(em.dimensions.participants.as_deref(), Some("alice, bob"));
        assert!(matches!(em.dimensions.temporal, Some(TemporalMark::Day(_))));
        assert_eq!(em.dimensions.domain, Domain::Coding);
        assert_eq!(em.dimensions.confidence, Confidence::Likely);
        assert!((em.dimensions.valence.get() - 0.4).abs() < 1e-9);
        // tag dedup + trim + empty-drop
        let tags: Vec<&str> = em.dimensions.tags.iter().map(|s| s.as_str()).collect();
        assert_eq!(tags, vec!["rust"]);
        assert!((em.importance.get() - 0.7).abs() < 1e-9);
    }

    #[test]
    fn from_extracted_rejects_empty_core_fact() {
        let mut f = sample_fact("");
        f.core_fact = "".to_string();
        let err = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap_err();
        assert_eq!(err, ConstructionError::EmptyCoreFact);
    }

    #[test]
    fn from_extracted_rejects_whitespace_only_core_fact() {
        let mut f = sample_fact("  \n\t ");
        f.core_fact = "   \t\n".to_string();
        let err = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap_err();
        assert_eq!(err, ConstructionError::EmptyCoreFact);
    }

    #[test]
    fn from_extracted_clamps_out_of_range_importance() {
        let mut f = sample_fact("x");
        f.importance = 5.0;
        let em = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap();
        assert!((em.importance.get() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn from_extracted_unknown_domain_becomes_other() {
        let mut f = sample_fact("x");
        f.domain = "ml".to_string();
        let em = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap();
        assert_eq!(em.dimensions.domain, Domain::Other("ml".to_string()));
    }

    #[test]
    fn from_extracted_unknown_confidence_becomes_uncertain() {
        let mut f = sample_fact("x");
        f.confidence = "idk".to_string();
        let em = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap();
        assert_eq!(em.dimensions.confidence, Confidence::Uncertain);
    }

    #[test]
    fn from_extracted_valence_nan_is_zero() {
        let mut f = sample_fact("x");
        f.valence = f64::NAN;
        let em = EnrichedMemory::from_extracted(f, None, None, serde_json::Value::Null).unwrap();
        assert_eq!(em.dimensions.valence.get(), 0.0);
    }

    // -- minimal --

    #[test]
    fn minimal_roundtrip() {
        let em = EnrichedMemory::minimal(
            "a fact",
            Importance::new(0.5),
            Some("test".to_string()),
            Some("ns1".to_string()),
        )
        .unwrap();
        assert_eq!(em.content, "a fact");
        assert_eq!(em.dimensions.core_fact.as_str(), "a fact");
        assert!(em.dimensions.participants.is_none());
        assert_eq!(em.source.as_deref(), Some("test"));
        assert_eq!(em.namespace.as_deref(), Some("ns1"));
        assert!(em.embedding.is_none());
        assert!(em.invariants_hold());
    }

    #[test]
    fn minimal_rejects_empty() {
        assert!(EnrichedMemory::minimal("", Importance::default(), None, None).is_err());
        assert!(EnrichedMemory::minimal("   ", Importance::default(), None, None).is_err());
    }

    // -- from_dimensions --

    #[test]
    fn from_dimensions_syncs_content() {
        let d = Dimensions::minimal("typed fact").unwrap();
        let em = EnrichedMemory::from_dimensions(
            d,
            Importance::new(0.4),
            None,
            None,
            serde_json::Value::Null,
        );
        assert_eq!(em.content, "typed fact");
        assert!(em.invariants_hold());
    }

    // -- with_embedding --

    #[test]
    fn with_embedding_attaches() {
        let em =
            EnrichedMemory::minimal("x", Importance::default(), None, None).unwrap();
        let em = em.with_embedding(vec![0.1, 0.2, 0.3]);
        assert_eq!(em.embedding.as_deref(), Some(&[0.1, 0.2, 0.3][..]));
    }

    // -- parse_temporal_mark --

    #[test]
    fn parse_temporal_rfc3339_becomes_exact() {
        let tm = parse_temporal_mark("2026-04-22T01:56:24Z");
        assert!(matches!(tm, TemporalMark::Exact(_)));
    }

    #[test]
    fn parse_temporal_naive_datetime_becomes_exact() {
        let tm = parse_temporal_mark("2026-04-22T01:56:24");
        assert!(matches!(tm, TemporalMark::Exact(_)));
    }

    #[test]
    fn parse_temporal_date_becomes_day() {
        let tm = parse_temporal_mark("2026-04-22");
        assert!(matches!(tm, TemporalMark::Day(_)));
    }

    #[test]
    fn parse_temporal_unparseable_becomes_vague_lossless() {
        let input = "last tuesday";
        let tm = parse_temporal_mark(input);
        match tm {
            TemporalMark::Vague(s) => assert_eq!(s, input),
            _ => panic!("expected Vague"),
        }
    }

    #[test]
    fn parse_temporal_whitespace_is_vague_preserving_input() {
        let tm = parse_temporal_mark("   ");
        match tm {
            TemporalMark::Vague(s) => assert_eq!(s, "   "),
            _ => panic!("expected Vague"),
        }
    }

    // -- serde round-trip --

    #[test]
    fn enriched_memory_serde_round_trip() {
        let em = EnrichedMemory::minimal(
            "hello",
            Importance::new(0.3),
            Some("src".to_string()),
            Some("ns".to_string()),
        )
        .unwrap()
        .with_embedding(vec![0.1, 0.2]);
        let j = serde_json::to_string(&em).unwrap();
        let back: EnrichedMemory = serde_json::from_str(&j).unwrap();
        assert_eq!(em, back);
    }
}
