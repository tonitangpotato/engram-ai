# Requirements: Knowledge Synthesis — Part 1: Synthesis Engine

> Core engine: cluster discovery, insight generation, gate check, and provenance tracing.

**Date**: 2026-04-09  
**Issue**: ISS-005  
**Status**: Draft v6 (split from master requirements)  
**Neuroscience Basis**: Complementary Learning Systems (CLS) theory, Systems Consolidation, Schema Theory  
**Scope**: GOAL-1 through GOAL-4 (engine internals)

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

---

## 3. Non-Goals (full feature scope)

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

### GUARD-4: No New External Dependencies
Use existing embedding provider and extractor trait. No new crate deps.

### GUARD-5: Insight Identity
Insights stored in same storage system as regular memories. Distinguishable via metadata, not separate tables. Participate in normal recall.

### GUARD-6: Existing Signal Reuse
Gate check uses only existing infrastructure: embeddings, entities, Hebbian, ACT-R, FTS. No new ML models.

---

## 5. Success Criteria (engine-specific)

- **SC-1**: 100+ memories on a topic → synthesis produces ≥1 insight capturing a pattern not in any single source
- **SC-3**: Gate check filters ≥60% of clusters without LLM (over 10 cycles on real data)
- **SC-4**: Cluster discovery <60s for 10K memories; full cycle (ex-LLM) <120s for 10K memories

---

## 6. Implementation Notes (engine-relevant)

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

### E. Incremental Operation
Cluster discovery may scan full DB, but LLM calls should fire only for new/changed clusters (≥1 member added since last synthesis). Track "processed" status via provenance — a memory is processed if it's a source of any non-archived insight. No separate "processed" flag needed.

### F. LLM Graceful Degradation
When no LLM provider is configured: cluster discovery and gate check complete normally; synthesis phase is skipped (not errored); result indicates "LLM unavailable"; warning log emitted.

### I. Neuroscience Mapping Reference
| Brain Process | Engram Today | This Feature |
|---|---|---|
| Synaptic consolidation | ✅ Murre-Chessa dual-trace | Keep as-is |
| Sleep replay (content) | ❌ | GOAL-1: Cluster Discovery |
| Pattern completion (schema) | ❌ | GOAL-2: Insight Generation |
| CLS fast/slow path | ❌ | GOAL-3: Gate Check |
| Schema↔episode links | Partial (Hebbian) | Note C (Part 2) |
| Reconsolidation | Partial (ACT-R boost) | GOAL-3: Gate re-synthesis |
| Amygdala priority | Partial | Note A: Emotional modulation |
