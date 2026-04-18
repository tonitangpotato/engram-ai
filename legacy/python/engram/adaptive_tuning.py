"""
Adaptive Parameter Tuning

Automatically adjusts memory system parameters based on performance metrics.
Implements a feedback loop that tunes configuration based on:
- Recall hit rate (search effectiveness)
- Reward feedback (agent performance)
- Forgetting rate (memory stability)

Design Philosophy:
    Rather than manually tuning parameters for each agent archetype,
    let the system learn its optimal configuration from usage patterns.
    
    This is inspired by adaptive learning rate in neural networks
    and Bayesian optimization in AutoML.

Key Metrics:
    - hit_rate: fraction of recalls that returned relevant results
    - reward_ratio: positive feedback / total feedback
    - forget_rate: memories forgotten per consolidation cycle
    - retrieval_latency: time to complete recall queries

Tuning Rules:
    1. Low hit rate → increase search diversity (lower activation threshold)
    2. High reward ratio → trust current params (slower adaptation)
    3. High forget rate → reduce decay rates (retain more)
    4. Low forget rate → increase decay rates (prune more aggressively)
"""

import time
import math
from dataclasses import dataclass, field
from typing import Optional
from engram.config import MemoryConfig


@dataclass
class AdaptiveMetrics:
    """Tracks performance metrics for adaptive tuning."""
    
    # Search effectiveness
    total_recalls: int = 0
    successful_recalls: int = 0  # Non-empty results
    
    # Feedback signal
    positive_rewards: int = 0
    negative_rewards: int = 0
    
    # Memory dynamics
    memories_forgotten: int = 0
    consolidation_cycles: int = 0
    
    # Performance tracking
    total_retrieval_time: float = 0.0
    
    # Timestamp
    last_updated: float = field(default_factory=time.time)
    
    def hit_rate(self) -> float:
        """Fraction of successful recalls (non-empty results)."""
        if self.total_recalls == 0:
            return 1.0  # Neutral until we have data
        return self.successful_recalls / self.total_recalls
    
    def reward_ratio(self) -> float:
        """Positive / (positive + negative) feedback ratio."""
        total_feedback = self.positive_rewards + self.negative_rewards
        if total_feedback == 0:
            return 0.5  # Neutral
        return self.positive_rewards / total_feedback
    
    def forget_rate(self) -> float:
        """Average memories forgotten per consolidation cycle."""
        if self.consolidation_cycles == 0:
            return 0.0
        return self.memories_forgotten / self.consolidation_cycles
    
    def avg_retrieval_time(self) -> float:
        """Average time per recall query (seconds)."""
        if self.total_recalls == 0:
            return 0.0
        return self.total_retrieval_time / self.total_recalls


class AdaptiveTuner:
    """
    Adaptive parameter tuner for memory system.
    
    Usage:
        tuner = AdaptiveTuner(config, adaptation_rate=0.1)
        
        # After each recall
        tuner.record_recall(results, latency)
        
        # After reward feedback
        tuner.record_reward(polarity)
        
        # After consolidation
        tuner.record_consolidation(n_forgotten)
        
        # Periodically update config
        if tuner.should_adapt():
            tuner.adapt()
    """
    
    def __init__(
        self,
        config: MemoryConfig,
        adaptation_rate: float = 0.05,
        min_samples: int = 20,
        adaptation_interval: float = 3600.0,  # 1 hour
    ):
        """
        Initialize adaptive tuner.
        
        Args:
            config: MemoryConfig to tune (modified in-place)
            adaptation_rate: How aggressively to adjust params (0.01-0.2)
            min_samples: Minimum samples before adapting
            adaptation_interval: Minimum seconds between adaptations
        """
        self.config = config
        self.adaptation_rate = adaptation_rate
        self.min_samples = min_samples
        self.adaptation_interval = adaptation_interval
        
        self.metrics = AdaptiveMetrics()
        self._last_adaptation = time.time()
        
        # Track original config for bounds checking
        self._original_config = MemoryConfig.default()
    
    def record_recall(self, results: list, latency: float = 0.0):
        """Record a recall query result."""
        self.metrics.total_recalls += 1
        if len(results) > 0:
            self.metrics.successful_recalls += 1
        self.metrics.total_retrieval_time += latency
        self.metrics.last_updated = time.time()
    
    def record_reward(self, polarity: str):
        """Record reward feedback (positive/negative)."""
        if polarity == "positive":
            self.metrics.positive_rewards += 1
        elif polarity == "negative":
            self.metrics.negative_rewards += 1
        self.metrics.last_updated = time.time()
    
    def record_consolidation(self, n_forgotten: int = 0):
        """Record a consolidation cycle completion."""
        self.metrics.consolidation_cycles += 1
        self.metrics.memories_forgotten += n_forgotten
        self.metrics.last_updated = time.time()
    
    def should_adapt(self) -> bool:
        """Check if enough data has been collected to adapt."""
        time_since_last = time.time() - self._last_adaptation
        has_enough_samples = (
            self.metrics.total_recalls >= self.min_samples or
            self.metrics.consolidation_cycles >= 3
        )
        return has_enough_samples and time_since_last >= self.adaptation_interval
    
    def adapt(self) -> dict[str, float]:
        """
        Adapt configuration based on collected metrics.
        
        Returns:
            Dict of parameter changes: {param_name: new_value}
        """
        if not self.should_adapt():
            return {}
        
        changes = {}
        
        # === Rule 1: Hit rate → Search threshold tuning ===
        hit_rate = self.metrics.hit_rate()
        if hit_rate < 0.6:
            # Low hit rate → lower activation threshold (more permissive search)
            # min_activation is negative, so we make it MORE negative by subtracting
            new_threshold = self.config.min_activation - abs(self.config.min_activation * self.adaptation_rate)
            new_threshold = max(new_threshold, -15.0)  # Don't go too permissive
            if new_threshold != self.config.min_activation:
                changes["min_activation"] = new_threshold
                self.config.min_activation = new_threshold
        
        elif hit_rate > 0.9:
            # Very high hit rate → might be too permissive, tighten threshold
            # Make it LESS negative (closer to 0) by adding
            new_threshold = self.config.min_activation + abs(self.config.min_activation * self.adaptation_rate / 2)
            new_threshold = min(new_threshold, -5.0)  # Don't go too restrictive
            if new_threshold != self.config.min_activation:
                changes["min_activation"] = new_threshold
                self.config.min_activation = new_threshold
        
        # === Rule 2: Reward ratio → Activation parameter tuning ===
        reward_ratio = self.metrics.reward_ratio()
        if reward_ratio < 0.4 and self.metrics.positive_rewards + self.metrics.negative_rewards > 5:
            # Low reward → increase context weight (more context-sensitive retrieval)
            new_weight = self.config.context_weight * (1 + self.adaptation_rate)
            new_weight = min(new_weight, 3.0)  # Cap at 3x
            if new_weight != self.config.context_weight:
                changes["context_weight"] = new_weight
                self.config.context_weight = new_weight
        
        # === Rule 3: Forget rate → Decay tuning ===
        forget_rate = self.metrics.forget_rate()
        if forget_rate > 10.0:  # More than 10 memories forgotten per cycle
            # Too much forgetting → slow down decay
            new_mu1 = self.config.mu1 * (1 - self.adaptation_rate)
            new_mu2 = self.config.mu2 * (1 - self.adaptation_rate)
            new_mu1 = max(new_mu1, 0.01)  # Don't go too slow
            new_mu2 = max(new_mu2, 0.0001)
            
            if new_mu1 != self.config.mu1:
                changes["mu1"] = new_mu1
                self.config.mu1 = new_mu1
            if new_mu2 != self.config.mu2:
                changes["mu2"] = new_mu2
                self.config.mu2 = new_mu2
        
        elif forget_rate < 2.0 and self.metrics.consolidation_cycles >= 5:
            # Too little forgetting → speed up decay (prevent memory bloat)
            new_mu1 = self.config.mu1 * (1 + self.adaptation_rate)
            new_mu2 = self.config.mu2 * (1 + self.adaptation_rate)
            new_mu1 = min(new_mu1, 0.5)  # Don't go too fast
            new_mu2 = min(new_mu2, 0.02)
            
            if new_mu1 != self.config.mu1:
                changes["mu1"] = new_mu1
                self.config.mu1 = new_mu1
            if new_mu2 != self.config.mu2:
                changes["mu2"] = new_mu2
                self.config.mu2 = new_mu2
        
        # === Rule 4: Consolidation effectiveness → Alpha tuning ===
        # If we're getting positive feedback, current consolidation rate is good
        # If negative, try adjusting consolidation speed
        if reward_ratio > 0.7 and self.metrics.positive_rewards >= 5:
            # Good performance → slightly increase consolidation (lock in knowledge faster)
            new_alpha = self.config.alpha * (1 + self.adaptation_rate / 2)
            new_alpha = min(new_alpha, 0.3)  # Cap consolidation rate
            if new_alpha != self.config.alpha:
                changes["alpha"] = new_alpha
                self.config.alpha = new_alpha
        
        # Update timestamp
        self._last_adaptation = time.time()
        
        return changes
    
    def get_metrics(self) -> dict:
        """Get current performance metrics."""
        return {
            "hit_rate": round(self.metrics.hit_rate(), 3),
            "reward_ratio": round(self.metrics.reward_ratio(), 3),
            "forget_rate": round(self.metrics.forget_rate(), 2),
            "avg_retrieval_time": round(self.metrics.avg_retrieval_time(), 4),
            "total_recalls": self.metrics.total_recalls,
            "successful_recalls": self.metrics.successful_recalls,
            "positive_rewards": self.metrics.positive_rewards,
            "negative_rewards": self.metrics.negative_rewards,
            "memories_forgotten": self.metrics.memories_forgotten,
            "consolidation_cycles": self.metrics.consolidation_cycles,
        }
    
    def reset_metrics(self):
        """Reset collected metrics (useful after major config changes)."""
        self.metrics = AdaptiveMetrics()
