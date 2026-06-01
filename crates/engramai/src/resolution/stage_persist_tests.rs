//! Tests for `stage_persist`.
//!
//! Two layers, mirroring the source split:
//!
//! - **`build_delta_*`** — pure tests against the algebra. No store, no IO.
//!   Exhaustively cover the decision matrix from §3.5 and the edge cases
//!   that the doc comments call out (DeferToLlm reaching persist, missing
//!   canonical row, proposed-predicate dedup, mention back-linking,
//!   failure-row carriage).
//! - **`drive_persist_*`** — driver tests against a one-method
//!   [`ApplyDelta`] mock. The full `apply_graph_delta` impl lands in
//!   v03-graph-layer Phase 4 (currently `unimplemented!()`); these tests
//!   verify the wire-up of error mapping and outcome construction
//!   independently of that work.

use chrono::{TimeZone, Utc};
use uuid::Uuid;

use crate::entities::{EntityType, ExtractedEntity};
use crate::graph::{
    delta::{ApplyReport, GraphDelta},
    edge::{Edge, EdgeEnd, ResolutionMethod},
    entity::{Entity, EntityKind},
    schema::{CanonicalPredicate, Predicate},
    GraphError,
};
use crate::resolution::context::{
    DraftEdge, DraftEdgeEnd, DraftEntity, PipelineContext, PipelineStage, StageFailure,
};
use crate::resolution::decision::Decision;
use crate::resolution::edge_decision::EdgeDecision;
use crate::resolution::pipeline::OnTiebreakFailure;
use crate::resolution::stage_persist::{
    build_delta, build_delta_with_policy, drive_persist, ApplyDelta, EdgeResolution,
    EntityResolution,
};
use crate::types::{MemoryLayer, MemoryRecord, MemoryType};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn fixture_memory(id: &str) -> MemoryRecord {
    MemoryRecord {
        id: id.into(),
        content: "Alice met Bob at Acme.".into(),
        memory_type: MemoryType::Episodic,
        layer: MemoryLayer::Working,
        created_at: Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap(),
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

fn fixture_ctx(id: &str) -> PipelineContext {
    PipelineContext::new(fixture_memory(id), Uuid::new_v4(), None, String::new())
}

fn fixture_draft(name: &str, kind: EntityKind) -> DraftEntity {
    let now = Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap();
    DraftEntity {
        canonical_name: name.into(),
        kind,
        aliases: vec![name.to_lowercase()],
        subtype_hint: None,
        kind_source: crate::resolution::context::KindSource::Default,
        first_seen: now,
        last_seen: now,
        somatic_fingerprint: None,
        // Default fixture stays embedding-free; tests that assert
        // embedding flow (build_new_entity wiring, alias upsert in delta)
        // construct DraftEntity inline with `embedding: Some(...)`.
        embedding: None,
    }
}

fn fixture_canonical(name: &str, kind: EntityKind) -> Entity {
    let now = Utc.with_ymd_and_hms(2026, 4, 1, 8, 0, 0).unwrap();
    let mut e = Entity::new_random_id(name.into(), kind, now);
    e.id = Uuid::new_v4();
    e.identity_confidence = 0.9;
    e
}

fn fixture_edge_draft(
    subject: &str,
    pred: Predicate,
    object: DraftEdgeEnd,
    conf: f64,
) -> DraftEdge {
    DraftEdge {
        subject_name: subject.into(),
        predicate: pred,
        object,
        source_confidence: conf,
        resolution_method: ResolutionMethod::Automatic,
    }
}

fn fixture_prior_edge(predicate: Predicate, object: EdgeEnd, confidence: f64) -> Edge {
    let now = Utc::now();
    let subject_id = Uuid::new_v4();
    Edge {
        id: Uuid::new_v4(),
        subject_id,
        predicate,
        object,
        summary: String::new(),
        valid_from: None,
        valid_to: None,
        recorded_at: now,
        invalidated_at: None,
        invalidated_by: None,
        supersedes: None,
        episode_id: None,
        memory_id: None,
        resolution_method: ResolutionMethod::Automatic,
        activation: 0.0,
        confidence,
        confidence_source: crate::graph::edge::ConfidenceSource::Recovered,
        agent_affect: None,
        created_at: now,
    }
}

// ---------------------------------------------------------------------------
// build_delta — entity decisions
// ---------------------------------------------------------------------------

#[test]
fn build_delta_empty_inputs_yields_empty_delta() {
    let ctx = fixture_ctx("mem-empty");
    let delta = build_delta(&ctx, &[], &[]);
    assert!(delta.entities.is_empty());
    assert!(delta.merges.is_empty());
    assert!(delta.edges.is_empty());
    assert!(delta.edges_to_invalidate.is_empty());
    assert!(delta.mentions.is_empty());
    assert!(delta.proposed_predicates.is_empty());
    assert!(delta.stage_failures.is_empty());
    // memory_id is derived deterministically from the string id.
    let again = build_delta(&fixture_ctx("mem-empty"), &[], &[]).memory_id;
    assert_eq!(
        delta.memory_id, again,
        "memory_id derivation must be stable"
    );
}

#[test]
fn build_delta_create_new_emits_one_entity_and_mention() {
    let ctx = fixture_ctx("mem-1");
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution::for_test(0, draft.clone(), Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.entities.len(), 1);
    assert_eq!(delta.entities[0].canonical_name, "Alice");
    assert_eq!(delta.entities[0].kind, EntityKind::Person);
    assert_eq!(delta.entities[0].identity_confidence, 1.0);
    assert_eq!(delta.mentions.len(), 1);
    assert_eq!(delta.mentions[0].entity_id, delta.entities[0].id);
    assert_eq!(delta.mentions[0].memory_id, delta.memory_id);
}

#[test]
fn build_delta_merge_into_emits_updated_canonical_and_mention() {
    let mut ctx = fixture_ctx("mem-2");
    ctx.memory.created_at = Utc.with_ymd_and_hms(2026, 4, 26, 14, 0, 0).unwrap();
    let canonical = fixture_canonical("Alice", EntityKind::Person);
    let canonical_id = canonical.id;
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution::for_test(
        0,
        draft,
        Decision::MergeInto {
            candidate_id: canonical_id,
        },
        Some(canonical),
    );
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(
        delta.entities.len(),
        1,
        "MergeInto upserts the canonical row"
    );
    assert_eq!(delta.entities[0].id, canonical_id);
    assert_eq!(delta.entities[0].canonical_name, "Alice");
    assert!(
        delta.entities[0].last_seen >= ctx.memory.created_at,
        "last_seen must advance to memory time"
    );
    assert_eq!(delta.mentions.len(), 1);
    assert_eq!(delta.mentions[0].entity_id, canonical_id);
    assert!(delta.stage_failures.is_empty());
}

#[test]
fn build_delta_merge_into_without_canonical_records_failure_skips_mention() {
    let ctx = fixture_ctx("mem-3");
    let draft = fixture_draft("Bob", EntityKind::Person);
    let res = EntityResolution::for_test(
        0,
        draft,
        Decision::MergeInto {
            candidate_id: Uuid::new_v4(),
        },
        None, // bug: caller forgot to load it
    );
    let delta = build_delta(&ctx, &[res], &[]);
    assert!(delta.entities.is_empty());
    assert!(delta.mentions.is_empty());
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "missing_canonical");
    assert_eq!(delta.stage_failures[0].stage, "persist");
}

#[test]
fn build_delta_defer_to_llm_under_default_conservative_emits_entity_and_tiebreak_audit() {
    // ISS-135: default policy = Conservative. DeferToLlm at persist mints
    // a fresh entity with identity_confidence = 0.1 and writes an
    // informational `tiebreak_fallback` audit row (NOT `unresolved_defer`).
    let ctx = fixture_ctx("mem-4");
    let draft = fixture_draft("Charlie", EntityKind::Person);
    let res = EntityResolution::for_test(
        0,
        draft,
        Decision::DeferToLlm {
            candidate_id: Uuid::new_v4(),
        },
        None,
    );
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.entities.len(), 1);
    assert_eq!(delta.mentions.len(), 1);
    assert!((delta.entities[0].identity_confidence - 0.1).abs() < 1e-9);
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "tiebreak_fallback");
    assert_eq!(delta.stage_failures[0].stage, "persist");
}

#[test]
fn build_delta_create_new_carries_subtype_hint_into_attributes() {
    let ctx = fixture_ctx("mem-5");
    let mut draft = fixture_draft("README.md", EntityKind::Artifact);
    draft.subtype_hint = Some("file".into());
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    let attrs = &delta.entities[0].attributes;
    let hint = attrs.get("subtype_hint").and_then(|v| v.as_str());
    assert_eq!(hint, Some("file"));
}

#[test]
fn build_delta_create_new_persists_kind_source_via_serde() {
    // ISS-072 GOAL-2 design test #6 — kind_source must be persisted into
    // attributes["kind_source"] using serde (PascalCase variant name), NOT
    // Debug formatting. The on-disk string is an explicit contract; the
    // GOAL-2.b merge precedence will round-trip via the same contract.
    let ctx = fixture_ctx("mem-7");
    let mut draft = fixture_draft("Caroline", EntityKind::Person);
    draft.kind_source = crate::resolution::context::KindSource::TripleHint;
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    let attrs = &delta.entities[0].attributes;
    let raw = attrs
        .get("kind_source")
        .expect("kind_source must be persisted on every new entity");
    // Must be a JSON string (NOT a number, object, or quoted Debug output)
    assert_eq!(raw.as_str(), Some("TripleHint"));
    // Round-trip via serde must give back the original variant.
    let round_trip: crate::resolution::context::KindSource =
        serde_json::from_value(raw.clone()).expect("round-trip parse");
    assert_eq!(
        round_trip,
        crate::resolution::context::KindSource::TripleHint
    );
}

#[test]
fn build_delta_create_new_persists_kind_source_default_when_no_hint() {
    // Even when the source is Default (no hint), the field must be present —
    // otherwise the GOAL-2.b merge logic can't distinguish "no signal" from
    // "missing field, must backfill".
    let ctx = fixture_ctx("mem-8");
    let mut draft = fixture_draft("Unknown thing", EntityKind::Other("unknown".into()));
    draft.kind_source = crate::resolution::context::KindSource::Default;
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    let attrs = &delta.entities[0].attributes;
    assert_eq!(
        attrs.get("kind_source").and_then(|v| v.as_str()),
        Some("Default")
    );
}

#[test]
fn build_delta_mention_text_uses_extracted_entity_name_when_available() {
    let mut ctx = fixture_ctx("mem-6");
    ctx.extracted_entities.push(ExtractedEntity {
        name: "Alice K.".into(),
        normalized: "alice k".into(),
        entity_type: EntityType::Person,
    });
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.mentions[0].mention_text, "Alice K.");
}

#[test]
fn build_delta_mention_text_falls_back_to_canonical_when_extracted_missing() {
    let ctx = fixture_ctx("mem-7"); // no extracted_entities
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.mentions[0].mention_text, "Alice");
}

// ---------------------------------------------------------------------------
// build_delta — edge decisions
// ---------------------------------------------------------------------------

#[test]
fn build_delta_edge_add_emits_new_edge_only() {
    let ctx = fixture_ctx("mem-edge-1");
    let subj = Uuid::new_v4();
    let obj_id = Uuid::new_v4();
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Acme".into()),
            0.9,
        ),
        decision: EdgeDecision::Add,
        subject_id: subj,
        object: EdgeEnd::Entity { id: obj_id },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert_eq!(delta.edges.len(), 1);
    assert_eq!(delta.edges[0].subject_id, subj);
    assert!(matches!(delta.edges[0].object, EdgeEnd::Entity { id } if id == obj_id));
    assert!(
        delta.edges[0].supersedes.is_none(),
        "Add must not set supersedes"
    );
    assert!(delta.edges_to_invalidate.is_empty());
    assert!((delta.edges[0].confidence - 0.9).abs() < 1e-9);
    // Provenance plumbed.
    assert_eq!(delta.edges[0].episode_id, Some(ctx.episode_id));
    assert_eq!(
        delta.edges[0].memory_id.as_deref(),
        Some(ctx.memory.id.as_str())
    );
}

#[test]
fn build_delta_edge_update_emits_new_edge_and_invalidation_chain() {
    let ctx = fixture_ctx("mem-edge-2");
    let subj = Uuid::new_v4();
    let acme = Uuid::new_v4();
    let beta = Uuid::new_v4();
    let prior = fixture_prior_edge(
        Predicate::Canonical(CanonicalPredicate::WorksAt),
        EdgeEnd::Entity { id: acme },
        0.85,
    );
    let prior_id = prior.id;
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Beta".into()),
            0.9,
        ),
        decision: EdgeDecision::Update {
            supersedes: prior_id,
        },
        subject_id: subj,
        object: EdgeEnd::Entity { id: beta },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert_eq!(delta.edges.len(), 1);
    assert_eq!(delta.edges[0].supersedes, Some(prior_id));
    assert_eq!(delta.edges_to_invalidate.len(), 1);
    assert_eq!(delta.edges_to_invalidate[0].edge_id, prior_id);
    assert_eq!(
        delta.edges_to_invalidate[0].superseded_by,
        Some(delta.edges[0].id),
        "successor.id must wire back into invalidation directive"
    );
}

#[test]
fn build_delta_edge_none_emits_nothing() {
    let ctx = fixture_ctx("mem-edge-3");
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Acme".into()),
            0.9,
        ),
        decision: EdgeDecision::None,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert!(delta.edges.is_empty());
    assert!(delta.edges_to_invalidate.is_empty());
    assert!(delta.stage_failures.is_empty());
    assert!(delta.proposed_predicates.is_empty());
}

#[test]
fn build_delta_edge_defer_to_llm_under_default_conservative_emits_edge_and_tiebreak_audit() {
    // ISS-135: default = Conservative. EdgeDecision::DeferToLlm now emits
    // the edge with confidence = 0.1 + ConfidenceSource::Defaulted, plus
    // an informational `tiebreak_fallback` audit row.
    let ctx = fixture_ctx("mem-edge-4");
    let er = EdgeResolution {
        draft_index: 7,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Acme".into()),
            0.9,
        ),
        decision: EdgeDecision::DeferToLlm,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert_eq!(delta.edges.len(), 1);
    assert!((delta.edges[0].confidence - 0.1).abs() < 1e-9);
    assert!(delta.edges_to_invalidate.is_empty());
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "tiebreak_fallback");
    assert!(delta.stage_failures[0]
        .error_detail
        .contains("triple index 7"));
}

#[test]
fn build_delta_clamps_oob_confidence_into_range() {
    let ctx = fixture_ctx("mem-edge-5");
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Acme".into()),
            1.7, // bogus
        ),
        decision: EdgeDecision::Add,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert_eq!(delta.edges[0].confidence, 1.0);
}

#[test]
fn build_delta_literal_object_passes_through() {
    let ctx = fixture_ctx("mem-edge-6");
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::Literal("Acme Corp".into()),
            0.8,
        ),
        decision: EdgeDecision::Add,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Literal {
            value: serde_json::Value::String("Acme Corp".into()),
        },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    let value = match &delta.edges[0].object {
        EdgeEnd::Literal { value } => value.as_str(),
        _ => None,
    };
    assert_eq!(value, Some("Acme Corp"));
}

// ---------------------------------------------------------------------------
// build_delta — proposed predicates
// ---------------------------------------------------------------------------

#[test]
fn build_delta_proposed_predicate_recorded_once_per_label() {
    let ctx = fixture_ctx("mem-prop-1");
    let pred = Predicate::proposed("collaborates_with");
    let er1 = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            pred.clone(),
            DraftEdgeEnd::EntityName("Bob".into()),
            0.8,
        ),
        decision: EdgeDecision::Add,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let er2 = EdgeResolution {
        draft_index: 1,
        draft: fixture_edge_draft(
            "Alice",
            pred.clone(),
            DraftEdgeEnd::EntityName("Carol".into()),
            0.8,
        ),
        decision: EdgeDecision::Add,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er1, er2]);
    assert_eq!(delta.proposed_predicates.len(), 1);
    assert_eq!(delta.proposed_predicates[0].label, "collaborates_with");
}

#[test]
fn build_delta_canonical_predicates_do_not_register() {
    let ctx = fixture_ctx("mem-prop-2");
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Acme".into()),
            0.9,
        ),
        decision: EdgeDecision::Add,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert!(delta.proposed_predicates.is_empty());
}

#[test]
fn build_delta_proposed_predicate_skipped_when_decision_is_none() {
    // Skipped triples must not pollute the predicate registry.
    let ctx = fixture_ctx("mem-prop-3");
    let pred = Predicate::proposed("mentioned_with");
    let er = EdgeResolution {
        draft_index: 0,
        draft: fixture_edge_draft(
            "Alice",
            pred.clone(),
            DraftEdgeEnd::EntityName("Bob".into()),
            0.8,
        ),
        decision: EdgeDecision::None,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta(&ctx, &[], &[er]);
    assert!(delta.proposed_predicates.is_empty());
}

// ---------------------------------------------------------------------------
// build_delta — stage failure carriage
// ---------------------------------------------------------------------------

#[test]
fn build_delta_carries_ctx_failures_into_delta() {
    let mut ctx = fixture_ctx("mem-fail-1");
    ctx.failures.push(StageFailure::new(
        PipelineStage::EdgeExtract,
        "llm_5xx",
        "anthropic 502",
    ));
    ctx.failures.push(StageFailure::new(
        PipelineStage::EntityExtract,
        "regex_panic",
        "bad pattern",
    ));
    let delta = build_delta(&ctx, &[], &[]);
    assert_eq!(delta.stage_failures.len(), 2);
    let stages: Vec<_> = delta
        .stage_failures
        .iter()
        .map(|f| f.stage.as_str())
        .collect();
    assert!(stages.contains(&"edge_extract"));
    assert!(stages.contains(&"entity_extract"));
}

#[test]
fn build_delta_combines_ctx_failures_and_locally_generated() {
    let mut ctx = fixture_ctx("mem-fail-2");
    ctx.failures.push(StageFailure::new(
        PipelineStage::Resolve,
        "candidate_retrieval",
        "store unavailable",
    ));
    let res = EntityResolution::for_test(
        0,
        fixture_draft("Bob", EntityKind::Person),
        Decision::DeferToLlm {
            candidate_id: Uuid::new_v4(),
        },
        None,
    );
    let delta = build_delta(&ctx, &[res], &[]);
    // Should have both the carried Resolve failure and the persist-stage
    // tiebreak_fallback (ISS-135 default = Conservative).
    assert_eq!(delta.stage_failures.len(), 2);
    let kinds: Vec<&str> = delta
        .stage_failures
        .iter()
        .map(|f| f.error_category.as_str())
        .collect();
    assert!(kinds.contains(&"candidate_retrieval"));
    assert!(kinds.contains(&"tiebreak_fallback"));
}

// ---------------------------------------------------------------------------
// build_delta — combined integration of all surfaces
// ---------------------------------------------------------------------------

#[test]
fn build_delta_full_run_combines_entities_edges_mentions_predicates() {
    // One CreateNew entity, one MergeInto, one Add edge, one Update edge,
    // one None edge, one proposed-predicate Add. Verifies all surfaces
    // populate together without cross-contamination.
    let mut ctx = fixture_ctx("mem-combo");
    ctx.extracted_entities.push(ExtractedEntity {
        name: "Alice".into(),
        normalized: "alice".into(),
        entity_type: EntityType::Person,
    });
    ctx.extracted_entities.push(ExtractedEntity {
        name: "Bob".into(),
        normalized: "bob".into(),
        entity_type: EntityType::Person,
    });

    let bob_canonical = fixture_canonical("Bob", EntityKind::Person);
    let bob_id = bob_canonical.id;
    let entity_decisions = vec![
        EntityResolution::for_test(
            0,
            fixture_draft("Alice", EntityKind::Person),
            Decision::CreateNew,
            None,
        ),
        EntityResolution::for_test(
            1,
            fixture_draft("Bob", EntityKind::Person),
            Decision::MergeInto {
                candidate_id: bob_id,
            },
            Some(bob_canonical),
        ),
    ];

    let prior = fixture_prior_edge(
        Predicate::Canonical(CanonicalPredicate::WorksAt),
        EdgeEnd::Entity { id: Uuid::new_v4() },
        0.85,
    );
    let edge_decisions = vec![
        // Add: Alice WorksAt Acme
        EdgeResolution {
            draft_index: 0,
            draft: fixture_edge_draft(
                "Alice",
                Predicate::Canonical(CanonicalPredicate::WorksAt),
                DraftEdgeEnd::EntityName("Acme".into()),
                0.9,
            ),
            decision: EdgeDecision::Add,
            subject_id: Uuid::new_v4(),
            object: EdgeEnd::Entity { id: Uuid::new_v4() },
        },
        // Update: Bob WorksAt Beta (supersedes prior)
        EdgeResolution {
            draft_index: 1,
            draft: fixture_edge_draft(
                "Bob",
                Predicate::Canonical(CanonicalPredicate::WorksAt),
                DraftEdgeEnd::EntityName("Beta".into()),
                0.92,
            ),
            decision: EdgeDecision::Update {
                supersedes: prior.id,
            },
            subject_id: bob_id,
            object: EdgeEnd::Entity { id: Uuid::new_v4() },
        },
        // None: skipped silently
        EdgeResolution {
            draft_index: 2,
            draft: fixture_edge_draft(
                "Alice",
                Predicate::Canonical(CanonicalPredicate::IsA),
                DraftEdgeEnd::EntityName("Programmer".into()),
                0.6,
            ),
            decision: EdgeDecision::None,
            subject_id: Uuid::new_v4(),
            object: EdgeEnd::Entity { id: Uuid::new_v4() },
        },
        // Proposed Add
        EdgeResolution {
            draft_index: 3,
            draft: fixture_edge_draft(
                "Alice",
                Predicate::proposed("met_at"),
                DraftEdgeEnd::EntityName("Acme".into()),
                0.7,
            ),
            decision: EdgeDecision::Add,
            subject_id: Uuid::new_v4(),
            object: EdgeEnd::Entity { id: Uuid::new_v4() },
        },
    ];

    let delta = build_delta(&ctx, &entity_decisions, &edge_decisions);

    assert_eq!(delta.entities.len(), 2, "1 CreateNew + 1 MergeInto upsert");
    assert_eq!(delta.mentions.len(), 2, "one mention per resolved entity");
    assert_eq!(
        delta.edges.len(),
        3,
        "Add + Update + Proposed Add (None skipped)"
    );
    assert_eq!(
        delta.edges_to_invalidate.len(),
        1,
        "Update produces one invalidation"
    );
    assert_eq!(delta.proposed_predicates.len(), 1);
    assert_eq!(delta.proposed_predicates[0].label, "met_at");
    assert!(
        delta.stage_failures.is_empty(),
        "no failures on a clean run"
    );
}

#[test]
fn build_delta_memory_id_is_deterministic_for_string_id() {
    let a = build_delta(&fixture_ctx("memX"), &[], &[]).memory_id;
    let b = build_delta(&fixture_ctx("memX"), &[], &[]).memory_id;
    assert_eq!(a, b);
    let c = build_delta(&fixture_ctx("memY"), &[], &[]).memory_id;
    assert_ne!(a, c);
}

#[test]
fn build_delta_memory_id_passes_through_verbatim() {
    // After revert to §5bis (memory_id: String), the delta carries the
    // memory id verbatim — no parsing, no hashing, no derivation. This
    // test guards against any future helper sneaking a transformation
    // back in.
    let real_uuid = Uuid::new_v4().to_string();
    let ctx = fixture_ctx(&real_uuid);
    let delta = build_delta(&ctx, &[], &[]);
    assert_eq!(delta.memory_id, real_uuid);

    // Non-UUID-shaped ids must also pass through unchanged (v0.2 schema
    // allows free-form strings like `mem-42` or `episode_2024_01_15`).
    let weird_id = "mem-42";
    let ctx2 = fixture_ctx(weird_id);
    let delta2 = build_delta(&ctx2, &[], &[]);
    assert_eq!(delta2.memory_id, weird_id);
}

// ---------------------------------------------------------------------------
// drive_persist — driver layer with mock applier
// ---------------------------------------------------------------------------

/// Minimal `ApplyDelta` test double: records every delta passed in and
/// returns a configurable result.
struct MockApplier {
    deltas: Vec<GraphDelta>,
    result: Result<ApplyReport, GraphError>,
}

impl MockApplier {
    fn ok() -> Self {
        Self {
            deltas: Vec::new(),
            result: Ok(ApplyReport::new()),
        }
    }

    fn err(msg: &'static str) -> Self {
        Self {
            deltas: Vec::new(),
            result: Err(GraphError::Invariant(msg)),
        }
    }
}

impl ApplyDelta for MockApplier {
    fn apply_graph_delta(&mut self, delta: &GraphDelta) -> Result<ApplyReport, GraphError> {
        self.deltas.push(delta.clone());
        // Clone the result: GraphError doesn't impl Clone universally,
        // so handle the Err arm by constructing a fresh value.
        match &self.result {
            Ok(r) => Ok(r.clone()),
            Err(GraphError::Invariant(m)) => Err(GraphError::Invariant(*m)),
            Err(_) => Err(GraphError::Invariant("mock error")),
        }
    }
}

#[test]
fn drive_persist_ok_returns_outcome_with_delta_and_report() {
    let mut store = MockApplier::ok();
    let mut ctx = fixture_ctx("mem-drive-1");
    let er = EntityResolution::for_test(
        0,
        fixture_draft("Alice", EntityKind::Person),
        Decision::CreateNew,
        None,
    );
    let outcome = drive_persist(&mut store, &mut ctx, &[er], &[]).expect("ok");
    assert_eq!(outcome.delta.entities.len(), 1);
    assert_eq!(store.deltas.len(), 1);
    assert_eq!(store.deltas[0].entities.len(), 1);
    assert!(!ctx.has_failures(), "ok path must not record a failure");
}

#[test]
fn drive_persist_records_failure_on_apply_error() {
    let mut store = MockApplier::err("boom");
    let mut ctx = fixture_ctx("mem-drive-2");
    let er = EntityResolution::for_test(
        0,
        fixture_draft("Alice", EntityKind::Person),
        Decision::CreateNew,
        None,
    );
    let result = drive_persist(&mut store, &mut ctx, &[er], &[]);
    assert!(result.is_err());
    assert!(ctx.has_failures());
    let persist_failures: Vec<_> = ctx.failures_for(PipelineStage::Persist).collect();
    assert_eq!(persist_failures.len(), 1);
    assert_eq!(persist_failures[0].kind, "apply_graph_delta_error");
    assert!(persist_failures[0].message.contains("boom"));
}

#[test]
fn drive_persist_passes_built_delta_verbatim_to_store() {
    // Sanity: whatever build_delta produced is exactly what the store
    // receives. Guards against accidental mutation in the driver.
    let mut store = MockApplier::ok();
    let mut ctx = fixture_ctx("mem-drive-3");
    let er = EntityResolution::for_test(
        0,
        fixture_draft("Alice", EntityKind::Person),
        Decision::CreateNew,
        None,
    );
    let outcome = drive_persist(&mut store, &mut ctx, &[er], &[]).expect("ok");
    assert_eq!(store.deltas[0].memory_id, outcome.delta.memory_id);
    assert_eq!(store.deltas[0].entities.len(), outcome.delta.entities.len());
    assert_eq!(store.deltas[0].mentions.len(), outcome.delta.mentions.len());
}

#[test]
fn drive_persist_empty_inputs_still_calls_apply() {
    // Even an empty pipeline (no extracted entities, no triples) must
    // produce a call to apply_graph_delta — the empty delta marks the
    // pipeline run as successful and registers idempotence.
    let mut store = MockApplier::ok();
    let mut ctx = fixture_ctx("mem-drive-4");
    let outcome = drive_persist(&mut store, &mut ctx, &[], &[]).expect("ok");
    assert_eq!(store.deltas.len(), 1);
    assert!(outcome.delta.entities.is_empty());
    assert!(outcome.delta.edges.is_empty());
}

// ─── ISS-075 regression tests ─────────────────────────────────────────
//
// The pipeline must emit alias rows whenever an entity is touched and
// must propagate the embedding from the draft into the persisted entity.
// Without these, `search_candidates` returns empty and every mention
// takes the `CreateNew` shortcut, producing duplicate-entity bugs like
// the 27× Caroline duplication observed in cogmembench (ISS-075).

#[test]
fn build_delta_create_new_emits_alias_upsert_for_each_surface_form() {
    let ctx = fixture_ctx("mem-iss075-create");
    // Draft with two surface forms: the normalized canonical and a
    // separate normalized variant. `draft_entity_from_mention` only ever
    // seeds one alias per draft today, but the algebra must handle Vec
    // length ≥ 1 generically — covered by this fixture.
    let mut draft = fixture_draft("Caroline", EntityKind::Person);
    draft.aliases = vec!["caroline".into(), "carol".into()];
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);

    let delta = build_delta(&ctx, &[res], &[]);

    assert_eq!(delta.aliases.len(), 2, "one alias upsert per surface form");
    let canonical_id = delta.entities[0].id;
    for alias in &delta.aliases {
        assert_eq!(
            alias.canonical_id, canonical_id,
            "alias must point at the same id stage_extract minted (ISS-076 single-mint)"
        );
        assert_eq!(
            alias.alias_raw, "Caroline",
            "raw form is the canonical_name"
        );
        assert_eq!(
            alias.source_episode,
            Some(ctx.episode_id),
            "source_episode tracks where the alias was first observed"
        );
    }
    let normalized: Vec<&str> = delta
        .aliases
        .iter()
        .map(|a| a.normalized.as_str())
        .collect();
    assert!(normalized.contains(&"caroline"));
    assert!(normalized.contains(&"carol"));
}

#[test]
fn build_delta_merge_into_emits_alias_upsert_to_accrete_new_surface_form() {
    // MergeInto path: the canonical entity already exists, but this
    // mention may have arrived under a surface form not yet aliased.
    // `upsert_alias` is idempotent on the normalized PK, so re-emitting
    // an existing alias is harmless; emitting a new one accretes the
    // alias set so future mentions of that form will hit on the merge
    // path instead of taking the CreateNew shortcut.
    let ctx = fixture_ctx("mem-iss075-merge");
    let canonical = fixture_canonical("Caroline", EntityKind::Person);
    let canonical_id = canonical.id;
    let mut draft = fixture_draft("Caroline", EntityKind::Person);
    // Imagine a later episode mentioning her as "Carol" — a new alias
    // that should be persisted against the same canonical id.
    draft.canonical_name = "Carol".into();
    draft.aliases = vec!["carol".into()];
    let res = EntityResolution::for_test(
        0,
        draft,
        Decision::MergeInto {
            candidate_id: canonical_id,
        },
        Some(canonical),
    );

    let delta = build_delta(&ctx, &[res], &[]);

    assert_eq!(delta.aliases.len(), 1, "one new surface form → one upsert");
    assert_eq!(delta.aliases[0].canonical_id, canonical_id);
    assert_eq!(delta.aliases[0].normalized, "carol");
    assert_eq!(delta.aliases[0].alias_raw, "Carol");
    assert_eq!(delta.aliases[0].source_episode, Some(ctx.episode_id));
}

#[test]
fn build_delta_create_new_with_empty_alias_list_emits_no_alias_rows() {
    // Defensive: if a future code path produces a draft with no aliases,
    // we must not insert a degenerate alias row (would violate the
    // graph_entity_aliases NOT NULL constraint on `normalized`).
    let ctx = fixture_ctx("mem-iss075-empty");
    let mut draft = fixture_draft("Alice", EntityKind::Person);
    draft.aliases = vec![];
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);

    let delta = build_delta(&ctx, &[res], &[]);

    assert_eq!(delta.entities.len(), 1, "entity row still emitted");
    assert!(delta.aliases.is_empty(), "no alias seeds → no alias rows");
}

#[test]
fn build_delta_create_new_propagates_embedding_into_entity_row() {
    // ISS-075 root fix: the embedding computed in stage_extract must
    // land on the persisted Entity. Without this, every entity is
    // written embedding-less and the s4 fusion signal is permanently
    // dark, even if alias rows are present.
    let ctx = fixture_ctx("mem-iss075-emb");
    let mut draft = fixture_draft("Alice", EntityKind::Person);
    let emb: Vec<f32> = (0..384).map(|i| (i as f32) * 0.001).collect();
    draft.embedding = Some(emb.clone());
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);

    let delta = build_delta(&ctx, &[res], &[]);

    assert_eq!(delta.entities.len(), 1);
    assert_eq!(
        delta.entities[0].embedding.as_ref(),
        Some(&emb),
        "draft.embedding must be copied into the persisted entity (else s4 dark)"
    );
}

#[test]
fn build_delta_create_new_without_embedding_yields_none_on_entity() {
    // Negative case: if extract failed to produce an embedding (e.g.
    // embedder transient error), the pipeline is non-fatal — the entity
    // still persists, just with `embedding: None`.
    let ctx = fixture_ctx("mem-iss075-emb-none");
    let draft = fixture_draft("Alice", EntityKind::Person);
    assert!(draft.embedding.is_none(), "fixture starts embedding-free");
    let res = EntityResolution::for_test(0, draft, Decision::CreateNew, None);

    let delta = build_delta(&ctx, &[res], &[]);

    assert_eq!(delta.entities.len(), 1);
    assert!(delta.entities[0].embedding.is_none());
}

// ---------------------------------------------------------------------------
// ISS-135 — OnTiebreakFailure policy (entity + edge × Conservative + Abort)
// ---------------------------------------------------------------------------

#[test]
fn build_delta_with_policy_entity_conservative_mints_fresh_id_and_emits_audit() {
    // Conservative: DeferToLlm becomes a CreateNew with identity_confidence
    // = 0.1, fresh Uuid (NOT candidate_id so ISS-076 single-mint holds),
    // and a `tiebreak_fallback` audit row.
    let ctx = fixture_ctx("mem-iss135-1");
    let draft = fixture_draft("Dora", EntityKind::Person);
    let candidate_id = Uuid::new_v4();
    let res = EntityResolution::for_test(0, draft, Decision::DeferToLlm { candidate_id }, None);
    let assigned_id = res.assigned_id;
    let delta = build_delta_with_policy(&ctx, &[res], &[], OnTiebreakFailure::Conservative);

    assert_eq!(delta.entities.len(), 1);
    let entity = &delta.entities[0];
    assert_eq!(entity.id, assigned_id);
    assert_ne!(entity.id, candidate_id, "must not reuse candidate_id");
    assert!((entity.identity_confidence - 0.1).abs() < 1e-9);

    assert_eq!(delta.mentions.len(), 1);
    assert_eq!(delta.stage_failures.len(), 1);
    let f = &delta.stage_failures[0];
    assert_eq!(f.error_category, "tiebreak_fallback");
    assert_eq!(f.stage, "persist");
    assert!(f.error_detail.contains(&assigned_id.to_string()));
}

#[test]
fn build_delta_with_policy_entity_abort_skips_entity_emits_unresolved_defer() {
    // Abort: legacy behaviour. No entity, no mention; one
    // `unresolved_defer` audit row so the record halts.
    let ctx = fixture_ctx("mem-iss135-2");
    let draft = fixture_draft("Erin", EntityKind::Person);
    let res = EntityResolution::for_test(
        0,
        draft,
        Decision::DeferToLlm {
            candidate_id: Uuid::new_v4(),
        },
        None,
    );
    let delta = build_delta_with_policy(&ctx, &[res], &[], OnTiebreakFailure::Abort);

    assert!(delta.entities.is_empty(), "Abort must not emit entity");
    assert!(delta.mentions.is_empty(), "Abort must not emit mention");
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "unresolved_defer");
}

#[test]
fn build_delta_with_policy_edge_conservative_emits_edge_low_conf_and_audit() {
    // Conservative: EdgeDecision::DeferToLlm now emits the edge with
    // confidence = 0.1 + ConfidenceSource::Defaulted and a
    // `tiebreak_fallback` audit row.
    let ctx = fixture_ctx("mem-iss135-3");
    let er = EdgeResolution {
        draft_index: 4,
        draft: fixture_edge_draft(
            "Fay",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Globex".into()),
            0.95,
        ),
        decision: EdgeDecision::DeferToLlm,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta_with_policy(&ctx, &[], &[er], OnTiebreakFailure::Conservative);

    assert_eq!(delta.edges.len(), 1);
    let edge = &delta.edges[0];
    assert!((edge.confidence - 0.1).abs() < 1e-9);
    assert_eq!(delta.stage_failures.len(), 1);
    let f = &delta.stage_failures[0];
    assert_eq!(f.error_category, "tiebreak_fallback");
    assert!(f.error_detail.contains("triple index 4"));
}

#[test]
fn build_delta_with_policy_edge_abort_skips_edge_emits_unresolved_defer() {
    // Abort: legacy behaviour for edges. No edge; one `unresolved_defer`
    // row so the record halts.
    let ctx = fixture_ctx("mem-iss135-4");
    let er = EdgeResolution {
        draft_index: 11,
        draft: fixture_edge_draft(
            "Gail",
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            DraftEdgeEnd::EntityName("Initech".into()),
            0.8,
        ),
        decision: EdgeDecision::DeferToLlm,
        subject_id: Uuid::new_v4(),
        object: EdgeEnd::Entity { id: Uuid::new_v4() },
    };
    let delta = build_delta_with_policy(&ctx, &[], &[er], OnTiebreakFailure::Abort);

    assert!(delta.edges.is_empty(), "Abort must not emit edge");
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "unresolved_defer");
    assert!(delta.stage_failures[0]
        .error_detail
        .contains("triple index 11"));
}
