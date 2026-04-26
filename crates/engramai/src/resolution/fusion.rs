//! Multi-signal fusion (§3.4.2) — combines per-signal scores into a single
//! confidence value with **proportional weight redistribution** for missing
//! signals.
//!
//! The fusion contract (per design §3.4.2 / GOAL-2.11 / GUARD-4):
//!  - Inputs: a per-signal weight map (sums to ~1.0) and a set of present
//!    `(Signal, value)` measurements.
//!  - For missing signals, redistribute their weight proportionally across
//!    *present* signals — preserving the relative importance ordering. This
//!    is `w_i' = w_i / (1 - sum_missing)`.
//!  - All-missing (degenerate) inputs return `confidence = 0.0` with
//!    `signals_missing = ALL` so the caller can decide CreateNew + record a
//!    `NoFusionSignals` failure.
//!
//! All functions are **pure**: no IO, deterministic, total. They are the
//! arithmetic core of resolution and must remain testable without a DB.
//!
//! See `.gid/features/v03-resolution/design.md` §3.4.2.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::signals::Signal;
use super::trace::{CandidateScore, SignalContribution};

/// Per-signal weight configuration. Weights MUST sum to ~1.0 (validated by
/// [`SignalWeights::validate`]).
///
/// Default values are master DESIGN §8.3 initial guesses, intentionally
/// favoring `NameMatch` and `SemanticSimilarity` while giving every other
/// signal a meaningful share. Tune via config; do not hardcode at call sites.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalWeights {
    pub semantic_similarity: f64,
    pub name_match: f64,
    pub graph_context: f64,
    pub recency: f64,
    pub cooccurrence: f64,
    pub affective_continuity: f64,
    pub identity_hint: f64,
    pub somatic_match: f64,
}

impl Default for SignalWeights {
    /// Initial weight guesses (master DESIGN §8.3). Sum = 1.00.
    fn default() -> Self {
        Self {
            semantic_similarity: 0.25,
            name_match: 0.30,
            graph_context: 0.10,
            recency: 0.05,
            cooccurrence: 0.10,
            affective_continuity: 0.05,
            identity_hint: 0.10,
            somatic_match: 0.05,
        }
    }
}

impl SignalWeights {
    /// Look up the weight for a signal.
    pub fn get(&self, s: Signal) -> f64 {
        match s {
            Signal::SemanticSimilarity => self.semantic_similarity,
            Signal::NameMatch => self.name_match,
            Signal::GraphContext => self.graph_context,
            Signal::Recency => self.recency,
            Signal::Cooccurrence => self.cooccurrence,
            Signal::AffectiveContinuity => self.affective_continuity,
            Signal::IdentityHint => self.identity_hint,
            Signal::SomaticMatch => self.somatic_match,
        }
    }

    /// Total weight (should be ~1.0 after construction).
    pub fn sum(&self) -> f64 {
        Signal::ALL.iter().map(|s| self.get(*s)).sum()
    }

    /// Validate that weights are non-negative and sum to 1.0 within `±tol`.
    /// Returns the offending sum on failure for diagnostics.
    pub fn validate(&self, tol: f64) -> Result<(), f64> {
        for s in Signal::ALL {
            if self.get(s) < 0.0 {
                return Err(self.get(s));
            }
        }
        let sum = self.sum();
        if (sum - 1.0).abs() > tol {
            return Err(sum);
        }
        Ok(())
    }
}

/// One present-signal measurement: signal id + value in `[0, 1]`.
///
/// The fusion driver builds these from the per-signal scorers in
/// `super::signals`. Only `Some(value)` results become `Measurement`s; `None`
/// results become "missing" and trigger weight redistribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub signal: Signal,
    pub value: f64,
}

/// Result of fusing one (mention, candidate) pair: the confidence and the
/// fully-decomposed score breakdown for the trace.
#[derive(Clone, Debug, PartialEq)]
pub struct FusionResult {
    pub confidence: f64,
    pub score: CandidateScore,
}

/// Fuse measurements into a single confidence + per-signal contribution
/// breakdown.
///
/// Algorithm (§3.4.2):
/// 1. Compute `sum_missing = sum of w_i for signals not present`.
/// 2. If `sum_missing ≈ 1.0` (all signals missing), return zero-confidence
///    with all signals listed missing.
/// 3. Otherwise scale present weights by `1 / (1 - sum_missing)` so they sum
///    to 1.0.
/// 4. Confidence = sum(scaled_weight × value) for present signals.
///
/// Inputs are deduplicated by signal — if the same `Signal` appears twice in
/// `measurements`, the *first* occurrence wins (we are deterministic; callers
/// should not pass duplicates and we do not silently average them).
pub fn fuse(
    candidate_id: Uuid,
    weights: &SignalWeights,
    measurements: &[Measurement],
) -> FusionResult {
    // Deduplicate measurements by signal: first wins.
    let mut by_signal: HashMap<Signal, f64> = HashMap::with_capacity(8);
    for m in measurements {
        by_signal.entry(m.signal).or_insert(m.value);
    }

    // Partition signals into present / missing.
    let mut present: Vec<(Signal, f64)> = Vec::new();
    let mut missing: Vec<Signal> = Vec::new();
    for s in Signal::ALL {
        if let Some(v) = by_signal.get(&s) {
            present.push((s, *v));
        } else {
            missing.push(s);
        }
    }

    // Degenerate: nothing measured.
    if present.is_empty() {
        return FusionResult {
            confidence: 0.0,
            score: CandidateScore {
                candidate_id,
                confidence: 0.0,
                contributions: Vec::new(),
                signals_missing: missing,
            },
        };
    }

    // Compute redistribution scaling.
    let sum_missing: f64 = missing.iter().map(|s| weights.get(*s)).sum();
    // Guard: if all weight is in missing signals (e.g., user set
    // present-signal weights to 0), fall back to uniform over present.
    let scale = if (1.0 - sum_missing).abs() < 1e-12 {
        f64::NAN
    } else {
        1.0 / (1.0 - sum_missing)
    };
    let n_present = present.len() as f64;

    let mut contributions: Vec<SignalContribution> = Vec::with_capacity(present.len());
    let mut confidence = 0.0_f64;
    for (s, v) in present {
        let raw_w = weights.get(s);
        let w = if scale.is_nan() {
            // Degenerate: present weights all zero. Use uniform fallback so
            // we still produce a meaningful score from the values alone.
            1.0 / n_present
        } else {
            raw_w * scale
        };
        let contribution = w * v;
        confidence += contribution;
        contributions.push(SignalContribution {
            signal: s,
            value: v,
            weight: w,
            contribution,
        });
    }

    // Clamp confidence to [0, 1] — should already be by construction, but
    // guard against float accumulation pushing 1.0000000002.
    let confidence = confidence.clamp(0.0, 1.0);
    FusionResult {
        confidence,
        score: CandidateScore {
            candidate_id,
            confidence,
            contributions,
            signals_missing: missing,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ----- weight defaults -----
    #[test]
    fn default_weights_sum_to_one() {
        let w = SignalWeights::default();
        assert!(approx(w.sum(), 1.0), "default sum = {}", w.sum());
        assert!(w.validate(1e-9).is_ok());
    }

    #[test]
    fn validate_rejects_negative_weight() {
        let w = SignalWeights {
            recency: -0.05,
            ..SignalWeights::default()
        };
        assert!(w.validate(1e-9).is_err());
    }

    #[test]
    fn validate_rejects_non_unit_sum() {
        let w = SignalWeights {
            recency: 0.5, // pushes sum way over 1.0
            ..SignalWeights::default()
        };
        assert!(w.validate(1e-9).is_err());
    }

    // ----- fusion: all present -----
    #[test]
    fn fuse_all_present_uses_raw_weights() {
        let w = SignalWeights::default();
        let id = Uuid::new_v4();
        let measurements: Vec<Measurement> = Signal::ALL
            .iter()
            .map(|s| Measurement {
                signal: *s,
                value: 1.0,
            })
            .collect();
        let r = fuse(id, &w, &measurements);
        // All values = 1.0 → confidence should equal the weight sum (= 1.0).
        assert!(approx(r.confidence, 1.0));
        assert!(r.score.signals_missing.is_empty());
        assert_eq!(r.score.contributions.len(), 8);
        // Each contribution.weight should equal raw weight (no redistribution).
        for c in &r.score.contributions {
            assert!(approx(c.weight, w.get(c.signal)));
            assert!(approx(c.contribution, c.weight * c.value));
        }
    }

    // ----- fusion: redistribution -----
    #[test]
    fn fuse_missing_signals_redistribute_proportionally() {
        let w = SignalWeights::default();
        // Provide only NameMatch and Recency. Their raw weights are 0.30 and 0.05
        // → sum_present_raw = 0.35, sum_missing = 0.65, scale = 1/0.35 ≈ 2.857.
        let id = Uuid::new_v4();
        let measurements = vec![
            Measurement {
                signal: Signal::NameMatch,
                value: 1.0,
            },
            Measurement {
                signal: Signal::Recency,
                value: 1.0,
            },
        ];
        let r = fuse(id, &w, &measurements);
        // After redistribution, scaled weights sum to 1.0.
        let weight_sum: f64 = r.score.contributions.iter().map(|c| c.weight).sum();
        assert!(approx(weight_sum, 1.0));
        // Confidence = 1.0 (all values = 1.0 and weights now sum to 1).
        assert!(approx(r.confidence, 1.0));
        // Six signals are recorded missing.
        assert_eq!(r.score.signals_missing.len(), 6);

        // Proportional check: name_match share / recency share = 0.30 / 0.05 = 6.
        let name_w = r
            .score
            .contributions
            .iter()
            .find(|c| c.signal == Signal::NameMatch)
            .unwrap()
            .weight;
        let rec_w = r
            .score
            .contributions
            .iter()
            .find(|c| c.signal == Signal::Recency)
            .unwrap()
            .weight;
        assert!(approx(name_w / rec_w, 6.0));
    }

    // ----- fusion: zero values -----
    #[test]
    fn fuse_zero_values_yield_zero_confidence() {
        let w = SignalWeights::default();
        let id = Uuid::new_v4();
        let measurements: Vec<Measurement> = Signal::ALL
            .iter()
            .map(|s| Measurement {
                signal: *s,
                value: 0.0,
            })
            .collect();
        let r = fuse(id, &w, &measurements);
        assert!(approx(r.confidence, 0.0));
        assert!(r.score.signals_missing.is_empty());
    }

    // ----- fusion: degenerate (no measurements) -----
    #[test]
    fn fuse_no_measurements_returns_zero_with_all_missing() {
        let w = SignalWeights::default();
        let id = Uuid::new_v4();
        let r = fuse(id, &w, &[]);
        assert_eq!(r.confidence, 0.0);
        assert_eq!(r.score.signals_missing.len(), 8);
        assert!(r.score.contributions.is_empty());
    }

    // ----- fusion: duplicate measurement deduplication -----
    #[test]
    fn fuse_duplicate_signal_first_wins() {
        let w = SignalWeights::default();
        let id = Uuid::new_v4();
        let measurements = vec![
            Measurement {
                signal: Signal::NameMatch,
                value: 1.0,
            },
            Measurement {
                signal: Signal::NameMatch,
                value: 0.0, // ignored
            },
        ];
        let r = fuse(id, &w, &measurements);
        let nm = r
            .score
            .contributions
            .iter()
            .find(|c| c.signal == Signal::NameMatch)
            .unwrap();
        assert_eq!(nm.value, 1.0);
    }

    // ----- fusion: contribution sums to confidence -----
    #[test]
    fn fuse_contributions_sum_to_confidence() {
        let w = SignalWeights::default();
        let id = Uuid::new_v4();
        let measurements = vec![
            Measurement {
                signal: Signal::NameMatch,
                value: 0.8,
            },
            Measurement {
                signal: Signal::Recency,
                value: 0.4,
            },
            Measurement {
                signal: Signal::SomaticMatch,
                value: 0.6,
            },
        ];
        let r = fuse(id, &w, &measurements);
        let summed: f64 = r.score.contributions.iter().map(|c| c.contribution).sum();
        assert!(
            approx(summed, r.confidence),
            "contributions sum {summed} vs confidence {}",
            r.confidence
        );
    }

    // ----- fusion: all-zero present weights (pathological config) -----
    #[test]
    fn fuse_zero_present_weights_falls_back_to_uniform() {
        // Pathological config: NameMatch weight = 1.0, all others = 0.0.
        // Provide only Recency (raw weight 0.0). Without the fallback we'd
        // divide by zero. With the fallback we score uniformly over present.
        let mut w = SignalWeights {
            semantic_similarity: 0.0,
            name_match: 1.0,
            graph_context: 0.0,
            recency: 0.0,
            cooccurrence: 0.0,
            affective_continuity: 0.0,
            identity_hint: 0.0,
            somatic_match: 0.0,
        };
        // sanity: validate sum = 1.0
        assert!(w.validate(1e-9).is_ok());
        let _ = &mut w; // suppress mut warning

        let id = Uuid::new_v4();
        let measurements = vec![Measurement {
            signal: Signal::Recency,
            value: 0.7,
        }];
        let r = fuse(id, &w, &measurements);
        // Uniform fallback over 1 present signal → weight 1.0 → confidence = value.
        assert!(approx(r.confidence, 0.7));
        assert_eq!(r.score.contributions.len(), 1);
        assert!(approx(r.score.contributions[0].weight, 1.0));
    }
}
