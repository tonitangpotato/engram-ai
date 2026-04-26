//! v0.3 canonical graph entity (L3'). See design §3.1.
//!
//! This is a **new type**, distinct from the v0.2 `ExtractedEntity` in
//! `crates/engramai/src/entities.rs`. The v0.2 mention-level type is
//! source-compatible and untouched (GOAL-1.13).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::graph::affect::SomaticFingerprint; // 8-dim, locked (master §3.7)

/// Canonical entity kind. Superset of v0.2 `EntityType`, but a new enum so
/// v0.2's serde representation is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Person,
    Organization,
    Place,
    Concept,
    Event,
    Artifact,
    Topic, // L5 bridge — see §3.4 and master §3.6
    /// Escape hatch for kinds outside the canonical set. The inner string is
    /// **normalized on insert**: lowercased, trimmed of surrounding whitespace,
    /// and NFKC-folded. Two `Other` values compare exactly on the normalized
    /// form (so `Other("Robot")` and `Other(" robot ")` collapse to one kind).
    /// Callers are discouraged from using `Other` for kinds that could
    /// plausibly be added as a canonical variant — file an issue and add the
    /// variant instead.
    ///
    /// **Use [`EntityKind::other`] to construct.** Direct `Other(s)`
    /// construction is permitted by the type system but bypasses normalization
    /// and may produce a kind that won't compare equal to the same logical
    /// value supplied through the sanctioned path.
    Other(String),
}

impl EntityKind {
    /// Sole sanctioned constructor for [`EntityKind::Other`]. Normalizes the
    /// input by trimming surrounding whitespace, lowercasing, and applying
    /// NFKC normalization (§3.1).
    pub fn other(s: &str) -> Self {
        let trimmed = s.trim().to_lowercase();
        let nfkc: String = trimmed.nfkc().collect();
        EntityKind::Other(nfkc)
    }
}

/// Append-only audit entry recording a `canonical_name` change.
/// Promoted from the legacy `attributes.history` slot so merge invariants
/// can't be clobbered by callers (§3.1).
///
/// **Deviation note:** the design §3.1 references `HistoryEntry` but does not
/// define its shape. This minimal record is provisional and should be
/// confirmed in a later design review.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: DateTime<Utc>,
    pub field: String, // e.g. "canonical_name"
    pub old: serde_json::Value,
    pub new: serde_json::Value,
    pub reason: String, // e.g. "merge", "manual_update"
}

/// A canonical graph-layer entity (L4 node).
///
/// Invariants:
///  - `id` is stable for the entity's lifetime; never reassigned on merge
///    (the loser's id is preserved in `entity_aliases`, §3.4; GOAL-1.6).
///  - `canonical_name` may be updated by later, more confident observations,
///    but the change is logged in `history` (audit, not overwrite).
///  - `first_seen <= last_seen`. Both are real-world (episode) times.
///  - `somatic_fingerprint`, when present, is an aggregate over episode-level
///    fingerprints (GOAL-1.13). Recomputed by v03-resolution; stored here.
///  - `activation`, `importance`, `identity_confidence` are in [0.0, 1.0].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub canonical_name: String,
    pub kind: EntityKind,

    /// Rolling summary. Empty until the first resolution pass completes.
    pub summary: String,

    /// Typed properties. Free-form JSON for caller-authored keys only.
    /// **Reserved keys are promoted to first-class fields** (`history`, `merged_into`
    /// — see below) and MUST NOT appear inside this map. `insert_entity` /
    /// `update_entity_*` reject any write whose `attributes` object contains
    /// a reserved key; the rejection is `GraphError::Invariant("reserved attribute key")`.
    pub attributes: serde_json::Value,

    /// Append-only audit of canonical_name changes (promoted from `attributes.history`
    /// so merge invariants can't be clobbered by a caller). Mutated only by
    /// `merge_entities` and by `update_entity_canonical_name` (not shown).
    #[serde(default)]
    pub history: Vec<HistoryEntry>,

    /// Set on merge loser: the winner's id. Promoted from `attributes.merged_into`
    /// so the redirect signal (§8 Reader semantics during merge) cannot be
    /// overwritten accidentally through `update_entity_cognitive`.
    /// None on winners and on entities that have never been merged.
    #[serde(default)]
    pub merged_into: Option<Uuid>,

    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub created_at: DateTime<Utc>, // ingest time of first asserting episode
    pub updated_at: DateTime<Utc>, // wall-clock of last mutation

    // Provenance (GOAL-1.3). Stored in join tables; materialized on read.
    #[serde(default)]
    pub episode_mentions: Vec<Uuid>,
    #[serde(default)]
    pub memory_mentions: Vec<String>,

    // Cognitive state (GOAL-1.2)
    pub activation: f64,
    pub agent_affect: Option<serde_json::Value>, // 11-dim affect tag as JSON
    pub arousal: f64,
    pub importance: f64,
    pub identity_confidence: f64,

    /// Entity-level somatic fingerprint (GOAL-1.13). None until first pass.
    pub somatic_fingerprint: Option<SomaticFingerprint>,
}

impl Entity {
    /// Construct a new canonical entity. All timestamps collapse to `now`,
    /// scalars zero-out, and provenance / fingerprint start empty.
    pub fn new(canonical_name: String, kind: EntityKind, now: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            canonical_name,
            kind,
            summary: String::new(),
            attributes: serde_json::Value::Object(Default::default()),
            history: vec![],
            merged_into: None,
            first_seen: now,
            last_seen: now,
            created_at: now,
            updated_at: now,
            episode_mentions: vec![],
            memory_mentions: vec![],
            activation: 0.0,
            agent_affect: None,
            arousal: 0.0,
            importance: 0.0,
            identity_confidence: 0.0,
            somatic_fingerprint: None,
        }
    }

    /// True iff this entity has been merged into another (i.e. is a merge loser).
    pub fn is_merged(&self) -> bool {
        self.merged_into.is_some()
    }
}

/// Attribute keys that are promoted to first-class `Entity` fields and
/// therefore forbidden inside the free-form `attributes` object (§3.1
/// reserved-keys clause).
pub const RESERVED_ATTRIBUTE_KEYS: &[&str] = &["history", "merged_into"];

/// Returns Err(GraphError::Invariant("reserved attribute key")) if the
/// attributes object contains any reserved key. This is the validation
/// hook called by future insert_entity / update_entity_* operations
/// (§3.1 reserved-keys clause).
///
/// Non-object values (null, array, scalar) are accepted; the storage layer
/// decides whether to coerce them.
pub fn validate_attributes(attrs: &serde_json::Value) -> Result<(), crate::graph::GraphError> {
    if let Some(map) = attrs.as_object() {
        for key in RESERVED_ATTRIBUTE_KEYS {
            if map.contains_key(*key) {
                return Err(crate::graph::GraphError::Invariant("reserved attribute key"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphError;
    use serde_json::json;

    #[test]
    fn new_entity_invariants() {
        let now = Utc::now();
        let e = Entity::new("Alice".into(), EntityKind::Person, now);
        assert!(e.summary.is_empty());
        assert_eq!(e.attributes, serde_json::Value::Object(Default::default()));
        assert!(e.attributes.as_object().unwrap().is_empty());
        assert_eq!(e.first_seen, now);
        assert_eq!(e.last_seen, now);
        assert_eq!(e.created_at, now);
        assert_eq!(e.updated_at, now);
        assert!(e.merged_into.is_none());
        assert!(!e.is_merged());
        assert_eq!(e.activation, 0.0);
        assert_eq!(e.arousal, 0.0);
        assert_eq!(e.importance, 0.0);
        assert_eq!(e.identity_confidence, 0.0);
        assert!(e.somatic_fingerprint.is_none());
        assert!(e.history.is_empty());
        assert!(e.episode_mentions.is_empty());
        assert!(e.memory_mentions.is_empty());
        assert!(e.agent_affect.is_none());
    }

    #[test]
    fn entity_kind_other_normalizes() {
        let a = EntityKind::other("  Robot ");
        let b = EntityKind::other("ROBOT");
        assert_eq!(a, b);
        assert_eq!(a, EntityKind::Other("robot".into()));
    }

    #[test]
    fn entity_kind_other_nfkc_or_documented() {
        // U+FB03 LATIN SMALL LIGATURE FFI → NFKC folds to "ffi".
        // unicode-normalization is in deps, so this MUST normalize.
        let folded = EntityKind::other("\u{FB03}");
        assert_eq!(folded, EntityKind::other("ffi"));
        assert_eq!(folded, EntityKind::Other("ffi".into()));
    }

    #[test]
    fn serde_roundtrip_entitykind() {
        // `rename_all = "lowercase"` produces tag-style JSON for unit variants
        // ("person", "topic", ...) and a single-key object for the data
        // variant `Other(String)` => {"other": "..."}.
        let canonical = [
            EntityKind::Person,
            EntityKind::Organization,
            EntityKind::Place,
            EntityKind::Concept,
            EntityKind::Event,
            EntityKind::Artifact,
            EntityKind::Topic,
        ];
        for k in canonical {
            let s = serde_json::to_string(&k).unwrap();
            let back: EntityKind = serde_json::from_str(&s).unwrap();
            assert_eq!(k, back, "roundtrip for {s}");
        }

        let other = EntityKind::Other("custom".into());
        let s = serde_json::to_string(&other).unwrap();
        // Documented shape: `{"other":"custom"}` (lowercase variant tag).
        assert_eq!(s, r#"{"other":"custom"}"#);
        let back: EntityKind = serde_json::from_str(&s).unwrap();
        assert_eq!(other, back);
    }

    #[test]
    fn serde_roundtrip_entity() {
        let now = Utc::now();
        let mut e = Entity::new("Alice".into(), EntityKind::Person, now);
        // Use plain finite values; NaN would break PartialEq on the manually
        // compared scalar fields below.
        e.activation = 0.25;
        e.arousal = 0.5;
        e.importance = 0.75;
        e.identity_confidence = 0.875;
        e.summary = "rolling summary".into();
        e.attributes = json!({"role": "ceo"});
        e.episode_mentions = vec![Uuid::new_v4()];
        e.memory_mentions = vec!["mem-1".into()];
        e.agent_affect = Some(json!({"v": 0.1}));
        e.somatic_fingerprint = Some(SomaticFingerprint::from_array([
            -0.1, 0.2, 0.3, 0.4, 0.5, -0.6, 0.7, 0.8,
        ]));
        e.history.push(HistoryEntry {
            at: now,
            field: "canonical_name".into(),
            old: json!("Al"),
            new: json!("Alice"),
            reason: "manual_update".into(),
        });
        e.merged_into = Some(Uuid::new_v4());

        let s = serde_json::to_string(&e).unwrap();
        let back: Entity = serde_json::from_str(&s).unwrap();

        assert_eq!(e.id, back.id);
        assert_eq!(e.canonical_name, back.canonical_name);
        assert_eq!(e.kind, back.kind);
        assert_eq!(e.summary, back.summary);
        assert_eq!(e.attributes, back.attributes);
        assert_eq!(e.history.len(), back.history.len());
        assert_eq!(e.history[0].field, back.history[0].field);
        assert_eq!(e.history[0].reason, back.history[0].reason);
        assert_eq!(e.merged_into, back.merged_into);
        assert_eq!(e.first_seen, back.first_seen);
        assert_eq!(e.last_seen, back.last_seen);
        assert_eq!(e.created_at, back.created_at);
        assert_eq!(e.updated_at, back.updated_at);
        assert_eq!(e.episode_mentions, back.episode_mentions);
        assert_eq!(e.memory_mentions, back.memory_mentions);
        assert_eq!(e.activation, back.activation);
        assert_eq!(e.arousal, back.arousal);
        assert_eq!(e.importance, back.importance);
        assert_eq!(e.identity_confidence, back.identity_confidence);
        assert_eq!(e.agent_affect, back.agent_affect);
        assert_eq!(e.somatic_fingerprint, back.somatic_fingerprint);
    }

    #[test]
    fn is_merged_reflects_merged_into() {
        let now = Utc::now();
        let mut e = Entity::new("X".into(), EntityKind::Concept, now);
        assert!(!e.is_merged());
        e.merged_into = Some(Uuid::new_v4());
        assert!(e.is_merged());
        e.merged_into = None;
        assert!(!e.is_merged());
    }

    #[test]
    fn validate_attributes_rejects_history() {
        match validate_attributes(&json!({"history": []})) {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "reserved attribute key"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_attributes_rejects_merged_into() {
        match validate_attributes(&json!({"merged_into": "00000000-0000-0000-0000-000000000000"}))
        {
            Err(GraphError::Invariant(msg)) => assert_eq!(msg, "reserved attribute key"),
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn validate_attributes_accepts_clean_object() {
        validate_attributes(&json!({"role": "ceo"})).unwrap();
    }

    #[test]
    fn validate_attributes_accepts_non_object() {
        validate_attributes(&json!(null)).unwrap();
        validate_attributes(&json!([1, 2, 3])).unwrap();
        validate_attributes(&json!(42)).unwrap();
        validate_attributes(&json!("string")).unwrap();
    }
}
