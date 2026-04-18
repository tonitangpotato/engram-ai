# FEAT-003: Memory Lifecycle — Requirements

> Feature: Memory Lifecycle Management
> Priority: P1
> Status: Partial (4 of 9 components implemented)
> Date: 2026-04-16

## Overview

Memory lifecycle covers everything that happens to a memory after initial storage: deduplication at write time, consolidation (clustering + synthesis), decay over time, and periodic rebalancing. The goal is a self-maintaining memory system that stays relevant, compact, and high-signal without manual intervention.

## Scope

**In scope (9 components):**
- C1: Synthesis Gate (✅ implemented)
- C2: Dedup / Near-Duplicate Detection (⚠️ partial — embedding dedup exists, semantic dedup missing)
- C3: Memory Merge / Reconciliation (⚠️ partial — `merge_memory_into` exists, reconciliation logic missing)
- C4: Cluster-Based Synthesize (⚠️ partial — clustering + insight generation exist, incremental updates missing)
- C5: Insight Synthesis / Prompt Templates (✅ implemented)
- C6: Entity Extraction & Indexing (✅ implemented)
- C7: Multi-Retrieval Fusion (✅ implemented)
- C8: Temporal Decay & Forgetting (❌ not implemented — only Hebbian link decay exists)
- C9: Memory Rebalancing (❌ not implemented)

**Out of scope:**
- Initial memory storage (handled by `add_raw` / `store_memory`)
- Embedding generation and vector search internals
- Event bus / notification system (FEAT-002)
- Interoceptive / insula layer (FEAT-001)
- Cognitive theory extensions (FEAT-004)

---

## Requirements

### §1 — Dedup & Near-Duplicate Detection (C2)

**GOAL-1**: When a new memory is stored, the system MUST check for near-duplicate existing memories before creating a new record.

- Near-duplicate = embedding cosine similarity ≥ configured threshold (default: 0.92)
- If a near-duplicate is found, the system merges into the existing record (see §2) instead of creating a new one
- Dedup MUST be toggleable via `MemoryConfig.dedup_enabled` (default: true)
- Dedup MUST respect namespace isolation — only compare within the same namespace

**GOAL-2**: The system MUST support semantic dedup beyond pure embedding similarity.

- Two memories with different wording but identical meaning should be detected as duplicates
- Semantic dedup uses entity overlap as a secondary signal: if two memories share ≥70% of extracted entities AND embedding similarity ≥ 0.85, treat as near-duplicate
- This prevents the "same fact, different phrasing" accumulation problem

**GOAL-3**: Dedup lookup SHOULD complete within 50ms (p95) for databases up to 100k memories on commodity hardware.

- The system uses the existing `find_nearest_embedding` (vector similarity search) as the primary dedup mechanism
- No full-table scan — dedup is index-driven
- A benchmark test MUST exist to verify this target under controlled conditions

**As-is status**: Embedding-based dedup exists in `storage.rs` (`find_nearest_embedding` + `merge_memory_into`). Config flag `dedup_enabled` works. Namespace isolation works. 22 tests in `dedup_test.rs`. **Missing**: semantic dedup via entity overlap (GOAL-2).

---

### §2 — Memory Merge & Reconciliation (C3)

**GOAL-4**: When merging a duplicate into an existing memory, the system MUST preserve the richer content.

- If new content is >30% longer than existing, replace content with new version
- If new content is shorter but was created more recently (within 1 hour) and has higher importance, replace content with new version
- If neither condition applies, append metadata noting the alternative phrasing (capped at 3 alt entries)
- Always take `importance = MAX(existing, new)`
- Record merge history in metadata (capped at 10 entries)
- Increment `merge_count` on the existing record
- Add an access log entry to boost activation

**GOAL-5**: The system MUST support explicit reconciliation — merging two arbitrary memories by ID.

- `reconcile(memory_id_a, memory_id_b)` → keeps A, merges B's content/metadata into A, deletes B
- Reconciliation MUST update all references: entity links, Hebbian links, access logs, insight provenance
- This enables both automated (dedup-triggered) and manual (user/agent-triggered) merging

**GOAL-6**: After any merge, the system MUST re-extract entities from the merged content.

- Merged content may contain new entities not present in either original
- Entity links for the deleted record must be transferred to the surviving record
- Hebbian links referencing the deleted record must be redirected to the surviving record

**As-is status**: `merge_memory_into` in `storage.rs` handles GOAL-4 (content replacement, importance MAX, merge history, merge count, access log). 22 tests cover this. **Missing**: GOAL-5 (reconcile by ID) and GOAL-6 (post-merge entity re-extraction).

---

### §3 — Synthesis Gate (C1)

**GOAL-7**: Before running cluster-based synthesis, the system MUST evaluate whether synthesis is justified via a cost/benefit gate.

- Gate checks: minimum cluster size (≥3 memories), minimum total importance, minimum Hebbian connectivity within cluster
- Gate outputs: `Proceed`, `Skip` (not enough signal), or `Defer` (wait for more data)
- Gate MUST be deterministic — same inputs always produce same decision
- Gate parameters MUST be configurable via `SynthesisConfig`

**GOAL-8**: The gate MUST estimate LLM cost before proceeding.

- `estimate_cost(members)` returns estimated token count based on content length
- If estimated cost exceeds configured budget, gate returns `Skip` with reason

**As-is status**: ✅ Fully implemented in `synthesis/gate.rs`. `check_gate()` evaluates cluster quality, `estimate_cost()` estimates tokens. 685 lines, tested via synthesis integration tests.

---

### §4 — Cluster Discovery & Synthesis (C4)

**GOAL-9**: The system MUST discover clusters of related memories using multi-signal similarity.

- Four signals combined with configurable weights:
  - Hebbian co-activation strength (default weight: 0.4)
  - Entity overlap / Jaccard similarity (default weight: 0.3)
  - Embedding cosine similarity (default weight: 0.2)
  - Temporal proximity (default weight: 0.1)
- Clustering uses single-linkage agglomerative algorithm with configurable threshold
- Minimum cluster size: 3 (configurable)
- Emotional modulation: clusters with high emotional valence get boosted priority

**GOAL-10**: For each valid cluster (passes gate), the system MUST generate a synthesized insight.

- Insight = a new memory record with `memory_type = "insight"` and `is_synthesis = true`
- Insight content is generated via LLM using a template selected by cluster characteristics (see §5)
- Insight importance = computed from source memories' importance distribution
- Insight MUST record full provenance: which source memories, which cluster, what similarity scores

**GOAL-11**: Synthesis MUST be incremental — don't re-synthesize clusters that haven't changed.

- Track a `last_synthesized_at` timestamp per cluster fingerprint
- If no new memories have been added to a cluster's neighborhood since last synthesis, skip it — running `sleep_cycle()` again with no new data MUST result in zero LLM calls
- If a cluster has grown (new members since last synthesis), re-synthesize with full membership — only clusters containing new memories trigger LLM calls
- A benchmark test MUST exist demonstrating zero LLM calls on a stable database after initial synthesis

**GOAL-12**: The system MUST support a `sleep_cycle()` operation that runs the full consolidation pipeline.

- Sleep cycle = discover clusters → gate each → synthesize passing clusters → store insights → decay Hebbian links → optionally run forget()
- Pipeline order ensures synthesis uses pre-decay link strengths
- Sleep cycle operates on a configurable time window (default: memories from last 7 days)
- Sleep cycle MUST be idempotent — running twice with no new data produces no new insights
- Sleep cycle MUST emit progress events for observability (e.g., "synthesizing cluster 3/7")

**As-is status**: Clustering (836 lines, `synthesis/cluster.rs`) and the synthesis engine (`synthesis/engine.rs`) are implemented. `sleep_cycle()` exists in `memory.rs`. **Missing**: GOAL-11 (incremental synthesis — `IncrementalState` is defined in types but not wired).

---

### §5 — Insight Synthesis & Prompt Templates (C5)

**GOAL-13**: The system MUST select an appropriate prompt template based on cluster characteristics.

- Template selection criteria: dominant memory type, entity overlap pattern, temporal spread
- At minimum, support these templates: `CommonThread` (shared theme), `Contradiction` (conflicting memories), `Evolution` (same topic over time), `CrossDomain` (connecting different areas)
- Template selection MUST be deterministic given the same cluster

**GOAL-14**: Generated insights MUST be validated before storage.

- Validation checks: non-empty content, reasonable length (50-2000 chars), no hallucinated entity references
- Invalid insights are logged and discarded, not stored
- Validation failure rate MUST be tracked as a metric

**GOAL-15**: Insight importance MUST be computed algorithmically, not copied from sources.

- `compute_insight_importance(sources)` uses: mean source importance, source count bonus, entity diversity bonus, temporal span bonus
- Result is clamped to [0.0, 1.0]

**As-is status**: ✅ Fully implemented in `synthesis/insight.rs`. 685 lines. `select_template()`, `build_prompt()`, `call_llm()`, `validate_output()`, `compute_insight_importance()` all exist. 4 prompt templates defined. Tested via synthesis integration tests.

---

### §6 — Entity Extraction & Indexing (C6)

**GOAL-16**: On every memory write, the system MUST extract named entities and store them in a dedicated entity index.

- Entity types: Project, Person, Technology, Concept, Organization, Location, Event
- Extraction uses Aho-Corasick pattern matching (fast, rule-based) + regex fallback
- Entities are upserted (deduplicated by normalized name + type)
- Each memory↔entity link is recorded in `memory_entities` table

**GOAL-17**: The system MUST maintain entity co-occurrence relationships.

- When two entities appear in the same memory, the system MUST automatically record a co-occurrence link during entity extraction (not via external API call)
- Co-occurrence links have a strength that increments with each shared memory
- Co-occurrence cap: max 50 links per entity pair (prevent runaway on high-frequency pairs)

**GOAL-18**: The system MUST support entity-based recall as a retrieval signal.

- `entity_recall(query, namespace, limit)` extracts entities from the query, looks up matching entity records, returns memories linked to those entities with relevance scores
- Direct entity match = score 1.0; 1-hop related entity = score 0.5
- Scores MUST preserve relative ordering: a memory found via direct match MUST always rank higher than a memory found only via 1-hop match
- Normalization MUST use a fixed maximum (e.g., `number_of_query_entities * 1.5`) rather than normalizing by actual max in results — normalizing by actual max destroys the direct vs 1-hop signal differentiation
- Entity recall is used as one signal in multi-retrieval fusion (§7)

**GOAL-19**: The system MUST support backfilling entities for memories stored before entity extraction was enabled.

- `backfill_entities(batch_size)` processes memories without entity links
- Backfill is idempotent — running again on already-processed memories is a no-op

**As-is status**: ✅ Mostly implemented. `entities.rs` (637 lines) + entity methods in `storage.rs` (300+ lines). 26 tests in `entity_integration_test.rs`. Backfill exists. **Missing**: automatic co-occurrence link creation on memory write (GOAL-17) — current `entity_relations` only populated by explicit API calls. **Known issue**: `entity_recall` in `memory.rs` (line 1097) has a normalization bug — 1-hop results normalize to 1.0 when no direct matches exist, making them indistinguishable from direct matches (see §8 Known Issues).

---

### §7 — Multi-Retrieval Fusion (C7)

**GOAL-20**: Memory recall MUST fuse multiple retrieval signals into a single ranked result.

- Signals: embedding similarity, FTS5 text match, Hebbian activation, entity recall, recency boost
- Each signal produces a `(memory_id, score)` map normalized to [0.0, 1.0]
- Final score = weighted combination of all signals (weights configurable)
- Default weights: embedding 0.35, FTS5 0.25, Hebbian 0.20, entity 0.15, recency 0.05

**GOAL-21**: Fusion MUST support query classification to dynamically adjust signal weights.

- `classify_query(query)` determines query type: factual, episodic, procedural, emotional, exploratory
- Each query type has a preferred signal weight profile
- Example: factual queries boost FTS5 + entity; episodic queries boost recency + Hebbian

**GOAL-22**: The fused recall MUST return results within 100ms (p95) for databases up to 100k memories on commodity hardware.

- No signal should block the pipeline — if entity recall times out, proceed with remaining signals
- Results are streamed (each signal contributes independently, fusion happens at the end)
- A benchmark test MUST exist to verify this target under controlled conditions

**As-is status**: ✅ Implemented. `hybrid_search.rs` (468 lines) handles multi-signal fusion. `query_classifier.rs` handles query type detection. Entity recall integrated as a signal. Namespace-aware.

---

### §8 — Temporal Decay & Forgetting (C8)

**GOAL-23**: The system MUST apply time-based activation decay to memories following ACT-R power law.

- Activation = base_level + Σ(recency_boost) where each access contributes t^(-d), d = decay rate (default: 0.5)
- Memories with activation below a configurable threshold become candidates for forgetting
- Decay is computed lazily on recall (not eagerly updated) — activation is a function of access_log timestamps, not a stored field

**GOAL-24**: The system MUST support a two-tier `forget()` operation for low-activation memories.

- Tier 1 — `forget()` = soft delete (archive):
  - Identifies memories below the activation threshold with no accesses in configurable period (default: 90 days)
  - Moves memories to `MemoryLayer::Archive` (existing behavior)
  - Cleans up stale references: entity links, Hebbian links, and access logs for archived memories
  - Forget MUST NOT archive insights (synthesized memories) — they represent distilled knowledge
  - Forget MUST NOT archive memories with high importance (≥ 0.8) regardless of activation
  - Forget MUST return a summary: how many memories archived, references cleaned, total storage estimate freed
- Tier 2 — hard delete is handled by `compact()` (GOAL-27), which permanently removes archived memories past retention period

**GOAL-24a**: When forgetting (archiving) a memory that is referenced as a source in insight provenance, the system MUST update the insight's provenance to mark that source as archived.

- If ALL source memories of an insight have been archived/forgotten, the insight MUST be flagged for review (add a `provenance_stale: true` metadata flag) but NOT automatically deleted
- Flagged insights are surfaced by `memory_health()` (GOAL-28) as requiring human/agent review

**GOAL-25**: Hebbian link decay MUST be applied periodically to prevent unbounded link accumulation.

- `decay_hebbian_links(factor)` multiplies all link strengths by `factor` (e.g., 0.95)
- Links with strength below a minimum threshold (default: 0.01) are pruned entirely
- Hebbian decay should run as part of `sleep_cycle()` or standalone

**As-is status**: `decay_hebbian_links` exists in `storage.rs` (GOAL-25 partially done). ACT-R base-level activation is computed in recall path. **Missing**: GOAL-23 (formal decay model as a standalone operation), GOAL-24 (`forget()` with full cleanup logic — current `forget` in CLI only does basic pruning without entity/Hebbian/insight cleanup).

---

### §9 — Memory Rebalancing (C9)

**GOAL-26**: The system MUST support periodic rebalancing of the memory store to maintain quality distribution.

- Rebalance checks:
  - Namespace balance: no single namespace exceeds 60% of total memories
  - Type balance: ensure factual, episodic, procedural types are all represented
  - Importance distribution: flag if >50% of memories have importance < 0.3 (suggests quality problem)
  - Entity coverage: flag memories with 0 entity links (suggests extraction gaps)
- Rebalance outputs a diagnostic report, not automatic deletions

**GOAL-27**: Rebalance MUST support a "compact" mode that actively reduces memory count.

- Compact targets: memories with merge_count = 0, importance < 0.3, no Hebbian links, activation below threshold
- Compact proposes a deletion list — requires explicit confirmation before executing
- Compact MUST preserve at minimum 1 memory per entity (no orphan entities after compact)

**GOAL-28**: Rebalance MUST track memory store health metrics over time.

- Metrics: total count, average importance, average activation, entity coverage %, insight ratio, dedup hit rate
- Metrics are stored with timestamps for trend analysis
- `memory_health()` returns current snapshot; `memory_health_trend(days)` returns time series

**As-is status**: ❌ Not implemented. `engram stats` CLI command provides basic counts but no rebalancing logic, no health metrics tracking, no compact operation.

---

## Guards (Cross-Cutting Constraints)

**GUARD-1**: All lifecycle operations MUST respect namespace isolation. A consolidation in namespace "agent-a" MUST NOT touch memories in namespace "agent-b".

**GUARD-2**: No lifecycle operation may delete user data without explicit opt-in. `forget()` and `compact()` require either a config flag (`auto_forget: true`) or explicit API call. Default is diagnostic-only.

**GUARD-3**: All lifecycle operations MUST be idempotent. Running `sleep_cycle()`, `forget()`, or `rebalance()` twice with no new data MUST produce identical state.

**GUARD-4**: LLM calls (synthesis, semantic dedup) MUST have configurable timeouts and budget caps. The system MUST degrade gracefully if LLM is unavailable — skip synthesis, fall back to embedding-only dedup.

**GUARD-5**: All lifecycle operations MUST support event emission via a configurable callback or trait. If no event bus is configured, operations proceed silently. Event types include: `MemoryMerged`, `InsightCreated`, `MemoryForgotten`, `RebalanceCompleted`. This decouples lifecycle from FEAT-002 — when the event bus lands, it plugs into this trait.

**GUARD-6**: Performance budget: no single lifecycle operation (except full sleep_cycle) should block the main thread for >1 second. Sleep cycle may take longer but MUST yield periodically.

---

## Component Dependencies

```
C2 (Dedup) → depends_on → C6 (Entity Extraction) — dedup uses entity overlap
C3 (Merge) → depends_on → C2 (Dedup) — dedup triggers merge
C4 (Synthesize) → depends_on → C1 (Gate) — gate before synthesis
C4 (Synthesize) → depends_on → C5 (Insight Templates) — templates used by synthesis
C7 (Fusion Recall) → depends_on → C6 (Entity Extraction) — entity recall as signal
C8 (Decay) → depends_on → C4 (Synthesize) — decay happens after synthesis
C9 (Rebalance) → depends_on → C8 (Decay) — rebalance uses decay metrics
```

## Implementation Priority

| Priority | Component | Rationale |
|----------|-----------|-----------|
| P0 | C2: Semantic Dedup (GOAL-2) | Prevents quality degradation from duplicate accumulation |
| P0 | C3: Reconcile (GOAL-5, GOAL-6) | Enables both automated and manual memory cleanup |
| P1 | C4: Incremental Synthesis (GOAL-11) | Reduces wasted LLM calls on stable data |
| P1 | C8: Decay & Forget (GOAL-23, GOAL-24) | Memory stores grow unbounded without this |
| P2 | C9: Rebalance (GOAL-26-28) | Quality monitoring — important but not urgent |

## Test Coverage Summary

| Component | Test File | Test Count | Coverage |
|-----------|-----------|------------|----------|
| C1 Gate | synthesis_integration_test.rs | ~4 | ✅ Good |
| C2 Dedup | dedup_test.rs | 22 | ✅ Good (embedding dedup) |
| C3 Merge | dedup_test.rs | ~10 | ⚠️ Partial (merge_memory_into only) |
| C4 Synthesize | synthesis_integration_test.rs | 12 | ⚠️ Partial (no incremental tests) |
| C5 Insight | synthesis_integration_test.rs | ~4 | ✅ Good |
| C6 Entity | entity_integration_test.rs | 26 | ✅ Good |
| C7 Fusion | (inline in integration_test.rs) | ~5 | ⚠️ Partial |
| C8 Decay | (none dedicated) | 0 | ❌ Missing |
| C9 Rebalance | (none) | 0 | ❌ Missing |

---

## Known Issues

1. **entity_recall score normalization** — `memory.rs:1097` — systematic bug, not edge case. The normalization divides by actual max in results, which means: (a) a memory found only via 1-hop (0.5) normalizes to 1.0, indistinguishable from direct match; (b) a memory with 1 direct match (1.0) ranks lower than one with 3 direct matches (3.0) after normalization (0.33 vs 1.0). Fix specified in GOAL-18: use fixed maximum normalization.
2. **IncrementalState defined but not wired** — `synthesis/types.rs` defines the struct but `engine.rs` doesn't use it
3. **forget() incomplete** — Current `forget()` archives memories (moves to `MemoryLayer::Archive`) but doesn't clean up entity links, Hebbian links, access logs, or insight provenance references. See GOAL-24 (two-tier model) and GOAL-24a (provenance handling).
4. **No memory health metrics** — No persistent tracking of store quality over time
