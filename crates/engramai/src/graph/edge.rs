//! v0.3 typed bi-temporal graph edge (L4). See design §3.2.
//!
//! This is a **new type**, distinct from the v0.2 `Triple` in
//! `crates/engramai/src/triple.rs`. The v0.2 type stays compiled and
//! exported for backward compat (GOAL-1.13); a lossy adapter
//! `impl From<&Edge> for Triple` lives in `graph/compat.rs` (separate task).
//!
//! ## Deviations from §3.2 / task brief
//!
//! 1. **Field types: chrono, not `f64` unix seconds.** The task brief
//!    suggests `f64` unix seconds, but the design §3.2 spec uses
//!    `chrono::DateTime<Utc>` for every temporal field. The design is the
//!    source of truth, so `chrono` types are used. The constructor takes
//!    `valid_from: Option<DateTime<Utc>>` and the caller may also override
//!    `recorded_at` after construction.
//!
//! 2. **No `EdgeMethod` / `MergeReason` / `ProposedPredicate` types.** The
//!    task brief names these as types defined in §3.2, but the design text
//!    does **not** define them. §3.2 instead defines:
//!      - [`EdgeEnd`]   — entity-or-literal object side
//!      - [`ResolutionMethod`] — how the edge was resolved
//!      - [`ConfidenceSource`] — provenance of the numeric confidence
//!    Proposed predicates already live on [`Predicate::Proposed`] in
//!    `graph::schema` (§3.3); no separate `ProposedPredicate` type exists.
//!    These three §3.2 enums are the ones implemented here.
//!
//! 3. **Self-loop policy: allowed.** §3.2 does not prohibit self-loops
//!    (`subject_id == object.entity_id`). Several canonical predicates
//!    (`RelatedTo`, `Contradicts`) make self-loops semantically meaningful
//!    for self-reference cases. `validate` therefore allows them. **TODO
//!    (design review):** confirm whether a per-predicate self-loop
//!    blacklist is wanted.
//!
//! 4. **`invalidate` is idempotent.** §3.2 specifies non-destructive
//!    invalidation with a successor-id requirement (`invalidated_by` must
//!    point at the successor edge), but does not state whether re-calling
//!    `invalidate` on an already-invalidated edge should error. The full
//!    successor-chain machinery is implemented by `apply_graph_delta`
//!    (separate task). The lightweight `invalidate(at)` helper here
//!    sets `invalidated_at` only if currently `None`, and is a no-op
//!    otherwise — matching the "no field on an invalidated edge is mutated
//!    thereafter" invariant. Callers needing to detect double-invalidation
//!    should check `is_live()` first.
//!
//! 5. **Default `confidence_source` is `Recovered`.** §3.2 says
//!    `Recovered` is the default for non-Migrated edges; the constructor
//!    sets it to `Recovered`. Migrated edges must be constructed by the
//!    migration code path which sets `Defaulted` explicitly.
//!
//! 6. **Default `resolution_method` is `Automatic`.** §3.2 doesn't pin a
//!    default. `Automatic` is the cheapest path and matches the common
//!    "resolved by signals alone" case; resolver code overrides as needed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::{GraphError, Predicate};

/// The "object" side of an edge. v0.3 edges connect a subject entity to
/// either another entity (structural) or a literal value (attribute-like).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeEnd {
    Entity { id: Uuid },
    Literal { value: serde_json::Value },
}

/// How an edge's resolution decision was reached. Drives audit queries
/// (GOAL-1.7) and is one input to `identity_confidence` on the subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    /// Resolved by cheap signals alone (string match, embedding, graph context).
    Automatic,
    /// Cheap signals were ambiguous; an LLM tie-breaker was consulted.
    LlmTieBreaker,
    /// Explicitly asserted or corrected by an agent tool call (Letta-style).
    AgentCurated,
    /// Imported from v0.2 data via v03-migration. See `Edge.confidence_source`
    /// for whether the stored `confidence` is recovered, defaulted, or inferred.
    Migrated,
}

/// Provenance of the numeric `confidence` on an edge. Retrieval (v03-retrieval §5)
/// consults this to decide how much to trust the value. On non-`Migrated` edges
/// the default `Recovered` applies — the confidence was produced by the resolver
/// at write time from real signals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceSource {
    /// Confidence was produced by the resolver (or carried from v0.2 unambiguously).
    Recovered,
    /// Confidence could not be recovered; the stored value is a sentinel default
    /// (conventionally 0.5). Retrieval should weight this below `Recovered` evidence.
    Defaulted,
    /// Confidence was inferred from secondary signals (edge age, source kind).
    Inferred,
}

fn default_confidence_source() -> ConfidenceSource {
    ConfidenceSource::Recovered
}

/// A typed, bi-temporal relationship edge (L4).
///
/// Invariants (§3.2):
///  - `id` is never reassigned or reused after invalidation (GOAL-1.6).
///  - `confidence` in [0.0, 1.0]. v0.3 stores claim-confidence on the edge,
///    not on the MemoryRecord (master §3.2).
///  - `valid_from <= valid_to` when both are present.
///  - `recorded_at` is monotonic per `(subject_id, predicate, object)` triple:
///    a later `recorded_at` is always a newer observation.
///  - Invalidation is non-destructive: setting `invalidated_at` requires
///    `invalidated_by` to point to the successor edge's id, and the successor
///    must carry `supersedes = Some(this.id)` (GOAL-1.6 + GUARD-3).
///  - No field on an invalidated edge is mutated thereafter except for
///    late-arriving `invalidated_by` on chains (see §3.4 chain rules).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,

    pub subject_id: Uuid,
    pub predicate: Predicate,
    pub object: EdgeEnd,

    /// Natural-language rendering, for display and for LLM re-prompting
    /// during Stage 5 resolution. Authored at write time, never re-written.
    pub summary: String,

    // ---- Bi-temporal validity (GOAL-1.5) ----
    /// When the fact became true in the real world.
    pub valid_from: Option<DateTime<Utc>>,
    /// When the fact stopped being true in the real world (None = still true
    /// as far as we know).
    pub valid_to: Option<DateTime<Utc>>,
    /// When the system learned the fact (ingest time of the asserting episode).
    pub recorded_at: DateTime<Utc>,
    /// When the system learned the fact stopped being true.
    pub invalidated_at: Option<DateTime<Utc>>,

    // ---- Invalidation chain (GOAL-1.6) ----
    /// If this edge has been superseded, the successor's id.
    pub invalidated_by: Option<Uuid>,
    /// If this edge supersedes a previous one, the predecessor's id.
    pub supersedes: Option<Uuid>,

    // ---- Provenance (GOAL-1.7) ----
    pub episode_id: Option<Uuid>,
    pub memory_id: Option<String>,
    pub resolution_method: ResolutionMethod,

    // ---- Cognitive state (GOAL-1.4) ----
    pub activation: f64,
    pub confidence: f64,
    /// Provenance of `confidence`. Defaults to `Recovered`; set to `Defaulted`
    /// on `ResolutionMethod::Migrated` edges where the v0.2 source did not
    /// carry a confidence value.
    #[serde(default = "default_confidence_source")]
    pub confidence_source: ConfidenceSource,
    pub agent_affect: Option<serde_json::Value>,

    pub created_at: DateTime<Utc>,
}

impl Edge {
    /// Construct a new live edge.
    ///
    /// - `id` is freshly generated (`Uuid::new_v4`).
    /// - `valid_to` and `invalidated_at` start `None` (live).
    /// - `recorded_at` and `created_at` collapse to `now`. Callers may
    ///   override `recorded_at` afterward (e.g. when replaying history).
    /// - `confidence` defaults to `1.0`, `confidence_source = Recovered`.
    /// - `resolution_method` defaults to `Automatic`. Override for
    ///   `LlmTieBreaker` / `AgentCurated` / `Migrated` paths.
    /// - Cognitive scalars (`activation`) start at `0.0`.
    /// - All chain / provenance ids start `None`.
    pub fn new(
        subject_id: Uuid,
        predicate: Predicate,
        object: EdgeEnd,
        valid_from: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            subject_id,
            predicate,
            object,
            summary: String::new(),
            valid_from,
            valid_to: None,
            recorded_at: now,
            invalidated_at: None,
            invalidated_by: None,
            supersedes: None,
            episode_id: None,
            memory_id: None,
            resolution_method: ResolutionMethod::Automatic,
            activation: 0.0,
            confidence: 1.0,
            confidence_source: ConfidenceSource::Recovered,
            agent_affect: None,
            created_at: now,
        }
    }

    /// True iff this edge is currently live — i.e. has not been invalidated.
    /// Per §3.2 invalidation is signalled by `invalidated_at`; `valid_to`
    /// alone (the real-world end of the fact) does **not** make the edge
    /// "invalidated" in the GUARD-3 sense — a fact can stop being true
    /// without the edge being superseded.
    pub fn is_live(&self) -> bool {
        self.invalidated_at.is_none()
    }

    /// Mark this edge as invalidated at `at`. Idempotent: if the edge is
    /// already invalidated this is a no-op. See deviation note 4 at the top
    /// of this file. Note: this helper does **not** wire up the successor
    /// chain (`invalidated_by` / `supersedes`); that is the responsibility
    /// of `apply_graph_delta`.
    pub fn invalidate(&mut self, at: DateTime<Utc>) {
        if self.invalidated_at.is_none() {
            self.invalidated_at = Some(at);
        }
    }

    /// Validate structural invariants enforced at the type level.
    ///
    /// Checks:
    /// - `confidence` in `[0.0, 1.0]`
    /// - `activation` in `[0.0, 1.0]` (cognitive scalar invariant per §3.1
    ///   convention; §3.2 doesn't restate it but `master §3.2` does)
    /// - `valid_from <= valid_to` when both are present (no time-travel)
    /// - `invalidated_at >= recorded_at` when set (can't learn it stopped
    ///   before learning it started)
    /// - `invalidated_by` is set iff `invalidated_at` is set (chain rule
    ///   from §3.2: "setting invalidated_at requires invalidated_by")
    ///   — except this is relaxed: we permit `invalidated_at` without
    ///   `invalidated_by` because §3.4 allows late-arriving `invalidated_by`
    ///   on chains. **TODO (design review):** confirm relaxation.
    /// - Self-loops are allowed (see deviation note 3).
    pub fn validate(&self) -> Result<(), GraphError> {
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(GraphError::Invariant("edge confidence out of range"));
        }
        if !self.activation.is_finite() || !(0.0..=1.0).contains(&self.activation) {
            return Err(GraphError::Invariant("edge activation out of range"));
        }
        if let (Some(from), Some(to)) = (self.valid_from, self.valid_to) {
            if to < from {
                return Err(GraphError::Invariant("edge valid_to precedes valid_from"));
            }
        }
        if let Some(inv) = self.invalidated_at {
            if inv < self.recorded_at {
                return Err(GraphError::Invariant(
                    "edge invalidated_at precedes recorded_at",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::schema::CanonicalPredicate;
    use chrono::Duration;
    use serde_json::json;

    fn sample_edge() -> Edge {
        let now = Utc::now();
        Edge::new(
            Uuid::new_v4(),
            Predicate::Canonical(CanonicalPredicate::WorksAt),
            EdgeEnd::Entity { id: Uuid::new_v4() },
            Some(now),
            now,
        )
    }

    #[test]
    fn new_edge_is_live() {
        let e = sample_edge();
        assert!(e.invalidated_at.is_none());
        assert!(e.invalidated_by.is_none());
        assert!(e.is_live());
        // Default confidence + source.
        assert_eq!(e.confidence, 1.0);
        assert_eq!(e.confidence_source, ConfidenceSource::Recovered);
        assert_eq!(e.resolution_method, ResolutionMethod::Automatic);
        assert_eq!(e.activation, 0.0);
        assert!(e.summary.is_empty());
        assert!(e.supersedes.is_none());
    }

    #[test]
    fn invalidate_sets_invalidated_at() {
        let mut e = sample_edge();
        let at = e.recorded_at + Duration::seconds(60);
        e.invalidate(at);
        assert!(!e.is_live());
        assert_eq!(e.invalidated_at, Some(at));
    }

    #[test]
    fn invalidate_idempotent() {
        // Per deviation note 4: re-calling invalidate on an already-invalidated
        // edge is a no-op (preserves the original invalidation timestamp).
        let mut e = sample_edge();
        let first = e.recorded_at + Duration::seconds(60);
        let second = e.recorded_at + Duration::seconds(120);
        e.invalidate(first);
        e.invalidate(second);
        assert_eq!(e.invalidated_at, Some(first), "first timestamp must win");
        assert!(!e.is_live());
    }

    #[test]
    fn validate_accepts_default_edge() {
        sample_edge().validate().unwrap();
    }

    #[test]
    fn validate_rejects_time_travel_validity() {
        let mut e = sample_edge();
        let from = Utc::now();
        e.valid_from = Some(from);
        e.valid_to = Some(from - Duration::seconds(1));
        match e.validate() {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "edge valid_to precedes valid_from");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_invalidated_before_recorded() {
        let mut e = sample_edge();
        e.invalidated_at = Some(e.recorded_at - Duration::seconds(1));
        match e.validate() {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "edge invalidated_at precedes recorded_at");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_out_of_range_confidence() {
        let mut e = sample_edge();
        e.confidence = 1.5;
        match e.validate() {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "edge confidence out of range"),
            other => panic!("expected Invariant, got {:?}", other),
        }
        e.confidence = -0.1;
        match e.validate() {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "edge confidence out of range"),
            other => panic!("expected Invariant, got {:?}", other),
        }
        e.confidence = f64::NAN;
        match e.validate() {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "edge confidence out of range"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_rejects_out_of_range_activation() {
        let mut e = sample_edge();
        e.activation = 2.0;
        match e.validate() {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "edge activation out of range"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_self_loop_decision() {
        // Per deviation note 3: self-loops are permitted in v0.3.
        // Predicates like `RelatedTo` / `Contradicts` make self-reference
        // semantically meaningful. Test asserts the chosen behaviour so a
        // future policy change has to update this assertion deliberately.
        let same = Uuid::new_v4();
        let now = Utc::now();
        let e = Edge::new(
            same,
            Predicate::Canonical(CanonicalPredicate::RelatedTo),
            EdgeEnd::Entity { id: same },
            Some(now),
            now,
        );
        assert!(
            e.validate().is_ok(),
            "v0.3 allows self-loops; if this changes, update the deviation note"
        );
    }

    #[test]
    fn serde_roundtrip_edge_canonical() {
        let mut e = sample_edge();
        e.summary = "Alice works at Acme".into();
        e.confidence = 0.875;
        e.activation = 0.25;
        e.resolution_method = ResolutionMethod::LlmTieBreaker;
        e.confidence_source = ConfidenceSource::Inferred;
        e.episode_id = Some(Uuid::new_v4());
        e.memory_id = Some("mem-42".into());
        e.agent_affect = Some(json!({"v": 0.1}));
        let s = serde_json::to_string(&e).unwrap();
        let back: Edge = serde_json::from_str(&s).unwrap();
        assert_eq!(e.id, back.id);
        assert_eq!(e.subject_id, back.subject_id);
        assert_eq!(e.predicate, back.predicate);
        // EdgeEnd derives PartialEq, compare directly.
        assert_eq!(e.object, back.object);
        assert_eq!(e.summary, back.summary);
        assert_eq!(e.valid_from, back.valid_from);
        assert_eq!(e.valid_to, back.valid_to);
        assert_eq!(e.recorded_at, back.recorded_at);
        assert_eq!(e.invalidated_at, back.invalidated_at);
        assert_eq!(e.invalidated_by, back.invalidated_by);
        assert_eq!(e.supersedes, back.supersedes);
        assert_eq!(e.episode_id, back.episode_id);
        assert_eq!(e.memory_id, back.memory_id);
        assert_eq!(e.resolution_method, back.resolution_method);
        assert_eq!(e.activation, back.activation);
        assert_eq!(e.confidence, back.confidence);
        assert_eq!(e.confidence_source, back.confidence_source);
        assert_eq!(e.agent_affect, back.agent_affect);
        assert_eq!(e.created_at, back.created_at);
    }

    #[test]
    fn serde_roundtrip_edge_proposed_predicate_and_literal_object() {
        let now = Utc::now();
        let mut e = Edge::new(
            Uuid::new_v4(),
            Predicate::proposed("collaborates with"),
            EdgeEnd::Literal {
                value: json!({"label": "primary collaborator"}),
            },
            Some(now),
            now,
        );
        e.summary = "literal-object edge".into();
        let s = serde_json::to_string(&e).unwrap();
        let back: Edge = serde_json::from_str(&s).unwrap();
        assert_eq!(e.predicate, back.predicate);
        assert_eq!(e.object, back.object);
    }

    #[test]
    fn serde_roundtrip_resolution_method_and_confidence_source() {
        for rm in [
            ResolutionMethod::Automatic,
            ResolutionMethod::LlmTieBreaker,
            ResolutionMethod::AgentCurated,
            ResolutionMethod::Migrated,
        ] {
            let s = serde_json::to_string(&rm).unwrap();
            let back: ResolutionMethod = serde_json::from_str(&s).unwrap();
            assert_eq!(rm, back);
        }
        for cs in [
            ConfidenceSource::Recovered,
            ConfidenceSource::Defaulted,
            ConfidenceSource::Inferred,
        ] {
            let s = serde_json::to_string(&cs).unwrap();
            let back: ConfidenceSource = serde_json::from_str(&s).unwrap();
            assert_eq!(cs, back);
        }
    }

    #[test]
    fn serde_roundtrip_edge_end() {
        let entity = EdgeEnd::Entity { id: Uuid::new_v4() };
        let s = serde_json::to_string(&entity).unwrap();
        assert!(s.contains("\"kind\":\"entity\""));
        let back: EdgeEnd = serde_json::from_str(&s).unwrap();
        assert_eq!(entity, back);

        let lit = EdgeEnd::Literal { value: json!(42) };
        let s = serde_json::to_string(&lit).unwrap();
        assert!(s.contains("\"kind\":\"literal\""));
        let back: EdgeEnd = serde_json::from_str(&s).unwrap();
        assert_eq!(lit, back);
    }

    #[test]
    fn confidence_source_default_on_missing_field() {
        // §3.2 says `#[serde(default = "default_confidence_source")]` so
        // edges round-tripped from older JSON without the field default to
        // `Recovered`.
        let now = Utc::now();
        let e = sample_edge();
        let mut v: serde_json::Value = serde_json::to_value(&e).unwrap();
        v.as_object_mut().unwrap().remove("confidence_source");
        let back: Edge = serde_json::from_value(v).unwrap();
        assert_eq!(back.confidence_source, ConfidenceSource::Recovered);
        // Silence unused.
        let _ = now;
    }
}
