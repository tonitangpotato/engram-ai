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
from engram.config import MemoryConfig


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
                            alpha: float = ALPHA,
                            mu1: float = MU1, mu2: float = MU2,
                            replay_boost: float = 0.01,
                            promote_threshold: float = 0.25,
                            demote_threshold: float = 0.05,
                            archive_threshold: float = 0.15):
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
        consolidate_single(entry, dt_days=dt_days, alpha=alpha, mu1=mu1, mu2=mu2)
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
            entry.core_strength += replay_boost * (0.5 + entry.importance)
            entry.consolidation_count += 1
            entry.last_consolidated = time.time()
            if _update:
                _update(entry)

    # Step 3: Also decay L2 (core) memories slightly
    core = [m for m in all_memories if m.layer == MemoryLayer.L2_CORE]
    for entry in core:
        apply_decay(entry, dt_days, mu1=0, mu2=mu2)  # No working decay for L2
        if _update:
            _update(entry)

    # Step 4: Layer promotion/demotion
    _rebalance_layers(store, promote_threshold=promote_threshold,
                      demote_threshold=demote_threshold,
                      archive_threshold=archive_threshold)


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


def consolidate_causal(store, config: Optional[MemoryConfig] = None):
    """
    STDP-based causal memory creation during consolidation.
    
    Checks Hebbian links for strong temporal signals. When memory A
    consistently precedes memory B in co-activation patterns, creates
    a type=causal memory capturing the inferred A→B relationship.
    
    This bridges correlation (Hebbian) with causation (STDP) —
    temporal ordering provides weak-but-useful causal evidence.
    
    Args:
        store: SQLiteStore instance (must have STDP columns migrated)
        config: MemoryConfig with STDP parameters. None = defaults.
    """
    if config is None:
        config = MemoryConfig.default()
    
    if not config.stdp_enabled:
        return
    
    from engram.hebbian import get_stdp_candidates, update_link_direction
    
    candidates = get_stdp_candidates(
        store,
        causal_threshold=config.stdp_causal_threshold,
        min_observations=config.stdp_min_observations,
    )
    
    for c in candidates:
        cause_id = c["cause_id"]
        effect_id = c["effect_id"]
        confidence = c["confidence"]
        
        # Fetch the actual memory content
        cause_row = store._conn.execute(
            "SELECT content FROM memories WHERE id=?", (cause_id,)
        ).fetchone()
        effect_row = store._conn.execute(
            "SELECT content FROM memories WHERE id=?", (effect_id,)
        ).fetchone()
        
        if not cause_row or not effect_row:
            continue
        
        cause_content = cause_row[0][:100]
        effect_content = effect_row[0][:100]
        
        # Check if we already created a causal memory for this pair
        # (avoid duplicates across multiple consolidation cycles)
        import json
        existing = store._conn.execute(
            """SELECT id FROM memories 
               WHERE memory_type = 'causal' AND metadata IS NOT NULL""",
        ).fetchall()
        
        already_exists = False
        for row in existing:
            mid = row[0]
            meta_row = store._conn.execute(
                "SELECT metadata FROM memories WHERE id=?", (mid,)
            ).fetchone()
            if meta_row and meta_row[0]:
                try:
                    meta = json.loads(meta_row[0])
                    if meta.get("cause_id") == cause_id and meta.get("effect_id") == effect_id:
                        already_exists = True
                        # Update confidence if it changed
                        if abs(meta.get("confidence", 0) - confidence) > 0.05:
                            meta["confidence"] = confidence
                            meta["observations"] = c["temporal_forward"] + c["temporal_backward"]
                            store._conn.execute(
                                "UPDATE memories SET metadata=? WHERE id=?",
                                (json.dumps(meta), mid)
                            )
                            store._conn.commit()
                        break
                except (json.JSONDecodeError, TypeError):
                    continue
        
        if already_exists:
            continue
        
        # Create the causal memory
        causal_content = f"CAUSAL: {cause_content} → {effect_content}"
        metadata = {
            "cause_id": cause_id,
            "effect_id": effect_id,
            "cause": cause_content,
            "effect": effect_content,
            "confidence": confidence,
            "observations": c["temporal_forward"] + c["temporal_backward"],
            "temporal_forward": c["temporal_forward"],
            "temporal_backward": c["temporal_backward"],
        }
        
        # Use link strength as importance basis, scaled by confidence
        importance = min(1.0, c["strength"] * confidence)
        
        store.add(
            content=causal_content,
            memory_type=MemoryType.CAUSAL,
            importance=importance,
            source_file="stdp:auto",
            metadata=metadata,
        )
        
        # Update the Hebbian link direction
        update_link_direction(
            store, c["source_id"], c["target_id"], c["direction"]
        )


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
