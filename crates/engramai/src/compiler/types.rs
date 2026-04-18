//! Knowledge Compiler shared types.
//!
//! All types from the KC architecture (§4) and feature-level designs.
//! Grouped: Core → Compilation → Maintenance → Platform → Error.

use std::collections::{HashMap, HashSet};
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ─── Type Aliases ────────────────────────────────────────────────────────────

/// Memory identifier (plain string alias).
pub type MemoryId = String;

// ═══════════════════════════════════════════════════════════════════════════════
//  CORE
// ═══════════════════════════════════════════════════════════════════════════════

/// Strongly-typed topic identifier.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct TopicId(pub String);

impl fmt::Display for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TopicId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TopicId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for TopicId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Current lifecycle status of a topic page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopicStatus {
    Active,
    Stale,
    Archived,
    FailedPermanent,
}

/// Bookkeeping metadata attached to every topic page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicMetadata {
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub compilation_count: u32,
    pub source_memory_ids: Vec<String>,
    pub tags: Vec<String>,
    pub quality_score: Option<f64>,
}

/// A named section within a topic page, tracking user edits.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicSection {
    pub heading: String,
    pub body: String,
    pub user_edited: bool,
    pub edited_at: Option<DateTime<Utc>>,
}

/// A compiled knowledge page representing a single topic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicPage {
    pub id: TopicId,
    pub title: String,
    pub content: String,
    pub sections: Vec<TopicSection>,
    pub summary: String,
    pub metadata: TopicMetadata,
    pub status: TopicStatus,
    pub version: u32,
}

/// Back-reference from a topic page to a source memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceMemoryRef {
    pub memory_id: String,
    pub relevance_score: f64,
    pub added_at: DateTime<Utc>,
}

/// Record of a single compilation / recompilation run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompilationRecord {
    pub topic_id: TopicId,
    pub compiled_at: DateTime<Utc>,
    pub source_count: usize,
    pub duration_ms: u64,
    pub quality_score: f64,
    pub recompile_reason: Option<String>,
}

/// Strategy used when recompiling an existing topic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecompileStrategy {
    /// Recompile on any change. >50% sources changed → Full, else Partial.
    Eager,
    /// Recompile only when significant changes accumulate. >30% → Full, >0 → Partial.
    Lazy,
    /// Never auto-trigger. Only recompile on explicit user request.
    Manual,
}

/// Top-level configuration for the Knowledge Compiler.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KcConfig {
    pub min_cluster_size: usize,
    pub quality_threshold: f64,
    pub recompile_strategy: RecompileStrategy,
    pub decay: DecayConfig,
    pub llm: LlmConfig,
    pub import: ImportConfig,
    pub intake: IntakeConfig,
    pub lifecycle: LifecycleConfig,
}

/// Configuration for topic lifecycle operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LifecycleConfig {
    /// Overlap ratio above which two topics should merge (default: 0.6)
    pub merge_overlap_threshold: f64,
    /// Maximum source memories per topic before suggesting split (default: 15)
    pub max_topic_points: usize,
    /// Minimum link strength to create a cross-topic link (default: 0.3)
    pub link_min_strength: f64,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  COMPILATION
// ═══════════════════════════════════════════════════════════════════════════════

/// A cluster of memories that may become a topic page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicCandidate {
    pub memories: Vec<String>,
    pub centroid_embedding: Vec<f32>,
    pub cohesion_score: f64,
    pub suggested_title: Option<String>,
}

impl TopicCandidate {
    /// Returns `true` when the Jaccard similarity of memory sets exceeds 0.3.
    pub fn overlaps_with(&self, other: &TopicCandidate) -> bool {
        let a: HashSet<&str> = self.memories.iter().map(|s| s.as_str()).collect();
        let b: HashSet<&str> = other.memories.iter().map(|s| s.as_str()).collect();
        let intersection = a.intersection(&b).count();
        let union = a.union(&b).count();
        if union == 0 {
            return false;
        }
        (intersection as f64 / union as f64) > 0.3
    }
}

/// Delta between the current memory store and the last compilation snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeSet {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
    pub last_compiled: Option<DateTime<Utc>>,
}

/// Decision produced by the trigger evaluator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TriggerDecision {
    /// No recompilation needed.
    Skip { reason: String },
    /// Partial recompilation — only incorporate changed memories.
    Partial { change_set: ChangeSet },
    /// Full recompilation — regenerate entire page from all sources.
    Full { change_set: ChangeSet },
}

/// A structural operation on the topic graph.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LifecycleOp {
    Merge {
        sources: Vec<TopicId>,
        target_title: String,
    },
    Split {
        source: TopicId,
        new_topics: Vec<String>,
    },
    Archive {
        topic_id: TopicId,
        reason: String,
    },
}

/// User / agent feedback on a topic page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicFeedback {
    pub topic_id: TopicId,
    pub kind: FeedbackKind,
    pub comment: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Persisted feedback entry with resolution tracking (design doc §3.5 / §3.6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedbackEntry {
    pub topic_id: TopicId,
    pub kind: FeedbackKind,
    pub comment: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub resolved: bool,
}

/// Specific kind of feedback.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FeedbackKind {
    ThumbsUp,
    ThumbsDown,
    Correction(String),
    TitleSuggestion(String),
    MergeRequest(TopicId),
    SplitRequest(Vec<String>),
}

/// Quality assessment for a single topic page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualityReport {
    pub topic_id: TopicId,
    pub coherence: f64,
    pub coverage: f64,
    pub freshness: f64,
    pub overall: f64,
    pub suggestions: Vec<String>,
}

/// Full dry-run report showing what a compilation pass *would* do.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunReport {
    pub entries: Vec<DryRunEntry>,
    pub total_topics_affected: usize,
    pub estimated_llm_calls: usize,
}

/// One line-item inside a dry-run report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunEntry {
    pub topic_id: Option<TopicId>,
    pub action: DryRunAction,
    pub affected_memories: usize,
    pub reason: String,
}

/// The kind of action a dry-run entry describes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DryRunAction {
    NewCompilation,
    Recompile,
    Merge,
    Split,
    Archive,
    Skip,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  MAINTENANCE
// ═══════════════════════════════════════════════════════════════════════════════

/// Configuration for time-based topic decay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecayConfig {
    pub check_interval_hours: u32,
    pub stale_threshold_days: u32,
    pub archive_threshold_days: u32,
    pub min_access_count: u32,
}

/// Action emitted by the decay evaluator.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DecayAction {
    MarkStale(TopicId),
    Archive(TopicId),
    Refresh(TopicId),
}

/// Strongly-typed conflict identifier.
#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictId(pub String);

impl fmt::Display for ConflictId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ConflictId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ConflictId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for ConflictId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Category of knowledge conflict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    Contradiction,
    Outdated,
    Redundant,
}

/// Processing status of a detected conflict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictStatus {
    Detected,
    Reviewing,
    Resolved,
    Dismissed,
}

/// Where a conflict was detected.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConflictScope {
    WithinTopic(TopicId),
    BetweenTopics(TopicId, TopicId),
}

/// A detected conflict between pieces of knowledge.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Conflict {
    pub id: ConflictId,
    pub conflict_type: ConflictType,
    pub scope: ConflictScope,
    pub description: String,
    pub status: ConflictStatus,
    pub detected_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// How severe a conflict is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// A conflict together with severity and supporting evidence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub conflict: Conflict,
    pub severity: ConflictSeverity,
    pub evidence: Vec<String>,
}

/// Type of directed relationship between topic pages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkType {
    References,
    DerivedFrom,
    Contradicts,
    Supersedes,
}

/// A directed link between two topic pages.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrossTopicLink {
    pub source: TopicId,
    pub target: TopicId,
    pub link_type: LinkType,
    pub strength: f64,
    pub shared_memory_ids: Vec<String>,
}

/// Health status of an inter-topic link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkStatus {
    Valid,
    Broken,
    Stale,
}

/// A link that failed validation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrokenLink {
    pub source_topic: TopicId,
    pub target_topic: TopicId,
    pub link_type: LinkType,
    pub status: LinkStatus,
    pub detected_at: DateTime<Utc>,
}

/// Possible repair actions for a broken link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LinkRepairAction {
    Remove,
    UpdateTarget(TopicId),
    MarkStale,
}

/// Result of executing a link repair action.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairResult {
    pub memory_id: String,
    pub topic_id: TopicId,
    pub action_taken: LinkRepairAction,
    pub success: bool,
    pub details: String,
}

/// Multi-dimensional health score for a topic page.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopicHealthScore {
    pub topic_id: TopicId,
    pub freshness: f64,
    pub coherence: f64,
    pub link_health: f64,
    pub access_frequency: f64,
    pub overall: f64,
}

/// A single maintenance suggestion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaintenanceRecommendation {
    pub topic_id: TopicId,
    pub action: String,
    pub priority: u8,
    pub reason: String,
}

/// Aggregate health report for the entire knowledge base.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthReport {
    pub generated_at: DateTime<Utc>,
    pub total_topics: usize,
    pub stale_topics: Vec<TopicId>,
    pub conflicts: Vec<ConflictRecord>,
    pub broken_links: Vec<BrokenLink>,
    pub recommendations: Vec<MaintenanceRecommendation>,
}

/// A set of topic pages detected as duplicates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub canonical: TopicId,
    pub duplicates: Vec<TopicId>,
    pub similarity: f64,
}

/// Supported export output formats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Json,
    Markdown,
    Html,
}

/// Criteria for filtering topics during export.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportFilter {
    pub topics: Option<Vec<TopicId>>,
    pub status: Option<Vec<TopicStatus>>,
    pub tags: Option<Vec<String>>,
    pub since: Option<DateTime<Utc>>,
}

/// Policy applied when importing data that already exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportPolicy {
    Merge,
    Replace,
    Skip,
}

/// Data classification level for access control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrivacyLevel {
    Public,
    Internal,
    Private,
    Sensitive,
}

/// Immutable audit-trail entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub operation: String,
    pub topic_id: Option<TopicId>,
    pub actor: String,
    pub details: String,
}

/// Aggregate counters for a completed bulk operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationSummary {
    pub operation: String,
    pub success_count: usize,
    pub failure_count: usize,
    pub skipped_count: usize,
    pub duration_ms: u64,
    pub token_cost: Option<TokenUsage>,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  PLATFORM
// ═══════════════════════════════════════════════════════════════════════════════

/// The kind of work we ask the LLM to perform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmTask {
    Compile,
    Enhance,
    Summarize,
    DetectConflict,
    GenerateTitle,
}

/// A request sent to the LLM abstraction layer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmRequest {
    pub task: LlmTask,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Token consumption for a single LLM call.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Response received from the LLM provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub usage: TokenUsage,
    pub model: String,
    pub duration_ms: u64,
}

/// Static metadata describing an LLM provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderMetadata {
    pub name: String,
    pub model: String,
    pub max_context_tokens: u32,
    pub supports_streaming: bool,
}

/// Combined platform-level configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlatformConfig {
    pub llm: LlmConfig,
    pub embedding: KcEmbeddingConfig,
    pub import: ImportConfig,
    pub intake: IntakeConfig,
}

/// LLM provider connection settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: String,
    pub max_retries: u32,
    pub timeout_secs: u64,
    pub temperature: f32,
}

/// Embedding provider settings specific to the KC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KcEmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub batch_size: usize,
}

/// Configuration for the memory intake pipeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntakeConfig {
    pub enabled: bool,
    pub auto_compile: bool,
    pub buffer_size: usize,
    pub deduplicate: bool,
}

/// How to split large documents during import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitStrategy {
    ByHeading,
    ByParagraph,
    ByTokenCount(usize),
    Smart,
}

/// How to handle duplicate content during import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicateStrategy {
    Skip,
    Replace,
    Append,
    Ask,
}

/// Document import pipeline configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportConfig {
    pub default_policy: ImportPolicy,
    pub split_strategy: SplitStrategy,
    pub duplicate_strategy: DuplicateStrategy,
    pub max_document_size_bytes: usize,
}

/// A single memory to be evaluated for import.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryCandidate {
    pub content: String,
    pub source: String,
    pub content_hash: String,
    pub metadata: HashMap<String, String>,
}

/// Summary of an import run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub total_processed: usize,
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

/// Outcome for a single item in an import batch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ItemStatus {
    Imported,
    Skipped,
    Failed(String),
}

/// Describes how fully a feature is supported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityLevel {
    None,
    Basic,
    Full,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  ERRORS
// ═══════════════════════════════════════════════════════════════════════════════

/// Top-level error type for the Knowledge Compiler.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum KcError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("compilation error: {0}")]
    Compilation(String),
    #[error("LLM error: {0}")]
    LlmError(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("conflict detection error: {0}")]
    ConflictDetection(String),
    #[error("import error: {0}")]
    ImportError(String),
    #[error("export error: {0}")]
    ExportError(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("privacy violation: {0}")]
    PrivacyViolation(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// LLM-specific errors with structured retry / limit info.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum LlmError {
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("context too long: {tokens} tokens (max {max})")]
    ContextTooLong { tokens: u32, max: u32 },
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("request timed out")]
    Timeout,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // ── TopicId ──────────────────────────────────────────────────────────

    #[test]
    fn topic_id_display() {
        let id = TopicId("rust-memory".to_owned());
        assert_eq!(id.to_string(), "rust-memory");
    }

    #[test]
    fn topic_id_from_string() {
        let id: TopicId = "hello".to_owned().into();
        assert_eq!(id.0, "hello");
    }

    #[test]
    fn topic_id_from_str() {
        let id: TopicId = "world".into();
        assert_eq!(id.0, "world");
    }

    #[test]
    fn topic_id_as_ref() {
        let id = TopicId("test".to_owned());
        let s: &str = id.as_ref();
        assert_eq!(s, "test");
    }

    #[test]
    fn topic_id_eq_and_hash() {
        use std::collections::HashSet;
        let a = TopicId("same".into());
        let b = TopicId("same".into());
        let c = TopicId("diff".into());
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1);
        set.insert(c);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn topic_id_serde_roundtrip() {
        let id = TopicId("serde-test".to_owned());
        let json = serde_json::to_string(&id).unwrap();
        let back: TopicId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    // ── ConflictId ───────────────────────────────────────────────────────

    #[test]
    fn conflict_id_display_and_conversions() {
        let id: ConflictId = "c-001".into();
        assert_eq!(id.to_string(), "c-001");
        assert_eq!(id.as_ref(), "c-001");

        let from_string: ConflictId = String::from("c-002").into();
        assert_eq!(from_string.0, "c-002");
    }

    #[test]
    fn conflict_id_eq_and_hash() {
        use std::collections::HashSet;
        let a: ConflictId = "x".into();
        let b: ConflictId = "x".into();
        assert_eq!(a, b);
        let mut set = HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 1);
    }

    // ── TopicStatus ──────────────────────────────────────────────────────

    #[test]
    fn topic_status_serde() {
        for status in &[
            TopicStatus::Active,
            TopicStatus::Stale,
            TopicStatus::Archived,
            TopicStatus::FailedPermanent,
        ] {
            let json = serde_json::to_string(status).unwrap();
            let back: TopicStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*status, back);
        }
    }

    // ── TopicPage full roundtrip ─────────────────────────────────────────

    #[test]
    fn topic_page_serde_roundtrip() {
        let now = Utc::now();
        let page = TopicPage {
            id: TopicId("tp-1".into()),
            title: "Rust Lifetimes".into(),
            content: "Content about lifetimes".into(),
            sections: vec![
                TopicSection {
                    heading: "Basics".into(),
                    body: "Lifetimes are…".into(),
                    user_edited: false,
                    edited_at: None,
                },
                TopicSection {
                    heading: "Advanced".into(),
                    body: "Higher-ranked…".into(),
                    user_edited: true,
                    edited_at: Some(now),
                },
            ],
            summary: "A summary of lifetimes".into(),
            metadata: TopicMetadata {
                created_at: now,
                updated_at: now,
                compilation_count: 3,
                source_memory_ids: vec!["m1".into(), "m2".into()],
                tags: vec!["rust".into(), "lifetimes".into()],
                quality_score: Some(0.92),
            },
            status: TopicStatus::Active,
            version: 5,
        };

        let json = serde_json::to_string_pretty(&page).unwrap();
        let back: TopicPage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, page.id);
        assert_eq!(back.title, page.title);
        assert_eq!(back.sections.len(), 2);
        assert!(back.sections[1].user_edited);
        assert_eq!(back.metadata.compilation_count, 3);
        assert_eq!(back.version, 5);
    }

    // ── TopicCandidate::overlaps_with ────────────────────────────────────

    fn make_candidate(memories: &[&str]) -> TopicCandidate {
        TopicCandidate {
            memories: memories.iter().map(|s| s.to_string()).collect(),
            centroid_embedding: vec![0.0; 3],
            cohesion_score: 0.8,
            suggested_title: None,
        }
    }

    #[test]
    fn overlaps_with_high_overlap() {
        // 3/4 shared = Jaccard 3/5 = 0.6 > 0.3
        let a = make_candidate(&["m1", "m2", "m3", "m4"]);
        let b = make_candidate(&["m2", "m3", "m4", "m5"]);
        assert!(a.overlaps_with(&b));
    }

    #[test]
    fn overlaps_with_low_overlap() {
        // 1/7 shared = Jaccard ~0.14 < 0.3
        let a = make_candidate(&["m1", "m2", "m3", "m4"]);
        let b = make_candidate(&["m4", "m5", "m6", "m7"]);
        assert!(!a.overlaps_with(&b));
    }

    #[test]
    fn overlaps_with_no_overlap() {
        let a = make_candidate(&["m1", "m2"]);
        let b = make_candidate(&["m3", "m4"]);
        assert!(!a.overlaps_with(&b));
    }

    #[test]
    fn overlaps_with_identical() {
        let a = make_candidate(&["m1", "m2"]);
        let b = make_candidate(&["m1", "m2"]);
        // Jaccard = 1.0 > 0.3
        assert!(a.overlaps_with(&b));
    }

    #[test]
    fn overlaps_with_empty() {
        let a = make_candidate(&[]);
        let b = make_candidate(&[]);
        // union = 0 → false
        assert!(!a.overlaps_with(&b));
    }

    #[test]
    fn overlaps_with_one_empty() {
        let a = make_candidate(&["m1"]);
        let b = make_candidate(&[]);
        // 0/1 = 0.0 < 0.3
        assert!(!a.overlaps_with(&b));
    }

    // ── ChangeSet serde ──────────────────────────────────────────────────

    #[test]
    fn change_set_serde() {
        let cs = ChangeSet {
            added: vec!["a".into()],
            modified: vec!["b".into()],
            removed: vec!["c".into()],
            last_compiled: Some(Utc::now()),
        };
        let json = serde_json::to_string(&cs).unwrap();
        let back: ChangeSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.added, cs.added);
        assert_eq!(back.modified, cs.modified);
        assert_eq!(back.removed, cs.removed);
    }

    // ── TriggerDecision variants ─────────────────────────────────────────

    #[test]
    fn trigger_decision_serde_all_variants() {
        let cs = ChangeSet {
            added: vec!["x".into()],
            modified: vec![],
            removed: vec![],
            last_compiled: None,
        };

        let variants: Vec<TriggerDecision> = vec![
            TriggerDecision::Skip { reason: "no changes".into() },
            TriggerDecision::Partial { change_set: cs.clone() },
            TriggerDecision::Full { change_set: cs },
        ];

        for v in &variants {
            let json = serde_json::to_string(v).unwrap();
            let back: TriggerDecision = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    // ── LifecycleOp variants ─────────────────────────────────────────────

    #[test]
    fn lifecycle_op_serde() {
        let ops: Vec<LifecycleOp> = vec![
            LifecycleOp::Merge {
                sources: vec![TopicId("a".into()), TopicId("b".into())],
                target_title: "merged".into(),
            },
            LifecycleOp::Split {
                source: TopicId("big".into()),
                new_topics: vec!["sub1".into(), "sub2".into()],
            },
            LifecycleOp::Archive {
                topic_id: TopicId("old".into()),
                reason: "outdated".into(),
            },
        ];

        for op in &ops {
            let json = serde_json::to_string(op).unwrap();
            let back: LifecycleOp = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    // ── FeedbackKind variants ────────────────────────────────────────────

    #[test]
    fn feedback_kind_serde() {
        let kinds: Vec<FeedbackKind> = vec![
            FeedbackKind::ThumbsUp,
            FeedbackKind::ThumbsDown,
            FeedbackKind::Correction("fix this".into()),
            FeedbackKind::TitleSuggestion("better title".into()),
            FeedbackKind::MergeRequest(TopicId("other".into())),
            FeedbackKind::SplitRequest(vec!["a".into(), "b".into()]),
        ];

        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let back: FeedbackKind = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    // ── RecompileStrategy ────────────────────────────────────────────────

    #[test]
    fn recompile_strategy_serde() {
        for s in &[RecompileStrategy::Eager, RecompileStrategy::Lazy, RecompileStrategy::Manual] {
            let json = serde_json::to_string(s).unwrap();
            let back: RecompileStrategy = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    // ── Conflict types ───────────────────────────────────────────────────

    #[test]
    fn conflict_full_serde() {
        let now = Utc::now();
        let record = ConflictRecord {
            conflict: Conflict {
                id: ConflictId("c-1".into()),
                conflict_type: ConflictType::Contradiction,
                scope: ConflictScope::BetweenTopics(
                    TopicId("a".into()),
                    TopicId("b".into()),
                ),
                description: "X says Y but Z says W".into(),
                status: ConflictStatus::Detected,
                detected_at: now,
                resolved_at: None,
            },
            severity: ConflictSeverity::High,
            evidence: vec!["source-1".into(), "source-2".into()],
        };

        let json = serde_json::to_string(&record).unwrap();
        let back: ConflictRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.conflict.id, record.conflict.id);
        assert_eq!(back.severity, record.severity);
        assert_eq!(back.evidence.len(), 2);
    }

    #[test]
    fn conflict_status_all_variants() {
        for s in &[
            ConflictStatus::Detected,
            ConflictStatus::Reviewing,
            ConflictStatus::Resolved,
            ConflictStatus::Dismissed,
        ] {
            let json = serde_json::to_string(s).unwrap();
            let back: ConflictStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, back);
        }
    }

    #[test]
    fn conflict_type_all_variants() {
        for t in &[
            ConflictType::Contradiction,
            ConflictType::Outdated,
            ConflictType::Redundant,
        ] {
            let json = serde_json::to_string(t).unwrap();
            let back: ConflictType = serde_json::from_str(&json).unwrap();
            assert_eq!(*t, back);
        }
    }

    #[test]
    fn conflict_severity_all_variants() {
        for s in &[
            ConflictSeverity::Low,
            ConflictSeverity::Medium,
            ConflictSeverity::High,
            ConflictSeverity::Critical,
        ] {
            let json = serde_json::to_string(s).unwrap();
            let back: ConflictSeverity = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, back);
        }
    }

    // ── CrossTopicLink & LinkType ────────────────────────────────────────

    #[test]
    fn cross_topic_link_serde() {
        let link = CrossTopicLink {
            source: TopicId("src".into()),
            target: TopicId("tgt".into()),
            link_type: LinkType::References,
            strength: 0.75,
            shared_memory_ids: vec!["m1".into()],
        };
        let json = serde_json::to_string(&link).unwrap();
        let back: CrossTopicLink = serde_json::from_str(&json).unwrap();
        assert_eq!(back.link_type, LinkType::References);
        assert!((back.strength - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn link_type_all_variants() {
        for lt in &[
            LinkType::References,
            LinkType::DerivedFrom,
            LinkType::Contradicts,
            LinkType::Supersedes,
        ] {
            let json = serde_json::to_string(lt).unwrap();
            let back: LinkType = serde_json::from_str(&json).unwrap();
            assert_eq!(*lt, back);
        }
    }

    #[test]
    fn link_status_all_variants() {
        for ls in &[LinkStatus::Valid, LinkStatus::Broken, LinkStatus::Stale] {
            let json = serde_json::to_string(ls).unwrap();
            let back: LinkStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*ls, back);
        }
    }

    // ── DryRunReport ─────────────────────────────────────────────────────

    #[test]
    fn dry_run_report_serde() {
        let report = DryRunReport {
            entries: vec![
                DryRunEntry {
                    topic_id: Some(TopicId("t1".into())),
                    action: DryRunAction::NewCompilation,
                    affected_memories: 5,
                    reason: "new topic".into(),
                },
                DryRunEntry {
                    topic_id: None,
                    action: DryRunAction::Skip,
                    affected_memories: 0,
                    reason: "no changes".into(),
                },
            ],
            total_topics_affected: 1,
            estimated_llm_calls: 1,
        };

        let json = serde_json::to_string(&report).unwrap();
        let back: DryRunReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.total_topics_affected, 1);
    }

    #[test]
    fn dry_run_action_all_variants() {
        let actions = vec![
            DryRunAction::NewCompilation,
            DryRunAction::Recompile,
            DryRunAction::Merge,
            DryRunAction::Split,
            DryRunAction::Archive,
            DryRunAction::Skip,
        ];
        for a in &actions {
            let json = serde_json::to_string(a).unwrap();
            let _: DryRunAction = serde_json::from_str(&json).unwrap();
        }
    }

    // ── Platform types ───────────────────────────────────────────────────

    #[test]
    fn llm_task_serde() {
        for task in &[
            LlmTask::Compile,
            LlmTask::Enhance,
            LlmTask::Summarize,
            LlmTask::DetectConflict,
            LlmTask::GenerateTitle,
        ] {
            let json = serde_json::to_string(task).unwrap();
            let back: LlmTask = serde_json::from_str(&json).unwrap();
            assert_eq!(*task, back);
        }
    }

    #[test]
    fn llm_request_response_roundtrip() {
        let req = LlmRequest {
            task: LlmTask::Compile,
            prompt: "Compile this topic".into(),
            max_tokens: Some(2000),
            temperature: Some(0.5),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: LlmRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_tokens, Some(2000));

        let resp = LlmResponse {
            content: "Compiled output".into(),
            usage: TokenUsage { input_tokens: 100, output_tokens: 200 },
            model: "gpt-4o".into(),
            duration_ms: 1500,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: LlmResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.usage.input_tokens, 100);
        assert_eq!(back.usage.output_tokens, 200);
    }

    // ── Import types ─────────────────────────────────────────────────────

    #[test]
    fn split_strategy_serde() {
        for s in &[
            SplitStrategy::ByHeading,
            SplitStrategy::ByParagraph,
            SplitStrategy::ByTokenCount(512),
            SplitStrategy::Smart,
        ] {
            let json = serde_json::to_string(s).unwrap();
            let back: SplitStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, back);
        }
    }

    #[test]
    fn duplicate_strategy_serde() {
        for s in &[
            DuplicateStrategy::Skip,
            DuplicateStrategy::Replace,
            DuplicateStrategy::Append,
            DuplicateStrategy::Ask,
        ] {
            let json = serde_json::to_string(s).unwrap();
            let back: DuplicateStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(*s, back);
        }
    }

    #[test]
    fn import_report_serde() {
        let report = ImportReport {
            total_processed: 100,
            imported: 90,
            skipped: 8,
            errors: vec!["bad format in line 42".into()],
            duration_ms: 3400,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: ImportReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_processed, 100);
        assert_eq!(back.errors.len(), 1);
    }

    #[test]
    fn item_status_serde() {
        for s in &[
            ItemStatus::Imported,
            ItemStatus::Skipped,
            ItemStatus::Failed("oops".into()),
        ] {
            let json = serde_json::to_string(s).unwrap();
            let _: ItemStatus = serde_json::from_str(&json).unwrap();
        }
    }

    // ── Privacy / Export types ────────────────────────────────────────────

    #[test]
    fn privacy_level_serde() {
        for p in &[
            PrivacyLevel::Public,
            PrivacyLevel::Internal,
            PrivacyLevel::Private,
            PrivacyLevel::Sensitive,
        ] {
            let json = serde_json::to_string(p).unwrap();
            let back: PrivacyLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*p, back);
        }
    }

    #[test]
    fn export_format_serde() {
        for f in &[ExportFormat::Json, ExportFormat::Markdown, ExportFormat::Html] {
            let json = serde_json::to_string(f).unwrap();
            let back: ExportFormat = serde_json::from_str(&json).unwrap();
            assert_eq!(*f, back);
        }
    }

    #[test]
    fn capability_level_serde() {
        for c in &[CapabilityLevel::None, CapabilityLevel::Basic, CapabilityLevel::Full] {
            let json = serde_json::to_string(c).unwrap();
            let back: CapabilityLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*c, back);
        }
    }

    // ── Error types ──────────────────────────────────────────────────────

    #[test]
    fn kc_error_display() {
        assert_eq!(
            KcError::Storage("db locked".into()).to_string(),
            "storage error: db locked"
        );
        assert_eq!(
            KcError::NotFound("topic-x".into()).to_string(),
            "not found: topic-x"
        );
        assert_eq!(
            KcError::PrivacyViolation("denied".into()).to_string(),
            "privacy violation: denied"
        );
    }

    #[test]
    fn kc_error_serde() {
        let errors: Vec<KcError> = vec![
            KcError::Storage("db".into()),
            KcError::Compilation("failed".into()),
            KcError::LlmError("timeout".into()),
            KcError::InvalidConfig("bad".into()),
            KcError::ConflictDetection("oops".into()),
            KcError::ImportError("format".into()),
            KcError::ExportError("write".into()),
            KcError::NotFound("x".into()),
            KcError::PrivacyViolation("no".into()),
            KcError::InvalidInput("empty".into()),
        ];

        for e in &errors {
            let json = serde_json::to_string(e).unwrap();
            let back: KcError = serde_json::from_str(&json).unwrap();
            // Display strings should match
            assert_eq!(e.to_string(), back.to_string());
        }
    }

    #[test]
    fn llm_error_display() {
        assert_eq!(
            LlmError::ProviderUnavailable("openai down".into()).to_string(),
            "provider unavailable: openai down"
        );
        assert_eq!(
            LlmError::RateLimited { retry_after_secs: 30 }.to_string(),
            "rate limited, retry after 30s"
        );
        assert_eq!(
            LlmError::ContextTooLong { tokens: 50000, max: 32000 }.to_string(),
            "context too long: 50000 tokens (max 32000)"
        );
        assert_eq!(LlmError::Timeout.to_string(), "request timed out");
    }

    #[test]
    fn llm_error_serde() {
        let errors: Vec<LlmError> = vec![
            LlmError::ProviderUnavailable("down".into()),
            LlmError::RateLimited { retry_after_secs: 60 },
            LlmError::ContextTooLong { tokens: 100, max: 50 },
            LlmError::InvalidResponse("bad json".into()),
            LlmError::Timeout,
        ];

        for e in &errors {
            let json = serde_json::to_string(e).unwrap();
            let back: LlmError = serde_json::from_str(&json).unwrap();
            assert_eq!(e.to_string(), back.to_string());
        }
    }

    // ── HealthReport ─────────────────────────────────────────────────────

    #[test]
    fn health_report_serde() {
        let report = HealthReport {
            generated_at: Utc::now(),
            total_topics: 42,
            stale_topics: vec![TopicId("old-1".into())],
            conflicts: vec![],
            broken_links: vec![],
            recommendations: vec![MaintenanceRecommendation {
                topic_id: TopicId("old-1".into()),
                action: "recompile".into(),
                priority: 1,
                reason: "stale content".into(),
            }],
        };

        let json = serde_json::to_string(&report).unwrap();
        let back: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_topics, 42);
        assert_eq!(back.stale_topics.len(), 1);
        assert_eq!(back.recommendations.len(), 1);
    }

    // ── QualityReport ────────────────────────────────────────────────────

    #[test]
    fn quality_report_serde() {
        let qr = QualityReport {
            topic_id: TopicId("q1".into()),
            coherence: 0.9,
            coverage: 0.8,
            freshness: 0.7,
            overall: 0.8,
            suggestions: vec!["add more sources".into()],
        };
        let json = serde_json::to_string(&qr).unwrap();
        let back: QualityReport = serde_json::from_str(&json).unwrap();
        assert!((back.overall - 0.8).abs() < f64::EPSILON);
    }

    // ── DuplicateGroup ───────────────────────────────────────────────────

    #[test]
    fn duplicate_group_serde() {
        let dg = DuplicateGroup {
            canonical: TopicId("main".into()),
            duplicates: vec![TopicId("dup1".into()), TopicId("dup2".into())],
            similarity: 0.95,
        };
        let json = serde_json::to_string(&dg).unwrap();
        let back: DuplicateGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(back.duplicates.len(), 2);
    }

    // ── OperationSummary ─────────────────────────────────────────────────

    #[test]
    fn operation_summary_serde() {
        let os = OperationSummary {
            operation: "bulk_import".into(),
            success_count: 90,
            failure_count: 5,
            skipped_count: 5,
            duration_ms: 12000,
            token_cost: Some(TokenUsage {
                input_tokens: 1500,
                output_tokens: 500,
            }),
        };
        let json = serde_json::to_string(&os).unwrap();
        let back: OperationSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.success_count + back.failure_count + back.skipped_count, 100);
        assert!(back.token_cost.is_some());
        let usage = back.token_cost.unwrap();
        assert_eq!(usage.input_tokens, 1500);
        assert_eq!(usage.output_tokens, 500);
    }

    // ── TopicSection ─────────────────────────────────────────────────────

    #[test]
    fn topic_section_serde() {
        let s = TopicSection {
            heading: "Overview".into(),
            body: "This is the overview.".into(),
            user_edited: true,
            edited_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: TopicSection = serde_json::from_str(&json).unwrap();
        assert!(back.user_edited);
        assert!(back.edited_at.is_some());
    }

    // ── BrokenLink + LinkRepairAction ────────────────────────────────────

    #[test]
    fn broken_link_serde() {
        let bl = BrokenLink {
            source_topic: TopicId("a".into()),
            target_topic: TopicId("b".into()),
            link_type: LinkType::DerivedFrom,
            status: LinkStatus::Broken,
            detected_at: Utc::now(),
        };
        let json = serde_json::to_string(&bl).unwrap();
        let back: BrokenLink = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, LinkStatus::Broken);
    }

    #[test]
    fn link_repair_action_serde() {
        let actions: Vec<LinkRepairAction> = vec![
            LinkRepairAction::Remove,
            LinkRepairAction::UpdateTarget(TopicId("new-target".into())),
            LinkRepairAction::MarkStale,
        ];
        for a in &actions {
            let json = serde_json::to_string(a).unwrap();
            let _: LinkRepairAction = serde_json::from_str(&json).unwrap();
        }
    }

    // ── DecayAction ──────────────────────────────────────────────────────

    #[test]
    fn decay_action_serde() {
        let actions: Vec<DecayAction> = vec![
            DecayAction::MarkStale(TopicId("s1".into())),
            DecayAction::Archive(TopicId("a1".into())),
            DecayAction::Refresh(TopicId("r1".into())),
        ];
        for a in &actions {
            let json = serde_json::to_string(a).unwrap();
            let _: DecayAction = serde_json::from_str(&json).unwrap();
        }
    }

    // ── ExportFilter ─────────────────────────────────────────────────────

    #[test]
    fn export_filter_serde() {
        let filter = ExportFilter {
            topics: Some(vec![TopicId("t1".into())]),
            status: Some(vec![TopicStatus::Active, TopicStatus::Stale]),
            tags: Some(vec!["rust".into()]),
            since: Some(Utc::now()),
        };
        let json = serde_json::to_string(&filter).unwrap();
        let back: ExportFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(back.topics.unwrap().len(), 1);
        assert_eq!(back.status.unwrap().len(), 2);

        // None fields
        let empty_filter = ExportFilter {
            topics: None,
            status: None,
            tags: None,
            since: None,
        };
        let json = serde_json::to_string(&empty_filter).unwrap();
        let back: ExportFilter = serde_json::from_str(&json).unwrap();
        assert!(back.topics.is_none());
    }

    // ── MemoryCandidate ──────────────────────────────────────────────────

    #[test]
    fn memory_candidate_serde() {
        let mut meta = HashMap::new();
        meta.insert("source".into(), "web".into());
        let mc = MemoryCandidate {
            content: "Rust is great".into(),
            source: "intake".into(),
            content_hash: "abc123".into(),
            metadata: meta,
        };
        let json = serde_json::to_string(&mc).unwrap();
        let back: MemoryCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metadata.get("source").unwrap(), "web");
    }

    // ── ImportPolicy ─────────────────────────────────────────────────────

    #[test]
    fn import_policy_serde() {
        for p in &[ImportPolicy::Merge, ImportPolicy::Replace, ImportPolicy::Skip] {
            let json = serde_json::to_string(p).unwrap();
            let back: ImportPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(*p, back);
        }
    }

    // ── ProviderMetadata ─────────────────────────────────────────────────

    #[test]
    fn provider_metadata_serde() {
        let pm = ProviderMetadata {
            name: "openai".into(),
            model: "gpt-4o".into(),
            max_context_tokens: 128000,
            supports_streaming: true,
        };
        let json = serde_json::to_string(&pm).unwrap();
        let back: ProviderMetadata = serde_json::from_str(&json).unwrap();
        assert!(back.supports_streaming);
        assert_eq!(back.max_context_tokens, 128000);
    }
}
