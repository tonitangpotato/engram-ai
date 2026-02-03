"""
Hybrid Search Engine — FTS5 + ACT-R + Structured Filtering

Pipeline:
1. FTS5 keyword search → candidate set
2. SQL filters (type, layer, time range) → narrow candidates
3. ACT-R activation scoring → rank candidates
4. Confidence scoring → attach metacognitive confidence
5. Return top-k with scores
"""

import time
from dataclasses import dataclass
from typing import Optional

from engram.core import MemoryEntry, MemoryType, MemoryLayer
from engram.store import SQLiteStore
from engram.activation import retrieval_activation
from engram.forgetting import effective_strength
from engram.confidence import confidence_score, confidence_label


@dataclass
class SearchResult:
    entry: MemoryEntry
    score: float            # Combined final score
    confidence: float       # 0-1 metacognitive confidence
    confidence_label: str   # "certain"/"likely"/"uncertain"/"vague"
    relevance: float        # FTS5 relevance component (0 if no query)


class SearchEngine:
    """Hybrid retrieval combining FTS5 + ACT-R + structured filtering."""

    def __init__(self, store: SQLiteStore):
        self.store = store

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
    ) -> list[SearchResult]:
        """Main search method."""
        candidates = self._get_candidates(query, types, layers, time_range)

        # Graph expansion: find entities in candidates, pull in related memories
        if graph_expand and candidates:
            candidates = self._expand_via_graph(candidates)

            # Re-apply filters on expanded set
            if types:
                type_set = {t for t in types}
                candidates = [c for c in candidates if c.memory_type.value in type_set]
            if layers:
                layer_set = {l for l in layers}
                candidates = [c for c in candidates if c.layer.value in layer_set]
            if time_range:
                t_min, t_max = time_range
                candidates = [c for c in candidates if t_min <= c.created_at <= t_max]

        scored = self._score_candidates(candidates, context_keywords, has_query=bool(query.strip()))
        return self._rank_and_filter(scored, limit, min_confidence)

    def _get_candidates(
        self,
        query: str,
        types: Optional[list[str]],
        layers: Optional[list[str]],
        time_range: Optional[tuple[float, float]],
    ) -> list[MemoryEntry]:
        """Get candidates via FTS5 or full scan, then apply SQL-level filters."""
        query = query.strip()

        if query:
            candidates = self.store.search_fts(query, limit=100)
            # Fall back to full scan if FTS returns nothing
            if not candidates:
                candidates = self.store.all()
        else:
            candidates = self.store.all()

        # Apply filters
        if types:
            type_set = {t for t in types}
            candidates = [c for c in candidates if c.memory_type.value in type_set]

        if layers:
            layer_set = {l for l in layers}
            candidates = [c for c in candidates if c.layer.value in layer_set]

        if time_range:
            t_min, t_max = time_range
            candidates = [c for c in candidates if t_min <= c.created_at <= t_max]

        return candidates

    def _expand_via_graph(self, candidates: list[MemoryEntry]) -> list[MemoryEntry]:
        """Expand candidate set by finding memories that share entities with current candidates."""
        seen_ids = {c.id for c in candidates}
        # Collect all entities from current candidates
        all_entities = set()
        for c in candidates:
            for entity, _rel in self.store.get_entities(c.id):
                all_entities.add(entity)

        # For each entity, find related entities (1 hop) and their memories
        expanded_entities = set(all_entities)
        for entity in all_entities:
            related = self.store.get_related_entities(entity, hops=1)
            expanded_entities.update(related)

        # Fetch memories for all entities
        new_candidates = []
        for entity in expanded_entities:
            for entry in self.store.search_by_entity(entity):
                if entry.id not in seen_ids:
                    seen_ids.add(entry.id)
                    new_candidates.append(entry)

        return candidates + new_candidates

    def _score_candidates(
        self,
        candidates: list[MemoryEntry],
        context_keywords: Optional[list[str]],
        has_query: bool = False,
    ) -> list[SearchResult]:
        """Score each candidate using ACT-R activation + confidence."""
        now = time.time()
        results = []

        for entry in candidates:
            # Ensure access_times are populated
            if not entry.access_times:
                entry.access_times = self.store.get_access_times(entry.id)

            # ACT-R activation (base-level + spreading + importance)
            act_score = retrieval_activation(
                entry,
                context_keywords=context_keywords,
                now=now,
            )

            # Skip unretrievable memories
            if act_score == float("-inf"):
                continue

            # Confidence from forgetting model
            conf = confidence_score(entry, store=None, now=now)
            label = confidence_label(conf)

            # FTS relevance bonus: if we came from FTS, candidates are already
            # relevance-ordered. Use position as a simple relevance proxy.
            # (SQLite FTS5 rank is internal; we approximate with order.)
            relevance = 1.0 if has_query else 0.0

            # Final combined score: ACT-R activation is the primary signal
            score = act_score + (0.5 * relevance)

            results.append(SearchResult(
                entry=entry,
                score=score,
                confidence=conf,
                confidence_label=label,
                relevance=relevance,
            ))

        return results

    def _rank_and_filter(
        self,
        scored: list[SearchResult],
        limit: int,
        min_confidence: float,
    ) -> list[SearchResult]:
        """Sort by score, apply min_confidence filter, return top-k."""
        if min_confidence > 0:
            scored = [r for r in scored if r.confidence >= min_confidence]

        scored.sort(key=lambda r: r.score, reverse=True)
        return scored[:limit]


if __name__ == "__main__":
    import time as _time

    print("=== SearchEngine Demo ===\n")

    store = SQLiteStore()
    engine = SearchEngine(store)

    # Add diverse memories
    m1 = store.add("SaltyHall uses Supabase for its backend", MemoryType.FACTUAL, importance=0.5)
    m2 = store.add("On Feb 2 we shipped the memory prototype", MemoryType.EPISODIC, importance=0.7)
    m3 = store.add("potato prefers action over discussion", MemoryType.RELATIONAL, importance=0.8)
    m4 = store.add("Always use www.moltbook.com not moltbook.com", MemoryType.PROCEDURAL, importance=0.6)
    m5 = store.add("I think graph plus text hybrid is the best approach", MemoryType.OPINION, importance=0.4)
    m6 = store.add("potato said I kinda like you after the late night session", MemoryType.EMOTIONAL, importance=0.9)

    # Simulate extra accesses for m3 (frequently recalled)
    for _ in range(5):
        store.record_access(m3.id)

    # 1. Keyword search
    print("--- FTS search: 'Supabase' ---")
    for r in engine.search("Supabase"):
        print(f"  [{r.confidence_label:10s}] score={r.score:.3f} conf={r.confidence:.3f} | {r.entry.content[:60]}")

    # 2. Context-based search (no query, just context keywords)
    print("\n--- Context search: ['potato', 'preferences'] ---")
    for r in engine.search(context_keywords=["potato", "preferences"], limit=3):
        print(f"  [{r.confidence_label:10s}] score={r.score:.3f} conf={r.confidence:.3f} | {r.entry.content[:60]}")

    # 3. Type-filtered search
    print("\n--- All procedural memories ---")
    for r in engine.search(types=["procedural"]):
        print(f"  [{r.confidence_label:10s}] score={r.score:.3f} conf={r.confidence:.3f} | {r.entry.content[:60]}")

    # 4. Full ranking (no query, no filters)
    print("\n--- All memories ranked by activation ---")
    for r in engine.search(limit=10):
        print(f"  [{r.confidence_label:10s}] score={r.score:.3f} conf={r.confidence:.3f} | {r.entry.content[:60]}")

    # 5. With min_confidence filter
    print("\n--- High confidence only (min_confidence=0.5) ---")
    for r in engine.search(min_confidence=0.5, limit=10):
        print(f"  [{r.confidence_label:10s}] score={r.score:.3f} conf={r.confidence:.3f} | {r.entry.content[:60]}")

    print("\n=== Done ===")
