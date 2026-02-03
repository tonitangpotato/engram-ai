"""
Forgetting Model — Ebbinghaus + Interference

Ebbinghaus forgetting curve:
    R(t) = e^(-t/S)

Where:
    R = retrievability (0-1, probability of successful recall)
    t = time since last access
    S = stability (determined by repetition, importance, memory type)

Stability grows with each successful retrieval (spacing effect):
    S_new = S_old * (1 + spacing_factor)

This models the well-known phenomenon that spaced repetition
(accessing a memory at increasing intervals) dramatically increases
its long-term stability.

Additionally: retrieval-induced forgetting — accessing one memory
actively suppresses competing memories (similar content, different answers).
"""

import math
import time
from typing import Optional
from engram.core import MemoryEntry, MemoryStore, MemoryType, DEFAULT_DECAY_RATES


def retrievability(entry: MemoryEntry, now: Optional[float] = None) -> float:
    """
    Ebbinghaus retrievability: R = e^(-t/S)

    Stability S is computed from:
    - Base decay rate (per memory type)
    - Number of accesses (spaced repetition effect)
    - Importance (emotional modulation)

    Returns 0-1 probability of successful retrieval.
    """
    now = now or time.time()

    # Time since last access (in days)
    last_access = max(entry.access_times) if entry.access_times else entry.created_at
    t_days = (now - last_access) / 86400

    if t_days <= 0:
        return 1.0

    # Compute stability S
    S = compute_stability(entry)

    return math.exp(-t_days / S)


def compute_stability(entry: MemoryEntry) -> float:
    """
    Compute memory stability S.

    Base stability comes from memory type.
    Each access multiplies stability (spacing effect).
    Importance further boosts stability.

    S = base_S * (1 + 0.5 * n_accesses) * (0.5 + importance)
    """
    # Base stability from memory type (in days)
    base_decay = DEFAULT_DECAY_RATES.get(entry.memory_type, 0.05)
    base_S = 1.0 / base_decay  # Invert: low decay → high stability

    # Spacing effect: each access increases stability
    n_accesses = len(entry.access_times)
    spacing_factor = 1.0 + 0.5 * math.log1p(n_accesses)  # Diminishing returns

    # Importance modulation
    importance_factor = 0.5 + entry.importance  # 0.5x to 1.5x

    # Consolidation bonus
    consolidation_factor = 1.0 + 0.2 * entry.consolidation_count

    return base_S * spacing_factor * importance_factor * consolidation_factor


def effective_strength(entry: MemoryEntry, now: Optional[float] = None) -> float:
    """
    Combined strength: Memory Chain trace strengths × Ebbinghaus retrievability.

    This is the final "how alive is this memory" score.
    """
    R = retrievability(entry, now=now)
    trace_strength = entry.working_strength + entry.core_strength

    return trace_strength * R


def should_forget(entry: MemoryEntry, threshold: float = 0.01,
                  now: Optional[float] = None) -> bool:
    """
    Should this memory be pruned?

    A memory is effectively forgotten when its combined strength
    drops below threshold — it's still "there" (in archive) but
    won't be retrieved unless specifically searched for.
    """
    if entry.pinned:
        return False

    return effective_strength(entry, now=now) < threshold


def prune_forgotten(store: MemoryStore, threshold: float = 0.01,
                    now: Optional[float] = None) -> list[MemoryEntry]:
    """
    Mark effectively-forgotten memories.

    Doesn't delete — moves to archive layer (like brain: information
    isn't truly lost, just becomes inaccessible).

    Returns list of pruned memories.
    """
    from engram.core import MemoryLayer

    pruned = []
    for entry in store.all():
        if should_forget(entry, threshold=threshold, now=now):
            if entry.layer != MemoryLayer.L4_ARCHIVE:
                entry.layer = MemoryLayer.L4_ARCHIVE
                pruned.append(entry)

    return pruned


def retrieval_induced_forgetting(store: MemoryStore, retrieved_entry: MemoryEntry,
                                 suppression_factor: float = 0.05):
    """
    Retrieval-induced forgetting: retrieving one memory
    suppresses competing memories.

    "Competing" = same memory type + similar content (simple heuristic).
    In a full implementation, this would use embedding similarity.
    """
    retrieved_words = set(retrieved_entry.content.lower().split())

    for entry in store.all():
        if entry.id == retrieved_entry.id:
            continue
        if entry.memory_type != retrieved_entry.memory_type:
            continue

        # Simple word overlap as competition measure
        entry_words = set(entry.content.lower().split())
        if not entry_words:
            continue

        overlap = len(retrieved_words & entry_words) / len(entry_words)

        if overlap > 0.3:  # >30% word overlap = competing memory
            # Suppress the competing memory slightly
            entry.working_strength *= (1 - suppression_factor * overlap)
