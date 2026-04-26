//! Edge resolution decision (§3.4.4) — given an extracted triple and the
//! current state of the `(subject, predicate)` slot, produce one of:
//!
//!   - [`EdgeDecision::Add`] — no matching prior; insert a new edge.
//!   - [`EdgeDecision::None`] — equivalent edge already live; no-op.
//!   - [`EdgeDecision::Update`] — supersede a prior edge (object change for
//!     functional predicate, or confidence-driven version bump).
//!   - [`EdgeDecision::DeferToLlm`] — anomalous slot state (multiple live
//!     edges in a functional slot); LLM tie-breaker required.
//!
//! Pure logic: takes the slot's current edges as a slice and the new
//! triple's confidence + object. The caller (resolution pipeline) is
//! responsible for the slot lookup itself (`GraphStore::find_edges` with
//! `object = None`, ISS-035). Keeping the decision pure makes it
//! exhaustively testable without a SQLite fixture.
//!
//! Branches first on [`Cardinality`]:
//!
//! - **Functional** (`OneToOne`): at most one live edge per slot. Empty →
//!   Add; same-object same-confidence → None; same-object higher-confidence
//!   → Update (version bump); different-object → Update (supersede the
//!   replaced fact); multi → DeferToLlm.
//! - **Multi-valued** (`OneToMany` / `ManyToMany`): many live edges per
//!   slot is normal. Per-object: not in slot → Add; in slot, same conf →
//!   None; in slot, higher conf → Update. **Never DeferToLlm** — the
//!   answer is always deterministic for cardinality-known cases.
//!
//! References: v03-resolution/design.md §3.4.4, ISS-035.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::edge::{Edge, EdgeEnd};
use crate::graph::schema::{Cardinality, Predicate};

/// Confidence delta below which a same-object same-slot edge is considered
/// redundant (no version bump). Per design §3.4.4.
pub const EDGE_CONFIDENCE_EPSILON: f64 = 0.05;

/// Outcome of applying an extracted triple to the current `(subject,
/// predicate)` slot. Pure data — actual writes happen in the persist phase
/// (§3.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EdgeDecision {
    /// No matching prior in the slot — insert a fresh edge.
    Add,
    /// Equivalent edge already live in the slot — skip the write.
    None,
    /// Supersede a prior edge: insert new edge with `supersedes =
    /// Some(prior.id)`, then call `invalidate_edge(prior.id, new.id, now)`.
    /// Triggered by either a confidence bump on the same object or a fact
    /// change (different object, functional predicate only).
    Update {
        /// The prior edge's id; the successor's `supersedes` field MUST be
        /// set to this and the prior MUST be invalidated by the successor.
        supersedes: Uuid,
    },
    /// Anomalous slot state (multiple live edges in a functional slot).
    /// LLM tie-breaker required to pick the correct successor or to flag
    /// the data as inconsistent.
    DeferToLlm,
}

/// Pure decision function for edge resolution.
///
/// Inputs:
/// - `predicate`: the extracted triple's predicate. Used only for its
///   [`Cardinality`] (looked up via `predicate.cardinality()`).
/// - `new_object`: the extracted triple's object end (entity or literal).
/// - `new_confidence`: the extracted triple's edge confidence in `[0, 1]`.
/// - `existing`: the current live edges in the `(subject, predicate)`
///   slot, as returned by `GraphStore::find_edges(subject, predicate,
///   None, valid_only=true)`. **Must be the complete unfiltered slot** —
///   passing a triple-filtered subset breaks the functional-predicate
///   "different object" detection (ISS-035).
///
/// Returns the [`EdgeDecision`] to drive the persist phase.
pub fn compute_edge_decision(
    predicate: &Predicate,
    new_object: &EdgeEnd,
    new_confidence: f64,
    existing: &[Edge],
) -> EdgeDecision {
    let cardinality = predicate.cardinality();

    match cardinality {
        // ──────────────────────────────────────────────────────────────
        // BRANCH 1 — Functional (OneToOne)
        // At most one live edge per slot is expected.
        // ──────────────────────────────────────────────────────────────
        Cardinality::OneToOne => match existing {
            // Slot empty: fresh fact.
            [] => EdgeDecision::Add,

            // Single prior: differentiate by object identity + confidence.
            [prior] => {
                if &prior.object == new_object {
                    // Same fact. Bump only if new confidence beats prior
                    // by more than EPSILON.
                    if new_confidence > prior.confidence + EDGE_CONFIDENCE_EPSILON {
                        EdgeDecision::Update { supersedes: prior.id }
                    } else {
                        EdgeDecision::None
                    }
                } else {
                    // Fact changed (e.g. WorksAt: Acme → Beta).
                    // Supersede the prior — GOAL-2.10, GUARD-3.
                    EdgeDecision::Update { supersedes: prior.id }
                }
            }

            // Multiple live edges in a functional slot is anomalous: legacy
            // data, race condition, or literal-object ambiguity. Defer to
            // LLM rather than guess.
            _multi => EdgeDecision::DeferToLlm,
        },

        // ──────────────────────────────────────────────────────────────
        // BRANCH 2 — Multi-valued (OneToMany / ManyToMany)
        // Many live edges per slot is the normal state. Decision is
        // per-object: in-slot → no-op or bump; not-in-slot → Add. Never
        // DeferToLlm — answer is deterministic.
        //
        // Removal of a multi-valued edge requires an explicit
        // contradicting observation or agent curation; not in scope here.
        // ──────────────────────────────────────────────────────────────
        Cardinality::OneToMany | Cardinality::ManyToMany => {
            match existing.iter().find(|e| &e.object == new_object) {
                None => EdgeDecision::Add,
                Some(prior) => {
                    if new_confidence > prior.confidence + EDGE_CONFIDENCE_EPSILON {
                        EdgeDecision::Update { supersedes: prior.id }
                    } else {
                        EdgeDecision::None
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::{ConfidenceSource, ResolutionMethod};
    use crate::graph::schema::CanonicalPredicate;
    use chrono::Utc;

    /// Build an `Edge` for testing — only the fields read by
    /// `compute_edge_decision` matter (id, object, confidence). All other
    /// fields are filled with sentinel defaults.
    fn fixture_edge(predicate: Predicate, object: EdgeEnd, confidence: f64) -> Edge {
        let id = Uuid::new_v4();
        let subject_id = Uuid::new_v4();
        let now = Utc::now();
        Edge {
            id,
            subject_id,
            predicate,
            object,
            summary: String::new(),
            valid_from: None,
            valid_to: None,
            recorded_at: now,
            invalidated_at: None,
            invalidated_by: None,
            supersedes: None,
            episode_id: None,
            memory_id: None,
            resolution_method: ResolutionMethod::Automatic,
            activation: 0.0,
            confidence,
            confidence_source: ConfidenceSource::Recovered,
            agent_affect: None,
            created_at: now,
        }
    }

    fn entity_obj() -> EdgeEnd {
        EdgeEnd::Entity { id: Uuid::new_v4() }
    }

    // ----- Functional (OneToOne) branch -----

    #[test]
    fn functional_empty_slot_returns_add() {
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let decision = compute_edge_decision(&pred, &entity_obj(), 0.9, &[]);
        assert_eq!(decision, EdgeDecision::Add);
    }

    #[test]
    fn functional_same_object_no_conf_delta_returns_none() {
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let obj = entity_obj();
        let prior = fixture_edge(pred.clone(), obj.clone(), 0.8);
        let decision = compute_edge_decision(&pred, &obj, 0.8, &[prior]);
        assert_eq!(decision, EdgeDecision::None);
    }

    #[test]
    fn functional_same_object_within_epsilon_returns_none() {
        // Delta below epsilon → still None (avoid noisy version bumps).
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let obj = entity_obj();
        let prior = fixture_edge(pred.clone(), obj.clone(), 0.80);
        let decision = compute_edge_decision(&pred, &obj, 0.84, &[prior]);
        assert_eq!(decision, EdgeDecision::None, "delta 0.04 < EPS 0.05");
    }

    #[test]
    fn functional_same_object_above_epsilon_returns_update() {
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let obj = entity_obj();
        let prior = fixture_edge(pred.clone(), obj.clone(), 0.80);
        let prior_id = prior.id;
        let decision = compute_edge_decision(&pred, &obj, 0.90, &[prior]);
        assert_eq!(decision, EdgeDecision::Update { supersedes: prior_id });
    }

    #[test]
    fn functional_different_object_returns_update() {
        // The killer ISS-035 case: "Alice WorksAt Acme" → "Alice WorksAt
        // Beta". Slot lookup MUST find the Acme edge despite the new
        // triple specifying Beta; without slot lookup this would route to
        // Add and produce two parallel live edges.
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let acme = entity_obj();
        let beta = entity_obj();
        let prior = fixture_edge(pred.clone(), acme.clone(), 0.85);
        let prior_id = prior.id;
        let decision = compute_edge_decision(&pred, &beta, 0.85, &[prior]);
        assert_eq!(
            decision,
            EdgeDecision::Update { supersedes: prior_id },
            "fact change must supersede, not Add"
        );
    }

    #[test]
    fn functional_multiple_live_edges_returns_defer_to_llm() {
        // Anomalous: two live WorksAt edges. Don't guess.
        let pred = Predicate::Canonical(CanonicalPredicate::WorksAt);
        let obj_a = entity_obj();
        let obj_b = entity_obj();
        let prior_a = fixture_edge(pred.clone(), obj_a, 0.85);
        let prior_b = fixture_edge(pred.clone(), obj_b, 0.80);
        let new_obj = entity_obj();
        let decision = compute_edge_decision(&pred, &new_obj, 0.9, &[prior_a, prior_b]);
        assert_eq!(decision, EdgeDecision::DeferToLlm);
    }

    // ----- Multi-valued (OneToMany / ManyToMany) branch -----

    #[test]
    fn multi_valued_empty_slot_returns_add() {
        let pred = Predicate::Canonical(CanonicalPredicate::IsA);
        let decision = compute_edge_decision(&pred, &entity_obj(), 0.9, &[]);
        assert_eq!(decision, EdgeDecision::Add);
    }

    #[test]
    fn multi_valued_new_object_returns_add_even_with_existing_edges() {
        // ISS-035 multi-valued case: "Alice IsA programmer" exists; new
        // triple "Alice IsA cyclist" should Add (not Update, not None).
        // Many-to-many predicates allow many parallel live edges.
        let pred = Predicate::Canonical(CanonicalPredicate::IsA);
        let programmer = entity_obj();
        let cyclist = entity_obj();
        let prior = fixture_edge(pred.clone(), programmer, 0.9);
        let decision = compute_edge_decision(&pred, &cyclist, 0.85, &[prior]);
        assert_eq!(
            decision,
            EdgeDecision::Add,
            "multi-valued: new object must Add, not supersede"
        );
    }

    #[test]
    fn multi_valued_existing_object_no_delta_returns_none() {
        let pred = Predicate::Canonical(CanonicalPredicate::IsA);
        let role = entity_obj();
        let prior = fixture_edge(pred.clone(), role.clone(), 0.85);
        let decision = compute_edge_decision(&pred, &role, 0.85, &[prior]);
        assert_eq!(decision, EdgeDecision::None);
    }

    #[test]
    fn multi_valued_existing_object_higher_conf_returns_update() {
        let pred = Predicate::Canonical(CanonicalPredicate::DependsOn);
        let lib = entity_obj();
        let prior = fixture_edge(pred.clone(), lib.clone(), 0.70);
        let prior_id = prior.id;
        let decision = compute_edge_decision(&pred, &lib, 0.90, &[prior]);
        assert_eq!(decision, EdgeDecision::Update { supersedes: prior_id });
    }

    #[test]
    fn multi_valued_never_returns_defer_to_llm() {
        // GOAL-2.9: multi-valued cases must be deterministic. Even with
        // many existing edges and no match, the decision is Add.
        let pred = Predicate::Canonical(CanonicalPredicate::IsA);
        let priors: Vec<Edge> = (0..5)
            .map(|_| fixture_edge(pred.clone(), entity_obj(), 0.8))
            .collect();
        let new_obj = entity_obj();
        let decision = compute_edge_decision(&pred, &new_obj, 0.85, &priors);
        assert_eq!(decision, EdgeDecision::Add);
        assert_ne!(decision, EdgeDecision::DeferToLlm);
    }

    #[test]
    fn one_to_many_routes_through_multi_valued_branch() {
        // OneToMany should behave identically to ManyToMany for the
        // EdgeDecision logic.
        let pred = Predicate::Canonical(CanonicalPredicate::ParentOf);
        let kid_a = entity_obj();
        let kid_b = entity_obj();
        let prior = fixture_edge(pred.clone(), kid_a, 0.9);
        let decision = compute_edge_decision(&pred, &kid_b, 0.9, &[prior]);
        assert_eq!(
            decision,
            EdgeDecision::Add,
            "OneToMany ParentOf: second child must Add"
        );
    }

    // ----- Proposed predicates default to ManyToMany -----

    #[test]
    fn proposed_predicate_defaults_to_multi_valued_branch() {
        // Per design §3.3 — Proposed predicates default to ManyToMany. A
        // new object with the same Proposed predicate must Add, not
        // supersede.
        let pred = Predicate::proposed("collaborates_with");
        let prior_obj = entity_obj();
        let new_obj = entity_obj();
        let prior = fixture_edge(pred.clone(), prior_obj, 0.85);
        let decision = compute_edge_decision(&pred, &new_obj, 0.85, &[prior]);
        assert_eq!(decision, EdgeDecision::Add);
    }

    // ----- Edge case: literal objects -----

    #[test]
    fn functional_literal_object_change_returns_update() {
        // Functional predicate with literal objects (e.g. a fact whose
        // object end is a string literal rather than an entity). The
        // decision logic should treat literal change as supersession too.
        let pred = Predicate::Canonical(CanonicalPredicate::CreatedBy);
        let lit_old = EdgeEnd::Literal { value: serde_json::Value::String("Alice".to_string()) };
        let lit_new = EdgeEnd::Literal { value: serde_json::Value::String("Bob".to_string()) };
        let prior = fixture_edge(pred.clone(), lit_old, 0.85);
        let prior_id = prior.id;
        let decision = compute_edge_decision(&pred, &lit_new, 0.85, &[prior]);
        assert_eq!(decision, EdgeDecision::Update { supersedes: prior_id });
    }

    // ----- EdgeDecision serde -----

    #[test]
    fn edge_decision_serde_roundtrip() {
        let id = Uuid::new_v4();
        for decision in [
            EdgeDecision::Add,
            EdgeDecision::None,
            EdgeDecision::Update { supersedes: id },
            EdgeDecision::DeferToLlm,
        ] {
            let json = serde_json::to_string(&decision).unwrap();
            let decoded: EdgeDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, decision);
        }
    }
}
