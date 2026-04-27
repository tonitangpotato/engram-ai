//! # Budget controller (`task:retr-impl-budget-cutoff`)
//!
//! Per-stage [`Duration`] caps + cost caps + cutoff behavior, per design §7.
//!
//! This module owns the *contract* between a plan executor and its time/cost
//! limits. Concrete numbers live in `RetrievalConfig` (wired by
//! `v03-benchmarks`); this module commits to the **structure** (per §7.1) and
//! the **semantics** (per §7.2 / §7.3).
//!
//! ## Shape (§7.1)
//!
//! Every plan executes a sequence of [`Stage`]s. Each stage has a [`Duration`]
//! cap from [`StageBudget`]. The plan's total cap is the sum of its stages'
//! caps; the **outer-query cap** is checked separately and may be tighter.
//!
//! ```text
//! Factual     = entity_resolution + edge_traversal + memory_lookup + rerank + fusion
//! Temporal    = time_parse + time_bounded_recall + optional_graph_filter + rerank + fusion
//! Associative = seed_recall + entity_extract + edge_hop + scoring + fusion
//! Hybrid      = max(sub_plan_a, sub_plan_b) + rrf_fusion
//! ```
//!
//! ## Cutoff semantics (§7.2)
//!
//! On stage timeout the plan **never** returns an error from the budget alone.
//! Instead, [`BudgetController::should_cutoff`] flips to `true`, the plan
//! returns whatever partial results it has (possibly the empty vec), and the
//! [`PlanTrace`] records a `Downgrade::StageBudgetExceeded { stage, .. }`.
//!
//! Outer-query timeout is the *only* budget-driven hard error
//! ([`crate::retrieval::api::RetrievalError::Timeout`]); per-stage cutoffs
//! degrade gracefully.
//!
//! ## Cost caps (§7.3)
//!
//! Independent of wall-clock budgets, [`CostCaps`] bounds work per query:
//! anchors resolved, hops walked, edges visited, candidates passed to rerank,
//! associative seed/pool sizes, affective seed sizes, the affect-divergence
//! sample rate, and the τ_graph_filter threshold for episodic-with-graph
//! expansion. Hitting a cap is **not** an error — it is recorded in
//! [`CostCounters`] and surfaced in the trace via
//! `Downgrade::CostCapHit { cap, .. }`.
//!
//! ## What this module does NOT do
//!
//! - It does not call the clock for you on the hot path; you sample it via
//!   [`BudgetController::elapsed`] / [`BudgetController::should_cutoff`].
//! - It does not race futures or cancel I/O; cooperative cutoff only.
//! - It does not own the absolute number choices — those live in
//!   `RetrievalConfig` (built by `v03-benchmarks`).
//!
//! [`PlanTrace`]: crate::retrieval::api::PlanTrace
//! [`Duration`]: std::time::Duration

use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Stages (§7.1)
// ---------------------------------------------------------------------------

/// Per-stage identity for budget tracking. Mirrors the structure committed in
/// design §7.1; new stages may be added (non-exhaustive) as plans evolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Resolve query tokens → anchor entities (Factual step 1).
    EntityResolution,
    /// Walk 1-hop neighborhood of anchors (Factual step 2).
    EdgeTraversal,
    /// Fetch episodes / candidates from memory (Factual step 3, Temporal/Episodic step 2).
    MemoryLookup,
    /// LLM-rerank or score-rerank pass over candidates (Factual/Temporal step 4).
    Rerank,
    /// Final score combination + ordering (all plans).
    Fusion,
    /// Parse the time expression in a Temporal/Episodic query.
    TimeParse,
    /// Time-bounded vector/keyword recall.
    TimeBoundedRecall,
    /// Optional graph filter triggered by `τ_graph_filter` (§4.2 step 3).
    OptionalGraphFilter,
    /// Initial vector recall for Associative / Affective (§4.3, §4.5 step 2).
    SeedRecall,
    /// Extract entity signals from seed results.
    EntityExtract,
    /// Hop along edges from extracted entities to expand the candidate pool.
    EdgeHop,
    /// Score candidates (sub-score blending, affect weighting, etc.).
    Scoring,
    /// Reciprocal-rank fusion across sub-plans (Hybrid step 3).
    RrfFusion,
}

impl Stage {
    /// Stable lowercase tag suitable for metrics labels and trace JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::EntityResolution => "entity_resolution",
            Stage::EdgeTraversal => "edge_traversal",
            Stage::MemoryLookup => "memory_lookup",
            Stage::Rerank => "rerank",
            Stage::Fusion => "fusion",
            Stage::TimeParse => "time_parse",
            Stage::TimeBoundedRecall => "time_bounded_recall",
            Stage::OptionalGraphFilter => "optional_graph_filter",
            Stage::SeedRecall => "seed_recall",
            Stage::EntityExtract => "entity_extract",
            Stage::EdgeHop => "edge_hop",
            Stage::Scoring => "scoring",
            Stage::RrfFusion => "rrf_fusion",
        }
    }
}

// ---------------------------------------------------------------------------
// Stage budget (§7.1)
// ---------------------------------------------------------------------------

/// Per-stage [`Duration`] caps. A `None` cap means "no cap on this stage" —
/// the outer-query budget still applies.
///
/// Defaults are *generous* placeholders meant only to keep tests deterministic
/// without timing fragility. `RetrievalConfig` overrides every field with
/// numbers tuned by `v03-benchmarks`; do not consume these defaults in
/// production paths.
#[derive(Debug, Clone, Default)]
pub struct StageBudget {
    pub entity_resolution: Option<Duration>,
    pub edge_traversal: Option<Duration>,
    pub memory_lookup: Option<Duration>,
    pub rerank: Option<Duration>,
    pub fusion: Option<Duration>,
    pub time_parse: Option<Duration>,
    pub time_bounded_recall: Option<Duration>,
    pub optional_graph_filter: Option<Duration>,
    pub seed_recall: Option<Duration>,
    pub entity_extract: Option<Duration>,
    pub edge_hop: Option<Duration>,
    pub scoring: Option<Duration>,
    pub rrf_fusion: Option<Duration>,
}

impl StageBudget {
    /// Cap for the given stage, if configured.
    pub fn cap_for(&self, stage: Stage) -> Option<Duration> {
        match stage {
            Stage::EntityResolution => self.entity_resolution,
            Stage::EdgeTraversal => self.edge_traversal,
            Stage::MemoryLookup => self.memory_lookup,
            Stage::Rerank => self.rerank,
            Stage::Fusion => self.fusion,
            Stage::TimeParse => self.time_parse,
            Stage::TimeBoundedRecall => self.time_bounded_recall,
            Stage::OptionalGraphFilter => self.optional_graph_filter,
            Stage::SeedRecall => self.seed_recall,
            Stage::EntityExtract => self.entity_extract,
            Stage::EdgeHop => self.edge_hop,
            Stage::Scoring => self.scoring,
            Stage::RrfFusion => self.rrf_fusion,
        }
    }

    /// Sum of all configured caps. Stages with `None` contribute zero.
    pub fn total(&self) -> Duration {
        [
            self.entity_resolution,
            self.edge_traversal,
            self.memory_lookup,
            self.rerank,
            self.fusion,
            self.time_parse,
            self.time_bounded_recall,
            self.optional_graph_filter,
            self.seed_recall,
            self.entity_extract,
            self.edge_hop,
            self.scoring,
            self.rrf_fusion,
        ]
        .into_iter()
        .flatten()
        .sum()
    }
}

// ---------------------------------------------------------------------------
// Cost caps (§7.3)
// ---------------------------------------------------------------------------

/// Hard-limit cost caps per query. Defaults match design §7.3.
///
/// Hitting a cap is recorded (see [`CostCounters`]) but never raises an error
/// — it is a downgrade signal. `RetrievalConfig` may tighten or loosen these
/// per workload.
#[derive(Debug, Clone)]
pub struct CostCaps {
    /// Cap on entities resolved from query tokens (Factual).
    pub max_anchors: usize,
    /// Cap on edge-traversal depth from each anchor (v0.3 default = 1).
    pub max_hops: usize,
    /// Aggregate cap on edges visited across all anchors.
    pub max_edges_visited: usize,
    /// Cap on memories passed to rerank.
    pub max_candidates: usize,
    /// Associative seed recall size (§4.3).
    pub k_seed: usize,
    /// Associative candidate pool after edge-hop expansion (§4.3).
    pub k_pool: usize,
    /// Affective seed recall size (§4.5 step 2). Caller passes
    /// `requested_k`; controller computes `min(3 * requested_k, k_seed_affective_max)`.
    pub k_seed_affective_max: usize,
    /// Multiplier for affective seed: `k_seed_affective = mul * requested_k`.
    pub k_seed_affective_mul: usize,
    /// Sample rate for affect-divergence telemetry (§4.5 step 5). 0..=1.
    pub affect_divergence_sample_rate: f64,
    /// Score threshold above which an Episodic query triggers 1-hop graph
    /// expansion (§4.2 step 3).
    pub tau_graph_filter: f64,
}

impl Default for CostCaps {
    fn default() -> Self {
        // Numbers from design §7.3.
        Self {
            max_anchors: 5,
            max_hops: 1,
            max_edges_visited: 500,
            max_candidates: 1000,
            k_seed: 10,
            k_pool: 100,
            k_seed_affective_max: 60,
            k_seed_affective_mul: 3,
            affect_divergence_sample_rate: 0.01,
            tau_graph_filter: 0.3,
        }
    }
}

impl CostCaps {
    /// Affective seed size for a request of size `requested_k`, per §4.5 step 2:
    /// `min(mul * requested_k, k_seed_affective_max)`.
    pub fn k_seed_affective(&self, requested_k: usize) -> usize {
        self.k_seed_affective_mul
            .saturating_mul(requested_k)
            .min(self.k_seed_affective_max)
    }
}

/// Identifier for a cost cap that was hit. Stable, used as a metrics label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CostCap {
    MaxAnchors,
    MaxHops,
    MaxEdgesVisited,
    MaxCandidates,
    KSeed,
    KPool,
    KSeedAffective,
}

impl CostCap {
    pub fn as_str(self) -> &'static str {
        match self {
            CostCap::MaxAnchors => "max_anchors",
            CostCap::MaxHops => "max_hops",
            CostCap::MaxEdgesVisited => "max_edges_visited",
            CostCap::MaxCandidates => "max_candidates",
            CostCap::KSeed => "k_seed",
            CostCap::KPool => "k_pool",
            CostCap::KSeedAffective => "k_seed_affective",
        }
    }
}

/// Running cost counters. The plan executor increments these as it works;
/// the controller reports cap hits via [`BudgetController::record_cost`] /
/// [`BudgetController::cost_caps_hit`]. Counters never decrement.
#[derive(Debug, Clone, Default)]
pub struct CostCounters {
    pub anchors: usize,
    pub max_hop_reached: usize,
    pub edges_visited: usize,
    pub candidates: usize,
    pub seed_recall_size: usize,
    pub pool_size: usize,
    pub affective_seed_size: usize,
}

// ---------------------------------------------------------------------------
// Budget controller
// ---------------------------------------------------------------------------

/// Per-query budget controller. Created once per `GraphQuery`, sampled
/// cooperatively by the plan executor at safe points.
///
/// Two budgets are tracked:
///
/// 1. **Per-stage cap** — checked via [`BudgetController::stage_should_cutoff`]
///    once a stage is entered. Triggers a graceful cutoff: partial results,
///    no error.
/// 2. **Outer-query cap** — checked via [`BudgetController::outer_should_cutoff`].
///    This is the only budget-driven hard error
///    ([`crate::retrieval::api::RetrievalError::Timeout`]).
///
/// The controller never spawns its own clock task. The hot path samples
/// `Instant::now()` at well-defined boundaries (entering / leaving a stage,
/// per K candidates processed, etc.).
#[derive(Debug)]
pub struct BudgetController {
    started_at: Instant,
    outer_cap: Option<Duration>,
    stages: StageBudget,
    cost_caps: CostCaps,
    counters: CostCounters,
    caps_hit: Vec<CostCap>,
    current_stage: Option<(Stage, Instant)>,
}

impl BudgetController {
    /// Construct a controller. `outer_cap = None` means no outer cap (still
    /// not recommended for production — set one).
    pub fn new(outer_cap: Option<Duration>, stages: StageBudget, cost_caps: CostCaps) -> Self {
        Self {
            started_at: Instant::now(),
            outer_cap,
            stages,
            cost_caps,
            counters: CostCounters::default(),
            caps_hit: Vec::new(),
            current_stage: None,
        }
    }

    /// Construct a controller with all defaults: no outer cap, no stage caps,
    /// design §7.3 cost caps. Useful for tests.
    pub fn with_defaults() -> Self {
        Self::new(None, StageBudget::default(), CostCaps::default())
    }

    /// Total time since the controller was created.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Configured outer cap (if any).
    pub fn outer_cap(&self) -> Option<Duration> {
        self.outer_cap
    }

    /// Borrow the cost caps in force.
    pub fn cost_caps(&self) -> &CostCaps {
        &self.cost_caps
    }

    /// Borrow the running cost counters.
    pub fn counters(&self) -> &CostCounters {
        &self.counters
    }

    /// Caps hit so far, in first-hit order. Each cap appears at most once.
    pub fn cost_caps_hit(&self) -> &[CostCap] {
        &self.caps_hit
    }

    // -- stage lifecycle ----------------------------------------------------

    /// Mark the start of a stage. The controller stamps the entry time so
    /// later [`BudgetController::stage_elapsed`] / [`BudgetController::stage_should_cutoff`]
    /// reports cover this stage only.
    ///
    /// Calling this twice without [`BudgetController::end_stage`] silently
    /// replaces the previous stage timestamp; plans that nest stages should
    /// not — design §7.1 enforces flat sequencing.
    pub fn begin_stage(&mut self, stage: Stage) {
        self.current_stage = Some((stage, Instant::now()));
    }

    /// End the current stage and return its elapsed [`Duration`]. Returns
    /// `None` if no stage was active.
    pub fn end_stage(&mut self) -> Option<(Stage, Duration)> {
        self.current_stage
            .take()
            .map(|(s, t)| (s, t.elapsed()))
    }

    /// Currently active stage (if any) and its elapsed time so far.
    pub fn stage_elapsed(&self) -> Option<(Stage, Duration)> {
        self.current_stage.map(|(s, t)| (s, t.elapsed()))
    }

    // -- cutoff checks ------------------------------------------------------

    /// `true` iff the outer-query cap has been exceeded.
    pub fn outer_should_cutoff(&self) -> bool {
        match self.outer_cap {
            Some(cap) => self.elapsed() >= cap,
            None => false,
        }
    }

    /// `true` iff the current stage's cap has been exceeded. Returns `false`
    /// if no stage is active or the stage has no cap configured.
    ///
    /// Per §7.2 a stage cutoff is **never** an error — the executor must
    /// surface partial results and record a `StageBudgetExceeded` downgrade.
    pub fn stage_should_cutoff(&self) -> bool {
        let Some((stage, started)) = self.current_stage else {
            return false;
        };
        let Some(cap) = self.stages.cap_for(stage) else {
            return false;
        };
        started.elapsed() >= cap
    }

    /// Combined check: outer cap exceeded OR current stage cap exceeded.
    /// Convenience for hot loops that want a single boolean.
    pub fn should_cutoff(&self) -> bool {
        self.outer_should_cutoff() || self.stage_should_cutoff()
    }

    /// Time remaining until the outer cap, if one is set. Returns `None` for
    /// uncapped controllers and `Some(Duration::ZERO)` once the cap is hit.
    pub fn outer_remaining(&self) -> Option<Duration> {
        self.outer_cap
            .map(|cap| cap.saturating_sub(self.elapsed()))
    }

    // -- cost-cap accounting ------------------------------------------------

    /// Record progress on a cost dimension. Returns `true` iff this update
    /// crossed a cap (the cap is recorded in [`Self::cost_caps_hit`] the
    /// *first* time it is hit; subsequent crossings still return `true`).
    ///
    /// Call this *after* the work is done (e.g. after extending
    /// `counters.edges_visited` by N) so the controller can decide whether to
    /// keep going or cut the next batch.
    pub fn record_cost(&mut self, cap: CostCap, value: usize) -> bool {
        let (current, limit): (&mut usize, usize) = match cap {
            CostCap::MaxAnchors => (&mut self.counters.anchors, self.cost_caps.max_anchors),
            CostCap::MaxHops => (
                &mut self.counters.max_hop_reached,
                self.cost_caps.max_hops,
            ),
            CostCap::MaxEdgesVisited => (
                &mut self.counters.edges_visited,
                self.cost_caps.max_edges_visited,
            ),
            CostCap::MaxCandidates => (
                &mut self.counters.candidates,
                self.cost_caps.max_candidates,
            ),
            CostCap::KSeed => (&mut self.counters.seed_recall_size, self.cost_caps.k_seed),
            CostCap::KPool => (&mut self.counters.pool_size, self.cost_caps.k_pool),
            CostCap::KSeedAffective => (
                &mut self.counters.affective_seed_size,
                self.cost_caps.k_seed_affective_max,
            ),
        };
        *current = (*current).saturating_add(value);
        let hit = *current >= limit;
        if hit && !self.caps_hit.contains(&cap) {
            self.caps_hit.push(cap);
        }
        hit
    }

    /// Returns true iff the affect-divergence telemetry should run for this
    /// query, sampled with the configured rate (§4.5 step 5).
    ///
    /// Deterministic: takes a `roll` in `[0.0, 1.0)` provided by the caller
    /// (typically `rand::random::<f64>()`), so tests can pin the outcome.
    pub fn should_sample_affect_divergence(&self, roll: f64) -> bool {
        roll < self.cost_caps.affect_divergence_sample_rate
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn stage_str_is_stable() {
        // Metrics labels — must not silently change.
        assert_eq!(Stage::EntityResolution.as_str(), "entity_resolution");
        assert_eq!(Stage::EdgeTraversal.as_str(), "edge_traversal");
        assert_eq!(Stage::MemoryLookup.as_str(), "memory_lookup");
        assert_eq!(Stage::Rerank.as_str(), "rerank");
        assert_eq!(Stage::Fusion.as_str(), "fusion");
        assert_eq!(Stage::RrfFusion.as_str(), "rrf_fusion");
    }

    #[test]
    fn stage_budget_total_sums_configured_caps_only() {
        let b = StageBudget {
            entity_resolution: Some(Duration::from_millis(10)),
            memory_lookup: Some(Duration::from_millis(40)),
            fusion: Some(Duration::from_millis(5)),
            ..StageBudget::default()
        };
        assert_eq!(b.total(), Duration::from_millis(55));
    }

    #[test]
    fn cost_caps_default_matches_design_7_3() {
        let c = CostCaps::default();
        assert_eq!(c.max_anchors, 5);
        assert_eq!(c.max_hops, 1);
        assert_eq!(c.max_edges_visited, 500);
        assert_eq!(c.max_candidates, 1000);
        assert_eq!(c.k_seed, 10);
        assert_eq!(c.k_pool, 100);
        assert_eq!(c.k_seed_affective_max, 60);
        assert_eq!(c.k_seed_affective_mul, 3);
        assert!((c.affect_divergence_sample_rate - 0.01).abs() < f64::EPSILON);
        assert!((c.tau_graph_filter - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn k_seed_affective_caps_at_max() {
        let c = CostCaps::default();
        // 3 * 5 = 15 < 60
        assert_eq!(c.k_seed_affective(5), 15);
        // 3 * 25 = 75, capped at 60
        assert_eq!(c.k_seed_affective(25), 60);
        // 3 * 100 = 300, capped at 60
        assert_eq!(c.k_seed_affective(100), 60);
    }

    #[test]
    fn no_caps_means_no_cutoff() {
        let bc = BudgetController::with_defaults();
        sleep(Duration::from_millis(2));
        assert!(!bc.outer_should_cutoff());
        assert!(!bc.stage_should_cutoff());
        assert!(!bc.should_cutoff());
        assert!(bc.outer_remaining().is_none());
    }

    #[test]
    fn outer_cutoff_fires_when_cap_exceeded() {
        let bc = BudgetController::new(
            Some(Duration::from_millis(5)),
            StageBudget::default(),
            CostCaps::default(),
        );
        sleep(Duration::from_millis(15));
        assert!(bc.outer_should_cutoff());
        assert!(bc.should_cutoff());
        assert_eq!(bc.outer_remaining(), Some(Duration::ZERO));
    }

    #[test]
    fn stage_cutoff_fires_independently_of_outer_cap() {
        let stages = StageBudget {
            memory_lookup: Some(Duration::from_millis(5)),
            ..StageBudget::default()
        };
        let mut bc = BudgetController::new(None, stages, CostCaps::default());
        bc.begin_stage(Stage::MemoryLookup);
        sleep(Duration::from_millis(15));
        assert!(bc.stage_should_cutoff());
        assert!(!bc.outer_should_cutoff());
        let (stage, elapsed) = bc.end_stage().unwrap();
        assert_eq!(stage, Stage::MemoryLookup);
        assert!(elapsed >= Duration::from_millis(15));
    }

    #[test]
    fn stage_without_cap_does_not_cutoff() {
        let mut bc = BudgetController::with_defaults();
        bc.begin_stage(Stage::Fusion);
        sleep(Duration::from_millis(2));
        assert!(!bc.stage_should_cutoff());
    }

    #[test]
    fn end_stage_with_no_active_stage_returns_none() {
        let mut bc = BudgetController::with_defaults();
        assert!(bc.end_stage().is_none());
    }

    #[test]
    fn record_cost_signals_when_cap_reached() {
        let caps = CostCaps {
            max_anchors: 5,
            ..CostCaps::default()
        };
        let mut bc = BudgetController::new(None, StageBudget::default(), caps);
        assert!(!bc.record_cost(CostCap::MaxAnchors, 2));
        assert!(!bc.record_cost(CostCap::MaxAnchors, 2)); // 4 < 5
        assert!(bc.record_cost(CostCap::MaxAnchors, 1)); // 5 >= 5
        assert!(bc.cost_caps_hit().contains(&CostCap::MaxAnchors));
    }

    #[test]
    fn record_cost_records_each_cap_only_once() {
        let caps = CostCaps {
            max_edges_visited: 10,
            ..CostCaps::default()
        };
        let mut bc = BudgetController::new(None, StageBudget::default(), caps);
        assert!(bc.record_cost(CostCap::MaxEdgesVisited, 100));
        assert!(bc.record_cost(CostCap::MaxEdgesVisited, 100));
        assert_eq!(bc.cost_caps_hit().len(), 1);
        assert_eq!(bc.cost_caps_hit()[0], CostCap::MaxEdgesVisited);
    }

    #[test]
    fn affect_divergence_sampling_respects_rate() {
        let bc = BudgetController::with_defaults(); // rate = 0.01
        assert!(bc.should_sample_affect_divergence(0.0));
        assert!(bc.should_sample_affect_divergence(0.005));
        assert!(!bc.should_sample_affect_divergence(0.01));
        assert!(!bc.should_sample_affect_divergence(0.5));
    }

    #[test]
    fn cutoff_is_partial_results_signal_not_error_path() {
        // Documents the §7.2 contract via behavior: should_cutoff returns
        // bool, not Result. Plan executors translate this into
        // `Ok(partial)`, never `Err(_)`.
        let mut bc = BudgetController::new(
            Some(Duration::from_millis(1)),
            StageBudget::default(),
            CostCaps::default(),
        );
        sleep(Duration::from_millis(5));
        let cut: bool = bc.should_cutoff();
        assert!(cut);
        // Also: the stage_elapsed introspection works even after cutoff.
        bc.begin_stage(Stage::Fusion);
        let (stage, _) = bc.stage_elapsed().unwrap();
        assert_eq!(stage, Stage::Fusion);
    }
}
