"""
Session Working Memory — Cognitive science-based session-level memory management.

Based on:
- Miller's Law: 7±2 chunks capacity
- Baddeley's Working Memory Model: 5-minute decay
- ACT-R: Spreading activation from context
- Hebbian Learning: Associative neighborhood checking

The core insight: Instead of deciding "should I recall?" per message,
maintain a working memory state and only recall when the topic changes.
"""

import time
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from engram.memory import Memory


class SessionWorkingMemory:
    """
    Simulates cognitive science working memory — limited capacity + time decay.
    
    Used to intelligently decide when to trigger full memory recall vs.
    reusing already-active memories. Reduces API calls by 70-80% for
    continuous conversation topics.
    
    Usage:
        swm = SessionWorkingMemory()
        
        # On each message:
        if swm.needs_recall(message, engram):
            results = engram.recall(message)
            swm.activate([r['id'] for r in results])
        else:
            # Reuse working memory context
            results = swm.get_active_memories(engram)
    """
    
    def __init__(self, capacity: int = 7, decay_seconds: float = 300.0):
        """
        Initialize working memory.
        
        Args:
            capacity: Maximum number of active memory chunks (Miller's Law: 7±2)
            decay_seconds: Time until a memory fades from working memory (default: 5 min)
        """
        self.capacity = capacity
        self.decay_seconds = decay_seconds
        self.items: dict[str, float] = {}  # memory_id -> last_activated timestamp
    
    def activate(self, memory_ids: list[str]) -> None:
        """
        Activate memories (bring into working memory).
        
        Called after a recall — the retrieved memories become active in WM.
        If capacity is exceeded, oldest items are pruned.
        
        Args:
            memory_ids: List of memory IDs to activate
        """
        now = time.time()
        for mid in memory_ids:
            self.items[mid] = now
        self._prune()
    
    def _prune(self) -> None:
        """
        Prune working memory:
        1. Remove decayed items (older than decay_seconds)
        2. If still over capacity, keep only the most recent
        """
        now = time.time()
        
        # Time decay — remove items older than decay_seconds
        self.items = {
            k: v for k, v in self.items.items()
            if now - v < self.decay_seconds
        }
        
        # Capacity limit — keep only the most recently activated
        if len(self.items) > self.capacity:
            sorted_items = sorted(self.items.items(), key=lambda x: -x[1])  # Most recent first
            self.items = dict(sorted_items[:self.capacity])
    
    def get_active_ids(self) -> list[str]:
        """
        Get currently active memory IDs (after pruning).
        
        Returns:
            List of active memory IDs
        """
        self._prune()
        return list(self.items.keys())
    
    def get_active_memories(self, engram: "Memory") -> list[dict]:
        """
        Get full memory objects for active working memory items.
        
        Args:
            engram: Memory instance to fetch from
            
        Returns:
            List of memory dicts (same format as recall())
        """
        self._prune()
        results = []
        for mid in self.items.keys():
            entry = engram._store.get(mid)
            if entry:
                from engram.forgetting import effective_strength
                from engram.confidence import confidence_label
                
                now = time.time()
                strength = effective_strength(entry, now)
                conf = min(1.0, strength * 1.2)  # Approximate confidence
                
                results.append({
                    "id": entry.id,
                    "content": entry.content,
                    "type": entry.memory_type.value,
                    "confidence": round(conf, 3),
                    "confidence_label": confidence_label(conf),
                    "strength": round(strength, 3),
                    "age_days": round(entry.age_days(), 1),
                    "layer": entry.layer.value,
                    "importance": round(entry.importance, 2),
                    "pinned": entry.pinned,
                    "source": entry.source_file,
                    "_from_wm": True,  # Flag indicating this came from working memory cache
                })
        return results
    
    def needs_recall(self, message: str, engram: "Memory") -> bool:
        """
        Determine if a full recall is needed, or if working memory suffices.
        
        Logic:
        1. If working memory is empty → need recall
        2. Do a lightweight probe (limit=3 cheap recall)
        3. Check if probe results overlap with:
           - Current working memory IDs
           - Hebbian neighbors of working memory IDs
        4. If ≥60% overlap → topic is continuous, skip full recall
        5. Otherwise → topic changed, do full recall
        
        Args:
            message: The new user message
            engram: Memory instance for probe and Hebbian link lookup
            
        Returns:
            True if full recall is needed, False if working memory suffices
        """
        self._prune()
        
        # Empty working memory → always recall
        if not self.items:
            return True
        
        current_ids = set(self.items.keys())
        
        # Collect Hebbian neighbors of current working memory
        neighbors = set()
        for mid in current_ids:
            links = engram.hebbian_links(mid)
            for source_id, target_id, strength in links:
                neighbors.add(target_id)
        
        # Lightweight probe — just 3 results to check topic continuity
        probe = engram.recall(message, limit=3, graph_expand=False)
        if not probe:
            return True  # No results → need full recall
        
        probe_ids = {r["id"] for r in probe}
        
        # Check overlap with current WM + Hebbian neighborhood
        overlap = probe_ids & (current_ids | neighbors)
        overlap_ratio = len(overlap) / len(probe_ids) if probe_ids else 0
        
        # ≥60% overlap → topic is continuous
        if overlap_ratio >= 0.6:
            return False
        
        return True
    
    def is_empty(self) -> bool:
        """Check if working memory is currently empty (after pruning)."""
        self._prune()
        return len(self.items) == 0
    
    def size(self) -> int:
        """Get current working memory size (after pruning)."""
        self._prune()
        return len(self.items)
    
    def clear(self) -> None:
        """Clear all items from working memory."""
        self.items = {}
    
    def __repr__(self) -> str:
        self._prune()
        return f"SessionWorkingMemory(size={len(self.items)}/{self.capacity})"
    
    def __len__(self) -> int:
        self._prune()
        return len(self.items)


# Session registry for MCP server — maps session_id to SessionWorkingMemory
_session_registry: dict[str, SessionWorkingMemory] = {}


def get_session_wm(session_id: str = "default") -> SessionWorkingMemory:
    """
    Get or create a SessionWorkingMemory for a given session ID.
    
    Used by MCP server to maintain per-session working memory state.
    
    Args:
        session_id: Unique session identifier (e.g., conversation ID)
        
    Returns:
        SessionWorkingMemory instance for this session
    """
    if session_id not in _session_registry:
        _session_registry[session_id] = SessionWorkingMemory()
    return _session_registry[session_id]


def clear_session(session_id: str) -> bool:
    """
    Clear and remove a session's working memory.
    
    Args:
        session_id: Session to clear
        
    Returns:
        True if session existed and was cleared, False otherwise
    """
    if session_id in _session_registry:
        del _session_registry[session_id]
        return True
    return False


def list_sessions() -> list[str]:
    """
    List all active session IDs.
    
    Returns:
        List of session IDs with active working memory
    """
    return list(_session_registry.keys())
