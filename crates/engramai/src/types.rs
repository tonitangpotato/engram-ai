//! Core memory data types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Access control permission levels for multi-agent memory sharing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Read access: can recall memories from this namespace
    Read,
    /// Write access: can store memories to this namespace
    Write,
    /// Admin access: full control (read + write + grant/revoke)
    Admin,
}

impl Permission {
    /// Check if this permission includes read access.
    pub fn can_read(&self) -> bool {
        matches!(self, Permission::Read | Permission::Write | Permission::Admin)
    }

    /// Check if this permission includes write access.
    pub fn can_write(&self) -> bool {
        matches!(self, Permission::Write | Permission::Admin)
    }

    /// Check if this permission includes admin access.
    pub fn is_admin(&self) -> bool {
        matches!(self, Permission::Admin)
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Permission::Read => write!(f, "read"),
            Permission::Write => write!(f, "write"),
            Permission::Admin => write!(f, "admin"),
        }
    }
}

impl FromStr for Permission {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "read" => Ok(Permission::Read),
            "write" => Ok(Permission::Write),
            "admin" => Ok(Permission::Admin),
            _ => Err(format!("unknown permission: {}", s)),
        }
    }
}

/// Access control list entry for namespace permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclEntry {
    /// Agent ID that has this permission
    pub agent_id: String,
    /// Namespace this permission applies to ("*" = all namespaces)
    pub namespace: String,
    /// Permission level
    pub permission: Permission,
    /// Agent ID that granted this permission
    pub granted_by: String,
    /// When this permission was granted
    pub created_at: DateTime<Utc>,
}

/// Memory type classification following neuroscience categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    /// Factual knowledge: "SaltyHall uses Supabase"
    Factual,
    /// Episodic events: "On Feb 2 we shipped 10 features"
    Episodic,
    /// Relational knowledge: "potato prefers action over discussion"
    Relational,
    /// Emotional memories: "potato said I kinda like you"
    Emotional,
    /// Procedural knowledge: "Use www.moltbook.com not moltbook.com"
    Procedural,
    /// Opinions: "I think graph+text hybrid is best"
    Opinion,
    /// Causal relationships: "changing auth.py → downstream tests fail"
    Causal,
}

impl MemoryType {
    /// Default importance value for this memory type.
    pub fn default_importance(&self) -> f64 {
        match self {
            MemoryType::Factual => 0.3,
            MemoryType::Episodic => 0.4,
            MemoryType::Relational => 0.6,
            MemoryType::Emotional => 0.9,
            MemoryType::Procedural => 0.5,
            MemoryType::Opinion => 0.3,
            MemoryType::Causal => 0.7,
        }
    }

    /// Default decay rate (mu parameter) for this memory type.
    /// Lower = decays slower = lasts longer.
    pub fn default_decay_rate(&self) -> f64 {
        match self {
            MemoryType::Factual => 0.03,
            MemoryType::Episodic => 0.10,
            MemoryType::Relational => 0.02,
            MemoryType::Emotional => 0.01,
            MemoryType::Procedural => 0.01,
            MemoryType::Opinion => 0.05,
            MemoryType::Causal => 0.02,
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryType::Factual => write!(f, "factual"),
            MemoryType::Episodic => write!(f, "episodic"),
            MemoryType::Relational => write!(f, "relational"),
            MemoryType::Emotional => write!(f, "emotional"),
            MemoryType::Procedural => write!(f, "procedural"),
            MemoryType::Opinion => write!(f, "opinion"),
            MemoryType::Causal => write!(f, "causal"),
        }
    }
}

/// Memory consolidation layer (Memory Chain Model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryLayer {
    /// Core: always loaded, distilled knowledge
    Core,
    /// Working: recent daily notes (7 days)
    Working,
    /// Archive: old, searched on demand
    Archive,
}

impl fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryLayer::Core => write!(f, "core"),
            MemoryLayer::Working => write!(f, "working"),
            MemoryLayer::Archive => write!(f, "archive"),
        }
    }
}

/// A single memory entry with all metadata for cognitive models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Unique memory ID (8-char UUID prefix)
    pub id: String,
    /// Memory content (natural language)
    pub content: String,
    /// Memory type
    pub memory_type: MemoryType,
    /// Current layer
    pub layer: MemoryLayer,
    
    /// Creation timestamp — wall-clock time when this row entered the DB.
    /// **Drives lifecycle/decay** (Ebbinghaus age, recency scoring).
    /// Always `Utc::now()` at insert time. Never overridden by callers.
    pub created_at: DateTime<Utc>,
    /// Optional event time — when the underlying event/fact actually occurred.
    /// **Drives temporal grounding & temporal queries** (e.g. "what happened
    /// last Tuesday"). `None` means "we don't know"; readers fall back to
    /// `created_at`. Set explicitly by callers via `StorageMeta.occurred_at`
    /// when ingesting historical content (gold conversations, replays).
    ///
    /// See ISS-103 for why this is split out from `created_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<DateTime<Utc>>,
    /// All access timestamps (for ACT-R base-level activation)
    pub access_times: Vec<DateTime<Utc>>,
    
    /// Working memory strength (hippocampal trace, fast decay)
    pub working_strength: f64,
    /// Core memory strength (neocortical trace, slow decay)
    pub core_strength: f64,
    
    /// Importance/emotional modulation (0-1)
    pub importance: f64,
    /// Pinned memories never decay
    pub pinned: bool,
    
    /// Number of consolidation cycles
    pub consolidation_count: i32,
    /// Last consolidation timestamp
    pub last_consolidated: Option<DateTime<Utc>>,
    
    /// Source identifier
    pub source: String,
    
    /// Contradiction links (legacy, penalty-based)
    pub contradicts: Option<String>,
    pub contradicted_by: Option<String>,
    
    /// Supersession link (filter-based).
    /// If set, this memory is excluded from all recall results.
    /// Contains the ID of the memory that replaced this one.
    pub superseded_by: Option<String>,
    
    /// Optional structured metadata (JSON)
    pub metadata: Option<serde_json::Value>,
}

impl MemoryRecord {
    /// Age in hours since creation.
    pub fn age_hours(&self) -> f64 {
        let now = Utc::now();
        (now - self.created_at).num_seconds() as f64 / 3600.0
    }

    /// Age in days since creation.
    pub fn age_days(&self) -> f64 {
        self.age_hours() / 24.0
    }

    /// Event time for temporal grounding/queries.
    ///
    /// Returns `occurred_at` if set, otherwise falls back to `created_at`.
    /// Use this — NOT `created_at` directly — for any code that asks
    /// "when did the event in this memory happen?":
    ///   - Temporal range filtering ("memories about last Tuesday")
    ///   - Reference anchor for natural-language relative time parsing
    ///   - User-facing date display
    ///
    /// Use `created_at` directly only for lifecycle concerns (decay, recency
    /// scoring of how long the memory has been in the DB).
    ///
    /// See ISS-103.
    pub fn event_time(&self) -> DateTime<Utc> {
        self.occurred_at.unwrap_or(self.created_at)
    }

    /// The store-time-derived temporal mark for this memory, if the
    /// enrichment pipeline produced one (ISS-190 / ISS-191 AC-1).
    ///
    /// engram is a memory **substrate**: consumers must not reach into
    /// raw `metadata` JSON paths (`metadata.engram.dimensions.temporal`)
    /// to read the derived temporal value — that couples them to the
    /// storage layout. This accessor parses the row's metadata through
    /// the canonical [`Dimensions::from_stored_metadata`] path and
    /// returns the typed [`TemporalMark`], so every consumer reads the
    /// derivation the same way.
    ///
    /// Returns `None` when there is no metadata, no temporal dimension,
    /// or the metadata fails to parse (a malformed blob is treated as
    /// "no derived mark", never an error — read paths must not panic on
    /// a bad row). `core_fact` is taken from `content`, matching the
    /// canonical parse contract.
    pub fn derived_temporal_mark(&self) -> Option<crate::dimensions::TemporalMark> {
        let metadata = self.metadata.as_ref()?;
        crate::dimensions::Dimensions::from_stored_metadata(metadata, &self.content)
            .ok()?
            .temporal
    }

    /// The store-time-derived temporal value rendered for a downstream
    /// answer model (ISS-191).
    ///
    /// Thin convenience over [`derived_temporal_mark`](Self::derived_temporal_mark):
    /// returns the [`Display`](std::fmt::Display) rendering of the mark
    /// (the resolved string with its provenance for `Vague`, an ISO date
    /// for the typed variants). `None` when no derived mark exists.
    ///
    /// This is exactly the string a context-assembly layer should prefer
    /// over the raw `occurred_at` date when present.
    pub fn derived_temporal_value(&self) -> Option<String> {
        self.derived_temporal_mark().map(|m| m.to_string())
    }
}

/// Search result with activation score and confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub record: MemoryRecord,
    pub activation: f64,
    pub confidence: f64,
    pub confidence_label: String,
}

/// Memory system statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_memories: usize,
    pub by_type: std::collections::HashMap<String, TypeStats>,
    pub by_layer: std::collections::HashMap<String, LayerStats>,
    pub pinned: usize,
    pub uptime_hours: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeStats {
    pub count: usize,
    pub avg_strength: f64,
    pub avg_importance: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerStats {
    pub count: usize,
    pub avg_working: f64,
    pub avg_core: f64,
}

/// Outcome of merging a duplicate memory into an existing one.
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    /// The ID of the existing memory that was merged into
    pub memory_id: String,
    /// Whether the content was updated (new content was significantly longer)
    pub content_updated: bool,
    /// Total number of times this memory has been merged into
    pub merge_count: i32,
}

/// A Hebbian link between memories from different namespaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossLink {
    /// Source memory ID
    pub source_id: String,
    /// Source namespace
    pub source_ns: String,
    /// Target memory ID
    pub target_id: String,
    /// Target namespace
    pub target_ns: String,
    /// Link strength (0.0-1.0)
    pub strength: f64,
    /// Optional description or context
    pub description: Option<String>,
}

/// A Hebbian link entry from the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HebbianLink {
    /// Source memory ID
    pub source_id: String,
    /// Target memory ID
    pub target_id: String,
    /// Link strength
    pub strength: f64,
    /// Number of co-activations
    pub coactivation_count: i32,
    /// Link direction
    pub direction: String,
    /// When the link was created
    pub created_at: DateTime<Utc>,
    /// Source memory namespace (if known)
    pub source_ns: Option<String>,
    /// Target memory namespace (if known)
    pub target_ns: Option<String>,
}

/// Result of recall with associations.
#[derive(Debug, Clone, Serialize)]
pub struct RecallWithAssociationsResult {
    /// Main recall results
    pub memories: Vec<RecallResult>,
    /// Cross-namespace associations found
    pub cross_links: Vec<CrossLink>,
}

/// Error type for supersession operations.
#[derive(Debug, thiserror::Error)]
pub enum SupersessionError {
    /// Memory ID not found in storage.
    #[error("Memory not found: {0}")]
    NotFound(String),

    /// Cannot supersede a memory with itself.
    #[error("Cannot supersede a memory with itself: {0}")]
    SelfSupersession(String),

    /// Cross-namespace supersession not allowed at the storage layer.
    #[error("Cross-namespace supersession not allowed: {old_ns} → {new_ns}")]
    CrossNamespace { old_ns: String, new_ns: String },

    /// Bulk supersession failed — some IDs are invalid.
    #[error("Bulk supersession failed — invalid IDs: {0:?}")]
    InvalidIds(Vec<String>),

    /// Database error.
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
}

/// Info about a superseded memory for observability listing.
#[derive(Debug, Clone)]
pub struct SupersessionInfo {
    /// The superseded memory record.
    pub superseded: MemoryRecord,
    /// The ID of the memory that replaced this one.
    pub superseded_by_id: String,
    /// The final non-superseded memory in the chain (None if cycle detected).
    pub chain_head: Option<String>,
}

/// Result of a bulk correction operation.
#[derive(Debug, Clone)]
pub struct BulkCorrectionResult {
    /// ID of the newly created correction memory.
    pub new_id: String,
    /// How many memories were superseded.
    pub superseded_count: usize,
    /// IDs of all superseded memories.
    pub superseded_ids: Vec<String>,
}

#[cfg(test)]
mod derived_temporal_tests {
    use super::*;
    use crate::dimensions::{Dimensions, TemporalMark};
    use chrono::NaiveDate;
    use serde_json::json;

    /// Build a minimal `MemoryRecord` carrying the given metadata blob and content.
    /// Mirrors the field set used by the other module test helpers.
    fn record_with(metadata: Option<serde_json::Value>, content: &str) -> MemoryRecord {
        let now = Utc::now();
        MemoryRecord {
            id: "rec0".to_string(),
            content: content.to_string(),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: now,
            occurred_at: None,
            access_times: vec![now],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata,
        }
    }

    /// Wrap a `Dimensions` into the canonical v2 stored-metadata layout.
    fn v2_metadata(dims: &Dimensions) -> serde_json::Value {
        json!({ "engram": { "dimensions": serde_json::to_value(dims).unwrap() } })
    }

    #[test]
    fn derived_mark_returns_store_time_vague_value() {
        // The ISS-190 case: extractor resolves a relative/duration phrase into a
        // provenance-bearing Vague mark. The consumer must read it back typed,
        // WITHOUT touching raw `metadata.engram.dimensions.temporal` JSON paths.
        let resolved = "~2020 (owned for 3 years as of 2023-03-27)";
        let mut dims = Dimensions::minimal("Audrey adopted Pepper, Precious and Panda").unwrap();
        dims.temporal = Some(TemporalMark::Vague(resolved.to_string()));
        let rec = record_with(Some(v2_metadata(&dims)), "Audrey adopted Pepper, Precious and Panda");

        match rec.derived_temporal_mark() {
            Some(TemporalMark::Vague(s)) => assert_eq!(s, resolved),
            other => panic!("expected Vague derived mark, got {other:?}"),
        }
        // The Display convenience surfaces the same verbatim string for the
        // downstream answer model (this is what the bench context block emits).
        assert_eq!(rec.derived_temporal_value().as_deref(), Some(resolved));
    }

    #[test]
    fn derived_mark_returns_typed_day_variant() {
        let mut dims = Dimensions::minimal("event content").unwrap();
        dims.temporal = Some(TemporalMark::Day(NaiveDate::from_ymd_opt(2024, 1, 10).unwrap()));
        let rec = record_with(Some(v2_metadata(&dims)), "event content");

        match rec.derived_temporal_mark() {
            Some(TemporalMark::Day(d)) => assert_eq!(d, NaiveDate::from_ymd_opt(2024, 1, 10).unwrap()),
            other => panic!("expected Day, got {other:?}"),
        }
        // typed variants render as ISO dates, not provenance strings.
        assert_eq!(rec.derived_temporal_value().as_deref(), Some("2024-01-10"));
    }

    #[test]
    fn derived_mark_is_none_without_metadata() {
        let rec = record_with(None, "no metadata content");
        assert!(rec.derived_temporal_mark().is_none());
        assert!(rec.derived_temporal_value().is_none());
    }

    #[test]
    fn derived_mark_is_none_when_no_temporal_dimension() {
        // metadata parses fine but carries no temporal dimension.
        let dims = Dimensions::minimal("content with no temporal").unwrap();
        let rec = record_with(Some(v2_metadata(&dims)), "content with no temporal");
        assert!(rec.derived_temporal_mark().is_none());
        assert!(rec.derived_temporal_value().is_none());
    }

    #[test]
    fn derived_mark_is_none_on_malformed_metadata() {
        // A bad blob must be treated as "no derived mark", never a panic.
        let rec = record_with(Some(json!({"garbage": [1, 2, 3]})), "content survives");
        assert!(rec.derived_temporal_mark().is_none());
    }
}
