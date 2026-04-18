# Requirements: Cognitive Consolidation — Knowledge Synthesis

> From "memory strengthening" to "knowledge creation" — the missing layer in engram's consolidation architecture.

**Date**: 2026-04-09  
**Issue**: ISS-005  
**Status**: Draft v6 (scope-trimmed)  
**Neuroscience Basis**: Complementary Learning Systems (CLS) theory, Systems Consolidation, Schema Theory

---

## 1. Problem Statement

Engram implements **synaptic consolidation** — Murre-Chessa dual-trace decay, ACT-R activation, Hebbian co-activation. This models the hippocampal→neocortical transfer at the *trace* level: numbers go up, numbers go down, memories migrate between layers.

What's missing is **systems consolidation** — the brain's process of examining memory *content* during sleep replay, discovering patterns across memories, and forming higher-order schemas. Concretely:

- 50 memories about Rust debugging stay as 50 separate memories — never distilled into "Rust debugging principles"
- Related facts ("potato prefers action", "potato says don't simplify", "potato wants to think before coding") never merge into a coherent user model
- Memory count grows monotonically; signal-to-noise ratio degrades over time
- The existing `consolidate()` has zero awareness of what memories *say*, only their numerical parameters

Industry approaches (Hindsight, Mem0) solve this by calling LLM on every write — 100% LLM invocation rate. We can do better by leveraging engram's existing cognitive signals (embedding, entity, ACT-R, Hebbian, FTS) to filter first, calling LLM only when genuine knowledge synthesis is needed.

---

## 2. Goals

### GOAL-1: Cluster Discovery (Sleep Replay)

The system must identify groups of memories that are candidates for synthesis. Cluster discovery models hippocampal replay — recent, strongly-associated memories are replayed first, and patterns emerge from activation overlap.

**Clustering signals**, ordered by priority:

1. **(Primary) Hebbian co-activation**: Connected by Hebbian links with strength ≥ configurable threshold (default 0.3). Two memories that were recalled together (fired together) are the strongest candidates for shared schema formation.
2. **(Secondary) Embedding similarity**: Cosine similarity within the cluster exceeds configurable threshold (default 0.75). Confirms semantic relatedness and catches memories that haven't been co-recalled yet.
3. **(Tertiary) Entity overlap**: Share ≥2 entity references. Catches factual relationships that embedding/Hebbian might miss.

A cluster is ≥N memories (configurable, default 3) satisfying **any** of these signals. Hebbian-primary clusters rank above embedding-primary, which rank above entity-only.

**Recency bias**: Clusters containing ≥1 recently-active memory (created/accessed within last K consolidation cycles, default 3) are processed before clusters composed entirely of older memories. Old memories are not excluded — just deprioritized.

Each memory may belong to multiple overlapping clusters. Filtering for existing coverage is handled by GOAL-3 (gate check), not at discovery.

### GOAL-2: Insight Generation (Schema Formation)

Given a cluster, the system must produce an **insight** — a new memory capturing the higher-order pattern, principle, or summary. Validation checks before storing:

- **(a)** Semantically distinct from any single source (embedding similarity to any source < 0.95)
- **(b)** Content length ≥ median of source content lengths (prevents degenerate truncation)
- **(c)** Actionable — an agent reading only the insight can make better decisions than reading any single source

If validation fails, the cluster is skipped and logged. Invalid LLM responses (empty, off-topic with centroid similarity < 0.5) are treated as validation failures.

### GOAL-3: Gate Check (CLS Fast/Slow Path)

Before invoking LLM for any cluster, a **zero-LLM gate check** using existing signals determines if synthesis is needed:

- **Near-duplicates**: average intra-cluster embedding similarity > 0.92 → SKIP
- **Too recent**: all memories created within same 1-hour window → DEFER
- **Already covered**: existing insight covers ≥80% of cluster members → SKIP
- **Cluster growth**: existing insight covers cluster but cluster grew ≥50% → RE-SYNTHESIZE

Classification output per cluster:
- **SYNTHESIZE** — genuine pattern, send to LLM
- **AUTO-UPDATE** — high overlap with existing insight, boost numerically without LLM
- **SKIP** — no synthesis needed

### GOAL-4: Source Traceability (Provenance)

Every insight must reference its source memory IDs. Must support:
- **Forward**: insight ID → source memories
- **Reverse**: memory ID → insights derived from it
- **Chain depth**: insights from insights (for future multi-level, see NON-GOAL-2)

O(1) per-insight lookups in both directions.

### GOAL-5: Source Demotion (Not Deletion)

After successful synthesis and validation:
- Source importance × configurable factor (default 0.5)
- Source layer moved toward archive
- Insight importance ≥ max(source importances before demotion)
- Sources **never deleted** — evidence chain preserved
- **Atomicity**: if synthesis fails mid-cycle, no sources modified

Insights participate in ALL existing memory dynamics after creation: ACT-R activation, Ebbinghaus forgetting, Murre-Chessa trace transfer, Hebbian co-activation. No special-casing — insights are memories, not privileged artifacts.

### GOAL-6: CLI Access

Expose synthesis through CLI:
- Trigger a synthesis cycle
- Dry-run: preview clusters and candidates without changes
- List all insights with source counts
- Inspect a specific insight and its sources

### GOAL-7: Programmatic API

Library API callable from Rust code:
- Run synthesis cycle, return structured results summary
- Query all insights
- Query provenance (sources for insight, insights for source)
- Recall results indicate insight vs. raw memory via `memory_type` or metadata

### GOAL-8: Configurable Parameters

All thresholds configurable through existing config mechanism. See **Implementation Notes §B** for the full parameter list.

---

## 3. Non-Goals

### NON-GOAL-1: Real-Time Synthesis
Synthesis does not run on every `add()` or `recall()`. Batch operation only.

### NON-GOAL-2: Multi-Level Synthesis (Schema Hierarchy)
Single-level only (memories → insights). Multi-level (insights → schemas → worldviews) is planned but out of scope. GOAL-4 provenance chains must not prevent future extension.

### NON-GOAL-2a: Context-Dependent Clustering
Batch synthesis is context-free — no "current query" to drive activation. Accepted limitation for v1.

### NON-GOAL-3: Automatic Scheduling
No background scheduler. Caller decides when to run (agent heartbeat, cron, manual).

### NON-GOAL-4: Cross-Namespace Synthesis
Within single namespace only. Hebbian cross-links handle cross-namespace at trace level.

### NON-GOAL-5: Contradiction Resolution
Insights note contradictions but don't resolve them.

---

## 4. Constraints

### GUARD-1: No Data Loss
Never delete source memories. Demotion only. Atomic per-insight (insight + demotions committed together, failure rolls back).

### GUARD-2: LLM Cost Control
Hard cap on LLM calls per cycle (configurable, default 5). Clusters prioritized by: (1) emotional significance, (2) size, (3) temporal spread.

### GUARD-3: Backward Compatibility
Additive only. Existing `consolidate()` unchanged. Users who never invoke synthesis see zero behavior change.

### GUARD-4: No New External Dependencies
Use existing embedding provider and extractor trait. No new crate deps.

### GUARD-5: Insight Identity
Insights stored in same storage system as regular memories. Distinguishable via metadata, not separate tables. Participate in normal recall.

### GUARD-6: Existing Signal Reuse
Gate check uses only existing infrastructure: embeddings, entities, Hebbian, ACT-R, FTS. No new ML models.

---

## 5. Success Criteria

- **SC-1**: 100+ memories on a topic → synthesis produces ≥1 insight capturing a pattern not in any single source
- **SC-2**: Insight appears in top-3 recall; ≥50% of demoted sources rank below position 10
- **SC-3**: Gate check filters ≥60% of clusters without LLM (over 10 cycles on real data)
- **SC-4**: Cluster discovery <60s for 10K memories; full cycle (ex-LLM) <120s for 10K memories
- **SC-5**: Zero data loss — memory IDs unchanged; only importance/layer modified for demoted sources
- **SC-6**: Fully reversible — restore archived insight + un-demote sources

---

## 6. Implementation Notes

> These are not requirements — they are design hints and details captured during requirements review. The design phase should consider these but is free to implement differently.

### A. Emotional Modulation
When EmotionalBus detects strong signal (|valence| > 0.7) in a domain, clusters in that domain should get synthesis priority (processed first). Resulting insights inherit emotional importance boost. Future extensions: emotional significance could modulate quality thresholds, initial importance, and decay resistance (amygdala-mediated consolidation enhancement).

### B. Full Parameter List
Expected configurable parameters (defaults in parens):
- Min cluster size (3), embedding similarity threshold (0.75), Hebbian link threshold (0.3)
- Gate similarity threshold for auto-skip (0.92), temporal spread minimum (1 hour)
- Max memories per LLM call (10), max cluster size (15, split if exceeded)
- Max insights per cycle (5), source demotion factor (0.5)
- Re-synthesis growth threshold (50%), re-synthesis age threshold (30 days)
- Recency boost window (3 consolidation cycles)

### C. Hebbian Links Between Insights and Sources
After storing an insight, call `record_coactivation()` between the insight and each source memory. This creates bidirectional Hebbian links so that recalling the insight naturally activates its sources, and vice versa. Reuses existing Hebbian infrastructure — one function call per source.

### D. Unified Sleep Cycle
Consider exposing a single `sleep_cycle()` entry point that runs: (1) existing `consolidate()` (trace-level, Murre-Chessa), then (2) `synthesize()` (content-level, this feature). This mirrors the brain's unified sleep consolidation. Implementation is a thin wrapper — one function calling two functions.

### E. Incremental Operation
Cluster discovery may scan full DB, but LLM calls should fire only for new/changed clusters (≥1 member added since last synthesis). Track "processed" status via provenance — a memory is processed if it's a source of any non-archived insight. No separate "processed" flag needed.

### F. LLM Graceful Degradation
When no LLM provider is configured: cluster discovery and gate check complete normally; synthesis phase is skipped (not errored); result indicates "LLM unavailable"; warning log emitted.

### G. Cycle Observability
Each cycle should return: clusters discovered, gate decisions (N synthesize / N auto-update / N skip with reasons), LLM calls made, insights stored, sources demoted, duration. Available via API return value and INFO-level log.

### H. Recall Integration
Insights naturally outrank sources due to higher importance (GOAL-5) and embedding quality. No special recall logic needed — existing hybrid search + ACT-R activation handles ranking. Recall results should indicate result type (insight vs. raw) so callers can filter if desired.

### I. Neuroscience Mapping Reference
| Brain Process | Engram Today | This Feature |
|---|---|---|
| Synaptic consolidation | ✅ Murre-Chessa dual-trace | Keep as-is |
| Sleep replay (content) | ❌ | GOAL-1: Cluster Discovery |
| Pattern completion (schema) | ❌ | GOAL-2: Insight Generation |
| Schema↔episode links | Partial (Hebbian) | Note C: Hebbian source links |
| Reconsolidation | Partial (ACT-R boost) | GOAL-3: Gate re-synthesis |
| CLS fast/slow path | ❌ | GOAL-3: Gate Check |
| Amygdala priority | Partial | Note A: Emotional modulation |
| Unified sleep cycle | ❌ | Note D: sleep_cycle() wrapper |
