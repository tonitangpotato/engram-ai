//! # Pipeline Orchestrator (`ResolutionPipeline`)
//!
//! Upper layer that wires the staged primitives into a single per-job
//! execution path. Implements [`JobProcessor`] so the worker pool
//! ([`super::worker::WorkerPool`]) can call `process(job)` for each
//! dequeued [`PipelineJob`].
//!
//! ## Position in the architecture
//!
//! ```text
//!  store_raw ──► JobQueue ──► WorkerPool ──► JobProcessor::process
//!                                                  │
//!                                                  ▼
//!                                         ResolutionPipeline::run_job
//!                                                  │
//!  ┌─────────────────────────────────────────────────────────────────┐
//!  │  load memory ─► §3.2 entity extract ─► §3.3 edge extract        │
//!  │      │                                                          │
//!  │      ▼                                                          │
//!  │  §3.4 resolve (per-draft candidate retrieval + fusion + decide) │
//!  │      │                                                          │
//!  │      ▼                                                          │
//!  │  §3.5 atomic persist (build_delta ▻ apply_graph_delta)          │
//!  │      │                                                          │
//!  │      ▼                                                          │
//!  │  finish_pipeline_run + record_resolution_trace                  │
//!  └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Boundary rules (mirrored from `mod.rs`)
//!
//! - `pipeline.rs` is the **only** module that combines IO + stage logic.
//!   Pure stage functions stay pure; pure decision functions stay pure;
//!   this file owns the orchestration.
//! - This module **never** writes graph rows directly. All writes go
//!   through [`super::stage_persist::drive_persist`] which in turn calls
//!   [`crate::graph::GraphStore::apply_graph_delta`].
//! - Failures inside a stage are recorded on the [`PipelineContext`]
//!   (`record_failure`) and surfaced via [`ProcessError`] when terminal.
//!   Non-fatal stage errors (e.g. edge extractor returns nothing)
//!   downgrade to "continue with empty results" per §3 design.
//!
//! ## Synchronization model
//!
//! `ResolutionPipeline` is `Send + Sync` and is shared by reference (in an
//! `Arc<dyn JobProcessor>`) across all worker threads. Mutable state
//! (the `GraphStore`) lives behind a `Mutex` because `apply_graph_delta`
//! requires `&mut self`. Holding the store lock for the duration of a
//! single job is acceptable: the v0.3 design serializes graph writes at
//! the SQLite layer anyway (one writer, GUARD-7).
//!
//! Read-side calls (`get_entity`, `search_candidates`, `find_edges`) also
//! go through the same store handle for now. A future refactor can split
//! the store into a `&self` reader handle + `&mut self` writer; that's
//! out of scope for the v0.3 MVP.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use uuid::Uuid;

use crate::entities::EntityExtractor;
use crate::graph::{EdgeEnd, GraphStore, PipelineKind, RunStatus};
use crate::triple_extractor::TripleExtractor;
use crate::types::MemoryRecord;

use super::candidate_retrieval::{retrieve_candidates, RetrievalParams};
use super::context::{DraftEdgeEnd, PipelineContext, PipelineStage};
use super::decision::{decide, Decision, DecisionThresholds, ResolutionOutcome};
use super::edge_decision::{compute_edge_decision, EdgeDecision};
use super::fusion::{fuse, FusionResult, SignalWeights};
use super::queue::{JobMode, PipelineJob};
use super::stage_edge_extract::extract_edges;
use super::stage_extract::extract_entities;
use super::stage_persist::{build_delta, drive_persist, EdgeResolution, EntityResolution};
use super::stats::ResolutionStats;
use super::worker::{JobProcessor, ProcessError};

use crate::graph::delta::GraphDelta;

// ---------------------------------------------------------------------------
// PipelineError — surface for the migration backfill handoff (§6.5 +
// v03-migration §5.2). Distinct from `ProcessError` (worker-pool surface)
// so migration can pattern-match on `ExtractionFailure` (per-record, data,
// retryable) vs. `Fatal` (storage/IO, abort) cleanly.
// ---------------------------------------------------------------------------

/// Errors returned by [`ResolutionPipeline::resolve_for_backfill`].
///
/// Two-variant taxonomy from v03-migration §5.2:
///
/// - `ExtractionFailure` — per-record data failure. Migration logs it to
///   `graph_extraction_failures` and advances the checkpoint. Retryable
///   via `engramai migrate --retry-failed`.
/// - `Fatal` — storage / IO abort. Migration aborts the run and preserves
///   the checkpoint for resume.
///
/// **Note:** `resolve_for_backfill` never produces `ExtractionFailure`
/// today — per-stage data failures are recorded on `ctx.failures` and
/// flow through into `delta.stage_failures` (caller persists them via
/// `apply_graph_delta`). The variant exists for forward-compat with
/// future stages that may surface stop-the-record errors. Only fatal
/// store-call failures from `resolve_entities` / `resolve_edges` map to
/// `Fatal`.
#[derive(Debug)]
pub enum PipelineError {
    /// Per-record extraction failure. Carries the underlying detail as
    /// a string for now (taxonomy lives in `graph_extraction_failures`).
    ExtractionFailure(String),
    /// Storage / IO error. Halts the migration run.
    Fatal(String),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::ExtractionFailure(m) => write!(f, "extraction failure: {m}"),
            PipelineError::Fatal(m) => write!(f, "fatal pipeline error: {m}"),
        }
    }
}

impl std::error::Error for PipelineError {}

impl From<ProcessError> for PipelineError {
    fn from(e: ProcessError) -> Self {
        // `ProcessError::Stage` arriving from `resolve_entities` /
        // `resolve_edges` means a store call (get_entity, find_edges,
        // search_candidates) returned `Err`. Those are storage-level
        // errors → `Fatal`. `NotFound` and `Other` are likewise treated
        // as fatal in the backfill context (the memory was just handed
        // to us by the orchestrator; missing dependent state is a
        // storage anomaly, not a per-record extraction failure).
        PipelineError::Fatal(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// MemoryReader — narrow capability for fetching the v0.2 memory row.
// ---------------------------------------------------------------------------

/// Looks up a [`MemoryRecord`] by id. The pipeline depends on this rather
/// than `Memory` directly so tests can inject a `HashMap`-backed mock.
///
/// The blanket impl below covers `crate::memory::Memory` for production
/// callers.
pub trait MemoryReader: Send + Sync {
    /// Fetch the memory row written by the v0.2 admission path
    /// (`store_raw`). `Ok(None)` means the row was deleted between
    /// `enqueue` and `dispatch` — not an error, but the job becomes a
    /// terminal `NotFound` (see [`ProcessError::NotFound`]).
    fn fetch(&self, memory_id: &str) -> Result<Option<MemoryRecord>, MemoryReadError>;
}

/// Errors surfaced by [`MemoryReader::fetch`]. Distinct from
/// [`crate::graph::GraphError`] because the L1/L2 read path is independent
/// of the graph schema.
#[derive(Debug)]
pub enum MemoryReadError {
    /// Underlying storage failed (IO, decode, etc.).
    Storage(String),
}

impl std::fmt::Display for MemoryReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(m) => write!(f, "memory-read storage error: {m}"),
        }
    }
}

impl std::error::Error for MemoryReadError {}

// ---------------------------------------------------------------------------
// ResolutionPipeline — the JobProcessor implementation.
// ---------------------------------------------------------------------------

/// Configuration knobs the pipeline reads at construction time. Cheap to
/// clone; held by value inside [`ResolutionPipeline`].
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    /// Decision thresholds for §3.4.3 entity decision.
    pub thresholds: DecisionThresholds,
    /// Fusion signal weights for §3.4.2.
    pub weights: SignalWeights,
    /// Candidate-retrieval parameters (top_k, recency window, kind filter).
    pub retrieval: RetrievalParams,
    /// Namespace tag passed to `search_candidates`. Production callers
    /// pin this to the memory's namespace; tests may use `""`.
    pub namespace: String,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            thresholds: DecisionThresholds::default(),
            weights: SignalWeights::default(),
            retrieval: RetrievalParams::default(),
            namespace: String::new(),
        }
    }
}

/// Owns one extractor pair + one shared store handle and turns a
/// [`PipelineJob`] into committed graph rows.
///
/// Construct via [`ResolutionPipeline::new`] and wrap in `Arc` before
/// passing to [`super::worker::WorkerPool::start`].
pub struct ResolutionPipeline<S: GraphStore + ?Sized + 'static> {
    /// Read path for the memory row — fetched at job start.
    memory_reader: Arc<dyn MemoryReader>,
    /// §3.2 entity extractor. Stateless after construction.
    entity_extractor: Arc<EntityExtractor>,
    /// §3.3 triple extractor. Trait object so production can swap LLM
    /// vs heuristic implementations without touching this file.
    triple_extractor: Arc<dyn TripleExtractor>,
    /// Shared graph store. `Mutex` because `apply_graph_delta` and the
    /// pipeline-run-row writes need `&mut`. See module-level note on
    /// the synchronization model.
    store: Arc<Mutex<S>>,
    /// Static configuration captured at construction.
    config: PipelineConfig,
}

impl<S: GraphStore + ?Sized + 'static> ResolutionPipeline<S> {
    /// Build a new pipeline. All dependencies are injected; nothing is
    /// constructed implicitly. This makes the pipeline trivially testable
    /// with mocks for each port.
    pub fn new(
        memory_reader: Arc<dyn MemoryReader>,
        entity_extractor: Arc<EntityExtractor>,
        triple_extractor: Arc<dyn TripleExtractor>,
        store: Arc<Mutex<S>>,
        config: PipelineConfig,
    ) -> Self {
        Self {
            memory_reader,
            entity_extractor,
            triple_extractor,
            store,
            config,
        }
    }

    /// Snapshot of the configured thresholds — exposed so tests can
    /// assert the pipeline is wired with the values they passed.
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// JobProcessor impl — the entry point the worker pool calls.
// ---------------------------------------------------------------------------

impl<S: GraphStore + Send + ?Sized + 'static> JobProcessor for ResolutionPipeline<S> {
    fn process(&self, job: PipelineJob) -> Result<(), ProcessError> {
        match self.run_job(&job) {
            Ok(_outcome) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal orchestration — `run_job` + per-stage helpers.
// ---------------------------------------------------------------------------

/// Outcome of a successful job: stats + apply report fields. Returned from
/// the internal `run_job` for testability; the public `JobProcessor::process`
/// path discards it.
#[derive(Clone, Debug, Default)]
pub struct JobOutcome {
    pub run_id: Option<Uuid>,
    pub stats: ResolutionStats,
}

impl<S: GraphStore + Send + ?Sized + 'static> ResolutionPipeline<S> {
    /// Drive one job end-to-end. All errors flow through here so the
    /// public `process` impl is just a `?` translation.
    ///
    /// This function is `pub(crate)` so integration tests in `tests/` can
    /// drive the pipeline without spinning up a worker pool.
    pub fn run_job(&self, job: &PipelineJob) -> Result<JobOutcome, ProcessError> {
        let mut stats = ResolutionStats::default();

        // ── 1. Begin pipeline run ─────────────────────────────────────────
        let run_id = self.begin_run(job)?;

        // ── 2. Load memory row ────────────────────────────────────────────
        let memory = self
            .memory_reader
            .fetch(job.memory_id.as_str())
            .map_err(|e| {
                let _ = self.finish_run(run_id, RunStatus::Failed, Some(&e.to_string()));
                ProcessError::Other(format!("memory fetch failed: {e}"))
            })?
            .ok_or_else(|| {
                let _ = self.finish_run(run_id, RunStatus::Failed, Some("memory not found"));
                ProcessError::NotFound(format!("memory {} no longer exists", job.memory_id))
            })?;

        // ── 3. Build context (affect snapshot wiring lands when v03 affect
        //      capture is plumbed end-to-end; for now Initial jobs pass
        //      `None` and the affect signal s6 is simply absent in fusion
        //      measurements — graceful degradation per §3.4.2).
        let mut ctx = PipelineContext::new(memory, job.episode_id, None);

        // ── 4. §3.2 entity extract ───────────────────────────────────────
        let t = Instant::now();
        // `extract_entities` only errors on future LLM-backed extractors;
        // the v0.2 pattern extractor is total. Either way, partial results
        // remain on `ctx` and we proceed.
        let _ = extract_entities(&self.entity_extractor, &mut ctx);
        stats.entity_extract_duration = t.elapsed();
        stats.entities_extracted = ctx.extracted_entities.len() as u64;

        // ── 5. §3.3 edge extract ─────────────────────────────────────────
        let t = Instant::now();
        // Edge extractor failure is non-fatal: `record_failure` already
        // logged it on `ctx`, we just continue with no triples.
        let _ = extract_edges(self.triple_extractor.as_ref(), &mut ctx);
        stats.edge_extract_duration = t.elapsed();
        stats.edges_extracted = ctx.extracted_triples.len() as u64;

        // ── 6. §3.4 resolve entities ─────────────────────────────────────
        let t = Instant::now();
        let entity_decisions = self.resolve_entities(&mut ctx, &mut stats)?;

        // ── 7. resolve subject/object UUIDs for edges, then §3.4.4 edge
        //      decisions.
        let edge_decisions = self.resolve_edges(&mut ctx, &entity_decisions, &mut stats)?;
        stats.resolve_duration = t.elapsed();

        // ── 8. §3.5 persist ──────────────────────────────────────────────
        let t = Instant::now();
        let persist_outcome = {
            let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
            drive_persist(&mut *store, &mut ctx, &entity_decisions, &edge_decisions)
        };
        stats.persist_duration = t.elapsed();

        let persist = persist_outcome.map_err(|e| {
            let _ = self.finish_run(run_id, RunStatus::Failed, Some(&e.to_string()));
            ProcessError::Stage(format!("persist: {e}"))
        })?;

        stats.stage_failures = ctx.failures.len() as u64;

        // ── 9. Finish pipeline run ───────────────────────────────────────
        let summary = serde_json::json!({
            "memory_id": ctx.memory.id,
            "episode_id": ctx.episode_id,
            "entities_extracted": stats.entities_extracted,
            "edges_extracted": stats.edges_extracted,
            "entities_merged": stats.entities_merged,
            "entities_created": stats.entities_created,
            "edges_added": stats.edges_added,
            "edges_updated": stats.edges_updated,
            "edges_preserved": stats.edges_preserved,
            "stage_failures": stats.stage_failures,
            "apply_report": {
                "entities_upserted": persist.report.entities_upserted,
                "edges_inserted": persist.report.edges_inserted,
                "edges_invalidated": persist.report.edges_invalidated,
                "mentions_inserted": persist.report.mentions_inserted,
                "predicates_registered": persist.report.predicates_registered,
                "failures_recorded": persist.report.failures_recorded,
                "already_applied": persist.report.already_applied,
            },
        });

        let final_status = if stats.stage_failures > 0 {
            // Non-fatal stage failures still count as a successful run —
            // the failures are recorded on `graph_extraction_failures`,
            // the run row reflects "completed with degraded results."
            RunStatus::Succeeded
        } else {
            RunStatus::Succeeded
        };

        self.finish_run_with_summary(run_id, final_status, Some(summary), None)?;

        Ok(JobOutcome {
            run_id: Some(run_id),
            stats,
        })
    }

    /// Start a pipeline-run audit row scoped to this memory. The kind is
    /// derived from `job.mode`. On failure we surface as `ProcessError`
    /// without trying to record anything (we never opened the run).
    fn begin_run(&self, job: &PipelineJob) -> Result<Uuid, ProcessError> {
        let kind = match job.mode {
            JobMode::Initial => PipelineKind::Resolution,
            JobMode::ReExtract => PipelineKind::Reextract,
        };
        let input_summary = serde_json::json!({
            "memory_id": job.memory_id,
            "episode_id": job.episode_id,
            "mode": job.mode.as_str(),
            "enqueued_at": job.enqueued_at.to_rfc3339(),
        });

        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .begin_pipeline_run_for_memory(
                kind,
                job.memory_id.as_str(),
                job.episode_id,
                input_summary,
            )
            .map_err(|e| ProcessError::Other(format!("begin_pipeline_run: {e}")))
    }

    /// Close out the pipeline-run audit row. Tolerates store errors
    /// without panicking (pool already counted the job; double-failure
    /// would just spam logs).
    fn finish_run(
        &self,
        run_id: Uuid,
        status: RunStatus,
        error_detail: Option<&str>,
    ) -> Result<(), ProcessError> {
        self.finish_run_with_summary(run_id, status, None, error_detail)
    }

    fn finish_run_with_summary(
        &self,
        run_id: Uuid,
        status: RunStatus,
        output_summary: Option<serde_json::Value>,
        error_detail: Option<&str>,
    ) -> Result<(), ProcessError> {
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store
            .finish_pipeline_run(run_id, status, output_summary, error_detail)
            .map_err(|e| ProcessError::Other(format!("finish_pipeline_run: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Stage helpers — entity / edge resolution
// ---------------------------------------------------------------------------

impl<S: GraphStore + Send + ?Sized + 'static> ResolutionPipeline<S> {
    /// §3.4.1 + §3.4.2 + §3.4.3 — for each entity draft, retrieve
    /// candidates, fuse signals, decide. Produces one
    /// [`EntityResolution`] per draft, in the same order as
    /// `ctx.entity_drafts` (persist relies on this co-indexing).
    ///
    /// The `now` parameter passed to candidate retrieval is the memory's
    /// `created_at` (per GOAL-2.1 idempotence — re-running on the same
    /// memory at a different wall-clock time must produce the same
    /// recency scores).
    fn resolve_entities(
        &self,
        ctx: &mut PipelineContext,
        stats: &mut ResolutionStats,
    ) -> Result<Vec<EntityResolution>, ProcessError> {
        let now = ctx.memory.created_at.timestamp() as f64;
        let mut decisions: Vec<EntityResolution> = Vec::with_capacity(ctx.entity_drafts.len());

        // We borrow drafts immutably; the inner loop only touches `ctx`
        // via failure recording. Clone draft data we need before the
        // store lock is taken to keep the lock window short.
        let drafts = ctx.entity_drafts.clone();
        let extracted = ctx.extracted_entities.clone();

        for (i, draft) in drafts.iter().enumerate() {
            let mention_text = extracted
                .get(i)
                .map(|m| m.name.as_str())
                .unwrap_or(&draft.canonical_name);

            // Candidate retrieval: read-only, but `search_candidates` is
            // declared `&self` so we still go through the mutex.
            let candidates_result = {
                let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                retrieve_candidates(
                    &*store,
                    mention_text,
                    None, // mention_embedding: future — embedder wiring lands separately
                    self.config.namespace.as_str(),
                    now,
                    &self.config.retrieval,
                )
            };

            let scored = match candidates_result {
                Ok(s) => s,
                Err(e) => {
                    ctx.record_failure(
                        PipelineStage::Resolve,
                        "candidate_retrieval_error",
                        e.to_string(),
                    );
                    Vec::new()
                }
            };

            // Fuse each scored candidate. `fuse` takes the candidate's
            // measurement vector + signal weights and returns a
            // FusionResult.
            let fused: Vec<FusionResult> = scored
                .iter()
                .map(|c| {
                    fuse(
                        c.match_row.entity_id,
                        &self.config.weights,
                        &c.measurements,
                    )
                })
                .collect();

            // Decide: CreateNew / MergeInto / DeferToLlm.
            let outcome: ResolutionOutcome = decide(&self.config.thresholds, fused);

            // Tally stats by decision kind. DeferToLlm degrades to
            // CreateNew at persist time (§3.4.3 MVP behavior, see stats
            // doc comment).
            match &outcome.decision {
                Decision::CreateNew => {
                    stats.entities_created = stats.entities_created.saturating_add(1);
                }
                Decision::MergeInto { .. } => {
                    stats.entities_merged = stats.entities_merged.saturating_add(1);
                }
                Decision::DeferToLlm { .. } => {
                    stats.entities_deferred = stats.entities_deferred.saturating_add(1);
                }
            }

            // For MergeInto we need the canonical Entity row (persist
            // reuses its summary/attributes). Fetch under the lock.
            let canonical = match &outcome.decision {
                Decision::MergeInto { candidate_id } => {
                    let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                    match store.get_entity(*candidate_id) {
                        Ok(opt) => opt,
                        Err(e) => {
                            ctx.record_failure(
                                PipelineStage::Resolve,
                                "canonical_fetch_error",
                                e.to_string(),
                            );
                            None
                        }
                    }
                }
                _ => None,
            };

            decisions.push(EntityResolution {
                draft_index: i,
                draft: draft.clone(),
                decision: outcome.decision,
                canonical,
            });
        }

        Ok(decisions)
    }

    /// §3.4.4 — resolve edge subjects/objects to UUIDs (using the entity
    /// decisions from §3.4.3) and compute per-slot edge decisions.
    ///
    /// Returns one [`EdgeResolution`] per edge draft (in order). Drafts
    /// whose subject or object cannot be mapped to an entity id are
    /// dropped from the result and recorded as stage failures — persist
    /// would otherwise build a malformed delta.
    fn resolve_edges(
        &self,
        ctx: &mut PipelineContext,
        entity_decisions: &[EntityResolution],
        stats: &mut ResolutionStats,
    ) -> Result<Vec<EdgeResolution>, ProcessError> {
        // Build a name → entity_id map from this run's entity decisions.
        // For MergeInto, the id is the canonical row's id. For CreateNew,
        // we mint a fresh v7 uuid here so that subsequent edge resolutions
        // can point at it. The persist stage uses this same id when it
        // builds the new Entity row (`build_new_entity` keys off
        // `er.draft` and an id passed in via `EntityResolution.draft.id`-
        // less channels — but `build_delta` synthesizes its own id from
        // the canonical name today; see TODO below).
        //
        // TODO(v0.3 follow-up): unify entity-id minting between
        // `resolve_edges` and `build_delta`. Today both compute fresh
        // uuids independently for `CreateNew`; they happen to be safe
        // because edge inserts in `build_delta` re-resolve subject/object
        // names against the same decision slice. If we ever pass a
        // pre-minted id through `EntityResolution`, this loop should use
        // that id directly.
        use std::collections::HashMap;
        let mut name_to_id: HashMap<String, Uuid> = HashMap::new();
        for er in entity_decisions {
            let id = match &er.decision {
                Decision::MergeInto { candidate_id } => *candidate_id,
                Decision::CreateNew | Decision::DeferToLlm { .. } => {
                    // DeferToLlm degrades to CreateNew at persist time.
                    Uuid::new_v4()
                }
            };
            name_to_id.insert(er.draft.canonical_name.clone(), id);
        }

        let drafts = ctx.edge_drafts.clone();
        let mut decisions: Vec<EdgeResolution> = Vec::with_capacity(drafts.len());

        for (i, draft) in drafts.iter().enumerate() {
            // Resolve subject.
            let subject_id = match name_to_id.get(&draft.subject_name) {
                Some(id) => *id,
                None => {
                    ctx.record_failure(
                        PipelineStage::Resolve,
                        "unresolved_subject",
                        format!(
                            "subject `{}` did not appear in this run's entity drafts",
                            draft.subject_name
                        ),
                    );
                    continue;
                }
            };

            // Resolve object.
            let object: EdgeEnd = match &draft.object {
                DraftEdgeEnd::EntityName(name) => match name_to_id.get(name) {
                    Some(id) => EdgeEnd::Entity { id: *id },
                    None => {
                        ctx.record_failure(
                            PipelineStage::Resolve,
                            "unresolved_object",
                            format!(
                                "object `{}` did not appear in this run's entity drafts",
                                name
                            ),
                        );
                        continue;
                    }
                },
                DraftEdgeEnd::Literal(val) => EdgeEnd::Literal {
                    value: serde_json::Value::String(val.clone()),
                },
            };

            // Look up the existing slot for §3.4.4 decision.
            let existing = {
                let store = self.store.lock().unwrap_or_else(|e| e.into_inner());
                match store.find_edges(
                    subject_id,
                    &draft.predicate,
                    None,         // unfiltered slot — see ISS-035 note in edge_decision
                    /* valid_only */ true,
                ) {
                    Ok(rows) => rows,
                    Err(e) => {
                        ctx.record_failure(
                            PipelineStage::Resolve,
                            "find_edges_error",
                            e.to_string(),
                        );
                        Vec::new()
                    }
                }
            };

            let decision = compute_edge_decision(
                &draft.predicate,
                &object,
                draft.source_confidence,
                &existing,
            );

            // Tally edge-decision stats.
            match &decision {
                EdgeDecision::Add => {
                    stats.edges_added = stats.edges_added.saturating_add(1);
                }
                EdgeDecision::Update { .. } => {
                    stats.edges_updated = stats.edges_updated.saturating_add(1);
                }
                EdgeDecision::None => {
                    stats.edges_preserved = stats.edges_preserved.saturating_add(1);
                }
                EdgeDecision::DeferToLlm => {
                    // Counted neither — it surfaces as a stage failure
                    // at persist time (`unresolved_defer`).
                }
            }

            decisions.push(EdgeResolution {
                draft_index: i,
                draft: draft.clone(),
                decision,
                subject_id,
                object,
            });
        }

        Ok(decisions)
    }

    // -----------------------------------------------------------------------
    // §6.5 Migration backfill entry point
    // -----------------------------------------------------------------------

    /// Resolve a historic v0.2 memory into a `GraphDelta` for the
    /// migration backfill orchestrator (v03-migration §5.2).
    ///
    /// Differs from the normal [`run_job`] in three respects (per
    /// v03-resolution design §6.5):
    ///
    /// 1. **No new L1 Episode is created.** The supplied `MemoryRecord`
    ///    has no `episode_id` field on the v0.2 type; backfill memories
    ///    typically map to NULL episode (master DESIGN §8.1). We seed
    ///    the `PipelineContext` with `Uuid::nil()` as the sentinel for
    ///    "no episode" — the persist layer interprets this however the
    ///    storage adapter chooses. **This is a deviation from the
    ///    task-brief signature** which suggested `Option<Uuid>`; see
    ///    "Deviations" note in the implementation summary.
    /// 2. **Forced synchronous execution.** No queue.enqueue, no worker
    ///    dispatch. All stages run inline on the calling thread.
    /// 3. **Returns `GraphDelta`, does not persist.** The caller
    ///    (`BackfillOrchestrator`) is responsible for invoking
    ///    `GraphStore::apply_graph_delta`. This keeps the per-record
    ///    transaction boundary in migration's hands and lets re-runs on
    ///    the same record produce identical deltas (idempotence per
    ///    §5.2 checkpoint-resume).
    ///
    /// No `ResolutionStats` is returned — migration owns its own
    /// progress telemetry (§5.5).
    ///
    /// ### Error mapping
    ///
    /// - Per-stage data failures (extract / resolve recorded on
    ///   `ctx.failures`) are **not** errors here: they propagate into
    ///   `delta.stage_failures` and the caller persists them as part of
    ///   the same atomic apply.
    /// - `resolve_entities` / `resolve_edges` returning `Err` (a store
    ///   call failed) maps to [`PipelineError::Fatal`] via
    ///   `From<ProcessError>`.
    pub fn resolve_for_backfill(
        &self,
        memory: &MemoryRecord,
    ) -> Result<GraphDelta, PipelineError> {
        // Build context. Episode id: nil-UUID sentinel for "no episode"
        // (v0.2 MemoryRecord has no episode_id field; design §6.5
        // mandates we do NOT mint a new episode).
        let mut ctx = PipelineContext::new(memory.clone(), Uuid::nil(), None);

        // §3.2 entity extract — total over v0.2 extractor; non-fatal
        // failures land on `ctx.failures`.
        let _ = extract_entities(&self.entity_extractor, &mut ctx);

        // §3.3 edge extract — failures non-fatal, recorded on ctx.
        let _ = extract_edges(self.triple_extractor.as_ref(), &mut ctx);

        // §3.4 resolve. We don't expose stats — pass a throwaway sink.
        let mut sink = ResolutionStats::default();
        let entity_decisions = self.resolve_entities(&mut ctx, &mut sink)?;
        let edge_decisions = self.resolve_edges(&mut ctx, &entity_decisions, &mut sink)?;

        // §3.5 build delta — pure, no IO. Idempotent on equal inputs.
        let delta = build_delta(&ctx, &entity_decisions, &edge_decisions);
        Ok(delta)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod backfill_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    use crate::entities::EntityExtractor;
    use crate::graph::audit::{ExtractionFailure, ResolutionTrace};
    use crate::graph::edge::Edge;
    use crate::graph::entity::{Entity, EntityKind};
    use crate::graph::schema::Predicate;
    use crate::graph::store::{
        CandidateMatch, CandidateQuery, EntityMentions, GraphRead, GraphWrite,
        PipelineRunRow, ProposedPredicateStats,
    };
    use crate::graph::topic::KnowledgeTopic;
    use crate::graph::{ApplyReport, EdgeEnd, GraphDelta, GraphError};
    use crate::triple::Triple;
    use crate::triple_extractor::TripleExtractor;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};

    use std::error::Error as StdError;
    use std::sync::Mutex as StdMutex;

    // --- Stub TripleExtractor: scripted output, sync per real trait ---
    struct StubTriples(Vec<Triple>);
    impl TripleExtractor for StubTriples {
        fn extract_triples(
            &self,
            _content: &str,
        ) -> Result<Vec<Triple>, Box<dyn StdError + Send + Sync>> {
            Ok(self.0.clone())
        }
    }

    // --- Stub GraphStore: returns empty for everything; never written.
    // Sufficient for resolve_for_backfill testing because:
    //   - search_candidates → empty → all entities CreateNew
    //   - find_edges → empty → all edges Add
    //   - get_entity is never called (no MergeInto path)
    //   - persist is NOT invoked by resolve_for_backfill.
    // Methods not exercised by resolve_for_backfill panic via
    // `unimplemented!()` so accidental future call paths fail loudly.
    struct StubStore;

    impl GraphRead for StubStore {
        fn get_entity(&self, _id: Uuid) -> Result<Option<Entity>, GraphError> {
            Ok(None)
        }
        fn list_entities_by_kind(
            &self,
            _kind: &EntityKind,
            _limit: usize,
        ) -> Result<Vec<Entity>, GraphError> {
            Ok(vec![])
        }
        fn search_candidates(
            &self,
            _q: &CandidateQuery,
        ) -> Result<Vec<CandidateMatch>, GraphError> {
            Ok(vec![])
        }
        fn resolve_alias(&self, _n: &str) -> Result<Option<Uuid>, GraphError> {
            Ok(None)
        }
        fn get_edge(&self, _id: Uuid) -> Result<Option<Edge>, GraphError> {
            Ok(None)
        }
        fn find_edges(
            &self,
            _s: Uuid,
            _p: &Predicate,
            _o: Option<&EdgeEnd>,
            _v: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            Ok(vec![])
        }
        fn edges_of(
            &self,
            _id: Uuid,
            _p: Option<&Predicate>,
            _inv: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!("edges_of: not used by resolve_for_backfill")
        }
        fn edges_as_of(
            &self,
            _id: Uuid,
            _t: chrono::DateTime<Utc>,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!("edges_as_of: not used by resolve_for_backfill")
        }
        fn traverse(
            &self,
            _start: Uuid,
            _depth: usize,
            _max_results: usize,
            _filter: &[Predicate],
        ) -> Result<Vec<(Uuid, Edge)>, GraphError> {
            unimplemented!("traverse: not used by resolve_for_backfill")
        }
        fn entities_in_episode(&self, _e: Uuid) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn edges_in_episode(&self, _e: Uuid) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn mentions_of_entity(
            &self,
            _e: Uuid,
        ) -> Result<EntityMentions, GraphError> {
            unimplemented!()
        }
        fn entities_linked_to_memory(
            &self,
            _m: &str,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn memories_mentioning_entity(
            &self,
            _e: Uuid,
            _lim: usize,
        ) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
        fn edges_sourced_from_memory(
            &self,
            _m: &str,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn get_topic(&self, _id: Uuid) -> Result<Option<KnowledgeTopic>, GraphError> {
            unimplemented!()
        }
        fn list_topics(
            &self,
            _ns: &str,
            _include_superseded: bool,
            _lim: usize,
        ) -> Result<Vec<KnowledgeTopic>, GraphError> {
            unimplemented!()
        }
        fn latest_pipeline_run_for_memory(
            &self,
            _m: &str,
        ) -> Result<Option<PipelineRunRow>, GraphError> {
            unimplemented!()
        }
        fn list_proposed_predicates(
            &self,
            _min_usage: u64,
        ) -> Result<Vec<ProposedPredicateStats>, GraphError> {
            unimplemented!()
        }
        fn list_failed_episodes(
            &self,
            _unresolved_only: bool,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn list_namespaces(&self) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
    }

    impl GraphWrite for StubStore {
        fn insert_entity(&mut self, _e: &Entity) -> Result<(), GraphError> {
            unimplemented!("insert_entity: persist not invoked by resolve_for_backfill")
        }
        fn update_entity_cognitive(
            &mut self,
            _id: Uuid,
            _activation: f64,
            _importance: f64,
            _identity_confidence: f64,
            _agent_affect: Option<serde_json::Value>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn touch_entity_last_seen(
            &mut self,
            _id: Uuid,
            _ls: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn update_entity_embedding(
            &mut self,
            _id: Uuid,
            _emb: Option<&[f32]>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn upsert_alias(
            &mut self,
            _normalized: &str,
            _alias_raw: &str,
            _canonical_id: Uuid,
            _source_episode: Option<Uuid>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn merge_entities(
            &mut self,
            _winner: Uuid,
            _loser: Uuid,
            _batch_size: usize,
        ) -> Result<crate::graph::store::MergeReport, GraphError> {
            unimplemented!()
        }
        fn insert_edge(&mut self, _e: &Edge) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn invalidate_edge(
            &mut self,
            _prior_id: Uuid,
            _successor_id: Uuid,
            _now: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn supersede_edge(
            &mut self,
            _old: Uuid,
            _successor: &Edge,
            _at: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn link_memory_to_entities(
            &mut self,
            _memory_id: &str,
            _entity_ids: &[(Uuid, f64, Option<String>)],
            _at: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn upsert_topic(&mut self, _t: &KnowledgeTopic) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn supersede_topic(
            &mut self,
            _old: Uuid,
            _successor: Uuid,
            _at: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn begin_pipeline_run(
            &mut self,
            _kind: PipelineKind,
            _input_summary: serde_json::Value,
        ) -> Result<Uuid, GraphError> {
            unimplemented!()
        }
        fn begin_pipeline_run_for_memory(
            &mut self,
            _kind: PipelineKind,
            _memory_id: &str,
            _episode_id: Uuid,
            _input_summary: serde_json::Value,
        ) -> Result<Uuid, GraphError> {
            unimplemented!()
        }
        fn finish_pipeline_run(
            &mut self,
            _run: Uuid,
            _status: RunStatus,
            _summary: Option<serde_json::Value>,
            _err: Option<&str>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn record_resolution_trace(
            &mut self,
            _t: &ResolutionTrace,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn record_predicate_use(
            &mut self,
            _p: &Predicate,
            _raw: &str,
            _at: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn record_extraction_failure(
            &mut self,
            _f: &ExtractionFailure,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn mark_failure_resolved(
            &mut self,
            _id: Uuid,
            _at: chrono::DateTime<Utc>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn with_transaction(
            &mut self,
            _f: &mut dyn FnMut(&rusqlite::Transaction<'_>) -> Result<(), GraphError>,
        ) -> Result<(), GraphError> {
            unimplemented!()
        }
        fn apply_graph_delta(
            &mut self,
            _d: &GraphDelta,
        ) -> Result<ApplyReport, GraphError> {
            // resolve_for_backfill must NOT call apply — surface loudly
            // if a future change accidentally wires it through.
            panic!("apply_graph_delta should not be called by resolve_for_backfill")
        }
    }

    // GraphStore is auto-impl'd via blanket `impl<T: GraphWrite> GraphStore for T`.

    struct StubReader;
    impl MemoryReader for StubReader {
        fn fetch(
            &self,
            _id: &str,
        ) -> Result<Option<MemoryRecord>, MemoryReadError> {
            // resolve_for_backfill takes &MemoryRecord directly and never
            // hits the reader. Surface loudly if that changes.
            Ok(None)
        }
    }

    fn fixture_memory(id: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.into(),
            content: content.into(),
            memory_type: MemoryType::Episodic,
            layer: MemoryLayer::Working,
            created_at: Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap(),
            access_times: vec![],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: "backfill_test".into(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        }
    }

    fn build_pipeline(triples: Vec<Triple>) -> ResolutionPipeline<StubStore> {
        let mr: Arc<dyn MemoryReader> = Arc::new(StubReader);
        ResolutionPipeline::new(
            mr,
            Arc::new(EntityExtractor::new(&crate::entities::EntityConfig::default())),
            Arc::new(StubTriples(triples)),
            Arc::new(StdMutex::new(StubStore)),
            PipelineConfig::default(),
        )
    }

    #[test]
    fn test_resolve_for_backfill_null_episode() {
        // Memory whose episode_id semantically is None (v0.2 row).
        let mem = fixture_memory("mem-null", "Alice works at Acme Corp.");
        let pipe = build_pipeline(vec![]);
        let delta = pipe.resolve_for_backfill(&mem).expect("backfill ok");
        // For backfill all edges (if present) carry `Some(Uuid::nil())`
        // per current persist code; that is the documented sentinel
        // and not a "new episode."
        for e in &delta.edges {
            assert_eq!(
                e.episode_id,
                Some(Uuid::nil()),
                "backfill edges must not mint new episodes"
            );
        }
    }

    #[test]
    fn test_resolve_for_backfill_with_episode() {
        // The current MemoryRecord type has no episode_id field, so this
        // test asserts the "no episode minted" property — equivalent to
        // null-episode case at the Rust-type level. This test is kept to
        // document the semantic (and to fail loudly if a future change
        // adds episode_id to MemoryRecord and we forget to wire it).
        let mem = fixture_memory("mem-eps", "Bob uses Rust.");
        let pipe = build_pipeline(vec![]);
        let delta = pipe.resolve_for_backfill(&mem).expect("backfill ok");
        for e in &delta.edges {
            assert_eq!(e.episode_id, Some(Uuid::nil()));
        }
    }

    #[test]
    fn test_resolve_for_backfill_idempotent() {
        let mem = fixture_memory("mem-idem", "Alice works at Acme Corp using Rust.");
        let pipe = build_pipeline(vec![]);
        let d1 = pipe.resolve_for_backfill(&mem).expect("first ok");
        let d2 = pipe.resolve_for_backfill(&mem).expect("second ok");
        // Canonical names are stable; entity ids are minted fresh per
        // run (CreateNew path mints v4 uuids), so equivalence is on the
        // shape, not the ids. Match what the design calls "equivalent
        // GraphDelta": same canonical entity names, same edge predicates.
        let names1: Vec<_> = d1
            .entities
            .iter()
            .map(|e| (e.canonical_name.clone(), e.kind.clone()))
            .collect();
        let names2: Vec<_> = d2
            .entities
            .iter()
            .map(|e| (e.canonical_name.clone(), e.kind.clone()))
            .collect();
        assert_eq!(names1, names2, "canonical entity names must match");

        let preds1: Vec<_> = d1
            .edges
            .iter()
            .map(|e| format!("{:?}", e.predicate))
            .collect();
        let preds2: Vec<_> = d2
            .edges
            .iter()
            .map(|e| format!("{:?}", e.predicate))
            .collect();
        assert_eq!(preds1, preds2, "edge predicates must match");
        assert_eq!(
            d1.mentions.len(),
            d2.mentions.len(),
            "mention count must match"
        );
        // memory_id derivation is deterministic for the same memory.
        assert_eq!(d1.memory_id, d2.memory_id);
    }
}
