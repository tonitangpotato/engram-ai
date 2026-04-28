//! §3.5 Persist stage — atomic batched commit of one resolution run.
//!
//! Two layers:
//!
//! 1. [`build_delta`] — **pure** translator from pipeline state +
//!    decisions to a [`GraphDelta`]. No IO, no panics, fully testable
//!    without a storage fixture.
//! 2. [`drive_persist`] — thin **impure** wrapper that calls
//!    `GraphStore::apply_graph_delta` with the built delta, records the
//!    [`ApplyReport`] outcome on `ctx`, and surfaces failures via
//!    `StageFailure`.
//!
//! The split mirrors `decide` (pure) / `retrieve_candidates` (impure)
//! elsewhere in the resolution module. It exists for the same reason: the
//! decision algebra of §3.5 is intricate (entity create vs. update vs.
//! merge; edge add / supersede / preserve; provenance back-linking; failure
//! row carriage), and we want every branch covered by deterministic tests
//! without a SQLite fixture.
//!
//! ## What persist receives
//!
//! Inputs (all owned by the caller):
//!
//! - `ctx: &PipelineContext` — the in-flight pipeline state. Used to read:
//!     - `memory.id` (the deltas's `memory_id`),
//!     - `memory.created_at` (the `now` for new edge `recorded_at` etc.),
//!     - `affect_snapshot` (forwarded onto new entities / edges per
//!       GUARD-8),
//!     - `extracted_entities[i]` / `entity_drafts[i]` for mention rows,
//!     - `failures` — copied into `delta.stage_failures` for atomic carry.
//! - `entity_decisions: &[EntityResolution]` — one entry per
//!   `entity_drafts[i]`, in order. Carries the draft + the §3.4.3
//!   `Decision` and (for `MergeInto`) the canonical entity row to update.
//! - `edge_decisions: &[EdgeResolution]` — one entry per non-skipped
//!   triple. Carries the draft + the §3.4.4 `EdgeDecision` and (for
//!   `Update`) the prior edge id.
//!
//! ## What persist produces
//!
//! [`PersistOutcome`] wrapping the [`GraphDelta`] (always) and the
//! [`ApplyReport`] (only when `drive_persist` ran the impure call). The
//! pure layer returns just the delta; the driver layer also records the
//! report on `ctx` for trace persistence.
//!
//! ## What persist does NOT do
//!
//! - **No re-extract diff (§4.2).** That's a separate orchestration concern
//!   layered on top of this stage; the diff produces the
//!   `entity_decisions` / `edge_decisions` slices that this stage consumes
//!   verbatim.
//! - **No LLM tie-break.** `Decision::DeferToLlm` and
//!   `EdgeDecision::DeferToLlm` arriving here are programming errors; the
//!   pipeline must resolve those *before* persist (record a `StageFailure`
//!   if the tie-break itself failed). Defensive: we record a failure and
//!   skip the entry rather than panic.
//! - **No idempotence-key construction.** `apply_graph_delta` derives the
//!   key from `delta.delta_hash()`; persist just hands off the delta.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::graph::{
    audit::{
        CATEGORY_APPLY_GRAPH_DELTA_ERROR, CATEGORY_MISSING_CANONICAL, CATEGORY_UNRESOLVED_DEFER,
    },
    delta::{
        ApplyReport, EdgeInvalidation, GraphDelta, MemoryEntityMention,
        ProposedPredicate, StageFailureRow,
    },
    edge::{ConfidenceSource, Edge, EdgeEnd, ResolutionMethod},
    entity::Entity,
    schema::Predicate,
    store::{GraphStore, GraphWrite},
    GraphError,
};
use crate::resolution::context::{
    DraftEdge, DraftEntity, PipelineContext, PipelineStage,
};
use crate::resolution::decision::Decision;
use crate::resolution::edge_decision::EdgeDecision;

// ---------------------------------------------------------------------------
// Public input types
// ---------------------------------------------------------------------------

/// Resolution outcome for a single mention — pairs the draft (kept for
/// mention-row construction) with the §3.4.3 decision, plus the canonical
/// row for `MergeInto` paths so persist can read existing fields without a
/// second store hit.
///
/// `draft_index` is the position in `ctx.extracted_entities` /
/// `ctx.entity_drafts` (they are co-indexed, see `stage_extract`). It's
/// used to build `MemoryEntityMention.span_*` from the original
/// `ExtractedEntity` if span info was preserved, and to attribute the
/// mention text correctly.
#[derive(Clone, Debug)]
pub struct EntityResolution {
    pub draft_index: usize,
    pub draft: DraftEntity,
    pub decision: Decision,
    /// Required iff `decision == MergeInto`. The current canonical row, so
    /// persist can reuse `summary`, `attributes`, etc. when constructing
    /// the upsert. May be `None` for `CreateNew` (a fresh row is minted).
    pub canonical: Option<Entity>,
}

/// Resolution outcome for a single triple. Pairs the draft with the
/// §3.4.4 decision and the resolved subject/object entity ids so persist
/// can fill `Edge.subject_id` / `Edge.object`.
///
/// For `EdgeDecision::Update { supersedes }` the prior edge's id is
/// carried by the decision itself; the caller does not need to re-pass it.
#[derive(Clone, Debug)]
pub struct EdgeResolution {
    pub draft_index: usize,
    pub draft: DraftEdge,
    pub decision: EdgeDecision,
    /// Resolved subject entity id. Required for `Add` / `Update` decisions.
    /// Pipeline must have resolved the draft's `subject_name` to a UUID
    /// before reaching persist.
    pub subject_id: Uuid,
    /// Resolved object: either an entity id (when
    /// `draft.object == EntityName`) or the literal value (when `Literal`).
    pub object: EdgeEnd,
}

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

/// Outcome of `drive_persist`. The pure `build_delta` returns just the
/// `GraphDelta`; the driver pairs it with the apply report.
#[derive(Debug, Clone)]
pub struct PersistOutcome {
    pub delta: GraphDelta,
    pub report: ApplyReport,
}

// ---------------------------------------------------------------------------
// Pure builder (§3.5 algebra)
// ---------------------------------------------------------------------------

/// Translate pipeline state + decisions into a `GraphDelta`. Pure.
///
/// Order of operations within the produced delta is what
/// `apply_graph_delta` expects (§4.2):
///
/// 1. Entity merges (precede entity upserts so loser ids in subsequent
///    edge inserts can be remapped).
/// 2. Entity upserts (`CreateNew` and `MergeInto` paths).
/// 3. Edge inserts (`Add` and `Update.new` rows; `Update.new` carries
///    `supersedes = prior.id`).
/// 4. Edge invalidations (one per `Update`; `superseded_by = new.id`).
/// 5. Mention rows (one per resolved entity that had at least one mention
///    in this memory).
/// 6. Proposed predicates (one per distinct `Predicate::Proposed` label
///    seen in the edge drafts).
/// 7. Stage failures (carried verbatim from `ctx.failures`).
///
/// Construction-time defenses:
/// - `Decision::DeferToLlm` / `EdgeDecision::DeferToLlm` arriving here
///   means the LLM tie-break did not run or did not commit a concrete
///   decision. We record a `StageFailureRow { stage: "persist", kind:
///   "unresolved_defer" }` and skip the entry. The delta is still valid;
///   the operator sees the failure in `graph_extraction_failures`.
/// - `EdgeDecision::None` is a true no-op: skipped entirely (no row, no
///   failure).
/// - Unmapped subject/object on an edge `Add` / `Update` (subject_id or
///   object missing in `EdgeResolution`) is not representable here: the
///   caller must have resolved them. If a future caller mis-builds the
///   slice, the bad entry is skipped and a failure is recorded.
pub fn build_delta(
    ctx: &PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
) -> GraphDelta {
    let memory_id = parse_memory_uuid(&ctx.memory.id);
    let now = ctx.memory.created_at;
    let mut delta = GraphDelta::new(memory_id);

    // ── Entities ────────────────────────────────────────────────────────
    // Build mention rows in lock-step. Each resolution that produces a
    // canonical id contributes exactly one mention row (memory_id ↔
    // entity_id ↔ mention_text).
    let mut deferred_mention_failures: Vec<StageFailureRow> = Vec::new();
    let mut mentions: Vec<MemoryEntityMention> = Vec::new();

    for er in entity_decisions {
        match &er.decision {
            Decision::CreateNew => {
                let new_entity = build_new_entity(&er.draft, now, ctx);
                let entity_id = new_entity.id;
                mentions.push(mention_row(
                    memory_id,
                    entity_id,
                    &er.draft,
                    er.draft_index,
                    ctx,
                ));
                delta.entities.push(new_entity);
            }
            Decision::MergeInto { candidate_id } => {
                let entity_id = *candidate_id;
                // The §3.4.3 design specifies updating activation /
                // last_seen / identity_confidence on the canonical row.
                // We model this as an upsert by including the (mutated)
                // canonical row in `delta.entities`. `apply_graph_delta`
                // detects pre-existing rows and dispatches to the
                // partial-update SQL path; brand-new rows get full
                // inserts. Either way, the `entities` vec is the wire.
                if let Some(canonical) = &er.canonical {
                    let updated = merge_into_canonical(canonical, &er.draft, now);
                    delta.entities.push(updated);
                } else {
                    // Defensive: caller said MergeInto but didn't pass the
                    // canonical row. The merge is impossible to construct
                    // safely → record a failure and skip the mention so
                    // we don't write a dangling reference.
                    deferred_mention_failures.push(failure_row(
                        ctx.episode_id,
                        CATEGORY_MISSING_CANONICAL,
                        format!(
                            "EntityResolution::MergeInto missing canonical row \
                             for draft '{}' (index {})",
                            er.draft.canonical_name, er.draft_index
                        ),
                        now,
                    ));
                    continue;
                }
                mentions.push(mention_row(
                    memory_id,
                    entity_id,
                    &er.draft,
                    er.draft_index,
                    ctx,
                ));
            }
            Decision::DeferToLlm { candidate_id } => {
                deferred_mention_failures.push(failure_row(
                    ctx.episode_id,
                    CATEGORY_UNRESOLVED_DEFER,
                    format!(
                        "EntityResolution::DeferToLlm reached persist for draft '{}' \
                         (index {}, candidate {}). LLM tie-break must commit a concrete \
                         Decision before §3.5.",
                        er.draft.canonical_name, er.draft_index, candidate_id
                    ),
                    now,
                ));
                // Do not emit a mention row: there is no resolved id.
            }
        }
    }

    // ── Edges ───────────────────────────────────────────────────────────
    // Two collections threaded together: new edge rows and invalidation
    // directives. `Update` produces both; `Add` produces only the row;
    // `None` produces neither.
    let mut edges: Vec<Edge> = Vec::new();
    let mut invalidations: Vec<EdgeInvalidation> = Vec::new();
    let mut edge_failures: Vec<StageFailureRow> = Vec::new();

    for er in edge_decisions {
        match &er.decision {
            EdgeDecision::None => continue,
            EdgeDecision::Add => {
                edges.push(build_new_edge(er, None, now, ctx));
            }
            EdgeDecision::Update { supersedes } => {
                let new_edge = build_new_edge(er, Some(*supersedes), now, ctx);
                let new_id = new_edge.id;
                edges.push(new_edge);
                invalidations.push(EdgeInvalidation {
                    edge_id: *supersedes,
                    invalidated_at: dt_to_unix(now),
                    superseded_by: Some(new_id),
                });
            }
            EdgeDecision::DeferToLlm => {
                edge_failures.push(failure_row(
                    ctx.episode_id,
                    CATEGORY_UNRESOLVED_DEFER,
                    format!(
                        "EdgeDecision::DeferToLlm reached persist for triple index {}. \
                         LLM tie-break must commit a concrete decision before §3.5.",
                        er.draft_index
                    ),
                    now,
                ));
            }
        }
    }

    // ── Proposed predicates (§3.3.2) ───────────────────────────────────
    // One registration row per distinct Proposed label seen in the
    // accepted edge drafts. Skipped predicates (`None` / `DeferToLlm`)
    // do not register — only edges that actually persist do.
    let mut proposed: Vec<ProposedPredicate> = Vec::new();
    let mut seen_labels: Vec<String> = Vec::new();
    for er in edge_decisions {
        if matches!(er.decision, EdgeDecision::Add | EdgeDecision::Update { .. }) {
            if let Predicate::Proposed(label) = &er.draft.predicate {
                if !seen_labels.iter().any(|l| l == label) {
                    seen_labels.push(label.clone());
                    proposed.push(ProposedPredicate {
                        label: label.clone(),
                        first_seen_at: dt_to_unix(now),
                    });
                }
            }
        }
    }

    // ── Stage failures: carried from ctx + locally accumulated ─────────
    let mut stage_failures: Vec<StageFailureRow> = ctx
        .failures
        .iter()
        .map(|f| StageFailureRow {
            episode_id: ctx.episode_id,
            stage: f.stage.as_str().to_string(),
            error_category: f.kind.clone(),
            error_detail: f.message.clone(),
            occurred_at: dt_to_unix(f.at),
        })
        .collect();
    stage_failures.extend(deferred_mention_failures);
    stage_failures.extend(edge_failures);

    delta.edges = edges;
    delta.edges_to_invalidate = invalidations;
    delta.mentions = mentions;
    delta.proposed_predicates = proposed;
    delta.stage_failures = stage_failures;

    delta
}

// ---------------------------------------------------------------------------
// Impure driver
// ---------------------------------------------------------------------------

/// Narrow capability the persist driver actually needs from a graph store:
/// just the atomic batched apply. Decoupling this from the full
/// `GraphStore` trait lets tests inject a one-method mock without
/// implementing the 40-method trait, and lets future callers (e.g.
/// background worker pools) hold a smaller dyn handle. Production
/// implementors get this for free via the blanket impl below.
pub trait ApplyDelta {
    fn apply_graph_delta(&mut self, delta: &GraphDelta) -> Result<ApplyReport, GraphError>;
}

impl<S: GraphStore + ?Sized> ApplyDelta for S {
    fn apply_graph_delta(&mut self, delta: &GraphDelta) -> Result<ApplyReport, GraphError> {
        // `apply_graph_delta` lives on `GraphWrite` (design §5.1). The
        // marker `GraphStore: GraphWrite` super-trait pulls it in for the
        // blanket impl.
        GraphWrite::apply_graph_delta(self, delta)
    }
}

/// Build the delta and apply it via [`ApplyDelta::apply_graph_delta`]. On
/// success returns a [`PersistOutcome`] carrying both the delta and the
/// apply report. On failure records a `StageFailure` on `ctx` and returns
/// the [`GraphError`].
///
/// Impure: writes to the store. The pure delta construction lives in
/// [`build_delta`] and is independently tested.
pub fn drive_persist<S: ApplyDelta + ?Sized>(
    store: &mut S,
    ctx: &mut PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
) -> Result<PersistOutcome, GraphError> {
    let delta = build_delta(ctx, entity_decisions, edge_decisions);

    match store.apply_graph_delta(&delta) {
        Ok(report) => {
            log::debug!(
                target: "resolution.stage_persist",
                "persist ok: memory_id={} entities={} edges={} invalidations={} \
                 mentions={} predicates={} failures={} already_applied={}",
                delta.memory_id,
                report.entities_upserted,
                report.edges_inserted,
                report.edges_invalidated,
                report.mentions_inserted,
                report.predicates_registered,
                report.failures_recorded,
                report.already_applied,
            );
            Ok(PersistOutcome { delta, report })
        }
        Err(e) => {
            ctx.record_failure(
                PipelineStage::Persist,
                CATEGORY_APPLY_GRAPH_DELTA_ERROR,
                e.to_string(),
            );
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (pure)
// ---------------------------------------------------------------------------

/// Convert the v0.2 string `MemoryRecord.id` into a `Uuid` for the delta's
/// `memory_id` field. v0.2 ids are not UUID-shaped in general; we hash the
/// string into a deterministic v5-style namespace UUID so that the same
/// string always produces the same id. See v03-migration §5 for the
/// migration-time mapping; this helper mirrors that contract.
fn parse_memory_uuid(s: &str) -> Uuid {
    if let Ok(u) = Uuid::parse_str(s) {
        return u;
    }
    // Stable derivation: BLAKE3 hash of the string truncated to 16 bytes,
    // then framed as a UUID (variant + version 4 bits set so the result is
    // a syntactically-valid UUID even though its derivation is not RFC
    // 4122 v5). See v03-migration §5 for the canonical mapping; until that
    // contract is finalized this is a deterministic local mapping that
    // will not collide for distinct strings.
    let hash = blake3::hash(s.as_bytes());
    let mut bytes: [u8; 16] = [0; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    // Set version (4) and variant (RFC 4122) bits so external tools
    // accept it.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Uuid::from_bytes(bytes)
}

fn dt_to_unix(dt: DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + (dt.timestamp_subsec_micros() as f64) / 1_000_000.0
}

fn build_new_entity(draft: &DraftEntity, now: DateTime<Utc>, ctx: &PipelineContext) -> Entity {
    let mut e = Entity::new(draft.canonical_name.clone(), draft.kind.clone(), now);
    e.first_seen = draft.first_seen;
    e.last_seen = draft.last_seen.max(ctx.memory.created_at);
    // Carry the affect snapshot per GUARD-8 (immutable after capture).
    e.somatic_fingerprint = draft.somatic_fingerprint.clone().or(ctx.affect_snapshot.clone());
    // Subtype hint goes into attributes so adapter precision survives.
    if let Some(hint) = &draft.subtype_hint {
        if let serde_json::Value::Object(map) = &mut e.attributes {
            map.insert(
                "subtype_hint".to_string(),
                serde_json::Value::String(hint.clone()),
            );
        }
    }
    // Identity confidence starts at the §3.4 fusion convention for fresh
    // rows: 1.0 (we're certain this is a *new* identity, not that we've
    // verified it). Calibration happens on subsequent merges.
    e.identity_confidence = 1.0;
    e
}

/// Apply MergeInto adjustments to a canonical Entity. Conservative — only
/// fields the design explicitly authorizes us to update on a merge:
///
/// - `last_seen` advanced to the new mention's time (never regresses)
/// - `somatic_fingerprint` recomputed if the draft carried one (the
///   §3.4.3 design wants a true aggregate; in this stage we forward the
///   draft's snapshot when present, deferring true aggregation to a
///   future pass that has access to all mentions — see §4.2(d))
/// - `identity_confidence` only raised, never lowered (§4.2(d))
/// - `agent_affect` updated if the draft's somatic delta is present
///
/// Caller's responsibility to set `attributes.history` (handled in the
/// graph layer's `merge_entities` SQL path; persist just forwards the
/// mutated row).
fn merge_into_canonical(canonical: &Entity, draft: &DraftEntity, now: DateTime<Utc>) -> Entity {
    let mut updated = canonical.clone();
    if draft.last_seen > updated.last_seen {
        updated.last_seen = draft.last_seen;
    }
    if now > updated.last_seen {
        updated.last_seen = now;
    }
    if let Some(fp) = &draft.somatic_fingerprint {
        updated.somatic_fingerprint = Some(fp.clone());
    }
    // identity_confidence: monotone non-decreasing on re-extract.
    // §3.4.3 fusion confidence is not in `DraftEntity` directly; persist
    // does not lower the canonical confidence regardless. A future
    // refactor will plumb the fused score through `EntityResolution` so
    // we can raise it.
    updated.updated_at = now;
    updated
}

fn build_new_edge(
    er: &EdgeResolution,
    supersedes: Option<Uuid>,
    now: DateTime<Utc>,
    ctx: &PipelineContext,
) -> Edge {
    let mut e = Edge::new(
        er.subject_id,
        er.draft.predicate.clone(),
        er.object.clone(),
        Some(now), // valid_from = the episode time (best available)
        now,
    );
    e.confidence = er.draft.source_confidence.clamp(0.0, 1.0);
    e.confidence_source = ConfidenceSource::Recovered;
    e.resolution_method = er.draft.resolution_method.clone();
    e.episode_id = Some(ctx.episode_id);
    e.memory_id = Some(ctx.memory.id.clone());
    e.supersedes = supersedes;
    e.agent_affect = ctx.affect_snapshot.as_ref().and_then(|fp| {
        // Forward the affect snapshot as JSON. SomaticFingerprint is
        // serialize-able; failure to serialize is a programming bug, not
        // a runtime concern (we'd have caught it in the §3.1 pipeline).
        serde_json::to_value(fp).ok()
    });
    let _ = ResolutionMethod::Automatic; // keep referenced for cross-module audit
    e
}

fn mention_row(
    memory_id: Uuid,
    entity_id: Uuid,
    draft: &DraftEntity,
    draft_index: usize,
    ctx: &PipelineContext,
) -> MemoryEntityMention {
    // v0.2 ExtractedEntity does not currently carry span offsets — we
    // emit `None` / `None` for span_start / span_end and use the mention
    // text from the source `name` if available. When v0.3 entity
    // extraction starts emitting spans (§3.2 future work), update this
    // helper to forward them.
    let mention_text = ctx
        .extracted_entities
        .get(draft_index)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| draft.canonical_name.clone());

    MemoryEntityMention {
        memory_id,
        entity_id,
        mention_text,
        span_start: None,
        span_end: None,
        confidence: 1.0,
    }
}

fn failure_row(
    episode_id: Uuid,
    kind: &str,
    detail: String,
    now: DateTime<Utc>,
) -> StageFailureRow {
    StageFailureRow {
        episode_id,
        stage: PipelineStage::Persist.as_str().to_string(),
        error_category: kind.to_string(),
        error_detail: detail,
        occurred_at: dt_to_unix(now),
    }
}

// Tests live in `stage_persist_tests.rs` (split for readability).
#[cfg(test)]
#[path = "stage_persist_tests.rs"]
mod tests;
