# Requirements Review: Knowledge Synthesis (R2)

**Document**: `.gid/features/knowledge-synthesis/requirements.md` (v2)  
**Review Date**: 2026-04-09  
**Review Depth**: Full (Phases 0-6, all 28 checks)  
**Reviewer**: RustClaw

---

## Phase 0: Document Size Check

### Check #0: Document size ✅
12 GOALs, 5 NON-GOALs, 6 GUARDs, 6 SCs. Under the 15-GOAL threshold. No split needed.

---

## Phase 1: Individual Requirement Quality

### Check #1: Specificity ✅
All GOALs have concrete conditions. Checked all 12 — no vague language ("fast", "robust", "appropriate"). Thresholds are numeric and configurable.

### Check #2: Testability ✅
Every GOAL has a pass/fail condition. GOAL-2 has explicit validation criteria (similarity < 0.95, length ≥ median). GOAL-4 has classification outputs (SYNTHESIZE/AUTO-UPDATE/SKIP). GOAL-12 has explicit behavior per condition.

### Check #3: Measurability ✅
All quantitative values have defaults: similarity thresholds (0.75, 0.92, 0.95), cluster size (3), cycle cap (5), demotion factor (0.5), temporal spread (1 hour), etc.

### Check #4: Atomicity

**FINDING-1** ✅ Applied [🟡 Important] **GOAL-1 mixes discovery mechanism with filtering logic**  
GOAL-1 says "only produce clusters containing memories not yet covered by an existing insight" — this is GOAL-5 (idempotency) leaking into GOAL-1 (discovery). Discovery should discover ALL clusters; filtering is a separate concern (GOAL-4/5 handle it).  
**Suggested fix**: Remove the last sentence of GOAL-1 ("Cluster discovery must operate on the full memory set but only produce clusters containing memories not yet covered by an existing insight (see GOAL-5)"). The gate check (GOAL-4) already handles filtering via "Existing coverage" signal. Keep GOAL-1 as pure discovery.

### Check #5: Completeness of each requirement ✅
Checked actor/trigger/behavior/outcome for all 12 GOALs. Each specifies what triggers it and what the expected output is.

### Check #6: Implementation leakage

**FINDING-2** ✅ Applied [🟡 Important] **GOAL-1(a) specifies "pairwise" similarity — forces O(n²) algorithm**  
"Pairwise embedding cosine similarity exceeds a configurable threshold" prescribes a specific clustering algorithm (all-pairs comparison). This is an implementation choice. The requirement should be "memories that are semantically similar" — the design can then choose pairwise, centroid-based, or approximate methods.  
**Suggested fix**: Rewrite GOAL-1(a) to: "Embedding cosine similarity within the cluster exceeds a configurable threshold (default 0.75)" — remove "Pairwise" to allow design freedom (centroid, leader-follower, etc.).

**FINDING-3** ✅ Applied [🟢 Minor] **GOAL-4 "avg pairwise similarity > 0.92" — same O(n²) leakage**  
Same issue as FINDING-2 but in the gate check. Can be fixed the same way — "average similarity within cluster" instead of "avg pairwise".  
**Suggested fix**: Change "avg pairwise similarity > 0.92" to "average embedding similarity within the cluster > 0.92".

---

## Phase 2: Coverage & Gaps

### Check #7: Happy path coverage ✅
Happy path: discover clusters → gate check → LLM synthesize → validate → store insight → demote sources. Fully covered by GOAL-1→4→2→3→7.

### Check #8: Error/edge case coverage

**FINDING-4** ✅ Applied [🟡 Important] **Missing: what happens when LLM produces garbage?**  
GOAL-2 validates semantic distinctness and length, but what if the LLM returns:
- Empty string
- Hallucinated content unrelated to sources
- Valid-looking text that fails to embed (embedding provider error)

The doc says "synthesis attempt is skipped and logged" for validation failure, but doesn't define what counts as a failed LLM call vs. a successful-but-invalid response.  
**Suggested fix**: Add to GOAL-2: "LLM response is considered invalid if: (a) empty/whitespace-only, (b) fails to produce an embedding, or (c) embedding similarity to the cluster centroid < 0.5 (response is off-topic). Invalid responses are treated identically to validation failures — skipped and logged."

**FINDING-5** ✅ Applied [🟢 Minor] **Missing: cluster with all members in Archive layer**  
If all source memories are already in Archive, should synthesis still run? Demoting archived memories further has no effect (Archive→Archive). Not a blocker — just worth specifying.  
**Suggested fix**: Add a note to GOAL-7: "If all source memories are already Archive with importance < 0.1, demotion is a no-op — acceptable."

### Check #9: Non-functional requirements

**FINDING-6** ✅ Applied [🟡 Important] **No observability/logging requirements**  
SC-3 requires measuring gate check filter rate, but there's no GOAL specifying what gets logged. For a system that calls LLM with cost implications, operators need visibility into: clusters discovered, gate decisions per cluster, LLM calls made, insights stored, sources demoted. This is essential for debugging and tuning thresholds.  
**Suggested fix**: Add GOAL-13 (or fold into GOAL-9 API): "Each synthesis cycle must return a structured summary: total clusters discovered, gate decisions (N synthesize, N auto-update, N skip), LLM calls made, insights stored, sources demoted, total duration. This summary must be available via both API return value and log output at INFO level."

### Check #10: Boundary conditions

**FINDING-7** ✅ Applied [🟡 Important] **GOAL-1: What happens with cluster size = database size?**  
If many memories are about the same topic (common for agents — hundreds of memories about "Rust development"), GOAL-1 could produce a single massive cluster. GOAL-10 caps "maximum memories per LLM synthesis call" at 10, but GOAL-1 has no max cluster size. A 500-memory cluster that passes gate check would need splitting.  
**Suggested fix**: Add to GOAL-10: "Maximum cluster size for synthesis (default: 15). Clusters larger than this are split by strongest internal sub-clusters or processed using only the highest-activation members."

### Check #11: State transitions ✅
Insight lifecycle: Created → (Active) → Superseded (via GOAL-5 re-synthesis). Source lifecycle: Active → Demoted (importance × factor, layer→Archive). Both clearly defined.

---

## Phase 3: Consistency & Contradictions

### Check #12: Internal consistency

**FINDING-8** ✅ Applied [🔴 Critical] **GOAL-4 "Existing coverage" contradicts GOAL-1 filtering**  
GOAL-1 says clusters should "only produce clusters containing memories not yet covered by an existing insight". GOAL-4's gate check also checks "Existing coverage: if an existing insight already covers ≥80% of the cluster members, skip". These are redundant and potentially contradictory — if GOAL-1 filters at discovery, GOAL-4's existing-coverage check would never fire.  
**Suggested fix**: Remove filtering from GOAL-1 (see FINDING-1). Let GOAL-1 discover freely, GOAL-4 filter. Clean separation of concerns.

### Check #13: Terminology consistency ✅
"Insight" used consistently throughout. "Cluster" always means the same thing. "Synthesis" vs "consolidation" are clearly distinguished.

### Check #14: Priority consistency ✅
No priority inversions. Gate check (GOAL-4) correctly depends on cluster discovery (GOAL-1).

### Check #15: Numbering/referencing ✅
Cross-references verified: GOAL-5 refs GOAL-3 (chain depth) ✅, GOAL-1 refs GOAL-5 ✅ (but should be removed per FINDING-1), NON-GOAL-2 refs GOAL-3 ✅, GUARD-3 refs consolidate() ✅.

### Check #16: GUARDs vs GOALs alignment ✅
GUARD-1 (no data loss) aligns with GOAL-7 (demotion not deletion). GUARD-2 (cost control) aligns with GOAL-10 (max insights per cycle). GUARD-5 (insight identity) aligns with GOAL-3 (provenance). No contradictions.

---

## Phase 4: Implementability

### Check #17: Technology assumptions ✅
Correctly references existing engram infrastructure: embeddings, entities, Hebbian links, EmotionalBus, ACT-R. No unstated technology assumptions.

### Check #18: External dependencies ✅
GUARD-4 explicitly prohibits new dependencies. LLM is the only external dependency, and GOAL-12 handles its absence. Good.

### Check #19: Data requirements

**FINDING-9** ✅ Applied [🟡 Important] **GOAL-3 provenance: where is it stored?**  
GUARD-5 says insights use the same storage as regular memories and are distinguishable through metadata. GOAL-3 requires forward/reverse provenance queries. But the existing `MemoryRecord` struct has no "source_ids" field — only `contradicts`/`contradicted_by` (single string each) and generic `metadata` (JSON blob). The requirements should specify HOW provenance is stored without prescribing schema.  
**Suggested fix**: Add to GOAL-3: "The provenance relationship must be queryable in both directions with O(1) per-insight lookups. The storage mechanism must support listing all source IDs for an insight and all insight IDs for a source memory, without scanning the full memory table."

### Check #20: Migration/compatibility ✅
GUARD-3 explicitly states backward compatibility. No migration needed — synthesis is additive.

### Check #21: Scope boundaries ✅
5 explicit NON-GOALs covering the key scope boundaries: real-time, multi-level, scheduling, cross-namespace, contradiction resolution. Well-defined scope.

---

## Phase 5: Traceability & Organization

### Check #22: Unique identifiers ✅
GOAL-1 through GOAL-12, GUARD-1 through GUARD-6, NON-GOAL-1 through NON-GOAL-5, SC-1 through SC-6. No duplicates, no gaps.

### Check #23: Grouping/categorization ✅
Logically grouped: GOAL-1-2 (core synthesis), GOAL-3 (provenance), GOAL-4-5 (efficiency), GOAL-6 (emotional), GOAL-7 (cleanup), GOAL-8-9 (interfaces), GOAL-10-12 (operational). Clear flow.

### Check #24: Dependency graph ✅
Implicit order is correct: GOAL-1 (discover) → GOAL-4 (gate) → GOAL-2 (synthesize) → GOAL-3 (provenance) → GOAL-7 (demote). No circular dependencies.

### Check #25: Acceptance criteria
SC-1 through SC-6 serve as acceptance criteria. All measurable and testable.

---

## Phase 6: Stakeholder Alignment

### Check #26: User perspective ✅
GOAL-8 (CLI) and GOAL-9 (API) address both operator and developer perspectives.

### Check #27: Success metrics ✅
SC-1 through SC-6 are production-verifiable.

### Check #28: Domain language ✅
Neuroscience terms (CLS, sleep replay, schema formation) are mapped clearly to implementation concepts. Section 2 provides the glossary.

---

## 📊 Summary

| Severity | Count | Findings |
|---|---|---|
| 🔴 Critical | 1 | FINDING-8 |
| 🟡 Important | 6 | FINDING-1, 2, 4, 6, 7, 9 |
| 🟢 Minor | 2 | FINDING-3, 5 |
| **Total** | **9** | |

### ✅ Passed Checks (19/28)
- Check #0: Document size ✅
- Check #1: Specificity ✅
- Check #2: Testability ✅
- Check #3: Measurability ✅
- Check #5: Completeness ✅
- Check #7: Happy path ✅
- Check #11: State transitions ✅
- Check #13: Terminology ✅
- Check #14: Priority ✅
- Check #15: Numbering ✅
- Check #16: GUARDs vs GOALs ✅
- Check #17: Technology ✅
- Check #18: Dependencies ✅
- Check #20: Migration ✅
- Check #21: Scope ✅
- Check #22: Identifiers ✅
- Check #23: Grouping ✅
- Check #24: Dependencies ✅
- Check #25-28: Traceability + Stakeholder ✅

### Key Improvements vs v1 (R1 Review)
- All 7 R1 Critical findings resolved (implementation leakage removed, validation criteria added, re-synthesis triggers specified)
- Neuroscience mapping (§2) is excellent — clear design rationale
- Gate Check (GOAL-4) is the strongest differentiator — well-specified
- NON-GOALs are comprehensive and prevent scope creep

### Recommendation
**Ready to proceed after fixing FINDING-8** (the GOAL-1/GOAL-4 redundancy). The remaining Important findings are improvements but not blockers. Apply FINDING-8 before design phase.
