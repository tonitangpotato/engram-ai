"""
Hebbian Learning — Co-activation forms memory links

"Neurons that fire together, wire together."

When memories are recalled together repeatedly, they form Hebbian links.
These links create an associative network independent of explicit entity
tagging — purely emergent from usage patterns.

Key insight: This captures implicit relationships that the agent discovers
through experience, not explicit knowledge stored at encoding time.
"""

import time
from itertools import combinations
from typing import Optional

from engram.store import SQLiteStore


def record_coactivation(
    store: SQLiteStore,
    memory_ids: list[str],
    threshold: int = 3,
    stdp_enabled: bool = True,
) -> list[tuple[str, str]]:
    """
    Record co-activation for a set of memory IDs.
    
    When multiple memories are retrieved together (e.g., in a single recall),
    each pair gets their coactivation_count incremented. When the count
    reaches the threshold, a Hebbian link is automatically formed.
    
    If STDP is enabled, also tracks temporal ordering (which memory was
    created first) to infer causal direction.
    
    Args:
        store: The SQLiteStore instance
        memory_ids: List of memory IDs that were co-activated
        threshold: Number of co-activations before link forms
        stdp_enabled: Whether to track temporal ordering for STDP
        
    Returns:
        List of (id1, id2) tuples for newly formed links
    """
    if len(memory_ids) < 2:
        return []
    
    # Pre-fetch created_at timestamps if STDP is enabled
    timestamps: dict[str, float] = {}
    if stdp_enabled:
        for mid in memory_ids:
            row = store._conn.execute(
                "SELECT created_at FROM memories WHERE id=?", (mid,)
            ).fetchone()
            if row:
                timestamps[mid] = row[0]
    
    new_links = []
    
    for id1, id2 in combinations(memory_ids, 2):
        # Ensure consistent ordering (smaller ID first)
        if id1 > id2:
            id1, id2 = id2, id1
            
        formed = maybe_create_link(store, id1, id2, threshold)
        if formed:
            new_links.append((id1, id2))
        
        # STDP: track temporal ordering
        if stdp_enabled and id1 in timestamps and id2 in timestamps:
            _update_temporal_counts(store, id1, id2, timestamps[id1], timestamps[id2])
    
    return new_links


def _update_temporal_counts(
    store: SQLiteStore,
    id1: str,
    id2: str,
    ts1: float,
    ts2: float,
):
    """
    Update temporal forward/backward counts for a Hebbian link pair.
    
    With consistent ordering (id1 < id2):
    - temporal_forward: id1 was created before id2 (id1 → id2 direction)
    - temporal_backward: id2 was created before id1 (id2 → id1 direction)
    
    Args:
        store: SQLiteStore instance
        id1: First memory ID (should be <= id2)
        id2: Second memory ID
        ts1: created_at timestamp for id1
        ts2: created_at timestamp for id2
    """
    if ts1 == ts2:
        return  # Simultaneous — no temporal signal
    
    conn = store._conn
    
    # Check if the row exists (it should, since maybe_create_link runs first)
    existing = conn.execute(
        "SELECT temporal_forward, temporal_backward FROM hebbian_links WHERE source_id=? AND target_id=?",
        (id1, id2)
    ).fetchone()
    
    if existing is None:
        return  # No link record yet — will be created on next co-activation
    
    if ts1 < ts2:
        # id1 was created before id2 → forward direction
        conn.execute(
            "UPDATE hebbian_links SET temporal_forward = temporal_forward + 1 WHERE source_id=? AND target_id=?",
            (id1, id2)
        )
    else:
        # id2 was created before id1 → backward direction
        conn.execute(
            "UPDATE hebbian_links SET temporal_backward = temporal_backward + 1 WHERE source_id=? AND target_id=?",
            (id1, id2)
        )
    conn.commit()


def maybe_create_link(
    store: SQLiteStore,
    id1: str,
    id2: str,
    threshold: int = 3,
) -> bool:
    """
    Increment coactivation count and create link if threshold is met.
    
    Uses UPSERT to atomically increment the counter. When threshold is
    reached for the first time, creates the bidirectional link.
    
    Args:
        store: The SQLiteStore instance
        id1: First memory ID (should be <= id2 for consistency)
        id2: Second memory ID
        threshold: Activation count needed to form link
        
    Returns:
        True if a new link was formed on this call
    """
    conn = store._conn
    
    # Ensure consistent ordering
    if id1 > id2:
        id1, id2 = id2, id1
    
    # Check if link already exists with strength > 0 (already formed)
    existing = conn.execute(
        "SELECT strength, coactivation_count FROM hebbian_links WHERE source_id=? AND target_id=?",
        (id1, id2)
    ).fetchone()
    
    if existing and existing[0] > 0:
        # Link already exists - strengthen it! (Hebbian: "use it or lose it")
        # This counteracts decay and keeps active associations strong
        current_strength = existing[0]
        # Boost by 10%, capped at 1.0
        new_strength = min(1.0, current_strength + 0.1)
        conn.execute(
            """UPDATE hebbian_links 
               SET coactivation_count = coactivation_count + 1,
                   strength = ?
               WHERE source_id=? AND target_id=?""",
            (new_strength, id1, id2)
        )
        # Also strengthen the reverse link
        conn.execute(
            """UPDATE hebbian_links 
               SET coactivation_count = coactivation_count + 1,
                   strength = ?
               WHERE source_id=? AND target_id=?""",
            (new_strength, id2, id1)
        )
        conn.commit()
        return False
    
    if existing:
        # Record exists but strength=0 (tracking phase), increment count
        new_count = existing[1] + 1
        if new_count >= threshold:
            # Threshold reached! Create bidirectional link
            now = time.time()
            conn.execute(
                """UPDATE hebbian_links 
                   SET strength = 1.0, coactivation_count = ? 
                   WHERE source_id=? AND target_id=?""",
                (new_count, id1, id2)
            )
            # Create reverse link
            conn.execute(
                """INSERT OR REPLACE INTO hebbian_links 
                   (source_id, target_id, strength, coactivation_count, created_at)
                   VALUES (?, ?, 1.0, ?, ?)""",
                (id2, id1, new_count, now)
            )
            conn.commit()
            return True
        else:
            conn.execute(
                """UPDATE hebbian_links 
                   SET coactivation_count = ? 
                   WHERE source_id=? AND target_id=?""",
                (new_count, id1, id2)
            )
            conn.commit()
            return False
    else:
        # First co-activation, create tracking record with strength=0
        # Use INSERT OR IGNORE to handle race conditions in concurrent access
        now = time.time()
        conn.execute(
            """INSERT OR IGNORE INTO hebbian_links 
               (source_id, target_id, strength, coactivation_count, created_at)
               VALUES (?, ?, 0.0, 1, ?)""",
            (id1, id2, now)
        )
        conn.commit()
        return False


def get_hebbian_neighbors(store: SQLiteStore, memory_id: str) -> list[str]:
    """
    Get all memories linked to this one via Hebbian connections.
    
    Only returns neighbors with positive link strength (formed links,
    not just tracked co-activations).
    
    Args:
        store: The SQLiteStore instance
        memory_id: Memory ID to find neighbors for
        
    Returns:
        List of connected memory IDs
    """
    rows = store._conn.execute(
        """SELECT target_id FROM hebbian_links 
           WHERE source_id = ? AND strength > 0""",
        (memory_id,)
    ).fetchall()
    return [r[0] for r in rows]


def get_all_hebbian_links(store: SQLiteStore) -> list[tuple[str, str, float]]:
    """
    Get all formed Hebbian links (strength > 0).
    
    Returns:
        List of (source_id, target_id, strength) tuples
    """
    rows = store._conn.execute(
        """SELECT source_id, target_id, strength FROM hebbian_links 
           WHERE strength > 0"""
    ).fetchall()
    return [(r[0], r[1], r[2]) for r in rows]


def decay_hebbian_links(store: SQLiteStore, factor: float = 0.95) -> int:
    """
    Decay all Hebbian link strengths by a factor.
    
    Called during consolidation to gradually weaken unused links.
    Links that decay below a threshold (0.1) are removed.
    
    Args:
        store: The SQLiteStore instance
        factor: Multiplicative decay factor (0.95 = 5% decay)
        
    Returns:
        Number of links pruned
    """
    conn = store._conn
    
    # Decay all link strengths
    conn.execute(
        "UPDATE hebbian_links SET strength = strength * ? WHERE strength > 0",
        (factor,)
    )
    
    # Prune very weak links (below 0.1)
    result = conn.execute(
        "DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1"
    )
    pruned = result.rowcount
    
    conn.commit()
    return pruned


def strengthen_link(store: SQLiteStore, id1: str, id2: str, boost: float = 0.1) -> bool:
    """
    Strengthen an existing Hebbian link.
    
    Called when linked memories are accessed together again.
    Caps strength at 2.0 to prevent unbounded growth.
    
    Args:
        store: The SQLiteStore instance
        id1: First memory ID
        id2: Second memory ID  
        boost: Amount to add to strength
        
    Returns:
        True if link existed and was strengthened
    """
    conn = store._conn
    
    # Update both directions
    for src, tgt in [(id1, id2), (id2, id1)]:
        conn.execute(
            """UPDATE hebbian_links 
               SET strength = MIN(2.0, strength + ?) 
               WHERE source_id = ? AND target_id = ? AND strength > 0""",
            (boost, src, tgt)
        )
    
    conn.commit()
    return conn.total_changes > 0


def get_coactivation_stats(store: SQLiteStore) -> dict[tuple[str, str], int]:
    """
    Get co-activation counts for all tracked pairs.
    
    Includes both formed links (strength > 0) and tracking records
    (strength = 0, not yet at threshold).
    
    Returns:
        Dict mapping (id1, id2) to coactivation_count
    """
    rows = store._conn.execute(
        "SELECT source_id, target_id, coactivation_count FROM hebbian_links"
    ).fetchall()
    return {(r[0], r[1]): r[2] for r in rows}


def get_stdp_candidates(
    store: SQLiteStore,
    causal_threshold: float = 2.0,
    min_observations: int = 3,
) -> list[dict]:
    """
    Find Hebbian links with strong temporal signal for causal inference.
    
    Returns links where temporal_forward > temporal_backward * causal_threshold
    or vice versa, indicating a consistent temporal ordering (potential causation).
    
    Args:
        store: SQLiteStore instance
        causal_threshold: Ratio threshold (forward/backward) to consider causal
        min_observations: Minimum total temporal observations required
        
    Returns:
        List of dicts with keys:
            source_id, target_id, strength, temporal_forward, temporal_backward,
            direction ('forward' or 'backward'), confidence, cause_id, effect_id
    """
    rows = store._conn.execute(
        """SELECT source_id, target_id, strength, temporal_forward, temporal_backward
           FROM hebbian_links 
           WHERE strength > 0 
             AND (temporal_forward + temporal_backward) >= ?""",
        (min_observations,)
    ).fetchall()
    
    candidates = []
    for r in rows:
        source_id = r[0]
        target_id = r[1]
        strength = r[2]
        fwd = r[3] or 0
        bwd = r[4] or 0
        total = fwd + bwd
        
        if total < min_observations:
            continue
        
        # Check forward direction: source was created before target
        if bwd == 0 and fwd >= min_observations:
            # Pure forward — strong causal signal
            candidates.append({
                "source_id": source_id,
                "target_id": target_id,
                "strength": strength,
                "temporal_forward": fwd,
                "temporal_backward": bwd,
                "direction": "forward",
                "confidence": 1.0,
                "cause_id": source_id,
                "effect_id": target_id,
            })
        elif fwd == 0 and bwd >= min_observations:
            # Pure backward — reverse causal signal
            candidates.append({
                "source_id": source_id,
                "target_id": target_id,
                "strength": strength,
                "temporal_forward": fwd,
                "temporal_backward": bwd,
                "direction": "backward",
                "confidence": 1.0,
                "cause_id": target_id,
                "effect_id": source_id,
            })
        elif fwd > bwd * causal_threshold:
            # Forward dominant
            candidates.append({
                "source_id": source_id,
                "target_id": target_id,
                "strength": strength,
                "temporal_forward": fwd,
                "temporal_backward": bwd,
                "direction": "forward",
                "confidence": round(fwd / total, 3),
                "cause_id": source_id,
                "effect_id": target_id,
            })
        elif bwd > fwd * causal_threshold:
            # Backward dominant
            candidates.append({
                "source_id": source_id,
                "target_id": target_id,
                "strength": strength,
                "temporal_forward": fwd,
                "temporal_backward": bwd,
                "direction": "backward",
                "confidence": round(bwd / total, 3),
                "cause_id": target_id,
                "effect_id": source_id,
            })
    
    return candidates


def update_link_direction(store: SQLiteStore, source_id: str, target_id: str, direction: str):
    """
    Update the direction field of a Hebbian link.
    
    Args:
        store: SQLiteStore instance
        source_id: Source memory ID
        target_id: Target memory ID  
        direction: One of 'bidirectional', 'forward', 'backward'
    """
    store._conn.execute(
        "UPDATE hebbian_links SET direction = ? WHERE source_id = ? AND target_id = ?",
        (direction, source_id, target_id)
    )
    store._conn.commit()
