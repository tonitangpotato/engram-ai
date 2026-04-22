# ISS-016: LLM Triple Extraction for Hebbian Link Quality

## Problem

Multi-signal Hebbian link formation (ISS-015 outcome) revealed a structural weakness:

- **Entity overlap signal** only fires for hardcoded Aho-Corasick dictionary terms (Rust, Python, React, etc.)
- **Embedding cosine** becomes the dominant signal (weight 0.5) by default
- Result: links form based on "vaguely similar" rather than "meaningfully related"
- Adjusting thresholds is a band-aid — the root cause is **poor entity coverage**

Adding more dictionaries is a dead end — unbounded maintenance, no relationship extraction, no novel concept recognition.

## Root Fix: LLM-Extracted Triples

Introduce `(Subject, Predicate, Object)` triple extraction as an optional LLM-powered enrichment layer.

### What are triples?

The atomic unit of a knowledge graph:

```
("engram", "uses", "ACT-R activation decay")
("Infomap", "is_a", "clustering algorithm")
("gid-core", "depends_on", "Infomap")
("embedding_cosine", "dominates", "entity_overlap")
```

- **Subject** — the entity (who/what)
- **Predicate** — the relationship type (how they relate)
- **Object** — the related entity (to whom/what)

### Why this fixes the problem

1. **Entity discovery** — LLM recognizes "Infomap", "ACT-R", "do-calculus" without dictionary entries
2. **Typed relationships** — not just "related" but `uses` / `is_a` / `caused_by` / `contradicts`
3. **Hebbian signal upgrade** — entity overlap computed on extracted entities (not just dictionary hits) → much stronger, more precise signal
4. **Reasoning** — `(A, uses, B)` + `(B, is_a, Algorithm)` → "A uses an Algorithm" — emergent inference

### Predicate vocabulary (starter set)

| Predicate | Meaning | Example |
|-----------|---------|---------|
| `is_a` | Classification | (Infomap, is_a, clustering_algorithm) |
| `part_of` | Composition | (executor, part_of, harness) |
| `uses` | Dependency/usage | (engram, uses, Hebbian_learning) |
| `depends_on` | Hard dependency | (gid-core, depends_on, petgraph) |
| `caused_by` | Causation | (link_noise, caused_by, embedding_dominance) |
| `leads_to` | Effect | (triple_extraction, leads_to, better_links) |
| `implements` | Realization | (storage.rs, implements, v2_schema) |
| `contradicts` | Conflict | (fast_writes, contradicts, full_validation) |
| `related_to` | Weak/fallback | (topic_A, related_to, topic_B) |

## Design Constraints

### Two-layer architecture (not replacement)

```
Hot path (write time):   Aho-Corasick dictionary → instant entity tags (existing, keep as-is)
Cold path (async/batch): LLM → (subject, predicate, object) triples → stored in new table
```

- Aho-Corasick stays as fast cache for known terms — zero latency, zero cost
- LLM runs asynchronously or during `consolidate()` — not on the hot write path
- Both layers feed into Hebbian link formation

### Integration point

`extractor.rs` already has LLM extraction capability, but outputs flat `ExtractedFact` structs. Change:

```
Before: text → LLM → ExtractedFact { content, category, ... }
After:  text → LLM → Vec<Triple { subject, predicate, object, confidence }>
```

### Storage

New SQLite table:

```sql
CREATE TABLE IF NOT EXISTS triples (
    id INTEGER PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memories(id),
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    confidence REAL DEFAULT 1.0,
    source TEXT DEFAULT 'llm',  -- 'llm' | 'rule' | 'manual'
    created_at TEXT NOT NULL,
    UNIQUE(memory_id, subject, predicate, object)
);

CREATE INDEX idx_triples_subject ON triples(subject);
CREATE INDEX idx_triples_object ON triples(object);
CREATE INDEX idx_triples_predicate ON triples(predicate);
```

### Hebbian signal upgrade

```
Current:  entity_overlap = jaccard(aho_corasick_entities_A, aho_corasick_entities_B)
Proposed: entity_overlap = jaccard(all_entities_A, all_entities_B)
          where all_entities = aho_corasick_entities ∪ triple_entities

New signal (optional):
  relation_overlap = count of shared (predicate, object) pairs between A and B
```

## Scope

- [ ] Triple struct + storage table + migration
- [ ] LLM prompt for triple extraction (few-shot, constrained predicate vocab)
- [ ] Async extraction during `consolidate()` — batch unprocessed memories
- [ ] Integrate extracted entities into Hebbian entity_overlap signal
- [ ] Config: enable/disable, model selection, batch size
- [ ] Tests: extraction quality, storage CRUD, Hebbian integration

## Out of Scope (future)

- Graph visualization of triples
- Triple-based reasoning/inference engine
- Manual triple editing UI
- Cross-memory triple merging (entity resolution / dedup)

## Status: ✅ DONE (2026-04-18, committed 0383584)
