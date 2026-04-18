# Requirements: Knowledge Synthesis — Part 2: Integration Layer

> Source demotion, CLI, programmatic API, and configuration — the integration surface.

**Date**: 2026-04-09  
**Issue**: ISS-005  
**Status**: Draft v6 (split from master requirements)  
**Depends on**: Part 1 (Synthesis Engine: GOAL-1 through GOAL-4)  
**Scope**: GOAL-5 through GOAL-8 (integration & access)

---

## 1. Context

Part 1 defines the synthesis engine: cluster discovery (GOAL-1), insight generation (GOAL-2), gate check (GOAL-3), and provenance (GOAL-4). This document covers how synthesized insights integrate with the broader system: how sources are managed after synthesis, how users access synthesis features, and how parameters are configured.

---

## 2. Goals

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

All thresholds configurable through existing config mechanism. See **Implementation Notes §B** (Part 1) for the full parameter list.

---

## 3. Non-Goals

Inherited from Part 1. See master requirements for full list.

Key for this part:
- **NON-GOAL-1**: No real-time synthesis (batch only)
- **NON-GOAL-3**: No automatic scheduling (caller decides when)

---

## 4. Constraints

### GUARD-1: No Data Loss
Never delete source memories. Demotion only. Atomic per-insight (insight + demotions committed together, failure rolls back).

### GUARD-3: Backward Compatibility
Additive only. Existing `consolidate()` unchanged. Users who never invoke synthesis see zero behavior change.

### GUARD-4: No New External Dependencies
Use existing embedding provider and extractor trait. No new crate deps.

### GUARD-5: Insight Identity
Insights stored in same storage system as regular memories. Distinguishable via metadata, not separate tables. Participate in normal recall.

---

## 5. Success Criteria (integration-specific)

- **SC-2**: Insight appears in top-3 recall; ≥50% of demoted sources rank below position 10
- **SC-5**: Zero data loss — memory IDs unchanged; only importance/layer modified for demoted sources
- **SC-6**: Fully reversible — restore archived insight + un-demote sources

---

## 6. Implementation Notes (integration-relevant)

### C. Hebbian Links Between Insights and Sources
After storing an insight, call `record_coactivation()` between the insight and each source memory. This creates bidirectional Hebbian links so that recalling the insight naturally activates its sources, and vice versa. Reuses existing Hebbian infrastructure — one function call per source.

### D. Unified Sleep Cycle
Consider exposing a single `sleep_cycle()` entry point that runs: (1) existing `consolidate()` (trace-level, Murre-Chessa), then (2) `synthesize()` (content-level, this feature). This mirrors the brain's unified sleep consolidation. Implementation is a thin wrapper — one function calling two functions.

### G. Cycle Observability
Each cycle should return: clusters discovered, gate decisions (N synthesize / N auto-update / N skip with reasons), LLM calls made, insights stored, sources demoted, duration. Available via API return value and INFO-level log.

### H. Recall Integration
Insights naturally outrank sources due to higher importance (GOAL-5) and embedding quality. No special recall logic needed — existing hybrid search + ACT-R activation handles ranking. Recall results should indicate result type (insight vs. raw) so callers can filter if desired.

### I. Neuroscience Mapping Reference
| Brain Process | Engram Today | This Feature |
|---|---|---|
| Synaptic consolidation | ✅ Murre-Chessa dual-trace | Keep as-is |
| Schema↔episode links | Partial (Hebbian) | Note C: Hebbian source links |
| Unified sleep cycle | ❌ | Note D: sleep_cycle() wrapper |
| Amygdala priority | Partial | Note A (Part 1) |
