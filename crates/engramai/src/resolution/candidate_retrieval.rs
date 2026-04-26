//! § 3.4.1 — Candidate retrieval driver.
//!
//! Bridges the `GraphStore::search_candidates` raw output to the fusion
//! module's `Measurement` inputs. This is the only piece of glue between
//! the *store* (which knows blobs and SQL) and the *fusion arithmetic*
//! (which knows weights and signal redistribution).
//!
//! ## Why a driver, not in the store
//!
//! `search_candidates` is deliberately *unranked* — it returns raw signals
//! per-candidate (alias hit bool, cosine in `[-1, 1]`, linear recency in
//! `[0, 1]`, projected `last_seen` / `identity_confidence`). Combining
//! those into a fusion-ready `Measurement{ signal, value ∈ [0, 1] }` is a
//! transform that lives at the resolution layer because:
//!
//! - The cosine→`[0, 1]` normalization (`0.5 * (cos + 1)`) must match
//!   `signals::semantic_similarity`'s convention exactly. Two places of
//!   truth would drift; one place that documents the bridge is safer.
//! - `alias_match: bool` becomes s2 (`name_match`) only as a discrete 0/1
//!   value here. Continuous Jaro-Winkler scoring (the full `signals::name_match`
//!   path) requires candidate aliases that `CandidateMatch` does *not*
//!   project — that path is for the (future) full driver that already has
//!   a hot `Entity` in hand.
//! - `recency_score: f32` from the store is already in `[0, 1]` and ready
//!   to be a `Measurement` value. We promote it to f64 and that is all.
//! - Signals s3/s5/s6/s7/s8 (graph_context, cooccurrence, affective,
//!   identity_hint, somatic) require richer mention-side data that the
//!   §3.4.1 driver does not have yet (no neighborhood walk, no Hebbian
//!   strengths, no affect snapshot, no session metadata, no fingerprint).
//!   Per the v0.3 design, missing signals are *first-class* — fusion
//!   redistributes their weight across present signals (§3.4.2). This
//!   driver therefore emits only the s1, s2, s4 measurements it can
//!   compute; the rest become `None` automatically.
//!
//! ## Determinism
//!
//! The driver does no IO of its own — it calls `graph_store.search_candidates`
//! once and transforms the rows. `now` and the recency window are both
//! supplied by the caller (no system clock reads). Output ordering matches
//! the store's ascending-`entity_id` ordering; ranking happens later in
//! fusion + decision.
//!
//! ## Scope (this file)
//!
//! - `retrieve_candidates`: §3.4.1 driver entry point
//! - `measurements_for`: pure helper, exposed for testing the bridge logic
//!   independently of a `GraphStore`
//!
//! Out of scope (other drivers, other ISSes):
//! - Mention embedding (caller's responsibility — `mention_embedding` is
//!   passed in)
//! - The full §3.4 pipeline (this is one of four sub-drivers)
//! - Edge candidate retrieval (`find_edges` — ISS-034)

use crate::graph::entity::EntityKind;
use crate::graph::error::GraphError;
use crate::graph::store::{CandidateMatch, CandidateQuery, GraphStore};
use crate::resolution::fusion::Measurement;
use crate::resolution::signals::Signal;
use std::time::Duration;

/// One retrieval result: the raw `CandidateMatch` row plus the fusion-ready
/// measurements derived from it.
///
/// Fusion only looks at `measurements`; we keep `match_row` alongside so
/// the caller can:
///   - assemble the resolution trace (canonical_name, last_seen, etc.)
///   - decide create-vs-merge using `entity_id` after fusion picks a winner
///   - log the raw signals for diagnostics without re-querying
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate {
    pub match_row: CandidateMatch,
    pub measurements: Vec<Measurement>,
}

/// Caller-tunable knobs for `retrieve_candidates`. Kept as a struct (not
/// loose params) so adding a knob later doesn't break call sites.
#[derive(Debug, Clone)]
pub struct RetrievalParams {
    /// Hard upper bound on candidates to consider. The store enforces its
    /// own ceiling ([`crate::graph::store::MAX_TOP_K`]) on top of this.
    pub top_k: usize,
    /// Recency-decay window (linear). `None` ⇒ scale recency over the
    /// candidate set's `last_seen` span.
    pub recency_window: Option<Duration>,
    /// Optional kind filter. `None` ⇒ all kinds in scope.
    pub kind_filter: Option<EntityKind>,
}

impl Default for RetrievalParams {
    fn default() -> Self {
        Self {
            top_k: 16,
            recency_window: Some(Duration::from_secs(60 * 60 * 24 * 30)), // 30d
            kind_filter: None,
        }
    }
}

/// Pull candidates from the store and bridge them into fusion-ready
/// measurements.
///
/// `now` is unix seconds (matches `CandidateQuery::now`). Pass the memory's
/// own `occurred_at` here, not wall-clock-now — re-extracts must score
/// recency relative to the memory's time, per GOAL-2.1 idempotence.
///
/// Returns an empty vec when the store finds no candidates — this is the
/// `CreateNew` short-circuit path; fusion is not called.
pub fn retrieve_candidates<S: GraphStore + ?Sized>(
    store: &S,
    mention_text: &str,
    mention_embedding: Option<&[f32]>,
    namespace: &str,
    now: f64,
    params: &RetrievalParams,
) -> Result<Vec<ScoredCandidate>, GraphError> {
    let query = CandidateQuery {
        mention_text: mention_text.to_string(),
        mention_embedding: mention_embedding.map(|s| s.to_vec()),
        kind_filter: params.kind_filter.clone(),
        namespace: namespace.to_string(),
        top_k: params.top_k,
        recency_window: params.recency_window,
        now,
    };

    let rows = store.search_candidates(&query)?;
    let scored = rows
        .into_iter()
        .map(|row| {
            let measurements = measurements_for(&row);
            ScoredCandidate {
                match_row: row,
                measurements,
            }
        })
        .collect();
    Ok(scored)
}

/// Pure transform from a `CandidateMatch` to the subset of fusion
/// measurements this driver knows how to emit.
///
/// Emits at most three measurements:
/// - **s1 SemanticSimilarity** — only when `embedding_score` is `Some`,
///   normalized from cosine `[-1, 1]` to `[0, 1]` via `0.5 * (cos + 1)`.
///   Matches `signals::semantic_similarity`'s convention exactly; if you
///   change one, change the other.
/// - **s2 NameMatch** — only when `alias_match` is true (1.0). False is
///   *not* emitted (the absence of an exact alias hit is missing data,
///   not a 0.0 score; the fuzzy/Jaro-Winkler path is reserved for the
///   future driver that has candidate aliases in hand).
/// - **s4 Recency** — always emitted (recency_score is always present and
///   already in `[0, 1]`).
///
/// All other signals (s3, s5, s6, s7, s8) are intentionally absent — see
/// the module-level rationale.
pub(crate) fn measurements_for(row: &CandidateMatch) -> Vec<Measurement> {
    let mut out: Vec<Measurement> = Vec::with_capacity(3);

    // s1 — semantic similarity.
    if let Some(cos) = row.embedding_score {
        // cosine ∈ [-1, 1] → [0, 1]. Mirror of `signals::semantic_similarity`.
        let normalized = 0.5_f64 * (cos as f64 + 1.0);
        let v = normalized.clamp(0.0, 1.0);
        out.push(Measurement {
            signal: Signal::SemanticSimilarity,
            value: v,
        });
    }

    // s2 — name match (discrete: alias hit only).
    if row.alias_match {
        out.push(Measurement {
            signal: Signal::NameMatch,
            value: 1.0,
        });
    }
    // alias_match=false → no measurement (missing signal); Jaro-Winkler
    // continuous scoring requires candidate aliases not projected here.

    // s4 — recency. Already in [0, 1].
    let r = (row.recency_score as f64).clamp(0.0, 1.0);
    out.push(Measurement {
        signal: Signal::Recency,
        value: r,
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::entity::EntityKind;
    use crate::graph::store::CandidateMatch;
    use uuid::Uuid;

    fn row(
        alias_match: bool,
        embedding_score: Option<f32>,
        recency_score: f32,
    ) -> CandidateMatch {
        CandidateMatch {
            entity_id: Uuid::nil(),
            kind: EntityKind::Person,
            canonical_name: "x".into(),
            alias_match,
            embedding_score,
            recency_score,
            last_seen: 0.0,
            identity_confidence: 0.0,
        }
    }

    #[test]
    fn measurements_for_alias_only_emits_s2_and_s4() {
        let m = measurements_for(&row(true, None, 0.5));
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|x| x.signal == Signal::NameMatch && x.value == 1.0));
        assert!(m
            .iter()
            .any(|x| x.signal == Signal::Recency && (x.value - 0.5).abs() < 1e-9));
        assert!(!m.iter().any(|x| x.signal == Signal::SemanticSimilarity));
    }

    #[test]
    fn measurements_for_embedding_only_emits_s1_and_s4() {
        // cosine = 1.0 → s1 = 0.5 * (1 + 1) = 1.0
        let m = measurements_for(&row(false, Some(1.0), 0.0));
        assert_eq!(m.len(), 2);
        let s1 = m
            .iter()
            .find(|x| x.signal == Signal::SemanticSimilarity)
            .unwrap();
        assert!((s1.value - 1.0).abs() < 1e-9);
        assert!(!m.iter().any(|x| x.signal == Signal::NameMatch));
    }

    #[test]
    fn measurements_for_both_signals_emits_three() {
        let m = measurements_for(&row(true, Some(0.0), 1.0));
        assert_eq!(m.len(), 3);
        // cosine 0 → s1 = 0.5
        let s1 = m
            .iter()
            .find(|x| x.signal == Signal::SemanticSimilarity)
            .unwrap();
        assert!((s1.value - 0.5).abs() < 1e-9);
    }

    #[test]
    fn measurements_for_negative_cosine_clamps_into_unit_range() {
        // cosine = -1.0 → s1 = 0.0
        let m = measurements_for(&row(false, Some(-1.0), 0.0));
        let s1 = m
            .iter()
            .find(|x| x.signal == Signal::SemanticSimilarity)
            .unwrap();
        assert_eq!(s1.value, 0.0);
    }

    #[test]
    fn measurements_for_alias_false_does_not_emit_s2() {
        let m = measurements_for(&row(false, None, 0.0));
        // Only s4 (recency) is always present.
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].signal, Signal::Recency);
    }

    #[test]
    fn measurements_for_recency_clamps_above_one() {
        // Defensive: store should emit ≤ 1.0 but if it ever drifts, clamp.
        let m = measurements_for(&row(false, None, 1.5));
        let r = m.iter().find(|x| x.signal == Signal::Recency).unwrap();
        assert_eq!(r.value, 1.0);
    }

    #[test]
    fn cosine_to_unit_normalization_matches_semantic_similarity_helper() {
        // Hand-check that we use 0.5 * (cos + 1), same as signals::semantic_similarity.
        // Cosine = 0.5 → expected = 0.75.
        let m = measurements_for(&row(false, Some(0.5), 0.0));
        let s1 = m
            .iter()
            .find(|x| x.signal == Signal::SemanticSimilarity)
            .unwrap();
        assert!((s1.value - 0.75).abs() < 1e-9);
    }

    #[test]
    fn default_retrieval_params_use_thirty_day_window() {
        let p = RetrievalParams::default();
        assert_eq!(p.recency_window, Some(Duration::from_secs(60 * 60 * 24 * 30)));
        assert_eq!(p.top_k, 16);
        assert!(p.kind_filter.is_none());
    }

    // ============================================================
    // E2E: driver against a real SqliteGraphStore.
    //
    // These tests exercise the whole bridge: store insert →
    // search_candidates → measurements_for → fusion-ready output.
    // They are the regression line for ISS-033 Layer 3.
    // ============================================================

    use crate::graph::store::SqliteGraphStore;
    use crate::graph::test_helpers::{fresh_conn, insert_test_entity};
    use chrono::Utc;

    fn dt_to_unix(dt: chrono::DateTime<Utc>) -> f64 {
        dt.timestamp() as f64 + (dt.timestamp_subsec_micros() as f64) / 1_000_000.0
    }

    #[test]
    fn driver_e2e_alias_only_emits_namematch_and_recency() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(4);
        let now_dt = Utc::now();
        let e = insert_test_entity(&mut store, "Mel", EntityKind::Person, now_dt, None);
        store.upsert_alias("mel", "Mel", e.id, None).unwrap();
        // Distractor: no alias, no embedding — must not appear.
        insert_test_entity(&mut store, "Bob", EntityKind::Person, now_dt, None);

        let scored = retrieve_candidates(
            &store,
            "Mel",
            None,
            "default",
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .expect("driver ok");

        assert_eq!(scored.len(), 1, "alias-only query returns just the hit");
        let c = &scored[0];
        assert_eq!(c.match_row.entity_id, e.id);
        assert!(c.match_row.alias_match);
        // s2 (NameMatch=1.0) + s4 (Recency) must be present; s1 absent.
        let signals: Vec<Signal> = c.measurements.iter().map(|m| m.signal).collect();
        assert!(signals.contains(&Signal::NameMatch));
        assert!(signals.contains(&Signal::Recency));
        assert!(!signals.contains(&Signal::SemanticSimilarity));
    }

    #[test]
    fn driver_e2e_embedding_only_emits_semsim_and_recency() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now_dt = Utc::now();
        let _e1 = insert_test_entity(
            &mut store,
            "A",
            EntityKind::Concept,
            now_dt,
            Some(vec![1.0, 0.0, 0.0]),
        );
        let _e2 = insert_test_entity(
            &mut store,
            "B",
            EntityKind::Concept,
            now_dt,
            Some(vec![0.0, 1.0, 0.0]),
        );

        let scored = retrieve_candidates(
            &store,
            "irrelevant",
            Some(&[1.0_f32, 0.0, 0.0]),
            "default",
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .expect("driver ok");
        assert_eq!(scored.len(), 2);

        // The aligned entity ("A") must have s1 ~ 1.0; the orthogonal one ~ 0.5.
        for c in &scored {
            assert!(!c.match_row.alias_match);
            let s1 = c
                .measurements
                .iter()
                .find(|m| m.signal == Signal::SemanticSimilarity)
                .expect("s1 present when embedding signal is present");
            if c.match_row.canonical_name == "A" {
                assert!(
                    (s1.value - 1.0).abs() < 1e-6,
                    "aligned vector → s1 ~ 1.0 (got {})",
                    s1.value
                );
            } else {
                // cosine 0 → 0.5
                assert!(
                    (s1.value - 0.5).abs() < 1e-6,
                    "orthogonal vector → s1 = 0.5 (got {})",
                    s1.value
                );
            }
            // s2 must be absent (alias_match=false → no NameMatch measurement).
            assert!(c
                .measurements
                .iter()
                .all(|m| m.signal != Signal::NameMatch));
        }
    }

    #[test]
    fn driver_e2e_both_signals_emits_three_measurements() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now_dt = Utc::now();
        let e = insert_test_entity(
            &mut store,
            "Mel",
            EntityKind::Person,
            now_dt,
            Some(vec![1.0, 0.0, 0.0]),
        );
        store.upsert_alias("mel", "Mel", e.id, None).unwrap();

        let scored = retrieve_candidates(
            &store,
            "Mel",
            Some(&[1.0_f32, 0.0, 0.0]),
            "default",
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .unwrap();
        assert_eq!(scored.len(), 1);
        let c = &scored[0];
        assert_eq!(c.measurements.len(), 3, "s1 + s2 + s4 all present");
    }

    #[test]
    fn driver_e2e_empty_store_returns_empty() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now_dt = Utc::now();

        let scored = retrieve_candidates(
            &store,
            "Anything",
            Some(&[0.5_f32, 0.5, 0.5]),
            "default",
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .unwrap();
        assert!(scored.is_empty(), "empty store → empty driver output");
    }

    #[test]
    fn driver_e2e_namespace_isolation() {
        // Two stores on the same conn under different namespaces; querying
        // one must never surface the other's entities.
        let mut conn = fresh_conn();
        let now_dt = Utc::now();
        // Insert into namespace A.
        let e_a = {
            let mut store_a =
                SqliteGraphStore::new(&mut conn).with_namespace("ns_a").with_embedding_dim(3);
            let e = insert_test_entity(
                &mut store_a,
                "Mel",
                EntityKind::Person,
                now_dt,
                None,
            );
            store_a.upsert_alias("mel", "Mel", e.id, None).unwrap();
            e
        };

        // Query from namespace B — must see nothing.
        let store_b = SqliteGraphStore::new(&mut conn).with_namespace("ns_b").with_embedding_dim(3);
        let scored = retrieve_candidates(
            &store_b,
            "Mel",
            None,
            "ns_b", // CandidateQuery.namespace is what filters.
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .unwrap();
        assert!(scored.is_empty(), "namespace isolation must hold");

        // Sanity: querying ns_a returns the hit.
        let store_a2 =
            SqliteGraphStore::new(&mut conn).with_namespace("ns_a").with_embedding_dim(3);
        let got = retrieve_candidates(
            &store_a2,
            "Mel",
            None,
            "ns_a",
            dt_to_unix(now_dt),
            &RetrievalParams::default(),
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].match_row.entity_id, e_a.id);
    }

    #[test]
    fn driver_e2e_kind_filter() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now_dt = Utc::now();

        let e_person = insert_test_entity(
            &mut store,
            "Mel",
            EntityKind::Person,
            now_dt,
            None,
        );
        store.upsert_alias("mel", "Mel", e_person.id, None).unwrap();

        // Same alias text, but a Place entity — must be filtered out when we
        // ask only for Persons.
        let e_place = insert_test_entity(
            &mut store,
            "Mel",
            EntityKind::Place,
            now_dt,
            None,
        );
        store.upsert_alias("mel-place", "Mel", e_place.id, None).unwrap();

        let params = RetrievalParams {
            kind_filter: Some(EntityKind::Person),
            ..RetrievalParams::default()
        };
        let scored = retrieve_candidates(
            &store,
            "Mel",
            None,
            "default",
            dt_to_unix(now_dt),
            &params,
        )
        .unwrap();
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].match_row.entity_id, e_person.id);
        assert_eq!(scored[0].match_row.kind, EntityKind::Person);
    }

    #[test]
    fn driver_e2e_top_k_capped_at_max() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn).with_embedding_dim(3);
        let now_dt = Utc::now();

        // Insert 60 entities with distinct embeddings — all alike enough to
        // be candidates.
        for i in 0..60_u32 {
            let _ = insert_test_entity(
                &mut store,
                &format!("e_{i}"),
                EntityKind::Concept,
                now_dt,
                Some(vec![i as f32 * 0.001, 1.0, 0.0]),
            );
        }

        // Caller asks for 100; store hard-caps at MAX_TOP_K (50).
        let params = RetrievalParams {
            top_k: 100,
            ..RetrievalParams::default()
        };
        let scored = retrieve_candidates(
            &store,
            "x",
            Some(&[0.0_f32, 1.0, 0.0]),
            "default",
            dt_to_unix(now_dt),
            &params,
        )
        .unwrap();
        // Hard cap from the store is MAX_TOP_K = 50 (see graph::store::MAX_TOP_K).
        assert!(
            scored.len() <= 50,
            "store MAX_TOP_K must bound driver output (got {})",
            scored.len()
        );
    }
}
