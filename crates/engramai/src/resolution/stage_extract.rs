//! §3.2 Entity-extraction stage driver.
//!
//! Wraps the existing v0.2 [`EntityExtractor`] (Aho-Corasick + regex over
//! `content`) and lifts each `ExtractedEntity` into a `DraftEntity` ready
//! for resolution (§3.4.3).
//!
//! Pure function — no IO beyond the extractor's own pattern scan; not
//! generic over storage. The only side effect is mutating
//! [`PipelineContext`] in place.
//!
//! Boundary rules (mirrors `mod.rs` boundaries):
//! - Never panics.
//! - Never returns early without recording the failure on `ctx`.
//! - Adapter mapping is total — every v0.2 `EntityType` lands in some
//!   v0.3 `EntityKind` (see [`crate::resolution::adapters::map_entity_kind`]).

use crate::entities::EntityExtractor;
use crate::resolution::adapters::draft_entity_from_mention;
use crate::resolution::context::{PipelineContext, PipelineStage};

/// Stage-execution error. Detail is recorded on `ctx.failures` before
/// returning; this type is just a control-flow marker so the driver can
/// decide whether to short-circuit. Use `ctx.failures_for(...)` to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageError;

/// Run §3.2: pattern-scan `ctx.memory.content`, push the resulting
/// `ExtractedEntity` mentions into `ctx.extracted_entities`, and lift each
/// one into a `DraftEntity` placed (in order) into `ctx.entity_drafts`.
///
/// Returns `Ok(())` on success. The v0.2 extractor never errors (it returns
/// an empty vec on no matches), so `Err` is reserved for future LLM-backed
/// extractors. When that happens, the failure is recorded on `ctx.failures`
/// and `Err(StageError)` is returned so the driver can decide whether to
/// short-circuit or continue with empty results.
pub fn extract_entities(
    extractor: &EntityExtractor,
    ctx: &mut PipelineContext,
) -> Result<(), StageError> {
    let _span_memory_id = &ctx.memory.id;

    let mentions = extractor.extract(&ctx.memory.content);

    // Lift each mention into a draft. Order is preserved (drafts[i] ↔
    // mentions[i]) — downstream signal scoring relies on this alignment.
    let occurred_at = ctx.memory.created_at;
    let affect = ctx.affect_snapshot;

    ctx.entity_drafts.clear();
    ctx.entity_drafts.reserve(mentions.len());
    for m in &mentions {
        ctx.entity_drafts
            .push(draft_entity_from_mention(m, occurred_at, affect));
    }

    ctx.extracted_entities = mentions;

    log::debug!(
        target: "resolution.stage_extract",
        "entity extraction complete: memory_id={} mentions={}",
        _span_memory_id,
        ctx.extracted_entities.len()
    );

    let _ = PipelineStage::EntityExtract; // keep enum referenced for stable cross-module audit.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{EntityConfig, EntityExtractor};
    use crate::graph::EntityKind;
    use crate::resolution::context::PipelineContext;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;
    use uuid::Uuid;

    fn fixture_memory(content: &str) -> MemoryRecord {
        MemoryRecord {
            id: "stage-extract-test".into(),
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

    fn extractor_with_people(names: &[&str]) -> EntityExtractor {
        let mut cfg = EntityConfig::default();
        cfg.known_people = names.iter().map(|s| s.to_string()).collect();
        EntityExtractor::new(&cfg)
    }

    #[test]
    fn extracts_no_drafts_on_empty_content() {
        let extractor = EntityExtractor::new(&EntityConfig::default());
        let mut ctx = ctx_for("");
        extract_entities(&extractor, &mut ctx).unwrap();
        assert!(ctx.extracted_entities.is_empty());
        assert!(ctx.entity_drafts.is_empty());
    }

    #[test]
    fn drafts_one_per_mention_in_order() {
        let extractor = extractor_with_people(&["Alice", "Bob"]);
        let mut ctx = ctx_for("Alice met Bob at the office.");
        extract_entities(&extractor, &mut ctx).unwrap();
        assert_eq!(ctx.extracted_entities.len(), ctx.entity_drafts.len());
        assert!(
            ctx.entity_drafts.len() >= 2,
            "expected ≥2 drafts, got {}",
            ctx.entity_drafts.len()
        );
        for (m, d) in ctx.extracted_entities.iter().zip(ctx.entity_drafts.iter()) {
            assert_eq!(d.canonical_name, m.name);
        }
    }

    #[test]
    fn draft_kind_matches_v02_to_v03_adapter() {
        let extractor = extractor_with_people(&["Charlie"]);
        let mut ctx = ctx_for("Charlie shipped the patch.");
        extract_entities(&extractor, &mut ctx).unwrap();
        let draft = ctx
            .entity_drafts
            .iter()
            .find(|d| d.canonical_name == "Charlie")
            .expect("Charlie draft");
        assert_eq!(draft.kind, EntityKind::Person);
    }

    #[test]
    fn rerun_overwrites_drafts_does_not_accumulate() {
        let extractor = extractor_with_people(&["Alice"]);
        let mut ctx = ctx_for("Alice.");
        extract_entities(&extractor, &mut ctx).unwrap();
        let n1 = ctx.entity_drafts.len();
        extract_entities(&extractor, &mut ctx).unwrap();
        let n2 = ctx.entity_drafts.len();
        assert_eq!(n1, n2, "second run should not double up drafts");
    }

    #[test]
    fn draft_first_seen_equals_memory_created_at() {
        let extractor = extractor_with_people(&["Dana"]);
        let mut ctx = ctx_for("Dana wrote the spec.");
        let memory_t = ctx.memory.created_at;
        extract_entities(&extractor, &mut ctx).unwrap();
        let draft = ctx
            .entity_drafts
            .iter()
            .find(|d| d.canonical_name == "Dana")
            .expect("Dana draft");
        assert_eq!(draft.first_seen, memory_t);
        assert_eq!(draft.last_seen, memory_t);
    }
}
