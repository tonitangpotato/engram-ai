"""
Memory Chain Consolidation Model (Murre & Chessa, 2011)

The brain's dual-system memory transfer, modeled as differential equations:

    dr₁/dt = -μ₁ · r₁(t)                     (hippocampal trace decays fast)
    dr₂/dt = α · r₁(t) - μ₂ · r₂(t)          (neocortical trace grows from hippocampal input, decays slowly)

Where:
    r₁(t) = working_strength (hippocampal / L3)
    r₂(t) = core_strength (neocortical / L2)
    μ₁ = fast decay rate (~0.1/day for working memory)
    μ₂ = slow decay rate (~0.005/day for core memory)
    α = consolidation rate (how fast working → core transfer happens)

During consolidation ("sleep replay"):
    1. Each memory's working_strength decays
    2. core_strength grows proportional to remaining working_strength
    3. Interleaved replay: old memories are also re-activated

This means:
    - Recent memories (high r₁) are actively being consolidated
    - Old memories (low r₁, high r₂) are stable in core
    - Memories that were never consolidated (low r₁, low r₂) are effectively forgotten
"""

import math
import time
from typing import Optional
from engram.core import MemoryEntry, MemoryStore, MemoryLayer, MemoryType


# Default parameters (per day)
MU1 = 0.15       # Working memory decay rate (fast — ~50% gone in 4.6 days)
MU2 = 0.005      # Core memory decay rate (slow — ~50% gone in 139 days)
ALPHA = 0.08     # Consolidation rate (transfer speed from working → core)


def apply_decay(entry: MemoryEntry, dt_days: float,
                mu1: float = MU1, mu2: float = MU2):
    """
    Apply time-based decay to both memory traces.

    r₁(t+dt) = r₁(t) · e^(-μ₁ · dt)
    r₂(t+dt) = r₂(t) · e^(-μ₂ · dt)
    """
    if entry.pinned:
        return  # Pinned memories don't decay

    entry.working_strength *= math.exp(-mu1 * dt_days)
    entry.core_strength *= math.exp(-mu2 * dt_days)


def consolidate_single(entry: MemoryEntry, dt_days: float = 1.0,
                       alpha: float = ALPHA, mu1: float = MU1, mu2: float = MU2):
    """
    Run one consolidation step for a single memory.

    This is the "sleep replay" — working trace transfers to core trace.

    dr₂ += α · r₁ · dt   (consolidation transfer)

    Then apply normal decay to both traces.

    Importance modulates consolidation rate (amygdala → hippocampus modulation):
    effective_alpha = alpha * (0.5 + importance)
    """
    if entry.pinned:
        return

    # Importance-modulated consolidation (emotional memories consolidate faster)
    # importance^2 makes low-importance memories consolidate much less
    effective_alpha = alpha * (0.2 + entry.importance ** 2)

    # Transfer from working to core
    transfer = effective_alpha * entry.working_strength * dt_days
    entry.core_strength += transfer

    # Apply decay
    apply_decay(entry, dt_days, mu1, mu2)

    # Update metadata
    entry.consolidation_count += 1
    entry.last_consolidated = time.time()


def run_consolidation_cycle(store: MemoryStore, dt_days: float = 1.0,
                            interleave_ratio: float = 0.3,
                            alpha: float = ALPHA):
    """
    Run a full consolidation cycle ("sleep").

    1. Consolidate all working (L3) memories
    2. Interleaved replay: also touch some archive (L4) memories
       (prevents catastrophic forgetting)
    3. Promote/demote memories between layers based on strength

    interleave_ratio: fraction of L4 memories to replay (0.3 = 30%)
    """
    import random

    all_memories = store.all()

    _update = getattr(store, 'update', None)

    # Step 1: Consolidate all L3 (working) memories
    working = [m for m in all_memories if m.layer == MemoryLayer.L3_WORKING]
    for entry in working:
        consolidate_single(entry, dt_days=dt_days, alpha=alpha)
        if _update:
            _update(entry)

    # Step 2: Interleaved replay of L4 (archive) memories
    # This is critical — prevents losing old knowledge when learning new things
    archive = [m for m in all_memories if m.layer == MemoryLayer.L4_ARCHIVE]
    if archive:
        n_replay = max(1, int(len(archive) * interleave_ratio))
        replay_sample = random.sample(archive, min(n_replay, len(archive)))
        for entry in replay_sample:
            # Replaying an archived memory slightly boosts its core_strength
            entry.core_strength += 0.01 * (0.5 + entry.importance)
            entry.consolidation_count += 1
            entry.last_consolidated = time.time()
            if _update:
                _update(entry)

    # Step 3: Also decay L2 (core) memories slightly
    core = [m for m in all_memories if m.layer == MemoryLayer.L2_CORE]
    for entry in core:
        apply_decay(entry, dt_days, mu1=0, mu2=MU2)  # No working decay for L2
        if _update:
            _update(entry)

    # Step 4: Layer promotion/demotion
    _rebalance_layers(store)


def _rebalance_layers(store: MemoryStore,
                      promote_threshold: float = 0.25,
                      demote_threshold: float = 0.05,
                      archive_threshold: float = 0.15):
    """
    Move memories between layers based on their strength.

    L3 → L2: core_strength > promote_threshold (consolidated enough)
    L2 → L4: total_strength < demote_threshold (fading from core)
    L3 → L4: working_strength < archive_threshold (expired from working)
    """
    _update = getattr(store, 'update', None)

    for entry in store.all():
        total = entry.working_strength + entry.core_strength
        old_layer = entry.layer

        if entry.pinned:
            entry.layer = MemoryLayer.L2_CORE
        elif entry.layer == MemoryLayer.L3_WORKING:
            if entry.core_strength >= promote_threshold:
                entry.layer = MemoryLayer.L2_CORE
            elif entry.working_strength < archive_threshold and entry.core_strength < archive_threshold:
                entry.layer = MemoryLayer.L4_ARCHIVE
        elif entry.layer == MemoryLayer.L2_CORE:
            if total < demote_threshold and not entry.pinned:
                entry.layer = MemoryLayer.L4_ARCHIVE

        if _update and entry.layer != old_layer:
            _update(entry)


def get_consolidation_stats(store: MemoryStore) -> dict:
    """Summary stats for the memory system."""
    all_mem = store.all()
    by_layer = {}
    for layer in MemoryLayer:
        entries = [m for m in all_mem if m.layer == layer]
        by_layer[layer.value] = {
            "count": len(entries),
            "avg_working": sum(m.working_strength for m in entries) / max(len(entries), 1),
            "avg_core": sum(m.core_strength for m in entries) / max(len(entries), 1),
            "avg_importance": sum(m.importance for m in entries) / max(len(entries), 1),
        }

    return {
        "total_memories": len(all_mem),
        "layers": by_layer,
        "pinned": sum(1 for m in all_mem if m.pinned),
    }
