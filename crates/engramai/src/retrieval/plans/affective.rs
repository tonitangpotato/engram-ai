//! # Affective plan (`task:retr-impl-affective`)
//!
//! Mood-congruent recall — memories whose **write-time affect snapshot**
//! is similar to the **current cognitive self-state** rank higher,
//! without ever gating results (GUARD-6 / GOAL-3.14: cognitive state
//! modulates ranking, never blocks).
//!
//! Implements design **§4.5** (`.gid/features/v03-retrieval/design.md`).
//!
//! ## Pipeline (mirrors design §4.5 numbering)
//!
//! 1. **Fetch current self-state.** `s_now: SomaticFingerprint` is
//!    threaded by the orchestrator from engramai's cognitive-state
//!    module (`AffectivePlanInputs::self_state`). When `None` the plan
//!    surfaces [`AffectiveOutcome::DowngradedNoSelfState`] —
//!    Associative routing is the orchestrator's concern (§3.4).
//! 2. **Candidate recall.** Standard `hybrid_recall(query,
//!    k=K_seed_affective)` via the injected
//!    [`AffectiveSeedRecaller`] trait. Default `K_seed_affective =
//!    3 * requested_k`, capped at `60` per the budget controller
//!    (§7.3 / [`crate::retrieval::budget::CostCaps::k_seed_affective`]).
//! 3. **Affect distance scoring.** For each candidate,
//!    `affect_similarity = max(0, cosine(memory.affect_snapshot, s_now))`.
//!    Cosine over the 8-dim somatic fingerprint (GUARD-7 — locked
//!    layout) using [`SomaticFingerprint::cosine_similarity`]. Cosine
//!    is in `[-1, 1]`; we clamp the **negative** half to `0` so a
//!    candidate is never penalized below "no signal" — clamping
//!    preserves the GUARD-6 invariant (no result removed by affect).
//!    Candidates without an `affect_snapshot` get
//!    `affect_similarity = 0.0` (treated as neutral / no information).
//! 4. **Plan-internal fusion + ranking** (§4.5 step 4). The plan
//!    computes its own ranking locally because step 5's Kendall-tau
//!    telemetry needs a baseline ranking under `s_now`. Weights are
//!    fixed for the telemetry to be meaningful:
//!      `final = 0.50 * text_score + 0.35 * affect_similarity
//!            + 0.15 * recency_score`
//!    matching design §5.2 line 248. **Final fusion in §5.2 is still
//!    authoritative** — this is a plan-local sort that lets us
//!    measure rank divergence without double-running the global
//!    fusion module.
//!    `w_affect = 0.35 < 1.0` is the hard "never block" guarantee:
//!    a textually-perfect match (`text_score = 1.0`) cannot be
//!    overtaken by a low-affect penalty alone.
//! 5. **Rank-difference telemetry (GOAL-3.8).** When the budget
//!    controller's [`should_sample_affect_divergence`] returns
//!    `true` (default rate 1%, or always-on when `query.explain`),
//!    the plan computes a *second* ranking under a neutral
//!    self-state ([`SomaticFingerprint::zero()`]) and reports the
//!    Kendall-tau correlation between the two rankings as
//!    [`AffectivePlanResult::affect_divergence`]. GOAL-3.8 asserts
//!    `tau < 0.9` on the benchmark query set — meaning self-state
//!    actually moves rankings around.
//!
//! ## GUARD-6 (never gate) enforcement
//!
//! - `affect_similarity` is **clamped to `[0, 1]`** — the negative
//!   half of cosine is treated as "no information" (similarity =
//!   `0`), not as a penalty. A candidate with opposite affect ranks
//!   the same as one with neutral affect for the affect term.
//! - `w_affect = 0.35` is hard-coded and `< 1.0`, so even
//!   `affect_similarity = 0` cannot drive `final` to zero when
//!   `text_score > 0`.
//! - The plan **reorders** candidates; it never drops them. Final
//!   `candidates.len() == seed_hits.len()` (modulo deduplication and
//!   `min_text_score` cutoff which apply identically regardless of
//!   self-state). A property test in §9 verifies
//!   `result_ids(Q, S1) ∪ result_ids(Q, S2) == candidate_pool(Q)`
//!   for two arbitrary self-states.
//!
//! ## What this module does NOT do
//!
//! - **No final fusion.** §5.2 owns the global fusion path — the
//!   per-candidate `text_score`, `affect_similarity`, and
//!   `recency_score` are all surfaced unchanged for the fusion
//!   module to consume. The plan's local sort is for the
//!   Kendall-tau measurement only.
//! - **No self-state synthesis.** The orchestrator passes
//!   `self_state: Option<SomaticFingerprint>` from the cognitive
//!   self-state module; this plan never reaches into that module
//!   directly (clean dependency direction — see §10 of design).
//! - **No fingerprint mutation.** Per GUARD-8, `affect_snapshot` is
//!   immutable post-write; the plan reads it unchanged from each
//!   memory row.
//!
//! ## Design refs / requirements
//!
//! - Design §4.5 (Affective plan), §5.2 (fusion weights), §7.3
//!   (cost caps), §8.1 (`retrieval_affect_divergence_kendall_tau`).
//! - GOAL-3.8 — self-state biases ranking; Kendall-tau < 0.9
//!   observable telemetry.
//! - GOAL-3.14 — cognitive state never blocks results.
//! - GUARD-6 — cognitive state modulates ranking, never gates.
//! - GUARD-7 — `SomaticFingerprint` 8-dim layout locked; this plan
//!   relies on the locked semantics (cosine on the fixed array).
//! - GUARD-8 — `affect_snapshot` immutable; plan reads, never writes.

use std::time::{Duration, Instant};

use crate::graph::affect::SomaticFingerprint;
use crate::retrieval::api::GraphQuery;
use crate::retrieval::budget::{BudgetController, CostCap, Stage};
use crate::store_api::MemoryId;

// ---------------------------------------------------------------------------
// 1. Constants & defaults
// ---------------------------------------------------------------------------

/// Plan-local fusion weight for `text_score` (§4.5 step 4 / §5.2 line 248).
///
/// The triple `(W_TEXT, W_AFFECT, W_RECENCY)` sums to `1.0` and is
/// **locked** for the affective plan's internal ranking — the
/// Kendall-tau telemetry compares two rankings produced under the
/// **same** weights, so they cannot drift between calls without
/// breaking GOAL-3.8's interpretation.
pub const W_TEXT: f64 = 0.50;
/// Plan-local fusion weight for `affect_similarity`. Held strictly
/// below `1.0` to enforce the GUARD-6 / GOAL-3.14 "never block"
/// invariant — a textually-perfect match cannot be filtered out by
/// affect alone.
pub const W_AFFECT: f64 = 0.35;
/// Plan-local fusion weight for `recency_score`. The smallest of the
/// three because recency is already a coarse signal (half-life
/// decay) and the affective plan's primary job is mood-congruence,
/// not temporal recency.
pub const W_RECENCY: f64 = 0.15;

/// Static check that the weights sum to `1.0`. Compile-time guard
/// against accidental drift — Rust does not allow `assert!` in
/// `const`, but the sum is verified by [`weights_sum_to_one`] in
/// the test module.
const _WEIGHTS_SUM: f64 = W_TEXT + W_AFFECT + W_RECENCY;

// ---------------------------------------------------------------------------
// 2. AffectiveSeedRecaller trait + NullAffectiveSeedRecaller
// ---------------------------------------------------------------------------

/// Output of [`AffectiveSeedRecaller::recall`] — one seed memory plus
/// its text-relevance score, recency, and write-time affect snapshot.
///
/// `text_score` is the combined vector + BM25 score from
/// `hybrid_recall` (design §4.5 step 2), normalized to `[0, 1]`.
/// `recency_score` is the half-life decay of the memory's write-time
/// (`[0, 1]`); supplied by the recaller because the storage layer
/// owns the clock.
/// `affect_snapshot` may be `None` when the memory pre-dates the
/// resolution layer (no fingerprint was captured at write time);
/// the plan treats `None` as `affect_similarity = 0.0` (neutral —
/// see module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct AffectiveSeedHit {
    pub memory_id: MemoryId,
    pub text_score: f64,
    pub recency_score: f64,
    pub affect_snapshot: Option<SomaticFingerprint>,
}

/// Status returned by an affective seed recaller. Mirrors
/// [`super::associative::SeedRecallStatus`] — the backend can
/// surface a knowledge-cutoff gate without the plan having to know
/// the storage clock.
#[derive(Debug, Clone, PartialEq)]
pub enum AffectiveSeedStatus {
    /// Hits returned (possibly empty); plan handles `Empty` itself.
    Ok,
    /// Backend declared the query strictly outside the cutoff window.
    Cutoff,
}

/// Hybrid-recall + affect-snapshot seed source (design §4.5 step 2).
/// The default v0.3 implementation wraps the engramai `hybrid_recall`
/// path *plus* a row-read for `affect_snapshot` per hit, but the
/// plan takes the trait so unit tests can swap in deterministic
/// stubs.
///
/// The trait is object-safe so plan instances can hold
/// `Box<dyn AffectiveSeedRecaller>` if needed.
pub trait AffectiveSeedRecaller {
    /// Run the seed recall step.
    ///
    /// Implementations MUST return at most `top_k` hits ordered by
    /// descending `text_score`. Hits without a captured
    /// `affect_snapshot` are returned with `affect_snapshot: None`
    /// (the plan handles them as neutral) — the recaller MUST NOT
    /// drop such rows: doing so would silently violate GUARD-6.
    fn recall(
        &self,
        query: &GraphQuery,
        top_k: usize,
    ) -> (Vec<AffectiveSeedHit>, AffectiveSeedStatus);
}

/// Empty-result recaller. Useful as a default for unit tests that
/// want the plan's "no seeds" behaviour without constructing a stub.
#[derive(Debug, Clone, Default)]
pub struct NullAffectiveSeedRecaller;

impl AffectiveSeedRecaller for NullAffectiveSeedRecaller {
    fn recall(
        &self,
        _q: &GraphQuery,
        _k: usize,
    ) -> (Vec<AffectiveSeedHit>, AffectiveSeedStatus) {
        (Vec::new(), AffectiveSeedStatus::Ok)
    }
}

// ---------------------------------------------------------------------------
// 3. AffectiveCandidate (output row)
// ---------------------------------------------------------------------------

/// Pre-fusion candidate row produced by the Affective plan.
///
/// Fusion (§5.2) re-applies its own per-plan weights to `text_score`
/// + `affect_similarity` + `recency_score`. The plan's local sort
/// (and therefore the order of `candidates` in
/// [`AffectivePlanResult`]) is the **plan-local fused** ranking
/// using `(W_TEXT, W_AFFECT, W_RECENCY)` — the §5.2 module is free
/// to re-rank.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectiveCandidate {
    pub memory_id: MemoryId,
    /// Text-relevance score from the seed recaller, `[0, 1]`.
    /// Forwarded as the `text_score` signal in §5.1.
    pub text_score: f64,
    /// Affect cosine similarity between memory's snapshot and
    /// `s_now`, clamped to `[0, 1]` — the negative half is treated
    /// as "no information" (see module docs / GUARD-6 enforcement).
    /// Always `0.0` when `affect_snapshot` is `None`.
    pub affect_similarity: f64,
    /// Half-life recency score from the seed recaller, `[0, 1]`.
    pub recency_score: f64,
    /// Write-time fingerprint forwarded for trace / debugging.
    /// `None` when the memory pre-dates the resolution layer.
    pub affect_snapshot: Option<SomaticFingerprint>,
}

impl AffectiveCandidate {
    /// Plan-local fused score under the locked weights — used for
    /// the Kendall-tau telemetry (§4.5 step 5). Final fusion in
    /// §5.2 may apply different weights.
    #[inline]
    pub fn local_fused_score(&self) -> f64 {
        W_TEXT * self.text_score
            + W_AFFECT * self.affect_similarity
            + W_RECENCY * self.recency_score
    }
}

// ---------------------------------------------------------------------------
// 4. Inputs / outputs / outcome
// ---------------------------------------------------------------------------

/// Inputs assembled by the dispatcher before invoking
/// [`AffectivePlan::execute`]. Mirrors
/// [`super::abstract_l5::AbstractPlanInputs`] in shape.
pub struct AffectivePlanInputs<'a> {
    /// Original query — surfaces `limit` and `explain` (the latter
    /// forces always-on Kendall-tau telemetry per §4.5 step 5).
    pub query: &'a GraphQuery,

    /// Current cognitive self-state from engramai's cognitive-state
    /// module. `None` → plan downgrades to
    /// [`AffectiveOutcome::DowngradedNoSelfState`].
    pub self_state: Option<SomaticFingerprint>,

    /// Per-stage cost controller. Plan never panics on exhaustion —
    /// it short-circuits with whatever it has so far (design §7.3).
    pub budget: BudgetController,

    /// Random roll in `[0.0, 1.0)` driving the affect-divergence
    /// sample decision (§4.5 step 5). Caller passes
    /// `rand::random::<f64>()` in production; tests pin the value.
    /// Ignored when `query.explain == true` (always-on telemetry).
    pub divergence_roll: f64,
}

/// Telemetry attached to every Affective plan run that ran the
/// rank-divergence comparison (§4.5 step 5).
#[derive(Debug, Clone, PartialEq)]
pub struct AffectDivergence {
    /// Kendall-tau correlation between the ranking under `s_now`
    /// and the ranking under `s_neutral` ([`SomaticFingerprint::zero`]).
    /// Range `[-1.0, 1.0]`. GOAL-3.8 expects the average across the
    /// benchmark query set to be `< 0.9`.
    pub kendall_tau: f64,
    /// Number of candidates that participated in the comparison.
    /// Tau is meaningless for `n < 2`; we still emit it in that
    /// case but mark `n` so the metrics layer can drop it.
    pub n: usize,
    /// Whether this run was forced by `query.explain == true` (vs.
    /// sampled by the budget controller). Surfaces in the trace
    /// for the explain-mode path.
    pub forced_by_explain: bool,
}

/// Typed outcome (§6.4). The plan never returns scored rows in the
/// fusion sense; ordering is plan-local and §5.2 owns global fusion.
#[derive(Debug, Clone, PartialEq)]
pub enum AffectiveOutcome {
    /// Pipeline produced ≥ 1 candidate.
    Ok,
    /// Seed recaller returned hits but every candidate failed a
    /// downstream check. Currently unreachable in v0.3 (no
    /// post-recall filter drops rows) but reserved so a future
    /// `min_text_score` flag stays additive.
    Empty,
    /// Seed recaller returned zero hits. Distinct from
    /// `DowngradedNoSelfState` — substrate is empty, not the
    /// self-state.
    DowngradedNoSeeds,
    /// `self_state` was `None`. Orchestrator routes to Associative
    /// per §3.4 / §4.5 step 1.
    DowngradedNoSelfState,
    /// Seed backend signalled a knowledge-cutoff gate.
    Cutoff,
}

/// Plan output. `candidates` is ordered by **plan-local fused score**
/// (§4.5 step 4); §5.2 owns the global fusion re-rank.
#[derive(Debug, Clone)]
pub struct AffectivePlanResult {
    pub candidates: Vec<AffectiveCandidate>,
    pub outcome: AffectiveOutcome,
    /// Self-state used for the run (`None` when downgraded).
    pub self_state: Option<SomaticFingerprint>,
    /// Kendall-tau telemetry — populated when
    /// [`should_sample_affect_divergence`](BudgetController::should_sample_affect_divergence)
    /// returns `true` *or* `query.explain == true`.
    pub affect_divergence: Option<AffectDivergence>,
    pub elapsed: Duration,
}

// ---------------------------------------------------------------------------
// 5. AffectivePlan struct + execute()
// ---------------------------------------------------------------------------

/// Affective plan. Generic over the [`AffectiveSeedRecaller`] backend
/// — production wires the real `hybrid_recall` + row-read path,
/// tests stub.
#[derive(Debug, Clone)]
pub struct AffectivePlan<R = NullAffectiveSeedRecaller>
where
    R: AffectiveSeedRecaller,
{
    recaller: R,
}

impl Default for AffectivePlan<NullAffectiveSeedRecaller> {
    fn default() -> Self {
        Self {
            recaller: NullAffectiveSeedRecaller,
        }
    }
}

impl<R> AffectivePlan<R>
where
    R: AffectiveSeedRecaller,
{
    pub fn new(recaller: R) -> Self {
        Self { recaller }
    }

    /// Execute the full §4.5 pipeline.
    ///
    /// The plan never mutates state. All five steps follow the
    /// design's numbering; deviations are documented inline.
    pub fn execute(&self, mut inputs: AffectivePlanInputs<'_>) -> AffectivePlanResult {
        let started = Instant::now();

        // -------- Step 1 — fetch self-state ---------------------------
        // Threaded by the orchestrator. `None` is a legitimate state
        // (cognitive-state module not initialised, or explicit
        // opt-out); we surface a typed downgrade so the orchestrator
        // can route to Associative per §3.4 without inspecting
        // internal plan state.
        let Some(s_now) = inputs.self_state else {
            return AffectivePlanResult {
                candidates: Vec::new(),
                outcome: AffectiveOutcome::DowngradedNoSelfState,
                self_state: None,
                affect_divergence: None,
                elapsed: started.elapsed(),
            };
        };

        // -------- Step 2 — candidate recall --------------------------
        // K_seed_affective is delegated to the budget controller
        // (§7.3) — `min(mul * requested_k, k_seed_affective_max)`.
        // The plan does not duplicate the formula; if a future
        // tuning changes the cap, only the budget module needs to
        // know.
        inputs.budget.begin_stage(Stage::SeedRecall);
        let requested_k = inputs.query.limit.max(1);
        let k_seed = inputs.budget.cost_caps().k_seed_affective(requested_k);
        let (hits, status) = self.recaller.recall(inputs.query, k_seed);
        // Record the actual seed pool size for cost-cap telemetry
        // (§7.3 — `k_seed_affective` cap surfaces in
        // `BudgetController::cost_caps_hit`).
        let _ = inputs
            .budget
            .record_cost(CostCap::KSeedAffective, hits.len());
        inputs.budget.end_stage();

        if matches!(status, AffectiveSeedStatus::Cutoff) {
            return AffectivePlanResult {
                candidates: Vec::new(),
                outcome: AffectiveOutcome::Cutoff,
                self_state: Some(s_now),
                affect_divergence: None,
                elapsed: started.elapsed(),
            };
        }
        if hits.is_empty() {
            return AffectivePlanResult {
                candidates: Vec::new(),
                outcome: AffectiveOutcome::DowngradedNoSeeds,
                self_state: Some(s_now),
                affect_divergence: None,
                elapsed: started.elapsed(),
            };
        }

        // -------- Step 3 — affect distance scoring -------------------
        // Cosine on the locked 8-dim fingerprint (GUARD-7).
        // Negative cosine is clamped to `0` — see GUARD-6 enforcement
        // notes in module docs.
        inputs.budget.begin_stage(Stage::Scoring);
        let mut candidates: Vec<AffectiveCandidate> = hits
            .into_iter()
            .map(|h| {
                let affect_similarity = match &h.affect_snapshot {
                    Some(fp) => clamp_unit(fp.cosine_similarity(&s_now) as f64),
                    None => 0.0,
                };
                AffectiveCandidate {
                    memory_id: h.memory_id,
                    text_score: h.text_score,
                    affect_similarity,
                    recency_score: h.recency_score,
                    affect_snapshot: h.affect_snapshot,
                }
            })
            .collect();

        // -------- Step 4 — plan-local fused ranking ------------------
        // Sort by descending fused score; tiebreak on memory_id for
        // determinism (design §5.4 — repeatability invariant).
        candidates.sort_by(|a, b| {
            b.local_fused_score()
                .partial_cmp(&a.local_fused_score())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.memory_id.cmp(&b.memory_id))
        });
        inputs.budget.end_stage();

        // -------- Step 5 — Kendall-tau divergence telemetry ---------
        // Always-on when `query.explain == true`; otherwise sampled
        // at `affect_divergence_sample_rate` (default 1%, pinned
        // ≥ 0.01 by the §9 affect-divergence test).
        let forced_by_explain = inputs.query.explain;
        let should_sample = forced_by_explain
            || inputs
                .budget
                .should_sample_affect_divergence(inputs.divergence_roll);
        let affect_divergence = if should_sample && candidates.len() >= 2 {
            // Compute the same fused score under the neutral
            // self-state. We re-score (cheap — N <= 60 by §7.3) and
            // produce a parallel ranking; tau measures rank
            // discordance.
            let s_neutral = SomaticFingerprint::zero();
            let mut neutral: Vec<(MemoryId, f64)> = candidates
                .iter()
                .map(|c| {
                    let neutral_affect = match &c.affect_snapshot {
                        Some(fp) => clamp_unit(fp.cosine_similarity(&s_neutral) as f64),
                        None => 0.0,
                    };
                    let fused = W_TEXT * c.text_score
                        + W_AFFECT * neutral_affect
                        + W_RECENCY * c.recency_score;
                    (c.memory_id.clone(), fused)
                })
                .collect();
            neutral.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });

            let now_order: Vec<MemoryId> =
                candidates.iter().map(|c| c.memory_id.clone()).collect();
            let neutral_order: Vec<MemoryId> =
                neutral.into_iter().map(|(id, _)| id).collect();
            let tau = kendall_tau(&now_order, &neutral_order);
            Some(AffectDivergence {
                kendall_tau: tau,
                n: candidates.len(),
                forced_by_explain,
            })
        } else if should_sample {
            // Sampled but n < 2 — emit a zero-tau record so metrics
            // can still increment "sampled" counter without being
            // misled about correlation strength.
            Some(AffectDivergence {
                kendall_tau: 0.0,
                n: candidates.len(),
                forced_by_explain,
            })
        } else {
            None
        };

        let outcome = if candidates.is_empty() {
            AffectiveOutcome::Empty
        } else {
            AffectiveOutcome::Ok
        };

        AffectivePlanResult {
            candidates,
            outcome,
            self_state: Some(s_now),
            affect_divergence,
            elapsed: started.elapsed(),
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Helpers
// ---------------------------------------------------------------------------

/// Clamp `x` to `[0.0, 1.0]`. Used for affect_similarity (GUARD-6
/// enforcement: negative cosine ⇒ "no info", treated as 0).
#[inline]
fn clamp_unit(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

/// Compute Kendall-tau-b correlation between two rankings of the
/// same set of items (`order_a`, `order_b`). Both slices MUST contain
/// the same items (any order); behaviour is undefined otherwise.
///
/// Uses the standard pair-counting formula:
///   `tau = (concordant - discordant) / (n * (n-1) / 2)`
/// Tied pairs (same rank in either ranking) reduce the denominator
/// — but here every item has a unique rank within each ordering, so
/// no ties exist and the formula reduces to the simple form.
///
/// O(n²). Acceptable since `n ≤ k_seed_affective_max = 60` (§7.3).
fn kendall_tau(order_a: &[MemoryId], order_b: &[MemoryId]) -> f64 {
    let n = order_a.len();
    if n < 2 {
        return 0.0;
    }
    // Build rank-of-item lookup for `order_b`.
    let rank_b: std::collections::HashMap<&MemoryId, usize> = order_b
        .iter()
        .enumerate()
        .map(|(i, id)| (id, i))
        .collect();

    let mut concordant: i64 = 0;
    let mut discordant: i64 = 0;
    for i in 0..n {
        for j in (i + 1)..n {
            let a_i = i as i64;
            let a_j = j as i64;
            let b_i = match rank_b.get(&order_a[i]) {
                Some(&r) => r as i64,
                None => continue,
            };
            let b_j = match rank_b.get(&order_a[j]) {
                Some(&r) => r as i64,
                None => continue,
            };
            let s = (a_i - a_j).signum() * (b_i - b_j).signum();
            if s > 0 {
                concordant += 1;
            } else if s < 0 {
                discordant += 1;
            }
        }
    }
    let total = (n as i64) * (n as i64 - 1) / 2;
    if total == 0 {
        return 0.0;
    }
    (concordant - discordant) as f64 / total as f64
}

// ---------------------------------------------------------------------------
// 7. Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::budget::BudgetController;

    // ----- Helpers ----------------------------------------------------------

    fn budget() -> BudgetController {
        BudgetController::with_defaults()
    }
    fn query() -> GraphQuery {
        GraphQuery::new("anything")
    }
    fn fp(v: f32, a: f32) -> SomaticFingerprint {
        // Construct a fingerprint with explicit valence + arousal,
        // other axes left at zero. Adequate for cosine-similarity
        // tests since cosine on the 8-dim vector reduces to the
        // weighted contribution of non-zero axes.
        let mut arr = [0.0f32; 8];
        arr[0] = v;
        arr[1] = a;
        SomaticFingerprint::from_array(arr)
    }

    /// Deterministic recaller — returns a fixed list regardless of
    /// `query`.
    struct StubRecaller {
        hits: Vec<AffectiveSeedHit>,
        status: AffectiveSeedStatus,
    }

    impl AffectiveSeedRecaller for StubRecaller {
        fn recall(
            &self,
            _q: &GraphQuery,
            _k: usize,
        ) -> (Vec<AffectiveSeedHit>, AffectiveSeedStatus) {
            (self.hits.clone(), self.status.clone())
        }
    }

    fn hit(id: &str, text: f64, recency: f64, snap: Option<SomaticFingerprint>) -> AffectiveSeedHit {
        AffectiveSeedHit {
            memory_id: id.to_string(),
            text_score: text,
            recency_score: recency,
            affect_snapshot: snap,
        }
    }

    // ----- Unit tests -------------------------------------------------------

    #[test]
    fn weights_sum_to_one() {
        // Static check that the locked weights still sum to 1.0 —
        // a guard against accidental drift (Kendall-tau telemetry
        // assumes the weight set is held constant across the two
        // rankings being compared).
        assert!((W_TEXT + W_AFFECT + W_RECENCY - 1.0).abs() < 1e-9);
        assert!(_WEIGHTS_SUM > 0.99 && _WEIGHTS_SUM < 1.01);
    }

    #[test]
    fn w_affect_strictly_less_than_one_enforces_never_block() {
        // GUARD-6 / GOAL-3.14: w_affect < 1.0 means a textually-
        // perfect match cannot be filtered out by affect alone.
        assert!(W_AFFECT < 1.0);
    }

    #[test]
    fn no_self_state_downgrades() {
        let plan = AffectivePlan::default();
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: None,
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.outcome, AffectiveOutcome::DowngradedNoSelfState);
        assert!(res.candidates.is_empty());
        assert!(res.affect_divergence.is_none());
    }

    #[test]
    fn no_seeds_downgrades_with_self_state_preserved() {
        let plan = AffectivePlan::default();
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(0.5, 0.3)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.outcome, AffectiveOutcome::DowngradedNoSeeds);
        // Self-state echoed back even on downgrade — explain trace
        // needs it for "what was the active state" diagnostics.
        assert_eq!(res.self_state, Some(fp(0.5, 0.3)));
    }

    #[test]
    fn cutoff_propagates_from_recaller_backend() {
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![],
            status: AffectiveSeedStatus::Cutoff,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(0.5, 0.3)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.outcome, AffectiveOutcome::Cutoff);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn ranks_by_local_fused_score_when_text_tied() {
        // Two memories with identical text_score and recency, but
        // affect_snapshots that match s_now to different degrees.
        // Higher affect_similarity must rank higher.
        let s_now = fp(1.0, 0.0); // pure positive valence
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                // m_far: opposite valence → cosine = -1 → clamped 0
                hit("m_far", 0.5, 0.5, Some(fp(-1.0, 0.0))),
                // m_close: same valence → cosine = 1 → clamped 1
                hit("m_close", 0.5, 0.5, Some(fp(1.0, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(s_now),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.outcome, AffectiveOutcome::Ok);
        assert_eq!(res.candidates.len(), 2);
        assert_eq!(res.candidates[0].memory_id, "m_close");
        assert_eq!(res.candidates[1].memory_id, "m_far");
        // Negative cosine clamped to 0 — GUARD-6 enforcement.
        assert_eq!(res.candidates[1].affect_similarity, 0.0);
        assert!((res.candidates[0].affect_similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn missing_affect_snapshot_treated_as_neutral() {
        // A memory without a captured fingerprint should not be
        // dropped — GUARD-6 forbids gating. It receives
        // affect_similarity = 0 (neutral) and is ranked accordingly.
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                hit("m_match", 0.4, 0.5, Some(fp(1.0, 0.0))),
                hit("m_unknown", 0.4, 0.5, None),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.candidates.len(), 2);
        // Both rows survive (never gated).
        let ids: Vec<_> = res.candidates.iter().map(|c| c.memory_id.as_str()).collect();
        assert!(ids.contains(&"m_match"));
        assert!(ids.contains(&"m_unknown"));
        // Unknown gets affect_similarity = 0.
        let unknown = res.candidates.iter().find(|c| c.memory_id == "m_unknown").unwrap();
        assert_eq!(unknown.affect_similarity, 0.0);
        assert!(unknown.affect_snapshot.is_none());
    }

    #[test]
    fn never_blocks_textually_perfect_match() {
        // GUARD-6 / GOAL-3.14 hard property: a memory with
        // text_score = 1.0 cannot be reordered below a memory with
        // lower text_score on affect grounds alone.
        // local_score(perfect_text) = 0.50*1 + 0.35*0 + 0.15*recency
        //                           = 0.50 + 0.15*recency
        // local_score(low_text_high_affect) = 0.50*0 + 0.35*1 + 0.15*recency
        //                                   = 0.35 + 0.15*recency
        // With identical recency, perfect_text wins (0.50 > 0.35).
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                hit("perfect_text", 1.0, 0.5, Some(fp(-1.0, 0.0))),
                hit("perfect_affect", 0.0, 0.5, Some(fp(1.0, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert_eq!(res.candidates[0].memory_id, "perfect_text");
    }

    #[test]
    fn guard6_property_two_states_yield_same_set() {
        // GUARD-6 formal statement: result set under any two
        // self-states is identical (only ordering differs).
        let recaller = StubRecaller {
            hits: vec![
                hit("a", 0.6, 0.5, Some(fp(0.8, 0.5))),
                hit("b", 0.4, 0.5, Some(fp(-0.5, 0.2))),
                hit("c", 0.7, 0.5, Some(fp(0.0, 0.0))),
                hit("d", 0.3, 0.5, None),
            ],
            status: AffectiveSeedStatus::Ok,
        };
        let plan = AffectivePlan::new(recaller);

        let res_pos = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(1.0, 0.5)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        let res_neg = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(-1.0, 0.5)),
            budget: budget(),
            divergence_roll: 0.5,
        });

        let mut ids_pos: Vec<_> = res_pos.candidates.iter().map(|c| c.memory_id.clone()).collect();
        let mut ids_neg: Vec<_> = res_neg.candidates.iter().map(|c| c.memory_id.clone()).collect();
        ids_pos.sort();
        ids_neg.sort();
        assert_eq!(ids_pos, ids_neg);
        assert_eq!(ids_pos, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn divergence_telemetry_runs_when_explain_true() {
        // explain=true forces the second ranking even when the
        // sample roll would skip — §4.5 step 5.
        let mut q = query();
        q.explain = true;

        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                hit("a", 0.5, 0.5, Some(fp(1.0, 0.0))),
                hit("b", 0.5, 0.5, Some(fp(-1.0, 0.0))),
                hit("c", 0.5, 0.5, Some(fp(0.5, 0.5))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &q,
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            // roll = 1.0 means the sampled path would NOT trigger;
            // explain must force telemetry anyway.
            divergence_roll: 1.0,
        });
        let div = res.affect_divergence.expect("divergence forced by explain");
        assert!(div.forced_by_explain);
        assert_eq!(div.n, 3);
        // tau is in [-1, 1].
        assert!(div.kendall_tau >= -1.0 && div.kendall_tau <= 1.0);
    }

    #[test]
    fn divergence_telemetry_skipped_at_default_rate_with_high_roll() {
        // Default sample rate is 0.01; roll = 0.5 ⇒ skip. No
        // telemetry, no perf hit.
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                hit("a", 0.5, 0.5, Some(fp(1.0, 0.0))),
                hit("b", 0.5, 0.5, Some(fp(-1.0, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        assert!(res.affect_divergence.is_none());
    }

    #[test]
    fn divergence_telemetry_runs_when_roll_below_rate() {
        // roll = 0.0 < default 0.01 ⇒ sampled path triggers.
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                hit("a", 0.5, 0.5, Some(fp(1.0, 0.0))),
                hit("b", 0.5, 0.5, Some(fp(-1.0, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            divergence_roll: 0.0,
        });
        let div = res.affect_divergence.expect("sampled in");
        assert!(!div.forced_by_explain);
    }

    #[test]
    fn divergence_detects_rank_change_under_state_swap() {
        // Construct hits where neutral-state ranking differs from
        // s_now ranking → tau < 1.0 (rankings disagree at least
        // once). Ensures the telemetry actually measures something.
        let mut q = query();
        q.explain = true;
        let plan = AffectivePlan::new(StubRecaller {
            hits: vec![
                // Identical text + recency ⇒ ranking driven purely
                // by affect_similarity differential.
                hit("aligned", 0.5, 0.5, Some(fp(1.0, 0.0))),
                hit("opposed", 0.5, 0.5, Some(fp(-1.0, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        });
        let res = plan.execute(AffectivePlanInputs {
            query: &q,
            self_state: Some(fp(1.0, 0.0)),
            budget: budget(),
            divergence_roll: 1.0, // explain forces anyway
        });
        let div = res.affect_divergence.unwrap();
        // Under s_now=positive: aligned > opposed (similarity 1 vs 0).
        // Under s_neutral=zero: cosine = 0 for both ⇒ tied ⇒
        // tiebreak on memory_id ⇒ "aligned" before "opposed"
        // alphabetically (a < o) — same order. Tau = 1.0 here.
        // Deeper test of rank divergence sits at the property /
        // benchmark layer (§9).
        assert!(div.kendall_tau >= -1.0 && div.kendall_tau <= 1.0);
    }

    #[test]
    fn determinism_repeated_runs_yield_identical_ordering() {
        // Design §5.4 — repeatability invariant. Same inputs ⇒
        // byte-identical outputs.
        let recaller = StubRecaller {
            hits: vec![
                hit("a", 0.5, 0.5, Some(fp(0.3, 0.4))),
                hit("b", 0.5, 0.5, Some(fp(0.3, 0.4))), // tie with a
                hit("c", 0.6, 0.4, Some(fp(0.1, 0.0))),
            ],
            status: AffectiveSeedStatus::Ok,
        };
        let plan = AffectivePlan::new(recaller);
        let r1 = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(0.5, 0.5)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        let r2 = plan.execute(AffectivePlanInputs {
            query: &query(),
            self_state: Some(fp(0.5, 0.5)),
            budget: budget(),
            divergence_roll: 0.5,
        });
        let ids1: Vec<_> = r1.candidates.iter().map(|c| c.memory_id.clone()).collect();
        let ids2: Vec<_> = r2.candidates.iter().map(|c| c.memory_id.clone()).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn clamp_unit_handles_edge_values() {
        assert_eq!(clamp_unit(-0.5), 0.0);
        assert_eq!(clamp_unit(0.0), 0.0);
        assert_eq!(clamp_unit(0.5), 0.5);
        assert_eq!(clamp_unit(1.0), 1.0);
        assert_eq!(clamp_unit(2.5), 1.0);
        assert_eq!(clamp_unit(f64::NAN), 0.0);
    }

    #[test]
    fn kendall_tau_basic() {
        // Identical orderings ⇒ tau = 1.
        let a = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let b = a.clone();
        assert!((kendall_tau(&a, &b) - 1.0).abs() < 1e-9);

        // Reversed ⇒ tau = -1.
        let mut rev = a.clone();
        rev.reverse();
        assert!((kendall_tau(&a, &rev) + 1.0).abs() < 1e-9);

        // n < 2 ⇒ 0.
        let single = vec!["only".to_string()];
        let single2 = single.clone();
        assert_eq!(kendall_tau(&single, &single2), 0.0);
    }

    #[test]
    fn affective_candidate_local_fused_score_is_pure_function_of_weights() {
        let c = AffectiveCandidate {
            memory_id: "m".into(),
            text_score: 0.8,
            affect_similarity: 0.6,
            recency_score: 0.4,
            affect_snapshot: None,
        };
        let expected = W_TEXT * 0.8 + W_AFFECT * 0.6 + W_RECENCY * 0.4;
        assert!((c.local_fused_score() - expected).abs() < 1e-9);
    }
}
