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
//! - **No LLM tie-break.** A real LLM tie-breaker is not implemented at
//!   the persist stage; design §3.4 places it between resolve and
//!   persist. What `build_delta` *does* offer is the
//!   [`crate::resolution::OnTiebreakFailure`] policy (design §8.1, ISS-135):
//!   if `Decision::DeferToLlm` or `EdgeDecision::DeferToLlm` arrives
//!   here, `Conservative` (default) materializes the draft as a
//!   `CreateNew` / `Add` with `confidence = low` and emits a
//!   `tiebreak_fallback` audit row (GUARD-2 visible trace);
//!   `Abort` emits an `unresolved_defer` row and skips the entry.
//!   Backfill (§6.5) never calls an LLM, so Conservative is what
//!   migration relies on.
//! - **No idempotence-key construction.** `apply_graph_delta` derives the
//!   key from `delta.delta_hash()`; persist just hands off the delta.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::graph::{
    audit::{
        CATEGORY_APPLY_GRAPH_DELTA_ERROR, CATEGORY_MISSING_CANONICAL, CATEGORY_TIEBREAK_FALLBACK,
        CATEGORY_UNRESOLVED_DEFER,
    },
    delta::{
        ApplyReport, EdgeInvalidation, GraphDelta, MemoryEntityMention, ProposedPredicate,
        StageFailureRow,
    },
    edge::{ConfidenceSource, Edge, EdgeEnd, ResolutionMethod},
    entity::Entity,
    schema::Predicate,
    store::{GraphStore, GraphWrite},
    GraphError,
};
use crate::resolution::context::{DraftEdge, DraftEntity, PipelineContext, PipelineStage};
use crate::resolution::decision::Decision;
use crate::resolution::edge_decision::EdgeDecision;
use crate::resolution::pipeline::OnTiebreakFailure;

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
    /// The single canonical entity id this resolution maps the mention to
    /// **and** which downstream stages (`resolve_edges`, `build_delta`,
    /// mention rows) must consume. ISS-076 root cause: previously each
    /// stage minted its own UUID for `CreateNew`, so edge endpoints
    /// pointed at non-existent rows. With `assigned_id` set once at
    /// resolution time, all downstream consumers reuse the same id.
    ///
    /// - `CreateNew` → a **deterministic** id derived from the
    ///   case-folded canonical key `lowercase(name)|kind|namespace`
    ///   (ISS-209 — was `Uuid::new_v4()`, which fragmented `Caroline` vs
    ///   `caroline` into two nodes). See
    ///   `ResolutionPipeline::deterministic_entity_id`.
    /// - `MergeInto { candidate_id }` → equal to `candidate_id`.
    /// - `DeferToLlm { candidate_id }` → depends on
    ///   `PipelineConfig::on_tiebreak_failure` (ISS-135 / design §8.1):
    ///   under `Conservative` (default) it is the same deterministic
    ///   case-folded id (Conservative fallback materializes the draft
    ///   as a low-confidence `CreateNew`); under `Abort` it equals
    ///   `candidate_id` (placeholder — persist records
    ///   `unresolved_defer` and skips the entry).
    pub assigned_id: Uuid,
}

impl EntityResolution {
    /// Test helper: construct an `EntityResolution` with `assigned_id`
    /// derived from `decision` the same way `resolve_entities` does.
    /// Saves test code from re-deriving the id and from forgetting to
    /// set the field (which would compile-fail at every fixture site
    /// after ISS-076).
    #[doc(hidden)]
    pub fn for_test(
        draft_index: usize,
        draft: DraftEntity,
        decision: Decision,
        canonical: Option<Entity>,
    ) -> Self {
        // Mirrors the production `assigned_id` policy in
        // `ResolutionPipeline::resolve_entities` under the default
        // `OnTiebreakFailure::Conservative` (ISS-135): `DeferToLlm`
        // gets a fresh id (Conservative fallback mints a new entity),
        // `MergeInto` reuses `candidate_id`. Tests exercising the
        // `Abort` branch construct `EntityResolution` directly with
        // `assigned_id = candidate_id`.
        let assigned_id = match &decision {
            Decision::CreateNew | Decision::DeferToLlm { .. } => Uuid::new_v4(),
            Decision::MergeInto { candidate_id } => *candidate_id,
        };
        Self {
            draft_index,
            draft,
            decision,
            canonical,
            assigned_id,
        }
    }
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
///   triggers the policy chosen by the `tiebreak_policy` parameter
///   (ISS-135 / design §8.1):
///   - [`OnTiebreakFailure::Conservative`] (default for [`build_delta`])
///     materializes the draft as a `CreateNew` / `Add` with
///     `identity_confidence = low` and emits one `StageFailureRow {
///     stage: "persist", category: "tiebreak_fallback" }` so the trace
///     is visible (GUARD-2). The entry **is** written.
///   - [`OnTiebreakFailure::Abort`] emits a `StageFailureRow { stage:
///     "persist", category: "unresolved_defer" }` and skips the entry.
///     The delta is still valid; the operator sees the failure in
///     `graph_extraction_failures`.
/// - `EdgeDecision::None` is a true no-op: skipped entirely (no row, no
///   failure).
/// - Unmapped subject/object on an edge `Add` / `Update` (subject_id or
///   object missing in `EdgeResolution`) is not representable here: the
///   caller must have resolved them. If a future caller mis-builds the
///   slice, the bad entry is skipped and a failure is recorded.
///
/// [`OnTiebreakFailure::Conservative`]: crate::resolution::OnTiebreakFailure::Conservative
/// [`OnTiebreakFailure::Abort`]: crate::resolution::OnTiebreakFailure::Abort
pub fn build_delta(
    ctx: &PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
) -> GraphDelta {
    // Default policy = Conservative (design §8.1). Tests and production
    // callers that need `Abort` semantics go through `build_delta_with_policy`.
    build_delta_with_policy(
        ctx,
        entity_decisions,
        edge_decisions,
        OnTiebreakFailure::default(),
    )
}

/// As [`build_delta`] but takes an explicit `tiebreak_policy`. Used by
/// `drive_persist` / `resolve_for_backfill` to thread
/// `PipelineConfig::on_tiebreak_failure`. Tests can also call this
/// directly to exercise `Abort` behaviour.
pub fn build_delta_with_policy(
    ctx: &PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
    tiebreak_policy: OnTiebreakFailure,
) -> GraphDelta {
    // Memory ids are free-form strings (v0.2 schema); the delta stores them
    // verbatim. The physical schema (`memories.id`,
    // `graph_memory_entity_mentions.memory_id`) is also TEXT, so no
    // conversion is needed at any boundary.
    let memory_id: &str = &ctx.memory.id;
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
                // ISS-076: use the id minted in resolve_entities, NOT a
                // fresh one. resolve_edges already published this id to
                // edge subject/object slots; minting again here would
                // produce dangling endpoints.
                let new_entity = build_new_entity(er.assigned_id, &er.draft, now, ctx);
                let entity_id = new_entity.id;
                debug_assert_eq!(
                    entity_id, er.assigned_id,
                    "ISS-076 invariant: entity row id must equal EntityResolution.assigned_id"
                );
                mentions.push(mention_row(
                    memory_id,
                    entity_id,
                    &er.draft,
                    er.draft_index,
                    ctx,
                ));
                delta.entities.push(new_entity);
                // ISS-075 root fix: emit one alias row per surface form
                // so future mentions of this entity (under any form
                // already seeded here) hit `search_candidates` and take
                // the `MergeInto` path instead of duplicating.
                delta.aliases.extend(alias_upserts_for_draft(
                    &er.draft,
                    entity_id,
                    ctx.episode_id,
                ));
            }
            Decision::MergeInto { candidate_id } => {
                let entity_id = *candidate_id;
                debug_assert_eq!(entity_id, er.assigned_id, "ISS-076 invariant: MergeInto candidate_id must equal EntityResolution.assigned_id");
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
                // ISS-075: also emit alias rows on the merge path. The
                // canonical entity already exists, but this mention may
                // have arrived under a new surface form (e.g. "Caroline"
                // first, then "caroline" / "Carol" later). `upsert_alias`
                // is idempotent on the normalized form, so re-emitting an
                // existing alias is a no-op; emitting a new one accretes
                // the alias set without changing the canonical row.
                delta.aliases.extend(alias_upserts_for_draft(
                    &er.draft,
                    entity_id,
                    ctx.episode_id,
                ));
            }
            Decision::DeferToLlm { candidate_id } => match tiebreak_policy {
                OnTiebreakFailure::Conservative => {
                    // ISS-135 / design §8.1: Conservative fallback.
                    // Materialize the draft as a low-confidence
                    // `CreateNew` (NOT a merge — the candidate may be
                    // the wrong entity, that's *why* the resolver
                    // deferred). `er.assigned_id` was minted fresh in
                    // `resolve_entities` under this policy so edge
                    // endpoints lifted via `name_to_id` already point
                    // here (ISS-076 invariant preserved).
                    let mut new_entity = build_new_entity(er.assigned_id, &er.draft, now, ctx);
                    // Stamp the confidence floor. design §1061:
                    // "method = Automatic, confidence = low". 0.1 is
                    // the smallest value the resolver may produce;
                    // retrieval reads `identity_confidence` to weight
                    // disambiguation candidates.
                    new_entity.identity_confidence = 0.1;
                    let entity_id = new_entity.id;
                    debug_assert_eq!(
                        entity_id, er.assigned_id,
                        "ISS-076 invariant: Conservative-fallback entity row id must equal EntityResolution.assigned_id",
                    );
                    mentions.push(mention_row(
                        memory_id,
                        entity_id,
                        &er.draft,
                        er.draft_index,
                        ctx,
                    ));
                    delta.entities.push(new_entity);
                    delta.aliases.extend(alias_upserts_for_draft(
                        &er.draft,
                        entity_id,
                        ctx.episode_id,
                    ));
                    // GUARD-2: visible trace. The fallback succeeded,
                    // but the operator must be able to find it later.
                    deferred_mention_failures.push(failure_row(
                        ctx.episode_id,
                        CATEGORY_TIEBREAK_FALLBACK,
                        format!(
                            "EntityResolution::DeferToLlm fell back to CreateNew (Conservative) \
                             for draft '{}' (index {}, candidate {}). \
                             New entity id = {}; identity_confidence = 0.1. \
                             Agent may merge via agent_curate_entity (§6.3).",
                            er.draft.canonical_name, er.draft_index, candidate_id, entity_id,
                        ),
                        now,
                    ));
                }
                OnTiebreakFailure::Abort => {
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
            },
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
            EdgeDecision::DeferToLlm => match tiebreak_policy {
                OnTiebreakFailure::Conservative => {
                    // ISS-135 / design §8.1: mirror the entity arm. We
                    // materialize the edge as a low-confidence `Add`
                    // (no `supersedes`) — the resolver was uncertain,
                    // but the draft itself is valid (extractor +
                    // subject/object resolution succeeded). The agent
                    // can revisit via curation.
                    let mut new_edge = build_new_edge(er, None, now, ctx);
                    new_edge.confidence = 0.1;
                    // Mark provenance: this confidence was *defaulted*
                    // by the policy, not recovered from real signals.
                    new_edge.confidence_source = ConfidenceSource::Defaulted;
                    let new_edge_id = new_edge.id;
                    edges.push(new_edge);
                    edge_failures.push(failure_row(
                        ctx.episode_id,
                        CATEGORY_TIEBREAK_FALLBACK,
                        format!(
                            "EdgeDecision::DeferToLlm fell back to Add (Conservative) for triple \
                             index {} (subject {} → object {:?}). \
                             New edge id = {}; confidence = 0.1 (Defaulted).",
                            er.draft_index, er.subject_id, er.object, new_edge_id,
                        ),
                        now,
                    ));
                }
                OnTiebreakFailure::Abort => {
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
            },
        }
    }

    // ── Proposed predicates (§3.3.2) ───────────────────────────────────
    // One registration row per distinct Proposed label seen in the
    // accepted edge drafts. Skipped predicates (`None`, or
    // `DeferToLlm` under `OnTiebreakFailure::Abort`) do not register —
    // only edges that actually persist do. Under `Conservative`,
    // `DeferToLlm` *does* persist (low-confidence Add), so its
    // predicate must also register.
    let mut proposed: Vec<ProposedPredicate> = Vec::new();
    let mut seen_labels: Vec<String> = Vec::new();
    for er in edge_decisions {
        let emits_edge = match &er.decision {
            EdgeDecision::Add | EdgeDecision::Update { .. } => true,
            EdgeDecision::DeferToLlm => {
                matches!(tiebreak_policy, OnTiebreakFailure::Conservative)
            }
            EdgeDecision::None => false,
        };
        if emits_edge {
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
///
/// Uses the default tie-break policy (`Conservative`). To override (e.g.
/// for `Abort` tests), use [`drive_persist_with_policy`].
pub fn drive_persist<S: ApplyDelta + ?Sized>(
    store: &mut S,
    ctx: &mut PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
) -> Result<PersistOutcome, GraphError> {
    drive_persist_with_policy(
        store,
        ctx,
        entity_decisions,
        edge_decisions,
        OnTiebreakFailure::default(),
    )
}

/// As [`drive_persist`] but takes an explicit `tiebreak_policy`. Used by
/// `ResolutionPipeline::run_job` to thread
/// `PipelineConfig::on_tiebreak_failure`.
pub fn drive_persist_with_policy<S: ApplyDelta + ?Sized>(
    store: &mut S,
    ctx: &mut PipelineContext,
    entity_decisions: &[EntityResolution],
    edge_decisions: &[EdgeResolution],
    tiebreak_policy: OnTiebreakFailure,
) -> Result<PersistOutcome, GraphError> {
    let delta = build_delta_with_policy(ctx, entity_decisions, edge_decisions, tiebreak_policy);

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
/// Internal helpers (pure)
// ---------------------------------------------------------------------------

fn dt_to_unix(dt: DateTime<Utc>) -> f64 {
    dt.timestamp() as f64 + (dt.timestamp_subsec_micros() as f64) / 1_000_000.0
}

/// Helper: build the alias-upsert rows for a single resolution outcome.
///
/// Emits one [`AliasUpsert`] per surface form recorded on the draft
/// (`draft.aliases` — already normalized at extract time, see
/// `draft_entity_from_mention`). The raw display form is the draft's
/// `canonical_name`; the same raw is used for every alias of this draft
/// because the extractor only sees one mention text per draft (multi-form
/// accretion happens by *re-mentioning* under a different surface form on
/// a later episode, which produces a fresh draft → fresh `AliasUpsert`).
///
/// `source_episode` is the L1 episode that introduced the alias — useful
/// for back-tracing where a surface form was first observed.
///
/// Returns an empty Vec when `draft.aliases` is empty (defensive: the
/// adapter always seeds at least one entry, but we don't want to insert
/// a degenerate row if a future code path produces an aliasless draft).
///
/// ISS-075 root fix: previously `build_delta` did not emit any alias
/// rows, so `graph_entity_aliases` stayed empty, `search_candidates`
/// returned no matches, and every mention took the `CreateNew`
/// shortcut (27× duplicate Carolines in cogmembench).
fn alias_upserts_for_draft(
    draft: &DraftEntity,
    canonical_id: Uuid,
    episode_id: Uuid,
) -> Vec<crate::graph::delta::AliasUpsert> {
    use crate::graph::delta::AliasUpsert;
    draft
        .aliases
        .iter()
        .filter(|a| !a.is_empty())
        .map(|normalized| AliasUpsert {
            normalized: normalized.clone(),
            alias_raw: draft.canonical_name.clone(),
            canonical_id,
            source_episode: Some(episode_id),
        })
        .collect()
}

fn build_new_entity(
    id: Uuid,
    draft: &DraftEntity,
    now: DateTime<Utc>,
    ctx: &PipelineContext,
) -> Entity {
    let mut e = Entity::new(id, draft.canonical_name.clone(), draft.kind.clone(), now);
    e.first_seen = draft.first_seen;
    e.last_seen = draft.last_seen.max(ctx.memory.created_at);
    // Carry the affect snapshot per GUARD-8 (immutable after capture).
    e.somatic_fingerprint = draft
        .somatic_fingerprint
        .clone()
        .or(ctx.affect_snapshot.clone());
    // Subtype hint goes into attributes so adapter precision survives.
    if let Some(hint) = &draft.subtype_hint {
        if let serde_json::Value::Object(map) = &mut e.attributes {
            map.insert(
                "subtype_hint".to_string(),
                serde_json::Value::String(hint.clone()),
            );
        }
    }
    // ISS-072 GOAL-2 (A-clean): persist `kind_source` provenance.
    //
    // CRITICAL: this is the ONLY place §1–§7 of ISS-072 writes provenance.
    // The merge path (`merge_into_canonical`) does NOT update `kind` or
    // `attributes["kind_source"]` on existing canonical rows — see design §8
    // for why this is benign today (dictionary path empty in production +
    // triple path always loses to itself: TripleHint == TripleHint, first
    // writer wins) and what the GOAL-2.b PR must add.
    //
    // Serialization uses `#[derive(Serialize)]` + `#[serde(rename_all =
    // "PascalCase")]` on `KindSource` (see context.rs) — NOT `format!("{:?}",
    // ...)`. Rust does not promise stable `Debug` output across compiler
    // versions or refactors; persisting the variant via serde makes the
    // on-disk string an explicit contract.
    if let serde_json::Value::Object(map) = &mut e.attributes {
        map.insert(
            "kind_source".to_string(),
            serde_json::to_value(draft.kind_source).expect("KindSource serialize is infallible"),
        );
    }
    // ISS-075 root fix: copy the draft's name embedding (computed in
    // stage_extract via the injected `Embedder`) into the persisted entity
    // row. Without this, every entity was written with `embedding: None`
    // and `search_candidates` had no embedding-similarity signal to fuse
    // → zero retrieval candidates → every mention forced into `CreateNew`.
    //
    // The dim invariant (system-wide single dim, see entity.rs
    // `validate_embedding_dim`) is enforced at write time by the storage
    // layer; if extract produced a bad-dim vector, persist still records
    // the entity with no embedding rather than corrupting the column.
    e.embedding = draft.embedding.clone();
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
        // valid_from = when the fact became true *in the real world*. We do
        // NOT know that from the write-clock — stamping `now` here (the
        // ingest time) conflates bitemporal fact-validity with event time
        // and corrupts as-of-T queries (ISS-204). The ingest time is already
        // captured in `recorded_at` (the `now` positional below). Event-
        // occurrence time, when known, is carried by an explicit `OccurredOn`
        // literal-object edge, never by `valid_from`. Leave validity unknown.
        None,
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
    memory_id: &str,
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
        memory_id: memory_id.to_string(),
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
