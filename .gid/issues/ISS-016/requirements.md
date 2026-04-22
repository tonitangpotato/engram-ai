# Requirements: ISS-016 — LLM Triple Extraction for Hebbian Link Quality

## Overview

The Hebbian association module computes link strength from three signals: entity overlap, embedding cosine similarity, and temporal proximity. Entity overlap currently relies solely on hardcoded Aho-Corasick dictionary terms, causing embedding cosine to dominate and producing links between "vaguely similar" rather than "meaningfully related" memories. This feature adds LLM-extracted (Subject, Predicate, Object) triples as an enrichment layer that runs during consolidation (not on the hot write path), feeding discovered entities back into the Hebbian entity_overlap signal for higher-quality association links.

## Priority Levels

- **P0**: Core — required for the system to function at all
- **P1**: Important — needed for production-quality operation
- **P2**: Enhancement — improves efficiency, UX, or observability

## Guard Severity

- **hard**: Violation = system is broken, execution must stop
- **soft**: Violation = degraded quality, should warn but can continue

## Goals

### Triple Storage

- **GOAL-1.1** [P0]: Triples are persisted durably — after a successful extraction and store, triples survive process restarts and are queryable by memory ID, subject, object, or predicate *(ref: ISS-016, Storage)*
- **GOAL-1.2** [P0]: Each triple carries a subject, predicate, object, confidence score (0.0–1.0), and source provenance (e.g. llm, rule, manual), all queryable independently *(ref: ISS-016, What are triples?)*
- **GOAL-1.3** [P1]: Duplicate triples for the same memory (identical subject + predicate + object) are rejected without error — storing the same triple twice is idempotent. Deleting a memory cascades to its triples *(ref: ISS-016, Storage / UNIQUE constraint)*
- **GOAL-1.4** [P0]: Storage schema changes are applied via migration that preserves all existing data — upgrading from a database without the triples table to one with it requires no manual intervention *(ref: ISS-016, Storage)*

### LLM Triple Extraction

- **GOAL-2.1** [P0]: Given a memory's text content, the system produces zero or more (subject, predicate, object) triples with a confidence score per triple *(ref: ISS-016, Root Fix: LLM-Extracted Triples)*
- **GOAL-2.2** [P1]: Extraction constrains predicates to a defined vocabulary (`is_a`, `part_of`, `uses`, `depends_on`, `caused_by`, `leads_to`, `implements`, `contradicts`, `related_to`) — unrecognized predicates are mapped to `related_to` as the fallback *(ref: ISS-016, Predicate vocabulary)*
- **GOAL-2.3** [P1]: Extraction handles malformed or empty LLM responses gracefully — parse failures produce zero triples and a logged warning, never a crash or panic *(ref: ISS-016, Design Constraints)*
- **GOAL-2.4** [P1]: Extraction includes few-shot examples in the prompt so the LLM produces consistently structured output across diverse memory content *(ref: ISS-016, Scope / LLM prompt)*

### Consolidation Integration

- **GOAL-3.1** [P0]: Triple extraction runs during the consolidation/sleep cycle, not during memory write — the `store()` hot path latency is unaffected by triple extraction *(ref: ISS-016, Two-layer architecture)*
- **GOAL-3.2** [P0]: Consolidation identifies memories that lack triples and processes them in batches — memories already enriched with triples are skipped. Memories that fail extraction are retried up to 3 times before being marked as permanently failed and skipped in future cycles *(ref: ISS-016, Scope / Async extraction during consolidate())*
- **GOAL-3.3** [P1]: Batch size for per-cycle extraction is configurable (default: 10, minimum: 1), preventing a single consolidation cycle from making an unbounded number of LLM calls *(ref: ISS-016, Scope / Config)*

### Hebbian Signal Enhancement

- **GOAL-4.1** [P0]: Entity overlap computation incorporates entities extracted from triples (subjects and objects) merged at the storage level — `get_entities_for_memory()` returns the union of Aho-Corasick entities and triple-derived entities, so two memories sharing LLM-extracted entities produce a non-zero entity overlap score *(ref: ISS-016, Hebbian signal upgrade)*
- **GOAL-4.2** [P0]: Existing memories that have no triples continue to participate in Hebbian link formation using only their Aho-Corasick entities — the system degrades gracefully when triples are absent *(ref: ISS-016, Two-layer architecture)*

### Configuration

- **GOAL-5.1** [P1]: Triple extraction is independently enable/disable-able in configuration — when disabled, consolidation skips extraction and Hebbian signals use only Aho-Corasick entities *(ref: ISS-016, Scope / Config)*
- **GOAL-5.2** [P2]: The LLM model used for triple extraction is configurable separately from the model used for memory extraction *(ref: ISS-016, Scope / Config / model selection)*

### Concurrency

- **GOAL-6.1** [P0]: Triple extraction during consolidation does not hold the database lock while waiting for LLM responses — the cycle identifies un-enriched memories, releases the lock, performs LLM calls, then re-acquires the lock to store results *(ref: review finding, existing consolidation transaction pattern)*

## Guards

- **GUARD-1** [hard]: No data loss — existing memories, links, and entities are never corrupted or deleted by triple extraction, storage migration, or consolidation changes *(ref: ISS-016, Design Constraints / Two-layer architecture)*
- **GUARD-2** [hard]: Hot path isolation — memory `store()` latency must not increase due to triple extraction; all LLM calls for triples occur exclusively in consolidation/sleep cycle *(ref: ISS-016, Two-layer architecture)*
- **GUARD-3** [soft]: LLM extraction failures are non-fatal — if the LLM is unavailable or returns garbage, consolidation logs the error and continues; memories that failed extraction are retried up to 3 times before being permanently skipped *(ref: ISS-016, Design Constraints)*
- **GUARD-4** [soft]: Backward compatibility — a database with no triples table (pre-migration) opens without error when triple extraction is disabled; enabling extraction triggers migration automatically *(ref: ISS-016, Storage)*

## Out of Scope

- Graph visualization of triples (no UI for exploring the triple graph)
- Triple-based reasoning or inference engine (no transitive closure, no rule chaining)
- Manual triple editing interface (no CRUD API for user-authored triples)
- Cross-memory entity resolution or deduplication (e.g. merging "Rust" and "rust-lang" into one entity)
- Relation overlap as a fourth Hebbian signal (mentioned in issue as optional/future — not required for ISS-016)
- Real-time / streaming triple extraction on the write path

## Dependencies

- **LLM provider** — triple extraction requires an LLM backend (same `MemoryExtractor` trait or equivalent); without one configured, the feature is inert
- **SQLite** — triples table extends the existing storage schema; requires migration support already present in `Storage`
- **Existing Aho-Corasick entity extraction** — remains as the fast-path entity source; triples augment rather than replace it
- **Existing association module** — `SignalComputer::entity_jaccard()` and `LinkFormer::discover_associations()` are the integration points for enriched entity sets

---

**15 GOALs** (8 P0 / 6 P1 / 1 P2) + **4 GUARDs** (2 hard / 2 soft)
