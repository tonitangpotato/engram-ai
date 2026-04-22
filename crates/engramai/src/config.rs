//! Memory configuration presets and tunable parameters.

use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingConfig;
use crate::entities::EntityConfig;

/// Configuration for LLM-based triple extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleConfig {
    /// Enable triple extraction during consolidation
    pub enabled: bool,
    /// Number of memories to process per consolidation cycle
    pub batch_size: usize,
    /// Maximum extraction attempts before skipping a memory
    pub max_retries: u32,
    /// Override model for triple extraction (None = use extractor default)
    pub model: Option<String>,
}

/// Configuration for knowledge promotion detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionConfig {
    /// Enable promotion detection (default: false, opt-in)
    pub enabled: bool,
    /// Minimum core_strength for a memory to be considered (default: 0.6)
    pub min_core_strength: f64,
    /// Minimum Hebbian link weight to count as connected (default: 0.3)
    pub min_hebbian_weight: f64,
    /// Minimum cluster size (default: 3)
    pub min_cluster_size: usize,
    /// Minimum time span in days across cluster members (default: 2.0)
    pub min_time_span_days: f64,
    /// Minimum average importance across cluster members (default: 0.4)
    pub min_avg_importance: f64,
}

/// Configuration for intent classification (L2 Haiku fallback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentClassificationConfig {
    /// Enable L2 Haiku fallback (default: false — requires explicit opt-in)
    pub haiku_l2_enabled: bool,
    /// Model for intent classification
    pub model: String,
    /// Timeout for L2 call in seconds
    pub timeout_secs: u64,
    /// API URL override (None = default "https://api.anthropic.com")
    pub api_url: Option<String>,
}

impl Default for IntentClassificationConfig {
    fn default() -> Self {
        Self {
            haiku_l2_enabled: false,
            model: "claude-haiku-4-5-20251001".to_string(),
            timeout_secs: 5,
            api_url: None,
        }
    }
}

impl Default for PromotionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_core_strength: 0.6,
            min_hebbian_weight: 0.3,
            min_cluster_size: 3,
            min_time_span_days: 2.0,
            min_avg_importance: 0.4,
        }
    }
}

impl Default for TripleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 10,
            max_retries: 3,
            model: None,
        }
    }
}

/// Configuration for write-time association discovery (multi-signal Hebbian).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssociationConfig {
    /// Enable/disable write-time association discovery
    pub enabled: bool,
    /// Weight for entity overlap signal
    pub w_entity: f64,
    /// Weight for embedding similarity signal
    pub w_embedding: f64,
    /// Weight for temporal proximity signal
    pub w_temporal: f64,
    /// Combined score threshold for link creation
    pub link_threshold: f64,
    /// Maximum new links per memory write
    pub max_links_per_memory: usize,
    /// Maximum candidates to evaluate
    pub candidate_limit: usize,
    /// Temporal window in days for candidate selection
    pub temporal_window_days: u64,
    /// Initial strength for write-time discovered links
    pub initial_strength: f64,
    /// Decay rate for co-recall links
    pub decay_corecall: f64,
    /// Decay rate for multi-signal links
    pub decay_multi: f64,
    /// Decay rate for single-signal links
    pub decay_single: f64,
}

impl Default for AssociationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            w_entity: 0.3,
            w_embedding: 0.5,
            w_temporal: 0.2,
            link_threshold: 0.4,
            max_links_per_memory: 5,
            candidate_limit: 50,
            temporal_window_days: 7,
            initial_strength: 0.5,
            decay_corecall: 0.95,
            decay_multi: 0.90,
            decay_single: 0.85,
        }
    }
}

/// All tunable parameters for the Engram memory system.
///
/// Default values come from neuroscience literature (ACT-R, Memory Chain Model,
/// Ebbinghaus forgetting curve).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    // === Consolidation (Memory Chain Model) ===
    /// Working memory decay rate (per day). Higher = faster decay.
    pub mu1: f64,
    /// Core memory decay rate (per day). Higher = faster decay.
    pub mu2: f64,
    /// Consolidation transfer rate (working → core per day)
    pub alpha: f64,
    /// Fraction of archived memories replayed per cycle
    pub interleave_ratio: f64,
    /// Core strength boost per replayed archived memory (base)
    pub replay_boost: f64,
    
    // Layer rebalancing thresholds
    pub promote_threshold: f64,
    pub demote_threshold: f64,
    pub archive_threshold: f64,
    
    // === Activation (ACT-R) ===
    /// Base-level activation decay parameter (d in t^-d)
    pub actr_decay: f64,
    /// Context spreading activation weight
    pub context_weight: f64,
    /// Importance weight in retrieval activation
    pub importance_weight: f64,
    /// Contradiction penalty in activation
    pub contradiction_penalty: f64,
    
    // === Forgetting ===
    /// Spacing effect multiplier
    pub spacing_factor: f64,
    /// Importance floor in stability
    pub importance_floor: f64,
    /// Consolidation bonus per consolidation count
    pub consolidation_bonus: f64,
    /// Effective strength threshold for pruning
    pub forget_threshold: f64,
    
    // === Reward ===
    /// Default reward magnitude
    pub reward_magnitude: f64,
    
    // === Downscaling ===
    /// Global downscaling factor per consolidation cycle
    pub downscale_factor: f64,
    
    // === Hebbian learning ===
    /// Enable Hebbian link formation
    pub hebbian_enabled: bool,
    /// Number of co-activations before link forms
    pub hebbian_threshold: i32,
    /// Link strength decay per consolidation cycle
    pub hebbian_decay: f64,
    
    // === STDP (causal inference) ===
    /// Enable temporal direction tracking
    pub stdp_enabled: bool,
    /// Forward/backward ratio threshold for causal inference
    pub stdp_causal_threshold: f64,
    /// Minimum observations before STDP inference
    pub stdp_min_observations: i32,
    
    // === Embedding ===
    /// Embedding provider configuration
    pub embedding: EmbeddingConfig,
    /// Weight for FTS exact matching in hybrid recall (0.0-1.0)
    /// Recommended: 0.15 for 15% FTS contribution
    pub fts_weight: f64,
    /// Weight for embedding similarity in recall scoring (0.0-1.0)
    /// Recommended: 0.60 for 60% semantic similarity contribution
    pub embedding_weight: f64,
    /// Weight for ACT-R activation in recall scoring (0.0-1.0)
    /// Recommended: 0.25 for 25% recency/frequency contribution
    /// Note: fts_weight + embedding_weight + actr_weight should sum to ~1.0
    pub actr_weight: f64,
    
    /// Sigmoid center for ACT-R activation normalization.
    /// Controls the "midpoint age" — memories with activation near this value
    /// get normalized to ~0.5. Default -5.5 ≈ 1-day-old single-access memory.
    /// Lower values shift the curve to favor older memories.
    #[serde(default = "default_actr_sigmoid_center")]
    pub actr_sigmoid_center: f64,
    
    /// Sigmoid scale for ACT-R activation normalization.
    /// Controls steepness: smaller = sharper transition, larger = gentler.
    /// Default 1.5 gives good discrimination across the 1min–30day range.
    #[serde(default = "default_actr_sigmoid_scale")]
    pub actr_sigmoid_scale: f64,
    
    // === Entity extraction ===
    /// Entity extraction configuration
    #[serde(default)]
    pub entity_config: EntityConfig,
    /// Weight for entity matches in hybrid recall scoring (0.0-1.0)
    #[serde(default = "default_entity_weight")]
    pub entity_weight: f64,
    
    // === Dedup on write ===
    /// Enable dedup checking on write (default: true)
    #[serde(default = "default_dedup_enabled")]
    pub dedup_enabled: bool,
    /// Cosine similarity threshold for considering memories as duplicates (default: 0.95)
    #[serde(default = "default_dedup_threshold")]
    pub dedup_threshold: f64,
    
    // === Auto-extraction importance cap ===
    /// Maximum importance for auto-extracted memories (default: 0.7).
    /// Prevents LLM extractor from assigning high importance to noise.
    /// Only affects memories stored via extraction pipeline, not manual add().
    #[serde(default = "default_auto_extract_importance_cap")]
    pub auto_extract_importance_cap: f64,

    // === Dedup on recall ===
    /// Enable dedup of recall results (default: true)
    #[serde(default = "default_recall_dedup_enabled")]
    pub recall_dedup_enabled: bool,
    /// Cosine similarity threshold for recall result dedup (default: 0.85)
    #[serde(default = "default_recall_dedup_threshold")]
    pub recall_dedup_threshold: f64,
    
    // === Multi-retrieval fusion ===
    /// Weight for temporal channel in hybrid recall (0.0-1.0)
    /// Only meaningful when query has temporal indicators
    #[serde(default = "default_temporal_weight")]
    pub temporal_weight: f64,

    /// Weight for Hebbian graph channel in hybrid recall (0.0-1.0)
    #[serde(default = "default_hebbian_recall_weight")]
    pub hebbian_recall_weight: f64,

    /// Weight for somatic marker channel in hybrid recall (0.0-1.0).
    /// Somatic markers (Damasio) bias recall toward emotionally significant memories.
    /// Memories associated with strong positive or negative emotional contexts
    /// get boosted — the system "remembers" emotionally charged situations.
    #[serde(default = "default_somatic_weight")]
    pub somatic_weight: f64,

    /// Enable query-type adaptive weight adjustment (default: true)
    #[serde(default = "default_adaptive_weights")]
    pub adaptive_weights: bool,

    /// Write-time association discovery configuration
    #[serde(default)]
    pub association: AssociationConfig,

    /// LLM triple extraction configuration
    #[serde(default)]
    pub triple: TripleConfig,

    /// Knowledge promotion configuration
    #[serde(default)]
    pub promotion: PromotionConfig,

    /// Intent classification configuration (L2 Haiku fallback)
    #[serde(default)]
    pub intent_classification: IntentClassificationConfig,

    /// Enable meta-cognition self-monitoring (default: false).
    /// When enabled, recall and synthesis events are tracked for metrics
    /// and parameter adjustment suggestions.
    #[serde(default)]
    pub metacognition_enabled: bool,
}

fn default_entity_weight() -> f64 {
    0.15
}

fn default_actr_sigmoid_center() -> f64 {
    -5.5
}

fn default_actr_sigmoid_scale() -> f64 {
    1.5
}

fn default_dedup_enabled() -> bool {
    true
}

fn default_dedup_threshold() -> f64 {
    0.95
}

fn default_auto_extract_importance_cap() -> f64 {
    0.7
}

fn default_recall_dedup_enabled() -> bool {
    true
}

fn default_recall_dedup_threshold() -> f64 {
    0.85
}

fn default_temporal_weight() -> f64 {
    0.10
}

fn default_hebbian_recall_weight() -> f64 {
    0.10
}

fn default_somatic_weight() -> f64 {
    0.08
}

fn default_adaptive_weights() -> bool {
    true
}

impl Default for MemoryConfig {
    /// Literature-based defaults.
    fn default() -> Self {
        Self {
            mu1: 0.15,
            mu2: 0.005,
            alpha: 0.08,
            interleave_ratio: 0.3,
            replay_boost: 0.01,
            promote_threshold: 0.25,
            demote_threshold: 0.05,
            archive_threshold: 0.15,
            actr_decay: 0.5,
            context_weight: 1.5,
            importance_weight: 2.0,
            contradiction_penalty: 3.0,
            spacing_factor: 0.5,
            importance_floor: 0.5,
            consolidation_bonus: 0.2,
            forget_threshold: 0.01,
            reward_magnitude: 0.15,
            downscale_factor: 0.95,
            hebbian_enabled: true,
            hebbian_threshold: 3,
            hebbian_decay: 0.95,
            stdp_enabled: true,
            stdp_causal_threshold: 2.0,
            stdp_min_observations: 3,
            embedding: EmbeddingConfig::default(),
            fts_weight: 0.15,        // 15% exact matching
            embedding_weight: 0.60,   // 60% semantic similarity
            actr_weight: 0.25,        // 25% recency/frequency/importance
            actr_sigmoid_center: default_actr_sigmoid_center(),
            actr_sigmoid_scale: default_actr_sigmoid_scale(),
            entity_config: EntityConfig::default(),
            entity_weight: default_entity_weight(),
            dedup_enabled: default_dedup_enabled(),
            dedup_threshold: default_dedup_threshold(),
            recall_dedup_enabled: default_recall_dedup_enabled(),
            recall_dedup_threshold: default_recall_dedup_threshold(),
            auto_extract_importance_cap: default_auto_extract_importance_cap(),
            temporal_weight: default_temporal_weight(),
            hebbian_recall_weight: default_hebbian_recall_weight(),
            somatic_weight: default_somatic_weight(),
            adaptive_weights: default_adaptive_weights(),
            association: AssociationConfig::default(),
            triple: TripleConfig::default(),
            promotion: PromotionConfig::default(),
            metacognition_enabled: false,
            intent_classification: IntentClassificationConfig::default(),
        }
    }
}

impl MemoryConfig {
    /// Preset for conversational chatbots.
    ///
    /// High replay, slow decay — optimized for long conversations.
    pub fn chatbot() -> Self {
        Self {
            mu1: 0.08,
            mu2: 0.003,
            alpha: 0.12,
            interleave_ratio: 0.4,
            replay_boost: 0.015,
            actr_decay: 0.4,
            context_weight: 2.0,
            downscale_factor: 0.96,
            reward_magnitude: 0.2,
            forget_threshold: 0.005,
            ..Default::default()
        }
    }

    /// Preset for short-lived task agents.
    ///
    /// Fast decay, low replay — focus on recent task context.
    pub fn task_agent() -> Self {
        Self {
            mu1: 0.25,
            mu2: 0.01,
            alpha: 0.05,
            interleave_ratio: 0.1,
            replay_boost: 0.005,
            actr_decay: 0.6,
            promote_threshold: 0.35,
            archive_threshold: 0.2,
            downscale_factor: 0.90,
            forget_threshold: 0.02,
            ..Default::default()
        }
    }

    /// Preset for long-term personal assistants.
    ///
    /// Very slow core decay — remember preferences for months.
    pub fn personal_assistant() -> Self {
        Self {
            mu1: 0.12,
            mu2: 0.001,
            alpha: 0.10,
            interleave_ratio: 0.3,
            replay_boost: 0.02,
            actr_decay: 0.45,
            importance_weight: 0.7,
            promote_threshold: 0.20,
            demote_threshold: 0.03,
            downscale_factor: 0.97,
            forget_threshold: 0.005,
            ..Default::default()
        }
    }

    /// Preset for research agents.
    ///
    /// Minimal forgetting — everything might be relevant later.
    pub fn researcher() -> Self {
        Self {
            mu1: 0.05,
            mu2: 0.001,
            alpha: 0.15,
            interleave_ratio: 0.5,
            replay_boost: 0.025,
            actr_decay: 0.35,
            context_weight: 2.0,
            importance_weight: 0.3,
            promote_threshold: 0.15,
            demote_threshold: 0.02,
            archive_threshold: 0.10,
            downscale_factor: 0.98,
            forget_threshold: 0.001,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_triple_config_defaults() {
        let config = TripleConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.batch_size, 10);
        assert_eq!(config.max_retries, 3);
        assert!(config.model.is_none());
    }

    #[test]
    fn test_triple_config_serde_roundtrip() {
        let original = TripleConfig {
            enabled: true,
            batch_size: 20,
            max_retries: 5,
            model: Some("claude-haiku-4-5-20251001".to_string()),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: TripleConfig = serde_json::from_str(&json).expect("deserialize");
        assert!(deserialized.enabled);
        assert_eq!(deserialized.batch_size, 20);
        assert_eq!(deserialized.max_retries, 5);
        assert_eq!(deserialized.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    }

    #[test]
    fn test_memory_config_has_triple() {
        let config = MemoryConfig::default();
        assert!(!config.triple.enabled);
        assert_eq!(config.triple.batch_size, 10);
    }

    #[test]
    fn test_association_config_defaults() {
        let config = AssociationConfig::default();
        assert!(!config.enabled);
        assert!((config.w_entity - 0.3).abs() < f64::EPSILON);
        assert!((config.w_embedding - 0.5).abs() < f64::EPSILON);
        assert!((config.w_temporal - 0.2).abs() < f64::EPSILON);
        assert!((config.link_threshold - 0.4).abs() < f64::EPSILON);
        assert_eq!(config.max_links_per_memory, 5);
        assert_eq!(config.candidate_limit, 50);
        assert_eq!(config.temporal_window_days, 7);
        assert!((config.initial_strength - 0.5).abs() < f64::EPSILON);
        assert!((config.decay_corecall - 0.95).abs() < f64::EPSILON);
        assert!((config.decay_multi - 0.90).abs() < f64::EPSILON);
        assert!((config.decay_single - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_memory_config_has_association() {
        let config = MemoryConfig::default();
        // Association should be present and disabled by default
        assert!(!config.association.enabled);
        assert_eq!(config.association.candidate_limit, 50);
    }

    #[test]
    fn test_association_config_serde_roundtrip() {
        let original = AssociationConfig::default();
        let json = serde_json::to_string(&original).expect("serialize");
        let deserialized: AssociationConfig = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(original.enabled, deserialized.enabled);
        assert!((original.w_entity - deserialized.w_entity).abs() < f64::EPSILON);
        assert!((original.w_embedding - deserialized.w_embedding).abs() < f64::EPSILON);
        assert!((original.w_temporal - deserialized.w_temporal).abs() < f64::EPSILON);
        assert!((original.link_threshold - deserialized.link_threshold).abs() < f64::EPSILON);
        assert_eq!(original.max_links_per_memory, deserialized.max_links_per_memory);
        assert_eq!(original.candidate_limit, deserialized.candidate_limit);
        assert_eq!(original.temporal_window_days, deserialized.temporal_window_days);
        assert!((original.initial_strength - deserialized.initial_strength).abs() < f64::EPSILON);
        assert!((original.decay_corecall - deserialized.decay_corecall).abs() < f64::EPSILON);
        assert!((original.decay_multi - deserialized.decay_multi).abs() < f64::EPSILON);
        assert!((original.decay_single - deserialized.decay_single).abs() < f64::EPSILON);
    }

    #[test]
    fn test_association_config_serde_custom_values() {
        let custom = AssociationConfig {
            enabled: true,
            w_entity: 0.5,
            w_embedding: 0.3,
            w_temporal: 0.2,
            link_threshold: 0.6,
            max_links_per_memory: 10,
            candidate_limit: 100,
            temporal_window_days: 14,
            initial_strength: 0.7,
            decay_corecall: 0.99,
            decay_multi: 0.95,
            decay_single: 0.80,
        };
        let json = serde_json::to_string(&custom).expect("serialize");
        let deserialized: AssociationConfig = serde_json::from_str(&json).expect("deserialize");

        assert!(deserialized.enabled);
        assert!((deserialized.w_entity - 0.5).abs() < f64::EPSILON);
        assert_eq!(deserialized.candidate_limit, 100);
        assert_eq!(deserialized.temporal_window_days, 14);
    }

    #[test]
    fn test_memory_config_serde_roundtrip_with_association() {
        let mut config = MemoryConfig::default();
        config.association.enabled = true;
        config.association.link_threshold = 0.6;

        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: MemoryConfig = serde_json::from_str(&json).expect("deserialize");

        assert!(deserialized.association.enabled);
        assert!((deserialized.association.link_threshold - 0.6).abs() < f64::EPSILON);
        // Other fields preserved
        assert!((deserialized.mu1 - config.mu1).abs() < f64::EPSILON);
    }
}
