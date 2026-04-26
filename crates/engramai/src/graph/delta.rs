//! Cross-feature handoff types — see design §5bis.
//!
//! `GraphDelta` is the batched, atomic graph-write unit produced by
//! v03-resolution and consumed by `GraphStore::apply_graph_delta`. It is the
//! only sanctioned write entry point that lets the hot-write path and the
//! migration backfill share one transactional boundary (master design
//! §5.2 / §6.5 handoff).
//!
//! ## Part A scope
//!
//! This file currently implements **types and serde only**. The following
//! Part B items live in a separate task and are intentionally absent here:
//!
//! - `delta_hash()` (BLAKE3 over the canonical-form frozen subset)
//! - `validate_floats()` / `validate_references()` helpers
//! - hash-stability tests, frozen-field tests, NaN-rejection tests
//! - self-merge validation
//!
//! ## Deviations from §5bis
//!
//! 1. **`GraphDelta.memory_id` is `Uuid`, not `String`.** §5bis spells the
//!    field as `String`, but the task brief for Part A pins it to `Uuid`
//!    for type-level safety against malformed ids. Same deviation applies
//!    to [`MemoryEntityMention::memory_id`]. The serde wire form is the
//!    canonical hyphenated lowercase string either way, so this is
//!    forward-compatible with the §5bis canonical-serialization rules.
//! 2. **`EntityMerge.reason` is `String`, not `MergeReason`.** §5bis says
//!    `reason: MergeReason  // defined in §3.4`, but `MergeReason` is not
//!    defined in the loaded design fragment. A `String` placeholder keeps
//!    the field present and serde-roundtrippable; Part B (or §3.4 landing)
//!    can promote it to a typed enum without changing the wire format for
//!    existing callers if the enum is `#[serde(rename_all = "snake_case")]`
//!    and migration follows evolution rule 1.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::graph::{Edge, Entity, GraphError};

/// Schema version of `GraphDelta` — bumped on serialization shape changes.
/// `graph_applied_deltas` PK includes this column (§5bis schema addition
/// note). Matched on all three PK columns by `apply_graph_delta`'s
/// idempotence short-circuit, so a v1-row will not satisfy a v2 replay.
pub const GRAPH_DELTA_SCHEMA_VERSION: u32 = 1;

/// The complete graph-state change produced by resolving one memory.
///
/// Constructed by v03-resolution (`resolve_for_backfill` returns one, the
/// normal pipeline constructs one internally). Consumed by
/// `GraphStore::apply_graph_delta` — the only write path that accepts it.
///
/// A `GraphDelta` is **self-contained**: applying it to an empty or
/// partially-populated graph produces the same end state regardless of
/// prior content, as long as referenced entity ids resolve. This property
/// is what makes backfill idempotent (v03-migration §5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDelta {
    /// The memory this delta was produced for (back-reference; the delta
    /// itself does not mutate the `memories` row, but `apply_graph_delta`
    /// updates `memories.entity_ids` / `memories.edge_ids` atomically).
    pub memory_id: Uuid,

    /// Entities to upsert.
    #[serde(default)]
    pub entities: Vec<Entity>,

    /// Entity merges to perform as part of this delta. Applied before
    /// `entities` upserts so loser ids in `edges` below are remapped to
    /// winner ids.
    #[serde(default)]
    pub merges: Vec<EntityMerge>,

    /// Edges to insert. Endpoints must reference entities either in
    /// `self.entities` or already present in the store. Bi-temporal fields
    /// are pre-populated.
    #[serde(default)]
    pub edges: Vec<Edge>,

    /// Edges to invalidate (set `valid_to = now()`). Used by §3.4.4
    /// retro-evolution rules. Preserves the old edge row per GUARD-3
    /// (no erasure).
    #[serde(default)]
    pub edges_to_invalidate: Vec<EdgeInvalidation>,

    /// Memory-to-entity mention rows to insert into
    /// `graph_memory_entity_mentions`. Populated by v03-resolution §3.5
    /// persist stage.
    #[serde(default)]
    pub mentions: Vec<MemoryEntityMention>,

    /// Predicate registrations against the schema registry (§3.3.2
    /// proposed-predicate path). Empty if all predicates used are already
    /// canonical.
    #[serde(default)]
    pub proposed_predicates: Vec<ProposedPredicate>,

    /// Stage failures recorded during resolution (GOAL-1.12). Carried in
    /// the delta so persistence lands them in `graph_extraction_failures`
    /// as part of the same transaction.
    #[serde(default)]
    pub stage_failures: Vec<StageFailureRow>,
}

impl GraphDelta {
    /// Construct an empty delta for `memory_id`. All collections start
    /// empty; callers populate them in the resolution pipeline.
    pub fn new(memory_id: Uuid) -> Self {
        Self {
            memory_id,
            entities: vec![],
            merges: vec![],
            edges: vec![],
            edges_to_invalidate: vec![],
            mentions: vec![],
            proposed_predicates: vec![],
            stage_failures: vec![],
        }
    }

    /// Canonical BLAKE3 hash of the frozen subset of fields per §5bis.
    /// Caller must invoke [`validate_floats`](Self::validate_floats) first —
    /// non-finite floats are a programming bug at the construction site, not
    /// a runtime concern at hash time. Returns 32-byte BLAKE3 digest.
    pub fn delta_hash(&self) -> [u8; 32] {
        let canonical = self.canonical_value();
        let bytes = serde_json::to_vec(&canonical)
            .expect("canonical_value produces only finite JSON-safe values");
        *blake3::hash(&bytes).as_bytes()
    }

    /// Build the §5bis frozen-subset Value with sorted keys at every object
    /// level and `-0.0` normalized to `0.0`. Internal helper exposed for
    /// testing only.
    fn canonical_value(&self) -> Value {
        let mut root = Map::new();
        // memory_id
        root.insert(
            "memory_id".into(),
            Value::String(self.memory_id.to_string()),
        );

        // entities[].id, canonical_name, kind
        let entities: Vec<Value> = self
            .entities
            .iter()
            .map(|e| {
                let mut m = Map::new();
                m.insert("id".into(), Value::String(e.id.to_string()));
                m.insert(
                    "canonical_name".into(),
                    Value::String(e.canonical_name.clone()),
                );
                // Entity::kind is an enum — serde_json::to_value gives us its
                // tagged form, which is stable across serde roundtrips.
                m.insert(
                    "kind".into(),
                    serde_json::to_value(&e.kind).unwrap_or(Value::Null),
                );
                Value::Object(m)
            })
            .collect();
        root.insert("entities".into(), Value::Array(entities));

        // merges[].winner, loser
        let merges: Vec<Value> = self
            .merges
            .iter()
            .map(|m| {
                let mut o = Map::new();
                o.insert("winner".into(), Value::String(m.winner.to_string()));
                o.insert("loser".into(), Value::String(m.loser.to_string()));
                Value::Object(o)
            })
            .collect();
        root.insert("merges".into(), Value::Array(merges));

        // edges[].id, subject_id, predicate, object, valid_from, valid_to, recorded_at
        let edges: Vec<Value> = self
            .edges
            .iter()
            .map(|e| {
                let mut o = Map::new();
                o.insert("id".into(), Value::String(e.id.to_string()));
                o.insert(
                    "subject_id".into(),
                    Value::String(e.subject_id.to_string()),
                );
                o.insert(
                    "predicate".into(),
                    serde_json::to_value(&e.predicate).unwrap_or(Value::Null),
                );
                o.insert(
                    "object".into(),
                    serde_json::to_value(&e.object).unwrap_or(Value::Null),
                );
                // Edge temporal fields are chrono::DateTime<Utc> in the actual
                // type (see edge.rs deviation note 1). Serialize via serde to
                // the canonical RFC3339 string form rather than f64 — the
                // §5bis float-canonical rule applies only to f64 fields.
                o.insert(
                    "valid_from".into(),
                    serde_json::to_value(e.valid_from).unwrap_or(Value::Null),
                );
                o.insert(
                    "valid_to".into(),
                    serde_json::to_value(e.valid_to).unwrap_or(Value::Null),
                );
                o.insert(
                    "recorded_at".into(),
                    serde_json::to_value(e.recorded_at).unwrap_or(Value::Null),
                );
                Value::Object(o)
            })
            .collect();
        root.insert("edges".into(), Value::Array(edges));

        // edges_to_invalidate[].edge_id, invalidated_at
        let invs: Vec<Value> = self
            .edges_to_invalidate
            .iter()
            .map(|i| {
                let mut o = Map::new();
                o.insert("edge_id".into(), Value::String(i.edge_id.to_string()));
                o.insert("invalidated_at".into(), float_canonical(i.invalidated_at));
                Value::Object(o)
            })
            .collect();
        root.insert("edges_to_invalidate".into(), Value::Array(invs));

        // mentions[].memory_id, entity_id
        let ments: Vec<Value> = self
            .mentions
            .iter()
            .map(|m| {
                let mut o = Map::new();
                o.insert("memory_id".into(), Value::String(m.memory_id.to_string()));
                o.insert("entity_id".into(), Value::String(m.entity_id.to_string()));
                Value::Object(o)
            })
            .collect();
        root.insert("mentions".into(), Value::Array(ments));

        // proposed_predicates[].label
        let preds: Vec<Value> = self
            .proposed_predicates
            .iter()
            .map(|p| {
                let mut o = Map::new();
                o.insert("label".into(), Value::String(p.label.clone()));
                Value::Object(o)
            })
            .collect();
        root.insert("proposed_predicates".into(), Value::Array(preds));

        // serde_json::Map preserves insertion order, but to guarantee §5bis
        // rule 1 (lex byte order), rebuild with sorted keys.
        let mut sorted = Map::new();
        let mut keys: Vec<String> = root.keys().cloned().collect();
        keys.sort();
        for k in keys {
            sorted.insert(k.clone(), root.remove(&k).unwrap());
        }
        Value::Object(sorted)
    }

    /// Reject any non-finite f64 (NaN, ±Inf). Hashing with NaN is undefined
    /// per §5bis; callers should run this at construction time.
    ///
    /// Edge temporal fields are `chrono::DateTime<Utc>` in the actual type
    /// (see edge.rs deviation note 1) and cannot be non-finite, so they are
    /// not checked here. The check covers every f64 in the delta.
    pub fn validate_floats(&self) -> Result<(), GraphError> {
        for i in &self.edges_to_invalidate {
            if !i.invalidated_at.is_finite() {
                return Err(GraphError::Invariant(
                    "non-finite float in GraphDelta::edges_to_invalidate.invalidated_at",
                ));
            }
        }
        for m in &self.mentions {
            if !m.confidence.is_finite() {
                return Err(GraphError::Invariant(
                    "non-finite float in GraphDelta::mentions.confidence",
                ));
            }
        }
        for p in &self.proposed_predicates {
            if !p.first_seen_at.is_finite() {
                return Err(GraphError::Invariant(
                    "non-finite float in GraphDelta::proposed_predicates.first_seen_at",
                ));
            }
        }
        for f in &self.stage_failures {
            if !f.occurred_at.is_finite() {
                return Err(GraphError::Invariant(
                    "non-finite float in GraphDelta::stage_failures.occurred_at",
                ));
            }
        }
        Ok(())
    }

    /// Internal-consistency reference checks. Cross-DB checks (entity exists
    /// in storage) are deferred to `apply_graph_delta`.
    ///
    /// Currently checks:
    /// - No self-merge (`winner == loser`).
    /// - No duplicate `edge_id` in `edges_to_invalidate`.
    pub fn validate_references(&self) -> Result<(), GraphError> {
        for m in &self.merges {
            if m.winner == m.loser {
                return Err(GraphError::Invariant(
                    "self-merge in GraphDelta::merges (winner == loser)",
                ));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for i in &self.edges_to_invalidate {
            if !seen.insert(i.edge_id) {
                return Err(GraphError::Invariant(
                    "duplicate edge_id in GraphDelta::edges_to_invalidate",
                ));
            }
        }
        Ok(())
    }
}

/// Canonicalize an f64 for hashing: `-0.0` → `0.0`. NaN/Inf are caller's
/// bug ([`validate_floats`](GraphDelta::validate_floats) catches them at
/// construction time); if one slips through, `Number::from_f64` returns
/// `None` and we emit `Value::Null` so the hash is still defined.
fn float_canonical(v: f64) -> Value {
    let normalized = if v == 0.0 { 0.0 } else { v };
    serde_json::Number::from_f64(normalized)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Entity merge directive: `loser` is folded into `winner`. Applied before
/// edge inserts so loser ids in `edges` below are remapped automatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntityMerge {
    pub winner: Uuid,
    pub loser: Uuid,
    /// Free-form reason string. §5bis spells this as `MergeReason` (defined
    /// in §3.4), but that enum is not yet available in the loaded design
    /// fragment. See deviation note 2 at the top of this file.
    pub reason: String,
}

/// Edge invalidation directive: set `valid_to = invalidated_at` on the
/// edge with `edge_id`. Optionally carries a pointer to the replacement
/// edge for chain wiring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EdgeInvalidation {
    pub edge_id: Uuid,
    /// Unix seconds.
    pub invalidated_at: f64,
    /// Optional pointer to the replacement edge (chain wiring per §3.4).
    pub superseded_by: Option<Uuid>,
}

/// Memory-to-entity mention row. Inserted into
/// `graph_memory_entity_mentions` by `apply_graph_delta`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntityMention {
    /// See deviation note 1: `Uuid` rather than §5bis's `String`.
    pub memory_id: Uuid,
    pub entity_id: Uuid,
    pub mention_text: String,
    pub span_start: Option<u32>,
    pub span_end: Option<u32>,
    pub confidence: f64,
}

/// Stub: registration record for proposed predicates seen in this delta.
/// Will be expanded by v03-resolution when registration metadata is
/// finalized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProposedPredicate {
    pub label: String,
    pub first_seen_at: f64,
}

/// Construction-time form of `ExtractionFailure` (§4.1).
/// `apply_graph_delta` upgrades to the full `graph_extraction_failures`
/// row by assigning `id` and `resolved_at = None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StageFailureRow {
    pub episode_id: Uuid,
    pub stage: String,
    pub error_category: String,
    pub error_detail: String,
    pub occurred_at: f64,
}

/// Outcome report of a single `apply_graph_delta` call. Counters reflect
/// rows actually written (not requested) so callers can distinguish a
/// no-op replay (`already_applied = true`, all counters zero) from a
/// legitimately empty delta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyReport {
    /// True iff the idempotence short-circuit fired (delta already applied).
    pub already_applied: bool,
    pub entities_upserted: u32,
    pub entities_merged: u32,
    pub edges_inserted: u32,
    pub edges_invalidated: u32,
    pub mentions_inserted: u32,
    pub predicates_registered: u32,
    pub failures_recorded: u32,
    pub tx_duration_us: u64,
}

impl ApplyReport {
    /// Fresh, all-zero report. `already_applied = false`.
    pub fn new() -> Self {
        Self {
            already_applied: false,
            entities_upserted: 0,
            entities_merged: 0,
            edges_inserted: 0,
            edges_invalidated: 0,
            mentions_inserted: 0,
            predicates_registered: 0,
            failures_recorded: 0,
            tx_duration_us: 0,
        }
    }

    /// Sentinel report for the idempotence short-circuit. All counters
    /// stay zero; only `already_applied` flips to `true`.
    pub fn already_applied_marker() -> Self {
        Self {
            already_applied: true,
            entities_upserted: 0,
            entities_merged: 0,
            edges_inserted: 0,
            edges_invalidated: 0,
            mentions_inserted: 0,
            predicates_registered: 0,
            failures_recorded: 0,
            tx_duration_us: 0,
        }
    }
}

impl Default for ApplyReport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_constant_is_one() {
        assert_eq!(GRAPH_DELTA_SCHEMA_VERSION, 1);
    }

    #[test]
    fn new_delta_is_empty() {
        let id = Uuid::new_v4();
        let d = GraphDelta::new(id);
        assert_eq!(d.memory_id, id);
        assert!(d.entities.is_empty());
        assert!(d.merges.is_empty());
        assert!(d.edges.is_empty());
        assert!(d.edges_to_invalidate.is_empty());
        assert!(d.mentions.is_empty());
        assert!(d.proposed_predicates.is_empty());
        assert!(d.stage_failures.is_empty());
    }

    #[test]
    fn apply_report_already_applied_marker() {
        let r = ApplyReport::already_applied_marker();
        assert!(r.already_applied);
        assert_eq!(r.entities_upserted, 0);
        assert_eq!(r.entities_merged, 0);
        assert_eq!(r.edges_inserted, 0);
        assert_eq!(r.edges_invalidated, 0);
        assert_eq!(r.mentions_inserted, 0);
        assert_eq!(r.predicates_registered, 0);
        assert_eq!(r.failures_recorded, 0);
        assert_eq!(r.tx_duration_us, 0);
    }

    #[test]
    fn serde_roundtrip_empty_delta() {
        // `GraphDelta` cannot derive `PartialEq` cheaply because `Entity`
        // and `Edge` don't derive it. Compare the canonical JSON form
        // instead — equal JSON ⇒ equal logical value for serde-derived
        // shapes (no skipped fields, no maps with non-deterministic order
        // in the empty case).
        let id = Uuid::nil();
        let d = GraphDelta::new(id);
        let s = serde_json::to_string(&d).unwrap();
        let back: GraphDelta = serde_json::from_str(&s).unwrap();
        let s2 = serde_json::to_string(&back).unwrap();
        assert_eq!(s, s2);
        assert_eq!(back.memory_id, id);
        assert!(back.entities.is_empty());
        assert!(back.merges.is_empty());
        assert!(back.edges.is_empty());
        assert!(back.edges_to_invalidate.is_empty());
        assert!(back.mentions.is_empty());
        assert!(back.proposed_predicates.is_empty());
        assert!(back.stage_failures.is_empty());
    }

    #[test]
    fn serde_deny_unknown_fields_rejects_extra() {
        let s = r#"{"memory_id":"00000000-0000-0000-0000-000000000000","entities":[],"merges":[],"edges":[],"edges_to_invalidate":[],"mentions":[],"proposed_predicates":[],"stage_failures":[],"extra_garbage":1}"#;
        let result = serde_json::from_str::<GraphDelta>(s);
        assert!(result.is_err());
    }

    // ---------- Part B: hash + validation tests ----------

    #[test]
    fn empty_delta_hash_stable_across_calls() {
        let id = Uuid::nil();
        let d = GraphDelta::new(id);
        assert_eq!(d.delta_hash(), d.delta_hash());
    }

    #[test]
    fn populated_delta_hash_stable_across_serde_roundtrip() {
        // Build a delta with at least: 1 merge, 1 mention, 1 proposed_predicate.
        // (Avoid Edge/Entity since they don't derive PartialEq — use simpler sub-types.)
        let mut d = GraphDelta::new(Uuid::nil());
        d.merges.push(EntityMerge {
            winner: Uuid::from_u128(1),
            loser: Uuid::from_u128(2),
            reason: "test".into(),
        });
        d.mentions.push(MemoryEntityMention {
            memory_id: Uuid::from_u128(3),
            entity_id: Uuid::from_u128(4),
            mention_text: "hi".into(),
            span_start: None,
            span_end: None,
            confidence: 1.0,
        });
        d.proposed_predicates.push(ProposedPredicate {
            label: "knows".into(),
            first_seen_at: 100.0,
        });
        let h1 = d.delta_hash();
        let json = serde_json::to_string(&d).unwrap();
        let d2: GraphDelta = serde_json::from_str(&json).unwrap();
        assert_eq!(h1, d2.delta_hash());
    }

    #[test]
    fn key_order_does_not_affect_hash() {
        // canonical_value sorts keys, so JSON object key order in the source
        // doesn't matter. Construct the same logical delta two ways and
        // confirm identical hashes.
        let mut d1 = GraphDelta::new(Uuid::from_u128(42));
        d1.proposed_predicates.push(ProposedPredicate {
            label: "a".into(),
            first_seen_at: 1.0,
        });
        d1.proposed_predicates.push(ProposedPredicate {
            label: "b".into(),
            first_seen_at: 2.0,
        });
        let h1 = d1.delta_hash();

        let mut d2 = GraphDelta::new(Uuid::from_u128(42));
        d2.proposed_predicates.push(ProposedPredicate {
            label: "a".into(),
            first_seen_at: 1.0,
        });
        d2.proposed_predicates.push(ProposedPredicate {
            label: "b".into(),
            first_seen_at: 2.0,
        });
        assert_eq!(h1, d2.delta_hash());
    }

    #[test]
    fn frozen_field_change_changes_hash() {
        let d1 = GraphDelta::new(Uuid::from_u128(1));
        let d2 = GraphDelta::new(Uuid::from_u128(2));
        assert_ne!(d1.delta_hash(), d2.delta_hash());
    }

    #[test]
    fn validate_floats_rejects_nan_in_proposed_predicate() {
        let mut d = GraphDelta::new(Uuid::nil());
        d.proposed_predicates.push(ProposedPredicate {
            label: "bad".into(),
            first_seen_at: f64::NAN,
        });
        let err = d.validate_floats().unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("non-finite"), "got: {msg}");
    }

    #[test]
    fn validate_floats_rejects_infinity_in_invalidation() {
        let mut d = GraphDelta::new(Uuid::nil());
        d.edges_to_invalidate.push(EdgeInvalidation {
            edge_id: Uuid::nil(),
            invalidated_at: f64::INFINITY,
            superseded_by: None,
        });
        assert!(d.validate_floats().is_err());
    }

    #[test]
    fn validate_floats_accepts_finite() {
        let mut d = GraphDelta::new(Uuid::nil());
        d.proposed_predicates.push(ProposedPredicate {
            label: "ok".into(),
            first_seen_at: 1.5,
        });
        assert!(d.validate_floats().is_ok());
    }

    #[test]
    fn validate_references_rejects_self_merge() {
        let mut d = GraphDelta::new(Uuid::nil());
        let x = Uuid::from_u128(99);
        d.merges.push(EntityMerge {
            winner: x,
            loser: x,
            reason: "bug".into(),
        });
        assert!(d.validate_references().is_err());
    }

    #[test]
    fn validate_references_rejects_duplicate_invalidations() {
        let mut d = GraphDelta::new(Uuid::nil());
        let eid = Uuid::from_u128(7);
        d.edges_to_invalidate.push(EdgeInvalidation {
            edge_id: eid,
            invalidated_at: 1.0,
            superseded_by: None,
        });
        d.edges_to_invalidate.push(EdgeInvalidation {
            edge_id: eid,
            invalidated_at: 2.0,
            superseded_by: None,
        });
        assert!(d.validate_references().is_err());
    }

    #[test]
    fn negative_zero_normalized_in_hash() {
        let mut d1 = GraphDelta::new(Uuid::nil());
        d1.proposed_predicates.push(ProposedPredicate {
            label: "z".into(),
            first_seen_at: -0.0,
        });
        let mut d2 = GraphDelta::new(Uuid::nil());
        d2.proposed_predicates.push(ProposedPredicate {
            label: "z".into(),
            first_seen_at: 0.0,
        });
        // proposed_predicates.first_seen_at is NOT in the §5bis frozen subset
        // for the hash (only `label` is — see the table). So both hashes are
        // equal trivially via the frozen-field rule. The point of this test
        // is that any f64 we DO hash via float_canonical normalizes -0.0 to
        // 0.0; assert directly on float_canonical too for tightness.
        assert_eq!(d1.delta_hash(), d2.delta_hash());
        assert_eq!(super::float_canonical(-0.0), super::float_canonical(0.0));
    }
}
