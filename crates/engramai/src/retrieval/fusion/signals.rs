//! # Per-signal scorers (`task:retr-impl-fusion`)
//!
//! Pure scoring helpers for the five signals listed in design §5.1:
//!
//! | Signal | Source | Range |
//! |---|---|---|
//! | `vector_score` | cosine similarity over embeddings | `[0, 1]` |
//! | `bm25_score` | raw BM25 normalized | `[0, 1]` |
//! | `graph_score` | edge-distance decay from anchors | `[0, 1]` |
//! | `recency_score` | half-life decay of `memory.age` | `[0, 1]` |
//! | `actr_score` | ACT-R activation level normalized | `[0, 1]` |
//!
//! These functions are **pure** (`fn`, no `&self`, no clock, no `rand`) so
//! they may be freely reused by any plan and by tests. They never panic
//! and never produce `NaN` / `Inf` — invalid inputs (negative, infinite,
//! `NaN`) are clamped to the legal range or replaced with a documented
//! sentinel value.
//!
//! ## Why a separate module
//!
//! Per design §5.1 the *signal definitions* are decoupled from the *plan
//! that consumes them*. Multiple plans use the same scorer (e.g., both
//! Factual and Associative use `graph_score`, every plan uses
//! `recency_score` as a tiebreaker), so the scorers live here in one
//! place. Plans construct a [`SubScores`] (see
//! [`crate::retrieval::api::SubScores`]) by calling whichever scorers
//! apply, and feed that to the [`combiner`](super::combiner) for the
//! per-plan weighted sum.
//!
//! ## Determinism (§5.4)
//!
//! Every scorer in this module is a pure function of its inputs. No clock
//! reads, no `rand`, no global state. Combined with the [`combiner`]
//! determinism guarantees, this means the entire `signals → combine`
//! pipeline is reproducible.
//!
//! [`combiner`]: super::combiner

use std::time::Duration;

// ---------------------------------------------------------------------------
// Vector
// ---------------------------------------------------------------------------

/// Score a candidate by cosine similarity between query and memory
/// embeddings.
///
/// Inputs are full vectors (not pre-computed similarity) so the scorer can
/// validate dimensions and normalize internally. Output is `[0, 1]`:
/// negative cosines (semantic dissimilarity) are clamped to `0`, the
/// `+1` end of the cosine range maps directly to `1`.
///
/// # Errors / edge cases
///
/// - Mismatched dimensions: returns `0.0`.
/// - Either vector all-zero (zero-norm): returns `0.0`.
/// - Any `NaN` / `Inf` in either input: returns `0.0` (signal absent).
pub fn vector_score(query_emb: &[f32], memory_emb: &[f32]) -> f64 {
    if query_emb.len() != memory_emb.len() || query_emb.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut q_norm_sq = 0.0_f64;
    let mut m_norm_sq = 0.0_f64;
    for (q, m) in query_emb.iter().zip(memory_emb.iter()) {
        if !q.is_finite() || !m.is_finite() {
            return 0.0;
        }
        let q = *q as f64;
        let m = *m as f64;
        dot += q * m;
        q_norm_sq += q * q;
        m_norm_sq += m * m;
    }
    if q_norm_sq == 0.0 || m_norm_sq == 0.0 {
        return 0.0;
    }
    let cos = dot / (q_norm_sq.sqrt() * m_norm_sq.sqrt());
    clamp01(cos)
}

// ---------------------------------------------------------------------------
// BM25
// ---------------------------------------------------------------------------

/// Normalize a raw BM25 score into `[0, 1]`.
///
/// Per design §5.1 the existing `hybrid_search.rs` BM25 produces raw
/// values in `[0, ~20]`. We map this into `[0, 1]` via a saturating
/// linear transform: `min(raw / saturation, 1.0)`. The default saturation
/// constant is `BM25_DEFAULT_SATURATION = 20.0` per the §5.1 "≈20" upper
/// bound observation.
///
/// # Errors / edge cases
///
/// - Negative input: clamped to `0.0`.
/// - `NaN` / `Inf`: returns `0.0` (signal absent).
/// - `saturation <= 0.0`: returns `0.0` (caller error, but we don't panic).
pub fn bm25_score(raw_bm25: f64, saturation: f64) -> f64 {
    if !raw_bm25.is_finite() || !saturation.is_finite() || saturation <= 0.0 {
        return 0.0;
    }
    if raw_bm25 <= 0.0 {
        return 0.0;
    }
    (raw_bm25 / saturation).min(1.0)
}

/// Default saturation constant for [`bm25_score`] (raw BM25 values in v0.3
/// rarely exceed `20` per design §5.1).
pub const BM25_DEFAULT_SATURATION: f64 = 20.0;

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// Score by edge-distance decay from a set of query anchor entities.
///
/// `hops` is the integer edge distance from the closest anchor entity
/// reachable in the v0.3 graph layer (see `v03-graph-layer/design.md` §3).
/// `hops = 0` (the anchor itself, when the candidate IS an anchor entity)
/// scores `1.0`; each additional hop multiplies by `decay`. With the
/// default decay `0.5` the schedule is `[1.0, 0.5, 0.25, 0.125, ...]`,
/// which closely matches the rule-of-thumb "halve every hop" in §5.1.
///
/// # Errors / edge cases
///
/// - `hops` representable as `u32` only — the v0.3 graph hop budget is
///   tiny (`<= 4` per design), `u32::MAX` is unreachable.
/// - `decay` outside `[0.0, 1.0]`: clamped to `[0.0, 1.0]`.
/// - `decay == 0.0`: returns `1.0` for `hops == 0`, `0.0` otherwise
///   (single-hop "anchor only" mode).
pub fn graph_score(hops: u32, decay: f64) -> f64 {
    let decay = clamp01(decay);
    if hops == 0 {
        return 1.0;
    }
    if decay == 0.0 {
        return 0.0;
    }
    decay.powi(hops as i32)
}

/// Default per-hop decay factor for [`graph_score`].
pub const GRAPH_DEFAULT_DECAY: f64 = 0.5;

// ---------------------------------------------------------------------------
// Recency
// ---------------------------------------------------------------------------

/// Half-life decay over a memory's age.
///
/// `score = 0.5 ^ (age / half_life)`. At `age = 0` returns `1.0`; at
/// `age = half_life` returns `0.5`; etc. The result is in `[0, 1]` for
/// any non-negative age.
///
/// # Errors / edge cases
///
/// - `half_life` zero or non-finite: returns `0.0` (degenerate, all
///   memories treated as infinitely old). Caller should avoid this; the
///   non-panic behavior is here for robustness.
/// - Negative age (clock skew): clamped to `0` (`score = 1.0`).
pub fn recency_score(age: Duration, half_life: Duration) -> f64 {
    let half_life_secs = half_life.as_secs_f64();
    if !half_life_secs.is_finite() || half_life_secs <= 0.0 {
        return 0.0;
    }
    let age_secs = age.as_secs_f64();
    if !age_secs.is_finite() || age_secs <= 0.0 {
        return 1.0;
    }
    let exponent = age_secs / half_life_secs;
    clamp01(0.5_f64.powf(exponent))
}

/// Default half-life for [`recency_score`] (24h matches v0.2 working-memory
/// decay constants in `consolidation.rs`).
pub const RECENCY_DEFAULT_HALF_LIFE: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// ACT-R
// ---------------------------------------------------------------------------

/// Normalize an ACT-R activation level into `[0, 1]`.
///
/// Per the existing engramai ACT-R model, base-level activation is a
/// `log`-shaped quantity in roughly `[-3, 3]` for typical workloads. We
/// project that onto `[0, 1]` via a sigmoid (`1 / (1 + exp(-x))`). The
/// sigmoid is monotonic and saturating, which preserves the relative
/// ordering of memories while normalizing the magnitude.
///
/// # Errors / edge cases
///
/// - `NaN` / `Inf` input: returns `0.0` (signal absent).
pub fn actr_score(activation: f64) -> f64 {
    if activation.is_nan() {
        return 0.0;
    }
    // For ±inf the sigmoid limit is well-defined:
    //   +inf → 1.0 (saturates high)
    //   -inf → 0.0 (saturates low)
    // For finite values the closed form below is numerically stable.
    if activation == f64::INFINITY {
        return 1.0;
    }
    if activation == f64::NEG_INFINITY {
        return 0.0;
    }
    let s = 1.0 / (1.0 + (-activation).exp());
    clamp01(s)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Clamp a `f64` to `[0.0, 1.0]`, mapping `NaN` to `0.0`.
///
/// Used internally by every scorer; exported for tests / property checks
/// that want to assert the same range invariant the combiner relies on.
#[inline]
pub fn clamp01(x: f64) -> f64 {
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

    // -------- clamp01 --------

    #[test]
    fn clamp01_handles_extremes() {
        assert_eq!(clamp01(0.0), 0.0);
        assert_eq!(clamp01(1.0), 1.0);
        assert_eq!(clamp01(-0.5), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert_eq!(clamp01(f64::NAN), 0.0);
        assert_eq!(clamp01(f64::INFINITY), 1.0);
        assert_eq!(clamp01(f64::NEG_INFINITY), 0.0);
    }

    // -------- vector_score --------

    #[test]
    fn vector_score_identical_returns_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let s = vector_score(&v, &v);
        assert!((s - 1.0).abs() < 1e-10, "got {s}");
    }

    #[test]
    fn vector_score_orthogonal_returns_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert_eq!(vector_score(&a, &b), 0.0);
    }

    #[test]
    fn vector_score_anti_parallel_clamps_to_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![-1.0_f32, 0.0];
        // cosine = -1 → clamped to 0
        assert_eq!(vector_score(&a, &b), 0.0);
    }

    #[test]
    fn vector_score_dimension_mismatch_returns_zero() {
        assert_eq!(vector_score(&[1.0_f32], &[1.0_f32, 2.0]), 0.0);
    }

    #[test]
    fn vector_score_zero_norm_returns_zero() {
        assert_eq!(vector_score(&[0.0_f32; 3], &[1.0_f32; 3]), 0.0);
        assert_eq!(vector_score(&[1.0_f32; 3], &[0.0_f32; 3]), 0.0);
    }

    #[test]
    fn vector_score_nan_input_returns_zero() {
        assert_eq!(vector_score(&[f32::NAN, 1.0], &[1.0, 1.0]), 0.0);
        assert_eq!(vector_score(&[1.0, 1.0], &[f32::INFINITY, 1.0]), 0.0);
    }

    #[test]
    fn vector_score_empty_returns_zero() {
        let empty: Vec<f32> = vec![];
        assert_eq!(vector_score(&empty, &empty), 0.0);
    }

    // -------- bm25_score --------

    #[test]
    fn bm25_score_zero_returns_zero() {
        assert_eq!(bm25_score(0.0, BM25_DEFAULT_SATURATION), 0.0);
    }

    #[test]
    fn bm25_score_at_saturation_returns_one() {
        assert_eq!(bm25_score(20.0, 20.0), 1.0);
    }

    #[test]
    fn bm25_score_above_saturation_clamps() {
        assert_eq!(bm25_score(100.0, 20.0), 1.0);
    }

    #[test]
    fn bm25_score_linear_below_saturation() {
        assert!((bm25_score(10.0, 20.0) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn bm25_score_negative_returns_zero() {
        assert_eq!(bm25_score(-1.0, 20.0), 0.0);
    }

    #[test]
    fn bm25_score_invalid_saturation_returns_zero() {
        assert_eq!(bm25_score(10.0, 0.0), 0.0);
        assert_eq!(bm25_score(10.0, -5.0), 0.0);
        assert_eq!(bm25_score(10.0, f64::NAN), 0.0);
    }

    #[test]
    fn bm25_score_nan_input_returns_zero() {
        assert_eq!(bm25_score(f64::NAN, 20.0), 0.0);
        assert_eq!(bm25_score(f64::INFINITY, 20.0), 0.0);
    }

    // -------- graph_score --------

    #[test]
    fn graph_score_zero_hops_returns_one() {
        assert_eq!(graph_score(0, GRAPH_DEFAULT_DECAY), 1.0);
    }

    #[test]
    fn graph_score_default_decay_schedule() {
        let d = GRAPH_DEFAULT_DECAY;
        assert!((graph_score(1, d) - 0.5).abs() < 1e-10);
        assert!((graph_score(2, d) - 0.25).abs() < 1e-10);
        assert!((graph_score(3, d) - 0.125).abs() < 1e-10);
    }

    #[test]
    fn graph_score_zero_decay_drops_off_after_anchor() {
        assert_eq!(graph_score(0, 0.0), 1.0);
        assert_eq!(graph_score(1, 0.0), 0.0);
        assert_eq!(graph_score(5, 0.0), 0.0);
    }

    #[test]
    fn graph_score_decay_clamped_to_unit() {
        // decay > 1 would inflate scores past 1.0 — must be clamped.
        let s = graph_score(2, 1.5);
        assert!(s <= 1.0, "got {s}");
        assert!(s >= 0.0);
    }

    #[test]
    fn graph_score_negative_decay_clamped() {
        // Negative decay would alternate sign; clamping to 0 is the
        // conservative behavior.
        assert_eq!(graph_score(1, -1.0), 0.0);
    }

    // -------- recency_score --------

    #[test]
    fn recency_score_zero_age_returns_one() {
        let s = recency_score(Duration::from_secs(0), RECENCY_DEFAULT_HALF_LIFE);
        assert_eq!(s, 1.0);
    }

    #[test]
    fn recency_score_at_half_life_returns_half() {
        let s = recency_score(RECENCY_DEFAULT_HALF_LIFE, RECENCY_DEFAULT_HALF_LIFE);
        assert!((s - 0.5).abs() < 1e-10, "got {s}");
    }

    #[test]
    fn recency_score_at_two_half_lives_returns_quarter() {
        let s = recency_score(
            RECENCY_DEFAULT_HALF_LIFE * 2,
            RECENCY_DEFAULT_HALF_LIFE,
        );
        assert!((s - 0.25).abs() < 1e-10, "got {s}");
    }

    #[test]
    fn recency_score_zero_half_life_returns_zero() {
        let s = recency_score(Duration::from_secs(60), Duration::from_secs(0));
        assert_eq!(s, 0.0);
    }

    #[test]
    fn recency_score_monotonic_decreasing() {
        let half = RECENCY_DEFAULT_HALF_LIFE;
        let s1 = recency_score(Duration::from_secs(60), half);
        let s2 = recency_score(Duration::from_secs(3600), half);
        let s3 = recency_score(Duration::from_secs(86400), half);
        assert!(s1 > s2, "{s1} > {s2}");
        assert!(s2 > s3, "{s2} > {s3}");
    }

    // -------- actr_score --------

    #[test]
    fn actr_score_zero_activation_returns_half() {
        // sigmoid(0) = 0.5
        let s = actr_score(0.0);
        assert!((s - 0.5).abs() < 1e-10, "got {s}");
    }

    #[test]
    fn actr_score_high_activation_saturates_near_one() {
        let s = actr_score(10.0);
        assert!(s > 0.999, "got {s}");
        assert!(s <= 1.0);
    }

    #[test]
    fn actr_score_low_activation_saturates_near_zero() {
        let s = actr_score(-10.0);
        assert!(s < 0.001, "got {s}");
        assert!(s >= 0.0);
    }

    #[test]
    fn actr_score_monotonic() {
        let a = actr_score(-1.0);
        let b = actr_score(0.0);
        let c = actr_score(1.0);
        assert!(a < b, "{a} < {b}");
        assert!(b < c, "{b} < {c}");
    }

    #[test]
    fn actr_score_nan_returns_zero() {
        assert_eq!(actr_score(f64::NAN), 0.0);
        assert_eq!(actr_score(f64::INFINITY), 1.0); // sigmoid saturates → 1
        assert_eq!(actr_score(f64::NEG_INFINITY), 0.0);
    }

    // -------- determinism property --------

    #[test]
    fn all_scorers_deterministic() {
        // Repeat 10x — every scorer must return byte-identical f64.
        for _ in 0..10 {
            assert_eq!(vector_score(&[1.0_f32, 2.0, 3.0], &[3.0, 2.0, 1.0]), {
                vector_score(&[1.0_f32, 2.0, 3.0], &[3.0, 2.0, 1.0])
            });
            assert_eq!(bm25_score(7.0, BM25_DEFAULT_SATURATION), {
                bm25_score(7.0, BM25_DEFAULT_SATURATION)
            });
            assert_eq!(graph_score(2, GRAPH_DEFAULT_DECAY), {
                graph_score(2, GRAPH_DEFAULT_DECAY)
            });
            assert_eq!(
                recency_score(Duration::from_secs(123), RECENCY_DEFAULT_HALF_LIFE),
                recency_score(Duration::from_secs(123), RECENCY_DEFAULT_HALF_LIFE),
            );
            assert_eq!(actr_score(0.7), actr_score(0.7));
        }
    }

    #[test]
    fn all_scorers_in_unit_range() {
        // Sample the input space — every output must be in [0, 1].
        for x in [-1.0_f64, 0.0, 0.5, 1.0, 5.0, -10.0, 100.0] {
            assert!((0.0..=1.0).contains(&clamp01(x)));
            assert!((0.0..=1.0).contains(&actr_score(x)));
            assert!((0.0..=1.0).contains(&bm25_score(x.abs(), BM25_DEFAULT_SATURATION)));
        }
        for h in [0_u32, 1, 2, 3, 10, 100] {
            assert!((0.0..=1.0).contains(&graph_score(h, GRAPH_DEFAULT_DECAY)));
        }
        for s in [0_u64, 1, 60, 3600, 86400, 86400 * 30] {
            let r = recency_score(Duration::from_secs(s), RECENCY_DEFAULT_HALF_LIFE);
            assert!((0.0..=1.0).contains(&r), "got {r} for s={s}");
        }
    }
}
