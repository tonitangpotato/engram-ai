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
) -> list[tuple[str, str]]:
    """
    Record co-activation for a set of memory IDs.
    
    When multiple memories are retrieved together (e.g., in a single recall),
    each pair gets their coactivation_count incremented. When the count
    reaches the threshold, a Hebbian link is automatically formed.
    
    Args:
        store: The SQLiteStore instance
        memory_ids: List of memory IDs that were co-activated
        threshold: Number of co-activations before link forms
        
    Returns:
        List of (id1, id2) tuples for newly formed links
    """
    if len(memory_ids) < 2:
        return []
    
    new_links = []
    
    for id1, id2 in combinations(memory_ids, 2):
        # Ensure consistent ordering (smaller ID first)
        if id1 > id2:
            id1, id2 = id2, id1
            
        formed = maybe_create_link(store, id1, id2, threshold)
        if formed:
            new_links.append((id1, id2))
    
    return new_links


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
