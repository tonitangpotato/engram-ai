# Design Review: design-1-engine.md (Round 1)

**Reviewer**: RustClaw  
**Date**: 2026-04-13  
**Documents reviewed**: design-1-engine.md v1.0 vs requirements-1-engine.md v6, requirements.md v6  
**Cross-reference**: design-2-integration.md v1.0

---

## 🔴 Critical (blocks implementation)

### FINDING-1: GOAL numbering mismatch — Gate Check is GOAL-3, not GOAL-2 ✅ Applied

Requirements-1-engine.md defines:
- GOAL-1: Cluster Discovery
- GOAL-2: Insight Generation
- **GOAL-3: Gate Check**
- GOAL-4: Source Traceability

Design maps:
- §2 → GOAL-1 (Cluster Discovery) ✓
- §3 → "GOAL-2" (Gate Check) ✗ — should be GOAL-3
- §4 → "GOAL-3" (Insight Generation) ✗ — should be GOAL-2
- §5 → GOAL-4 (Provenance) ✓

The design swapped GOAL-2 and GOAL-3. Every section header, the traceability matrix (§9), and internal references are wrong. Implementers will map to the wrong requirements.

**Suggested fix**: Swap §3 and §4 labels, or (better) reorder sections to match requirements: §2=Cluster, §3=Insight Generation (GOAL-2), §4=Gate Check (GOAL-3), §5=Provenance. Update §9 traceability matrix.

---

### FINDING-2: GUARD numbering mismatches requirements ✅ Applied

Requirements-1-engine.md defines:
- GUARD-1: No Data Loss (never delete source memories)
- GUARD-2: LLM Cost Control (hard cap on LLM calls per cycle, default 5)
- GUARD-4: No New External Dependencies
- GUARD-5: Insight Identity (insights stored as regular memories, not separate tables)
- GUARD-6: Existing Signal Reuse

Design references different GUARDs:
- "GUARD-1" in design = "Zero-LLM clustering" — not in requirements (this is implicit in GOAL-1/GOAL-3 signal constraints + GUARD-6)
- "GUARD-2" in design = "Provenance integrity" — not a named GUARD in requirements
- "GUARD-3" in design = "Reversibility" — not a named GUARD in requirements (reversibility is under GUARD-1's atomicity clause)
- "GUARD-4" in design = "No data loss" — matches requirements GUARD-1
- "GUARD-5" in design = "Deterministic scheduling" — not a named GUARD in requirements (NON-GOAL-3 covers this)
- "GUARD-6" in design = "Graceful degradation" — matches requirements Note F, but not a numbered GUARD

The design invented 6 GUARDs that don't match the requirements' GUARD numbers. This makes traceability broken.

**Suggested fix**: Use the actual GUARD numbers from requirements. Map: GUARD-1 (No Data Loss), GUARD-2 (LLM Cost Control), GUARD-4 (No New External Dependencies), GUARD-5 (Insight Identity), GUARD-6 (Existing Signal Reuse). Add design handling for each. Concepts like "zero-LLM clustering" and "reversibility" should reference the appropriate GOALs/GUARDs, not invent new numbers.

---

### FINDING-3: GUARD-2 (LLM Cost Control) not implemented ✅ Applied

Requirements GUARD-2 specifies: "Hard cap on LLM calls per cycle (configurable, default 5). Clusters prioritized by: (1) emotional significance, (2) size, (3) temporal spread."

Design has:
- `max_insights_per_consolidation: usize` in SynthesisSettings (default: 10) — this is per-consolidation, not per-cycle, and default differs from requirements (5 vs 10)
- No cluster **prioritization** logic anywhere. The requirements demand ordering by emotional significance → size → temporal spread. The design has no sorting/ranking step between gate check and synthesis.
- The gate config has a `cost_threshold` in USD, but this is per-cluster cost estimation, not a hard cap on total LLM calls.

**Suggested fix**: Add a §4.X "Cluster Prioritization" section that sorts SYNTHESIZE-gated clusters by (1) emotional significance (need to define how — e.g., any Emotional-type member, or high importance variance), (2) cluster size (larger first), (3) temporal spread (wider spread = more diverse = higher priority). Add `max_llm_calls_per_cycle: usize` (default: 5) to SynthesisSettings. Process only the top-N clusters after prioritization.

---

### FINDING-4: GUARD-5 (Insight Identity) violated — design uses separate `synthesis_provenance` table ✅ Applied

Requirements GUARD-5: "Insights stored in same storage system as regular memories. Distinguishable via metadata, not separate tables."

Design §5.1 creates `synthesis_provenance` as a new table. This technically doesn't violate GUARD-5 for the *insights themselves* (those are stored as MemoryRecord), but the provenance data is in a separate table. 

More critically: how are insights *distinguished* from regular memories? Design §4.4 stores insights as `memory_type: MemoryType::Factual`. There's no `MemoryType::Synthesis` variant — and the requirements say "Distinguishable via metadata". The design needs to specify what metadata field marks a memory as an insight (e.g., `metadata.synthesis_cluster_id` or a dedicated `is_synthesis: bool` field).

Note: §2.4 says `memory_type != "synthesis"` for candidate filtering, but there's no `MemoryType::Synthesis` in the existing enum. This is contradictory.

**Suggested fix**: 
1. Decide: add `MemoryType::Synthesis` to the enum, or use a metadata field `{"source": "synthesis", "cluster_id": "..."}`. Either way, define it explicitly.
2. Ensure candidate filtering in §2.4 uses whatever mechanism is chosen.
3. The `synthesis_provenance` table is probably fine — GUARD-5 refers to insight *memories*, not auxiliary tracking data. But clarify this interpretation.

---

### FINDING-5: Clustering algorithm contradicts description — says "agglomerative" but implements connected components ✅ Applied

§2.3 title says "Greedy agglomerative clustering with single-linkage" and justifies the choice. But the actual process described is:
1. Compute pairwise scores
2. Build edges where score ≥ threshold
3. **Apply connected components** to find initial clusters
4. Split large clusters using higher threshold

Connected components on a thresholded graph is NOT agglomerative clustering. Agglomerative starts with each item as its own cluster and iteratively merges the closest pairs. What's described is graph-based thresholding + connected components — a simpler and perfectly valid approach, but the name and justification are wrong.

**Suggested fix**: Either (a) rename to "Graph-based clustering with threshold segmentation" and update the justification, or (b) actually describe agglomerative clustering if that's what's intended.

---

## 🟡 Important (should fix before implementation)

### FINDING-6: Missing "Already Covered" and "Cluster Growth" gate checks (GOAL-3 requirements) ✅ Applied

Requirements GOAL-3 specifies four gate conditions:
1. Near-duplicates (avg similarity > 0.92) → SKIP ✓ (design has 0.95 threshold, close enough)
2. Too recent (all memories created within 1-hour window) → DEFER ✗ **Missing**
3. Already covered (existing insight covers ≥80% of cluster members) → SKIP ✗ **Missing**
4. Cluster growth (existing insight covers cluster but grew ≥50%) → RE-SYNTHESIZE ✗ **Missing**

Design §3.2 only has: size check, duplicate check, quality check, type diversity check, cost check. It's missing the temporal "too recent" check, the "already covered" lookup, and the "cluster growth" re-synthesis trigger.

**Suggested fix**: Add gate decision tree steps for:
- DEFER: all members created_at within 1 hour of each other → too recent for synthesis
- SKIP (already covered): query synthesis_provenance to find if ≥80% of members are already sources of an existing insight
- RE-SYNTHESIZE: if an existing insight covers the cluster but the cluster has grown ≥50% new members → re-synthesize (not skip)

---

### FINDING-7: Missing "DEFER" gate decision variant ✅ Applied

Requirements GOAL-3 "Too recent" check returns DEFER. Design's GateDecision enum only has: Synthesize, AutoUpdate, Skip. There's no Defer variant.

**Suggested fix**: Add `Defer { reason: String, retry_after_cycles: u32 }` to GateDecision enum.

---

### FINDING-8: Duplicate similarity threshold diverges from requirements ✅ Applied

Requirements GOAL-3: "average intra-cluster embedding similarity > 0.92 → SKIP"
Design §3.2: "all members have embedding similarity > 0.95 → AUTO-UPDATE(MergeDuplicates)"

Two issues:
1. Threshold: 0.92 (req) vs 0.95 (design)
2. Outcome: SKIP (req) vs AUTO-UPDATE (design)
3. Condition: "average" (req) vs "all members" (design)

These are different behaviors. Requirements say high similarity = skip entirely (no insight needed). Design says high similarity = merge duplicates (an active operation).

**Suggested fix**: Match requirements: average intra-cluster similarity > 0.92 → SKIP. If the design intentionally chose AUTO-UPDATE as a better approach than SKIP, document the rationale and note the deviation from requirements.

---

### FINDING-9: Insight validation checks incomplete vs GOAL-2 ✅ Applied

Requirements GOAL-2 validation:
- (a) Semantically distinct: similarity to any source < 0.95 ✓ (design §4.3 check #3 covers this loosely but uses "must differ from source content" — not quantified)
- (b) Content length ≥ median of source content lengths ✗ **Missing**
- (c) Actionable — subjective, but design doesn't mention it at all ✗ **Missing**
- Invalid LLM response: centroid similarity < 0.5 → failure ✗ **Missing**

Design §4.3 checks: source_ids valid, confidence in range, not empty, valid enum. Missing the embedding-based similarity check, the length check, and the centroid similarity check.

**Suggested fix**: Add explicit validation steps:
1. `cosine_similarity(insight_embedding, source_embedding) < 0.95` for each source
2. `insight.content.len() >= median(sources.map(|s| s.content.len()))`
3. `cosine_similarity(insight_embedding, cluster_centroid_embedding) >= 0.5` (sanity check)

---

### FINDING-10: Emotional modulation completely absent ✅ Applied

Requirements Note A specifies: "When EmotionalBus detects strong signal (|valence| > 0.7), clusters in that domain get synthesis priority. Resulting insights inherit emotional importance boost."

Also GUARD-2 prioritization: "(1) emotional significance" is the #1 sorting criterion.

Design has zero mention of emotional signals, EmotionalBus, valence, or emotional priority. This is required for cluster prioritization (GUARD-2) and called out as a specific implementation note.

**Suggested fix**: Add §2.5 or equivalent covering:
- How emotional significance is computed for a cluster (e.g., any member with emotional valence > 0.7, or any Emotional MemoryType member)
- How it feeds into cluster prioritization (sort order)
- Emotional importance boost for resulting insights

---

### FINDING-11: Incremental operation not designed (Note E) ✅ Applied

Requirements Note E: "LLM calls should fire only for new/changed clusters (≥1 member added since last synthesis). Track 'processed' status via provenance."

Design has no mechanism for detecting which clusters are new/changed vs already processed. Every consolidation cycle would re-discover all clusters and re-gate them. Without incremental tracking, the system wastes compute re-evaluating identical clusters.

**Suggested fix**: Add a section on incremental detection:
- After gate check, query synthesis_provenance for each cluster member
- If all members are already sources of an existing insight → cluster is "covered" (this overlaps with FINDING-6's "already covered" check)
- Only clusters with ≥1 unprocessed member pass to synthesis

---

### FINDING-12: Cluster signal priorities don't match requirements ✅ Applied

Requirements GOAL-1 specifies a priority ordering:
1. **(Primary)** Hebbian co-activation (threshold 0.3)
2. **(Secondary)** Embedding similarity (threshold 0.75)
3. **(Tertiary)** Entity overlap (≥2 shared entities)

And: "Hebbian-primary clusters rank above embedding-primary, which rank above entity-only."

Design §2.2 uses weighted sum with weights (0.4, 0.3, 0.2, 0.1) — this is a flat scoring model, not a priority hierarchy. A cluster held together purely by entity overlap with zero Hebbian links would score the same as one with moderate Hebbian links, depending on the numbers.

The requirements want a clear ranking: Hebbian-primary > embedding-primary > entity-only. The design's weighted sum doesn't guarantee this.

**Suggested fix**: Either (a) add cluster classification (primary signal = whichever signal contributed most) and sort clusters by signal tier before processing, or (b) adjust weights to be dramatically skewed (e.g., Hebbian=0.7, embedding=0.2, entity=0.1) so Hebbian always dominates. Option (a) is cleaner and matches requirements more directly.

---

### FINDING-13: Embedding similarity threshold mismatch ✅ Applied

Requirements GOAL-1: embedding similarity threshold default 0.75
Design §2: no explicit embedding threshold for clustering — uses composite score threshold of 0.3

The requirements envision a clear per-signal threshold (Hebbian ≥ 0.3, embedding ≥ 0.75, entity ≥ 2 shared). The design merges all signals into one composite score with a single threshold. This means a pair with embedding similarity 0.50 (below requirements' 0.75) could still cluster if Hebbian is strong enough.

This may or may not be desirable, but it deviates from requirements without justification.

**Suggested fix**: Clarify whether per-signal thresholds (as in requirements) apply before the composite score, or whether the composite score replaces them. If replacing, document the rationale.

---

### FINDING-14: Type conflict with design-2-integration.md ✅ Applied

Design-2 §1 says it depends on Part 1 providing:
- `SynthesisCluster` — design-1 calls it `MemoryCluster`
- `GateDecision` — matches ✓
- `SynthesisResult` — design-1 calls it `SynthesisReport`

Name mismatches between the two design docs will cause confusion during implementation.

**Suggested fix**: Align naming. Either update design-1 to use `SynthesisCluster`/`SynthesisResult`, or update design-2 to use `MemoryCluster`/`SynthesisReport`. Pick one set and be consistent.

---

## 🟢 Minor (can fix during implementation)

### FINDING-15: Temporal signal uses hours_apart but requirements don't specify time unit ✅ Applied

Design §2.2: `temporal_proximity: exp(-λ × hours_apart)`. This is reasonable but the decay constant `temporal_decay_lambda: 0.01` seems very slow — at 0.01/hour, even memories 100 hours apart get score ~0.37. With thousands of memories over months, temporal proximity would be nearly uniform and contribute almost no signal.

**Suggested fix**: Consider a more aggressive default (e.g., λ=0.1, giving ~0.37 at 10 hours apart) or use days instead of hours.

---

### FINDING-16: SynthesisError type referenced but never defined ✅ Applied

§6.2 `SynthesisReport` has `errors: Vec<SynthesisError>` but `SynthesisError` is never defined anywhere in the design.

**Suggested fix**: Define SynthesisError enum with variants: LlmTimeout, LlmInvalidResponse, ValidationFailed, StorageError, etc.

---

### FINDING-17: Prompt template variants described but only General template shown ✅ Applied

§4.2 shows a single prompt template (the General one). FactualPattern, EpisodicThread, and CausalChain templates are mentioned but not specified. An implementer wouldn't know how they differ.

**Suggested fix**: Add at least a one-paragraph description of how each template differs from General. Full prompt text can be deferred to implementation, but the *intent* of each variant should be clear.

---

### FINDING-18: No mention of namespace scoping ✅ Applied

Requirements NON-GOAL-4 says "Within single namespace only." Design has no mention of namespace filtering at all. Cluster discovery should scope all queries to the current namespace.

**Suggested fix**: Add namespace parameter to ClusterDiscoveryConfig and mention that all SQL queries include `WHERE namespace = ?`.

---

### FINDING-19: LLM provider integration not specified ✅ Applied

Design §4 describes what prompt to send and what response to expect, but doesn't specify how to call the LLM. Requirements GUARD-4 says "Use existing embedding provider and extractor trait." The design should reference which existing trait/function handles LLM calls.

**Suggested fix**: Reference the existing `Extractor` trait or LLM provider interface from the codebase. If synthesis needs a new trait method (e.g., `generate_insight(prompt) -> String`), define it.

---

## ✅ Passed Checks

- §1 Overview: Clear, sets correct scope ✅
- §2 Cluster Discovery: 4 signal sources correctly identified (Hebbian, entity, embedding, temporal) ✅
- §2.2 Composite scoring: Mathematical formula is well-defined ✅
- §2.4 Candidate filtering: Pre-filter conditions are reasonable ✅
- §3 Gate Check: Zero-LLM confirmed — all checks use existing signals ✅ (GUARD-6/Existing Signal Reuse)
- §4.2 Prompt engineering: Structured JSON output format is good practice ✅
- §4.3 Source ID validation: Checks for hallucinated references ✅
- §4.5 Deterministic scheduling: Only via consolidate() ✅ (NON-GOAL-3)
- §5.1 Provenance schema: SQL is valid, indexes correct, FKs present ✅
- §5.2 Bidirectional queries: Forward + reverse + chain all specified ✅ (GOAL-4)
- §5.3 Reversibility: Transaction-based undo with stored original values ✅ (GUARD-1 atomicity)
- §8 Error handling: Transaction rollback, no partial state ✅
- §6.1 API trait: Clean separation of concerns ✅
- §7 Config: Integrates into existing MemoryConfig ✅

---

## 📊 Traceability Matrix

| Requirement | Design Coverage | Status |
|---|---|---|
| GOAL-1 (Cluster Discovery) | §2 | 🟡 Partially — signal priorities not hierarchical (FINDING-12,13) |
| GOAL-2 (Insight Generation) | §4 (mislabeled as GOAL-3) | 🟡 Missing validation checks (FINDING-9) |
| GOAL-3 (Gate Check) | §3 (mislabeled as GOAL-2) | 🟡 Missing DEFER/covered/growth checks (FINDING-6,7,8) |
| GOAL-4 (Provenance) | §5 | ✅ Covered |
| GUARD-1 (No Data Loss) | §5.3, §8 | ✅ Covered |
| GUARD-2 (LLM Cost Control) | — | 🔴 Missing hard cap + prioritization (FINDING-3) |
| GUARD-4 (No New External Deps) | implicit | ✅ No new crates |
| GUARD-5 (Insight Identity) | §4.4 | 🔴 Ambiguous — memory_type contradiction (FINDING-4) |
| GUARD-6 (Existing Signal Reuse) | §2, §3 | ✅ Uses only existing signals |
| Note A (Emotional Modulation) | — | 🟡 Missing (FINDING-10) |
| Note E (Incremental Operation) | — | 🟡 Missing (FINDING-11) |
| Note F (LLM Graceful Degradation) | §8 partially | ✅ Covered |
| NON-GOAL-4 (Namespace Scoping) | — | 🟢 Not mentioned (FINDING-18) |

---

## Summary

- **Critical**: 5 (FINDING-1 through FINDING-5)
- **Important**: 9 (FINDING-6 through FINDING-14)
- **Minor**: 5 (FINDING-15 through FINDING-19)
- **Total findings**: 19

**Top 3 priorities:**
1. Fix GOAL/GUARD numbering (FINDING-1, 2) — fundamental traceability broken
2. Add missing gate checks: DEFER, already-covered, cluster-growth (FINDING-6, 7)
3. Add LLM cost control + cluster prioritization (FINDING-3)

**Recommendation**: Needs revision before implementation. The core architecture is sound (clustering → gate → synthesis → provenance pipeline is well-structured), but the requirements mapping has significant gaps and the numbering mismatches would cause confusion during implementation.

---

## Apply Status

- **All 19 findings applied** (2026-04-13)
- Applied in batches by sub-agents to avoid context overflow
- **Batch 1**: FINDING-1, 5, 8 (numbering, algorithm name, threshold)
- **Batch 2**: FINDING-2, 4, 14 (GUARD numbers, memory type, type names)
- **Batch 3**: FINDING-3, 6, 7, 9 (LLM budget, DEFER path, gate checks, validation)
- **Batch 4**: FINDING-10, 11, 12, 13, 15-19 (emotional, incremental, weights, minor fixes)
