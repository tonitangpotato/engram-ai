//! # Hybrid plan (`task:retr-impl-hybrid`)
//!
//! Multi-intent fusion path. Used when the classifier assigns
//! [`Intent::Hybrid`] (design §3.1, §4.7) — i.e. ≥ 2 strong signals
//! cross the τ_high bar. The plan:
//!
//! 1. **Selects** the (up to 2) strongest signals → maps each to its
//!    single-intent plan (Factual, Episodic, Abstract, Affective).
//! 2. **Executes** those sub-plans (sequentially in v0.3; tokio parallel
//!    is deferred — see design §4.7 *Concurrency note*).
//! 3. **Merges** their heterogeneous candidate sets ([`MemoryId`] from
//!    factual/episodic/affective, topic [`Uuid`] from abstract) into
//!    a single ranked list of [`HybridItem`]s. Hybrid only carries
//!    *identities* — payload hydration (full `Memory` / `KnowledgeTopic`
//!    rows) is the orchestrator's job (§6.4), so the plan stays cheap
//!    and decoupled from storage shape.
//! 4. **Fuses** scores with **Reciprocal Rank Fusion** (`1 / (k + rank)`,
//!    k = 60 by default) — RRF is scale-invariant so heterogeneous score
//!    distributions across plans don't need normalisation.
//! 5. **Truncates** to top-K and emits [`DroppedSignal`]s for any strong
//!    signals that were not run (≥ 3 strong → 2 dropped, etc.) so
//!    `PlanTrace.hybrid_truncated` is non-empty per GUARD-2 (§4.7).
//!
//! Hybrid does **not** rescore. Sub-plan scores are only used to *order*
//! within each list; RRF then operates on those ranks.
//!
//! ## Determinism
//!
//! Per design §6.7, all randomness is forbidden in retrieval. Tiebreaking:
//!
//! - Equal RRF score → smaller [`Uuid`] first (memories) /
//!   smaller topic UUID first (topics) / `Memory` before `Topic` if
//!   both kinds tie on UUID prefix.
//! - Sub-plan ranking inputs are assumed already deterministic (each
//!   sub-plan's contract).

use std::cmp::Ordering;
use std::collections::HashMap;

use uuid::Uuid;

use crate::retrieval::classifier::heuristic::{SignalKind, SignalScores};
use crate::store_api::MemoryId;

/// Default `k` parameter for the RRF formula `1 / (k + rank)`.
///
/// 60 is the canonical value from Cormack et al. (2009) — the same
/// constant `hybrid_search.rs::reciprocal_rank_fusion` uses internally
/// for the FTS+vector path. Keeping the same default lets sub-system
/// behaviour be reasoned about uniformly.
pub const DEFAULT_RRF_K: f64 = 60.0;

/// Maximum number of sub-plans Hybrid will run in v0.3.
///
/// Design §4.7: cap at 2 to keep latency bounded; surplus strong signals
/// are surfaced as [`DroppedSignal`] telemetry so GUARD-2 (never silent
/// degrade) is honoured.
pub const HYBRID_SUBPLAN_CAP: usize = 2;

// --------------------------------------------------------------------
// Sub-plan selection
// --------------------------------------------------------------------

/// Which single-intent plan a signal maps to.
///
/// This is the smaller, plan-side mirror of the classifier's `Intent`
/// enum — narrower because Hybrid never recurses into Hybrid, and
/// Associative is not a sub-plan of Hybrid (design §4.7 enumerates the
/// fusion set as Factual / Episodic / Abstract / Affective).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubPlanKind {
    Factual,
    Episodic,
    Abstract,
    Affective,
}

impl SubPlanKind {
    /// Map a [`SignalKind`] to its sub-plan (1:1 per design §3.1).
    pub fn from_signal(kind: SignalKind) -> Self {
        match kind {
            SignalKind::Entity => Self::Factual,
            SignalKind::Temporal => Self::Episodic,
            SignalKind::Abstract => Self::Abstract,
            SignalKind::Affective => Self::Affective,
        }
    }
}

/// Telemetry record for a strong signal that Hybrid chose **not** to
/// execute (because the cap was already reached).
///
/// Mirrors the spec in design §6.4 *Trace types*. Surfaces in
/// `PlanTrace.hybrid_truncated` so callers can see what was dropped and
/// — via the `score` — how strong it was. Never silent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DroppedSignal {
    pub kind: SignalKind,
    pub score: f64,
}

/// Pick at most [`HYBRID_SUBPLAN_CAP`] sub-plans from a [`SignalScores`]
/// snapshot using the high-water threshold `tau_high`.
///
/// Returns `(selected, dropped)`. `selected` is sorted by score descending
/// (highest first); ties broken by [`SignalKind`] declaration order
/// (`Entity < Temporal < Abstract < Affective`) so tests are deterministic.
///
/// `dropped` carries every strong signal that didn't make the cut, also
/// sorted by score descending. Empty when ≤ cap strong signals exist.
pub fn select_subplans(
    scores: &SignalScores,
    tau_high: f64,
) -> (Vec<(SubPlanKind, f64)>, Vec<DroppedSignal>) {
    let all = [
        (SignalKind::Entity, scores.entity),
        (SignalKind::Temporal, scores.temporal),
        (SignalKind::Abstract, scores.abstract_),
        (SignalKind::Affective, scores.affective),
    ];

    let mut strong: Vec<(SignalKind, f64)> = all
        .into_iter()
        .filter(|(_, s)| *s >= tau_high)
        .collect();

    // Highest score first; ties broken by SignalKind ordinal (stable).
    strong.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| signal_ord(a.0).cmp(&signal_ord(b.0)))
    });

    let (head, tail) = if strong.len() > HYBRID_SUBPLAN_CAP {
        let cut = strong.split_off(HYBRID_SUBPLAN_CAP);
        (strong, cut)
    } else {
        (strong, Vec::new())
    };

    let selected = head
        .into_iter()
        .map(|(k, s)| (SubPlanKind::from_signal(k), s))
        .collect();
    let dropped = tail
        .into_iter()
        .map(|(k, s)| DroppedSignal { kind: k, score: s })
        .collect();

    (selected, dropped)
}

fn signal_ord(k: SignalKind) -> u8 {
    match k {
        SignalKind::Entity => 0,
        SignalKind::Temporal => 1,
        SignalKind::Abstract => 2,
        SignalKind::Affective => 3,
    }
}

// --------------------------------------------------------------------
// Heterogeneous candidate model
// --------------------------------------------------------------------

/// A ranked candidate from a single sub-plan, before fusion.
///
/// Sub-plans return either memory hits (factual / episodic / affective)
/// or topic hits (abstract). [`HybridItem`] unifies them by their
/// stable identifiers — Hybrid does not need the full payload, only
/// the rank position and identity, since RRF is rank-based and the
/// orchestrator is responsible for hydrating final responses (§6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HybridItem {
    Memory(MemoryId),
    Topic(Uuid),
}

impl HybridItem {
    /// Stable identity used for cross-plan deduplication and tiebreaking.
    ///
    /// Returns `(kind_tag, key)` where `kind_tag = 0` for memories and
    /// `1` for topics. Memory ids and topic UUIDs live in disjoint
    /// namespaces in v0.3, but the kind tag keeps the equality contract
    /// explicit — two items are "the same candidate" only if they share
    /// kind *and* key.
    pub fn id(&self) -> HybridItemId {
        match self {
            HybridItem::Memory(m) => HybridItemId::Memory(m.clone()),
            HybridItem::Topic(t) => HybridItemId::Topic(*t),
        }
    }
}

/// Stable, hashable identity for a [`HybridItem`].
///
/// Ord is defined so memories sort before topics on ties (kind_tag 0 < 1),
/// and within a kind by ascending key — fully deterministic per §6.7.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HybridItemId {
    Memory(MemoryId),
    Topic(Uuid),
}

impl HybridItemId {
    fn kind_tag(&self) -> u8 {
        match self {
            HybridItemId::Memory(_) => 0,
            HybridItemId::Topic(_) => 1,
        }
    }
}

impl Ord for HybridItemId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kind_tag().cmp(&other.kind_tag()).then_with(|| match (self, other) {
            (HybridItemId::Memory(a), HybridItemId::Memory(b)) => a.cmp(b),
            (HybridItemId::Topic(a), HybridItemId::Topic(b)) => a.cmp(b),
            _ => Ordering::Equal,
        })
    }
}

impl PartialOrd for HybridItemId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One sub-plan's contribution to the fusion: kind + ranked items.
///
/// Items are assumed already sorted by the sub-plan's own scoring
/// (rank 0 = best). Hybrid does not re-sort within a list.
#[derive(Debug, Clone)]
pub struct SubPlanResult {
    pub kind: SubPlanKind,
    /// Ranked candidates (rank 0 = highest-scoring per the sub-plan).
    pub items: Vec<HybridItem>,
}

// --------------------------------------------------------------------
// Sub-plan executor trait
// --------------------------------------------------------------------

/// Trait the orchestrator uses to actually *invoke* a sub-plan.
///
/// Hybrid does not own the sub-plans (they have widely different input
/// types). Instead it asks an executor to run a [`SubPlanKind`] and
/// return a ranked, heterogeneous candidate list. The orchestrator
/// (`graph_query_api` dispatcher) provides a real implementation that
/// holds the storage adaptors; tests use [`StubExecutor`].
///
/// Errors: in v0.3 a sub-plan failure degrades to "empty list + record
/// in trace" rather than aborting — but Hybrid leaves that policy to
/// the orchestrator and treats every result as authoritative here.
pub trait HybridSubPlanExecutor {
    fn run(&mut self, kind: SubPlanKind) -> SubPlanResult;
}

/// Test-only executor: returns pre-canned results per kind.
#[derive(Debug, Default, Clone)]
pub struct StubExecutor {
    pub canned: HashMap<SubPlanKind, Vec<HybridItem>>,
}

impl StubExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, kind: SubPlanKind, items: Vec<HybridItem>) -> Self {
        self.canned.insert(kind, items);
        self
    }
}

impl HybridSubPlanExecutor for StubExecutor {
    fn run(&mut self, kind: SubPlanKind) -> SubPlanResult {
        let items = self.canned.get(&kind).cloned().unwrap_or_default();
        SubPlanResult { kind, items }
    }
}

// --------------------------------------------------------------------
// RRF fusion
// --------------------------------------------------------------------

/// Reciprocal Rank Fusion over heterogeneous sub-plan outputs.
///
/// Formula (per design §4.7 and Cormack et al. 2009):
///
/// ```text
/// RRF(d) = Σ_{plan p where d ∈ p}   1 / (k + rank_p(d))
/// ```
///
/// where `rank_p(d)` is the 0-indexed position of `d` in plan `p`'s
/// ranked list. Items appearing in both lists accumulate.
///
/// Tiebreaking (deterministic per GOAL §6.7):
/// - Higher fused score first.
/// - Equal score → smaller `(kind_tag, uuid)` first.
fn fuse_rrf(plans: &[SubPlanResult], k: f64) -> Vec<(HybridItemId, f64, HybridItem)> {
    // id → (accumulated_score, item_clone)
    let mut acc: HashMap<HybridItemId, (f64, HybridItem)> = HashMap::new();

    for plan in plans {
        for (rank, item) in plan.items.iter().enumerate() {
            let id = item.id();
            let contrib = 1.0 / (k + rank as f64);
            acc.entry(id)
                .and_modify(|(s, _)| *s += contrib)
                .or_insert_with(|| (contrib, item.clone()));
        }
    }

    let mut fused: Vec<(HybridItemId, f64, HybridItem)> = acc
        .into_iter()
        .map(|(id, (score, item))| (id, score, item))
        .collect();

    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    fused
}

// --------------------------------------------------------------------
// Plan struct + execution
// --------------------------------------------------------------------

/// Inputs for a single Hybrid execution.
#[derive(Debug, Clone)]
pub struct HybridPlanInputs<'a> {
    /// Classifier signal scores from the originating
    /// [`crate::retrieval::api::GraphQuery`].
    pub signals: &'a SignalScores,
    /// τ_high threshold (§3.1) — typically pulled from
    /// `RetrievalConfig::strong_signal_threshold`.
    pub tau_high: f64,
    /// Top-K cutoff after fusion.
    pub top_k: usize,
}

/// Outcome of a Hybrid execution.
#[derive(Debug, Clone)]
pub struct HybridPlanResult {
    /// Sub-plans actually executed (in execution order).
    pub executed: Vec<SubPlanKind>,
    /// Strong signals that were dropped because the cap was reached.
    /// Surfaces as `PlanTrace.hybrid_truncated` per §4.7 / GUARD-2.
    pub dropped: Vec<DroppedSignal>,
    /// Fused and ranked candidates, top-K applied.
    pub items: Vec<RankedHybridItem>,
}

/// One row of the fused output.
#[derive(Debug, Clone)]
pub struct RankedHybridItem {
    pub item: HybridItem,
    pub rrf_score: f64,
}

/// Hybrid plan executor.
///
/// Stateless wrt. data — all inputs flow through [`HybridPlanInputs`].
/// The `k` parameter is the only internal config and defaults to
/// [`DEFAULT_RRF_K`].
#[derive(Debug, Clone)]
pub struct HybridPlan {
    rrf_k: f64,
}

impl Default for HybridPlan {
    fn default() -> Self {
        Self { rrf_k: DEFAULT_RRF_K }
    }
}

impl HybridPlan {
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the RRF `k` parameter.
    pub fn with_rrf_k(mut self, k: f64) -> Self {
        self.rrf_k = k;
        self
    }

    /// Execute the Hybrid plan against an external sub-plan executor.
    ///
    /// Sequential in v0.3 — see module docs.
    pub fn execute<E: HybridSubPlanExecutor>(
        &self,
        inputs: HybridPlanInputs<'_>,
        executor: &mut E,
    ) -> HybridPlanResult {
        let (selected, dropped) = select_subplans(inputs.signals, inputs.tau_high);

        let mut sub_results: Vec<SubPlanResult> = Vec::with_capacity(selected.len());
        let mut executed: Vec<SubPlanKind> = Vec::with_capacity(selected.len());
        for (kind, _score) in &selected {
            let res = executor.run(*kind);
            executed.push(*kind);
            sub_results.push(res);
        }

        let fused = fuse_rrf(&sub_results, self.rrf_k);

        let items: Vec<RankedHybridItem> = fused
            .into_iter()
            .take(inputs.top_k)
            .map(|(_id, score, item)| RankedHybridItem {
                item,
                rrf_score: score,
            })
            .collect();

        HybridPlanResult {
            executed,
            dropped,
            items,
        }
    }
}

// --------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    // ---- helpers ----

    fn mk_signals(entity: f64, temporal: f64, abstract_: f64, affective: f64) -> SignalScores {
        SignalScores::from_primary(entity, temporal, abstract_, affective)
    }

    fn mem(id: &str) -> HybridItem {
        HybridItem::Memory(id.to_string())
    }

    fn topic(n: u128) -> HybridItem {
        HybridItem::Topic(Uuid::from_u128(n))
    }

    // ---- select_subplans ----

    #[test]
    fn select_subplans_picks_top_two_above_threshold() {
        let scores = mk_signals(0.9, 0.85, 0.4, 0.95);
        let (selected, dropped) = select_subplans(&scores, 0.7);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].0, SubPlanKind::Affective);
        assert_eq!(selected[1].0, SubPlanKind::Factual);

        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].kind, SignalKind::Temporal);
        assert!((dropped[0].score - 0.85).abs() < 1e-9);
    }

    #[test]
    fn select_subplans_empty_when_nothing_strong() {
        let scores = mk_signals(0.1, 0.2, 0.0, 0.3);
        let (selected, dropped) = select_subplans(&scores, 0.7);
        assert!(selected.is_empty());
        assert!(dropped.is_empty());
    }

    #[test]
    fn select_subplans_one_strong_no_drops() {
        let scores = mk_signals(0.95, 0.1, 0.1, 0.1);
        let (selected, dropped) = select_subplans(&scores, 0.7);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0, SubPlanKind::Factual);
        assert!(dropped.is_empty());
    }

    #[test]
    fn select_subplans_tiebreak_by_signal_ord() {
        // All four equal at 0.8 → entity, temporal selected (lowest ordinals).
        let scores = mk_signals(0.8, 0.8, 0.8, 0.8);
        let (selected, dropped) = select_subplans(&scores, 0.7);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].0, SubPlanKind::Factual); // Entity
        assert_eq!(selected[1].0, SubPlanKind::Episodic); // Temporal
        assert_eq!(dropped.len(), 2);
        assert_eq!(dropped[0].kind, SignalKind::Abstract);
        assert_eq!(dropped[1].kind, SignalKind::Affective);
    }

    // ---- RRF fusion ----

    #[test]
    fn rrf_combines_overlap_higher_than_unique() {
        let plan_a = SubPlanResult {
            kind: SubPlanKind::Factual,
            items: vec![mem("shared"), mem("only_a")],
        };
        let plan_b = SubPlanResult {
            kind: SubPlanKind::Episodic,
            items: vec![mem("shared"), mem("only_b")],
        };

        let fused = fuse_rrf(&[plan_a, plan_b], DEFAULT_RRF_K);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].2, mem("shared"));
        assert!((fused[0].1 - 2.0 / 60.0).abs() < 1e-12);
        // Tied unique items at score 1/61 — sorted by HybridItemId ascending.
        assert!((fused[1].1 - 1.0 / 61.0).abs() < 1e-12);
        assert!((fused[2].1 - 1.0 / 61.0).abs() < 1e-12);
        assert!(fused[1].0 < fused[2].0);
        assert_eq!(fused[1].2, mem("only_a"));
        assert_eq!(fused[2].2, mem("only_b"));
    }

    #[test]
    fn rrf_heterogeneous_memory_and_topic() {
        let plan_a = SubPlanResult {
            kind: SubPlanKind::Factual,
            items: vec![mem("m1")],
        };
        let plan_b = SubPlanResult {
            kind: SubPlanKind::Abstract,
            items: vec![topic(20)],
        };
        let fused = fuse_rrf(&[plan_a, plan_b], DEFAULT_RRF_K);
        assert_eq!(fused.len(), 2);
        // Both rank 0 → equal score; deterministic order: kind_tag 0 (Memory) first.
        assert!(matches!(fused[0].2, HybridItem::Memory(_)));
        assert!(matches!(fused[1].2, HybridItem::Topic(_)));
    }

    #[test]
    fn rrf_empty_inputs_produce_empty_output() {
        let fused = fuse_rrf(&[], DEFAULT_RRF_K);
        assert!(fused.is_empty());
    }

    // ---- end-to-end execute ----

    #[test]
    fn execute_runs_selected_subplans_and_returns_top_k() {
        let stub = StubExecutor::new()
            .with(SubPlanKind::Factual, vec![mem("m1"), mem("m2")])
            .with(SubPlanKind::Affective, vec![mem("m1"), mem("m3")]);
        let mut exec = stub;

        let signals = mk_signals(0.9, 0.1, 0.1, 0.95);
        let plan = HybridPlan::new();
        let res = plan.execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 2,
            },
            &mut exec,
        );

        assert_eq!(res.executed.len(), 2);
        assert!(res.executed.contains(&SubPlanKind::Factual));
        assert!(res.executed.contains(&SubPlanKind::Affective));
        assert!(res.dropped.is_empty());
        assert_eq!(res.items.len(), 2);
        // m1 appears in both → highest fused score.
        assert_eq!(res.items[0].item, mem("m1"));
    }

    #[test]
    fn execute_emits_dropped_signal_when_three_strong() {
        let stub = StubExecutor::new()
            .with(SubPlanKind::Factual, vec![])
            .with(SubPlanKind::Episodic, vec![])
            .with(SubPlanKind::Affective, vec![]);
        let mut exec = stub;

        // entity=0.95, temporal=0.9, affective=0.92 — top 2 = entity, affective.
        let signals = mk_signals(0.95, 0.9, 0.1, 0.92);
        let plan = HybridPlan::new();
        let res = plan.execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 10,
            },
            &mut exec,
        );

        assert_eq!(res.executed.len(), 2);
        assert_eq!(res.dropped.len(), 1);
        assert_eq!(res.dropped[0].kind, SignalKind::Temporal);
    }

    #[test]
    fn execute_with_no_strong_signals_runs_nothing() {
        let mut exec = StubExecutor::new();
        let signals = mk_signals(0.1, 0.1, 0.1, 0.1);
        let plan = HybridPlan::new();
        let res = plan.execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 5,
            },
            &mut exec,
        );
        assert!(res.executed.is_empty());
        assert!(res.dropped.is_empty());
        assert!(res.items.is_empty());
    }

    #[test]
    fn execute_top_k_zero_returns_empty_but_records_dropped() {
        let stub = StubExecutor::new()
            .with(SubPlanKind::Factual, vec![mem("a")])
            .with(SubPlanKind::Episodic, vec![mem("b")])
            .with(SubPlanKind::Affective, vec![mem("c")]);
        let mut exec = stub;
        let signals = mk_signals(0.9, 0.85, 0.1, 0.92);
        let res = HybridPlan::new().execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 0,
            },
            &mut exec,
        );
        assert!(res.items.is_empty());
        assert_eq!(res.executed.len(), 2);
        assert_eq!(res.dropped.len(), 1);
    }

    #[test]
    fn execute_is_deterministic_across_runs() {
        let make_exec = || {
            StubExecutor::new()
                .with(SubPlanKind::Factual, vec![mem("a"), mem("b")])
                .with(SubPlanKind::Affective, vec![mem("b"), mem("a")])
        };
        let signals = mk_signals(0.9, 0.1, 0.1, 0.92);
        let plan = HybridPlan::new();
        let mut e1 = make_exec();
        let mut e2 = make_exec();
        let r1 = plan.execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 5,
            },
            &mut e1,
        );
        let r2 = plan.execute(
            HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: 5,
            },
            &mut e2,
        );
        let ids1: Vec<_> = r1.items.iter().map(|x| x.item.clone()).collect();
        let ids2: Vec<_> = r2.items.iter().map(|x| x.item.clone()).collect();
        assert_eq!(ids1, ids2);
    }
}
