"""
ACT-R Activation-Based Retrieval

The core equation from Anderson's ACT-R theory:
    A_i = B_i + Σ(W_j · S_ji) + ε

Where:
    B_i = base-level activation (frequency × recency)
    Σ(W_j · S_ji) = spreading activation from current context
    ε = noise (stochastic retrieval)

Base-level activation (power law of practice and recency):
    B_i = ln(Σ_k  t_k^(-d))

Where t_k = time since k-th access, d = decay parameter (~0.5)

This naturally implements:
- Recency: recent accesses boost activation
- Frequency: more accesses → higher base activation
- Graceful forgetting: unused memories become unretrievable
"""

import math
import time
from typing import Optional
from engram.core import MemoryEntry, MemoryStore


def base_level_activation(entry: MemoryEntry, now: Optional[float] = None,
                          decay: float = 0.5) -> float:
    """
    ACT-R base-level activation.

    B_i = ln(Σ_k (now - t_k)^(-d))

    Higher when accessed more often and more recently.
    Returns -inf if no accesses (unretrievable).
    """
    now = now or time.time()

    if not entry.access_times:
        return float("-inf")

    total = 0.0
    for t_k in entry.access_times:
        age = now - t_k
        if age <= 0:
            age = 0.001  # Avoid division by zero for very recent
        total += age ** (-decay)

    if total <= 0:
        return float("-inf")

    return math.log(total)


def spreading_activation(entry: MemoryEntry, context_keywords: list[str],
                         weight: float = 1.0) -> float:
    """
    Simple spreading activation from current context.

    In full ACT-R, this uses semantic similarity between context elements
    and memory chunks. Here we use keyword overlap as a proxy.

    Σ(W_j · S_ji) ≈ weight × (overlap / total_keywords)
    """
    if not context_keywords:
        return 0.0

    content_lower = entry.content.lower()
    matches = sum(1 for kw in context_keywords if kw.lower() in content_lower)

    return weight * (matches / len(context_keywords))


def retrieval_activation(entry: MemoryEntry, context_keywords: list[str] = None,
                         now: Optional[float] = None,
                         base_decay: float = 0.5,
                         context_weight: float = 1.5,
                         importance_weight: float = 0.5) -> float:
    """
    Full retrieval activation score.

    A_i = B_i + context_match + importance_boost

    Combines ACT-R base-level with context spreading activation
    and emotional/importance modulation.
    """
    base = base_level_activation(entry, now=now, decay=base_decay)

    if base == float("-inf"):
        return float("-inf")

    context = 0.0
    if context_keywords:
        context = spreading_activation(entry, context_keywords, weight=context_weight)

    # Importance modulation (amygdala analog)
    importance_boost = entry.importance * importance_weight

    return base + context + importance_boost


def retrieve_top_k(store: MemoryStore, context_keywords: list[str] = None,
                   k: int = 5, now: Optional[float] = None,
                   min_activation: float = -10.0) -> list[tuple[MemoryEntry, float]]:
    """
    Retrieve top-k memories by activation score.

    This is the main retrieval function — replaces simple cosine similarity
    with a neuroscience-grounded scoring function.
    """
    scored = []
    for entry in store.all():
        score = retrieval_activation(entry, context_keywords=context_keywords, now=now)
        if score > min_activation:
            scored.append((entry, score))

    scored.sort(key=lambda x: x[1], reverse=True)
    return scored[:k]
