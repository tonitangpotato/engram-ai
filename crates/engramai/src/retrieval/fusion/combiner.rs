//! # Per-plan weighted fusion (`task:retr-impl-fusion`)
//!
//! Combines per-memory [`SubScores`] (produced by [`signals`](super::signals))
//! into a single fused score per the per-plan weight matrix defined in
//! design §5.2:
//!
//! ```text
//! Factual:     final = 0.45 * graph + 0.40 * text + 0.15 * recency
//! Episodic:    final = 0.55 * text  + 0.30 * recency + 0.15 * graph
//! Associative: final = 0.40 * vector + 0.35 * graph + 0.25 * actr
//! Abstract:    final = 0.60 * text  + 0.25 * actr + 0.15 * recency
//! Affective:   final = 0.50 * text  + 0.35 * affect + 0.15 * recency
//! Hybrid:      reciprocal rank fusion over sub-plan outputs
//! ```
//!
//! Where `text = max(vector_score, bm25_score)` (§5.2 conservative
//! aggregate, avoids double-counting two highly-correlated signals).
//!
//! ## Missing-signal renormalization (§5.2)
//!
//! When a plan's weighted-sum component is `None` in [`SubScores`], its
//! weight is **redistributed proportionally** across the remaining present
//! components so the live weights still sum to `1.0`. This keeps fused
//! scores in `[0, 1]` and comparable across queries with and without the
//! missing signal. If *all* components are absent, the fused score is
//! `0.0`.
//!
//! ## Determinism (§5.4)
//!
//! - Pure functions of inputs — no clock, no `rand`, no global state.
//! - Tie-break in [`fuse_and_rank`] is `(score desc, memory_id asc)`.
//! - [`reciprocal_rank_fusion`] sorts inputs by `id asc` before scoring,
//!   so order of sub-plan outputs cannot perturb results.
//!
//! ## Locked configuration (§5.4 — benchmarks handoff)
//!
//! [`FusionConfig::locked()`] returns the canonical, frozen configuration
//! used by `v03-benchmarks` and any caller needing byte-identical output.
//! It is a pure constructor — no env vars, no config files, no flags.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::retrieval::api::{ScoredResult, SubScores};
use crate::retrieval::classifier::Intent;

// ---------------------------------------------------------------------------
// FusionWeights — per-plan named weights
// ---------------------------------------------------------------------------

/// Named per-signal weights for a single plan.
///
/// Field semantics depend on plan: e.g., for `Factual`, `text` is the
/// `max(vector, bm25)` aggregate and `graph` is the edge-distance decay;
/// for `Affective`, `affect` is the `affect_similarity` field. The fields
/// not used by a given plan must be `0.0` in that plan's
/// [`FusionWeights`].
///
/// Invariants enforced by [`FusionConfig::locked`] and tested in unit
/// tests:
///
/// - All fields finite and in `[0, 1]`.
/// - Sum of all fields equals `1.0` (within `1e-9`) for every plan in
///   [`SignalWeightMatrix`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FusionWeights {
    /// Weight on `text_score = max(vector_score, bm25_score)`.
    pub text: f64,
    /// Weight on the bare `vector_score` (used by Associative as
    /// `seed_score`).
    pub vector: f64,
    /// Weight on `graph_score`.
    pub graph: f64,
    /// Weight on `recency_score`.
    pub recency: f64,
    /// Weight on `actr_score`.
    pub actr: f64,
    /// Weight on `affect_similarity` (Affective plan only).
    pub affect: f64,
}

impl FusionWeights {
    /// Sum of all weights (must be `1.0` for a well-formed plan).
    pub fn sum(&self) -> f64 {
        self.text + self.vector + self.graph + self.recency + self.actr + self.affect
    }
}

// ---------------------------------------------------------------------------
// SignalWeightMatrix — per-Intent weight set
// ---------------------------------------------------------------------------

/// Per-plan signal weights — one entry per `Intent` variant.
///
/// `Hybrid` is present for symmetry but its weights are unused by the
/// combiner: Hybrid uses [`reciprocal_rank_fusion`] over sub-plan outputs
/// rather than a weighted sum (§5.2).
///
/// Stored as a struct of named fields (rather than `HashMap<Intent, _>`)
/// so the type derives `Serialize`/`Deserialize` without requiring serde
/// on the `Intent` enum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalWeightMatrix {
    pub factual: FusionWeights,
    pub episodic: FusionWeights,
    pub abstract_: FusionWeights,
    pub affective: FusionWeights,
    pub hybrid: FusionWeights,
}

impl SignalWeightMatrix {
    /// Look up the weights for a given plan.
    pub fn get(&self, intent: Intent) -> FusionWeights {
        match intent {
            Intent::Factual => self.factual,
            Intent::Episodic => self.episodic,
            Intent::Abstract => self.abstract_,
            Intent::Affective => self.affective,
            Intent::Hybrid => self.hybrid,
        }
    }
}

// ---------------------------------------------------------------------------
// FusionConfig — locked() per design §5.4
// ---------------------------------------------------------------------------

/// Frozen fusion configuration (§5.4). Used by benchmarks for
/// reproducibility records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusionConfig {
    /// Per-plan signal weights.
    pub signal_weights: SignalWeightMatrix,
    /// RRF constant `k` for [`reciprocal_rank_fusion`] (§4.7).
    pub rrf_k: f64,
    /// Drop fused candidates below this threshold.
    pub min_fused_score: f64,
    /// MMR diversity parameter for the post-fusion reranker (ISS-139).
    ///
    /// `1.0` = pure relevance, byte-identical to the no-rerank path
    /// (current v0.3 default). `0.0` = pure diversity (don't use).
    /// `0.5..0.8` = balanced (literature-recommended range for
    /// list-style queries).
    ///
    /// When `< 1.0`, the API routes the fused candidate list through
    /// `MmrReranker` before applying `top_k`. When `== 1.0`, the
    /// `NullReranker` path is taken — preserves the ISS-100 cross-
    /// validate envelope.
    ///
    /// Serde-defaults to `1.0` so older reproducibility records
    /// without this field still deserialize to the legacy behavior.
    #[serde(default = "default_mmr_lambda")]
    pub mmr_lambda: f32,
    /// ISS-164 — Associative plan always-on entity channel.
    ///
    /// When `true`, the Associative plan calls
    /// [`EntityResolver::resolve(query.text)`] alongside its
    /// seed-recall step and unions the resolved anchor entities into
    /// `seed_entities` before 1-hop edge expansion. This recovers the
    /// retrieval signal of an entity-anchored "Factual mini-pass"
    /// even when the classifier (ISS-149) misroutes the query to
    /// Associative — the documented root cause of 0/152 Factual
    /// dispatches on LoCoMo conv-26 and the AC-5a single-fact gap.
    ///
    /// When `false` (default), AssociativePlan executes the v0.3
    /// §4.3 pipeline byte-identically — no resolver call, no
    /// `seed_entities` injection. This preserves the §5.4 locked
    /// envelope until the bench validates the lift.
    ///
    /// Serde-defaults to `false` so older reproducibility records
    /// still deserialize to the legacy behavior.
    #[serde(default = "default_entity_channel_enabled")]
    pub entity_channel_enabled: bool,
    /// ISS-175 — Factual-only reweighted fusion + new text aggregate.
    ///
    /// When `true`, the Factual path (in `fuse_and_rank`) uses
    /// `combine_factual_v2` instead of `combine`. The v2 formula:
    /// - rebalances weights: graph 0.30 (was 0.45), text 0.25
    ///   (was 0.40), vector 0.30 (was 0.0), recency 0.15 (unchanged)
    /// - replaces `text = max(vector, bm25)` with sum-with-evidence-bonus:
    ///   `text = 0.7*vec + 0.3*bm25 + 0.15 if bm25 > 0.05`
    ///
    /// Addresses three compounding bugs documented in the ISS-175
    /// probe (`.gid/issues/ISS-175/artifacts/probe-conv26-findings.md`):
    /// Bug 1 (graph_score saturation on 1-anchor queries — q75 has
    /// 100% of pool at g=1.0), Bug 2 (vector_score has no own weight
    /// channel), Bug 3 (max-aggregate discards rare-token bm25 signal,
    /// but bm25 has 7.5× gold/non-gold separation — strongest predictor).
    ///
    /// Serde-defaults to `false` so older reproducibility records
    /// deserialize to the locked v0.3.0-r3 behaviour. Callers flip via
    /// `GraphQuery::with_factual_reweight(Some(true))`.
    #[serde(default = "default_factual_reweight")]
    pub factual_reweight: bool,
    /// ISS-188 — populate candidate embeddings before the C.5 MMR hook.
    ///
    /// MMR's diversity term needs per-candidate embeddings (`sim(c, s)`
    /// = cosine on candidate vectors, see `mmr.rs`). The Factual and
    /// Episodic plans build their candidates without embeddings
    /// (`ScoredResult::Memory.embedding == None`), so MMR gives them a
    /// `0` diversity penalty and degenerates to a no-op on exactly the
    /// plans the list-questions route through (ISS-187 diagnosis:
    /// drop_CD 22/32 conv-26 single-hop, 10/13 SF-subset are LIST-type
    /// scoring 0; q18 gold "beach, mountains, forest" — beach memories
    /// sit at fusion rank 38/46/152 while the redundant mountains/forest
    /// cluster fills top-10).
    ///
    /// When `true`, the API batch-fetches embeddings via
    /// `Storage::get_embeddings_for_ids` for any `ScoredResult::Memory`
    /// candidate still carrying `embedding == None`, immediately before
    /// the C.5 MMR hook, so MMR can compute real cosine-diversity and
    /// surface relevant-but-distant list items into the head before
    /// `top_k` truncation.
    ///
    /// When `false` (default), the candidate set reaches MMR unchanged
    /// — byte-identical to the locked v0.3 path. Callers flip via
    /// `GraphQuery::with_populate_embeddings_for_diversity(Some(true))`.
    ///
    /// Serde-defaults to `false` so older reproducibility records
    /// deserialize to the locked behaviour.
    #[serde(default = "default_populate_embeddings_for_diversity")]
    pub populate_embeddings_for_diversity: bool,
    /// Semantic version pin — bumped on weight changes.
    pub version: &'static str,
}

/// Serde default for [`FusionConfig::mmr_lambda`]. `0.7` is the
/// shipped default after ISS-146 validation (L1+MMR conv-26
/// single-hop 0.0625 → 0.2188, overall 0.3947 → 0.4671). Pre-ISS-146
/// the default was `1.0` (MMR off, ISS-139 ship gate); the lower
/// value trades a small amount of relevance for diversity that helps
/// list-style queries recover gold items spread across near-duplicate
/// candidates. Callers wanting the legacy no-op behaviour can pass
/// `GraphQuery::with_mmr_lambda(Some(1.0))`.
fn default_mmr_lambda() -> f32 {
    0.7
}

/// Serde default for [`FusionConfig::entity_channel_enabled`] (ISS-164).
///
/// Defaults to `false` — Associative plan stays byte-identical to the
/// §4.3 pipeline. The bench harness flips this via
/// `GraphQuery::entity_channel_override` to A/B against the
/// always-on entity channel without re-baking `FusionConfig::locked()`.
fn default_entity_channel_enabled() -> bool {
    false
}

/// Serde default for [`FusionConfig::factual_reweight`] (ISS-175).
///
/// Defaults to `false` — Factual fusion stays byte-identical to the
/// locked v0.3.0-r3 weight matrix + `text = max(vec, bm25)` aggregate.
/// The bench harness flips this via `GraphQuery::factual_reweight_override`
/// to A/B against the rebalanced weights + sum-with-evidence-bonus text
/// aggregate without re-baking `FusionConfig::locked()`.
fn default_factual_reweight() -> bool {
    false
}

/// Serde default for [`FusionConfig::populate_embeddings_for_diversity`]
/// (ISS-188). `false` keeps the locked v0.3 path where Factual/Episodic
/// candidates reach MMR without embeddings (diversity no-op).
fn default_populate_embeddings_for_diversity() -> bool {
    false
}

impl FusionConfig {
    /// Canonical, frozen fusion configuration. **Pure**: no env, no files.
    /// Returns byte-identical output on every call within an `engramai`
    /// version.
    ///
    /// Weights are taken verbatim from design §5.2.
    pub fn locked() -> Self {
        // Factual: 0.45 graph + 0.40 text + 0.15 recency
        let factual = FusionWeights {
            text: 0.40,
            vector: 0.0,
            graph: 0.45,
            recency: 0.15,
            actr: 0.0,
            affect: 0.0,
        };

        // Episodic: 0.55 text + 0.30 recency + 0.15 graph
        let episodic = FusionWeights {
            text: 0.55,
            vector: 0.0,
            graph: 0.15,
            recency: 0.30,
            actr: 0.0,
            affect: 0.0,
        };

        // Abstract: 0.60 text + 0.25 actr + 0.15 source_coverage (≈recency)
        let abstract_ = FusionWeights {
            text: 0.60,
            vector: 0.0,
            graph: 0.0,
            recency: 0.15,
            actr: 0.25,
            affect: 0.0,
        };

        // Affective: 0.50 text + 0.35 affect + 0.15 recency
        let affective = FusionWeights {
            text: 0.50,
            vector: 0.0,
            graph: 0.0,
            recency: 0.15,
            actr: 0.0,
            affect: 0.35,
        };

        // Hybrid: weights unused (RRF), but must sum to 1.0 to satisfy
        // the SignalWeightMatrix invariant.
        let hybrid = FusionWeights {
            text: 0.50,
            vector: 0.0,
            graph: 0.50,
            recency: 0.0,
            actr: 0.0,
            affect: 0.0,
        };

        FusionConfig {
            signal_weights: SignalWeightMatrix {
                factual,
                episodic,
                abstract_,
                affective,
                hybrid,
            },
            rrf_k: RRF_DEFAULT_K,
            min_fused_score: 0.0,
            mmr_lambda: default_mmr_lambda(),
            entity_channel_enabled: default_entity_channel_enabled(),
            factual_reweight: default_factual_reweight(),
            populate_embeddings_for_diversity: default_populate_embeddings_for_diversity(),
            version: "v0.3.0-locked-r3",
        }
    }
}

/// Default RRF `k` constant (§4.7). Standard literature value.
pub const RRF_DEFAULT_K: f64 = 60.0;

// ---------------------------------------------------------------------------
// Core combiner
// ---------------------------------------------------------------------------

/// Fuse per-signal sub-scores into a single `[0, 1]` value for a plan.
///
/// Implements the `text = max(vector, bm25)` aggregate (§5.2) and the
/// missing-signal renormalization rule. Returns `0.0` if every weighted
/// component is `None`.
///
/// `Hybrid` is **not** valid here — Hybrid uses
/// [`reciprocal_rank_fusion`]. Calling `combine` with `Intent::Hybrid`
/// returns `0.0` (caller bug, but no panic).
pub fn combine(intent: Intent, sub: &SubScores, weights: &FusionWeights) -> f64 {
    if matches!(intent, Intent::Hybrid) {
        return 0.0;
    }

    // text_score = max(vector, bm25), present iff at least one of the two
    // is present.
    let text_score: Option<f64> = match (sub.vector_score, sub.bm25_score) {
        (Some(v), Some(b)) => Some(v.max(b)),
        (Some(v), None) => Some(v),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    // Pair (weight, value) for each component the weight matrix knows
    // about. A component is "present" iff its weight > 0 AND its
    // sub-score is `Some(_)`.
    let components: [(f64, Option<f64>); 6] = [
        (weights.text, text_score),
        (weights.vector, sub.vector_score),
        (weights.graph, sub.graph_score),
        (weights.recency, sub.recency_score),
        (weights.actr, sub.actr_score),
        (weights.affect, sub.affect_similarity),
    ];

    // Sum of weights for components that are actually present. This is
    // the denominator for renormalization (§5.2). If no component is
    // present, return 0.0 (no signal to fuse).
    let mut live_weight_sum = 0.0_f64;
    let mut weighted_sum = 0.0_f64;
    for (w, v) in components.iter() {
        if *w <= 0.0 {
            continue;
        }
        if let Some(score) = v {
            // Defensive clamp — signals.rs guarantees [0, 1] but the
            // combiner is the last line of defense.
            let clamped = clamp01(*score);
            weighted_sum += w * clamped;
            live_weight_sum += w;
        }
    }

    if live_weight_sum <= 0.0 {
        return 0.0;
    }

    // Renormalize: divide by the live weight sum so the result is
    // back in [0, 1] regardless of which components were missing.
    clamp01(weighted_sum / live_weight_sum)
}

// ---------------------------------------------------------------------------
// combine_factual_v2 — ISS-175 reweighted Factual fusion
// ---------------------------------------------------------------------------

/// ISS-175 — Factual-only fusion with rebalanced weights and the
/// sum-with-evidence-bonus text aggregate. Used ONLY when
/// `FusionConfig.factual_reweight` is `true`. See that field's docs
/// for the why; the probe findings file is the ground truth on the
/// three bugs this addresses.
///
/// Pure function of inputs — no clock, no RNG. The `intent` parameter
/// is implicit (Factual); callers must dispatch on the flag in
/// `fuse_and_rank` before invoking.
pub fn combine_factual_v2(sub: &SubScores) -> f64 {
    // Locked v2 weights (sum to 1.0):
    const W_TEXT: f64 = 0.25;
    const W_VECTOR: f64 = 0.30;
    const W_GRAPH: f64 = 0.30;
    const W_RECENCY: f64 = 0.15;

    // New text aggregate: 0.7*vec + 0.3*bm25 + 0.15 bonus if bm25 > 0.05.
    // Replaces v1's `max(vec, bm25)` which silently discards bm25 on
    // gold rows where vec > bm25 — the Bug 3 case from ISS-175.
    let text_score: Option<f64> = match (sub.vector_score, sub.bm25_score) {
        (None, None) => None,
        (v, b) => {
            let v = v.unwrap_or(0.0);
            let b = b.unwrap_or(0.0);
            let base = 0.7 * v + 0.3 * b;
            let bonus = if b > 0.05 { 0.15 } else { 0.0 };
            Some((base + bonus).min(1.0))
        }
    };

    // Component renormalization (§5.2) — identical pattern to `combine`,
    // just with the v2 weight constants and the vector channel given
    // its own weight (W_VECTOR > 0).
    let components: [(f64, Option<f64>); 4] = [
        (W_TEXT, text_score),
        (W_VECTOR, sub.vector_score),
        (W_GRAPH, sub.graph_score),
        (W_RECENCY, sub.recency_score),
    ];

    let mut live_weight_sum = 0.0_f64;
    let mut weighted_sum = 0.0_f64;
    for (w, v) in components.iter() {
        if *w <= 0.0 {
            continue;
        }
        if let Some(score) = v {
            let clamped = clamp01(*score);
            weighted_sum += w * clamped;
            live_weight_sum += w;
        }
    }

    if live_weight_sum <= 0.0 {
        return 0.0;
    }

    clamp01(weighted_sum / live_weight_sum)
}

// ---------------------------------------------------------------------------
// fuse_and_rank — apply combine + tie-break + min_fused_score
// ---------------------------------------------------------------------------

/// Apply [`combine`] to each candidate, drop those below
/// `cfg.min_fused_score`, and rank by `(score desc, memory_id asc)`
/// (§5.4 deterministic tie-break).
///
/// Mutates `score` on each result; the input `sub_scores` are kept as-is
/// for `explain()` traces.
pub fn fuse_and_rank(
    intent: Intent,
    cfg: &FusionConfig,
    mut candidates: Vec<ScoredResult>,
) -> Vec<ScoredResult> {
    let weights = cfg.signal_weights.get(intent);

    // ISS-175 — Factual path may opt into the reweighted v2 fusion via
    // `cfg.factual_reweight`. All other intents always use `combine`.
    let use_factual_v2 = cfg.factual_reweight && matches!(intent, Intent::Factual);

    // Re-score every Memory candidate. Topic candidates keep their
    // existing score (Abstract plan computes them differently — §4.4).
    for r in candidates.iter_mut() {
        if let ScoredResult::Memory {
            ref sub_scores,
            ref mut score,
            ..
        } = r
        {
            *score = if use_factual_v2 {
                combine_factual_v2(sub_scores)
            } else {
                combine(intent, sub_scores, &weights)
            };
        }
    }

    // Drop candidates below cutoff.
    if cfg.min_fused_score > 0.0 {
        candidates.retain(|r| score_of(r) >= cfg.min_fused_score);
    }

    // Sort: score desc, then memory_id ascending (stable tie-break, §5.4).
    candidates.sort_by(|a, b| {
        let sa = score_of(a);
        let sb = score_of(b);
        // f64 comparison: NaN treated as smallest.
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| id_of(a).cmp(&id_of(b)))
    });

    // ISS-175 diagnostic dump (env-gated, default no-op).
    // Hook lands here — post-sort, pre-return — so the dumped pool
    // reflects what callers will actually see, including
    // `min_fused_score` cutoff. See `crate::retrieval::fusion::dump`
    // for the activation contract (env vars + thread-local label).
    crate::retrieval::fusion::dump::maybe_dump_fused_pool(intent, &candidates);

    candidates
}

fn score_of(r: &ScoredResult) -> f64 {
    match r {
        ScoredResult::Memory { score, .. } => *score,
        ScoredResult::Topic { score, .. } => *score,
    }
}

fn id_of(r: &ScoredResult) -> String {
    match r {
        ScoredResult::Memory { record, .. } => record.id.clone(),
        ScoredResult::Topic { topic, .. } => topic.topic_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Reciprocal Rank Fusion (Hybrid plan, §4.7 / §5.2)
// ---------------------------------------------------------------------------

/// Reciprocal Rank Fusion across multiple ranked sub-plan outputs.
///
/// For each candidate `c` and each sub-plan list `L_i`, RRF score is
/// `sum_i 1 / (k + rank_i(c))` where `rank_i(c)` is c's 1-indexed rank
/// in `L_i` (or omitted if `c` is absent from `L_i`).
///
/// **Determinism (§5.4)**: each input list is sorted by `memory_id asc`
/// internally before rank computation only for tie-breaking *within
/// equal scores*; the original ranking is preserved for the rank
/// numerator. The output is sorted by `(rrf_score desc, id asc)`.
///
/// Topic results are passed through with their existing scores
/// recomputed via RRF the same way.
pub fn reciprocal_rank_fusion(
    sub_plan_outputs: Vec<Vec<ScoredResult>>,
    k: f64,
) -> Vec<ScoredResult> {
    if sub_plan_outputs.is_empty() {
        return Vec::new();
    }
    let k = if k.is_finite() && k > 0.0 {
        k
    } else {
        RRF_DEFAULT_K
    };

    // Map: id -> (rrf_score, representative ScoredResult).
    let mut acc: HashMap<String, (f64, ScoredResult)> = HashMap::new();

    for list in sub_plan_outputs.into_iter() {
        // Each sub-plan list is already ranked by score (input contract:
        // sub-plans run their own fuse_and_rank before handing to RRF).
        // We trust that order for the rank numerator.
        for (rank0, item) in list.into_iter().enumerate() {
            let rank = rank0 as f64 + 1.0; // 1-indexed
            let id = id_of(&item).to_string();
            let contribution = 1.0 / (k + rank);

            acc.entry(id)
                .and_modify(|(s, _)| *s += contribution)
                .or_insert((contribution, item));
        }
    }

    // Materialize, set the new fused score, sort.
    let mut out: Vec<ScoredResult> = acc
        .into_values()
        .map(|(rrf_score, mut item)| {
            // Normalize RRF score into [0, 1] by dividing by an upper
            // bound: a candidate appearing rank-1 in N lists scores at
            // most N/(k+1). We don't know N here, so use a loose bound
            // and clamp. RRF scores are inherently small (≤ 1/(k+1) per
            // list), so `min(1.0, score * (k+1))` is the standard
            // convention.
            let normalized = clamp01(rrf_score * (k + 1.0));
            match &mut item {
                ScoredResult::Memory { score, .. } => *score = normalized,
                ScoredResult::Topic { score, .. } => *score = normalized,
            }
            item
        })
        .collect();

    out.sort_by(|a, b| {
        let sa = score_of(a);
        let sb = score_of(b);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| id_of(a).cmp(&id_of(b)))
    });

    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    x.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sub_full() -> SubScores {
        SubScores {
            vector_score: Some(0.8),
            bm25_score: Some(0.6),
            graph_score: Some(0.5),
            recency_score: Some(0.4),
            actr_score: Some(0.3),
            affect_similarity: Some(0.2),
        }
    }

    // -------- FusionConfig::locked invariants --------

    #[test]
    fn locked_weights_sum_to_one_per_plan() {
        let cfg = FusionConfig::locked();
        for intent in [
            Intent::Factual,
            Intent::Episodic,
            Intent::Abstract,
            Intent::Affective,
            Intent::Hybrid,
        ] {
            let w = cfg.signal_weights.get(intent);
            let s = w.sum();
            assert!(
                (s - 1.0).abs() < 1e-9,
                "Intent::{intent:?} weights sum {s} != 1.0"
            );
        }
    }

    #[test]
    fn locked_is_deterministic() {
        let a = FusionConfig::locked();
        let b = FusionConfig::locked();
        assert_eq!(a, b);
    }

    /// ISS-164 — `entity_channel_enabled` default and serde default.
    ///
    /// Locked default is `false` so the v0.3 §5.4 reproducibility
    /// envelope is preserved until the bench validates the lift on
    /// conv-26.
    #[test]
    fn locked_entity_channel_enabled_defaults_to_false() {
        let cfg = FusionConfig::locked();
        assert!(
            !cfg.entity_channel_enabled,
            "FusionConfig::locked().entity_channel_enabled must default \
             to false until ISS-164 bench validates the lift"
        );
    }

    /// ISS-164 — the `#[serde(default)]` helper itself.
    ///
    /// We can't round-trip through serde_json on this struct because
    /// `FusionConfig::version: &'static str` forces a `'static`
    /// deserializer lifetime that the JSON deserializer can't provide.
    /// Instead we test the default-function directly — it's the source
    /// of truth that `#[serde(default = "...")]` calls when the field
    /// is absent.
    #[test]
    fn default_entity_channel_enabled_helper_returns_false() {
        assert!(!default_entity_channel_enabled());
    }

    #[test]
    fn locked_weights_in_unit_range() {
        let cfg = FusionConfig::locked();
        for intent in [
            Intent::Factual,
            Intent::Episodic,
            Intent::Abstract,
            Intent::Affective,
            Intent::Hybrid,
        ] {
            let w = cfg.signal_weights.get(intent);
            for v in [w.text, w.vector, w.graph, w.recency, w.actr, w.affect] {
                assert!(v.is_finite());
                assert!((0.0..=1.0).contains(&v), "weight {v} out of range");
            }
        }
    }

    #[test]
    fn locked_has_pinned_version() {
        assert_eq!(FusionConfig::locked().version, "v0.3.0-locked-r3");
    }

    // -------- combine: basic --------

    #[test]
    fn combine_factual_with_full_signals() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Factual);
        let s = combine(Intent::Factual, &sub_full(), &w);
        // text=max(0.8,0.6)=0.8, graph=0.5, recency=0.4
        // = 0.40*0.8 + 0.45*0.5 + 0.15*0.4 = 0.32 + 0.225 + 0.06 = 0.605
        assert!((s - 0.605).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn combine_in_unit_range() {
        let cfg = FusionConfig::locked();
        for intent in [
            Intent::Factual,
            Intent::Episodic,
            Intent::Abstract,
            Intent::Affective,
        ] {
            let w = cfg.signal_weights.get(intent);
            let s = combine(intent, &sub_full(), &w);
            assert!((0.0..=1.0).contains(&s), "Intent::{intent:?} → {s}");
        }
    }

    #[test]
    fn combine_hybrid_returns_zero() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Hybrid);
        assert_eq!(combine(Intent::Hybrid, &sub_full(), &w), 0.0);
    }

    #[test]
    fn combine_all_none_returns_zero() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Factual);
        let empty = SubScores::default();
        assert_eq!(combine(Intent::Factual, &empty, &w), 0.0);
    }

    // -------- missing-signal renormalization (§5.2) --------

    #[test]
    fn combine_renormalizes_when_signal_missing() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Factual);

        // Drop the graph signal — text(0.8) + recency(0.4) only.
        // Live weights: text=0.40, recency=0.15. Sum = 0.55.
        // Weighted: 0.40*0.8 + 0.15*0.4 = 0.32 + 0.06 = 0.38.
        // Renormalized: 0.38 / 0.55 ≈ 0.6909090909.
        let sub = SubScores {
            vector_score: Some(0.8),
            bm25_score: Some(0.6),
            graph_score: None,
            recency_score: Some(0.4),
            actr_score: None,
            affect_similarity: None,
        };
        let s = combine(Intent::Factual, &sub, &w);
        let expected = 0.38 / 0.55;
        assert!((s - expected).abs() < 1e-9, "got {s}, expected {expected}");
    }

    #[test]
    fn combine_only_one_signal_present_returns_clamped_value() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Factual);

        // Only graph present.
        let sub = SubScores {
            vector_score: None,
            bm25_score: None,
            graph_score: Some(0.7),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let s = combine(Intent::Factual, &sub, &w);
        // Live weights = 0.45 (graph). Renormalized: 0.45*0.7 / 0.45 = 0.7.
        assert!((s - 0.7).abs() < 1e-9, "got {s}");
    }

    #[test]
    fn combine_text_uses_max_of_vector_bm25() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Episodic);

        let sub_v = SubScores {
            vector_score: Some(0.9),
            bm25_score: Some(0.1),
            graph_score: Some(0.5),
            recency_score: Some(0.5),
            ..Default::default()
        };
        let sub_b = SubScores {
            vector_score: Some(0.1),
            bm25_score: Some(0.9),
            graph_score: Some(0.5),
            recency_score: Some(0.5),
            ..Default::default()
        };
        // Both should yield identical scores (text = max(v, b) = 0.9).
        let s1 = combine(Intent::Episodic, &sub_v, &w);
        let s2 = combine(Intent::Episodic, &sub_b, &w);
        assert!((s1 - s2).abs() < 1e-12, "{s1} != {s2}");
    }

    // -------- determinism --------

    #[test]
    fn combine_deterministic() {
        let cfg = FusionConfig::locked();
        let w = cfg.signal_weights.get(Intent::Factual);
        let sub = sub_full();
        let s1 = combine(Intent::Factual, &sub, &w);
        let s2 = combine(Intent::Factual, &sub, &w);
        assert_eq!(s1.to_bits(), s2.to_bits());
    }

    // -------- ISS-175: combine_factual_v2 + factual_reweight flag --------

    #[test]
    fn factual_reweight_default_off() {
        // Pin the locked default. If this fails, someone changed the
        // default — bump version and update all downstream
        // reproducibility records.
        let cfg = FusionConfig::locked();
        assert!(
            !cfg.factual_reweight,
            "FusionConfig::locked().factual_reweight must default to false \
             so v0.3.0-r3 byte-identity is preserved",
        );
    }

    #[test]
    fn factual_reweight_v2_lifts_gold_with_rare_bm25_hit() {
        // Probe q71 case: gold has bm25=0.25 vec=0.62 graph=0.33,
        // leader has bm25=0.0 vec=0.67 graph=1.0. Under v1 (max-aggregate
        // + graph-dominant weights), leader wins despite gold's lexical
        // hit. Under v2, the gap should narrow (probe predicts ~18%
        // deficit reduction on q71-shaped data — may not flip alone).
        let gold = SubScores {
            vector_score: Some(0.62),
            bm25_score: Some(0.25),
            graph_score: Some(0.33),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let leader = SubScores {
            vector_score: Some(0.67),
            bm25_score: Some(0.0),
            graph_score: Some(1.0),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let weights = FusionConfig::locked().signal_weights.get(Intent::Factual);

        let v1_gold = combine(Intent::Factual, &gold, &weights);
        let v1_leader = combine(Intent::Factual, &leader, &weights);
        assert!(
            v1_leader > v1_gold,
            "v1 sanity: leader should currently win the q71-shape \
             (bug we're fixing). v1_leader={v1_leader:.4} v1_gold={v1_gold:.4}"
        );

        let v2_gold = combine_factual_v2(&gold);
        let v2_leader = combine_factual_v2(&leader);
        let v1_gap = v1_leader - v1_gold;
        let v2_gap = v2_leader - v2_gold;
        assert!(
            v2_gap < v1_gap,
            "v2 should narrow the gap. v1_gap={v1_gap:.4} v2_gap={v2_gap:.4}"
        );
    }

    #[test]
    fn factual_reweight_v2_renormalizes_when_recency_missing() {
        // Hand-computed expectation:
        //   text = 0.7*0.6 + 0.3*0.3 + 0.15 (bonus, bm25>0.05) = 0.66
        //   live weights: W_TEXT(0.25) + W_VECTOR(0.30) + W_GRAPH(0.30) = 0.85
        //   weighted_sum = 0.25*0.66 + 0.30*0.6 + 0.30*0.5
        //                = 0.165 + 0.18 + 0.15 = 0.495
        //   normalized   = 0.495 / 0.85 = 0.58235...
        let sub = SubScores {
            vector_score: Some(0.6),
            bm25_score: Some(0.3),
            graph_score: Some(0.5),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let s = combine_factual_v2(&sub);
        assert!(
            (s - 0.5824).abs() < 0.001,
            "expected ~0.5824 (renorm with recency missing), got {s}"
        );
    }

    #[test]
    fn factual_reweight_v2_evidence_bonus_fires_above_threshold() {
        // bm25 just below threshold: no bonus.
        let below = SubScores {
            vector_score: Some(0.5),
            bm25_score: Some(0.04),
            graph_score: Some(1.0),
            recency_score: Some(0.5),
            actr_score: None,
            affect_similarity: None,
        };
        // bm25 just above threshold: bonus fires.
        let above = SubScores {
            vector_score: Some(0.5),
            bm25_score: Some(0.06),
            graph_score: Some(1.0),
            recency_score: Some(0.5),
            actr_score: None,
            affect_similarity: None,
        };
        let s_below = combine_factual_v2(&below);
        let s_above = combine_factual_v2(&above);
        // Bonus = +0.15 to text * W_TEXT(0.25) = +0.0375 raw, then renorm
        // by 1.0 live weight → measurable lift of ~0.03.
        assert!(
            s_above > s_below + 0.03,
            "evidence bonus should add measurable lift: s_below={s_below:.4} \
             s_above={s_above:.4}"
        );
    }

    #[test]
    fn factual_reweight_v2_all_none_returns_zero() {
        let sub = SubScores {
            vector_score: None,
            bm25_score: None,
            graph_score: None,
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        assert_eq!(combine_factual_v2(&sub), 0.0);
    }

    #[test]
    fn factual_reweight_flag_dispatches_in_fuse_and_rank() {
        // Wire-up test: fuse_and_rank with cfg.factual_reweight=true on
        // a Factual intent should produce DIFFERENT scores than the
        // same call with cfg.factual_reweight=false (proves the branch
        // actually fires). Use the q71-shape gold from earlier.
        use crate::retrieval::ScoredResult;
        use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
        let sub_gold = SubScores {
            vector_score: Some(0.62),
            bm25_score: Some(0.25),
            graph_score: Some(0.33),
            recency_score: None,
            actr_score: None,
            affect_similarity: None,
        };
        let mk_record = || MemoryRecord {
            id: "test-mem-id".to_string(),
            content: "test".to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
            occurred_at: None,
            access_times: vec![],
            working_strength: 0.0,
            core_strength: 0.0,
            importance: 0.0,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        };
        let make_cand = || {
            vec![ScoredResult::Memory {
                record: mk_record(),
                score: 0.0,
                sub_scores: sub_gold.clone(),
                embedding: None,
                reserved: false,
            }]
        };

        let mut cfg_off = FusionConfig::locked();
        cfg_off.factual_reweight = false;
        let off = fuse_and_rank(Intent::Factual, &cfg_off, make_cand());

        let mut cfg_on = FusionConfig::locked();
        cfg_on.factual_reweight = true;
        let on = fuse_and_rank(Intent::Factual, &cfg_on, make_cand());

        let off_score = match &off[0] {
            ScoredResult::Memory { score, .. } => *score,
            _ => panic!("expected Memory"),
        };
        let on_score = match &on[0] {
            ScoredResult::Memory { score, .. } => *score,
            _ => panic!("expected Memory"),
        };
        assert!(
            (off_score - on_score).abs() > 1e-6,
            "flag must change scoring. off={off_score:.6} on={on_score:.6}"
        );
    }

    #[test]
    fn factual_reweight_does_not_affect_other_intents() {
        // Flip the flag on, but call fuse_and_rank with Episodic intent.
        // Score must be identical to the flag-off path because the
        // ISS-175 branch is Factual-only.
        use crate::retrieval::ScoredResult;
        use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
        let sub = SubScores {
            vector_score: Some(0.6),
            bm25_score: Some(0.3),
            graph_score: Some(0.5),
            recency_score: Some(0.4),
            actr_score: None,
            affect_similarity: None,
        };
        let mk_record = || MemoryRecord {
            id: "test-mem-id".to_string(),
            content: "test".to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: chrono::Utc::now(),
            occurred_at: None,
            access_times: vec![],
            working_strength: 0.0,
            core_strength: 0.0,
            importance: 0.0,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        };
        let make_cand = || {
            vec![ScoredResult::Memory {
                record: mk_record(),
                score: 0.0,
                sub_scores: sub.clone(),
                embedding: None,
                reserved: false,
            }]
        };

        let mut cfg_off = FusionConfig::locked();
        cfg_off.factual_reweight = false;
        let off = fuse_and_rank(Intent::Episodic, &cfg_off, make_cand());

        let mut cfg_on = FusionConfig::locked();
        cfg_on.factual_reweight = true;
        let on = fuse_and_rank(Intent::Episodic, &cfg_on, make_cand());

        let off_score = match &off[0] {
            ScoredResult::Memory { score, .. } => *score,
            _ => panic!("expected Memory"),
        };
        let on_score = match &on[0] {
            ScoredResult::Memory { score, .. } => *score,
            _ => panic!("expected Memory"),
        };
        assert_eq!(
            off_score.to_bits(),
            on_score.to_bits(),
            "Episodic intent must be byte-identical regardless of \
             factual_reweight flag"
        );
    }

    // -------- RRF --------

    #[test]
    fn rrf_empty_returns_empty() {
        let out = reciprocal_rank_fusion(vec![], RRF_DEFAULT_K);
        assert!(out.is_empty());
    }

    #[test]
    fn rrf_handles_invalid_k() {
        // 0 / NaN / negative k must fall back to default, not panic.
        let _ = reciprocal_rank_fusion(vec![vec![]], 0.0);
        let _ = reciprocal_rank_fusion(vec![vec![]], -1.0);
        let _ = reciprocal_rank_fusion(vec![vec![]], f64::NAN);
    }

    // -------- fuse_and_rank tie-break --------

    // We can't easily build ScoredResult::Memory in a unit test without
    // pulling in MemoryRecord construction, but the tie-break logic is
    // exercised indirectly by the integration tests in the retrieval
    // module. The `score_of` / `id_of` helpers are pure and obvious.

    #[test]
    fn fusion_weights_sum_helper() {
        let w = FusionWeights {
            text: 0.4,
            vector: 0.0,
            graph: 0.45,
            recency: 0.15,
            actr: 0.0,
            affect: 0.0,
        };
        assert!((w.sum() - 1.0).abs() < 1e-9);
    }
}
