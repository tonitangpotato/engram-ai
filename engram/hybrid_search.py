"""
Hybrid Search Engine with Embedding Support

Pipeline:
1. Vector search → semantic candidates (if embedding adapter provided)
2. FTS5 keyword search → lexical candidates (always runs)
3. Merge & dedupe candidates
4. ACT-R activation scoring → rank candidates
5. Confidence scoring → attach metacognitive confidence
6. Return top-k with scores

The key insight: embedding finds candidates, ACT-R decides priority.
"""

import re
import time
from dataclasses import dataclass
from typing import Optional


TEMPORAL_KEYWORDS = {
    'recently', 'recent', 'lately', 'last time', 'how often', 'frequently',
    'when did', 'when was', 'how long ago', 'before', 'after', 'earlier',
    'previous', 'previously', 'first time', 'most recent', 'latest',
    'yesterday', 'last week', 'last month', 'ago', 'since', 'until',
    'often', 'always', 'never', 'sometimes', 'usually', 'regularly'
}


def detect_temporal_alpha(query: str) -> float:
    """
    Detect if query is temporal and return alpha for score blending.
    
    High alpha (0.9) = embedding-dominant (semantic queries)
    Low alpha (0.3) = ACT-R-dominant (temporal queries)
    """
    query_lower = query.lower()
    
    # Check for temporal keywords
    temporal_score = 0
    for keyword in TEMPORAL_KEYWORDS:
        if keyword in query_lower:
            temporal_score += 1
    
    # More temporal keywords = lower alpha (more ACT-R influence)
    if temporal_score >= 2:
        return 0.3  # Strong temporal signal
    elif temporal_score == 1:
        return 0.6  # Moderate temporal signal
    else:
        return 0.9  # Semantic query, embedding dominant


def sanitize_fts_query(query: str) -> str:
    """Sanitize query for FTS5 by keeping only alphanumeric characters and removing stop words."""
    sanitized = re.sub(r'[^a-zA-Z0-9\s]', ' ', query)
    sanitized = re.sub(r'\s+', ' ', sanitized).strip()
    if not sanitized:
        return "memory"
    # Remove stop words for better FTS5 matching
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 'is', 'was', 
                  'are', 'were', 'be', 'been', 'what', 'where', 'when', 'who', 'does', 'do', 'did',
                  'go', 'going', 'went', 'has', 'have', 'had', 'this', 'that', 'these', 'those'}
    words = [w for w in sanitized.lower().split() if w not in stop_words and len(w) > 2]
    return ' '.join(words) if words else sanitized

from engram.core import MemoryEntry, MemoryType, MemoryLayer
from engram.store import SQLiteStore
from engram.activation import retrieval_activation
from engram.forgetting import effective_strength
from engram.confidence import confidence_score, confidence_label
from engram.hebbian import get_hebbian_neighbors


@dataclass
class HybridSearchResult:
    entry: MemoryEntry
    score: float            # Combined final score (ACT-R activation)
    confidence: float       # 0-1 metacognitive confidence
    confidence_label: str   # "certain"/"likely"/"uncertain"/"vague"
    vector_score: float     # Cosine similarity from vector search (0 if not used)
    fts_matched: bool       # Whether FTS5 found this candidate


class HybridSearchEngine:
    """
    Hybrid retrieval combining Vector + FTS5 + ACT-R.
    
    This is the recommended search engine when using embeddings.
    """

    def __init__(self, store: SQLiteStore, vector_store=None):
        """
        Initialize hybrid search.
        
        Args:
            store: SQLite store for memories
            vector_store: Optional VectorStore for embedding-based retrieval
        """
        self.store = store
        self.vector_store = vector_store

    def search(
        self,
        query: str = "",
        limit: int = 5,
        context_keywords: Optional[list[str]] = None,
        types: Optional[list[str]] = None,
        layers: Optional[list[str]] = None,
        min_confidence: float = 0.0,
        time_range: Optional[tuple[float, float]] = None,
        graph_expand: bool = True,
        vector_weight: float = 0.7,
    ) -> list[HybridSearchResult]:
        """
        Hybrid search combining vector, FTS5, and ACT-R.
        
        Args:
            query: Search query
            limit: Max results to return
            context_keywords: Additional context for ACT-R spreading activation
            types: Filter by memory types
            layers: Filter by memory layers
            min_confidence: Minimum confidence threshold
            time_range: (start_ts, end_ts) filter
            graph_expand: Enable graph/Hebbian expansion
            vector_weight: Weight for vector similarity in final score (0-1)
            
        Returns:
            List of HybridSearchResult sorted by combined score
        """
        query = query.strip()
        candidates: dict[str, tuple[MemoryEntry, float, bool]] = {}  # id -> (entry, vector_score, fts_matched)
        
        # 1. Vector search (semantic matching)
        if self.vector_store and query:
            vector_results = self.vector_store.search(query, limit=100, min_similarity=0.1)
            for memory_id, similarity in vector_results:
                entry = self.store.get(memory_id)
                if entry:
                    candidates[memory_id] = (entry, similarity, False)
        
        # 2. FTS5 search (lexical matching)
        if query:
            fts_query = sanitize_fts_query(query)
            fts_results = self.store.search_fts(fts_query, limit=100)
            for entry in fts_results:
                if entry.id in candidates:
                    # Already found by vector search, mark as FTS matched too
                    existing = candidates[entry.id]
                    candidates[entry.id] = (existing[0], existing[1], True)
                else:
                    candidates[entry.id] = (entry, 0.0, True)
        
        # 3. If no candidates found, fall back to full scan
        if not candidates:
            for entry in self.store.all():
                candidates[entry.id] = (entry, 0.0, False)
        
        # Convert to list
        candidate_list = [(entry, vec_score, fts_matched) 
                          for entry, vec_score, fts_matched in candidates.values()]
        
        # 4. Apply filters
        if types:
            type_set = set(types)
            candidate_list = [(e, v, f) for e, v, f in candidate_list 
                             if e.memory_type.value in type_set]
        
        if layers:
            layer_set = set(layers)
            candidate_list = [(e, v, f) for e, v, f in candidate_list 
                             if e.layer.value in layer_set]
        
        if time_range:
            t_min, t_max = time_range
            candidate_list = [(e, v, f) for e, v, f in candidate_list 
                             if t_min <= e.created_at <= t_max]
        
        # 5. Graph expansion (Hebbian)
        hebbian_boosts: dict[str, float] = {}
        if graph_expand:
            candidate_list, hebbian_boosts = self._expand_via_graph(candidate_list)
        
        # 6. Score with ACT-R + vector similarity
        scored = self._score_candidates(
            candidate_list,
            context_keywords,
            hebbian_boosts,
            vector_weight,
        )
        
        # 7. Rank and filter
        return self._rank_and_filter(scored, limit, min_confidence)

    def _expand_via_graph(
        self, 
        candidates: list[tuple[MemoryEntry, float, bool]]
    ) -> tuple[list[tuple[MemoryEntry, float, bool]], dict[str, float]]:
        """Expand via Hebbian links and compute spreading activation boosts."""
        seen_ids = {e.id for e, _, _ in candidates}
        new_candidates = []
        hebbian_boosts: dict[str, float] = {}
        
        for entry, vec_score, fts_matched in candidates:
            hebbian_neighbors = get_hebbian_neighbors(self.store, entry.id)
            for neighbor_id in hebbian_neighbors:
                strength = self._get_hebbian_strength(entry.id, neighbor_id)
                boost = 0.5 * strength
                hebbian_boosts[neighbor_id] = hebbian_boosts.get(neighbor_id, 0) + boost
                
                if neighbor_id not in seen_ids:
                    neighbor_entry = self.store.get(neighbor_id)
                    if neighbor_entry:
                        seen_ids.add(neighbor_id)
                        new_candidates.append((neighbor_entry, 0.0, False))
        
        return candidates + new_candidates, hebbian_boosts

    def _get_hebbian_strength(self, source_id: str, target_id: str) -> float:
        """Get Hebbian link strength between two memories."""
        try:
            row = self.store._conn.execute(
                """SELECT strength FROM hebbian_links 
                   WHERE (source_id=? AND target_id=?) OR (source_id=? AND target_id=?)""",
                (source_id, target_id, target_id, source_id)
            ).fetchone()
            return row[0] if row else 0.0
        except Exception:
            return 0.0

    def _score_candidates(
        self,
        candidates: list[tuple[MemoryEntry, float, bool]],
        context_keywords: Optional[list[str]],
        hebbian_boosts: dict[str, float],
        vector_weight: float,
    ) -> list[HybridSearchResult]:
        """Score candidates using combined FTS + embedding + ACT-R.
        
        Weights (matching Rust implementation):
        - 15% FTS for exact term matching
        - ~60% embedding for semantic similarity
        - ~25% ACT-R for recency/frequency/importance
        
        This ensures users always get the benefit of both search strategies
        without needing to choose.
        """
        now = time.time()
        results = []
        
        # Weights matching Rust: fts=0.15, embedding=embedding_weight*0.85, actr=actr_weight
        fts_weight = 0.15
        emb_adj_weight = vector_weight * 0.85  # ~0.6 with default vector_weight=0.7
        actr_adj_weight = 1.0 - vector_weight   # ~0.3 with default vector_weight=0.7
        
        # Build FTS rank-based score map for proper normalization
        fts_entries = [(e, v, f) for e, v, f in candidates if f]
        fts_count = len(fts_entries)
        fts_rank_scores: dict[str, float] = {}
        for rank, (entry, _, _) in enumerate(fts_entries):
            fts_rank_scores[entry.id] = 1.0 - (rank / max(1, fts_count))
        
        for entry, vector_score, fts_matched in candidates:
            # Ensure access_times are populated
            if not entry.access_times:
                entry.access_times = self.store.get_access_times(entry.id)
            
            # ACT-R activation
            act_score = retrieval_activation(
                entry,
                context_keywords=context_keywords,
                now=now,
            )
            
            if act_score == float("-inf"):
                continue
            
            # Normalize activation to 0..1 range (matching Rust)
            activation_normalized = max(0.0, min(1.0, (act_score + 10.0) / 20.0))
            
            # Normalize embedding score to 0..1 range
            embedding_score = (vector_score + 1.0) / 2.0 if vector_score != 0.0 else 0.0
            
            # FTS score (rank-based, 0 if not in FTS results)
            fts_score = fts_rank_scores.get(entry.id, 0.0)
            
            # Confidence
            conf = confidence_score(entry, store=None, now=now)
            label = confidence_label(conf)
            
            # Hebbian boost
            hebbian_boost = min(3.0, hebbian_boosts.get(entry.id, 0.0))
            
            # Pinned memory boost
            pinned_boost = 5.0 if entry.pinned else 0.0
            
            # Combined score: FTS + embedding + ACT-R (always blended)
            combined_score = (
                (fts_weight * fts_score)
                + (emb_adj_weight * embedding_score)
                + (actr_adj_weight * activation_normalized)
                + hebbian_boost
                + pinned_boost
            )
            
            results.append(HybridSearchResult(
                entry=entry,
                score=combined_score,
                confidence=conf,
                confidence_label=label,
                vector_score=vector_score,
                fts_matched=fts_matched,
            ))
        
        return results

    def _rank_and_filter(
        self,
        scored: list[HybridSearchResult],
        limit: int,
        min_confidence: float,
    ) -> list[HybridSearchResult]:
        """Sort by score, filter by confidence, return top-k."""
        if min_confidence > 0:
            scored = [r for r in scored if r.confidence >= min_confidence]
        
        scored.sort(key=lambda r: r.score, reverse=True)
        return scored[:limit]
