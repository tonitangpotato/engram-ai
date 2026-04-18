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


from typing import Optional, Union, TYPE_CHECKING

if TYPE_CHECKING:
    from engram.session_wm import SessionWorkingMemory

from engram.config import MemoryConfig
from engram.core import MemoryEntry, MemoryStore, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE
from engram.store import SQLiteStore
from engram.activation import retrieve_top_k
from engram.search import SearchEngine
from engram.hybrid_search import HybridSearchEngine
from engram.consolidation import run_consolidation_cycle, get_consolidation_stats, consolidate_causal
from engram.forgetting import effective_strength, should_forget, prune_forgotten
from engram.confidence import confidence_score, confidence_label
from engram.reward import detect_feedback, apply_reward
from engram.downscaling import synaptic_downscale
from engram.anomaly import BaselineTracker
from engram.hebbian import (
    record_coactivation,
    get_hebbian_neighbors,
    get_all_hebbian_links,
    decay_hebbian_links,
)
from engram.adaptive_tuning import AdaptiveTuner
from engram.extractor import MemoryExtractor, ExtractedFact


# Map string type names to MemoryType enum
_TYPE_MAP = {t.value: t for t in MemoryType}


class Memory:
    """
    Main interface to the Engram memory system.

    Wraps the neuroscience math models behind a clean API.
    All complexity is hidden — you just add, recall, and consolidate.

    Backend: SQLiteStore for persistent storage with FTS5 search.
    """

    def __init__(
        self, 
        path: str = "./engram.db", 
        config: MemoryConfig = None,
        embedding = None,
        adaptive_tuning: bool = False,
        extractor: Optional[MemoryExtractor] = None,
    ):
        """
        Initialize Engram memory system.

        Args:
            path: Path to SQLite database file. Created if it doesn't exist.
                  Use ":memory:" for in-memory (non-persistent) operation.
            config: MemoryConfig with tunable parameters. None = literature defaults.
            embedding: Optional embedding adapter for semantic search.
                      Can be an EmbeddingAdapter instance or a string shortcut:
                      - "openai" -> OpenAIAdapter (requires OPENAI_API_KEY)
                      - "ollama" -> OllamaAdapter (requires local Ollama)
                      - None -> FTS5-only mode (no embeddings)
            adaptive_tuning: Enable automatic parameter tuning based on performance.
            extractor: Optional MemoryExtractor for LLM-based fact extraction.
                      If set, add() will extract structured facts from raw text.
                      If None, auto-configures from env vars / config file.
        """
        self.path = path
        self.config = config or MemoryConfig.default()
        self._store = SQLiteStore(path)
        self._tracker = BaselineTracker(window_size=self.config.anomaly_window_size)
        self._created_at = time.time()
        
        # Adaptive tuning (optional)
        self._adaptive_tuner = None
        if adaptive_tuning:
            self._adaptive_tuner = AdaptiveTuner(self.config)
        
        # Initialize embedding support
        self._embedding_adapter = None
        self._vector_store = None
        
        if embedding is not None:
            self._init_embedding(embedding)
        
        # Initialize extractor (code param > auto-detect from env/config)
        self._extractor: Optional[MemoryExtractor] = extractor
        if self._extractor is None:
            self._extractor = self._auto_configure_extractor()

    def _init_embedding(self, embedding):
        """Initialize embedding adapter and vector store."""
        from engram.vector_store import VectorStore
        
        # Handle string shortcuts
        if isinstance(embedding, str):
            if embedding == "openai":
                from engram.embeddings import OpenAIAdapter
                self._embedding_adapter = OpenAIAdapter()
            elif embedding == "ollama":
                from engram.embeddings import OllamaAdapter
                self._embedding_adapter = OllamaAdapter()
            else:
                raise ValueError(f"Unknown embedding shortcut: {embedding}. Use 'openai', 'ollama', or pass an adapter instance.")
        else:
            # Assume it's an adapter instance
            self._embedding_adapter = embedding
        
        # Initialize vector store
        self._vector_store = VectorStore(self._store._conn, self._embedding_adapter)

    def _auto_configure_extractor(self) -> Optional[MemoryExtractor]:
        """
        Auto-configure extractor from environment and config file.

        Detection order (high → low priority):
        1. ANTHROPIC_AUTH_TOKEN env var → AnthropicExtractor with OAuth
        2. ANTHROPIC_API_KEY env var → AnthropicExtractor with API key
        3. ~/.config/engram/config.json extractor section
        4. None → no extraction (backward compatible)

        Model can be overridden via ENGRAM_EXTRACTOR_MODEL env var.
        """
        import logging
        logger = logging.getLogger(__name__)

        model = os.environ.get("ENGRAM_EXTRACTOR_MODEL", "claude-haiku-4-5-20251001")

        # 1. ANTHROPIC_AUTH_TOKEN (OAuth mode)
        token = os.environ.get("ANTHROPIC_AUTH_TOKEN")
        if token:
            from engram.extractor import AnthropicExtractor
            logger.info("Extractor: Anthropic (OAuth) from ANTHROPIC_AUTH_TOKEN")
            return AnthropicExtractor(auth_token=token, is_oauth=True, model=model)

        # 2. ANTHROPIC_API_KEY (API key mode)
        key = os.environ.get("ANTHROPIC_API_KEY")
        if key:
            from engram.extractor import AnthropicExtractor
            logger.info("Extractor: Anthropic (API key) from ANTHROPIC_API_KEY")
            return AnthropicExtractor(auth_token=key, is_oauth=False, model=model)

        # 3. Config file
        return self._load_extractor_from_config()

    def _load_extractor_from_config(self) -> Optional[MemoryExtractor]:
        """Load extractor configuration from ~/.config/engram/config.json."""
        import json
        import logging
        from pathlib import Path
        logger = logging.getLogger(__name__)

        config_path = Path("~/.config/engram/config.json").expanduser()
        if not config_path.exists():
            return None

        try:
            data = json.loads(config_path.read_text())
        except (json.JSONDecodeError, OSError):
            return None

        ext_cfg = data.get("extractor")
        if not ext_cfg:
            return None

        provider = ext_cfg.get("provider")
        if provider == "anthropic":
            # Still need env var for auth — config file NEVER stores tokens
            token = os.environ.get("ANTHROPIC_AUTH_TOKEN") or os.environ.get("ANTHROPIC_API_KEY")
            if not token:
                return None
            is_oauth = bool(os.environ.get("ANTHROPIC_AUTH_TOKEN"))
            model = ext_cfg.get("model", "claude-haiku-4-5-20251001")
            from engram.extractor import AnthropicExtractor
            logger.info("Extractor: Anthropic (%s) from config file", model)
            return AnthropicExtractor(auth_token=token, is_oauth=is_oauth, model=model)
        elif provider == "ollama":
            model = ext_cfg.get("model", "llama3.2:3b")
            host = ext_cfg.get("host", "http://localhost:11434")
            from engram.extractor import OllamaExtractor
            logger.info("Extractor: Ollama (%s) from config file", model)
            return OllamaExtractor(model=model, host=host)

        return None

    def set_extractor(self, extractor: Optional[MemoryExtractor]):
        """Set or clear the memory extractor."""
        self._extractor = extractor

    def has_extractor(self) -> bool:
        """Check if an extractor is configured."""
        return self._extractor is not None

    def add(self, content: str, type: str = "factual", importance: float = None,
            source: str = "", tags: list[str] = None,
            entities: list = None, contradicts: str = None,
            created_at: float = None, metadata: dict = None,
            extract: bool = True) -> str:
        """
        Store a new memory. Returns memory ID.

        If an extractor is configured and extract=True, the content is first
        passed through the LLM to extract structured facts. Each extracted
        fact is stored as a separate memory. If extraction fails or returns
        nothing, the raw content is stored as a fallback.

        Args:
            content: The memory content (natural language)
            type: Memory type — one of: factual, episodic, relational,
                  emotional, procedural, opinion, causal
            importance: 0-1 importance score (None = auto from type)
            source: Source identifier (e.g., filename, conversation ID)
            tags: Optional tags for categorization (stored in content for now)
            metadata: Optional structured metadata dict (e.g., causal memories
                     use cause/effect/confidence/domain fields)
            extract: If True and extractor is configured, extract facts from content.
                    Set to False to force raw storage (e.g., for already-extracted facts).

        Returns:
            Memory ID string (8-char UUID prefix). If multiple facts extracted,
            returns the ID of the last one stored.
        """
        import logging
        logger = logging.getLogger(__name__)

        # If extractor is configured and extraction is enabled, try to extract facts
        if extract and self._extractor is not None:
            try:
                facts = self._extractor.extract(content)
                if facts:
                    logger.info(
                        "Extracted %d facts from content (%.40s...)",
                        len(facts), content,
                    )
                    last_id = ""
                    for fact in facts:
                        logger.info(
                            "  → [%s] (imp=%.1f) %.80s",
                            fact.memory_type, fact.importance, fact.content,
                        )
                        last_id = self.add(
                            content=fact.content,
                            type=fact.memory_type,
                            importance=fact.importance,
                            source=source,
                            tags=tags,
                            entities=entities,
                            created_at=created_at,
                            metadata=metadata,
                            extract=False,  # Don't re-extract extracted facts
                        )
                    return last_id
                else:
                    # Nothing worth storing according to the LLM
                    logger.info(
                        "Extractor: nothing worth storing in: %.50s...", content,
                    )
                    return ""
            except Exception as e:
                # Extractor failed — fall back to storing raw text
                logger.warning("Extractor failed, storing raw: %s", e)

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
            created_at=created_at,
            metadata=metadata,
        )

        # Handle contradiction linking
        if contradicts:
            old_entry = self._store.get(contradicts)
            if old_entry:
                entry.contradicts = contradicts
                # Update entry in store with contradiction field
                self._store.update(entry)
                # Mark old memory as contradicted
                old_entry.contradicted_by = entry.id
                self._store.update(old_entry)

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
        
        # Store embedding if adapter is configured
        if self._vector_store is not None:
            self._vector_store.add(entry.id, actual_content)

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
        # Use hybrid search if embeddings are available, else FTS5-only
        if self._vector_store is not None:
            engine = HybridSearchEngine(self._store, self._vector_store)
        else:
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
                "pinned": r.entry.pinned,  # Pinned memories are sorted first
                "contradicted": bool(r.entry.contradicted_by),
                "source": r.entry.source_file,  # Include source for evidence matching
            })

        # Track retrieval for anomaly detection
        self._tracker.update("retrieval_count", len(output))

        # ACT-R: Record access for all retrieved memories (boosts future retrieval)
        for r in search_results:
            self._store.record_access(r.entry.id)

        # Hebbian learning: record co-activation for recalled memories
        # STDP: also track temporal ordering for causal inference
        if self.config.hebbian_enabled and len(output) >= 2:
            memory_ids = [r["id"] for r in output]
            record_coactivation(
                self._store,
                memory_ids,
                threshold=self.config.hebbian_threshold,
                stdp_enabled=self.config.stdp_enabled,
            )
        
        # Adaptive tuning: record recall metrics
        if self._adaptive_tuner is not None:
            self._adaptive_tuner.record_recall(output)
            # Auto-adapt if ready
            if self._adaptive_tuner.should_adapt():
                changes = self._adaptive_tuner.adapt()
                if changes:
                    # Log parameter changes (could emit event here)
                    pass

        return output

    def session_recall(
        self,
        query: str,
        session_wm: "SessionWorkingMemory" = None,
        limit: int = 5,
        context: list[str] = None,
        types: list[str] = None,
        min_confidence: float = 0.0,
        graph_expand: bool = True,
    ) -> list[dict]:
        """
        Session-aware recall — only execute full recall when topic changes.
        
        This is the intelligent recall entry point for conversational agents.
        Instead of always doing expensive retrieval, it:
        
        1. Checks if the query topic overlaps with current working memory
        2. If yes (continuous topic) → return cached working memory items
        3. If no (topic switch) → do full recall and update working memory
        
        Based on cognitive science: we don't re-search our entire memory
        for every utterance; we keep ~7 items active and only search
        when the context changes.
        
        Args:
            query: Natural language query
            session_wm: SessionWorkingMemory instance. If None, falls back to regular recall.
            limit: Maximum number of results (only used for full recall)
            context: Additional context keywords
            types: Filter by memory types
            min_confidence: Minimum confidence threshold
            graph_expand: Whether to expand via graph links
            
        Returns:
            List of memory dicts. Same format as recall().
            Results from working memory cache have "_from_wm": True.
        """
        # Import here to avoid circular import
        from engram.session_wm import SessionWorkingMemory
        
        # No session WM provided → fall back to standard recall
        if session_wm is None:
            return self.recall(
                query, limit=limit, context=context, types=types,
                min_confidence=min_confidence, graph_expand=graph_expand,
            )
        
        # Check if we need a full recall
        if session_wm.needs_recall(query, self):
            # Topic changed or WM empty → full recall
            results = self.recall(
                query, limit=limit, context=context, types=types,
                min_confidence=min_confidence, graph_expand=graph_expand,
            )
            # Update working memory with new results
            session_wm.activate([r["id"] for r in results])
            return results
        else:
            # Topic continuous → return working memory items
            return session_wm.get_active_memories(self)

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
        # Track count before consolidation
        n_memories_before = len(self._store.all())
        
        run_consolidation_cycle(
            self._store, dt_days=days,
            interleave_ratio=self.config.interleave_ratio,
            alpha=self.config.alpha,
            mu1=self.config.mu1, mu2=self.config.mu2,
            replay_boost=self.config.replay_boost,
            promote_threshold=self.config.promote_threshold,
            demote_threshold=self.config.demote_threshold,
            archive_threshold=self.config.archive_threshold,
        )
        synaptic_downscale(self._store, factor=self.config.downscale_factor)

        # Decay Hebbian links during consolidation
        if self.config.hebbian_enabled:
            decay_hebbian_links(self._store, factor=self.config.hebbian_decay)
        
        # STDP: auto-create causal memories from temporal patterns
        if self.config.stdp_enabled and self.config.hebbian_enabled:
            consolidate_causal(self._store, self.config)
        
        # Adaptive tuning: record consolidation metrics
        if self._adaptive_tuner is not None:
            n_memories_after = len(self._store.all())
            n_forgotten = max(0, n_memories_before - n_memories_after)
            self._adaptive_tuner.record_consolidation(n_forgotten)

    def forget(self, memory_id: str = None, threshold: float = None):
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
        if threshold is None:
            threshold = self.config.forget_threshold
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
                     reward_magnitude=self.config.reward_magnitude * conf)
        
        # Adaptive tuning: record reward feedback
        if self._adaptive_tuner is not None:
            self._adaptive_tuner.record_reward(polarity)

    def downscale(self, factor: float = None):
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
        if factor is None:
            factor = self.config.downscale_factor
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

        stats_dict = {
            "total_memories": len(all_mem),
            "by_type": by_type,
            "layers": consolidation["layers"],
            "pinned": consolidation["pinned"],
            "uptime_hours": round((now - self._created_at) / 3600, 1),
            "anomaly_metrics": self._tracker.metrics(),
        }
        
        # Add adaptive tuning metrics if enabled
        if self._adaptive_tuner is not None:
            stats_dict["adaptive_tuning"] = self._adaptive_tuner.get_metrics()
        
        return stats_dict

    def export(self, path: str):
        """
        Export memory database to file (SQLite copy).

        Args:
            path: Output file path
        """
        self._store.export(path)

    def update_memory(self, memory_id: str, new_content: str, reason: str = "correction") -> str:
        """Update a memory's content, marking the old version as contradicted.
        Creates a new memory with the correction and links them.

        Args:
            memory_id: ID of the memory to update
            new_content: The corrected content
            reason: Reason for the update (stored in source)

        Returns:
            New memory ID string
        """
        old_entry = self._store.get(memory_id)
        if old_entry is None:
            raise ValueError(f"Memory {memory_id} not found")

        return self.add(
            content=new_content,
            type=old_entry.memory_type.value,
            importance=old_entry.importance,
            source=f"{reason}:{memory_id}",
            contradicts=memory_id,
        )

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

    def recall_associated(self, cause_query: str = None, limit: int = 10,
                          min_confidence: float = 0.0) -> list[dict]:
        """
        Recall associated memories — memories about cause→effect relationships.
        
        These are either:
        1. Manually stored with type=causal
        2. Auto-created by STDP during consolidation
        
        Note: Renamed from recall_causal for API consistency with Rust crate.
        Uses Hebbian links to find memories that frequently co-occur.
        
        If cause_query is provided, does semantic search filtered to type=causal.
        Otherwise returns all causal memories sorted by importance.
        
        Args:
            cause_query: Optional search query to find relevant associated memories.
                        If None, returns all causal memories.
            limit: Maximum results to return
            min_confidence: Minimum confidence from metadata (0-1)
            
        Returns:
            List of dicts with associated memory info including metadata
        """
        import json
        
        if cause_query:
            # Semantic search filtered to causal type
            results = self.recall(
                cause_query, limit=limit * 2,  # Over-fetch since we filter
                types=["causal"],
                min_confidence=0.0,
                graph_expand=False,
            )
        else:
            # Get all causal memories
            entries = self._store.search_causal(limit=limit * 2)
            now = time.time()
            results = []
            for entry in entries:
                strength = effective_strength(entry, now=now)
                conf = confidence_score(entry)
                results.append({
                    "id": entry.id,
                    "content": entry.content,
                    "type": "causal",
                    "confidence": round(conf, 3),
                    "confidence_label": confidence_label(conf),
                    "strength": round(strength, 3),
                    "age_days": round(entry.age_days(), 1),
                    "layer": entry.layer.value,
                    "importance": round(entry.importance, 2),
                    "pinned": entry.pinned,
                    "metadata": entry.metadata,
                })
        
        # Filter by metadata confidence if requested
        if min_confidence > 0:
            filtered = []
            for r in results:
                meta = r.get("metadata") or {}
                meta_conf = meta.get("confidence", 1.0)  # Default 1.0 for manual causal
                if meta_conf >= min_confidence:
                    filtered.append(r)
            results = filtered
        
        # Ensure metadata is included in results from recall()
        for r in results:
            if "metadata" not in r:
                entry = self._store.get(r["id"])
                if entry:
                    r["metadata"] = entry.metadata
        
        return results[:limit]

    # Backward compatibility alias
    def recall_causal(self, *args, **kwargs):
        """Deprecated: Use recall_associated() instead."""
        return self.recall_associated(*args, **kwargs)

    def hebbian_links(self, memory_id: str = None) -> list[tuple[str, str, float]]:
        """
        Get Hebbian links for a specific memory or all links.

        Hebbian links are formed when memories are repeatedly recalled
        together — "neurons that fire together, wire together."

        Args:
            memory_id: If provided, get links for this memory only.
                      If None, get all Hebbian links.

        Returns:
            List of (source_id, target_id, strength) tuples.
            For a specific memory_id, source_id will always be that ID.
        """
        if memory_id:
            neighbors = get_hebbian_neighbors(self._store, memory_id)
            # Fetch strengths for each neighbor
            links = []
            for neighbor_id in neighbors:
                row = self._store._conn.execute(
                    "SELECT strength FROM hebbian_links WHERE source_id=? AND target_id=?",
                    (memory_id, neighbor_id)
                ).fetchone()
                if row:
                    links.append((memory_id, neighbor_id, row[0]))
            return links
        else:
            return get_all_hebbian_links(self._store)

    # ═══ Engram v2: Multi-Agent & Emotional Bus ═══
    
    @staticmethod
    def with_emotional_bus(
        db_path: str,
        workspace_dir: str,
        config: MemoryConfig = None,
        embedding = None,
    ):
        """
        Create a Memory instance with an Emotional Bus attached.
        
        The Emotional Bus connects memory to workspace files (SOUL.md, HEARTBEAT.md)
        for drive alignment and emotional feedback loops.
        
        Args:
            db_path: Path to SQLite database file
            workspace_dir: Path to the agent workspace directory
            config: Optional MemoryConfig
            embedding: Optional embedding adapter
            
        Returns:
            Memory instance with emotional bus attached
        """
        from engram.bus import EmotionalBus
        
        mem = Memory(db_path, config, embedding)
        mem._emotional_bus = EmotionalBus(workspace_dir, mem._store._conn)
        mem._agent_id = None
        return mem
    
    def set_agent_id(self, agent_id: str):
        """
        Set the agent ID for this memory instance.
        
        This is used for ACL checks when storing and recalling memories.
        Each agent should identify itself before performing operations.
        """
        self._agent_id = agent_id
        if not hasattr(self, '_acl_manager'):
            from engram.acl import AclManager
            self._acl_manager = AclManager(self._store._conn)
    
    def add_to_namespace(
        self,
        content: str,
        type: str = "factual",
        importance: float = None,
        source: str = "",
        namespace: str = "default",
        **kwargs
    ) -> str:
        """
        Store a new memory in a specific namespace.
        
        Args:
            content: The memory content
            type: Memory type (factual, episodic, etc.)
            importance: 0-1 importance score (None = auto from type)
            source: Source identifier
            namespace: Namespace to store in (default: "default")
            **kwargs: Additional arguments (tags, entities, metadata, etc.)
            
        Returns:
            Memory ID string
        """
        memory_type = _TYPE_MAP.get(type, MemoryType.FACTUAL)
        
        # Apply drive alignment boost if Emotional Bus is attached
        base_importance = importance or DEFAULT_IMPORTANCE[memory_type]
        if hasattr(self, '_emotional_bus') and self._emotional_bus:
            boost = self._emotional_bus.align_importance(content)
            importance = min(1.0, base_importance * boost)
        else:
            importance = base_importance
        
        # Store with namespace
        entry = self._store.add(
            content=content,
            memory_type=memory_type,
            importance=importance,
            source_file=source,
            namespace=namespace,
            **kwargs
        )
        
        # Store embedding if adapter is configured
        if self._vector_store is not None:
            self._vector_store.add(entry.id, content)
        
        return entry.id
    
    def add_with_emotion(
        self,
        content: str,
        type: str = "factual",
        importance: float = None,
        source: str = "",
        namespace: str = "default",
        emotion: float = 0.0,
        domain: str = "general",
    ) -> str:
        """
        Store a new memory with emotional tracking.
        
        This method both stores the memory and records the emotional valence
        in the Emotional Bus for trend tracking. Requires an Emotional Bus
        to be attached.
        
        Args:
            content: The memory content
            type: Memory type
            importance: 0-1 importance score (None = auto)
            source: Source identifier
            namespace: Namespace to store in
            emotion: Emotional valence (-1.0 to 1.0)
            domain: Domain for emotional tracking
            
        Returns:
            Memory ID string
        """
        if not hasattr(self, '_emotional_bus') or not self._emotional_bus:
            raise RuntimeError("Emotional Bus not attached. Use Memory.with_emotional_bus()")
        
        # Store the memory (with importance boost from alignment)
        memory_id = self.add_to_namespace(content, type, importance, source, namespace)
        
        # Record emotion
        self._emotional_bus.process_interaction(content, emotion, domain)
        
        return memory_id
    
    def recall_from_namespace(
        self,
        query: str,
        namespace: str = "default",
        limit: int = 5,
        context: list[str] = None,
        min_confidence: float = 0.0,
    ) -> list[dict]:
        """
        Retrieve relevant memories from a specific namespace.
        
        Args:
            query: Natural language query
            namespace: Namespace to search (None = "default", "*" = all namespaces)
            limit: Maximum number of results
            context: Additional context keywords to boost relevant memories
            min_confidence: Minimum confidence threshold (0-1)
            
        Returns:
            List of memory dicts
        """
        # Use namespace-aware FTS search
        candidates = self._store.search_fts_ns(query, limit * 3, namespace)
        
        # Score with ACT-R activation
        from engram.activation import retrieval_activation
        
        now = time.time()
        scored = []
        
        for entry in candidates:
            activation = retrieval_activation(
                entry,
                context or [],
                now,
                self.config.actr_decay,
                self.config.context_weight,
                self.config.importance_weight,
            )
            conf = confidence_score(entry, activation, now)
            
            if conf >= min_confidence:
                scored.append((entry, activation, conf))
        
        # Sort by activation descending
        scored.sort(key=lambda x: x[1], reverse=True)
        scored = scored[:limit]
        
        # Record access
        for entry, _, _ in scored:
            self._store.record_access(entry.id)
        
        # Format output
        output = []
        for entry, activation, conf in scored:
            output.append({
                "id": entry.id,
                "content": entry.content,
                "type": entry.memory_type.value,
                "confidence": round(conf, 3),
                "confidence_label": confidence_label(conf),
                "strength": round(effective_strength(entry, now), 3),
                "activation": round(activation, 3),
                "age_days": round(entry.age_days(), 1),
                "layer": entry.layer.value,
                "importance": round(entry.importance, 2),
                "pinned": entry.pinned,
                "source": entry.source_file,
            })
        
        return output
    
    def emotional_bus(self):
        """Get the Emotional Bus, if attached."""
        return getattr(self, '_emotional_bus', None)
    
    def grant(self, agent_id: str, namespace: str, permission: str, as_system: bool = False):
        """
        Grant a permission to an agent for a namespace.
        
        Args:
            agent_id: The agent ID to grant permission to
            namespace: Namespace this permission applies to ("*" = all namespaces)
            permission: Permission level ("read", "write", or "admin")
            as_system: If True, bypass permission checks (use for initial setup)
        """
        from engram.acl import Permission
        
        if not hasattr(self, '_acl_manager'):
            from engram.acl import AclManager
            self._acl_manager = AclManager(self._store._conn)
        
        perm = Permission[permission.upper()]
        
        if as_system:
            grantor = "system"
        else:
            grantor = getattr(self, '_agent_id', None) or "system"
        
        self._acl_manager.grant(agent_id, namespace, perm, grantor)
    
    def revoke(self, agent_id: str, namespace: str) -> bool:
        """
        Revoke a permission from an agent for a namespace.
        
        Returns:
            True if permission was revoked, False if no permission existed
        """
        if not hasattr(self, '_acl_manager'):
            from engram.acl import AclManager
            self._acl_manager = AclManager(self._store._conn)
        
        return self._acl_manager.revoke(agent_id, namespace)
    
    def subscribe(self, agent_id: str, namespace: str, min_importance: float):
        """
        Subscribe to notifications for a namespace.
        
        The agent will receive notifications when new memories are stored
        with importance >= min_importance in the specified namespace.
        
        Args:
            agent_id: The subscribing agent's ID
            namespace: Namespace to watch ("*" for all)
            min_importance: Minimum importance threshold (0.0-1.0)
        """
        if not hasattr(self, '_subscription_manager'):
            from engram.subscriptions import SubscriptionManager
            self._subscription_manager = SubscriptionManager(self._store._conn)
        
        self._subscription_manager.subscribe(agent_id, namespace, min_importance)
    
    def unsubscribe(self, agent_id: str, namespace: str) -> bool:
        """
        Unsubscribe from a namespace.
        
        Returns:
            True if subscription was removed, False if no subscription existed
        """
        if not hasattr(self, '_subscription_manager'):
            from engram.subscriptions import SubscriptionManager
            self._subscription_manager = SubscriptionManager(self._store._conn)
        
        return self._subscription_manager.unsubscribe(agent_id, namespace)
    
    def check_notifications(self, agent_id: str) -> list:
        """
        Check for notifications since last check.
        
        Returns new memories that exceed the subscription thresholds.
        Updates the cursor so the same notifications aren't returned twice.
        
        Args:
            agent_id: The agent ID to check notifications for
            
        Returns:
            List of notification dicts
        """
        if not hasattr(self, '_subscription_manager'):
            from engram.subscriptions import SubscriptionManager
            self._subscription_manager = SubscriptionManager(self._store._conn)
        
        notifs = self._subscription_manager.check_notifications(agent_id)
        
        # Convert to dicts for easier use
        return [
            {
                "memory_id": n.memory_id,
                "namespace": n.namespace,
                "content": n.content,
                "importance": n.importance,
                "created_at": n.created_at.isoformat(),
                "subscription_namespace": n.subscription_namespace,
                "threshold": n.threshold,
            }
            for n in notifs
        ]

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
