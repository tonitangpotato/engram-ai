# Design Review: design-1-engine.md — §1–§5 (Part 1)

**Reviewer**: Claude (automated)  
**Date**: 2026-04-14  
**Scope**: Sections §1 through §5 (lines 1–490)  
**Verdict**: Issues found — see below

---

## Issues Found

### 1. 🔴 GUARD label mismatch — design redefines GUARD numbers

The design's §10 traceability matrix (and §1 overview) assigns different meanings to GUARD numbers than the requirements documents define:

| GUARD | In requirements.md / requirements-1-engine.md | In design-1-engine.md (§1, §10) |
|-------|-----------------------------------------------|----------------------------------|
| GUARD-1 | **No Data Loss** | **Backward Compatibility** |
| GUARD-3 | **Backward Compatibility** | **Thread Safety** |
| GUARD-4 | **No New External Dependencies** | **Latency Budget** |
| GUARD-5 | **Insight Identity** | **Deterministic scheduling** |
| GUARD-6 | **Existing Signal Reuse** | **Provenance Integrity** |

- In §1: "Every operation is reversible (GUARD-1)" — correct label in requirements (No Data Loss), but the §10 traceability matrix relabels GUARD-1 as "Backward Compatibility"
- In §1: "Thread-safe transactions (GUARD-3, see master requirements.md)" — requirements define GUARD-3 as "Backward Compatibility", not thread safety. There is no GUARD for thread safety in the requirements.
- In §1: "Latency budgets respected (GUARD-4, see master requirements.md)" — requirements define GUARD-4 as "No New External Dependencies", not latency. There is no latency GUARD in the requirements.
- In §4.5: "Deterministic Scheduling (GUARD-5)" — requirements define GUARD-5 as "Insight Identity" (insights stored as regular memories, participate in recall). The design repurposes this label for scheduling concerns.
- In §4.3: "do NOT demote sources (GUARD-6)" — requirements define GUARD-6 as "Existing Signal Reuse" (gate uses existing infrastructure only). The design treats this as a provenance integrity guard.

**Impact**: Any reader cross-referencing the design against requirements will get confused. The design either needs to use the correct GUARD labels or explicitly document a renumbering.

### 2. 🟡 §1 pipeline order vs section order inconsistency

§1 states the pipeline is: "cluster discovery → **gate check** → **insight generation** → provenance tracing"

But §3 is titled "Gate Check (GOAL-3)" and §4 is "Insight Generation (GOAL-2)". The GOAL numbering in the requirements is GOAL-1=Cluster Discovery, GOAL-2=Insight Generation, GOAL-3=Gate Check. So the pipeline order (gate before insight) is logically correct, but the GOAL numbers are out of order (GOAL-3 before GOAL-2). This is fine — the sections follow pipeline order, not GOAL numbering — but it could be made explicit with a note.

### 3. 🟡 §2.4 cross-reference to "§6.A implementation note"

Line 130: "Emotional modulation extends beyond stated requirements (§6.A implementation note)..."

This references §6.A in requirements-1-engine.md (section 6, Implementation Notes, subsection A: Emotional Modulation), not §6 of the design document itself. The reference is ambiguous — a reader might look for §6.A within the design doc. Should clarify: "(requirements-1-engine.md §6.A)" or similar.

### 4. 🟡 §3.2 decision tree step 7 contradicts GUARD-6 intent

Step 7: "IF cluster members span < 2 distinct MemoryTypes → SKIP" with exception for all-Factual with high entity overlap.

GUARD-6 in requirements says "Gate check uses only existing infrastructure: embeddings, entities, Hebbian, ACT-R, FTS." The type-diversity check is consistent with this (it uses MemoryType, existing data). However, the rationale "unlikely to produce cross-domain insight" is an assumption — a cluster of purely episodic memories about the same topic could produce valid narrative synthesis. The exception only covers Factual, not Episodic. This could filter out valid EpisodicThread synthesis targets.

### 5. 🟢 §5.1 provenance schema vs GUARD-5 (Insight Identity)

GUARD-5 in requirements says: "Insights stored in same storage system as regular memories. Distinguishable via metadata, not separate tables."

§5.1 introduces a new `synthesis_provenance` table. This does NOT violate GUARD-5 — the insight itself is stored as a MemoryRecord (§4.4), and the provenance table stores the relationship metadata, not the insight. However, this is worth noting: the provenance table IS a new table, which is consistent with GUARD-5 (which only says insights themselves shouldn't be in separate tables).

### 6. 🟢 §2.5 Candidate Pool Selection — ordering concern

§2.5 defines pre-filters for candidate memories. This section logically should come BEFORE §2.2 (Multi-Signal Scoring) and §2.3 (Clustering Algorithm) since filtering happens before scoring. The current ordering implies: signals → clustering → then filtering, but filtering should happen first. The implementation would naturally do it in the right order, but the document order is slightly misleading.

### 7. 🟢 §4.3 validation check 6 — semantic similarity threshold

Check 6 requires insight embedding has >0.4 cosine similarity to cluster centroid embedding. This requires generating an embedding for the insight, which uses the existing embedding provider (consistent with GUARD-4/No New Dependencies). However, this post-validation embedding generation is not counted in any cost/latency budget discussion. For the LLM cost gate in §3.2, only LLM calls are estimated — embedding calls are assumed free/cheap.

### 8. 🟢 Section numbering

Section numbering flows correctly: §1, §2, §2.1–§2.5, §3, §3.1–§3.3, §4, §4.1–§4.5, §5, §5.1–§5.3. No gaps or misnumbering in §1–§5.

### 9. 🟢 Internal cross-references in §1–§5

- §2.4 → §6.A: exists in requirements-1-engine.md (ambiguous, see issue #3)
- §4.5 → consolidate(): valid external reference
- §5.3 → GUARD-3: valid but mislabeled (see issue #1)
- No broken references to nonexistent sections within §1–§5

---

## Summary

| Severity | Count | Description |
|----------|-------|-------------|
| 🔴 Critical | 1 | GUARD label mismatch — 5 of 6 GUARDs have wrong names vs requirements |
| 🟡 Moderate | 3 | Pipeline/GOAL order note, ambiguous cross-ref, type-diversity filter gap |
| 🟢 Minor | 4 | Provenance table clarification, section ordering, embedding cost, numbering OK |
