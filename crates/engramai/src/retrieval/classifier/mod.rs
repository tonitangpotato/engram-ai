//! # Query intent classifier (v0.3)
//!
//! Two-stage classifier per design §3.2 (heuristic-first, LLM fallback).
//!
//! - [`heuristic`] — Stage-1 pure-function signal scorers
//!   (`task:retr-impl-classifier-heuristic`). No IO, no LLM, sub-millisecond.
//! - Stage-2 LLM fallback (`task:retr-impl-classifier-llm`) is implemented
//!   in `llm_fallback` (sibling task; not yet present at the time this file
//!   was authored — the orchestrator below stops at Stage 1 and signals
//!   `Stage1Outcome::NeedsLlmFallback` so the LLM stage can plug in via the
//!   trait abstraction without refactoring this file).
//!
//! Design refs: `.gid/features/v03-retrieval/design.md` §3.1 (intent
//! categories), §3.2 (classifier design), §3.3 (caller override),
//! §3.4 (fallback / total classifier).

pub mod heuristic;
pub mod llm_fallback;

use std::sync::Arc;

use heuristic::{EntityLookup, NullEntityLookup, SignalKind, SignalScores};

// ---------------------------------------------------------------------------
// Intent — the 5-variant taxonomy from §3.1
// ---------------------------------------------------------------------------

/// The five query-intent categories from design §3.1.
///
/// `Associative` is intentionally **not** an `Intent` variant: per §3.1 it is
/// a *plan-kind* (§4.3) reached when the classifier emits `Intent::Factual`
/// with a `downgrade_hint = Associative`, or when any concrete plan downgrades
/// at execution time. Trying to add it here would violate the "exactly 5
/// intents" rule and is what burned the v0.2 query_classifier — the
/// downgrade lattice goes via `RetrievalOutcome::Downgraded*` (§6.4), not by
/// changing the `Intent` value.
///
/// `Hybrid` fires only when ≥ 2 primary signals are simultaneously
/// high-confidence — it is **not** a low-confidence fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intent {
    /// Anchor-entity ranking; semantic-graph grounded with bi-temporal
    /// validity. (§4.1)
    Factual,
    /// Time-window-bounded source-memory recall, optional `as-of-T`
    /// projection. (§4.2)
    Episodic,
    /// L5 Knowledge-Topic synthesis with traceability. (§4.4)
    Abstract,
    /// Mood-congruent recall; modulates ranking, never gates. (§4.5)
    Affective,
    /// ≥ 2 strong primary signals → cross-layer fusion (§4.7).
    Hybrid,
}

impl Intent {
    /// Stable string form for logging / metrics (used by `classifier_method`
    /// observability hooks per GOAL-3.2).
    pub fn as_str(&self) -> &'static str {
        match self {
            Intent::Factual => "factual",
            Intent::Episodic => "episodic",
            Intent::Abstract => "abstract",
            Intent::Affective => "affective",
            Intent::Hybrid => "hybrid",
        }
    }
}

/// Provenance of the chosen intent, surfaced in `PlanTrace.classifier`
/// (GOAL-3.2 — `classifier_method` MUST be observable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierMethod {
    /// Stage 1 was sufficient.
    Heuristic,
    /// Stage 1 was ambiguous → Stage 2 LLM fallback was consulted.
    LlmFallback,
    /// Stage 2 LLM fallback timed out or the budget was exhausted; we used
    /// the heuristic best-guess (default `Associative` plan via Factual + hint).
    HeuristicTimeout,
    /// `GraphQuery.intent = Some(_)` — both stages bypassed (§3.3).
    CallerOverride,
}

/// When the heuristic routing rule produces a single plan, the orchestrator
/// may still want to nudge the executor toward a downgrade target (e.g.,
/// `Factual` with **no strong signals** → run the Associative plan, but
/// still report `plan_used = Factual` per §3.1's "exactly 5 intents"
/// invariant). `DowngradeHint::Associative` carries that nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DowngradeHint {
    /// No downgrade — execute the plan corresponding to `intent`.
    None,
    /// Caller should materialize the Associative plan (§4.3).
    Associative,
}

/// Stage-1 routing outcome.
///
/// The orchestrator consumes this to decide whether to invoke Stage 2 (LLM
/// fallback) or short-circuit. Tests on the heuristic stage can assert routing
/// correctness against this enum without needing an LLM in the loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage1Outcome {
    /// Stage 1 produced a confident decision. The orchestrator returns this
    /// directly (`method = Heuristic`).
    Decided {
        intent: Intent,
        downgrade_hint: DowngradeHint,
    },
    /// Stage 1 is ambiguous — at least one signal is weak-but-present.
    /// Orchestrator should consult Stage 2 (`method = LlmFallback` if it
    /// returns in time, else `HeuristicTimeout`).
    NeedsLlmFallback {
        /// Best-guess intent if Stage 2 is unavailable. Always defined per
        /// §3.4 (classifier is total).
        heuristic_best_guess: Intent,
        /// Downgrade hint paired with the best guess (mirrors the `Decided`
        /// variant's pairing).
        downgrade_hint: DowngradeHint,
    },
}

// ---------------------------------------------------------------------------
// Configuration thresholds (defaults from design §3.2)
// ---------------------------------------------------------------------------

/// Per-signal strength threshold (`threshold_s`). For binary signals (which
/// this stage emits today) only `1.0` clears `>= 1.0`, so the threshold is
/// really only meaningful for the entity signal (which is graded). Default
/// matches §3.2: entity ≥ `0.7`, others ≥ `1.0`.
#[derive(Debug, Clone, Copy)]
pub struct SignalThresholds {
    pub entity: f64,
    pub temporal: f64,
    pub abstract_: f64,
    pub affective: f64,
    /// `τ_high` — "high-confidence" cutoff for promoting to Hybrid or
    /// single-intent plan.
    pub tau_high: f64,
}

impl Default for SignalThresholds {
    fn default() -> Self {
        Self {
            entity: 0.7,
            temporal: 1.0,
            abstract_: 1.0,
            affective: 1.0,
            tau_high: 0.7,
        }
    }
}

impl SignalThresholds {
    fn for_signal(&self, kind: SignalKind) -> f64 {
        match kind {
            SignalKind::Entity => self.entity,
            SignalKind::Temporal => self.temporal,
            SignalKind::Abstract => self.abstract_,
            SignalKind::Affective => self.affective,
        }
    }
}

// ---------------------------------------------------------------------------
// Orchestrator scaffolding
// ---------------------------------------------------------------------------

/// Stage-1 (heuristic) classifier orchestrator.
///
/// Holds the pluggable [`EntityLookup`] and threshold config, exposes a single
/// [`classify_stage1`](Self::classify_stage1) entry point. The full classifier
/// (with Stage 2 LLM fallback wired in) is built on top of this in the
/// `task:retr-impl-classifier-core` task — that task only needs to add the
/// LLM client + budget, and decide what to do with `Stage1Outcome::NeedsLlmFallback`.
#[derive(Clone)]
pub struct HeuristicClassifier {
    entity_lookup: Arc<dyn EntityLookup>,
    thresholds: SignalThresholds,
}

impl HeuristicClassifier {
    /// Build with explicit entity lookup + thresholds.
    pub fn new(entity_lookup: Arc<dyn EntityLookup>, thresholds: SignalThresholds) -> Self {
        Self {
            entity_lookup,
            thresholds,
        }
    }

    /// Build with the null entity lookup (no graph populated). Useful for
    /// tests and for v0.2 databases where the v0.3 graph hasn't been
    /// backfilled yet — the entity signal will always be `0.0`.
    pub fn with_null_lookup() -> Self {
        Self::new(Arc::new(NullEntityLookup), SignalThresholds::default())
    }

    /// Score a query and apply the §3.2 stage-1 routing rule.
    ///
    /// Returns the per-signal scores **and** the routing decision so the
    /// caller can populate `ClassifierTrace` (§6.3) without recomputing.
    pub fn classify_stage1(&self, query: &str) -> (SignalScores, Stage1Outcome) {
        let scores = heuristic::score_all(query, self.entity_lookup.as_ref());
        let outcome = route_stage1(&scores, &self.thresholds);
        (scores, outcome)
    }
}

impl Default for HeuristicClassifier {
    fn default() -> Self {
        Self::with_null_lookup()
    }
}

/// Apply the §3.2 stage-1 routing rule to the four primary signal scores.
///
/// Implementation of (verbatim from design):
///
/// ```text
/// strong_signals = {s for s in {entity, temporal, abstract, affective}
///                     if s.score >= threshold_s}
///
/// if |strong_signals| == 0:
///     -> Intent::Factual with downgrade_hint = Associative
/// elif |strong_signals| == 1 and confidence >= tau_high:
///     -> single-intent plan (Factual/Episodic/Abstract/Affective)
/// elif |strong_signals| >= 2 and each >= tau_high:
///     -> Hybrid (§4.7)
/// else:
///     -> Stage 2 (LLM fallback)
/// ```
pub fn route_stage1(scores: &SignalScores, thresholds: &SignalThresholds) -> Stage1Outcome {
    // Build the strong_signals set. A signal is "strong" iff its score is
    // ≥ its per-kind threshold. Entity uses 0.7 (graded); others use 1.0
    // (binary today, but threshold path keeps the door open for graded
    // futures without an API break).
    let strong: Vec<(SignalKind, f64)> = scores
        .primary()
        .into_iter()
        .filter(|(kind, score)| *score >= thresholds.for_signal(*kind))
        .collect();

    match strong.len() {
        // No strong signal → Factual with Associative downgrade hint.
        // The plan builder (§4.3) materializes the Associative plan.
        0 => Stage1Outcome::Decided {
            intent: Intent::Factual,
            downgrade_hint: DowngradeHint::Associative,
        },

        // Exactly one strong signal — check if it crosses τ_high.
        1 => {
            let (kind, score) = strong[0];
            if score >= thresholds.tau_high {
                Stage1Outcome::Decided {
                    intent: intent_for(kind),
                    downgrade_hint: DowngradeHint::None,
                }
            } else {
                // Weak-but-present → ambiguous → LLM fallback.
                // Best guess if LLM is unavailable: the single signal we have.
                Stage1Outcome::NeedsLlmFallback {
                    heuristic_best_guess: intent_for(kind),
                    downgrade_hint: DowngradeHint::None,
                }
            }
        }

        // ≥ 2 strong signals — promote to Hybrid only if all of them clear
        // τ_high. Otherwise we have a "mixed strength" situation per §3.2,
        // which is the canonical LLM-fallback trigger.
        _ => {
            let all_high = strong.iter().all(|(_, s)| *s >= thresholds.tau_high);
            if all_high {
                Stage1Outcome::Decided {
                    intent: Intent::Hybrid,
                    downgrade_hint: DowngradeHint::None,
                }
            } else {
                // Best guess: the strongest single signal among the strong set.
                // Stable tie-break: order matches `SignalScores::primary()`
                // (Entity, Temporal, Abstract, Affective).
                let best = strong
                    .iter()
                    .copied()
                    .max_by(|a, b| {
                        a.1.partial_cmp(&b.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .expect("strong is non-empty");
                Stage1Outcome::NeedsLlmFallback {
                    heuristic_best_guess: intent_for(best.0),
                    downgrade_hint: DowngradeHint::None,
                }
            }
        }
    }
}

/// Map a primary signal to the single-intent plan it implies.
///
/// `Entity` → Factual, `Temporal` → Episodic, `Abstract` → Abstract,
/// `Affective` → Affective. (Matches §3.1's "Mapping to DESIGN-v0.3 terms".)
fn intent_for(kind: SignalKind) -> Intent {
    match kind {
        SignalKind::Entity => Intent::Factual,
        SignalKind::Temporal => Intent::Episodic,
        SignalKind::Abstract => Intent::Abstract,
        SignalKind::Affective => Intent::Affective,
    }
}

// ---------------------------------------------------------------------------
// Tests for the routing rule
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> SignalThresholds {
        SignalThresholds::default()
    }

    #[test]
    fn no_signals_routes_to_factual_with_associative_hint() {
        let s = SignalScores::from_primary(0.0, 0.0, 0.0, 0.0);
        match route_stage1(&s, &t()) {
            Stage1Outcome::Decided {
                intent: Intent::Factual,
                downgrade_hint: DowngradeHint::Associative,
            } => {}
            other => panic!("expected Factual+Associative, got {other:?}"),
        }
    }

    #[test]
    fn single_strong_temporal_routes_to_episodic() {
        let s = SignalScores::from_primary(0.0, 1.0, 0.0, 0.0);
        match route_stage1(&s, &t()) {
            Stage1Outcome::Decided {
                intent: Intent::Episodic,
                downgrade_hint: DowngradeHint::None,
            } => {}
            other => panic!("expected Episodic, got {other:?}"),
        }
    }

    #[test]
    fn single_strong_entity_routes_to_factual() {
        let s = SignalScores::from_primary(1.0, 0.0, 0.0, 0.0);
        match route_stage1(&s, &t()) {
            Stage1Outcome::Decided {
                intent: Intent::Factual,
                downgrade_hint: DowngradeHint::None,
            } => {}
            other => panic!("expected Factual, got {other:?}"),
        }
    }

    #[test]
    fn single_strong_abstract_routes_to_abstract() {
        let s = SignalScores::from_primary(0.0, 0.0, 1.0, 0.0);
        assert!(matches!(
            route_stage1(&s, &t()),
            Stage1Outcome::Decided {
                intent: Intent::Abstract,
                ..
            }
        ));
    }

    #[test]
    fn single_strong_affective_routes_to_affective() {
        let s = SignalScores::from_primary(0.0, 0.0, 0.0, 1.0);
        assert!(matches!(
            route_stage1(&s, &t()),
            Stage1Outcome::Decided {
                intent: Intent::Affective,
                ..
            }
        ));
    }

    #[test]
    fn two_strong_signals_high_confidence_routes_to_hybrid() {
        // Entity + Temporal both strong + ≥ τ_high
        let s = SignalScores::from_primary(1.0, 1.0, 0.0, 0.0);
        assert!(matches!(
            route_stage1(&s, &t()),
            Stage1Outcome::Decided {
                intent: Intent::Hybrid,
                ..
            }
        ));
    }

    #[test]
    fn entity_below_threshold_falls_to_no_strong_branch() {
        // Entity 0.5 < threshold 0.7 → not in strong → "no signals" → Factual+Assoc
        let s = SignalScores::from_primary(0.5, 0.0, 0.0, 0.0);
        match route_stage1(&s, &t()) {
            Stage1Outcome::Decided {
                intent: Intent::Factual,
                downgrade_hint: DowngradeHint::Associative,
            } => {}
            other => panic!("expected Factual+Associative, got {other:?}"),
        }
    }

    #[test]
    fn entity_alias_strength_exactly_at_threshold_is_strong() {
        // Entity 0.8 (alias) ≥ 0.7 threshold AND ≥ τ_high 0.7 → Factual.
        let s = SignalScores::from_primary(0.8, 0.0, 0.0, 0.0);
        match route_stage1(&s, &t()) {
            Stage1Outcome::Decided {
                intent: Intent::Factual,
                downgrade_hint: DowngradeHint::None,
            } => {}
            other => panic!("expected Factual, got {other:?}"),
        }
    }

    #[test]
    fn classifier_orchestrator_smoke() {
        let c = HeuristicClassifier::with_null_lookup();
        let (scores, outcome) = c.classify_stage1("what happened yesterday");
        assert_eq!(scores.temporal, 1.0);
        assert!(matches!(
            outcome,
            Stage1Outcome::Decided {
                intent: Intent::Episodic,
                ..
            }
        ));
    }

    #[test]
    fn classifier_total_for_empty_query() {
        // Per §3.4 the classifier is *total* — it always returns *some* outcome.
        // Empty query has no signals → Factual + Associative downgrade.
        let c = HeuristicClassifier::with_null_lookup();
        let (_, outcome) = c.classify_stage1("");
        assert!(matches!(
            outcome,
            Stage1Outcome::Decided {
                intent: Intent::Factual,
                downgrade_hint: DowngradeHint::Associative,
            }
        ));
    }

    #[test]
    fn classifier_is_deterministic_for_identical_input() {
        let c = HeuristicClassifier::with_null_lookup();
        let q = "summarize what made me anxious last week";
        let (s1, o1) = c.classify_stage1(q);
        let (s2, o2) = c.classify_stage1(q);
        assert_eq!(s1, s2);
        assert_eq!(o1, o2);
    }
}
