# Interleaved Replay in Bots vs Biological Systems

## The Question

Interleaved replay in neuroscience prevents catastrophic forgetting — new learning physically interfering with old memory traces in neural networks. But bots store memories in databases. New memories don't physically overwrite old ones. So why do bots need interleaved replay?

## The Answer: Activation-Level Forgetting

Bots don't lose data, but they can lose **accessibility**. The problem is at the activation/ranking layer:

- New memories enter with high `working_strength`
- Old memories undergo continuous decay
- At recall time, old memories get ranked below newer ones
- Over time, old memories effectively become unretrievable — not because they're gone, but because they never surface

This is analogous to a human saying "I know I know this, but I can't remember it right now." The information exists but can't be accessed.

## What Interleaved Replay Does for Bots

During consolidation ("sleep"), engram randomly samples archived memories and gives them a small `core_strength` boost. This:

1. **Maintains a retrieval floor** — old memories don't sink to zero activation
2. **Preserves long-term knowledge** — important old facts can still compete with recent ones
3. **Prevents recency bias** — agent doesn't become myopic, only remembering recent events

## Key Distinction

| | Biological Systems | Bot Systems |
|---|---|---|
| **What's at risk** | Physical memory traces | Activation rankings |
| **Failure mode** | Data loss (overwritten synapses) | Data inaccessibility (activation too low) |
| **Replay prevents** | Catastrophic forgetting | Recency-dominated retrieval |
| **Mechanism** | Hippocampal replay during sleep | Periodic core_strength boost during consolidation |

## Implication

The mathematical mechanism (periodic reactivation of old traces) transfers from neuroscience to bots, but the *reason* is different. In brains, it's about survival of information. In bots, it's about fairness of access. The implementation is the same; the justification is distinct.

This is a good example of engram's philosophy: borrow the mechanism from neuroscience when it solves a real bot problem, but be clear about *why* it works in this new context.
