//! §3.3 Edge-extraction stage driver.
//!
//! Wraps a [`TripleExtractor`] (LLM-backed in production; rule-based or
//! mock in tests) and lifts each emitted [`Triple`] into a [`DraftEdge`]
//! ready for resolution (§3.4.4).
//!
//! Boundary rules (mirrors `mod.rs` boundaries):
//! - Never panics.
//! - Extractor errors are recorded as `StageFailure { stage: EdgeExtract }`
//!   on `ctx.failures` and returned as `Err(())`. The driver may decide to
//!   continue the pipeline with `extracted_triples = []` (entity-only run)
//!   or short-circuit, depending on policy. We do not decide here.
//! - Predicate normalization is total: every emitted `Triple.predicate` is
//!   mapped to a v0.3 [`Predicate`] via the adapter — canonical predicates
//!   pass through, novel labels become `Predicate::Proposed(label)`.

use crate::resolution::adapters::map_predicate;
use crate::resolution::context::{
    DraftEdge, DraftEdgeEnd, PipelineContext, PipelineStage,
};
use crate::resolution::stage_extract::StageError;
use crate::triple_extractor::TripleExtractor;
use crate::graph::ResolutionMethod;

/// Run §3.3: invoke the triple extractor on `ctx.memory.content`, populate
/// `ctx.extracted_triples`, and lift each into a `DraftEdge` placed (in
/// order) into `ctx.edge_drafts`.
///
/// Behavior:
/// - On extractor success: drafts are emitted; `Ok(())`.
/// - On extractor error: a `StageFailure { stage: EdgeExtract, kind:
///   "extractor_error", … }` is appended to `ctx.failures` and `Err(())`
///   is returned. The driver decides whether to continue with the
///   entity-only path or abort.
///
/// The default `resolution_method` for emitted drafts is
/// [`ResolutionMethod::Automatic`] — drafts may be upgraded to
/// `LlmTieBreaker` later in §3.4.4 if a tie-breaker LLM call is invoked.
pub fn extract_edges(
    extractor: &dyn TripleExtractor,
    ctx: &mut PipelineContext,
) -> Result<(), StageError> {
    let memory_id = ctx.memory.id.clone();

    let triples = match extractor.extract_triples(&ctx.memory.content) {
        Ok(t) => t,
        Err(e) => {
            let msg = e.to_string();
            log::warn!(
                target: "resolution.stage_edge_extract",
                "edge extraction failed: memory_id={} error={}",
                memory_id, msg
            );
            ctx.record_failure(
                PipelineStage::EdgeExtract,
                "extractor_error",
                msg,
            );
            return Err(StageError);
        }
    };

    // Lift each triple into a DraftEdge (1:1, order preserved). Predicate
    // normalization handles both canonical (lossless) and novel
    // (Proposed-preserving) v0.2 labels.
    ctx.edge_drafts.clear();
    ctx.edge_drafts.reserve(triples.len());
    for t in &triples {
        ctx.edge_drafts.push(DraftEdge {
            subject_name: t.subject.clone(),
            predicate: map_predicate(&t.predicate),
            // v0.2 `Triple.object` is always a string. We treat it as an
            // entity name by default; literal-vs-entity discrimination is
            // future work and out of scope for v0.3 (currently every v0.2
            // edge has an entity object — the v0.3 graph layer reserves
            // `DraftEdgeEnd::Literal` for the future literal-edge variant).
            object: DraftEdgeEnd::EntityName(t.object.clone()),
            source_confidence: t.confidence.clamp(0.0, 1.0),
            resolution_method: ResolutionMethod::Automatic,
        });
    }

    ctx.extracted_triples = triples;

    log::debug!(
        target: "resolution.stage_edge_extract",
        "edge extraction complete: memory_id={} triples={}",
        memory_id,
        ctx.extracted_triples.len()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{CanonicalPredicate, Predicate};
    use crate::resolution::context::PipelineContext;
    use crate::triple::{Predicate as V02Predicate, Triple, TripleSource};
    use crate::triple_extractor::TripleExtractor;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;
    use std::error::Error;
    use uuid::Uuid;

    /// Test extractor that returns a fixed triple list.
    struct StaticExtractor(Vec<Triple>);
    impl TripleExtractor for StaticExtractor {
        fn extract_triples(
            &self,
            _content: &str,
        ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
            Ok(self.0.clone())
        }
    }

    /// Test extractor that errors.
    struct FailingExtractor(&'static str);
    impl TripleExtractor for FailingExtractor {
        fn extract_triples(
            &self,
            _content: &str,
        ) -> Result<Vec<Triple>, Box<dyn Error + Send + Sync>> {
            Err(self.0.into())
        }
    }

    fn fixture_memory(content: &str) -> MemoryRecord {
        MemoryRecord {
            id: "edge-extract-test".into(),
            content: content.into(),
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

    fn ctx_for(content: &str) -> PipelineContext {
        PipelineContext::new(fixture_memory(content), Uuid::new_v4(), None)
    }

    #[test]
    fn empty_extraction_yields_no_drafts() {
        let extractor = StaticExtractor(vec![]);
        let mut ctx = ctx_for("nothing here");
        extract_edges(&extractor, &mut ctx).unwrap();
        assert!(ctx.extracted_triples.is_empty());
        assert!(ctx.edge_drafts.is_empty());
        assert!(!ctx.has_failures());
    }

    #[test]
    fn drafts_one_per_triple_in_order() {
        let extractor = StaticExtractor(vec![
            Triple::new(
                "Alice".into(),
                V02Predicate::Uses,
                "Rust".into(),
                0.9,
            ),
            Triple::new(
                "Bob".into(),
                V02Predicate::IsA,
                "Engineer".into(),
                0.8,
            ),
        ]);
        let mut ctx = ctx_for("Alice uses Rust. Bob is an engineer.");
        extract_edges(&extractor, &mut ctx).unwrap();
        assert_eq!(ctx.extracted_triples.len(), ctx.edge_drafts.len());
        assert_eq!(ctx.edge_drafts.len(), 2);
        assert_eq!(ctx.edge_drafts[0].subject_name, "Alice");
        assert_eq!(ctx.edge_drafts[1].subject_name, "Bob");
    }

    #[test]
    fn predicate_canonical_passes_through() {
        let extractor = StaticExtractor(vec![Triple::new(
            "X".into(),
            V02Predicate::DependsOn,
            "Y".into(),
            1.0,
        )]);
        let mut ctx = ctx_for("X depends on Y.");
        extract_edges(&extractor, &mut ctx).unwrap();
        let draft = &ctx.edge_drafts[0];
        match &draft.predicate {
            Predicate::Canonical(CanonicalPredicate::DependsOn) => {}
            other => panic!("expected canonical DependsOn, got {:?}", other),
        }
    }

    #[test]
    fn confidence_is_clamped_to_unit_interval() {
        // Triple::new already clamps, but verify the draft preserves the
        // clamped value (i.e. doesn't accidentally re-widen).
        let extractor = StaticExtractor(vec![Triple {
            subject: "A".into(),
            predicate: V02Predicate::RelatedTo,
            object: "B".into(),
            confidence: 1.5, // intentional out-of-range
            source: TripleSource::Llm,
        }]);
        let mut ctx = ctx_for("A relates to B.");
        extract_edges(&extractor, &mut ctx).unwrap();
        let c = ctx.edge_drafts[0].source_confidence;
        assert!((0.0..=1.0).contains(&c), "confidence not clamped: {}", c);
    }

    #[test]
    fn extractor_error_records_failure_and_returns_err() {
        let extractor = FailingExtractor("LLM 503");
        let mut ctx = ctx_for("anything");
        let res = extract_edges(&extractor, &mut ctx);
        assert!(res.is_err());
        assert!(ctx.has_failures());
        let f = ctx
            .failures_for(PipelineStage::EdgeExtract)
            .next()
            .expect("failure recorded");
        assert_eq!(f.kind, "extractor_error");
        assert!(f.message.contains("LLM 503"));
        // No drafts produced on failure path.
        assert!(ctx.edge_drafts.is_empty());
    }

    #[test]
    fn rerun_overwrites_drafts_does_not_accumulate() {
        let extractor = StaticExtractor(vec![Triple::new(
            "A".into(),
            V02Predicate::Uses,
            "B".into(),
            0.7,
        )]);
        let mut ctx = ctx_for("A uses B.");
        extract_edges(&extractor, &mut ctx).unwrap();
        let n1 = ctx.edge_drafts.len();
        extract_edges(&extractor, &mut ctx).unwrap();
        let n2 = ctx.edge_drafts.len();
        assert_eq!(n1, n2);
    }

    #[test]
    fn default_resolution_method_is_automatic() {
        let extractor = StaticExtractor(vec![Triple::new(
            "A".into(),
            V02Predicate::IsA,
            "B".into(),
            0.5,
        )]);
        let mut ctx = ctx_for("A is a B.");
        extract_edges(&extractor, &mut ctx).unwrap();
        assert_eq!(
            ctx.edge_drafts[0].resolution_method,
            ResolutionMethod::Automatic
        );
    }
}
