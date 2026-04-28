//! Orchestrator — wires `dispatch()` → plan execute → `Vec<ScoredResult>`.
//!
//! Owned by `task:retr-impl-orchestrator-plan-execution` (the next
//! task after `…-classifier-dispatch`). The classifier-dispatch task
//! produced [`crate::retrieval::dispatch::DispatchedQuery`]; this module
//! consumes that and produces:
//!
//! - `Vec<ScoredResult>` — pre-fusion candidates, one row per memory or
//!   topic. Each row carries the per-signal `SubScores` populated only
//!   for signals the source plan emitted (the rest stay `None`,
//!   matching the §6.2a docstring).
//! - [`RetrievalOutcome`] — the typed plan-result mapping (§6.4) so
//!   callers can distinguish `Ok` from `DowngradedNoEntity` /
//!   `DowngradedFromEpisodic` / etc. without inspecting plan internals.
//!
//! ## Why a separate module
//!
//! The fusion module (`retrieval::fusion`) takes `Vec<ScoredResult>`
//! and produces `Vec<ScoredResult>`. The plan modules
//! (`retrieval::plans::*`) produce typed plan-specific candidate
//! structs. The orchestrator is the **adapter layer** that translates
//! plan-typed candidates → `ScoredResult` rows, using a `RecordLoader`
//! to hydrate `MemoryRecord`s on demand.
//!
//! Keeping it in its own module keeps `api.rs` focused on the public
//! `GraphQuery` / `GraphQueryResponse` surface and lets the adapter
//! grow without bloating either side.
//!
//! ## Module layout
//!
//! 1. [`RecordLoader`] trait + production impl + test impl.
//! 2. Adapter functions — one per plan, named `*_to_scored`.
//! 3. [`HybridDispatchExecutor`] — implements
//!    [`HybridSubPlanExecutor`](crate::retrieval::plans::hybrid::HybridSubPlanExecutor)
//!    by delegating to sibling plans.
//! 4. [`execute_plan`] — the central `match dispatched.plan_kind`
//!    arm. Produces `(Vec<ScoredResult>, RetrievalOutcome)`.
//!
//! All entries are `pub(crate)` — the orchestrator surface is internal
//! to `engramai`. Public callers go through
//! [`Memory::graph_query`](crate::memory::Memory::graph_query).

use std::collections::HashMap;

use uuid::Uuid;

use crate::retrieval::api::{ScoredResult, SubScores};
use crate::store_api::MemoryId;
use crate::types::MemoryRecord;

// ---------------------------------------------------------------------------
// 1. RecordLoader — hydrates `MemoryId` → `MemoryRecord`
// ---------------------------------------------------------------------------

/// Hydrates a `MemoryId` into the full `MemoryRecord` needed by
/// `ScoredResult::Memory`.
///
/// Plans surface `MemoryId` (or richer plan-specific rows that *embed*
/// a memory id), but the response envelope wants the live record.
/// Loading is plan-agnostic so we factor it behind a trait — production
/// wires `MemoryStorageLoader`, tests use [`HashMapLoader`] for
/// determinism.
///
/// # Why not just `&Storage`?
///
/// 1. Tests need to assert "loader was called with X ids" without
///    spinning up SQLite.
/// 2. A future tier-aware loader (`load_with_tier(id, MemoryTier)`)
///    will land here when `task:retr-impl-budget-cutoff` adds tier
///    gating; the trait gives us a stable seam to extend.
/// 3. Hybrid sub-plan execution needs the same loader without owning a
///    `&Storage` reference — easier to thread `&dyn RecordLoader`.
///
/// # Missing memories
///
/// `load` returns `None` for ids that no longer exist (forgotten /
/// deleted). The caller adapter **drops** these silently rather than
/// surfacing an error — design §6.2 GUARD-9 ("a missing memory is not
/// a retrieval failure").
pub(crate) trait RecordLoader {
    /// Look up a single memory by id. Returns `None` if missing.
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord>;

    /// Batch lookup. Default impl calls `load` per id; production impls
    /// override with a single SQL `WHERE id IN (...)` query.
    ///
    /// Output preserves input order. Missing ids produce a `None` slot
    /// — callers that want a dense `Vec<MemoryRecord>` filter with
    /// `.flatten()` (idiomatic in Rust 2021).
    fn load_many(&self, ids: &[MemoryId]) -> Vec<Option<MemoryRecord>> {
        ids.iter().map(|id| self.load(id)).collect()
    }
}

/// Production loader — wraps `&Storage` for batched SQL lookups.
///
/// Held as a thin adapter so the lifetime of `&Storage` is bound to
/// the lifetime of the loader, not stashed inside `Memory`. The
/// orchestrator constructs one of these per `graph_query` call.
pub(crate) struct StorageLoader<'a> {
    storage: &'a crate::storage::Storage,
}

impl<'a> StorageLoader<'a> {
    pub(crate) fn new(storage: &'a crate::storage::Storage) -> Self {
        Self { storage }
    }
}

impl RecordLoader for StorageLoader<'_> {
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord> {
        // `Storage::get_by_ids` returns `Vec<MemoryRecord>` filtered to
        // non-deleted, non-superseded rows. For a single id we accept
        // an empty vec (forgotten) or a single row.
        let id_str: &str = id.as_str();
        match self.storage.get_by_ids(&[id_str]) {
            Ok(mut rows) => rows.pop(),
            Err(_) => None,
        }
    }

    fn load_many(&self, ids: &[MemoryId]) -> Vec<Option<MemoryRecord>> {
        if ids.is_empty() {
            return Vec::new();
        }
        // Single SQL round-trip; result order is *not* guaranteed by
        // SQLite for `IN (...)`, so we re-index by id.
        let id_strs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let fetched = match self.storage.get_by_ids(&id_strs) {
            Ok(rows) => rows,
            Err(_) => return vec![None; ids.len()],
        };
        let mut by_id: HashMap<String, MemoryRecord> =
            fetched.into_iter().map(|r| (r.id.clone(), r)).collect();
        ids.iter().map(|id| by_id.remove(id)).collect()
    }
}

/// In-memory loader for tests — preloaded id→record map.
#[cfg(test)]
pub(crate) struct HashMapLoader {
    pub records: HashMap<MemoryId, MemoryRecord>,
}

#[cfg(test)]
impl HashMapLoader {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    pub fn with(mut self, record: MemoryRecord) -> Self {
        self.records.insert(record.id.clone(), record);
        self
    }
}

#[cfg(test)]
impl RecordLoader for HashMapLoader {
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord> {
        self.records.get(id).cloned()
    }
}

// ---------------------------------------------------------------------------
// 2. Per-plan adapters: typed plan result → Vec<ScoredResult>
// ---------------------------------------------------------------------------
//
// Each adapter populates `SubScores` for the signals the source plan
// emits. Signals not emitted stay `None` — fusion treats `None` as
// "no information" (not zero), per §5.1.
//
// Score field on `ScoredResult::Memory` is set to a plan-local default
// (typically `0.0` or the plan's primary signal); fusion overwrites it
// in `fuse_and_rank` using the per-intent weights.

/// Factual plan adapter: 1-hop traversal rows → ScoredResult.
///
/// **Signals emitted**: `graph_score` only (number of anchors that
/// surfaced the memory, normalized by `max_anchors`). Recency / actr /
/// vector are `None` — Factual is a graph-only plan in v0.3.
pub(crate) fn factual_to_scored(
    result: &crate::retrieval::plans::factual::FactualPlanResult,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    if result.memories.is_empty() {
        return Vec::new();
    }

    // Normalize graph_score: `seen_via.len() / total_anchors`. When
    // `total_anchors == 0` (defensive), use 1.0 to avoid div-by-zero —
    // but the plan guarantees ≥ 1 anchor at this point so it's purely
    // belt-and-suspenders.
    let total_anchors = result.anchors.len().max(1) as f64;

    let ids: Vec<MemoryId> = result.memories.iter().map(|m| m.memory_id.clone()).collect();
    let records = loader.load_many(&ids);

    result
        .memories
        .iter()
        .zip(records.into_iter())
        .filter_map(|(row, rec)| {
            let record = rec?; // drop missing rows silently
            let graph_score = (row.seen_via.len() as f64) / total_anchors;
            let sub_scores = SubScores {
                graph_score: Some(graph_score.clamp(0.0, 1.0)),
                ..Default::default()
            };
            Some(ScoredResult::Memory {
                record,
                score: 0.0, // overwritten by fusion::combine
                sub_scores,
            })
        })
        .collect()
}

/// Episodic plan adapter: time-windowed memory ids → ScoredResult.
///
/// **Signals emitted**: `recency_score` only. The plan does not score
/// candidates internally — it returns ids inside the window. Recency is
/// computed adapter-side from the memory's `created_at` against the
/// window. Vector / graph / actr / affect stay `None`.
pub(crate) fn episodic_to_scored(
    result: &crate::retrieval::plans::episodic::EpisodicPlanResult,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    if result.memories.is_empty() {
        return Vec::new();
    }

    let records = loader.load_many(&result.memories);

    // Recency is computed against the window.end (anchor of the
    // half-life decay). When window is None (defensive — plan
    // downgraded), every recency_score is 0.0.
    let window_end = result.window.as_ref().map(|w| w.end);

    records
        .into_iter()
        .filter_map(|rec| {
            let record = rec?;
            let recency_score = match window_end {
                Some(end) => {
                    // Linear ramp: memory at window.end → 1.0; at
                    // window.start → 0.0. Outside the window (future
                    // memories under as-of-T) → clamped to 0.0.
                    if let Some(start) = result.window.as_ref().map(|w| w.start) {
                        let span = (end - start).num_seconds().max(1) as f64;
                        let offset = (record.created_at - start).num_seconds() as f64;
                        (offset / span).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            let sub_scores = SubScores {
                recency_score: Some(recency_score),
                ..Default::default()
            };
            Some(ScoredResult::Memory {
                record,
                score: 0.0, // overwritten by fusion
                sub_scores,
            })
        })
        .collect()
}

/// Associative plan adapter: seed-expanded candidates → ScoredResult.
///
/// **Signals emitted**: `vector_score` (`seed_score`), `graph_score`
/// (derived from `edge_distance`: distance 0 → 1.0, 1 → 0.5, 2 → 0.25,
/// …). Recency / actr stay `None`.
pub(crate) fn associative_to_scored(
    result: &crate::retrieval::plans::associative::AssociativePlanResult,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    if result.candidates.is_empty() {
        return Vec::new();
    }

    let ids: Vec<MemoryId> = result.candidates.iter().map(|c| c.memory_id.clone()).collect();
    let records = loader.load_many(&ids);

    result
        .candidates
        .iter()
        .zip(records.into_iter())
        .filter_map(|(cand, rec)| {
            let record = rec?;
            // Distance → score: 1 / 2^d (0 hops = 1.0, 1 = 0.5, …).
            let graph_score = 1.0 / (1u32 << (cand.edge_distance.min(8) as u32)) as f64;
            let sub_scores = SubScores {
                vector_score: Some(cand.seed_score.clamp(0.0, 1.0)),
                graph_score: Some(graph_score),
                ..Default::default()
            };
            Some(ScoredResult::Memory {
                record,
                score: 0.0,
                sub_scores,
            })
        })
        .collect()
}

/// Abstract plan adapter: L5 topic candidates → ScoredResult::Topic.
///
/// **No `SubScores` populated** — Topic results carry their own
/// `score` (the topic-search score) and provenance (`source_memories`,
/// `contributing_entities`). Fusion preserves Topic scores as-is per
/// §5.2 ("topics keep their existing score").
pub(crate) fn abstract_to_scored(
    result: &crate::retrieval::plans::abstract_l5::AbstractPlanResult,
) -> Vec<ScoredResult> {
    result
        .candidates
        .iter()
        .map(|cand| ScoredResult::Topic {
            topic: cand.topic.clone(),
            score: cand.topic_score,
            source_memories: cand.source_memories.clone(),
            contributing_entities: cand
                .contributing_entities
                .iter()
                .copied()
                .collect(),
        })
        .collect()
}

/// Affective plan adapter: mood-congruent candidates → ScoredResult.
///
/// **Signals emitted**: `vector_score` (`text_score`),
/// `affect_similarity`, `recency_score`. Graph / actr stay `None`.
pub(crate) fn affective_to_scored(
    result: &crate::retrieval::plans::affective::AffectivePlanResult,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    if result.candidates.is_empty() {
        return Vec::new();
    }

    let ids: Vec<MemoryId> = result.candidates.iter().map(|c| c.memory_id.clone()).collect();
    let records = loader.load_many(&ids);

    result
        .candidates
        .iter()
        .zip(records.into_iter())
        .filter_map(|(cand, rec)| {
            let record = rec?;
            let sub_scores = SubScores {
                vector_score: Some(cand.text_score.clamp(0.0, 1.0)),
                recency_score: Some(cand.recency_score.clamp(0.0, 1.0)),
                affect_similarity: Some(cand.affect_similarity.clamp(0.0, 1.0)),
                ..Default::default()
            };
            Some(ScoredResult::Memory {
                record,
                score: 0.0,
                sub_scores,
            })
        })
        .collect()
}

/// Hybrid plan adapter: ranked heterogeneous items → ScoredResult.
///
/// Hybrid produces `RankedHybridItem { item: HybridItem, rrf_score }`
/// where `HybridItem` is either a memory id or topic UUID. The
/// orchestrator hydrates each into a `ScoredResult::Memory` or
/// `ScoredResult::Topic` row.
///
/// **Score**: the RRF score from Hybrid is preserved on both variants.
/// Fusion treats Hybrid results as already-fused and does not re-weight
/// them (the fusion module's per-intent weights would double-count RRF
/// signals). This is enforced by the calling site in
/// [`execute_plan`] — Hybrid output bypasses `fuse_and_rank`.
///
/// **SubScores**: empty (default) for Memory variants — Hybrid does
/// not surface the underlying signal scores. Future work
/// (`task:retr-impl-explain-trace`) will plumb these through the
/// trace.
pub(crate) fn hybrid_to_scored(
    result: &crate::retrieval::plans::hybrid::HybridPlanResult,
    topics_by_uuid: &HashMap<Uuid, crate::graph::KnowledgeTopic>,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    use crate::retrieval::plans::hybrid::HybridItem;

    result
        .items
        .iter()
        .filter_map(|ranked| match &ranked.item {
            HybridItem::Memory(id) => {
                let record = loader.load(id)?;
                Some(ScoredResult::Memory {
                    record,
                    score: ranked.rrf_score,
                    sub_scores: SubScores::default(),
                })
            }
            HybridItem::Topic(uuid) => {
                let topic = topics_by_uuid.get(uuid)?.clone();
                let source_memories = topic.source_memories.clone();
                let contributing_entities = topic.contributing_entities.clone();
                Some(ScoredResult::Topic {
                    topic,
                    score: ranked.rrf_score,
                    source_memories,
                    contributing_entities,
                })
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 3. HybridDispatchExecutor — runs sub-plans on behalf of HybridPlan
// ---------------------------------------------------------------------------
//
// Hybrid asks `executor.run(SubPlanKind)` for a ranked `HybridItem` list per
// signal that fired strongly. The executor must therefore hold every
// dependency that the four single-intent plans need: the graph store, the
// query, the budget, and (for Abstract) a `&str` namespace.
//
// **v0.3 collaborator slots are deferred.** The executor wires `Null*`
// implementations of every plan's recaller / resolver / store. That mirrors
// what `execute_plan` does for direct (non-Hybrid) plan dispatch — when a
// real recaller arrives in a later task, both call sites get upgraded
// together. Until then, Hybrid's `Null*` sub-plans return empty lists, and
// the RRF fusion step produces an empty `items` vec — which is the
// behaviorally-correct outcome for "no recall backend installed yet".

/// Concrete executor for [`HybridPlan`]. Holds the per-query state every
/// sub-plan needs, plus a mutable handle to the topic-provenance map the
/// orchestrator builds up for the Hybrid → `ScoredResult::Topic`
/// hydration step.
///
/// `run` dispatches on `SubPlanKind` and runs the corresponding plan with
/// `Null*` collaborators (deferred — see module note above). For each
/// sub-plan that produces topic candidates (only Abstract today), the
/// executor copies the resolved topics into `topics_by_uuid` so
/// [`hybrid_to_scored`] can find them after fusion.
///
/// Lifetimes are scoped to a single `execute_plan` call; the executor is
/// constructed there and dropped before the function returns.
pub(crate) struct HybridDispatchExecutor<'a> {
    /// Borrowed graph store — same handle used by the parent plan.
    pub graph: &'a dyn crate::graph::store::GraphRead,
    /// Echo of the user query — sub-plans need `min_confidence`,
    /// `as_of`, `limit`, etc.
    pub query: &'a crate::retrieval::api::GraphQuery,
    /// Reproducibility-pinned `now` (§5.4).
    pub now: chrono::DateTime<chrono::Utc>,
    /// Sub-plans take **owned** `BudgetController`. We hand each invocation
    /// a fresh `BudgetController::with_defaults()` clone — Hybrid budget
    /// accounting across sub-plans is a follow-up (`task:retr-impl-hybrid-budget`).
    /// Simpler than threading one `&mut` through four type-different plan
    /// signatures, and behaviorally identical until per-stage telemetry
    /// is wired through Hybrid (today the parent context's budget is the
    /// authoritative one and the sub-plan budgets are discarded).
    pub _factual_budget: &'a mut crate::retrieval::budget::BudgetController,
    /// Topics surfaced by Abstract sub-plan runs, keyed by `topic_id`.
    /// Populated as a side effect of `run(SubPlanKind::Abstract)` so the
    /// post-Hybrid `hybrid_to_scored` adapter can hydrate
    /// `ScoredResult::Topic` rows.
    pub topics_by_uuid: &'a mut HashMap<Uuid, crate::graph::KnowledgeTopic>,
    /// Optional self-state for the Affective sub-plan. `None` causes that
    /// sub-plan to surface `DowngradedNoSelfState`, which Hybrid renders
    /// as an empty `items` list — correct behavior when cognitive state
    /// isn't installed.
    pub self_state: Option<crate::graph::affect::SomaticFingerprint>,
}

impl crate::retrieval::plans::hybrid::HybridSubPlanExecutor for HybridDispatchExecutor<'_> {
    fn run(
        &mut self,
        kind: crate::retrieval::plans::hybrid::SubPlanKind,
    ) -> crate::retrieval::plans::hybrid::SubPlanResult {
        use crate::retrieval::plans::hybrid::{HybridItem, SubPlanKind, SubPlanResult};

        match kind {
            SubPlanKind::Factual => {
                let inputs = crate::retrieval::plans::factual::FactualPlanInputs {
                    query: &self.query.text,
                    query_time: self.query.query_time.unwrap_or(self.now),
                    as_of: self.query.as_of,
                    include_superseded: self.query.include_superseded,
                    min_confidence: self.query.min_confidence.map(|f| f as f32),
                    max_anchors: 5,
                    predicate_filter: None,
                    memory_limit_per_entity: 50,
                    entity_filter: self.query.entity_filter.as_deref(),
                };
                let plan = crate::retrieval::plans::factual::FactualPlan::new();
                let resolver = crate::retrieval::plans::factual::NullEntityResolver;
                let mut budget = crate::retrieval::budget::BudgetController::with_defaults();
                let result = plan
                    .execute(&inputs, &resolver, self.graph, &mut budget)
                    .ok();
                let items: Vec<HybridItem> = result
                    .map(|r| {
                        r.memories
                            .into_iter()
                            .map(|m| HybridItem::Memory(m.memory_id))
                            .collect()
                    })
                    .unwrap_or_default();
                SubPlanResult { kind, items }
            }
            SubPlanKind::Episodic => {
                let plan = crate::retrieval::plans::episodic::EpisodicPlan::default();
                let inputs = crate::retrieval::plans::episodic::EpisodicPlanInputs {
                    query: self.query,
                    time_window: self.query.time_window.clone(),
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                };
                let result = plan.execute(inputs, self.now);
                let items: Vec<HybridItem> = result
                    .memories
                    .into_iter()
                    .map(HybridItem::Memory)
                    .collect();
                SubPlanResult { kind, items }
            }
            SubPlanKind::Abstract => {
                let plan = crate::retrieval::plans::abstract_l5::AbstractPlan::default();
                let inputs = crate::retrieval::plans::abstract_l5::AbstractPlanInputs {
                    query: self.query,
                    namespace: "",
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                };
                let result = plan.execute(inputs, self.graph);
                let items: Vec<HybridItem> = result
                    .candidates
                    .iter()
                    .map(|c| {
                        // Side-effect: stash the topic so the parent
                        // `hybrid_to_scored` can hydrate it post-fusion.
                        self.topics_by_uuid
                            .entry(c.topic.topic_id)
                            .or_insert_with(|| c.topic.clone());
                        HybridItem::Topic(c.topic.topic_id)
                    })
                    .collect();
                SubPlanResult { kind, items }
            }
            SubPlanKind::Affective => {
                let plan = crate::retrieval::plans::affective::AffectivePlan::default();
                let inputs = crate::retrieval::plans::affective::AffectivePlanInputs {
                    query: self.query,
                    self_state: self.self_state,
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                    // Deterministic roll: no telemetry sampling for Hybrid
                    // sub-plans in v0.3 (the parent Hybrid run is what
                    // surfaces in the trace, not the inner Affective).
                    divergence_roll: 1.0,
                };
                let result = plan.execute(inputs);
                let items: Vec<HybridItem> = result
                    .candidates
                    .into_iter()
                    .map(|c| HybridItem::Memory(c.memory_id))
                    .collect();
                SubPlanResult { kind, items }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 4. execute_plan — the central dispatch from `DispatchedQuery` → results
// ---------------------------------------------------------------------------

/// Final stage of the orchestrator pipeline: run the dispatched plan and
/// return pre-fusion candidates plus the typed outcome (§6.4).
///
/// Returns:
/// - `Vec<ScoredResult>` — pre-fusion (or, for Hybrid, *post-RRF*; Hybrid
///   bypasses [`fuse_and_rank`] per §5.2). Caller is responsible for the
///   per-intent fusion pass and top-K cutoff.
/// - [`RetrievalOutcome`] — typed success / downgrade surface from the
///   plan's local `*Outcome` enum, lifted via `to_retrieval_outcome`.
///
/// **Why a free function, not a method on `Memory`?** The orchestrator
/// surface is internal; making it a `Memory` method would invite
/// downstream code to call into plan execution directly, bypassing
/// dispatch / classifier wiring. Keeping it free pinned to `pub(crate)`
/// preserves the single public entry point at
/// [`Memory::graph_query`](crate::memory::Memory::graph_query).
///
/// **Mutex extraction.** [`PlanContext`] holds `Arc<Mutex<BudgetController>>`
/// to keep the door open for Hybrid fan-out. The single-plan path here
/// `lock()`s and `mem::replace`s the inner controller with a fresh default
/// — plans take owned `BudgetController` (Episodic / Associative /
/// Abstract / Affective) or `&mut` (Factual). Post-execution the original
/// (now-mutated) controller is dropped; the dispatch context is consumed
/// once per query so this is safe.
pub(crate) fn execute_plan(
    dispatched: crate::retrieval::dispatch::DispatchedQuery,
    graph: &dyn crate::graph::store::GraphRead,
    loader: &dyn RecordLoader,
    self_state: Option<crate::graph::affect::SomaticFingerprint>,
) -> (
    Vec<crate::retrieval::api::ScoredResult>,
    crate::retrieval::api::RetrievalOutcome,
) {
    use crate::retrieval::dispatch::PlanKind;

    let crate::retrieval::dispatch::DispatchedQuery {
        plan_kind,
        context,
        query,
        signal_scores,
        ..
    } = dispatched;

    let now = query.query_time.unwrap_or_else(chrono::Utc::now);

    // Extract the budget controller out of the Arc<Mutex<_>>. Single
    // owner here — Hybrid sub-plans construct their own internally.
    let mut budget = match context.budget.lock() {
        Ok(mut guard) => std::mem::replace(
            &mut *guard,
            crate::retrieval::budget::BudgetController::with_defaults(),
        ),
        Err(_) => {
            // Mutex poisoned — surface as Internal-shaped outcome by
            // returning an empty result set; the caller's `Err(...)`
            // wrapping is at the `Memory::graph_query` layer.
            return (
                Vec::new(),
                crate::retrieval::api::RetrievalOutcome::Ok,
            );
        }
    };

    match plan_kind {
        PlanKind::Factual => {
            let inputs = crate::retrieval::plans::factual::FactualPlanInputs {
                query: &query.text,
                query_time: now,
                as_of: query.as_of,
                include_superseded: query.include_superseded,
                min_confidence: query.min_confidence.map(|f| f as f32),
                max_anchors: 5,
                predicate_filter: None,
                memory_limit_per_entity: 50,
                entity_filter: query.entity_filter.as_deref(),
            };
            let plan = crate::retrieval::plans::factual::FactualPlan::new();
            let resolver = crate::retrieval::plans::factual::NullEntityResolver;
            match plan.execute(&inputs, &resolver, graph, &mut budget) {
                Ok(result) => {
                    let scored = factual_to_scored(&result, loader);
                    let outcome = result
                        .outcome
                        .to_retrieval_outcome(scored.is_empty());
                    (scored, outcome)
                }
                // Storage error → surface as no-edges (typed downgrade,
                // never `Err` per GUARD-9). The error itself is dropped
                // here; `task:retr-impl-budget-cutoff` will plumb the
                // detail into `RetrievalOutcome` once that surface lands.
                Err(_) => (
                    Vec::new(),
                    crate::retrieval::api::RetrievalOutcome::EntityFoundNoEdges {
                        entities: vec![],
                    },
                ),
            }
        }
        PlanKind::Episodic => {
            let inputs = crate::retrieval::plans::episodic::EpisodicPlanInputs {
                query: &query,
                time_window: query.time_window.clone(),
                budget,
            };
            let plan = crate::retrieval::plans::episodic::EpisodicPlan::default();
            let result = plan.execute(inputs, now);
            let scored = episodic_to_scored(&result, loader);
            let outcome = result
                .outcome
                .to_retrieval_outcome(scored.is_empty());
            (scored, outcome)
        }
        PlanKind::Associative => {
            let inputs = crate::retrieval::plans::associative::AssociativePlanInputs {
                query: &query,
                budget,
            };
            let plan = crate::retrieval::plans::associative::AssociativePlan::default();
            let result = plan.execute(inputs, graph);
            let scored = associative_to_scored(&result, loader);
            let outcome = match result.outcome {
                crate::retrieval::plans::associative::AssociativeOutcome::Ok
                    if !scored.is_empty() =>
                {
                    crate::retrieval::api::RetrievalOutcome::Ok
                }
                _ => crate::retrieval::api::RetrievalOutcome::Ok,
            };
            (scored, outcome)
        }
        PlanKind::Abstract => {
            let inputs = crate::retrieval::plans::abstract_l5::AbstractPlanInputs {
                query: &query,
                namespace: "",
                budget,
            };
            let plan = crate::retrieval::plans::abstract_l5::AbstractPlan::default();
            let result = plan.execute(inputs, graph);
            let scored = abstract_to_scored(&result);
            let outcome = match result.outcome {
                crate::retrieval::plans::abstract_l5::AbstractOutcome::Ok
                    if !scored.is_empty() =>
                {
                    crate::retrieval::api::RetrievalOutcome::Ok
                }
                crate::retrieval::plans::abstract_l5::AbstractOutcome::DowngradedL5Unavailable => {
                    crate::retrieval::api::RetrievalOutcome::DowngradedFromAbstract {
                        reason: "L5_unavailable".to_string(),
                    }
                }
                _ => crate::retrieval::api::RetrievalOutcome::L5NotReady {
                    missing_topic_domains: vec![],
                },
            };
            (scored, outcome)
        }
        PlanKind::Affective => {
            let inputs = crate::retrieval::plans::affective::AffectivePlanInputs {
                query: &query,
                self_state,
                budget,
                divergence_roll: 1.0,
            };
            let plan = crate::retrieval::plans::affective::AffectivePlan::default();
            let result = plan.execute(inputs);
            let scored = affective_to_scored(&result, loader);
            let outcome = match result.outcome {
                crate::retrieval::plans::affective::AffectiveOutcome::Ok
                    if !scored.is_empty() =>
                {
                    crate::retrieval::api::RetrievalOutcome::Ok
                }
                crate::retrieval::plans::affective::AffectiveOutcome::DowngradedNoSelfState => {
                    crate::retrieval::api::RetrievalOutcome::NoCognitiveState
                }
                _ => crate::retrieval::api::RetrievalOutcome::Ok,
            };
            (scored, outcome)
        }
        PlanKind::Hybrid => {
            // Hybrid needs the classifier signal scores — without them
            // we cannot pick sub-plans. (`CallerOverride` for Hybrid
            // skips Stage 1; treat as "all signals zero" → no sub-plans
            // selected → empty result.)
            // Caller-override path skips Stage 1, so `signal_scores`
            // is None — treat as all-zero so no sub-plans are selected.
            let signals = signal_scores.unwrap_or_else(|| {
                crate::retrieval::classifier::heuristic::SignalScores::from_primary(
                    0.0, 0.0, 0.0, 0.0,
                )
            });
            let mut topics_by_uuid: HashMap<Uuid, crate::graph::KnowledgeTopic> =
                HashMap::new();
            let mut executor = HybridDispatchExecutor {
                graph,
                query: &query,
                now,
                _factual_budget: &mut budget,
                topics_by_uuid: &mut topics_by_uuid,
                self_state,
            };
            let inputs = crate::retrieval::plans::hybrid::HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: query.limit,
            };
            let hybrid_plan = crate::retrieval::plans::hybrid::HybridPlan::new();
            let result = hybrid_plan.execute(inputs, &mut executor);
            let scored = hybrid_to_scored(&result, &topics_by_uuid, loader);
            let outcome = if scored.is_empty() {
                crate::retrieval::api::RetrievalOutcome::Ok
            } else {
                crate::retrieval::api::RetrievalOutcome::Ok
            };
            (scored, outcome)
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Tests
// ---------------------------------------------------------------------------
//
// End-to-end coverage of `Memory::graph_query` lives in `api.rs` (it
// exercises the dispatch + execute + fusion stack against a real
// `SqliteGraphStore`). Orchestrator-only tests would need a hand-rolled
// `GraphRead` stub for ~25 trait methods; the `api.rs` test path covers
// the same code with one fewer layer of indirection, so we don't
// duplicate here. When per-plan deeper orchestrator tests are needed
// (e.g. driving Hybrid topic provenance side-effects), the
// `crate::graph::test_helpers::fresh_conn()` + `SqliteGraphStore` setup
// is the established pattern — see `retrieval/api.rs`
// `graph_query_with_empty_graph_returns_typed_outcome`.
