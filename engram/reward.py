"""
Reward-Modulated Learning — Dopaminergic Feedback Signals

Neuroscience basis: The brain's dopamine system modulates memory encoding
and consolidation based on reward prediction errors. Positive outcomes
(unexpected rewards) strengthen associated memory traces via VTA → hippocampus
projections. Negative outcomes weaken or suppress them.

In an agent context, the "master" (user) provides implicit feedback through
natural language. We detect positive/negative signals and apply them as
reward modulation to recently active memories.

This implements a simplified version of:
- Lisman & Grace (2005) — Hippocampal-VTA loop for memory encoding
- Shohamy & Adcock (2010) — Dopamine, motivation, and memory

The key insight: memories formed around rewarded events are preferentially
consolidated, while punished memories are suppressed. This naturally
shapes the agent's behavior over time.
"""

import sys
import os
import time


from engram.core import MemoryStore, MemoryEntry


# Bilingual feedback signals (Chinese + English)
POSITIVE_SIGNALS = [
    "好的", "不错", "对", "对的", "很好", "棒", "可以", "行",
    "good", "nice", "correct", "yes", "right", "exactly", "perfect",
    "great", "thanks", "thank you", "awesome", "love it", "well done",
]

NEGATIVE_SIGNALS = [
    "不对", "别这样", "错", "错了", "不行", "不好", "停", "别",
    "wrong", "no", "don't", "stop", "bad", "incorrect", "nope",
    "that's wrong", "not right", "undo", "cancel",
]


def detect_feedback(text: str) -> tuple[str, float]:
    """
    Detect positive/negative feedback from natural language.

    Uses keyword matching with confidence based on signal strength.
    Returns (polarity, confidence) where:
    - polarity: "positive", "negative", or "neutral"
    - confidence: 0-1 how sure we are about the detection

    Multiple matching signals increase confidence (additive evidence).

    Args:
        text: User's message text

    Returns:
        Tuple of (polarity: str, confidence: float)
    """
    text_lower = text.lower().strip()

    pos_matches = sum(1 for s in POSITIVE_SIGNALS if s.lower() in text_lower)
    neg_matches = sum(1 for s in NEGATIVE_SIGNALS if s.lower() in text_lower)

    if pos_matches == 0 and neg_matches == 0:
        return ("neutral", 0.0)

    # Net polarity with confidence from match count
    if pos_matches > neg_matches:
        # Confidence: 1 match = 0.5, 2 = 0.75, 3+ = 0.9
        confidence = min(0.95, 0.3 + 0.2 * pos_matches)
        return ("positive", confidence)
    elif neg_matches > pos_matches:
        confidence = min(0.95, 0.3 + 0.2 * neg_matches)
        return ("negative", confidence)
    else:
        # Equal matches — ambiguous
        return ("neutral", 0.1)


def apply_reward(store, feedback_polarity: str,
                 recent_n: int = 3, reward_magnitude: float = 0.15):
    """
    Apply reward/punishment to the N most recently accessed memories.

    Neuroscience basis: Dopamine release is temporally diffuse — it
    doesn't just affect the exact moment of reward, but spreads to
    recent experiences (eligibility traces). This is why we reward
    the last N memories, not just the most recent one.

    Positive feedback:
    - Boosts importance (makes memory consolidate faster)
    - Adds a small strength bonus to working_strength

    Negative feedback:
    - Reduces importance (deprioritizes in consolidation)
    - Slightly suppresses working_strength

    Args:
        store: MemoryStore to modify
        feedback_polarity: "positive" or "negative"
        recent_n: Number of recent memories to affect
        reward_magnitude: Strength of the reward signal (0-1)
    """
    if feedback_polarity not in ("positive", "negative"):
        return

    # Get N most recently accessed memories
    all_memories = store.all()
    if not all_memories:
        return

    # Sort by most recent access time
    def last_access(m: MemoryEntry) -> float:
        return max(m.access_times) if m.access_times else m.created_at

    sorted_memories = sorted(all_memories, key=last_access, reverse=True)
    targets = sorted_memories[:recent_n]

    for i, entry in enumerate(targets):
        # Temporal discount: most recent gets full reward, earlier ones get less
        # This models the eligibility trace decay
        discount = 1.0 / (1.0 + 0.5 * i)

        if feedback_polarity == "positive":
            # Boost importance (clamped to 1.0)
            entry.importance = min(1.0, entry.importance + reward_magnitude * discount)
            # Small working strength boost (dopamine-enhanced encoding)
            entry.working_strength += 0.05 * discount
        else:
            # Reduce importance (clamped to 0.0)
            entry.importance = max(0.0, entry.importance - reward_magnitude * discount)
            # Slight suppression
            entry.working_strength *= (1.0 - 0.1 * discount)

        _update = getattr(store, 'update', None)
        if _update: _update(entry)


if __name__ == "__main__":
    """Demo: feedback detection and reward application."""
    from engram.core import MemoryType

    # Test feedback detection
    test_phrases = [
        "good job, that's exactly right",
        "no that's wrong, stop",
        "好的不错",
        "the weather is nice today",
        "yes but also no",
        "错了别这样",
    ]

    print("=== Feedback Detection Demo ===\n")
    for phrase in test_phrases:
        polarity, conf = detect_feedback(phrase)
        print(f"  '{phrase}' → {polarity} (conf={conf:.2f})")

    # Test reward application
    print("\n=== Reward Application Demo ===\n")
    store = MemoryStore()
    m1 = store.add("SaltyHall uses Supabase", MemoryType.FACTUAL, importance=0.3)
    m2 = store.add("potato prefers Opus", MemoryType.RELATIONAL, importance=0.5)
    m3 = store.add("Deploy with vercel --prod", MemoryType.PROCEDURAL, importance=0.4)

    print("  Before reward:")
    for m in store.all():
        print(f"    imp={m.importance:.2f} w={m.working_strength:.2f} | {m.content[:40]}")

    apply_reward(store, "positive", recent_n=3)

    print("\n  After positive reward:")
    for m in store.all():
        print(f"    imp={m.importance:.2f} w={m.working_strength:.2f} | {m.content[:40]}")
