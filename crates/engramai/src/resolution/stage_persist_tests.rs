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
use crate::resolution::stage_persist::{
    build_delta, drive_persist, ApplyDelta, EdgeResolution, EntityResolution,
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
    PipelineContext::new(fixture_memory(id), Uuid::new_v4(), None)
}

fn fixture_draft(name: &str, kind: EntityKind) -> DraftEntity {
    let now = Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap();
    DraftEntity {
        canonical_name: name.into(),
        kind,
        aliases: vec![name.to_lowercase()],
        subtype_hint: None,
        first_seen: now,
        last_seen: now,
        somatic_fingerprint: None,
    }
}

fn fixture_canonical(name: &str, kind: EntityKind) -> Entity {
    let now = Utc.with_ymd_and_hms(2026, 4, 1, 8, 0, 0).unwrap();
    let mut e = Entity::new(name.into(), kind, now);
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
    assert_eq!(delta.memory_id, again, "memory_id derivation must be stable");
}

#[test]
fn build_delta_create_new_emits_one_entity_and_mention() {
    let ctx = fixture_ctx("mem-1");
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution {
        draft_index: 0,
        draft: draft.clone(),
        decision: Decision::CreateNew,
        canonical: None,
    };
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
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::MergeInto { candidate_id: canonical_id },
        canonical: Some(canonical),
    };
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.entities.len(), 1, "MergeInto upserts the canonical row");
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
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::MergeInto { candidate_id: Uuid::new_v4() },
        canonical: None, // bug: caller forgot to load it
    };
    let delta = build_delta(&ctx, &[res], &[]);
    assert!(delta.entities.is_empty());
    assert!(delta.mentions.is_empty());
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "missing_canonical");
    assert_eq!(delta.stage_failures[0].stage, "persist");
}

#[test]
fn build_delta_defer_to_llm_records_failure_no_entity_no_mention() {
    let ctx = fixture_ctx("mem-4");
    let draft = fixture_draft("Charlie", EntityKind::Person);
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::DeferToLlm { candidate_id: Uuid::new_v4() },
        canonical: None,
    };
    let delta = build_delta(&ctx, &[res], &[]);
    assert!(delta.entities.is_empty());
    assert!(delta.mentions.is_empty());
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "unresolved_defer");
}

#[test]
fn build_delta_create_new_carries_subtype_hint_into_attributes() {
    let ctx = fixture_ctx("mem-5");
    let mut draft = fixture_draft("README.md", EntityKind::Artifact);
    draft.subtype_hint = Some("file".into());
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::CreateNew,
        canonical: None,
    };
    let delta = build_delta(&ctx, &[res], &[]);
    let attrs = &delta.entities[0].attributes;
    let hint = attrs.get("subtype_hint").and_then(|v| v.as_str());
    assert_eq!(hint, Some("file"));
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
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::CreateNew,
        canonical: None,
    };
    let delta = build_delta(&ctx, &[res], &[]);
    assert_eq!(delta.mentions[0].mention_text, "Alice K.");
}

#[test]
fn build_delta_mention_text_falls_back_to_canonical_when_extracted_missing() {
    let ctx = fixture_ctx("mem-7"); // no extracted_entities
    let draft = fixture_draft("Alice", EntityKind::Person);
    let res = EntityResolution {
        draft_index: 0,
        draft,
        decision: Decision::CreateNew,
        canonical: None,
    };
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
    assert!(delta.edges[0].supersedes.is_none(), "Add must not set supersedes");
    assert!(delta.edges_to_invalidate.is_empty());
    assert!((delta.edges[0].confidence - 0.9).abs() < 1e-9);
    // Provenance plumbed.
    assert_eq!(delta.edges[0].episode_id, Some(ctx.episode_id));
    assert_eq!(delta.edges[0].memory_id.as_deref(), Some(ctx.memory.id.as_str()));
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
        decision: EdgeDecision::Update { supersedes: prior_id },
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
fn build_delta_edge_defer_to_llm_records_failure_no_edge() {
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
    assert!(delta.edges.is_empty());
    assert!(delta.edges_to_invalidate.is_empty());
    assert_eq!(delta.stage_failures.len(), 1);
    assert_eq!(delta.stage_failures[0].error_category, "unresolved_defer");
    assert!(delta.stage_failures[0].error_detail.contains("triple index 7"));
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
    let res = EntityResolution {
        draft_index: 0,
        draft: fixture_draft("Bob", EntityKind::Person),
        decision: Decision::DeferToLlm { candidate_id: Uuid::new_v4() },
        canonical: None,
    };
    let delta = build_delta(&ctx, &[res], &[]);
    // Should have both the carried Resolve failure and the persist-stage
    // unresolved_defer.
    assert_eq!(delta.stage_failures.len(), 2);
    let kinds: Vec<&str> = delta
        .stage_failures
        .iter()
        .map(|f| f.error_category.as_str())
        .collect();
    assert!(kinds.contains(&"candidate_retrieval"));
    assert!(kinds.contains(&"unresolved_defer"));
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
        EntityResolution {
            draft_index: 0,
            draft: fixture_draft("Alice", EntityKind::Person),
            decision: Decision::CreateNew,
            canonical: None,
        },
        EntityResolution {
            draft_index: 1,
            draft: fixture_draft("Bob", EntityKind::Person),
            decision: Decision::MergeInto { candidate_id: bob_id },
            canonical: Some(bob_canonical),
        },
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
            decision: EdgeDecision::Update { supersedes: prior.id },
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
    assert_eq!(delta.edges.len(), 3, "Add + Update + Proposed Add (None skipped)");
    assert_eq!(delta.edges_to_invalidate.len(), 1, "Update produces one invalidation");
    assert_eq!(delta.proposed_predicates.len(), 1);
    assert_eq!(delta.proposed_predicates[0].label, "met_at");
    assert!(delta.stage_failures.is_empty(), "no failures on a clean run");
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
fn build_delta_memory_id_passes_through_uuid_string() {
    let real_uuid = Uuid::new_v4();
    let ctx = fixture_ctx(&real_uuid.to_string());
    let delta = build_delta(&ctx, &[], &[]);
    assert_eq!(delta.memory_id, real_uuid);
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
    let er = EntityResolution {
        draft_index: 0,
        draft: fixture_draft("Alice", EntityKind::Person),
        decision: Decision::CreateNew,
        canonical: None,
    };
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
    let er = EntityResolution {
        draft_index: 0,
        draft: fixture_draft("Alice", EntityKind::Person),
        decision: Decision::CreateNew,
        canonical: None,
    };
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
    let er = EntityResolution {
        draft_index: 0,
        draft: fixture_draft("Alice", EntityKind::Person),
        decision: Decision::CreateNew,
        canonical: None,
    };
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
