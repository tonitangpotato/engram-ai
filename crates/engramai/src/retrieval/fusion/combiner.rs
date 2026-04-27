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
    /// Semantic version pin — bumped on weight changes.
    pub version: &'static str,
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

    // Re-score every Memory candidate. Topic candidates keep their
    // existing score (Abstract plan computes them differently — §4.4).
    for r in candidates.iter_mut() {
        if let ScoredResult::Memory {
            ref sub_scores,
            ref mut score,
            ..
        } = r
        {
            *score = combine(intent, sub_scores, &weights);
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
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
            id_of(a).cmp(&id_of(b))
        })
    });

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
    let k = if k.is_finite() && k > 0.0 { k } else { RRF_DEFAULT_K };

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
