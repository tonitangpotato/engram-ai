# Design Review: design-1-engine.md — Part 2 (§6–§10)

**Reviewer**: Claude Code  
**Date**: 2026-04-14  
**Scope**: Lines 300–699 (§4.5 tail through §10)

---

## Section Numbering

- ✅ Sections flow correctly: §4.5 → §5 → §6 → §7 → §8 → §9 → §10
- ✅ No gaps or duplicate section numbers
- ✅ Sub-sections correctly numbered (§5.1–§5.3, §6.1, §7.1–§7.2, §8.1–§8.2)

## §6 Incremental Operations

- ✅ Clearly describes new-memory-to-existing-cluster flow
- ✅ Staleness thresholds are defined with defaults and have a config struct
- ✅ No silent data loss — old insight remains queryable until replaced
- ⚠️ **Missing from config defaults table**: The `staleness_member_change_pct` and `staleness_quality_delta` fields are actually present in §8.1 table — confirmed OK.
- ⚠️ **Minor**: §6.1 references "cluster_threshold (same threshold as §2.3)" — correct cross-reference, but the value (0.3) is not restated here. Acceptable for DRY but worth noting.

## §7 Public API

- ✅ `SynthesisEngine` trait covers all four GOALs: `discover_clusters` (GOAL-1), `synthesize` (GOAL-2), `check_gate` (GOAL-3), `get_provenance`/`undo_synthesis` (GOAL-4/GUARD-1)
- ✅ `SynthesisReport` includes comprehensive telemetry with `gate_results`, `errors`, `duration`
- ✅ `SynthesisError` enum has 8 variants covering LLM failures, validation failures, storage errors, embedding errors, budget exhaustion, and stale clusters — thorough
- ⚠️ **`clusters_auto_updated` in `SynthesisReport`**: AUTO-UPDATE is a gate decision but §7 doesn't specify what exactly happens for auto-updated clusters. §3.1 defines `AutoUpdateAction` (MergeDuplicates, StrengthenLinks) but no implementation detail for actually executing those actions is provided in any section. Minor gap — could be deferred to implementation spec.

## §8 Configuration

- ✅ Config struct `SynthesisSettings` wraps all sub-configs cleanly
- ✅ Budget enforcement strategy (§8.2) is well-defined with prioritization order
- **Config defaults table completeness check (§8.1)**:
  - ✅ All `ClusterDiscoveryConfig` fields present (weights, threshold, sizes, importance, temporal params, cooldown, temporal_spread_minimum)
  - ✅ All `GateConfig` fields present (quality thresholds, duplicate_similarity, diversity, cost, premium)
  - ✅ All `SynthesisConfig` fields present (model, max_tokens, temperature, prompt_template, max_memories_per_llm_call, resynthesis_age_threshold)
  - ✅ All `EmotionalModulationConfig` fields present
  - ✅ All `IncrementalConfig` fields present
  - ✅ All `SynthesisSettings` top-level fields present (enabled, demotion_factor, max_insights, max_llm_calls)
  - ⚠️ **`max_memories_per_llm_call`** appears in the config defaults table under SynthesisConfig (default: 10, matches §6.B in requirements). However, this field is NOT in the `SynthesisConfig` struct defined in §4.1. The struct only has `model`, `max_tokens`, `temperature`, `prompt_template`. **Missing field in struct definition.**
  - ⚠️ **`resynthesis_age_threshold`** appears in defaults table (default: 30 days) but is also NOT in the `SynthesisConfig` struct in §4.1. **Missing field in struct definition.**
  - ⚠️ **`temporal_spread_minimum`** appears in defaults table under ClusterDiscoveryConfig (default: 1 hour) but is NOT in the `ClusterDiscoveryConfig` struct in §2.5. **Missing field in struct definition.**

## §9 Error Handling (GUARD-6)

- ✅ Failure table is clear and shows "None" data impact for all cases
- ✅ Transaction wrapping (BEGIN/INSERT/UPDATE/COMMIT or ROLLBACK) is explicit
- ✅ Consistent with GUARD-1 (no data loss) and GUARD-3 (thread safety via transactions)

## §10 Traceability Matrix

- **GOAL coverage**:
  - ✅ GOAL-1 → §2 (Cluster Discovery)
  - ✅ GOAL-2 → §4 (Insight Generation)
  - ✅ GOAL-3 → §3 (Gate Check)
  - ✅ GOAL-4 → §5 (Provenance)
- **GUARD coverage**:
  - ✅ GUARD-1 → §5.3, §9
  - ✅ GUARD-2 → §2, §3, §8.2
  - ✅ GUARD-3 → §5.3, §9
  - ✅ GUARD-4 → §3.2, §8
  - ✅ GUARD-5 → §4.5
  - ✅ GUARD-6 → §4.3, §5.1, §9
- ⚠️ **GUARD numbering mismatch with requirements**: Requirements define GUARD-4 as "No New External Dependencies" and GUARD-5 as "Insight Identity". The traceability matrix maps GUARD-4 to "Latency Budget" and GUARD-5 to "Insight Identity" with §4.5 (Deterministic Scheduling). **The requirements doc has no GUARD-3 (Thread Safety) or GUARD-4 (Latency Budget) — these come from the master requirements.md.** The design correctly notes "(see master requirements.md)" for GUARD-3 and GUARD-4, but the traceability matrix doesn't distinguish between engine-requirements GUARDs and master GUARDs. The requirements file skips from GUARD-2 to GUARD-4 (no GUARD-3), and has GUARD-4 = "No New External Dependencies" which is NOT in the traceability matrix at all.
- ⚠️ **GUARD-4 "No New External Dependencies"** from requirements-1-engine.md is completely absent from the traceability matrix. Design should confirm it's satisfied (it appears to be — design uses existing embedding provider/extractor).
- ⚠️ **GUARD-6 label inconsistency**: Requirements call it "Existing Signal Reuse" (gate uses only existing infrastructure). The traceability matrix labels it "Provenance Integrity" and maps it to §4.3, §5.1, §9 (validation, provenance table, error handling). These are different concerns. The actual "existing signal reuse" is satisfied by §2 and §3 (zero-LLM clustering/gate), but this isn't what the traceability matrix points to.

## Cross-Section Contradictions

- ⚠️ **`max_cluster_size` default**: §2.3 says default 10. Requirements §6.B says default 15. The config table in §8.1 says 10. Design chose 10 but should note the deviation from requirements, or requirements should be updated.
- ⚠️ **`max_insights_per_consolidation` vs requirements**: Requirements §6.B says "Max insights per cycle (5)". Design §8 says default 10. Deviation not noted.
- ⚠️ **Already-covered threshold**: Requirements GOAL-3 says "existing insight covers ≥80% of cluster members → SKIP". Design §3.2 rule 5 says "all source memories already have provenance records" (100% coverage). This is stricter than requirements — a cluster with 80% covered sources would still be synthesized per the design but should be SKIPped per requirements.
- ⚠️ **Cluster growth re-synthesis threshold**: Requirements GOAL-3 says "cluster grew ≥50%". Design §6.1 says ">30% of cluster members change". These are different thresholds for triggering re-evaluation.

---

## Summary

**No blocking issues.** The second half is well-structured with proper section numbering and comprehensive coverage.

**3 structural issues to fix**:
1. Three config fields in defaults table (§8.1) are missing from their struct definitions in earlier sections (`max_memories_per_llm_call`, `resynthesis_age_threshold`, `temporal_spread_minimum`)
2. GUARD numbering/labeling in traceability matrix doesn't match requirements-1-engine.md (GUARD-4 = "No New External Dependencies" is missing; GUARD-6 is mislabeled)
3. Several default values deviate from requirements without documented rationale (max_cluster_size 10 vs 15, max_insights 10 vs 5, already-covered 100% vs 80%, growth threshold 30% vs 50%)
