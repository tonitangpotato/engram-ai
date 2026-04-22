//! Type definitions for the synthesis engine.
//!
//! This module defines all data structures for memory synthesis:
//! cluster discovery, gate checking, insight generation, provenance tracking,
//! incremental updates, and top-level configuration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

use crate::storage::Storage;
use crate::types::MemoryRecord;

// ---------------------------------------------------------------------------
// Duration serde helper — stores Duration as f64 seconds
// ---------------------------------------------------------------------------

mod duration_secs {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(duration.as_secs_f64())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = f64::deserialize(deserializer)?;
        Ok(Duration::from_secs_f64(secs))
    }
}

// ===========================================================================
// §2 — Cluster Discovery
// ===========================================================================

/// Weights for combining clustering signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterWeights {
    /// Hebbian co-activation weight (default: 0.4)
    pub hebbian: f64,
    /// Entity overlap weight (default: 0.3)
    pub entity: f64,
    /// Embedding similarity weight (default: 0.2)
    pub embedding: f64,
    /// Temporal proximity weight (default: 0.1)
    pub temporal: f64,
}

impl Default for ClusterWeights {
    fn default() -> Self {
        Self {
            hebbian: 0.4,
            entity: 0.3,
            embedding: 0.2,
            temporal: 0.1,
        }
    }
}

/// Raw signal scores between two memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairwiseSignals {
    /// From hebbian_links table, `None` if no link exists.
    pub hebbian_weight: Option<f64>,
    /// Jaccard index of shared entities (0.0–1.0).
    pub entity_overlap: f64,
    /// Cosine similarity of embeddings (0.0–1.0).
    pub embedding_similarity: f64,
    /// Decay function of time gap (0.0–1.0).
    pub temporal_proximity: f64,
}

/// Summary of signal contributions within a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalsSummary {
    /// Which signal contributed most to clustering.
    pub dominant_signal: ClusterSignal,
    /// Hebbian signal contribution (weighted).
    pub hebbian_contribution: f64,
    /// Entity overlap contribution (weighted).
    pub entity_contribution: f64,
    /// Embedding similarity contribution (weighted).
    pub embedding_contribution: f64,
    /// Temporal proximity contribution (weighted).
    pub temporal_contribution: f64,
}

/// The dominant clustering signal type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClusterSignal {
    Hebbian,
    Entity,
    Embedding,
    Temporal,
}

/// A discovered cluster of related memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryCluster {
    /// Deterministic ID: hash of sorted member IDs.
    pub id: String,
    /// Memory IDs, sorted.
    pub members: Vec<String>,
    /// Average intra-cluster relatedness.
    pub quality_score: f64,
    /// Member with highest average relatedness to others.
    pub centroid_id: String,
    /// Summary of signal contributions.
    pub signals_summary: SignalsSummary,
}

/// Configuration for cluster discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterDiscoveryConfig {
    /// Signal combination weights.
    pub weights: ClusterWeights,
    /// Minimum combined score to consider a pair related (default: 0.3).
    pub cluster_threshold: f64,
    /// Minimum cluster size (default: 3).
    pub min_cluster_size: usize,
    /// Maximum cluster size (default: 15).
    pub max_cluster_size: usize,
    /// Minimum memory importance to consider (default: 0.3).
    pub min_importance: f64,
    /// Temporal decay lambda (default: 0.00413, 7-day half-life).
    pub temporal_decay_lambda: f64,
    /// Temporal half-life in hours (default: 168.0).
    pub temporal_half_life_hours: f64,
    /// Cooldown cycles before re-evaluating a cluster (default: 3).
    pub cooldown_cycles: u32,
    /// Minimum temporal spread among cluster members (default: 1 hour).
    #[serde(with = "duration_secs")]
    pub temporal_spread_minimum: Duration,
    /// Max neighbors per node for k-NN edge sparsification.
    /// `None` = adaptive: `clamp(sqrt(n), 5, 30)` where n = node count.
    #[serde(default)]
    pub max_neighbors_per_node: Option<usize>,
    /// Number of Infomap optimization trials.
    /// `None` = adaptive: 1 if edge density < 5, else 3.
    #[serde(default)]
    pub infomap_trials: Option<usize>,
    /// Whether Infomap uses hierarchical (multi-level) clustering.
    /// `None` = adaptive: true if node count > 2000, else false.
    #[serde(default)]
    pub infomap_hierarchical: Option<bool>,
    /// Hot assign threshold: cosine similarity to nearest centroid.
    /// Above this → assign to cluster. Below → pending.
    /// `None` = default 0.6.
    #[serde(default)]
    pub hot_assign_threshold: Option<f64>,
    /// Cold recluster trigger: full recluster when pending exceeds this
    /// fraction of total memories. `None` = default 0.2 (20%).
    #[serde(default)]
    pub cold_recluster_ratio: Option<f64>,
    /// Warm recluster trigger: recluster when pending count exceeds this.
    /// `None` = default 100.
    #[serde(default)]
    pub warm_recluster_interval: Option<usize>,
}

impl Default for ClusterDiscoveryConfig {
    fn default() -> Self {
        Self {
            weights: ClusterWeights::default(),
            cluster_threshold: 0.3,
            min_cluster_size: 3,
            max_cluster_size: 15,
            min_importance: 0.3,
            temporal_decay_lambda: 0.00413,
            temporal_half_life_hours: 168.0,
            cooldown_cycles: 3,
            temporal_spread_minimum: Duration::from_secs(3600),
            max_neighbors_per_node: None,
            infomap_trials: None,
            infomap_hierarchical: None,
            hot_assign_threshold: None,
            cold_recluster_ratio: None,
            warm_recluster_interval: None,
        }
    }
}

/// Configuration for emotional modulation of synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionalModulationConfig {
    /// Weight applied to emotional boost (default: 0.2).
    pub emotional_boost_weight: f64,
    /// Whether to prioritize emotionally significant clusters (default: true).
    pub prioritize_emotional: bool,
    /// Whether to include emotion context in the LLM prompt (default: true).
    pub include_emotion_in_prompt: bool,
}

impl Default for EmotionalModulationConfig {
    fn default() -> Self {
        Self {
            emotional_boost_weight: 0.2,
            prioritize_emotional: true,
            include_emotion_in_prompt: true,
        }
    }
}

// ===========================================================================
// §3 — Gate Check
// ===========================================================================

/// Decision from the gate check for a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GateDecision {
    /// Cluster qualifies for full LLM synthesis.
    Synthesize { reason: String },
    /// Cluster can be handled by a cheaper auto-update action.
    AutoUpdate { action: AutoUpdateAction },
    /// Cluster is promising but not yet ready; revisit later.
    Defer { reason: String },
    /// Cluster does not meet quality bar.
    Skip { reason: String },
}

/// Cheap actions the gate can take without an LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutoUpdateAction {
    /// Merge near-duplicate memories: keep one, demote the rest.
    MergeDuplicates { keep: String, demote: Vec<String> },
    /// Strengthen Hebbian links between pairs.
    StrengthenLinks { pairs: Vec<(String, String)> },
}

/// Configuration for the gate check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateConfig {
    /// Minimum cluster size to consider for synthesis (default: 3).
    pub min_cluster_size: usize,
    /// Minimum quality score to pass the gate (default: 0.4).
    pub gate_quality_threshold: f64,
    /// Quality score above which synthesis is deferred instead of skipped (default: 0.6).
    pub defer_quality_threshold: f64,
    /// Cosine similarity threshold for duplicate detection (default: 0.95).
    pub duplicate_similarity: f64,
    /// Minimum number of distinct memory types in a cluster (default: 2).
    pub min_type_diversity: usize,
    /// Cost threshold for gating LLM calls (default: 0.05).
    pub cost_threshold: f64,
    /// Quality score above which synthesis is always approved (default: 0.8).
    pub premium_threshold: f64,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: 3,
            gate_quality_threshold: 0.4,
            defer_quality_threshold: 0.6,
            duplicate_similarity: 0.95,
            min_type_diversity: 2,
            cost_threshold: 0.05,
            premium_threshold: 0.8,
        }
    }
}

/// Result of a gate check on a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    /// Cluster that was evaluated.
    pub cluster_id: String,
    /// Gate decision.
    pub decision: GateDecision,
    /// Scores used to make the decision.
    pub scores: GateScores,
    /// When the gate check occurred.
    pub timestamp: DateTime<Utc>,
}

/// Scores computed during gate evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateScores {
    /// Cluster quality score.
    pub quality: f64,
    /// Number of distinct memory types in the cluster.
    pub type_diversity: usize,
    /// Estimated LLM cost for synthesis.
    pub estimated_cost: f64,
    /// Number of members in the cluster.
    pub member_count: usize,
}

// ===========================================================================
// §4 — Insight Generation
// ===========================================================================

/// Request to synthesize an insight from a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisRequest {
    /// The cluster to synthesize.
    pub cluster: MemoryCluster,
    /// Full memory records for each cluster member.
    pub members: Vec<MemoryRecord>,
    /// Synthesis configuration.
    pub config: SynthesisConfig,
}

/// Configuration for LLM-based insight generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisConfig {
    /// LLM model identifier (e.g. "claude-3-haiku-20240307").
    pub model: String,
    /// Maximum tokens for LLM response (default: 512).
    pub max_tokens: usize,
    /// LLM temperature (default: 0.3).
    pub temperature: f64,
    /// Which prompt template to use.
    pub prompt_template: PromptTemplate,
    /// Maximum memories to include per LLM call (default: 10).
    pub max_memories_per_llm_call: usize,
    /// Age threshold for re-synthesizing existing insights (default: 30 days).
    #[serde(with = "duration_secs")]
    pub resynthesis_age_threshold: Duration,
}

impl Default for SynthesisConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            max_tokens: 512,
            temperature: 0.3,
            prompt_template: PromptTemplate::General,
            max_memories_per_llm_call: 10,
            resynthesis_age_threshold: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// Prompt template selection for synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptTemplate {
    /// General-purpose synthesis.
    General,
    /// Factual pattern extraction.
    FactualPattern,
    /// Episodic thread linking.
    EpisodicThread,
    /// Causal chain inference.
    CausalChain,
}

/// Output from a synthesis LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisOutput {
    /// The generated insight text.
    pub insight_text: String,
    /// Confidence in the insight (0.0–1.0).
    pub confidence: f64,
    /// Classification of the insight.
    pub insight_type: InsightType,
    /// Memory IDs referenced in the insight.
    pub source_references: Vec<String>,
}

/// Classification of a synthesized insight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsightType {
    /// A recurring pattern across memories.
    Pattern,
    /// A derived rule or heuristic.
    Rule,
    /// A connection between seemingly unrelated memories.
    Connection,
    /// A contradiction or tension between memories.
    Contradiction,
}

// ===========================================================================
// §5 — Provenance
// ===========================================================================

/// Record linking a synthesized insight to its source memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    /// Unique provenance record ID.
    pub id: String,
    /// ID of the synthesized insight memory.
    pub insight_id: String,
    /// ID of the source memory.
    pub source_id: String,
    /// ID of the cluster that produced this insight.
    pub cluster_id: String,
    /// When synthesis occurred.
    pub synthesis_timestamp: DateTime<Utc>,
    /// Gate decision description.
    pub gate_decision: String,
    /// Gate scores at time of synthesis.
    pub gate_scores: Option<GateScores>,
    /// Confidence of the insight.
    pub confidence: f64,
    /// Original importance of the source memory (before demotion).
    pub source_original_importance: Option<f64>,
}

/// A chain of provenance records tracing insights to their roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceChain {
    /// Root memory/insight ID.
    pub root_id: String,
    /// Layers of provenance, from newest to oldest.
    pub layers: Vec<Vec<ProvenanceRecord>>,
}

/// Result of undoing a synthesis operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoSynthesis {
    /// ID of the insight that was undone.
    pub insight_id: String,
    /// Sources that were restored.
    pub restored_sources: Vec<RestoredSource>,
}

/// A source memory restored after undoing synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoredSource {
    /// Memory ID of the restored source.
    pub memory_id: String,
    /// The original importance before demotion.
    pub original_importance: f64,
    /// Whether the restore succeeded.
    pub restored: bool,
}

// ===========================================================================
// §6 — Incremental Updates
// ===========================================================================

/// Per-cluster incremental state for staleness detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalState {
    /// Member IDs from last synthesis run.
    pub last_member_snapshot: HashSet<String>,
    /// Quality score from last synthesis run.
    pub last_quality_score: f64,
    /// When last synthesis ran.
    pub last_run: DateTime<Utc>,
    /// How many times this cluster has been synthesized.
    pub run_count: usize,
    /// When this cluster was last attempted (gate-checked), regardless of outcome.
    /// Defaults to `last_run` for backward compatibility with pre-existing state.
    #[serde(default = "default_attempt_timestamp")]
    pub last_attempt_timestamp: DateTime<Utc>,
    /// How many times this cluster has been attempted (gate-checked), regardless of outcome.
    /// Includes successful synthesis, deferred, skipped, and auto-updated attempts.
    #[serde(default)]
    pub attempt_count: usize,
    /// Member snapshot at the time of the last attempt (may differ from last_member_snapshot
    /// which is only updated on successful synthesis).
    #[serde(default)]
    pub last_attempt_members: HashSet<String>,
}

/// Default for backward compatibility: returns Unix epoch so old states without
/// `last_attempt_timestamp` are treated as never-attempted.
fn default_attempt_timestamp() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).unwrap()
}

/// Configuration for incremental/staleness detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalConfig {
    /// Fraction of members that must change to trigger re-synthesis (default: 0.5).
    pub staleness_member_change_pct: f64,
    /// Quality score delta that triggers re-synthesis (default: 0.2).
    pub staleness_quality_delta: f64,
}

impl Default for IncrementalConfig {
    fn default() -> Self {
        Self {
            staleness_member_change_pct: 0.5,
            staleness_quality_delta: 0.2,
        }
    }
}

// ===========================================================================
// §7 — Engine Report + Errors
// ===========================================================================

/// Report produced after a synthesis run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisReport {
    /// Number of clusters discovered.
    pub clusters_found: usize,
    /// Number of clusters that went through LLM synthesis.
    pub clusters_synthesized: usize,
    /// Number of clusters handled by auto-update actions.
    pub clusters_auto_updated: usize,
    /// Number of clusters deferred for later.
    pub clusters_deferred: usize,
    /// Number of clusters skipped.
    pub clusters_skipped: usize,
    /// Number of full synthesis runs (no prior state).
    pub synthesis_runs_full: usize,
    /// Number of incremental synthesis runs (seeded from existing insight).
    pub synthesis_runs_incremental: usize,
    /// IDs of newly created insight memories.
    pub insights_created: Vec<String>,
    /// IDs of source memories whose importance was demoted.
    pub sources_demoted: Vec<String>,
    /// Errors encountered during the run.
    pub errors: Vec<SynthesisError>,
    /// Total wall-clock duration of the synthesis run.
    #[serde(with = "duration_secs")]
    pub duration: Duration,
    /// Gate results for each evaluated cluster.
    pub gate_results: Vec<GateResult>,
}

/// Errors that can occur during synthesis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SynthesisError {
    /// LLM call timed out.
    LlmTimeout { cluster_id: String },
    /// LLM returned unparseable response.
    LlmInvalidResponse {
        cluster_id: String,
        raw_response: String,
    },
    /// LLM referenced memory IDs not in the cluster.
    HallucinatedReferences {
        cluster_id: String,
        invalid_ids: Vec<String>,
    },
    /// Post-synthesis validation failed.
    ValidationFailed { cluster_id: String, reason: String },
    /// Storage operation failed.
    StorageError { cluster_id: String, message: String },
    /// Embedding computation failed.
    EmbeddingError { memory_id: String, message: String },
    /// LLM call budget exhausted for this run.
    BudgetExhausted { remaining_clusters: usize },
    /// Cluster became stale during processing.
    ClusterStale { cluster_id: String },
}

impl std::fmt::Display for SynthesisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LlmTimeout { cluster_id } => {
                write!(f, "LLM timeout for cluster {}", cluster_id)
            }
            Self::LlmInvalidResponse { cluster_id, .. } => {
                write!(f, "LLM invalid response for cluster {}", cluster_id)
            }
            Self::HallucinatedReferences {
                cluster_id,
                invalid_ids,
            } => {
                write!(
                    f,
                    "Hallucinated references in cluster {}: {:?}",
                    cluster_id, invalid_ids
                )
            }
            Self::ValidationFailed { cluster_id, reason } => {
                write!(
                    f,
                    "Validation failed for cluster {}: {}",
                    cluster_id, reason
                )
            }
            Self::StorageError {
                cluster_id,
                message,
            } => {
                write!(f, "Storage error for cluster {}: {}", cluster_id, message)
            }
            Self::EmbeddingError {
                memory_id,
                message,
            } => {
                write!(f, "Embedding error for memory {}: {}", memory_id, message)
            }
            Self::BudgetExhausted {
                remaining_clusters,
            } => {
                write!(
                    f,
                    "LLM budget exhausted with {} clusters remaining",
                    remaining_clusters
                )
            }
            Self::ClusterStale { cluster_id } => {
                write!(f, "Cluster {} became stale", cluster_id)
            }
        }
    }
}

// ===========================================================================
// §8 — Top-level Config
// ===========================================================================

/// Top-level synthesis settings controlling all subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisSettings {
    /// Master switch for synthesis (default: false).
    pub enabled: bool,
    /// Cluster discovery configuration.
    pub cluster_discovery: ClusterDiscoveryConfig,
    /// Gate check configuration.
    pub gate: GateConfig,
    /// LLM synthesis configuration.
    pub synthesis: SynthesisConfig,
    /// Emotional modulation configuration.
    pub emotional: EmotionalModulationConfig,
    /// Incremental/staleness configuration.
    pub incremental: IncrementalConfig,
    /// Factor to multiply source importance by after synthesis (default: 0.5).
    pub demotion_factor: f64,
    /// Maximum insights to create per consolidation cycle (default: 5).
    pub max_insights_per_consolidation: usize,
    /// Maximum LLM calls per synthesis run (default: 5).
    pub max_llm_calls_per_run: u32,
}

impl Default for SynthesisSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            cluster_discovery: ClusterDiscoveryConfig::default(),
            gate: GateConfig::default(),
            synthesis: SynthesisConfig::default(),
            emotional: EmotionalModulationConfig::default(),
            incremental: IncrementalConfig::default(),
            demotion_factor: 0.5,
            max_insights_per_consolidation: 5,
            max_llm_calls_per_run: 5,
        }
    }
}

// ===========================================================================
// Traits
// ===========================================================================

/// LLM provider for synthesis insight generation.
///
/// This is intentionally separate from [`crate::extractor::MemoryExtractor`],
/// which performs structured fact extraction. `SynthesisLlmProvider` generates
/// free-form insight text from a cluster of memories.
pub trait SynthesisLlmProvider: Send + Sync {
    /// Generate insight text from a prompt using the given synthesis config.
    fn generate(
        &self,
        prompt: &str,
        config: &SynthesisConfig,
    ) -> Result<String, Box<dyn std::error::Error>>;
}

/// The main synthesis engine trait.
///
/// Implementations orchestrate the full synthesis pipeline: cluster discovery,
/// gate checking, LLM insight generation, provenance tracking, and storage.
pub trait SynthesisEngine: Send + Sync {
    /// Run a full synthesis cycle: discover clusters, gate-check, synthesize, store.
    fn synthesize(
        &self,
        storage: &mut Storage,
        settings: &SynthesisSettings,
    ) -> Result<SynthesisReport, Box<dyn std::error::Error>>;

    /// Discover clusters of related memories.
    fn discover_clusters(
        &self,
        storage: &Storage,
        config: &ClusterDiscoveryConfig,
    ) -> Result<Vec<MemoryCluster>, Box<dyn std::error::Error>>;

    /// Evaluate whether a cluster should be synthesized, auto-updated, deferred, or skipped.
    fn check_gate(
        &self,
        cluster: &MemoryCluster,
        members: &[MemoryRecord],
        config: &GateConfig,
    ) -> GateResult;

    /// Undo a previous synthesis: remove the insight and restore source importances.
    fn undo_synthesis(
        &self,
        storage: &mut Storage,
        insight_id: &str,
    ) -> Result<UndoSynthesis, Box<dyn std::error::Error>>;

    /// Trace provenance of a memory/insight back to its roots.
    fn get_provenance(
        &self,
        storage: &Storage,
        memory_id: &str,
        max_depth: usize,
    ) -> Result<ProvenanceChain, Box<dyn std::error::Error>>;
}
