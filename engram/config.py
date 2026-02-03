"""
Memory Configuration — Tunable Parameters

All hardcoded parameters from the neuroscience models, extracted into
a single config dataclass. Supports presets for common agent archetypes.

See docs/design/parameter-tuning.md for the tuning strategy.
"""

from dataclasses import dataclass, field
from typing import Optional


@dataclass
class MemoryConfig:
    """All tunable parameters for the Engram memory system.

    Default values come from neuroscience literature (ACT-R, Memory Chain Model,
    Ebbinghaus forgetting curve). These are reasonable starting points but
    not optimized for AI agents — use presets for better defaults.
    """

    # === Forgetting (Ebbinghaus + interference) ===
    # Spacing effect multiplier in stability computation: log1p(n_accesses) weight
    spacing_factor: float = 0.5
    # Importance floor in stability: importance_factor = floor + importance
    importance_floor: float = 0.5
    # Consolidation bonus per consolidation count
    consolidation_bonus: float = 0.2
    # Effective strength threshold for pruning
    forget_threshold: float = 0.01
    # Retrieval-induced forgetting: suppression magnitude
    suppression_factor: float = 0.05
    # Word overlap threshold for competing memories
    overlap_threshold: float = 0.3

    # === Consolidation (Memory Chain Model — Murre & Chessa 2011) ===
    # Working memory decay rate (per day). Higher = faster decay.
    mu1: float = 0.15
    # Core memory decay rate (per day). Higher = faster decay.
    mu2: float = 0.005
    # Consolidation transfer rate (working → core per day)
    alpha: float = 0.08
    # Importance modulation floor for consolidation
    consolidation_importance_floor: float = 0.2
    # Fraction of archived memories replayed per cycle
    interleave_ratio: float = 0.3
    # Core strength boost per replayed archived memory (base)
    replay_boost: float = 0.01
    # Layer rebalancing thresholds
    promote_threshold: float = 0.25
    demote_threshold: float = 0.05
    archive_threshold: float = 0.15

    # === Activation (ACT-R) ===
    # Base-level activation decay parameter (d in t^-d)
    actr_decay: float = 0.5
    # Context spreading activation weight
    context_weight: float = 1.5
    # Importance weight in retrieval activation
    importance_weight: float = 0.5
    # Minimum activation for retrieval
    min_activation: float = -10.0

    # === Confidence (metacognitive scoring) ===
    # Default content reliability by memory type
    default_reliability: dict = field(default_factory=lambda: {
        "factual": 0.85,
        "episodic": 0.90,
        "relational": 0.75,
        "emotional": 0.95,
        "procedural": 0.90,
        "opinion": 0.60,
    })
    # Weight of reliability vs salience in combined confidence
    confidence_reliability_weight: float = 0.7
    confidence_salience_weight: float = 0.3
    # Sigmoid steepness for absolute salience mapping
    salience_sigmoid_k: float = 2.0

    # === Reward (dopaminergic feedback) ===
    # Default reward magnitude
    reward_magnitude: float = 0.15
    # Number of recent memories affected by reward
    reward_recent_n: int = 3
    # Working strength boost on positive feedback
    reward_strength_boost: float = 0.05
    # Working strength suppression on negative feedback
    reward_suppression: float = 0.1
    # Temporal discount factor for eligibility trace
    reward_temporal_discount: float = 0.5

    # === Downscaling (synaptic homeostasis) ===
    # Global downscaling factor per consolidation cycle
    downscale_factor: float = 0.95

    # === Anomaly detection (predictive coding) ===
    # Rolling window size for baseline tracking
    anomaly_window_size: int = 100
    # Standard deviations for anomaly threshold
    anomaly_sigma_threshold: float = 2.0
    # Minimum samples before anomaly detection activates
    anomaly_min_samples: int = 5

    @classmethod
    def default(cls) -> "MemoryConfig":
        """Literature-based defaults (same as no-arg constructor)."""
        return cls()

    @classmethod
    def chatbot(cls) -> "MemoryConfig":
        """Preset for conversational chatbots.

        High replay, slow decay — optimized for long conversations
        where recalling old context matters.
        """
        return cls(
            mu1=0.08,           # Slower working decay (keep context longer)
            mu2=0.003,          # Slower core decay
            alpha=0.12,         # Faster consolidation (conversations move fast)
            interleave_ratio=0.4,  # More replay (don't lose old conversation context)
            replay_boost=0.015,
            actr_decay=0.4,     # Gentler activation decay
            context_weight=2.0, # Stronger context matching (conversation is contextual)
            downscale_factor=0.96,  # Gentler downscaling
            reward_magnitude=0.2,   # Stronger reward signal (chatbots get lots of feedback)
            forget_threshold=0.005, # Harder to forget
        )

    @classmethod
    def task_agent(cls) -> "MemoryConfig":
        """Preset for short-lived task agents.

        Fast decay, low replay — focus on recent task context,
        let old task memories expire quickly.
        """
        return cls(
            mu1=0.25,           # Fast working decay (tasks are short)
            mu2=0.01,           # Faster core decay too
            alpha=0.05,         # Slower consolidation (tasks don't need long-term)
            interleave_ratio=0.1,  # Minimal replay
            replay_boost=0.005,
            actr_decay=0.6,     # Steeper activation decay
            promote_threshold=0.35,  # Harder to promote to core
            archive_threshold=0.2,   # Easier to archive
            downscale_factor=0.90,   # Aggressive downscaling
            forget_threshold=0.02,   # Easier to forget
        )

    @classmethod
    def personal_assistant(cls) -> "MemoryConfig":
        """Preset for long-term personal assistants.

        Very slow core decay, medium replay — remember preferences
        and facts about the user for months.
        """
        return cls(
            mu1=0.12,           # Moderate working decay
            mu2=0.001,          # Very slow core decay (remember for months)
            alpha=0.10,         # Good consolidation rate
            interleave_ratio=0.3,
            replay_boost=0.02,  # Stronger replay boost
            actr_decay=0.45,    # Moderate activation decay
            importance_weight=0.7,  # Importance matters more (preferences are key)
            promote_threshold=0.20,  # Easier to promote (keep more in core)
            demote_threshold=0.03,   # Harder to demote
            downscale_factor=0.97,   # Very gentle downscaling
            forget_threshold=0.005,  # Very hard to forget
            confidence_reliability_weight=0.8,  # Trust stored facts more
            confidence_salience_weight=0.2,
        )

    @classmethod
    def researcher(cls) -> "MemoryConfig":
        """Preset for research agents.

        Minimal forgetting — everything might be relevant later.
        High replay to maintain all knowledge.
        """
        return cls(
            mu1=0.05,           # Very slow working decay
            mu2=0.001,          # Minimal core decay
            alpha=0.15,         # Fast consolidation (lock things into core)
            interleave_ratio=0.5,  # Heavy replay (everything matters)
            replay_boost=0.025,
            actr_decay=0.35,    # Gentle activation decay
            context_weight=2.0, # Strong context matching
            importance_weight=0.3,  # Don't over-weight importance (all info matters)
            promote_threshold=0.15,  # Easy to promote
            demote_threshold=0.02,   # Hard to demote
            archive_threshold=0.10,
            downscale_factor=0.98,   # Minimal downscaling
            forget_threshold=0.001,  # Almost never forget
        )
