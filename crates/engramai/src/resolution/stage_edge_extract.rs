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

use crate::graph::audit::CATEGORY_EXTRACTOR_ERROR;
use crate::graph::{CanonicalPredicate, Predicate, ResolutionMethod};
use crate::resolution::adapters::map_predicate;
use crate::resolution::context::{DraftEdge, DraftEdgeEnd, PipelineContext, PipelineStage};
use crate::resolution::stage_extract::StageError;
use crate::triple_extractor::TripleExtractor;

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
            ctx.record_failure(PipelineStage::EdgeExtract, CATEGORY_EXTRACTOR_ERROR, msg);
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

    // ISS-204 Component 1 — emit an event-occurrence edge.
    //
    // Event-time is a first-class graph relation, not a memory-metadata
    // afterthought. When the enrichment pipeline derived a *day-precision*
    // date for this memory (`TemporalMark::Day`/`Exact` — a single calendar
    // day, the only precision a literal `OccurredOn` object can faithfully
    // carry), project it as an explicit `OccurredOn` literal-object edge so
    // temporal queries resolve by graph traversal instead of degrading to
    // vector recall over a huge competitor pool.
    //
    // Lower-precision marks (`Range`/`Approx`/`Vague`) are intentionally
    // skipped: a literal ISO day would lie about the precision. Those cases
    // (e.g. conv-26 q33/q35) need ISS-190/191 to pin the day into the mark
    // first; once they resolve to `Day`, this path picks them up unchanged.
    //
    // Subject anchor: the primary participant — the subject of the first
    // triple-derived edge. That entity is guaranteed to enter the graph
    // (it is lifted into `entity_drafts`), so the literal edge is not
    // orphaned and a query anchor that resolves to it can traverse the date.
    // When no triple was extracted there is no anchor entity to hang the
    // date on, so we skip rather than emit a dangling edge.
    if let Some(day) = ctx
        .memory
        .derived_temporal_mark()
        .and_then(|m| day_precision_iso(&m))
    {
        if let Some(anchor) = ctx
            .extracted_triples
            .first()
            .map(|t| t.subject.clone())
            .filter(|s| !s.trim().is_empty())
        {
            ctx.edge_drafts.push(DraftEdge {
                subject_name: anchor,
                predicate: Predicate::Canonical(CanonicalPredicate::OccurredOn),
                object: DraftEdgeEnd::Literal(day),
                // Derived from the store-time temporal mark, not an LLM
                // assertion; full confidence in the derivation itself.
                source_confidence: 1.0,
                resolution_method: ResolutionMethod::Automatic,
            });
        }
    }

    log::debug!(
        target: "resolution.stage_edge_extract",
        "edge extraction complete: memory_id={} triples={}",
        memory_id,
        ctx.extracted_triples.len()
    );

    Ok(())
}

/// Render a [`TemporalMark`](crate::dimensions::TemporalMark) as an ISO
/// `YYYY-MM-DD` string **iff** it pins a single calendar day.
///
/// Only `Day` and `Exact` carry day precision. `Range`/`Approx`/`Vague`
/// describe intervals or uncertainty and have no single day to assert as a
/// literal `OccurredOn` object — returning `None` for them keeps the graph
/// from claiming a precision the mark does not have (ISS-204).
fn day_precision_iso(mark: &crate::dimensions::TemporalMark) -> Option<String> {
    use crate::dimensions::TemporalMark;
    match mark {
        TemporalMark::Day(d) => Some(d.format("%Y-%m-%d").to_string()),
        TemporalMark::Exact(dt) => Some(dt.format("%Y-%m-%d").to_string()),
        TemporalMark::Range { .. } | TemporalMark::Approx { .. } | TemporalMark::Vague(_) => None,
    }
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
            occurred_at: None,
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
        PipelineContext::new(fixture_memory(content), Uuid::new_v4(), None, String::new())
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
            Triple::new("Alice".into(), V02Predicate::Uses, "Rust".into(), 0.9),
            Triple::new("Bob".into(), V02Predicate::IsA, "Engineer".into(), 0.8),
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
            subject_kind_hint: None,
            object_kind_hint: None,
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

    // ---- ISS-204 Component 1: OccurredOn event-time literal edge ----

    use crate::dimensions::TemporalMark;
    use chrono::NaiveDate;

    /// Build a context whose memory carries `mark` as its derived temporal
    /// dimension, written in the canonical `dimensions.temporal` tagged-enum
    /// layout that `derived_temporal_mark()` reads.
    fn ctx_with_temporal(content: &str, mark: &TemporalMark) -> PipelineContext {
        let mut mem = fixture_memory(content);
        let temporal = serde_json::to_value(mark).unwrap();
        mem.metadata = Some(serde_json::json!({
            "dimensions": { "temporal": temporal }
        }));
        PipelineContext::new(mem, Uuid::new_v4(), None, String::new())
    }

    fn day(y: i32, m: u32, d: u32) -> TemporalMark {
        TemporalMark::Day(NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    #[test]
    fn day_precision_iso_only_for_single_day_marks() {
        let d = NaiveDate::from_ymd_opt(2023, 7, 5).unwrap();
        assert_eq!(
            day_precision_iso(&TemporalMark::Day(d)).as_deref(),
            Some("2023-07-05")
        );
        assert_eq!(
            day_precision_iso(&TemporalMark::Exact(
                d.and_hms_opt(14, 30, 0).unwrap().and_utc()
            ))
            .as_deref(),
            Some("2023-07-05")
        );
        // Interval / uncertain marks have no single day to assert.
        assert_eq!(
            day_precision_iso(&TemporalMark::Range {
                start: d,
                end: NaiveDate::from_ymd_opt(2023, 7, 9).unwrap()
            }),
            None
        );
        assert_eq!(
            day_precision_iso(&TemporalMark::Approx {
                start: d,
                end: None,
                approximate: true,
                note: None
            }),
            None
        );
        assert_eq!(
            day_precision_iso(&TemporalMark::Vague("a while back".into())),
            None
        );
    }

    #[test]
    fn day_precision_mark_emits_occurred_on_literal_edge() {
        let extractor = StaticExtractor(vec![Triple::new(
            "Melanie".into(),
            V02Predicate::RelatedTo,
            "museum".into(),
            0.9,
        )]);
        let mut ctx = ctx_with_temporal("Melanie took her kids to the museum.", &day(2023, 7, 5));
        extract_edges(&extractor, &mut ctx).unwrap();

        // One triple-derived edge + one OccurredOn literal edge.
        assert_eq!(ctx.edge_drafts.len(), 2);
        let occ = ctx
            .edge_drafts
            .iter()
            .find(|d| {
                matches!(
                    &d.predicate,
                    Predicate::Canonical(CanonicalPredicate::OccurredOn)
                )
            })
            .expect("OccurredOn edge emitted");
        // Anchored on the primary participant (first triple subject).
        assert_eq!(occ.subject_name, "Melanie");
        // Literal ISO day object, day-precision.
        match &occ.object {
            DraftEdgeEnd::Literal(s) => assert_eq!(s, "2023-07-05"),
            other => panic!("expected literal object, got {:?}", other),
        }
        assert_eq!(occ.resolution_method, ResolutionMethod::Automatic);
    }

    #[test]
    fn no_temporal_mark_emits_no_occurred_on_edge() {
        let extractor = StaticExtractor(vec![Triple::new(
            "Alice".into(),
            V02Predicate::Uses,
            "Rust".into(),
            0.9,
        )]);
        let mut ctx = ctx_for("Alice uses Rust."); // metadata: None
        extract_edges(&extractor, &mut ctx).unwrap();
        assert_eq!(ctx.edge_drafts.len(), 1);
        assert!(!ctx.edge_drafts.iter().any(|d| matches!(
            &d.predicate,
            Predicate::Canonical(CanonicalPredicate::OccurredOn)
        )));
    }

    #[test]
    fn low_precision_mark_emits_no_occurred_on_edge() {
        // An Approx (year-granular) mark is NOT day precision — q33/q35 case
        // that needs ISS-190/191 to pin the day first. We must not fabricate
        // a literal day for it.
        let extractor = StaticExtractor(vec![Triple::new(
            "Bob".into(),
            V02Predicate::RelatedTo,
            "camping".into(),
            0.8,
        )]);
        let mark = TemporalMark::Approx {
            start: NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            end: Some(NaiveDate::from_ymd_opt(2023, 12, 31).unwrap()),
            approximate: true,
            note: Some("sometime in 2023".into()),
        };
        let mut ctx = ctx_with_temporal("Bob went camping.", &mark);
        extract_edges(&extractor, &mut ctx).unwrap();
        assert_eq!(ctx.edge_drafts.len(), 1);
        assert!(!ctx.edge_drafts.iter().any(|d| matches!(
            &d.predicate,
            Predicate::Canonical(CanonicalPredicate::OccurredOn)
        )));
    }

    #[test]
    fn day_precision_but_no_triple_emits_no_occurred_on_edge() {
        // No triple → no anchor entity to hang the date on → skip rather
        // than emit a dangling literal edge.
        let extractor = StaticExtractor(vec![]);
        let mut ctx = ctx_with_temporal("Something happened.", &day(2023, 7, 5));
        extract_edges(&extractor, &mut ctx).unwrap();
        assert!(ctx.edge_drafts.is_empty());
    }
}
