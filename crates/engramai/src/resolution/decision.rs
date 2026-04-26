//! Entity resolution decision (§3.4.3) — given the fused confidences for all
//! candidates of one mention, pick exactly one of:
//!
//!   - [`Decision::MergeInto`] — confidence ≥ `merge_threshold`. The mention
//!     is the same identity as the named candidate.
//!   - [`Decision::DeferToLlm`] — `defer_threshold ≤ confidence < merge_threshold`.
//!     We're uncertain; an LLM tie-breaker (mem0-style prompt) decides.
//!   - [`Decision::CreateNew`] — confidence below `defer_threshold`, or no
//!     candidates at all. Mint a fresh `Entity` row.
//!
//! Thresholds are config (per GOAL-2.7 — "thresholds are tuning, not
//! requirements"). Defaults are the master DESIGN §8.3 starting points and
//! are intentionally conservative (high merge bar, narrow defer band).
//!
//! All logic here is pure: it consumes fused [`FusionResult`]s and produces a
//! decision + the winning [`CandidateScore`] (if any) for the trace.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::fusion::FusionResult;
use super::trace::CandidateScore;

/// Threshold pair for entity resolution decisions.
///
/// Invariant: `0.0 ≤ defer_threshold ≤ merge_threshold ≤ 1.0`. Validated at
/// construction; downstream code may rely on this without re-checking.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionThresholds {
    /// Confidence at or above this → MergeInto.
    pub merge_threshold: f64,
    /// Confidence at or above this (and below merge) → DeferToLlm.
    pub defer_threshold: f64,
}

impl Default for DecisionThresholds {
    /// Conservative defaults from master DESIGN §8.3.
    fn default() -> Self {
        Self {
            merge_threshold: 0.85,
            defer_threshold: 0.60,
        }
    }
}

impl DecisionThresholds {
    /// Construct after invariant check. Returns `Err(reason)` on violation.
    pub fn new(defer_threshold: f64, merge_threshold: f64) -> Result<Self, &'static str> {
        if !(0.0..=1.0).contains(&defer_threshold) {
            return Err("defer_threshold out of [0, 1]");
        }
        if !(0.0..=1.0).contains(&merge_threshold) {
            return Err("merge_threshold out of [0, 1]");
        }
        if defer_threshold > merge_threshold {
            return Err("defer_threshold > merge_threshold");
        }
        Ok(Self {
            defer_threshold,
            merge_threshold,
        })
    }
}

/// Outcome of a single mention's resolution.
///
/// `MergeInto(id)` and `DeferToLlm(id)` carry the *current best candidate's*
/// id — the LLM tie-breaker uses it as the proposed merge target. `CreateNew`
/// carries no id because the new entity's id will be minted at persist time
/// (§3.5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Decision {
    MergeInto { candidate_id: Uuid },
    DeferToLlm { candidate_id: Uuid },
    CreateNew,
}

impl Decision {
    /// Stable string label used in tracing spans / metric counters.
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::MergeInto { .. } => "merge_into",
            Decision::DeferToLlm { .. } => "defer_to_llm",
            Decision::CreateNew => "create_new",
        }
    }
}

/// Combined output: the decision plus all candidate score breakdowns
/// (for [`crate::resolution::trace`] persistence).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolutionOutcome {
    pub decision: Decision,
    /// All fused candidates, sorted by confidence descending. Empty when
    /// candidate retrieval returned no rows. The first entry (if any) is the
    /// candidate referenced by [`Decision::MergeInto`] / [`Decision::DeferToLlm`].
    pub scored_candidates: Vec<CandidateScore>,
}

/// Pick the resolution decision from a set of fused candidates.
///
/// Algorithm (§3.4.3):
///  1. If `candidates.is_empty()` → `CreateNew`.
///  2. Otherwise sort by confidence descending; the top candidate wins.
///  3. Compare top confidence to `merge_threshold` then `defer_threshold`.
///
/// **Tie handling.** Sort is stable on `confidence`; if two candidates tie at
/// the threshold, whichever appeared earlier in `candidates` wins. Callers
/// who care about determinism across runs should pre-sort by id (we do not
/// impose this here — the candidate retrieval order is the source of truth).
pub fn decide(thresholds: &DecisionThresholds, candidates: Vec<FusionResult>) -> ResolutionOutcome {
    if candidates.is_empty() {
        return ResolutionOutcome {
            decision: Decision::CreateNew,
            scored_candidates: Vec::new(),
        };
    }

    // Sort descending by confidence; stable so input order breaks ties.
    let mut sorted: Vec<FusionResult> = candidates;
    sorted.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let top = &sorted[0];
    let decision = if top.confidence >= thresholds.merge_threshold {
        Decision::MergeInto {
            candidate_id: top.score.candidate_id,
        }
    } else if top.confidence >= thresholds.defer_threshold {
        Decision::DeferToLlm {
            candidate_id: top.score.candidate_id,
        }
    } else {
        Decision::CreateNew
    };

    let scored_candidates: Vec<CandidateScore> = sorted.into_iter().map(|f| f.score).collect();
    ResolutionOutcome {
        decision,
        scored_candidates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::trace::CandidateScore;

    fn make_fusion(id: Uuid, conf: f64) -> FusionResult {
        FusionResult {
            confidence: conf,
            score: CandidateScore {
                candidate_id: id,
                confidence: conf,
                contributions: Vec::new(),
                signals_missing: Vec::new(),
            },
        }
    }

    // ----- threshold invariants -----
    #[test]
    fn thresholds_default_valid() {
        let t = DecisionThresholds::default();
        assert!(t.defer_threshold <= t.merge_threshold);
        assert!(DecisionThresholds::new(t.defer_threshold, t.merge_threshold).is_ok());
    }

    #[test]
    fn thresholds_reject_out_of_range() {
        assert!(DecisionThresholds::new(-0.1, 0.5).is_err());
        assert!(DecisionThresholds::new(0.5, 1.1).is_err());
    }

    #[test]
    fn thresholds_reject_inverted_order() {
        // defer > merge is illegal.
        assert!(DecisionThresholds::new(0.9, 0.5).is_err());
    }

    #[test]
    fn thresholds_allow_equal_bounds() {
        // defer == merge collapses the "defer" band but is legal config.
        assert!(DecisionThresholds::new(0.7, 0.7).is_ok());
    }

    // ----- decision: empty -----
    #[test]
    fn decide_no_candidates_returns_create_new() {
        let out = decide(&DecisionThresholds::default(), Vec::new());
        assert_eq!(out.decision, Decision::CreateNew);
        assert!(out.scored_candidates.is_empty());
    }

    // ----- decision: above merge threshold -----
    #[test]
    fn decide_high_conf_merges() {
        let id = Uuid::new_v4();
        let out = decide(
            &DecisionThresholds::default(),
            vec![make_fusion(id, 0.9)],
        );
        assert_eq!(out.decision, Decision::MergeInto { candidate_id: id });
        assert_eq!(out.scored_candidates.len(), 1);
    }

    // ----- decision: in defer band -----
    #[test]
    fn decide_mid_conf_defers() {
        let id = Uuid::new_v4();
        let out = decide(
            &DecisionThresholds::default(),
            vec![make_fusion(id, 0.7)],
        );
        assert_eq!(out.decision, Decision::DeferToLlm { candidate_id: id });
    }

    // ----- decision: below defer threshold -----
    #[test]
    fn decide_low_conf_creates_new() {
        let id = Uuid::new_v4();
        let out = decide(
            &DecisionThresholds::default(),
            vec![make_fusion(id, 0.3)],
        );
        assert_eq!(out.decision, Decision::CreateNew);
        // Even on CreateNew we still return scored candidates for trace.
        assert_eq!(out.scored_candidates.len(), 1);
    }

    // ----- decision: top candidate wins -----
    #[test]
    fn decide_picks_highest_confidence() {
        let weak = Uuid::new_v4();
        let strong = Uuid::new_v4();
        let out = decide(
            &DecisionThresholds::default(),
            vec![make_fusion(weak, 0.4), make_fusion(strong, 0.95)],
        );
        assert_eq!(
            out.decision,
            Decision::MergeInto {
                candidate_id: strong
            }
        );
        // Sorted descending → strong is first in the trace.
        assert_eq!(out.scored_candidates[0].candidate_id, strong);
        assert_eq!(out.scored_candidates[1].candidate_id, weak);
    }

    // ----- decision: boundary equality (merge) -----
    #[test]
    fn decide_at_merge_threshold_inclusive() {
        let id = Uuid::new_v4();
        let t = DecisionThresholds::new(0.6, 0.85).unwrap();
        let out = decide(&t, vec![make_fusion(id, 0.85)]);
        // ≥ merge_threshold → MergeInto
        assert_eq!(out.decision, Decision::MergeInto { candidate_id: id });
    }

    // ----- decision: boundary equality (defer) -----
    #[test]
    fn decide_at_defer_threshold_inclusive() {
        let id = Uuid::new_v4();
        let t = DecisionThresholds::new(0.6, 0.85).unwrap();
        let out = decide(&t, vec![make_fusion(id, 0.6)]);
        // ≥ defer_threshold but < merge_threshold → DeferToLlm
        assert_eq!(out.decision, Decision::DeferToLlm { candidate_id: id });
    }

    // ----- decision: collapsed band (defer == merge) -----
    #[test]
    fn decide_collapsed_band_skips_defer() {
        let id = Uuid::new_v4();
        let t = DecisionThresholds::new(0.7, 0.7).unwrap();
        // At threshold → MergeInto (since ≥ merge_threshold check runs first).
        let out = decide(&t, vec![make_fusion(id, 0.7)]);
        assert_eq!(out.decision, Decision::MergeInto { candidate_id: id });

        // Just below → CreateNew (defer band has zero width).
        let out = decide(&t, vec![make_fusion(id, 0.69)]);
        assert_eq!(out.decision, Decision::CreateNew);
    }

    // ----- decision: NaN confidence is treated as low -----
    #[test]
    fn decide_nan_confidence_falls_through_to_create() {
        let id = Uuid::new_v4();
        let out = decide(
            &DecisionThresholds::default(),
            vec![make_fusion(id, f64::NAN)],
        );
        // NaN comparisons return None → ordering treated as Equal → top is the
        // NaN candidate. NaN ≥ 0.85 is false (NaN compared to anything is
        // false), so we fall through CreateNew.
        assert_eq!(out.decision, Decision::CreateNew);
    }

    // ----- decision label string -----
    #[test]
    fn decision_as_str_stable() {
        assert_eq!(Decision::CreateNew.as_str(), "create_new");
        assert_eq!(
            Decision::MergeInto {
                candidate_id: Uuid::nil()
            }
            .as_str(),
            "merge_into"
        );
        assert_eq!(
            Decision::DeferToLlm {
                candidate_id: Uuid::nil()
            }
            .as_str(),
            "defer_to_llm"
        );
    }
}
