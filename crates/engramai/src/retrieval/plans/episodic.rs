//! # Episodic plan (`task:retr-impl-episodic`)
//!
//! Time-bounded source-memory retrieval. Implements design **§4.2**
//! (`.gid/features/v03-retrieval/design.md`) and shares the bi-temporal
//! projection helper with [`super::factual`] (design §4.6).
//!
//! ## What this plan does
//!
//! Given a query whose dominant signal is a **time window** (e.g. "what
//! happened yesterday", "the meeting last Tuesday", or an explicit
//! `as_of`), return the *source memories* that fall inside that window —
//! optionally filtered by an entity set — with their bi-temporal edges
//! projected to `as_of`. Episodic answers are **source memories**, not
//! synthesized facts (contrast: Factual / Abstract-L5).
//!
//! ## Steps (mirrors design §4.2)
//!
//! 1. **Parse / accept time window** — use the heuristic
//!    [`TimeWindow`](crate::retrieval::api::TimeWindow) on
//!    [`GraphQuery::time_window`](crate::retrieval::api::GraphQuery::time_window),
//!    falling back to the classifier's parse. If the resolved window is
//!    `None` (no temporal anchor at all) the plan **downgrades** to
//!    Factual via [`EpisodicOutcome::DowngradedFromEpisodic`] —
//!    [`EpisodicPlan::execute`] returns no rows; the caller is expected
//!    to re-dispatch through the Factual plan.
//! 2. **Time-bounded recall** — call the [`EpisodicMemoryStore`] trait
//!    (plug-in, mirroring [`EntityResolver`](super::factual::EntityResolver))
//!    with the resolved `[start, end]` instants. The store returns raw
//!    [`MemoryId`]s; this plan does not re-rank — fusion / scoring is
//!    owned by the post-plan pipeline (design §5).
//! 3. **Optional entity filter** — if the request carries
//!    [`entity_filter`](crate::retrieval::api::GraphQuery::entity_filter),
//!    intersect the time-bounded set with memories that mention any of
//!    those entities.
//! 4. **Bi-temporal projection mode** — translate `(as_of,
//!    include_superseded, query_time)` into an
//!    [`AsOfMode`] via [`AsOfMode::from_query`] so a downstream
//!    [`super::bitemporal::project_edges`] call (wired in
//!    `task:retr-impl-pipeline`) honours GOAL-3.4 ("as-of-T" queries).
//!    The mode is surfaced on [`EpisodicPlanResult`] for the pipeline to
//!    consume; this plan does not source `Edge` rows itself (that lives
//!    in the dispatcher with `GraphRead` access).
//! 5. **Cutoff** — if the resolved window lies entirely outside the
//!    knowledge cutoff (`as_of` < earliest record, or window strictly
//!    in the future of `as_of`), surface [`EpisodicOutcome::Cutoff`]
//!    with no rows.
//!
//! ## Outcomes
//!
//! Mirrors [`super::factual::FactualOutcome`] but with episodic-specific
//! variants. Promoted to a public
//! [`crate::retrieval::api::RetrievalOutcome`] via
//! [`EpisodicOutcome::to_retrieval_outcome`].
//!
//! - [`EpisodicOutcome::Ok`] — non-empty result inside window.
//! - [`EpisodicOutcome::Empty`] — window valid, no memories matched.
//! - [`EpisodicOutcome::DowngradedFromEpisodic`] — no temporal anchor;
//!   caller should retry as Factual.
//! - [`EpisodicOutcome::Cutoff`] — window outside the knowledge cutoff.
//!
//! ## Status
//!
//! T0 skeleton: deterministic logic + outcome surface + plug-in trait.
//! End-to-end edge projection + fusion lives in
//! `task:retr-impl-pipeline`.

use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::retrieval::api::{EntityId, GraphQuery, RetrievalOutcome, TimeWindow};
use crate::retrieval::budget::{BudgetController, Stage};
use crate::store_api::MemoryId;

use super::bitemporal::AsOfMode;

// ---------------------------------------------------------------------------
// Plug-in trait — EpisodicMemoryStore
// ---------------------------------------------------------------------------

/// Resolved (absolute) time window — output of step 1.
///
/// `start <= end`, both inclusive. Produced by
/// [`EpisodicPlan::resolve_window`] from a [`TimeWindow`]. Open-ended
/// `Range` endpoints are clamped against `query_time` and the
/// [`KnowledgeCutoff`] to keep downstream stores' lives easy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl ResolvedWindow {
    /// True iff `instant` ∈ `[start, end]`.
    #[inline]
    pub fn contains(&self, instant: DateTime<Utc>) -> bool {
        instant >= self.start && instant <= self.end
    }
}

/// Plug-in for time-bounded memory recall. Concrete wiring lives behind a
/// future `task:retr-impl-episodic-store` (mirrors how
/// [`EntityResolver`](super::factual::EntityResolver) decouples
/// [`super::factual::FactualPlan`] from the real storage backend).
///
/// ## Contract
///
/// - Implementations MUST return only memory ids whose *valid time*
///   intersects `window` — bi-temporal filtering by *transaction time*
///   is a separate concern handled by the projection step (design §4.6).
/// - Implementations MAY truncate to `limit` for cost control; the plan
///   does not assume a stable order.
/// - Implementations SHOULD be cheap to call — the plan uses this on the
///   hot retrieval path inside the [`Stage::TimeBoundedRecall`] budget.
pub trait EpisodicMemoryStore {
    /// Return memory ids whose valid time intersects `window`.
    fn memories_in_window(
        &self,
        window: &ResolvedWindow,
        limit: usize,
    ) -> Vec<MemoryId>;

    /// Return memory ids that mention any of `entities`. Used for the
    /// optional entity filter in step 3. The default implementation
    /// returns `None` to signal "not supported"; the plan then skips
    /// filtering rather than producing an empty result by accident.
    fn memories_mentioning_entities(
        &self,
        _entities: &[EntityId],
        _limit: usize,
    ) -> Option<Vec<MemoryId>> {
        None
    }
}

/// Inert default — used in unit tests / when episodic backend is absent.
/// Always returns an empty vec; the plan surfaces
/// [`EpisodicOutcome::Empty`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEpisodicStore;

impl EpisodicMemoryStore for NullEpisodicStore {
    fn memories_in_window(
        &self,
        _window: &ResolvedWindow,
        _limit: usize,
    ) -> Vec<MemoryId> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Inputs / outputs
// ---------------------------------------------------------------------------

/// Inputs assembled by the dispatcher before invoking
/// [`EpisodicPlan::execute`].
///
/// Mirrors [`super::factual::FactualPlanInputs`] but carries the resolved
/// time window instead of an entity anchor. Built by the pipeline once
/// the classifier returns
/// [`Intent::Episodic`](crate::retrieval::classifier::Intent::Episodic).
pub struct EpisodicPlanInputs<'a> {
    /// The original query — surfaced for `as_of`, optional
    /// `entity_filter`, `query_time`, and `limit`.
    pub query: &'a GraphQuery,

    /// Effective time window. `None` ⇒ plan downgrades. Typically derived
    /// from `query.time_window` or the classifier's heuristic parse.
    pub time_window: Option<TimeWindow>,

    /// Per-stage cost controller. Plan never panics on exhaustion — it
    /// short-circuits with whatever it has so far.
    pub budget: BudgetController,
}

/// Knowledge cutoff — earliest instant for which the system has data.
/// Windows entirely before this surface [`EpisodicOutcome::Cutoff`].
///
/// Wired in from the request envelope or
/// [`crate::cognitive_state::CognitiveState`]; the T0 skeleton keeps it
/// as a free parameter on [`EpisodicPlan`].
#[derive(Debug, Clone, Copy)]
pub struct KnowledgeCutoff {
    pub earliest: DateTime<Utc>,
}

impl Default for KnowledgeCutoff {
    fn default() -> Self {
        // Unix epoch — i.e. "no effective cutoff". Concrete wiring will
        // replace this with the per-store earliest insert.
        Self {
            earliest: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }
}

/// Plan output. Memories are returned **unscored** — fusion is owned by
/// [`crate::retrieval::fusion`]. The bi-temporal projection step is
/// represented by [`EpisodicPlanResult::projection_mode`]; the dispatcher
/// applies it to graph edges sourced via `GraphRead` in the surrounding
/// pipeline.
#[derive(Debug, Clone)]
pub struct EpisodicPlanResult {
    /// Memory ids inside the window (after optional entity intersection).
    pub memories: Vec<MemoryId>,

    /// The window actually used after resolution. `None` when the plan
    /// downgraded.
    pub window: Option<ResolvedWindow>,

    /// Bi-temporal projection mode (step 4) — `None` when the plan
    /// downgraded or hit the cutoff before reaching step 4.
    pub projection_mode: Option<AsOfMode>,

    /// Outcome surface. Promoted via
    /// [`EpisodicOutcome::to_retrieval_outcome`].
    pub outcome: EpisodicOutcome,

    /// Wall-clock latency of the whole `execute` call.
    pub elapsed: std::time::Duration,
}

/// Plan-local outcome — see module-level docs for variant semantics.
///
/// `Empty` is **not** an error: the plan ran cleanly, the window was
/// valid, the answer set is just empty. `DowngradedFromEpisodic` and
/// `Cutoff` *are* meaningful signals the caller must act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodicOutcome {
    /// Non-empty result inside a valid window.
    Ok,
    /// Window valid, no memories matched.
    Empty,
    /// No temporal anchor — caller should re-dispatch as Factual.
    DowngradedFromEpisodic,
    /// Window lies outside the knowledge cutoff (or strictly in the future).
    Cutoff,
}

impl EpisodicOutcome {
    /// Promote to the public [`RetrievalOutcome`] surface.
    ///
    /// `results_empty` lets us distinguish `Ok(non-empty)` from a window
    /// that was valid but produced no rows; the latter still maps to
    /// [`RetrievalOutcome::Empty`] for the caller's convenience.
    pub fn to_retrieval_outcome(&self, results_empty: bool) -> RetrievalOutcome {
        match self {
            EpisodicOutcome::Ok if !results_empty => RetrievalOutcome::Ok,
            EpisodicOutcome::Ok
            | EpisodicOutcome::Empty
            | EpisodicOutcome::DowngradedFromEpisodic
            | EpisodicOutcome::Cutoff => RetrievalOutcome::Empty,
        }
    }
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// Episodic plan — orchestrates steps 1-5 above.
///
/// Generic over the [`EpisodicMemoryStore`] (time-bounded recall);
/// defaults to an inert no-op impl so unit tests can drive the plan
/// without real backends.
pub struct EpisodicPlan<S = NullEpisodicStore>
where
    S: EpisodicMemoryStore,
{
    pub store: S,
    pub cutoff: KnowledgeCutoff,
    /// Default span used for `Relative(d)` semantics: `[query_time - d,
    /// query_time]` (design §4.2 pins this — confirmed in this task).
    pub relative_span_anchor_at_now: bool,
}

impl Default for EpisodicPlan<NullEpisodicStore> {
    fn default() -> Self {
        Self {
            store: NullEpisodicStore,
            cutoff: KnowledgeCutoff::default(),
            relative_span_anchor_at_now: true,
        }
    }
}

impl<S> EpisodicPlan<S>
where
    S: EpisodicMemoryStore,
{
    /// Construct from explicit components.
    pub fn new(store: S, cutoff: KnowledgeCutoff) -> Self {
        Self {
            store,
            cutoff,
            relative_span_anchor_at_now: true,
        }
    }

    // ----- step 1 -------------------------------------------------------

    /// Resolve a [`TimeWindow`] into an absolute `[start, end]` pair.
    ///
    /// Returns `None` for [`TimeWindow::None`] — the caller treats this
    /// as a downgrade signal. `Range` open endpoints are clamped:
    /// missing `from` → `cutoff.earliest`, missing `to` → `now`.
    /// `Relative(d)` resolves to `[now - d, now]` (per design §4.2,
    /// pinned by this task: `Relative(d) = [now - d, now]`, not
    /// `[now - d, ∞)`).
    pub fn resolve_window(
        &self,
        window: &TimeWindow,
        now: DateTime<Utc>,
    ) -> Option<ResolvedWindow> {
        match window {
            TimeWindow::None => None,
            TimeWindow::At(instant) => Some(ResolvedWindow {
                start: *instant,
                end: *instant,
            }),
            TimeWindow::Range { from, to } => {
                let start = from.unwrap_or(self.cutoff.earliest);
                let end = to.unwrap_or(now);
                // Defensive: tolerate inverted ranges by swapping rather
                // than returning `None` — the caller already gated on
                // "has temporal anchor".
                let (a, b) = if start <= end {
                    (start, end)
                } else {
                    (end, start)
                };
                Some(ResolvedWindow { start: a, end: b })
            }
            TimeWindow::Relative(d) => {
                // Pin: `Relative(d)` = `[now - d, now]`. `TimeWindow`
                // carries `std::time::Duration`; convert to `chrono`.
                let span = chrono::Duration::from_std(*d)
                    .unwrap_or_else(|_| chrono::Duration::zero());
                Some(ResolvedWindow {
                    start: now - span,
                    end: now,
                })
            }
        }
    }

    // ----- step 5 (early gate) -----------------------------------------

    /// Cutoff check: window must intersect `[cutoff.earliest, +∞)`. A
    /// window strictly before the cutoff (or strictly in the future of
    /// `as_of`) is a [`EpisodicOutcome::Cutoff`].
    fn outside_cutoff(
        &self,
        window: &ResolvedWindow,
        as_of: Option<DateTime<Utc>>,
    ) -> bool {
        if window.end < self.cutoff.earliest {
            return true;
        }
        if let Some(t) = as_of {
            if window.start > t {
                return true;
            }
        }
        false
    }

    // ----- driver -------------------------------------------------------

    /// Execute the plan end-to-end. See module docs for step semantics.
    ///
    /// `now` is the reproducibility-pinned instant (design §5.4); callers
    /// pass `query.query_time.unwrap_or_else(Utc::now)`.
    pub fn execute(
        &self,
        mut inputs: EpisodicPlanInputs<'_>,
        now: DateTime<Utc>,
    ) -> EpisodicPlanResult {
        let started = Instant::now();

        // Step 1 — resolve window (or downgrade).
        inputs.budget.begin_stage(Stage::TimeParse);
        let window_input = inputs.time_window.clone().unwrap_or(TimeWindow::None);
        let resolved = self.resolve_window(&window_input, now);
        inputs.budget.end_stage();

        let Some(window) = resolved else {
            return EpisodicPlanResult {
                memories: Vec::new(),
                window: None,
                projection_mode: None,
                outcome: EpisodicOutcome::DowngradedFromEpisodic,
                elapsed: started.elapsed(),
            };
        };

        // Step 5 (early) — knowledge cutoff gate.
        if self.outside_cutoff(&window, inputs.query.as_of) {
            return EpisodicPlanResult {
                memories: Vec::new(),
                window: Some(window),
                projection_mode: None,
                outcome: EpisodicOutcome::Cutoff,
                elapsed: started.elapsed(),
            };
        }

        // Step 2 — time-bounded recall.
        inputs.budget.begin_stage(Stage::TimeBoundedRecall);
        let limit = inputs.query.limit.max(1);
        let mut memories = self.store.memories_in_window(&window, limit);
        inputs.budget.end_stage();

        // Step 3 — optional entity filter.
        if let Some(entities) = inputs.query.entity_filter.as_ref() {
            if !entities.is_empty() {
                inputs.budget.begin_stage(Stage::OptionalGraphFilter);
                if let Some(entity_set) =
                    self.store.memories_mentioning_entities(entities, limit)
                {
                    use std::collections::HashSet;
                    let keep: HashSet<&MemoryId> = entity_set.iter().collect();
                    memories.retain(|m| keep.contains(m));
                }
                inputs.budget.end_stage();
            }
        }

        // Step 4 — bi-temporal projection mode.
        inputs.budget.begin_stage(Stage::EdgeTraversal);
        let mode = AsOfMode::from_query(
            inputs.query.as_of,
            inputs.query.include_superseded,
            now,
        );
        inputs.budget.end_stage();

        let outcome = if memories.is_empty() {
            EpisodicOutcome::Empty
        } else {
            EpisodicOutcome::Ok
        };

        EpisodicPlanResult {
            memories,
            window: Some(window),
            projection_mode: Some(mode),
            outcome,
            elapsed: started.elapsed(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::budget::BudgetController;
    use chrono::TimeZone;

    fn query() -> GraphQuery {
        GraphQuery::new("what happened yesterday")
    }

    fn budget() -> BudgetController {
        BudgetController::with_defaults()
    }

    #[test]
    fn resolve_window_none_is_downgrade() {
        let plan = EpisodicPlan::default();
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        assert!(plan.resolve_window(&TimeWindow::None, now).is_none());
    }

    #[test]
    fn resolve_window_at_is_point() {
        let plan = EpisodicPlan::default();
        let t = Utc.with_ymd_and_hms(2026, 4, 27, 12, 0, 0).unwrap();
        let w = plan.resolve_window(&TimeWindow::At(t), t).unwrap();
        assert_eq!(w.start, t);
        assert_eq!(w.end, t);
        assert!(w.contains(t));
    }

    #[test]
    fn resolve_window_range_open_ended_clamps() {
        let plan = EpisodicPlan::default(); // earliest = epoch
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let w = plan
            .resolve_window(&TimeWindow::Range { from: None, to: None }, now)
            .unwrap();
        assert_eq!(w.end, now);
        assert!(w.start < w.end);
    }

    #[test]
    fn resolve_window_range_inverted_is_swapped() {
        let plan = EpisodicPlan::default();
        let a = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 4, 26, 0, 0, 0).unwrap();
        let w = plan
            .resolve_window(
                &TimeWindow::Range {
                    from: Some(a),
                    to: Some(b),
                },
                a,
            )
            .unwrap();
        assert!(w.start <= w.end);
    }

    #[test]
    fn resolve_window_relative_is_lookback() {
        let plan = EpisodicPlan::default();
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 12, 0, 0).unwrap();
        let w = plan
            .resolve_window(
                &TimeWindow::Relative(std::time::Duration::from_secs(24 * 3600)),
                now,
            )
            .unwrap();
        assert_eq!(w.end, now);
        assert_eq!(w.end - w.start, chrono::Duration::hours(24));
    }

    #[test]
    fn execute_no_window_downgrades() {
        let plan = EpisodicPlan::default();
        let q = query();
        let inputs = EpisodicPlanInputs {
            query: &q,
            time_window: None,
            budget: budget(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let out = plan.execute(inputs, now);
        assert_eq!(out.outcome, EpisodicOutcome::DowngradedFromEpisodic);
        assert!(out.memories.is_empty());
        assert!(out.window.is_none());
        assert!(out.projection_mode.is_none());
    }

    #[test]
    fn execute_window_outside_cutoff() {
        let cutoff = KnowledgeCutoff {
            earliest: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        };
        let plan = EpisodicPlan::new(NullEpisodicStore, cutoff);
        let q = query();
        let early_window = TimeWindow::Range {
            from: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2025, 1, 2, 0, 0, 0).unwrap()),
        };
        let inputs = EpisodicPlanInputs {
            query: &q,
            time_window: Some(early_window),
            budget: budget(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let out = plan.execute(inputs, now);
        assert_eq!(out.outcome, EpisodicOutcome::Cutoff);
        assert!(out.memories.is_empty());
        assert!(out.window.is_some());
        assert!(out.projection_mode.is_none());
    }

    #[test]
    fn execute_empty_when_store_has_nothing() {
        let plan = EpisodicPlan::default();
        let q = query();
        let window = TimeWindow::Range {
            from: Some(Utc.with_ymd_and_hms(2026, 4, 26, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap()),
        };
        let inputs = EpisodicPlanInputs {
            query: &q,
            time_window: Some(window),
            budget: budget(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let out = plan.execute(inputs, now);
        assert_eq!(out.outcome, EpisodicOutcome::Empty);
        assert!(out.memories.is_empty());
        assert!(out.window.is_some());
        assert!(out.projection_mode.is_some());
    }

    #[test]
    fn outcome_lift_to_retrieval_outcome() {
        // `RetrievalOutcome` does not derive PartialEq today (T12 owns
        // the full surface), so we pattern-match instead.
        assert!(matches!(
            EpisodicOutcome::Ok.to_retrieval_outcome(false),
            RetrievalOutcome::Ok
        ));
        assert!(matches!(
            EpisodicOutcome::Ok.to_retrieval_outcome(true),
            RetrievalOutcome::Empty
        ));
        assert!(matches!(
            EpisodicOutcome::Empty.to_retrieval_outcome(true),
            RetrievalOutcome::Empty
        ));
        assert!(matches!(
            EpisodicOutcome::DowngradedFromEpisodic.to_retrieval_outcome(true),
            RetrievalOutcome::Empty
        ));
        assert!(matches!(
            EpisodicOutcome::Cutoff.to_retrieval_outcome(true),
            RetrievalOutcome::Empty
        ));
    }

    /// Smoke-test the entity-filter branch: a store that returns one
    /// in-window memory but reports a *different* memory for the entity
    /// should produce an empty intersection.
    #[test]
    fn execute_entity_filter_intersects() {
        struct OneInWindow;
        impl EpisodicMemoryStore for OneInWindow {
            fn memories_in_window(
                &self,
                _w: &ResolvedWindow,
                _l: usize,
            ) -> Vec<MemoryId> {
                vec![MemoryId::from("mem-A")]
            }
            fn memories_mentioning_entities(
                &self,
                _e: &[EntityId],
                _l: usize,
            ) -> Option<Vec<MemoryId>> {
                Some(vec![MemoryId::from("mem-B")])
            }
        }

        let plan = EpisodicPlan::new(OneInWindow, KnowledgeCutoff::default());
        let q = GraphQuery {
            entity_filter: Some(vec![uuid::Uuid::nil()]),
            ..query()
        };
        let window = TimeWindow::Range {
            from: Some(Utc.with_ymd_and_hms(2026, 4, 26, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap()),
        };
        let inputs = EpisodicPlanInputs {
            query: &q,
            time_window: Some(window),
            budget: budget(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let out = plan.execute(inputs, now);
        assert!(out.memories.is_empty());
        assert_eq!(out.outcome, EpisodicOutcome::Empty);
    }

    /// Step-4 sanity: as_of=Some(t) → AsOfMode::At(t).
    #[test]
    fn execute_as_of_translates_mode() {
        let plan = EpisodicPlan::default();
        let t = Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap();
        let q = GraphQuery {
            as_of: Some(t),
            ..query()
        };
        let window = TimeWindow::Range {
            from: Some(Utc.with_ymd_and_hms(2026, 4, 26, 0, 0, 0).unwrap()),
            to: Some(Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap()),
        };
        let inputs = EpisodicPlanInputs {
            query: &q,
            time_window: Some(window),
            budget: budget(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap();
        let out = plan.execute(inputs, now);
        assert!(matches!(out.projection_mode, Some(AsOfMode::At(_))));
    }
}
