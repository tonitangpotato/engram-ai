# Adaptive Parameter Tuning

Engram can automatically tune its own parameters based on performance metrics. This is useful when you're not sure which preset to use, or when your agent's usage patterns change over time.

## Quick Start

```python
from engram import Memory

# Enable adaptive tuning
mem = Memory("agent.db", adaptive_tuning=True)

# Use normally - parameters adjust automatically
mem.add("some knowledge")
results = mem.recall("query")
mem.reward("good job!")  # Feedback helps tuning
mem.consolidate()

# Check tuning metrics
stats = mem.stats()
print(stats["adaptive_tuning"])
```

## How It Works

The adaptive tuner tracks three key metrics:

1. **Hit Rate** - Fraction of successful recalls (non-empty results)
2. **Reward Ratio** - Positive feedback / total feedback
3. **Forget Rate** - Memories forgotten per consolidation cycle

Based on these metrics, it adjusts parameters:

| Metric | Condition | Parameter Change | Effect |
|--------|-----------|-----------------|--------|
| Hit rate < 60% | Too many empty recalls | Lower `min_activation` | More permissive search |
| Hit rate > 90% | Almost always successful | Raise `min_activation` | More selective search |
| Reward ratio < 40% | Lots of negative feedback | Increase `context_weight` | More context-sensitive |
| Forget rate > 10 | Too much forgetting | Reduce `mu1`, `mu2` | Slower decay |
| Forget rate < 2 | Too little forgetting | Increase `mu1`, `mu2` | Faster decay |
| Reward ratio > 70% | Mostly positive | Increase `alpha` | Faster consolidation |

## Configuration

```python
from engram import Memory, AdaptiveTuner

mem = Memory("agent.db", adaptive_tuning=True)

# Customize tuning behavior
mem._adaptive_tuner.adaptation_rate = 0.1  # How aggressively to adapt (0.01-0.2)
mem._adaptive_tuner.min_samples = 20       # Minimum data before adapting
mem._adaptive_tuner.adaptation_interval = 3600  # Min seconds between adaptations
```

## When to Use Adaptive Tuning

**Use it when:**
- You're building a new agent and don't know which preset to use
- Your agent's workload varies significantly over time
- You want to optimize for your specific use case

**Don't use it when:**
- You need deterministic behavior (e.g., testing, research)
- Your agent has very few interactions (not enough data to tune)
- You've already carefully tuned parameters manually

## Monitoring

Check tuning metrics in `mem.stats()`:

```python
stats = mem.stats()
print(stats["adaptive_tuning"])
# {
#   "hit_rate": 0.65,
#   "reward_ratio": 0.55,
#   "forget_rate": 4.2,
#   "avg_retrieval_time": 0.0023,
#   "total_recalls": 142,
#   "successful_recalls": 92,
#   "positive_rewards": 11,
#   "negative_rewards": 9,
#   "memories_forgotten": 21,
#   "consolidation_cycles": 5
# }
```

## Advanced: Manual Tuning

You can also use `AdaptiveTuner` separately for offline analysis:

```python
from engram import MemoryConfig, AdaptiveTuner

config = MemoryConfig.chatbot()
tuner = AdaptiveTuner(config, adaptation_rate=0.05)

# Manually record metrics
tuner.record_recall([result1, result2])
tuner.record_reward("positive")
tuner.record_consolidation(n_forgotten=3)

# Check if ready to adapt
if tuner.should_adapt():
    changes = tuner.adapt()
    print(f"Parameter changes: {changes}")

# Get current metrics
metrics = tuner.get_metrics()
```

## Limitations

- Requires at least 20 recall samples before first adaptation
- Only adapts once per hour by default (to avoid over-tuning)
- Does not save tuning state across restarts (resets each session)
- Parameter changes are gradual (5-10% per adaptation by default)

## Design Philosophy

The goal is **not** to find the "perfect" parameters, but to prevent catastrophically bad configurations. Think of it as an automatic safety net, not a replacement for thoughtful design.

For most agents, starting with a preset and letting adaptive tuning handle edge cases is the best approach.
