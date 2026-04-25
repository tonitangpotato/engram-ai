# Requirements: Engram v0.3 — Graph Layer

> **Feature:** v03-graph-layer
> **Master doc:** `.gid/docs/requirements-v03.md` (GUARDs live there; referenced, not restated)
> **Design source:** `docs/DESIGN-v0.3.md` §3 (data model) + §4.5 (extraction failure handling)
> **GOAL namespace:** `GOAL-1.X`

## Overview

The graph layer introduces the static data model that underpins v0.3's semantic graph (L4) and knowledge topic presence (L5). It defines the types that represent real-world entities, their relationships, and the cognitive metadata that makes engram's graph unique — somatic fingerprints, activation, affect, bi-temporal validity. This feature owns the **shape of the data at rest**: what exists, what each thing carries, and what invariants hold on the data itself. It does NOT own the write pipeline (v03-resolution), retrieval queries (v03-retrieval), or migration from v0.2 (v03-migration).

## Priority Levels

- **P0**: Core — required for the graph layer to function at all
- **P1**: Important — needed for production-quality operation
- **P2**: Enhancement — improves observability or future extensibility

## Goals

### Entity model

- **GOAL-1.1** [P0]: Every entity is uniquely identifiable and carries enough metadata to determine its canonical name, known aliases, type category, temporal span (first and last observation), and a human-readable summary — sufficient for resolution, display, and provenance queries without consulting raw episodes. *(ref: DESIGN §3.3, Entity struct)*

- **GOAL-1.2** [P0]: Every entity carries cognitive state — activation level, self-directed affect, arousal, importance, and identity confidence — such that decay, Hebbian strengthening, and affect-modulated resolution can operate on entities the same way they operate on memory records. *(ref: DESIGN §3.3, "engram-unique" fields)*

- **GOAL-1.3** [P0]: Every entity maintains provenance links to the episodes and memory records that mention it, queryable in both directions (entity → mentions, mention → entities). *(ref: DESIGN §3.3, episode_mentions / memory_mentions)*

### Edge model

- **GOAL-1.4** [P0]: Every edge connects a subject entity to either another entity or a literal value, carries a typed predicate, a natural-language summary, and cognitive metadata (activation, confidence, self-directed affect), and is uniquely identifiable. *(ref: DESIGN §3.4, Edge struct)*

- **GOAL-1.5** [P0]: Every edge carries bi-temporal validity — when the fact became true in the real world, when it stopped being true, and when the system learned it — such that "what was believed at time T?" queries are answerable from edge data alone. *(ref: DESIGN §3.4, bi-temporal validity fields)*

- **GOAL-1.6** [P0]: Edge invalidation is non-destructive: superseded edges are marked with a reference to their successor and retain full audit trail. No edge is ever deleted or overwritten in place. Invalidation chains are traversable (old → new and new → old). *(ref: DESIGN §3.4, invalidated_by / supersedes + §1/INV3)*

- **GOAL-1.7** [P1]: Every edge records its provenance — which episode asserted it and, when applicable, which memory record it was extracted from — and how it was resolved (automatically via cheap signals, via LLM tie-breaker, or via agent curation). Provenance is queryable: given an episode, all edges it produced are retrievable; given an edge, its source episode is retrievable. *(ref: DESIGN §3.4, episode_id / memory_id / resolution_method)*

### Predicate schema

- **GOAL-1.8** [P0]: The predicate system distinguishes between canonical relationship types (with known semantics for inverse/symmetric queries and traversal) and novel relationship proposals (preserved verbatim, no information loss). Canonical and novel predicates are distinguishable in all query and display operations. *(ref: DESIGN §3.5, hybrid schema)*

- **GOAL-1.9** [P1]: Novel predicates do not participate in inverse or symmetric graph traversal logic. Only canonical predicates have structural query behavior. Novel predicates are treated as opaque labels for retrieval and future canonicalization. *(ref: DESIGN §3.5, invariant 1 on Proposed)*

- **GOAL-1.10** [P2]: The count of distinct novel predicate strings and their usage frequency is queryable, enabling operators to monitor schema drift and identify candidates for future canonicalization. *(ref: DESIGN §3.5, "Schema inducer — deferred to v0.4")*

### MemoryRecord extensions

- **GOAL-1.11** [P0]: Memory records (L2/L3) are extended — not replaced — with provenance and structural cross-references: each record can link to the episode it originated from and to the entities and edges derived from it. Existing memory record consumers that do not use these new capabilities continue to function without modification. *(ref: DESIGN §3.2, v0.3 new fields + §1/NG5)*

- **GOAL-1.12** [P0]: When graph extraction fails for an episode (LLM error, timeout, rate limit), the failure is recorded as visible, queryable data on the affected episode — including which stage failed, the error category, and when it occurred. No extraction failure is silent. A batch re-extraction command can target all episodes with recorded failures. *(ref: DESIGN §4.5, extraction failure handling + §1/INV1)*

### Somatic fingerprint

- **GOAL-1.13** [P0]: A somatic fingerprint is observable at both the episode level (captured once at write time, immutable thereafter) and the entity level (an aggregate over the fingerprints of episodes that mention the entity, recomputed on new mentions). Both levels use the same dimensionality and semantic layout. *(ref: DESIGN §3.7, "Two fingerprint flavors, same schema")*

### Layer classification

- **GOAL-1.14** [P1]: Memory layer classification (working, core, archived) is derived from existing strength and status fields, not stored as a primary source-of-truth field. Given the same underlying field values, classification yields the same layer across processes and versions. *(ref: DESIGN §2, layer responsibilities + §3.2, MemoryLayer)*

### Knowledge topic layer

- **GOAL-1.15** [P1]: A knowledge topic layer (L5) exists as a typed layer distinct from the episodic layers (L1–L3) and the semantic graph (L4). Topics can link to entities (not just memories), enabling entity-aware topic synthesis and abstract-query routing. *(ref: DESIGN §3.6, Knowledge Topic)*

## Guards

All cross-cutting guards live in the master requirements document:
→ `.gid/docs/requirements-v03.md`

This feature is bound by all 12 GUARDs defined there. Of particular relevance:

- **GUARD-1** (episodic completeness) — GOAL-1.12 is the graph-layer's enforcement surface for this guard
- **GUARD-2** (never silent degrade) — GOAL-1.12 is the graph-layer's enforcement surface for this guard
- **GUARD-3** (bi-temporal never erases) — GOAL-1.6 is the graph-layer's enforcement surface for this guard
- **GUARD-7** (somatic fingerprint schema stability) — GOAL-1.13 depends on this guard's dimensional lock
- **GUARD-8** (episode affect snapshot immutability) — GOAL-1.13's episode-level fingerprint immutability derives from this guard

## Out of Scope

- **Write pipeline logic** — how entities/edges are extracted and resolved (v03-resolution)
- **Query and retrieval logic** — how the graph is searched, ranked, or traversed (v03-retrieval)
- **Migration from v0.2** — schema migration, backfill, rollback (v03-migration)
- **Schema induction** — automatic promotion of novel predicates to canonical (deferred to v0.4, ISS-031)
- **Emotional contagion** — EmpathyState → AffectState flow (deferred to v0.4+, Q8)
- **Consolidation algorithms** — decay, Hebbian, retro-evolution operate on these types but are owned by other features
- **Specific field names, enum variants, or storage formats** — those are design decisions in DESIGN.md §3

## Dependencies

- **engramai v0.2.2** — MemoryRecord and existing cognitive types are the extension point (GOAL-1.11)
- **SQLite (rusqlite)** — existing dependency; graph-layer types must persist to SQLite (GUARD-9 constrains: no new required external dependency)
- **DESIGN-v0.3.md §3** — authoritative design source for all type schemas and field-level decisions

## References

- Master requirements: `.gid/docs/requirements-v03.md`
- Design document: `docs/DESIGN-v0.3.md` (§3, §4.5)

---

**15 GOALs** (10 P0 / 4 P1 / 1 P2) — GUARDs in master doc (12 total: 10 hard / 2 soft)
