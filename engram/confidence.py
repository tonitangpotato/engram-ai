"""
Confidence Scoring — Two-Dimensional Metacognitive Monitoring

Key insight: There are TWO distinct types of confidence for a memory:

1. **Content reliability** — How accurate/trustworthy is this memory's content?
   - Based on source quality, not time
   - A bot's own recorded event is reliable regardless of age
   - Hearsay or uncertain info has lower reliability from the start
   - Does NOT decay with time (facts don't become less true)

2. **Retrieval salience** — How "top of mind" is this memory?
   - Based on ACT-R activation (recency, frequency, importance)
   - Decays with time — old memories are harder to "think of"
   - Used for ranking search results, NOT for judging accuracy

Previous (incorrect) design: used effective_strength as "confidence",
implying old memories are unreliable. This is wrong — a 3-month-old
factual record is just as accurate, it's just not top-of-mind.

Neuroscience basis:
- Retrieval salience ≈ activation level (ACT-R base-level activation)
- Content reliability ≈ source monitoring (prefrontal cortex)
- The brain distinguishes these: you can "know you know something"
  (high reliability) but struggle to recall it (low salience)

References:
- Nelson & Narens (1990) — Metamemory framework
- Koriat (1993) — Feeling-of-knowing judgments
- Johnson et al. (1993) — Source monitoring framework
"""

import math
import time as _time
from engram.core import MemoryEntry, MemoryStore
from engram.forgetting import effective_strength


# Default content reliability by memory type
# These reflect how inherently trustworthy each type is
DEFAULT_RELIABILITY = {
    "factual": 0.85,       # Facts recorded by bot — generally reliable
    "episodic": 0.90,      # Events bot witnessed — very reliable
    "relational": 0.75,    # Inferred preferences — somewhat reliable
    "emotional": 0.95,     # Emotional events — vividly remembered
    "procedural": 0.90,    # How-to knowledge — tested and verified
    "opinion": 0.60,       # Opinions — inherently subjective
}


def content_reliability(entry: MemoryEntry) -> float:
    """
    How trustworthy is this memory's content?

    Based on:
    - Memory type (factual/episodic are reliable, opinions less so)
    - Source (bot's own observation vs hearsay)
    - Whether it's been contradicted/updated

    Does NOT decay with time. A fact recorded 6 months ago
    is just as reliable as one recorded today.

    Returns:
        Float 0-1 reliability score
    """
    type_str = entry.memory_type.value if hasattr(entry.memory_type, 'value') else str(entry.memory_type)
    base = DEFAULT_RELIABILITY.get(type_str, 0.7)

    # Pinned memories are explicitly verified by human
    if entry.pinned:
        base = max(base, 0.95)

    # High importance memories were tagged as significant at creation
    importance_boost = entry.importance * 0.1  # up to +0.1
    
    return min(1.0, base + importance_boost)


def retrieval_salience(entry: MemoryEntry, store=None,
                       now: float = None) -> float:
    """
    How "top of mind" is this memory?

    Uses effective_strength (ACT-R activation × forgetting curve).
    Normalized against store max if available.

    This DOES decay with time — old memories are less salient.
    Used for search result ranking, NOT for judging accuracy.

    Returns:
        Float 0-1 salience score
    """
    eff = effective_strength(entry, now=now)

    if store is not None:
        all_strengths = [effective_strength(m, now=now) for m in store.all()]
        max_strength = max(all_strengths) if all_strengths else 1.0

        if max_strength <= 0:
            return 0.0

        raw = eff / max_strength
    else:
        # Sigmoid mapping for absolute salience
        raw = 2.0 / (1.0 + math.exp(-2.0 * eff)) - 1.0
        raw = max(0.0, raw)

    return min(1.0, max(0.0, raw))


def confidence_score(entry: MemoryEntry, store=None,
                     now: float = None) -> float:
    """
    Overall confidence — combines reliability and salience.

    For backwards compatibility, returns a single 0-1 score.
    Weighted toward reliability (content accuracy matters more
    than how easily it comes to mind).

    Formula: 0.7 * reliability + 0.3 * salience
    """
    rel = content_reliability(entry)
    sal = retrieval_salience(entry, store=store, now=now)
    return 0.7 * rel + 0.3 * sal


def confidence_detail(entry: MemoryEntry, store=None,
                      now: float = None) -> dict:
    """
    Full confidence breakdown.

    Returns dict with:
    - reliability: content trustworthiness (0-1, stable over time)
    - salience: retrieval strength (0-1, decays over time)
    - combined: weighted overall score
    - label: human-readable label
    - description: explanation string for the agent to use
    """
    rel = content_reliability(entry)
    sal = retrieval_salience(entry, store=store, now=now)
    combined = 0.7 * rel + 0.3 * sal
    label = confidence_label(combined)

    # Generate description the agent can use in responses
    if rel >= 0.8 and sal >= 0.7:
        desc = "I clearly remember this"
    elif rel >= 0.8 and sal < 0.4:
        desc = "I have a reliable record of this, though it's from a while ago"
    elif rel < 0.6:
        desc = "I have a note about this but I'm not sure how accurate it is"
    else:
        desc = "I recall this but the details might not be exact"

    return {
        "reliability": round(rel, 3),
        "salience": round(sal, 3),
        "combined": round(combined, 3),
        "label": label,
        "description": desc,
    }


def confidence_label(score: float) -> str:
    """
    Human-readable confidence label.

    - certain (0.8-1.0): Strong on both reliability and salience
    - likely (0.6-0.8): Good reliability, moderate salience
    - uncertain (0.4-0.6): Moderate reliability or low salience
    - vague (0.0-0.4): Low reliability or very old/weak memory
    """
    if score >= 0.8:
        return "certain"
    elif score >= 0.6:
        return "likely"
    elif score >= 0.4:
        return "uncertain"
    else:
        return "vague"


if __name__ == "__main__":
    """Demo: two-dimensional confidence scoring."""
    from engram.core import MemoryType

    store = MemoryStore()
    now = _time.time()

    # Fresh factual memory
    m1 = store.add("SaltyHall uses Supabase", MemoryType.FACTUAL, importance=0.5)

    # Old episodic memory (30 days ago)
    m2 = store.add("SaltyHall launched on Jan 3", MemoryType.EPISODIC, importance=0.7)
    m2.access_times = [now - 30 * 86400]
    m2.created_at = now - 30 * 86400
    m2.working_strength = 0.1

    # Opinion
    m3 = store.add("I think graph+text hybrid is best", MemoryType.OPINION, importance=0.3)

    print("=== Two-Dimensional Confidence Demo ===\n")
    print(f"  {'Content':<45} {'Reliability':>11} {'Salience':>9} {'Combined':>9} {'Label':>10}")
    print(f"  {'─'*90}")
    for m in store.all():
        d = confidence_detail(m, store=store, now=now)
        print(f"  {m.content[:43]:<45} {d['reliability']:>9.2f}   {d['salience']:>7.2f}   {d['combined']:>7.2f}   {d['label']:>9}")
        print(f"  {' '*45} → \"{d['description']}\"")
        print()
