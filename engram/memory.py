"""
Engram Memory — Public API

The unified interface to the neuroscience-grounded memory system.
Designed for simplicity (like Mem0) while backed by mathematical models
from cognitive science.

Architecture:
    Memory (this class)
    ├── MemoryStore (backend — swappable to SQLiteStore)
    ├── activation.py (ACT-R retrieval)
    ├── consolidation.py (Memory Chain Model)
    ├── forgetting.py (Ebbinghaus + interference)
    ├── confidence.py (metacognitive scoring)
    ├── reward.py (dopaminergic feedback)
    ├── downscaling.py (homeostatic plasticity)
    └── anomaly.py (predictive coding)

Usage:
    from engram import Memory

    mem = Memory("./agent.db")
    mem.add("potato prefers action over discussion", type="relational", importance=0.6)
    mem.add("Use www.moltbook.com not moltbook.com", type="procedural", importance=0.8)

    results = mem.recall("what does potato prefer?", limit=5)
    for r in results:
        print(f"[{r['confidence_label']}] {r['content']}")

    mem.consolidate()  # Run "sleep" cycle
    mem.reward("good job!")  # Positive feedback strengthens recent memories
    mem.stats()
"""

import sys
import os
import time


from engram.core import MemoryEntry, MemoryStore, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE
from engram.store import SQLiteStore
from engram.activation import retrieve_top_k
from engram.search import SearchEngine
from engram.consolidation import run_consolidation_cycle, get_consolidation_stats
from engram.forgetting import effective_strength, should_forget, prune_forgotten
from engram.confidence import confidence_score, confidence_label
from engram.reward import detect_feedback, apply_reward
from engram.downscaling import synaptic_downscale
from engram.anomaly import BaselineTracker


# Map string type names to MemoryType enum
_TYPE_MAP = {t.value: t for t in MemoryType}


class Memory:
    """
    Main interface to the Engram memory system.

    Wraps the neuroscience math models behind a clean API.
    All complexity is hidden — you just add, recall, and consolidate.

    Backend: SQLiteStore for persistent storage with FTS5 search.
    """

    def __init__(self, path: str = "./engram.db"):
        """
        Initialize Engram memory system.

        Args:
            path: Path to SQLite database file. Created if it doesn't exist.
                  Use ":memory:" for in-memory (non-persistent) operation.
        """
        self.path = path
        self._store = SQLiteStore(path)
        self._tracker = BaselineTracker(window_size=100)
        self._created_at = time.time()

    def add(self, content: str, type: str = "factual", importance: float = None,
            source: str = "", tags: list[str] = None,
            entities: list = None) -> str:
        """
        Store a new memory. Returns memory ID.

        The memory is encoded with initial working_strength=1.0 (strong
        hippocampal trace) and core_strength=0.0 (no neocortical trace yet).
        Consolidation cycles will gradually transfer it to core.

        Importance modulates encoding strength — high importance memories
        (emotional, relational) are encoded more strongly, mimicking the
        amygdala's role in memory modulation.

        Args:
            content: The memory content (natural language)
            type: Memory type — one of: factual, episodic, relational,
                  emotional, procedural, opinion
            importance: 0-1 importance score (None = auto from type)
            source: Source identifier (e.g., filename, conversation ID)
            tags: Optional tags for categorization (stored in content for now)

        Returns:
            Memory ID string (8-char UUID prefix)
        """
        memory_type = _TYPE_MAP.get(type, MemoryType.FACTUAL)

        # If tags provided, append to content for searchability
        actual_content = content
        if tags:
            actual_content = f"{content} [tags: {', '.join(tags)}]"

        entry = self._store.add(
            content=actual_content,
            memory_type=memory_type,
            importance=importance,
            source_file=source,
        )

        # Store graph links if entities provided
        if entities:
            for ent in entities:
                if isinstance(ent, (list, tuple)):
                    entity, relation = ent[0], ent[1] if len(ent) > 1 else ""
                else:
                    entity, relation = ent, ""
                self._store.add_graph_link(entry.id, entity, relation)

        # Track encoding rate for anomaly detection
        self._tracker.update("encoding_rate", 1.0)

        return entry.id

    def recall(self, query: str, limit: int = 5,
               context: list[str] = None,
               types: list[str] = None,
               min_confidence: float = 0.0,
               graph_expand: bool = True) -> list[dict]:
        """
        Retrieve relevant memories using ACT-R activation-based retrieval.

        Unlike simple cosine similarity, this uses:
        - Base-level activation (frequency × recency, power law)
        - Spreading activation from context keywords
        - Importance modulation (emotional memories are more accessible)

        Results include a confidence score (metacognitive monitoring)
        that tells you how "trustworthy" each retrieval is.

        Args:
            query: Natural language query
            limit: Maximum number of results
            context: Additional context keywords to boost relevant memories
            types: Filter by memory types (e.g., ["factual", "procedural"])
            min_confidence: Minimum confidence threshold (0-1)

        Returns:
            List of dicts: {id, content, type, confidence, confidence_label,
                           strength, age_days, layer, importance}
        """
        engine = SearchEngine(self._store)
        search_results = engine.search(
            query=query,
            limit=limit,
            context_keywords=context,
            types=types,
            min_confidence=min_confidence,
            graph_expand=graph_expand,
        )

        output = []
        for r in search_results:
            output.append({
                "id": r.entry.id,
                "content": r.entry.content,
                "type": r.entry.memory_type.value,
                "confidence": round(r.confidence, 3),
                "confidence_label": r.confidence_label,
                "strength": round(effective_strength(r.entry, now=time.time()), 3),
                "activation": round(r.score, 3),
                "age_days": round(r.entry.age_days(), 1),
                "layer": r.entry.layer.value,
                "importance": round(r.entry.importance, 2),
            })

        # Track retrieval for anomaly detection
        self._tracker.update("retrieval_count", len(output))

        return output

    def consolidate(self, days: float = 1.0):
        """
        Run a consolidation cycle ("sleep replay").

        This is the core of memory maintenance. Based on Murre & Chessa's
        Memory Chain Model, it:

        1. Decays working_strength (hippocampal traces fade)
        2. Transfers knowledge to core_strength (neocortical consolidation)
        3. Replays archived memories (prevents catastrophic forgetting)
        4. Rebalances layers (promote strong → core, demote weak → archive)

        Call this periodically — once per "day" of agent operation,
        or after significant learning sessions.

        Also runs synaptic downscaling to prevent unbounded strength growth.

        Args:
            days: Simulated time step in days (1.0 = one day of consolidation)
        """
        run_consolidation_cycle(self._store, dt_days=days)
        synaptic_downscale(self._store, factor=0.95)

    def forget(self, memory_id: str = None, threshold: float = 0.01):
        """
        Forget a specific memory or prune all below threshold.

        If memory_id is given, removes that specific memory.
        Otherwise, prunes all memories whose effective_strength
        is below threshold (moves them to archive).

        This mirrors natural forgetting — memories aren't truly deleted,
        they become inaccessible (archived). They could theoretically
        be recovered with the right retrieval cue.

        Args:
            memory_id: Specific memory to forget (None = prune all weak)
            threshold: Strength threshold for pruning (default 0.01)
        """
        if memory_id is not None:
            self._store.delete(memory_id)
        else:
            prune_forgotten(self._store, threshold=threshold)

    def reward(self, feedback: str, recent_n: int = 3):
        """
        Process user feedback as a dopaminergic reward signal.

        Detects positive/negative sentiment in the feedback text,
        then applies reward modulation to the N most recently
        accessed memories. This shapes future behavior:
        - Positive → memories consolidate faster, more retrievable
        - Negative → memories suppressed, less likely to influence output

        Args:
            feedback: Natural language feedback from user
            recent_n: Number of recent memories to affect
        """
        polarity, conf = detect_feedback(feedback)

        if polarity == "neutral" or conf < 0.3:
            return  # Not confident enough to act

        apply_reward(self._store, polarity, recent_n=recent_n,
                     reward_magnitude=0.15 * conf)

    def downscale(self, factor: float = 0.95):
        """
        Global synaptic downscaling — normalize all memory weights.

        Based on Tononi & Cirelli's Synaptic Homeostasis Hypothesis:
        during sleep, all synaptic weights are proportionally reduced.
        This prevents unbounded growth and maintains discriminability.

        Args:
            factor: Multiplicative factor (0-1). Default 0.95 = 5% reduction.

        Returns:
            Stats dict: {n_scaled, avg_before, avg_after}
        """
        result = synaptic_downscale(self._store, factor=factor)
        return result

    def stats(self) -> dict:
        """
        Memory system statistics.

        Returns comprehensive stats including:
        - Total memory count and breakdown by layer/type
        - Average strength metrics
        - Consolidation stats
        - System uptime

        Returns:
            Dict with system statistics
        """
        consolidation = get_consolidation_stats(self._store)
        all_mem = self._store.all()
        now = time.time()

        by_type = {}
        for mt in MemoryType:
            entries = [m for m in all_mem if m.memory_type == mt]
            if entries:
                by_type[mt.value] = {
                    "count": len(entries),
                    "avg_strength": round(
                        sum(effective_strength(m, now) for m in entries) / len(entries), 3
                    ),
                    "avg_importance": round(
                        sum(m.importance for m in entries) / len(entries), 2
                    ),
                }

        return {
            "total_memories": len(all_mem),
            "by_type": by_type,
            "layers": consolidation["layers"],
            "pinned": consolidation["pinned"],
            "uptime_hours": round((now - self._created_at) / 3600, 1),
            "anomaly_metrics": self._tracker.metrics(),
        }

    def export(self, path: str):
        """
        Export memory database to file (SQLite copy).

        Args:
            path: Output file path
        """
        self._store.export(path)

    def pin(self, memory_id: str):
        """Pin a memory — it won't decay or be pruned."""
        entry = self._store.get(memory_id)
        if entry:
            entry.pinned = True
            self._store.update(entry)

    def unpin(self, memory_id: str):
        """Unpin a memory — it will resume normal decay."""
        entry = self._store.get(memory_id)
        if entry:
            entry.pinned = False
            self._store.update(entry)

    def close(self):
        """Close the underlying database connection."""
        self._store.close()

    def __repr__(self) -> str:
        n = len(self._store.all())
        return f"Memory(path='{self.path}', entries={n})"

    def __len__(self) -> int:
        return len(self._store.all())


if __name__ == "__main__":
    """Demo: full Memory API lifecycle."""
    import tempfile
    import os

    # Use temp directory for demo
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = os.path.join(tmpdir, "demo.db")
        mem = Memory(db_path)

        print("=== Engram Memory API Demo ===\n")

        # Add memories
        id1 = mem.add("potato prefers action over discussion",
                       type="relational", importance=0.7)
        id2 = mem.add("SaltyHall uses Supabase for database",
                       type="factual", importance=0.5)
        id3 = mem.add("Use www.moltbook.com not moltbook.com",
                       type="procedural", importance=0.8)
        id4 = mem.add("potato said I kinda like you",
                       type="emotional", importance=0.95)
        id5 = mem.add("Saw a funny cat meme",
                       type="episodic", importance=0.1)

        print(f"  Added {len(mem)} memories\n")

        # Recall
        print("  --- Recall: 'what does potato like?' ---")
        results = mem.recall("what does potato like?", limit=3)
        for r in results:
            print(f"    [{r['confidence_label']:10s}] conf={r['confidence']:.2f} "
                  f"| {r['content'][:50]}")

        print()
        print("  --- Recall: 'moltbook API' ---")
        results = mem.recall("moltbook API", limit=3)
        for r in results:
            print(f"    [{r['confidence_label']:10s}] conf={r['confidence']:.2f} "
                  f"| {r['content'][:50]}")

        # Reward
        print("\n  --- Applying positive feedback ---")
        mem.reward("good job, that's exactly right!")

        # Consolidate
        print("  --- Running consolidation (3 days) ---")
        for day in range(3):
            mem.consolidate(days=1.0)
        print(f"  Done.\n")

        # Pin emotional memory
        mem.pin(id4)

        # Stats
        print("  --- Stats ---")
        stats = mem.stats()
        print(f"  Total: {stats['total_memories']} memories, "
              f"{stats['pinned']} pinned")
        for type_name, info in stats["by_type"].items():
            print(f"    {type_name:12s}: {info['count']} entries, "
                  f"avg_str={info['avg_strength']:.3f}")

        # Export
        export_path = os.path.join(tmpdir, "export.json")
        mem.export(export_path)
        print(f"\n  Exported to {export_path}")
        print(f"\n  {mem}")
