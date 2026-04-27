//! # Typed retrieval outcomes — design §6.4 / §6.6
//!
//! This module owns two surfaces:
//!
//! 1. [`RetrievalOutcome`] (design §6.4, **GOAL-3.10**) — typed
//!    success / soft-failure modes. A non-`Ok` outcome is *not* an
//!    [`Err`]: results may still be present (e.g. associative fallback
//!    after [`RetrievalOutcome::EntityFoundNoEdges`]). The
//!    distinction lets callers render graceful UX
//!    ("we couldn't resolve any entity for your tokens" vs.
//!    "entities exist but no edges match") instead of collapsing
//!    every shape into an empty result set.
//!
//! 2. [`RetrievalError`] (design §6.2a) — infrastructure-level
//!    failures only (DB error, outer-query timeout, config invariant
//!    violation, classifier crash). Business-logic "didn't find what
//!    you asked for" stays in `Ok(_)` with a typed
//!    [`RetrievalOutcome`]. This is the read-path instantiation of
//!    GUARD-6 (cognitive state / missing data never fails the call).
//!
//! ## Why a dedicated module
//!
//! Per the v0.3 retrieval build plan (`code:planned:retrieval-outcomes`,
//! file `src/retrieval/outcomes.rs`) and design §6.4 + §6.6, this is a
//! self-contained surface: callers depend on the variant set, and every
//! plan adapter (`*Outcome::to_retrieval_outcome` in
//! `crate::retrieval::plans::*`) lifts plan-local outcomes into
//! [`RetrievalOutcome`]. Keeping the surface in one file makes the
//! invariants reviewable in isolation.
//!
//! ## Versioning posture
//!
//! Both enums are `#[non_exhaustive]`. New variants land via additive
//! changes (no breaking API churn) so future plans (e.g. a graph-query
//! shortcut returning a brand-new outcome) can extend the surface
//! without a `1.0 → 2.0` bump. Callers MUST handle unknown variants in
//! a wildcard arm — the compiler enforces this via `non_exhaustive`.
//!
//! ## Spec references
//!
//! - Design §6.4 — `RetrievalOutcome` definition and variant semantics.
//! - Design §6.2a — `RetrievalError` infrastructure-only contract.
//! - Design §6.6 / **GOAL-3.12** — novel-predicate retrieval surface
//!   (`Memory::list_proposed_predicates`, see `memory.rs`).
//! - GOAL-3.10 (P1): typed failure modes are explicit and
//!   distinguishable by the caller.
//! - GOAL-3.14 / GUARD-6: the read path never fails on cognitive
//!   absence — encoded as [`RetrievalOutcome::NoCognitiveState`] in
//!   the `Ok(_)` arm, never as an [`Err`].

use chrono::{DateTime, Utc};

use crate::retrieval::api::EntityId;
use crate::retrieval::classifier::Intent;

/// Typed retrieval outcome — design §6.4 / **GOAL-3.10**.
///
/// Returned in [`crate::retrieval::api::GraphQueryResponse::outcome`]
/// alongside `results: Vec<ScoredResult>`. Non-empty results may
/// accompany **any** non-`Ok` variant — for example,
/// `EntityFoundNoEdges` typically carries associative-fallback
/// candidates so the caller still gets *something* useful while the
/// outcome explains *why* the strict-factual path returned nothing.
///
/// `Err(RetrievalError)` is reserved for infrastructure failures (DB
/// errors, outer-query timeout, config errors). Business-logic
/// shortfalls — "no entity matched", "window empty", "L5 not ready",
/// "self-state absent" — stay in `Ok(_)` with one of the variants
/// below. This is the read-path expression of GUARD-6.
///
/// ## Variant catalogue (design §6.4)
///
/// | Variant | Plan | Trigger |
/// |---|---|---|
/// | `Ok` | any | results non-empty, no degraded path taken |
/// | `NoEntityFound` | Factual | no query token resolved to any entity |
/// | `EntityFoundNoEdges` | Factual | entities exist, every 1-hop edge filtered out (e.g. `as-of-T` precedes their `valid_from`) |
/// | `NoMemoriesInWindow` | Episodic | window is valid but no memories landed inside it |
/// | `AmbiguousQuery` | classifier (LLM fallback) | Stage 2 returned multiple plausible intents with near-tied confidence |
/// | `L5NotReady` | Abstract | searcher returned nothing for the query domain (synthesis hasn't covered it) |
/// | `DowngradedFromAbstract` | Abstract | plan downgraded to Associative (e.g. `reason = "L5_unavailable"`) |
/// | `DowngradedFromEpisodic` | Episodic | plan downgraded to Associative (e.g. `reason = "no_time_expression"`) |
/// | `NoCognitiveState` | Affective | self-state absent — plan downgraded to Associative |
///
/// `non_exhaustive`: the orchestrator and Hybrid plan may surface
/// future variants (e.g. `HybridTruncated`) without a breaking change.
/// Match on this enum with an explicit wildcard arm.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RetrievalOutcome {
    /// Results non-empty; the plan executed normally with no
    /// downgrades, no missing substrate. The default success state.
    Ok,

    /// **Factual.** No query token resolved to any entity in the
    /// graph (after alias normalization). Carries the literal token
    /// list so callers can surface a "we don't recognize any of: …"
    /// message and so traces can pinpoint which tokens were tried.
    NoEntityFound {
        /// The query tokens that were probed against the entity
        /// resolver, in canonicalized order (post-tokenization,
        /// post-stop-word removal).
        query_tokens: Vec<String>,
    },

    /// **Factual.** One or more anchors resolved, but no 1-hop edge
    /// from those anchors survived projection / filtering. Distinct
    /// from `Ok` with `results: vec![]` in two ways:
    ///   1. anchors *did* exist (UI may show "we found these
    ///      entities, but no relevant facts");
    ///   2. associative fallback may have populated `results` — the
    ///      response can be non-empty.
    EntityFoundNoEdges {
        /// Anchors that resolved successfully but had no live edges
        /// at the projection instant.
        entities: Vec<EntityId>,
    },

    /// **Episodic.** Time window parsed cleanly but the recaller
    /// returned zero memories inside it. `start` / `end` are the
    /// boundaries actually queried (after relative-window resolution
    /// against `query_time`); `None` means open on that side.
    NoMemoriesInWindow {
        /// Inclusive lower bound of the queried window. `None` =
        /// `(-∞, end]`.
        start: Option<DateTime<Utc>>,
        /// Inclusive upper bound of the queried window. `None` =
        /// `[start, +∞)`.
        end: Option<DateTime<Utc>>,
    },

    /// **Classifier (Stage 2 LLM fallback).** Multiple intents had
    /// near-tied scores and the LLM fallback could not break the tie
    /// confidently. The orchestrator typically picks the highest-mean
    /// candidate, executes its plan, and surfaces this outcome so the
    /// caller knows the routing was not unambiguous.
    AmbiguousQuery {
        /// The set of intents the classifier judged plausible
        /// (descending confidence). Always ≥ 2 entries.
        candidate_intents: Vec<Intent>,
        /// Human-readable explanation suitable for traces / logs
        /// (e.g. `"Factual (0.42) and Episodic (0.41) within 0.05"`).
        reason: String,
    },

    /// **Abstract.** L5 substrate has no topics for the query
    /// domain — synthesis hasn't reached it yet. Distinct from
    /// `DowngradedFromAbstract` because here the *substrate* is
    /// missing; in the downgrade case the plan voluntarily switched
    /// strategies (e.g. all topic scores below `l5_min_topic_score`).
    L5NotReady {
        /// Domain hints / namespaces that had no synthesized topics.
        /// Empty when the recaller returned nothing for the whole
        /// query (no domain hint extracted).
        missing_topic_domains: Vec<String>,
    },

    /// **Abstract.** Plan downgraded to Associative. Reason captures
    /// *why* the downgrade fired: `"L5_unavailable"`,
    /// `"all_below_min_topic_score"`, or future operator-tunable
    /// reasons. Free-form so plans can evolve their downgrade
    /// vocabulary without a breaking enum change.
    DowngradedFromAbstract {
        /// Stable identifier for the downgrade reason. Plans SHOULD
        /// use a snake_case constant from a documented list (see
        /// `crate::retrieval::plans::abstract_l5`).
        reason: String,
    },

    /// **Episodic.** Plan downgraded to Associative because the
    /// query had no time expression that could be parsed with
    /// confidence ≥ 0.5 (`"no_time_expression"`), or because the
    /// requested window lay outside the knowledge cutoff
    /// (`"window_outside_cutoff"`). Additional reasons may be added.
    DowngradedFromEpisodic {
        /// Stable identifier for the downgrade reason (snake_case).
        reason: String,
    },

    /// **Affective.** Cognitive self-state was `None` at query time.
    /// The plan downgraded to Associative per design §3.4 / §4.5
    /// step 1. Read-path instantiation of GUARD-6: missing self-state
    /// is *never* an `Err`.
    NoCognitiveState,
}

impl RetrievalOutcome {
    /// Stable string slug for metrics / log labels (paired with the
    /// `retrieval_outcome_*` Prometheus counter family wired by
    /// `task:retr-impl-metrics`). Always a small ASCII identifier so
    /// it can be embedded directly in label values without escaping.
    pub fn slug(&self) -> &'static str {
        match self {
            RetrievalOutcome::Ok => "ok",
            RetrievalOutcome::NoEntityFound { .. } => "no_entity_found",
            RetrievalOutcome::EntityFoundNoEdges { .. } => "entity_found_no_edges",
            RetrievalOutcome::NoMemoriesInWindow { .. } => "no_memories_in_window",
            RetrievalOutcome::AmbiguousQuery { .. } => "ambiguous_query",
            RetrievalOutcome::L5NotReady { .. } => "l5_not_ready",
            RetrievalOutcome::DowngradedFromAbstract { .. } => "downgraded_from_abstract",
            RetrievalOutcome::DowngradedFromEpisodic { .. } => "downgraded_from_episodic",
            RetrievalOutcome::NoCognitiveState => "no_cognitive_state",
        }
    }

    /// Returns `true` when the outcome represents a clean, non-empty
    /// success. All other variants — including downgrades that
    /// successfully populated results via fallback — return `false`
    /// because callers typically want to surface the divergent
    /// shape regardless of result count.
    pub fn is_ok(&self) -> bool {
        matches!(self, RetrievalOutcome::Ok)
    }

    /// Returns `true` when the outcome is a *downgrade* to a
    /// different plan. Useful for trace assertions and the
    /// orchestrator's "did the active plan change?" check.
    pub fn is_downgrade(&self) -> bool {
        matches!(
            self,
            RetrievalOutcome::DowngradedFromAbstract { .. }
                | RetrievalOutcome::DowngradedFromEpisodic { .. }
                | RetrievalOutcome::NoCognitiveState
        )
    }
}

/// Infrastructure-level failure — design §6.2a.
///
/// "Empty result" is **not** here — that goes in the `Ok(_)` path
/// with [`RetrievalOutcome`]. Variants are kept narrow on purpose so
/// callers can route each kind to an appropriate UX (timeout → retry,
/// store unavailable → degrade to v0.2 recall, config error → fail
/// loud at boot, etc.).
///
/// `non_exhaustive`: future infrastructure failure modes (e.g.
/// `EmbeddingProviderDown`) land additively without breaking matches.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RetrievalError {
    /// Outer-query timeout (design §7.2). Per-stage cutoffs return
    /// partial results inside `Ok(_)` and are **not** modeled as
    /// `Err` — only the top-level deadline triggers this variant.
    #[error("retrieval timed out")]
    Timeout,

    /// Backing store (graph or memory) not available — the
    /// connection failed, the file is missing, etc.
    #[error("retrieval store unavailable")]
    StoreUnavailable,

    /// Configuration invariant violated (e.g. `FusionWeights` don't
    /// sum to 1.0, `RetrievalConfig::strong_signal_threshold` outside
    /// `[0, 1]`). String body identifies the offending field.
    #[error("retrieval config error: {0}")]
    ConfigError(String),

    /// Classifier (Stage 1 heuristic or Stage 2 LLM) returned an
    /// unrecoverable error. Stage 1 errors here are unusual (the
    /// heuristic is pure-function); Stage 2 wraps LLM provider
    /// failures.
    #[error("classifier error: {0}")]
    ClassifierError(String),

    /// Catch-all for unexpected internal failures. Implementation
    /// tasks should narrow these into specific variants where
    /// possible — every appearance in production is a candidate for
    /// promotion to a typed variant.
    #[error("retrieval internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_distinct_per_variant() {
        let ok = RetrievalOutcome::Ok;
        let no_ent = RetrievalOutcome::NoEntityFound {
            query_tokens: vec!["alice".into()],
        };
        let no_edges = RetrievalOutcome::EntityFoundNoEdges { entities: vec![] };
        let no_window = RetrievalOutcome::NoMemoriesInWindow {
            start: None,
            end: None,
        };
        let amb = RetrievalOutcome::AmbiguousQuery {
            candidate_intents: vec![Intent::Factual, Intent::Episodic],
            reason: "tied".into(),
        };
        let l5 = RetrievalOutcome::L5NotReady {
            missing_topic_domains: vec!["physics".into()],
        };
        let abs_d = RetrievalOutcome::DowngradedFromAbstract {
            reason: "L5_unavailable".into(),
        };
        let ep_d = RetrievalOutcome::DowngradedFromEpisodic {
            reason: "no_time_expression".into(),
        };
        let no_cog = RetrievalOutcome::NoCognitiveState;

        let slugs = [
            ok.slug(),
            no_ent.slug(),
            no_edges.slug(),
            no_window.slug(),
            amb.slug(),
            l5.slug(),
            abs_d.slug(),
            ep_d.slug(),
            no_cog.slug(),
        ];
        let mut sorted = slugs.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), slugs.len(), "slugs collide: {slugs:?}");
    }

    #[test]
    fn slug_is_pure_ascii_snake_case() {
        let cases = [
            RetrievalOutcome::Ok,
            RetrievalOutcome::NoEntityFound {
                query_tokens: vec![],
            },
            RetrievalOutcome::EntityFoundNoEdges { entities: vec![] },
            RetrievalOutcome::NoMemoriesInWindow {
                start: None,
                end: None,
            },
            RetrievalOutcome::NoCognitiveState,
        ];
        for c in &cases {
            let s = c.slug();
            assert!(
                s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "slug {s:?} has non-snake_case chars",
            );
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn is_ok_only_true_for_ok() {
        assert!(RetrievalOutcome::Ok.is_ok());
        assert!(!RetrievalOutcome::NoCognitiveState.is_ok());
        assert!(!RetrievalOutcome::DowngradedFromEpisodic {
            reason: "x".into()
        }
        .is_ok());
        assert!(!RetrievalOutcome::EntityFoundNoEdges { entities: vec![] }.is_ok());
    }

    #[test]
    fn is_downgrade_matches_design_set() {
        // Design §6.4: the explicit downgrade family.
        assert!(RetrievalOutcome::DowngradedFromAbstract {
            reason: "L5_unavailable".into()
        }
        .is_downgrade());
        assert!(RetrievalOutcome::DowngradedFromEpisodic {
            reason: "no_time_expression".into()
        }
        .is_downgrade());
        assert!(RetrievalOutcome::NoCognitiveState.is_downgrade());

        // Substrate / lookup misses are NOT downgrades — the plan
        // ran end-to-end, just produced no rows.
        assert!(!RetrievalOutcome::Ok.is_downgrade());
        assert!(!RetrievalOutcome::NoEntityFound {
            query_tokens: vec![]
        }
        .is_downgrade());
        assert!(!RetrievalOutcome::L5NotReady {
            missing_topic_domains: vec![]
        }
        .is_downgrade());
    }

    #[test]
    fn retrieval_error_display_strings_are_human_readable() {
        // Surface-level guarantee: thiserror produces non-empty
        // messages for every variant. Useful for log/UX consumers.
        let cases = [
            RetrievalError::Timeout,
            RetrievalError::StoreUnavailable,
            RetrievalError::ConfigError("weights".into()),
            RetrievalError::ClassifierError("provider 502".into()),
            RetrievalError::Internal("bug".into()),
        ];
        for e in &cases {
            let s = format!("{e}");
            assert!(!s.is_empty());
            assert!(
                s.starts_with("retrieval") || s.starts_with("classifier"),
                "unexpected display prefix: {s:?}",
            );
        }
    }
}
