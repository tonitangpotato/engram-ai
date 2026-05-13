//! v0.4 substrate types: `Node`, `Edge`, kind enums, and per-kind
//! typed-attribute views. See `mod.rs` for module-level rationale and
//! `design.md` §3.1 / §3.2 for the source-of-truth schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::str::FromStr;

// =============================================================================
// NodeKind
// =============================================================================

/// Closed enum of node kinds stored in `nodes.node_kind` (TEXT).
///
/// Per design §3.1, the column is conceptually open-ended (`memory|entity|
/// topic|insight|episode|plan|...`) but every kind that participates in
/// dual-write / backfill / read-switch must be explicitly declared, so
/// we encode them as a closed enum with a wildcard `Other` for forward
/// compatibility with rows that may appear on a partial mid-migration DB.
///
/// `Other(String)` is **not** intended to be written by application code
/// (the writer layer will reject it); it exists so that a `SELECT … FROM
/// nodes` from a future version of engram against an older binary
/// produces parseable rows rather than panicking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Memory,
    Entity,
    Topic,
    Insight,
    Episode,
    Plan,
    /// Forward-compat escape hatch. Application code must not write this.
    #[serde(untagged)]
    Other(String),
}

impl NodeKind {
    /// The SQL representation (matches `nodes.node_kind` text values).
    pub fn as_str(&self) -> &str {
        match self {
            NodeKind::Memory => "memory",
            NodeKind::Entity => "entity",
            NodeKind::Topic => "topic",
            NodeKind::Insight => "insight",
            NodeKind::Episode => "episode",
            NodeKind::Plan => "plan",
            NodeKind::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for NodeKind {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "memory" => NodeKind::Memory,
            "entity" => NodeKind::Entity,
            "topic" => NodeKind::Topic,
            "insight" => NodeKind::Insight,
            "episode" => NodeKind::Episode,
            "plan" => NodeKind::Plan,
            other => NodeKind::Other(other.to_string()),
        })
    }
}

// =============================================================================
// EdgeKind
// =============================================================================

/// Closed enum of edge kinds stored in `edges.edge_kind` (TEXT).
///
/// Per design §3.2, these are the six top-level relation families. Unlike
/// `NodeKind` there is **no** `Other` escape hatch — the design fixes the
/// edge-kind taxonomy at six and the writer layer rejects anything else
/// (this is what makes the §3.2 partial-unique indexes safe).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Structural,
    Associative,
    Containment,
    Provenance,
    Temporal,
    Supersession,
}

impl EdgeKind {
    pub fn as_str(&self) -> &str {
        match self {
            EdgeKind::Structural => "structural",
            EdgeKind::Associative => "associative",
            EdgeKind::Containment => "containment",
            EdgeKind::Provenance => "provenance",
            EdgeKind::Temporal => "temporal",
            EdgeKind::Supersession => "supersession",
        }
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse failures for `EdgeKind` (a fixed taxonomy: no `Other`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEdgeKind(pub String);

impl fmt::Display for UnknownEdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown edge_kind {:?} (expected one of: structural, \
                   associative, containment, provenance, temporal, supersession)",
               self.0)
    }
}

impl std::error::Error for UnknownEdgeKind {}

impl FromStr for EdgeKind {
    type Err = UnknownEdgeKind;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "structural"   => EdgeKind::Structural,
            "associative"  => EdgeKind::Associative,
            "containment"  => EdgeKind::Containment,
            "provenance"   => EdgeKind::Provenance,
            "temporal"     => EdgeKind::Temporal,
            "supersession" => EdgeKind::Supersession,
            other => return Err(UnknownEdgeKind(other.to_string())),
        })
    }
}

// =============================================================================
// MemoryLayer (memory-specific sub-classification, per §3.1)
// =============================================================================

/// `nodes.layer` (TEXT, nullable). Only set for `node_kind = 'memory'`.
///
/// Mirrors the legacy v0.3 `MemoryLayer` triple (`core | working | archive`)
/// without depending on `crate::types::MemoryLayer` so that the substrate
/// module stays a leaf in the dep graph. A `From` adapter to/from the
/// legacy type will be added when dual-write lands (T12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    Core,
    Working,
    Archive,
}

impl MemoryLayer {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryLayer::Core => "core",
            MemoryLayer::Working => "working",
            MemoryLayer::Archive => "archive",
        }
    }
}

impl FromStr for MemoryLayer {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "core" => MemoryLayer::Core,
            "working" => MemoryLayer::Working,
            "archive" => MemoryLayer::Archive,
            other => return Err(format!("unknown memory layer {other:?}")),
        })
    }
}

// =============================================================================
// MemoryType (memory-specific sub-classification, per §3.1)
// =============================================================================

/// `nodes.memory_type` (TEXT, nullable). Only set for memory rows.
/// Mirrors `crate::types::MemoryType` variants without the dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Factual,
    Episodic,
    Relational,
    Emotional,
    Procedural,
    Opinion,
    Causal,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Factual    => "factual",
            MemoryType::Episodic   => "episodic",
            MemoryType::Relational => "relational",
            MemoryType::Emotional  => "emotional",
            MemoryType::Procedural => "procedural",
            MemoryType::Opinion    => "opinion",
            MemoryType::Causal     => "causal",
        }
    }
}

impl FromStr for MemoryType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "factual"    => MemoryType::Factual,
            "episodic"   => MemoryType::Episodic,
            "relational" => MemoryType::Relational,
            "emotional"  => MemoryType::Emotional,
            "procedural" => MemoryType::Procedural,
            "opinion"    => MemoryType::Opinion,
            "causal"     => MemoryType::Causal,
            other => return Err(format!("unknown memory_type {other:?}")),
        })
    }
}

// =============================================================================
// Node — row mirror of `nodes` (§3.1)
// =============================================================================

/// Plain Rust mirror of one row in the `nodes` table.
///
/// **Time representation**: `f64` unix-seconds (matches SQL `REAL`). This
/// is a deliberate departure from `graph::Edge` (chrono `DateTime<Utc>`)
/// because the substrate layer is intentionally close to SQL and avoids
/// chrono in its public surface. Higher-level callers convert at the
/// boundary.
///
/// **`attributes`**: authoritative storage is `serde_json::Value` (matches
/// the SQL TEXT-as-JSON column). For typed access use
/// [`Node::typed_attributes`] / [`Node::set_typed_attributes`], which
/// project to/from [`NodeAttributes`] per kind.
///
/// **Field grouping** mirrors the SQL DDL grouping in §3.1
/// (identity / memory-specific / content / vector / temporal / decay /
/// supersession / provenance / namespace + audit / fts surrogate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    // --- identity ---
    pub id: String,
    pub node_kind: NodeKind,
    pub namespace: String,

    // --- memory-specific sub-classification (None for non-memory kinds) ---
    pub layer: Option<MemoryLayer>,
    pub memory_type: Option<MemoryType>,

    // --- content ---
    pub content: String,
    pub summary: String,
    pub attributes: Value,

    // --- vector ---
    pub embedding: Option<Vec<u8>>,        // raw bytes; encoding is the
                                            // writer's contract
    pub embedding_model: Option<String>,    // NULL iff embedding NULL

    // --- temporal (bi-temporal) ---
    pub occurred_at: Option<f64>,           // event time (memory)
    pub valid_from: Option<f64>,            // truth window start
    pub valid_to: Option<f64>,              // truth window end
    pub created_at: f64,                    // ingest wall-clock
    pub updated_at: f64,
    pub first_seen: Option<f64>,            // entity observation window
    pub last_seen: Option<f64>,

    // --- decay / activation / strength ---
    pub activation: f64,
    pub working_strength: f64,
    pub core_strength: f64,

    // --- supersession (lifecycle) ---
    pub superseded_by: Option<String>,      // node id
    pub deleted_at: Option<f64>,

    // --- provenance / agent context ---
    pub agent_id: Option<String>,
    pub source_run_id: Option<String>,      // pipeline_runs.id

    // --- fts surrogate (§3.3) ---
    pub fts_rowid: i64,
}

impl Node {
    /// Project the JSON `attributes` blob into the typed view for this
    /// node's kind. Unknown / not-yet-typed kinds yield
    /// [`NodeAttributes::Unknown`] preserving the raw JSON.
    pub fn typed_attributes(&self) -> NodeAttributes {
        NodeAttributes::from_json(&self.node_kind, &self.attributes)
    }

    /// Replace `attributes` with the JSON form of the given typed view.
    /// Panics if `typed.kind() != self.node_kind` — this is a programming
    /// error, not a runtime condition (mismatched kind would silently
    /// corrupt the row otherwise).
    pub fn set_typed_attributes(&mut self, typed: NodeAttributes) {
        assert_eq!(
            typed.kind_str(), self.node_kind.as_str(),
            "set_typed_attributes: typed view kind {:?} ≠ node_kind {:?}",
            typed.kind_str(), self.node_kind.as_str()
        );
        self.attributes = typed.into_json();
    }
}

// =============================================================================
// Edge — row mirror of `edges` (§3.2)
// =============================================================================

/// Plain Rust mirror of one row in the `edges` table.
///
/// **`target_id` vs `target_literal`**: exactly one is `Some` — the design
/// §3.2 invariant (`target_literal NULL iff target_id NOT NULL`) is the
/// writer's responsibility to enforce. The substrate type permits all
/// shapes and provides [`Edge::validate_target`] for callers who want
/// the check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    // --- identity ---
    pub id: String,
    pub source_id: String,
    pub target_id: Option<String>,
    pub target_literal: Option<Value>,      // JSON when present

    // --- typing ---
    pub edge_kind: EdgeKind,
    pub predicate_kind: String,             // 'canonical' | 'proposed'
    pub predicate: String,                  // within-kind discriminator

    // --- payload ---
    pub summary: String,
    pub attributes: Value,
    pub weight: f64,
    pub activation: f64,
    pub confidence: f64,

    // --- temporal ---
    pub valid_from: Option<f64>,
    pub valid_to: Option<f64>,
    pub recorded_at: f64,

    // --- supersession ---
    pub invalidated_at: Option<f64>,
    pub invalidated_by: Option<String>,     // edge id
    pub supersedes: Option<String>,         // edge id

    // --- affect / provenance ---
    pub agent_affect: Option<String>,
    pub source_run_id: Option<String>,
    pub source_memory_id: Option<String>,   // node id (memory)
    pub resolution_method: String,          // 'direct' | …

    // --- namespace + audit ---
    pub namespace: String,
    pub created_at: f64,
    pub updated_at: f64,
}

impl Edge {
    /// §3.2 invariant: exactly one of (`target_id`, `target_literal`) is
    /// `Some`. Returns `Ok(())` if so, else an error describing the
    /// violation.
    pub fn validate_target(&self) -> Result<(), &'static str> {
        match (self.target_id.is_some(), self.target_literal.is_some()) {
            (true, false) | (false, true) => Ok(()),
            (true, true) => Err("both target_id and target_literal set (must be exactly one)"),
            (false, false) => Err("neither target_id nor target_literal set (must be exactly one)"),
        }
    }

    pub fn typed_attributes(&self) -> EdgeAttributes {
        EdgeAttributes::from_json(&self.edge_kind, &self.attributes)
    }

    pub fn set_typed_attributes(&mut self, typed: EdgeAttributes) {
        assert_eq!(
            typed.kind_str(), self.edge_kind.as_str(),
            "set_typed_attributes: typed view kind {:?} ≠ edge_kind {:?}",
            typed.kind_str(), self.edge_kind.as_str()
        );
        self.attributes = typed.into_json();
    }
}

// =============================================================================
// Typed attribute views (§3.1 attributes column, per-kind shape)
// =============================================================================

/// Kind-specific attribute view, projected from `nodes.attributes` JSON.
///
/// Per-kind variants are added incrementally — see `mod.rs` rationale.
/// T10 ships `Memory` (driven by T12) and `Unknown` (forward-compat for
/// kinds that don't yet have writers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeAttributes {
    Memory(MemoryAttributes),
    /// Round-trips unknown / not-yet-typed kinds as raw JSON. The string
    /// records the kind for round-tripping; the value is the verbatim
    /// `attributes` blob.
    Unknown { kind: String, attributes: Value },
}

impl NodeAttributes {
    /// Wrap the raw `attributes` JSON in the typed view appropriate for
    /// `kind`. Falls back to `Unknown` for any kind not yet typed,
    /// preserving the JSON verbatim.
    pub fn from_json(kind: &NodeKind, attributes: &Value) -> Self {
        match kind {
            NodeKind::Memory => match serde_json::from_value::<MemoryAttributes>(
                attributes.clone(),
            ) {
                Ok(memory) => NodeAttributes::Memory(memory),
                // Malformed memory attributes round-trip as Unknown rather
                // than panicking — the writer is the layer that enforces
                // attribute schemas (Phase B).
                Err(_) => NodeAttributes::Unknown {
                    kind: kind.as_str().to_string(),
                    attributes: attributes.clone(),
                },
            },
            other => NodeAttributes::Unknown {
                kind: other.as_str().to_string(),
                attributes: attributes.clone(),
            },
        }
    }

    /// Serialize the typed view back to the JSON form stored in SQL.
    pub fn into_json(self) -> Value {
        match self {
            NodeAttributes::Memory(m) => serde_json::to_value(m)
                .expect("MemoryAttributes serializes to JSON"),
            NodeAttributes::Unknown { attributes, .. } => attributes,
        }
    }

    pub fn kind_str(&self) -> &str {
        match self {
            NodeAttributes::Memory(_) => "memory",
            NodeAttributes::Unknown { kind, .. } => kind.as_str(),
        }
    }
}

/// Typed attributes for `node_kind = 'memory'`.
///
/// **Scope**: T10 captures the fields needed by T12 (memory dual-write):
/// `tags`, `source`, and the original legacy `metadata` blob (kept as
/// passthrough JSON so that store_raw callers that stuff arbitrary keys
/// into metadata don't lose them on the substrate side).
///
/// More memory-specific fields can be lifted out of `metadata` into
/// first-class fields as later tasks need them — that's a backward-compat
/// extension because `metadata` is the catch-all.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MemoryAttributes {
    /// Free-form tags. Empty vec means "no tags" (round-trips as `[]`,
    /// not `null`).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional source descriptor (e.g. `"user_message"`, `"file:foo.md"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Catch-all for the legacy v0.3 `metadata` blob. Anything not yet
    /// promoted to a first-class field lives here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Kind-specific attribute view for edges. Same staged-typing story as
/// `NodeAttributes`: T10 ships `Unknown` only, then Phase B writers add
/// typed variants alongside their dual-writers (Hebbian → `Associative`,
/// containment → `Containment`, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EdgeAttributes {
    Unknown { kind: String, attributes: Value },
}

impl EdgeAttributes {
    pub fn from_json(kind: &EdgeKind, attributes: &Value) -> Self {
        EdgeAttributes::Unknown {
            kind: kind.as_str().to_string(),
            attributes: attributes.clone(),
        }
    }

    pub fn into_json(self) -> Value {
        match self {
            EdgeAttributes::Unknown { attributes, .. } => attributes,
        }
    }

    pub fn kind_str(&self) -> &str {
        match self {
            EdgeAttributes::Unknown { kind, .. } => kind.as_str(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------- NodeKind --------

    #[test]
    fn node_kind_round_trips_known_strings() {
        for s in &["memory", "entity", "topic", "insight", "episode", "plan"] {
            let k: NodeKind = s.parse().unwrap();
            assert_eq!(k.as_str(), *s, "round-trip {s}");
        }
    }

    #[test]
    fn node_kind_other_variant_preserves_unknown() {
        let k: NodeKind = "experimental_v05".parse().unwrap();
        assert_eq!(k, NodeKind::Other("experimental_v05".to_string()));
        assert_eq!(k.as_str(), "experimental_v05");
    }

    // -------- EdgeKind --------

    #[test]
    fn edge_kind_round_trips_all_six() {
        for s in &["structural", "associative", "containment",
                   "provenance", "temporal", "supersession"] {
            let k: EdgeKind = s.parse().unwrap();
            assert_eq!(k.as_str(), *s);
        }
    }

    #[test]
    fn edge_kind_rejects_unknown() {
        let err = "made_up_kind".parse::<EdgeKind>().unwrap_err();
        assert!(err.to_string().contains("made_up_kind"));
        // Error message lists the legal alternatives.
        assert!(err.to_string().contains("structural"));
    }

    // -------- MemoryLayer / MemoryType --------

    #[test]
    fn memory_layer_round_trips() {
        for s in &["core", "working", "archive"] {
            let l: MemoryLayer = s.parse().unwrap();
            assert_eq!(l.as_str(), *s);
        }
        assert!("nonsense".parse::<MemoryLayer>().is_err());
    }

    #[test]
    fn memory_type_round_trips_all_seven() {
        for s in &["factual", "episodic", "relational", "emotional",
                   "procedural", "opinion", "causal"] {
            let t: MemoryType = s.parse().unwrap();
            assert_eq!(t.as_str(), *s);
        }
        assert!("garbage".parse::<MemoryType>().is_err());
    }

    // -------- Node round-trip --------

    fn sample_memory_node() -> Node {
        Node {
            id: "mem-1".into(),
            node_kind: NodeKind::Memory,
            namespace: "default".into(),
            layer: Some(MemoryLayer::Working),
            memory_type: Some(MemoryType::Episodic),
            content: "potato discovered KC degeneration".into(),
            summary: "KC bug".into(),
            attributes: json!({"tags": ["bench", "kc"], "source": "telegram"}),
            embedding: None,
            embedding_model: None,
            occurred_at: Some(1_715_000_000.0),
            valid_from: None,
            valid_to: None,
            created_at: 1_715_000_500.0,
            updated_at: 1_715_000_500.0,
            first_seen: None,
            last_seen: None,
            activation: 0.5,
            working_strength: 1.0,
            core_strength: 0.0,
            superseded_by: None,
            deleted_at: None,
            agent_id: Some("rustclaw".into()),
            source_run_id: None,
            fts_rowid: 42,
        }
    }

    #[test]
    fn node_serde_round_trips() {
        let n = sample_memory_node();
        let json = serde_json::to_string(&n).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }

    #[test]
    fn node_typed_attributes_memory_returns_typed_view() {
        let n = sample_memory_node();
        match n.typed_attributes() {
            NodeAttributes::Memory(m) => {
                assert_eq!(m.tags, vec!["bench".to_string(), "kc".to_string()]);
                assert_eq!(m.source.as_deref(), Some("telegram"));
            }
            other => panic!("expected Memory variant, got {other:?}"),
        }
    }

    #[test]
    fn node_typed_attributes_non_memory_kind_round_trips_unknown() {
        let mut n = sample_memory_node();
        n.node_kind = NodeKind::Entity;
        n.attributes = json!({"canonical_name": "potato"});
        match n.typed_attributes() {
            NodeAttributes::Unknown { kind, attributes } => {
                assert_eq!(kind, "entity");
                assert_eq!(attributes, json!({"canonical_name": "potato"}));
            }
            _ => panic!("entity kind has no typed variant yet"),
        }
    }

    #[test]
    fn node_set_typed_attributes_round_trip() {
        let mut n = sample_memory_node();
        let new = MemoryAttributes {
            tags: vec!["x".into()],
            source: None,
            metadata: Some(json!({"k": 1})),
        };
        n.set_typed_attributes(NodeAttributes::Memory(new.clone()));
        match n.typed_attributes() {
            NodeAttributes::Memory(m) => {
                assert_eq!(m.tags, vec!["x".to_string()]);
                assert_eq!(m.source, None);
                assert_eq!(m.metadata, Some(json!({"k": 1})));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    #[should_panic(expected = "≠ node_kind")]
    fn node_set_typed_attributes_panics_on_kind_mismatch() {
        let mut n = sample_memory_node();
        n.node_kind = NodeKind::Entity;
        // Memory typed view + Entity node kind — programming error.
        n.set_typed_attributes(NodeAttributes::Memory(MemoryAttributes::default()));
    }

    #[test]
    fn malformed_memory_attributes_round_trip_as_unknown() {
        let mut n = sample_memory_node();
        // tags should be Vec<String>; force it to a string and expect graceful
        // fallback to Unknown rather than a panic.
        n.attributes = json!({"tags": "should-be-list"});
        match n.typed_attributes() {
            NodeAttributes::Unknown { kind, .. } => assert_eq!(kind, "memory"),
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    // -------- Edge round-trip & invariants --------

    fn sample_edge() -> Edge {
        Edge {
            id: "edge-1".into(),
            source_id: "mem-1".into(),
            target_id: Some("mem-2".into()),
            target_literal: None,
            edge_kind: EdgeKind::Associative,
            predicate_kind: "canonical".into(),
            predicate: "co_activated".into(),
            summary: "".into(),
            attributes: json!({"coactivation_count": 3}),
            weight: 0.7,
            activation: 0.0,
            confidence: 0.9,
            valid_from: None,
            valid_to: None,
            recorded_at: 1_715_000_500.0,
            invalidated_at: None,
            invalidated_by: None,
            supersedes: None,
            agent_affect: None,
            source_run_id: None,
            source_memory_id: None,
            resolution_method: "direct".into(),
            namespace: "default".into(),
            created_at: 1_715_000_500.0,
            updated_at: 1_715_000_500.0,
        }
    }

    #[test]
    fn edge_serde_round_trips() {
        let e = sample_edge();
        let json = serde_json::to_string(&e).unwrap();
        let back: Edge = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn edge_validate_target_happy_paths() {
        // target_id set, literal None: OK
        let e = sample_edge();
        assert!(e.validate_target().is_ok());

        // target_literal set, id None: OK
        let mut e2 = sample_edge();
        e2.target_id = None;
        e2.target_literal = Some(json!("literal value"));
        assert!(e2.validate_target().is_ok());
    }

    #[test]
    fn edge_validate_target_rejects_both() {
        let mut e = sample_edge();
        e.target_literal = Some(json!("also a literal"));
        let err = e.validate_target().unwrap_err();
        assert!(err.contains("both"));
    }

    #[test]
    fn edge_validate_target_rejects_neither() {
        let mut e = sample_edge();
        e.target_id = None;
        e.target_literal = None;
        let err = e.validate_target().unwrap_err();
        assert!(err.contains("neither"));
    }

    #[test]
    fn edge_typed_attributes_returns_unknown_for_all_kinds_today() {
        // T10 ships no typed edge variants yet; everything round-trips
        // as Unknown preserving the JSON.
        let e = sample_edge();
        match e.typed_attributes() {
            EdgeAttributes::Unknown { kind, attributes } => {
                assert_eq!(kind, "associative");
                assert_eq!(attributes, json!({"coactivation_count": 3}));
            }
        }
    }
}
