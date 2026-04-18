//! Memory configuration presets and tunable parameters.

use serde::{Deserialize, Serialize};

use crate::embeddings::EmbeddingConfig;
use crate::entities::EntityConfig;

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
