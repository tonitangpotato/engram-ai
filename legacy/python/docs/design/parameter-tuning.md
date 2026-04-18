# Parameter Tuning Strategy

## Problem

Engram has many hardcoded parameters (decay rates, consolidation rates, replay ratios, suppression factors, etc.) derived from neuroscience literature. These values were fitted to biological neurons, not AI agents. Hardcoding is wrong.

## Three-Layer Approach

### Layer 1: Configurable Defaults (from Literature)

All parameters exposed via `Memory("agent.db", config={...})`. Default values come from peer-reviewed sources:

- **ACT-R decay** (d=0.5) — Anderson et al., fitted from human recall experiments
- **Ebbinghaus forgetting curve** — 100+ years of replication data
- **Memory Chain rates** (μ₁=0.15, μ₂=0.005, α=0.08) — Murre & Chessa 2011
- **Interleaved replay** (30% ratio, 0.01 boost) — needs tuning for bots
- **Retrieval-induced forgetting** (0.05 suppression, 0.3 overlap threshold)

These are reasonable starting points but not optimal for bots.

### Layer 2: Presets (Agent Archetypes)

Pre-tuned parameter sets for common use cases:

| Preset | Replay | Decay | Consolidation | Use Case |
|--------|--------|-------|---------------|----------|
| `chatbot` | High replay, slow decay | Low μ₁ | Fast α | Long conversations, lots of memories, need to recall old context |
| `task-agent` | Low replay, fast decay | High μ₁ | Slow α | Short-lived tasks, memories expire quickly, focus on recent |
| `personal-assistant` | Medium replay, very slow core decay | Low μ₂ | Medium α | Long-term relationship, remember preferences/facts for months |
| `researcher` | High replay, slow decay everywhere | Low μ₁, μ₂ | Fast α | Never lose information, everything might be relevant later |

Usage: `Memory("agent.db", preset="personal-assistant")`

### Layer 3: Adaptive Tuning (Future / Paper Contribution)

Self-adjusting parameters based on actual usage feedback:

**Signals:**
- Recall hit rate — if recalled memories are frequently used by the agent, parameters are good
- Reward feedback — if `reward()` is called after recall, that memory was useful
- Miss rate — if agent can't find what it needs, old memories are decaying too fast
- Noise rate — if recalled memories are mostly irrelevant, not enough forgetting

**Mechanism:**
- Track running statistics of recall quality
- Adjust parameters with small deltas (e.g., ±5% per consolidation cycle)
- Bounded by min/max ranges to prevent runaway

**This is a paper contribution point** — "adaptive parameterization of cognitive memory models for artificial agents."

## Bot ≠ Human

Key differences that affect parameter choices:

| Factor | Human | Bot |
|--------|-------|-----|
| Access pattern | Irregular, sleep cycles | Continuous or bursty |
| Memory volume | ~100k/day experiences | Depends on usage, could be 10 or 10,000 |
| Time scale | Years of memories | Days to months typically |
| Forgetting | Biological, irreversible | Activation decay, always recoverable |
| Consolidation | During sleep (8h cycles) | Scheduled, can run anytime |

Neuroscience parameters are a starting point. Bot-optimal values will diverge.
