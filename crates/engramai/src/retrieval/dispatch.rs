//! # Orchestrator dispatch (`task:retr-impl-orchestrator-classifier-dispatch`)
//!
//! Stage A of the `Memory::graph_query` orchestrator pipeline:
//! turn a user-supplied [`GraphQuery`] into a [`DispatchedQuery`] — the
//! tuple of *(intent, plan kind, classifier provenance, signal scores,
//! plan context)* that downstream stages need.
//!
//! This module **does not execute plans**. Plan execution is the next
//! task (`task:retr-impl-orchestrator-plan-execution`); fusion + trace
//! assembly come after that. Splitting dispatch out keeps each follow-up
//! task small enough to test in isolation and lets the executor task
//! land without re-deriving classifier wiring decisions.
//!
//! ## What the dispatcher does
//!
//! 1. **Caller-override short-circuit (§3.3).** If `query.intent` is set,
//!    skip both classifier stages — `classifier_method = CallerOverride`,
//!    `signal_scores = None`.
//! 2. **Stage-1 heuristic classify (§3.2).** Run the heuristic classifier
//!    on `query.text`. If the outcome is `Decided`, that is the dispatch.
//! 3. **Stage-2 LLM fallback (§3.4) — deferred.** When stage 1 returns
//!    `NeedsLlmFallback`, the dispatcher today falls back to the
//!    heuristic best guess and reports `classifier_method = HeuristicTimeout`.
//!    The LLM client wiring (`task:retr-impl-classifier-llm-fallback`)
//!    will replace this branch by consulting the LLM under a budget cap;
//!    the surrounding orchestrator structure stays the same.
//!
//! ## Plan kind vs. intent
//!
//! Per design §3.1 there are exactly **5 intents** but the executable plan
//! lattice has **6 leaves** (the 5 intents + the `Associative` plan that is
//! materialized when `Intent::Factual` is paired with
//! `DowngradeHint::Associative`). [`PlanKind`] encodes the leaf actually
//! reached after applying any downgrade hint, so the executor can
//! dispatch on a single `match` without re-implementing the downgrade
//! rule. Crucially, [`DispatchedQuery::intent`] still records the original
//! 5-variant value so `PlanTrace` and metrics retain the §3.1 invariant.

use std::sync::Arc;

use crate::retrieval::api::{GraphQuery, MemoryTier};
use crate::retrieval::budget::{BudgetController, CostCaps, StageBudget};
use crate::retrieval::classifier::{
    heuristic::SignalScores, ClassifierMethod, DowngradeHint, HeuristicClassifier, Intent,
    Stage1Outcome,
};

// ---------------------------------------------------------------------------
// PlanKind — the 6-leaf executable plan lattice (§3.1 + §4.3)
// ---------------------------------------------------------------------------

/// Concrete plan to execute. Six leaves: the five [`Intent`] variants plus
/// `Associative` (materialized via the `Factual + Associative` downgrade
/// hint per §4.3).
///
/// `PlanKind` is the *executor's* dispatch key — `Memory::graph_query`'s
/// next stage matches on this enum to pick which plan implementation to
/// run. The original intent is still preserved separately on
/// [`DispatchedQuery`] for trace fidelity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanKind {
    /// Anchor-entity ranking (§4.1).
    Factual,
    /// Time-window-bounded source-memory recall (§4.2).
    Episodic,
    /// Vector-seed expansion when no strong signal fired (§4.3).
    Associative,
    /// L5 Knowledge-Topic synthesis (§4.4).
    Abstract,
    /// Mood-congruent recall (§4.5).
    Affective,
    /// Cross-layer fusion of ≥ 2 sub-plans (§4.7).
    Hybrid,
}

impl PlanKind {
    /// Stable string form for logs / metrics labels.
    pub fn as_str(self) -> &'static str {
        match self {
            PlanKind::Factual => "factual",
            PlanKind::Episodic => "episodic",
            PlanKind::Associative => "associative",
            PlanKind::Abstract => "abstract",
            PlanKind::Affective => "affective",
            PlanKind::Hybrid => "hybrid",
        }
    }

    /// Resolve `intent + downgrade hint` to the executable leaf.
    fn from_intent(intent: Intent, hint: DowngradeHint) -> PlanKind {
        match (intent, hint) {
            (Intent::Factual, DowngradeHint::Associative) => PlanKind::Associative,
            (Intent::Factual, DowngradeHint::None) => PlanKind::Factual,
            (Intent::Episodic, _) => PlanKind::Episodic,
            (Intent::Abstract, _) => PlanKind::Abstract,
            (Intent::Affective, _) => PlanKind::Affective,
            (Intent::Hybrid, _) => PlanKind::Hybrid,
        }
    }
}

// ---------------------------------------------------------------------------
// PlanContext — shared budget / cutoff / tier scoping for plan execution
// ---------------------------------------------------------------------------

/// Per-query execution context handed to the plan executor.
///
/// Bundles the dynamic state every plan needs but that is not part of the
/// plan-specific `*PlanInputs` type:
///
/// - `budget` — the per-stage [`BudgetController`] that all plans must
///   call into via `begin_stage` / `end_stage`. Owned by the dispatcher
///   (one controller per `graph_query` invocation) so cost caps and
///   outer-deadline behavior are uniform across plans.
/// - `tier` — optional tier scoping (§6.5 / GOAL-3.9). Mirrors
///   `GraphQuery::tier` so plans don't have to thread the original
///   query through.
/// - `limit` — top-K cutoff (`GraphQuery::limit`).
/// - `explain` — `true` iff `PlanTrace` assembly should be populated
///   (GOAL-3.11).
///
/// `PlanContext` is constructed by [`dispatch`] and consumed by the
/// plan-execution stage; both ends live in this crate so the type is
/// not part of the public retrieval surface.
#[derive(Debug)]
pub struct PlanContext {
    /// Per-stage budget controller. Boxed via `Arc` because plan
    /// executors that fan out to async sub-plans (Hybrid, §4.7) need a
    /// shared handle, but most plans hold a single owned reference.
    /// Today's usage is single-owner; the `Arc` keeps the door open
    /// without an API break when Hybrid lands.
    pub budget: Arc<std::sync::Mutex<BudgetController>>,
    /// Tier scoping (`GraphQuery::tier`). `None` = unrestricted.
    pub tier: Option<MemoryTier>,
    /// Top-K cutoff (`GraphQuery::limit`).
    pub limit: usize,
    /// Whether to populate [`crate::retrieval::api::GraphQueryResponse::trace`].
    pub explain: bool,
}

impl PlanContext {
    /// Build a context with [`BudgetController::with_defaults`]. Used by
    /// the dispatcher; tests that want explicit budgets construct
    /// `PlanContext` field-wise.
    fn from_query(query: &GraphQuery) -> Self {
        // Today the dispatcher uses the cross-query default budget. The
        // budget-controller task (`code:planned:budget-controller`) will
        // route per-query overrides through here.
        let outer_cap = None;
        let stages = StageBudget::default();
        let cost_caps = CostCaps::default();
        let bc = BudgetController::new(outer_cap, stages, cost_caps);
        Self {
            budget: Arc::new(std::sync::Mutex::new(bc)),
            tier: query.tier,
            limit: query.limit,
            explain: query.explain,
        }
    }
}

// ---------------------------------------------------------------------------
// DispatchedQuery — the dispatcher output handed to the executor
// ---------------------------------------------------------------------------

/// Output of [`dispatch`]: everything the plan executor needs to run a
/// single intent plan, plus the classifier provenance for trace fidelity.
///
/// The plan executor (next task) matches on `plan_kind` to pick the
/// implementation; `intent` is preserved verbatim for trace / metrics
/// (GOAL-3.2 — `classifier_method` MUST be observable, and the §3.1
/// "exactly 5 intents" invariant requires the executor to report
/// `plan_used = intent` even when `plan_kind = Associative`).
#[derive(Debug)]
pub struct DispatchedQuery {
    /// Original 5-variant intent. Used by trace and `plan_used` reporting
    /// (§3.1 invariant).
    pub intent: Intent,
    /// Concrete plan leaf to execute.
    pub plan_kind: PlanKind,
    /// Where the intent decision came from (heuristic / LLM fallback /
    /// caller override / heuristic-timeout).
    pub classifier_method: ClassifierMethod,
    /// Per-signal scores from Stage 1. `None` when
    /// `classifier_method = CallerOverride` (Stage 1 was skipped).
    pub signal_scores: Option<SignalScores>,
    /// Shared per-query execution context.
    pub context: PlanContext,
    /// Echo of the original query — the executor needs `text`, time
    /// window, entity filters, etc. Cloned (not borrowed) because the
    /// executor is `async` and may outlive the caller's stack frame.
    pub query: GraphQuery,
}

// ---------------------------------------------------------------------------
// dispatch — the entry point
// ---------------------------------------------------------------------------

/// Run the dispatcher for a single [`GraphQuery`].
///
/// Today this consults only the heuristic classifier. When the LLM
/// fallback task (`task:retr-impl-classifier-llm-fallback`) lands, the
/// `NeedsLlmFallback` branch will be replaced by an LLM consultation
/// under a budget cap; the rest of the function stays the same.
///
/// **Pure** — no IO, no async — so it is trivially testable from sync
/// contexts. The async wrapper at the call site (`Memory::graph_query`)
/// only exists because the *executor* will eventually be async.
pub fn dispatch(query: GraphQuery, classifier: &HeuristicClassifier) -> DispatchedQuery {
    // (1) §3.3 caller override short-circuit.
    if let Some(intent) = query.intent {
        let plan_kind = PlanKind::from_intent(intent, DowngradeHint::None);
        let context = PlanContext::from_query(&query);
        return DispatchedQuery {
            intent,
            plan_kind,
            classifier_method: ClassifierMethod::CallerOverride,
            signal_scores: None,
            context,
            query,
        };
    }

    // (2) Stage 1 heuristic classify.
    let (scores, outcome) = classifier.classify_stage1(&query.text);

    let (intent, hint, method) = match outcome {
        Stage1Outcome::Decided {
            intent,
            downgrade_hint,
        } => (intent, downgrade_hint, ClassifierMethod::Heuristic),

        // (3) Stage 2 deferred — fall back to heuristic best guess and
        // report HeuristicTimeout. The LLM-fallback task replaces this
        // branch with an LLM consultation under budget; the surrounding
        // structure (and PlanContext lifetime) stays identical.
        Stage1Outcome::NeedsLlmFallback {
            heuristic_best_guess,
            downgrade_hint,
        } => (
            heuristic_best_guess,
            downgrade_hint,
            ClassifierMethod::HeuristicTimeout,
        ),
    };

    let plan_kind = PlanKind::from_intent(intent, hint);
    let context = PlanContext::from_query(&query);

    log::debug!(
        target: "engramai::retrieval",
        "dispatch classified intent={:?} hint={:?} plan_kind={} method={:?} scores={:?}",
        intent,
        hint,
        plan_kind.as_str(),
        method,
        scores,
    );

    DispatchedQuery {
        intent,
        plan_kind,
        classifier_method: method,
        signal_scores: Some(scores),
        context,
        query,
    }
}

// ---------------------------------------------------------------------------
// Tests — assert each routing path produces the right (intent, plan_kind,
// classifier_method) triple. Plan execution is out of scope (next task).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::classifier::heuristic::{EntityLookup, NullEntityLookup};
    use crate::retrieval::classifier::SignalThresholds;
    use std::sync::Arc;

    /// Heuristic classifier with the null entity lookup — entity signal
    /// always 0.0. Sufficient for routing tests that drive the temporal /
    /// abstract / affective signals via query text.
    fn null_classifier() -> HeuristicClassifier {
        HeuristicClassifier::new(
            Arc::new(NullEntityLookup) as Arc<dyn EntityLookup>,
            SignalThresholds::default(),
        )
    }

    // -- Caller override (§3.3) ---------------------------------------------

    #[test]
    fn caller_override_short_circuits_classifier() {
        let q = GraphQuery::new("anything").with_intent(Intent::Hybrid);
        let d = dispatch(q, &null_classifier());

        assert_eq!(d.intent, Intent::Hybrid);
        assert_eq!(d.plan_kind, PlanKind::Hybrid);
        assert_eq!(d.classifier_method, ClassifierMethod::CallerOverride);
        assert!(
            d.signal_scores.is_none(),
            "caller override skips Stage 1 — no signal scores"
        );
    }

    #[test]
    fn caller_override_factual_picks_factual_plan_not_associative() {
        // Caller override does NOT pass through the Associative downgrade
        // — the user explicitly asked for Factual.
        let q = GraphQuery::new("").with_intent(Intent::Factual);
        let d = dispatch(q, &null_classifier());
        assert_eq!(d.plan_kind, PlanKind::Factual);
    }

    // -- No strong signal → Factual + Associative downgrade ----------------

    #[test]
    fn no_signals_routes_to_associative_plan() {
        // Empty/neutral text → no signals fire on null classifier.
        let q = GraphQuery::new("hmm");
        let d = dispatch(q, &null_classifier());

        // §3.1 invariant: intent stays Factual (one of the 5)…
        assert_eq!(d.intent, Intent::Factual);
        // …but the executable plan is Associative (the 6th leaf).
        assert_eq!(d.plan_kind, PlanKind::Associative);
        assert_eq!(d.classifier_method, ClassifierMethod::Heuristic);
        assert!(d.signal_scores.is_some());
    }

    // -- Single strong signal → single-intent plan -------------------------

    #[test]
    fn temporal_signal_routes_to_episodic() {
        // Pure temporal — query that fires only the temporal scorer
        // (matches the heuristic crate's own positive test fixture).
        let q = GraphQuery::new("what happened yesterday");
        let d = dispatch(q, &null_classifier());
        assert_eq!(d.intent, Intent::Episodic);
        assert_eq!(d.plan_kind, PlanKind::Episodic);
        assert_eq!(d.classifier_method, ClassifierMethod::Heuristic);
    }

    #[test]
    fn abstract_signal_routes_to_abstract_plan() {
        // Pure abstract — "summarize" trigger from heuristic test corpus.
        let q = GraphQuery::new("summarize our work on retrieval");
        let d = dispatch(q, &null_classifier());
        assert_eq!(d.intent, Intent::Abstract);
        assert_eq!(d.plan_kind, PlanKind::Abstract);
    }

    #[test]
    fn affective_signal_routes_to_affective_plan() {
        // Pure affective — "felt" trigger from heuristic test corpus.
        let q = GraphQuery::new("things I felt good about");
        let d = dispatch(q, &null_classifier());
        assert_eq!(d.intent, Intent::Affective);
        assert_eq!(d.plan_kind, PlanKind::Affective);
        assert_eq!(d.classifier_method, ClassifierMethod::Heuristic);
    }

    #[test]
    fn multi_signal_query_routes_to_hybrid() {
        // Temporal ("yesterday") + affective ("felt") → 2 strong
        // signals, each high-confidence → Hybrid plan per §3.2.
        let q = GraphQuery::new("what made me anxious yesterday");
        let d = dispatch(q, &null_classifier());
        assert_eq!(d.intent, Intent::Hybrid);
        assert_eq!(d.plan_kind, PlanKind::Hybrid);
        assert_eq!(d.classifier_method, ClassifierMethod::Heuristic);
    }

    // -- Stage-1 ambiguous → HeuristicTimeout (LLM deferred) --------------

    #[test]
    fn stage1_ambiguous_falls_back_to_heuristic_timeout() {
        // Force ambiguity by lowering tau_high so a single signal at
        // score 1.0 still doesn't clear the bar — that yields
        // NeedsLlmFallback in route_stage1, which the dispatcher must
        // collapse to ClassifierMethod::HeuristicTimeout.
        let high_bar = SignalThresholds {
            entity: 0.7,
            temporal: 1.0,
            abstract_: 1.0,
            affective: 1.0,
            // Set tau_high above the maximum possible binary signal so
            // strong-but-not-high-confidence triggers ambiguity.
            tau_high: 1.5,
        };
        let classifier = HeuristicClassifier::new(
            Arc::new(NullEntityLookup) as Arc<dyn EntityLookup>,
            high_bar,
        );
        let q = GraphQuery::new("yesterday I did something");
        let d = dispatch(q, &classifier);

        assert_eq!(d.classifier_method, ClassifierMethod::HeuristicTimeout);
        // best-guess is still produced (classifier is total per §3.4).
        assert!(d.signal_scores.is_some());
    }

    // -- PlanContext propagation --------------------------------------------

    #[test]
    fn plan_context_carries_query_options() {
        let q = GraphQuery::new("test")
            .with_limit(42)
            .with_tier(MemoryTier::Core)
            .with_explain(true);
        let d = dispatch(q, &null_classifier());

        assert_eq!(d.context.limit, 42);
        assert_eq!(d.context.tier, Some(MemoryTier::Core));
        assert!(d.context.explain);
    }

    #[test]
    fn plan_context_has_default_budget_controller() {
        let q = GraphQuery::new("test");
        let d = dispatch(q, &null_classifier());

        let bc = d.context.budget.lock().unwrap();
        // Default controller has no outer cap. (Elapsed is non-zero —
        // BudgetController::new starts the wall-clock immediately so
        // outer-deadline checks include construction-to-execute latency,
        // not just the executor's own time.)
        assert!(bc.outer_cap().is_none());
    }

    // -- Plan-kind derivation ----------------------------------------------

    #[test]
    fn plan_kind_from_intent_matrix() {
        use DowngradeHint::*;
        use Intent::*;
        // The 6-leaf lattice: 5 intents + Factual+Associative downgrade.
        assert_eq!(PlanKind::from_intent(Factual, None), PlanKind::Factual);
        assert_eq!(
            PlanKind::from_intent(Factual, Associative),
            PlanKind::Associative
        );
        assert_eq!(PlanKind::from_intent(Episodic, None), PlanKind::Episodic);
        assert_eq!(PlanKind::from_intent(Abstract, None), PlanKind::Abstract);
        assert_eq!(PlanKind::from_intent(Affective, None), PlanKind::Affective);
        assert_eq!(PlanKind::from_intent(Hybrid, None), PlanKind::Hybrid);
        // Downgrade hints on non-Factual intents are inert (defensive).
        assert_eq!(
            PlanKind::from_intent(Episodic, Associative),
            PlanKind::Episodic
        );
    }

    #[test]
    fn plan_kind_as_str_is_stable() {
        assert_eq!(PlanKind::Factual.as_str(), "factual");
        assert_eq!(PlanKind::Episodic.as_str(), "episodic");
        assert_eq!(PlanKind::Associative.as_str(), "associative");
        assert_eq!(PlanKind::Abstract.as_str(), "abstract");
        assert_eq!(PlanKind::Affective.as_str(), "affective");
        assert_eq!(PlanKind::Hybrid.as_str(), "hybrid");
    }
}
