//! # v0.3 Graph Layer
//!
//! Knowledge graph (L3'/L4) for episodic memories. See
//! `.gid/features/v03-graph-layer/design.md` for the full spec.
//!
//! ## Modules
//!
//! - [`entity`] — L3' Entity records (people, places, concepts) + history versioning.
//! - [`edge`] — L4 bi-temporal Edge records (subject, predicate, object) with valid_from/valid_to.
//! - [`schema`] — Predicate enum + CanonicalPredicate registry + directionality rules.
//! - [`delta`] — `GraphDelta` cross-feature handoff type with canonical BLAKE3 hashing (§5bis).
//! - [`topic`] — L5 KnowledgeTopic (synthesized cluster summaries).
//! - [`affect`] — SomaticFingerprint affect snapshot attached to edges.
//! - [`audit`] — PipelineRun, ResolutionTrace, ExtractionFailure (append-only audit rows).
//! - [`telemetry`] — TelemetrySink trait + WatermarkTracker for OperationalLoad / ResourcePressure emission.
//! - [`layer_classify`] — `pub(crate)` MemoryRecord → MemoryLayer pure classifier.
//! - [`error`] — GraphError enum (§7).
//!
//! ## Boundary rules
//!
//! - The graph layer is a strict **publisher** of telemetry signals — it never consumes any
//!   signal. This keeps the dependency DAG acyclic (graph → telemetry, never the reverse).
//! - Bi-temporal: edges have both `valid_from`/`valid_to` (when the fact held) and
//!   `recorded_at` (when we learned it). Supersession never deletes; it sets `valid_to`.
//! - Append-only audit: PipelineRun status transitions are one-way (Running → terminal).

pub mod affect;
pub mod audit;
pub mod delta;
pub mod edge;
pub mod entity;
pub mod error;
pub mod layer_classify;
pub mod schema;
pub mod storage_graph;
pub mod store;
pub mod telemetry;
pub mod topic;
pub use affect::SomaticFingerprint;
pub use audit::{ExtractionFailure, PipelineKind, PipelineRun, ResolutionTrace, RunStatus};
pub use delta::{
    ApplyReport, EdgeInvalidation, EntityMerge, GraphDelta, MemoryEntityMention,
    ProposedPredicate, StageFailureRow, GRAPH_DELTA_SCHEMA_VERSION,
};
pub use edge::{ConfidenceSource, Edge, EdgeEnd, ResolutionMethod};
pub use entity::{validate_attributes, Entity, EntityKind, HistoryEntry, RESERVED_ATTRIBUTE_KEYS};
pub use error::GraphError;
pub use schema::{CanonicalPredicate, Directionality, Predicate};
pub use storage_graph::init_graph_tables;
pub use store::{
    EntityMentions, GraphStore, MergeReport, ProposedPredicateStats, SqliteGraphStore,
    GRAPH_TABLES,
};
pub use telemetry::{
    emit_operational_load, emit_pressure_if_crossed, NoopSink, TelemetrySink, WatermarkTracker,
};
pub use topic::KnowledgeTopic;

#[cfg(test)]
mod public_api_smoke {
    //! Smoke test: the canonical public symbols are reachable via `crate::graph::*`.
    //! If a re-export is accidentally dropped, this fails to compile — which is the point.

    #[test]
    fn public_symbols_compile() {
        // Importing the names is the assertion; no runtime check needed.
        #[allow(unused_imports)]
        use crate::graph::{
            // entity
            Entity, EntityKind, HistoryEntry, RESERVED_ATTRIBUTE_KEYS,
            // edge
            Edge, EdgeEnd, ConfidenceSource, ResolutionMethod,
            // schema
            Predicate, CanonicalPredicate, Directionality,
            // delta
            GraphDelta, EntityMerge, EdgeInvalidation, MemoryEntityMention,
            ProposedPredicate, StageFailureRow, ApplyReport, GRAPH_DELTA_SCHEMA_VERSION,
            // topic
            KnowledgeTopic,
            // affect
            SomaticFingerprint,
            // audit
            PipelineRun, PipelineKind, RunStatus, ExtractionFailure, ResolutionTrace,
            // telemetry
            TelemetrySink, NoopSink, WatermarkTracker,
            emit_operational_load, emit_pressure_if_crossed,
            // error
            GraphError,
        };
        // Concrete construction smoke: build one of each "easy" type to ensure the
        // re-exports are not just type aliases that lost their constructors.
        let _ = NoopSink;
        let _ = WatermarkTracker::new(100);
        let _ = ApplyReport::new();
    }
}
