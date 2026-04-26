//! Resolution trace — per-mention observability record produced by §3.4.
//!
//! Every fused candidate contributes one [`CandidateScore`] to the per-mention
//! trace. The full trace is later persisted alongside the memory (§7) and is
//! the audit source for "why did this mention resolve to entity X?"
//!
//! Design rules:
//!  - **Per-signal contributions are individually observable** (GOAL-2.6,
//!    GOAL-2.8). Every signal's value, weight (after redistribution), and
//!    contribution are stored.
//!  - **Missing signals are recorded** (GUARD-2 — never silent). The
//!    `signals_missing` set lists which signals were absent for this fusion.
//!  - The trace is `Serialize` so it can be persisted as JSON. It is also
//!    `Clone` because tests and observability consumers may both need a copy.
//!
//! See `.gid/features/v03-resolution/design.md` §3.4.2 / §7.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::signals::Signal;

/// Score breakdown for a single (mention, candidate) pair after fusion.
///
/// `confidence` is the final fused score in `[0.0, 1.0]`. The `contributions`
/// map decomposes that score into per-signal pieces such that
/// `contributions.values().sum() ≈ confidence` (within float epsilon, ignoring
/// signals that were missing). Each contribution is `weight_after × value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateScore {
    pub candidate_id: Uuid,
    pub confidence: f64,
    /// One entry per signal that *participated* in fusion. Missing signals
    /// are absent here and present in `signals_missing` instead.
    pub contributions: Vec<SignalContribution>,
    /// Signals that had no input data and were dropped from fusion (their
    /// weight was redistributed to present signals — see §3.4.2).
    pub signals_missing: Vec<Signal>,
}

/// One signal's contribution to the final confidence score.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalContribution {
    pub signal: Signal,
    /// Raw signal value in `[0.0, 1.0]` produced by the scorer.
    pub value: f64,
    /// Effective weight after missing-signal redistribution. Sums to 1.0
    /// across all present signals for a fusion run.
    pub weight: f64,
    /// `weight × value` — the additive contribution to `confidence`.
    pub contribution: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_score_round_trip_json() {
        let s = CandidateScore {
            candidate_id: Uuid::new_v4(),
            confidence: 0.82,
            contributions: vec![
                SignalContribution {
                    signal: Signal::NameMatch,
                    value: 0.9,
                    weight: 0.5,
                    contribution: 0.45,
                },
                SignalContribution {
                    signal: Signal::Recency,
                    value: 0.74,
                    weight: 0.5,
                    contribution: 0.37,
                },
            ],
            signals_missing: vec![Signal::SemanticSimilarity, Signal::Cooccurrence],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: CandidateScore = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn contribution_equals_weight_times_value() {
        let c = SignalContribution {
            signal: Signal::NameMatch,
            value: 0.8,
            weight: 0.25,
            contribution: 0.2,
        };
        // Document the intended invariant — actual enforcement is in fusion.rs.
        assert!((c.weight * c.value - c.contribution).abs() < 1e-9);
    }
}
