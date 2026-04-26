//! Pipeline context types — `PipelineContext`, `DraftEntity`, `DraftEdge`, and
//! `StageFailure` — threaded through every stage of the v0.3 resolution
//! pipeline. See `.gid/features/v03-resolution/design.md` §3.
//!
//! These types are **not** the persisted forms (`Entity`, `Edge`); they are
//! the in-flight slice of the pipeline. The conversion from `Draft*` to
//! persisted rows happens in §3.4 (resolution decision) + §3.5 (persist).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entities::ExtractedEntity;
use crate::graph::{
    Edge, Entity, EntityKind, Predicate, ResolutionMethod, SomaticFingerprint,
};
use crate::triple::Triple;
use crate::types::MemoryRecord;

/// Pipeline stage identifier — used in `StageFailure.stage` and tracing spans.
///
/// The order matches the design's §3 numbering. Variants are append-only;
/// re-ordering or removing a variant is a breaking change to the audit trail
/// schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    /// §3.1 — ingest into queue. Failure here is non-fatal (L1/L2 already
    /// committed).
    Ingest,
    /// §3.2 — entity extraction (Aho-Corasick / regex over content).
    EntityExtract,
    /// §3.3 — edge extraction (LLM call to `TripleExtractor`).
    EdgeExtract,
    /// §3.4 — resolution: candidate retrieval, fusion, entity/edge decisions.
    Resolve,
    /// §3.5 — single-transaction persist of memory + entities + edges.
    Persist,
}

impl PipelineStage {
    /// Stable string form used in DB rows and metric labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::EntityExtract => "entity_extract",
            Self::EdgeExtract => "edge_extract",
            Self::Resolve => "resolve",
            Self::Persist => "persist",
        }
    }
}

/// Structured failure record for a single pipeline stage.
///
/// **Never silent (GUARD-2 / GOAL-2.3).** Every failure that prevents a stage
/// from completing produces one of these. Multiple failures may accumulate on
/// a single `PipelineContext.failures` vec when stages are independent (e.g.
/// edge extraction fails but entity extraction succeeded — both proceed and
/// the persist stage records the partial result + the typed failure).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageFailure {
    pub stage: PipelineStage,
    /// Coarse error category for grouping/alerting (network, llm_5xx, parse,
    /// queue_full, etc.). Not an enum yet — kept as `String` until we have
    /// real error data to taxonomize.
    pub kind: String,
    /// Human-readable detail. May contain LLM error messages verbatim.
    pub message: String,
    /// When the failure was observed.
    pub at: DateTime<Utc>,
}

impl StageFailure {
    pub fn new(stage: PipelineStage, kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage,
            kind: kind.into(),
            message: message.into(),
            at: Utc::now(),
        }
    }
}

/// Draft entity — `ExtractedEntity` mention upgraded to v0.3 shape but **not
/// yet resolved** to a canonical id. Resolution (§3.4.3) decides whether the
/// draft becomes a new `Entity` row or merges into an existing one.
///
/// The draft carries enough information to:
/// 1. Run candidate retrieval against `GraphStore::search_candidates`.
/// 2. Score signals (s1–s8) for fusion.
/// 3. Build the final `Entity` row when the decision is "create".
///
/// Subtype loss from v0.2 `EntityType` (e.g. File / Url / Technology folding
/// into v0.3 `Artifact`) is recorded in `subtype_hint` so downstream code can
/// preserve precision in `Entity.summary` or alias rows.
#[derive(Clone, Debug)]
pub struct DraftEntity {
    pub canonical_name: String,
    pub kind: EntityKind,
    /// v0.2 mention's normalized form, used as the seed alias.
    pub aliases: Vec<String>,
    /// Lossy-mapped subtype hint when the v0.3 `EntityKind` is broader than
    /// the v0.2 `EntityType`. `None` when the mapping was lossless.
    pub subtype_hint: Option<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Affect snapshot at write time (GUARD-8: immutable after capture).
    pub somatic_fingerprint: Option<SomaticFingerprint>,
}

/// Draft edge — `Triple` upgraded to v0.3 shape with subject/object as
/// **draft pointers** (entity name not yet resolved to UUID). Resolution
/// (§3.4.4) decides ADD / UPDATE / NONE and binds names to entity IDs.
#[derive(Clone, Debug)]
pub struct DraftEdge {
    /// Subject's draft entity name (resolved to `EntityId` in §3.4.4).
    pub subject_name: String,
    pub predicate: Predicate,
    /// Object — either an entity name (most cases) or a literal value
    /// (e.g. `Edge::Literal` semantics from v0.3 graph layer).
    pub object: DraftEdgeEnd,
    /// Confidence carried over from `Triple.confidence`. Final confidence
    /// after fusion is computed in §3.4.4.
    pub source_confidence: f64,
    /// How this edge will be resolved if persisted (set by §3.4.4 — defaults
    /// to `Cheap` for non-LLM signals, `LlmTieBreak` if LLM was called).
    pub resolution_method: ResolutionMethod,
}

/// Object side of a `DraftEdge` — either a named entity (will be resolved to
/// UUID) or a literal value.
#[derive(Clone, Debug)]
pub enum DraftEdgeEnd {
    EntityName(String),
    Literal(String),
}

/// In-flight context threaded through every stage. Mutated in place.
///
/// Reads are open to extractors (they only need `memory.content`). Writes
/// happen after each stage; the resolution stage (§3.4) is the only one that
/// mutates the resolution slots, and persist (§3.5) is the only one that
/// reads everything to build the final transaction.
pub struct PipelineContext {
    /// The v0.2 memory row, extended with `episode_id` once L1 is minted.
    pub memory: MemoryRecord,
    /// L1 episode anchor (created in §3.1 — `Uuid::now_v7` for time-ordered
    /// inserts).
    pub episode_id: Uuid,
    /// Captured at write time (§3.1) and immutable thereafter (GUARD-8).
    pub affect_snapshot: Option<SomaticFingerprint>,

    /// Filled by §3.2.
    pub extracted_entities: Vec<ExtractedEntity>,
    /// Filled by §3.3.
    pub extracted_triples: Vec<Triple>,

    /// Filled by §3.4 — one entry per `extracted_entities[i]` (in order).
    pub entity_drafts: Vec<DraftEntity>,
    /// Filled by §3.4 — one entry per `extracted_triples[i]` (in order).
    pub edge_drafts: Vec<DraftEdge>,

    /// Resolved entities — set after §3.4.3 decides each draft's fate. Empty
    /// until then.
    pub resolved_entities: Vec<Entity>,
    /// Resolved edges — set after §3.4.4. Empty until then.
    pub resolved_edges: Vec<Edge>,

    /// Accumulator for stage failures (GOAL-2.3). May be non-empty even
    /// when the pipeline as a whole succeeds (partial completion).
    pub failures: Vec<StageFailure>,
}

impl PipelineContext {
    /// Build a new context for a fresh memory record.
    pub fn new(
        memory: MemoryRecord,
        episode_id: Uuid,
        affect_snapshot: Option<SomaticFingerprint>,
    ) -> Self {
        Self {
            memory,
            episode_id,
            affect_snapshot,
            extracted_entities: Vec::new(),
            extracted_triples: Vec::new(),
            entity_drafts: Vec::new(),
            edge_drafts: Vec::new(),
            resolved_entities: Vec::new(),
            resolved_edges: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// Record a stage failure. Convenience wrapper around `StageFailure::new`
    /// + push.
    pub fn record_failure(
        &mut self,
        stage: PipelineStage,
        kind: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.failures.push(StageFailure::new(stage, kind, message));
    }

    /// True iff any stage failure has been recorded.
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    /// Iterator over failures filtered to a specific stage.
    pub fn failures_for(&self, stage: PipelineStage) -> impl Iterator<Item = &StageFailure> {
        self.failures.iter().filter(move |f| f.stage == stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MemoryLayer, MemoryType};

    fn fixture_memory() -> MemoryRecord {
        MemoryRecord {
            id: "deadbeef".into(),
            content: "test content".into(),
            memory_type: MemoryType::Episodic,
            layer: MemoryLayer::Working,
            created_at: Utc::now(),
            access_times: Vec::new(),
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "test".into(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    #[test]
    fn pipeline_stage_as_str_stable() {
        assert_eq!(PipelineStage::Ingest.as_str(), "ingest");
        assert_eq!(PipelineStage::EntityExtract.as_str(), "entity_extract");
        assert_eq!(PipelineStage::EdgeExtract.as_str(), "edge_extract");
        assert_eq!(PipelineStage::Resolve.as_str(), "resolve");
        assert_eq!(PipelineStage::Persist.as_str(), "persist");
    }

    #[test]
    fn stage_failure_serializes_round_trip() {
        let f = StageFailure::new(PipelineStage::EdgeExtract, "llm_5xx", "Anthropic 502");
        let json = serde_json::to_string(&f).unwrap();
        let back: StageFailure = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stage, PipelineStage::EdgeExtract);
        assert_eq!(back.kind, "llm_5xx");
        assert_eq!(back.message, "Anthropic 502");
    }

    #[test]
    fn pipeline_context_starts_empty() {
        let mem = fixture_memory();
        let ctx = PipelineContext::new(mem, Uuid::new_v4(), None);
        assert!(ctx.extracted_entities.is_empty());
        assert!(ctx.extracted_triples.is_empty());
        assert!(ctx.entity_drafts.is_empty());
        assert!(ctx.edge_drafts.is_empty());
        assert!(ctx.resolved_entities.is_empty());
        assert!(ctx.resolved_edges.is_empty());
        assert!(!ctx.has_failures());
    }

    #[test]
    fn record_failure_accumulates() {
        let mem = fixture_memory();
        let mut ctx = PipelineContext::new(mem, Uuid::new_v4(), None);
        ctx.record_failure(PipelineStage::EntityExtract, "panic", "regex blew up");
        ctx.record_failure(PipelineStage::EdgeExtract, "llm_5xx", "Anthropic timeout");
        assert!(ctx.has_failures());
        assert_eq!(ctx.failures.len(), 2);

        let edge_fails: Vec<_> = ctx.failures_for(PipelineStage::EdgeExtract).collect();
        assert_eq!(edge_fails.len(), 1);
        assert_eq!(edge_fails[0].kind, "llm_5xx");

        let persist_fails: Vec<_> = ctx.failures_for(PipelineStage::Persist).collect();
        assert!(persist_fails.is_empty());
    }
}
