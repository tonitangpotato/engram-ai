# Requirements Review — FEAT-003: Memory Lifecycle (R1)

**Document**: `.gid/features/memory-lifecycle/requirements.md`
**Review depth**: Standard (Phase 0–5)
**Reviewer**: RustClaw
**Date**: 2026-04-16

---

## Summary

28 GOALs, 6 GUARDs across 9 components. Document is well-structured with accurate as-is status sections. Found **10 findings**: 1 critical, 4 important, 5 minor.

**Verdict**: Solid foundation. Fix the critical (FINDING-1) and the 4 important ones before writing design doc. The rest are polish.

---

## Applied: All 10 findings ✅

## Findings

### FINDING-1 [Critical] — `forget()` archives instead of deleting, contradicts GOAL-24

**GOAL-24** says: "Forget deletes associated entity links, Hebbian links, and access logs" and "Forget MUST return a summary: how many memories removed, total storage freed."

**Actual code** (`memory.rs:1407-1430`): `forget()` moves memories to `MemoryLayer::Archive`, it does NOT delete them. It also doesn't touch entity links, Hebbian links, or access logs. And it returns `Ok(())`, not a summary.

The requirements describe desired behavior as if it's a missing implementation, but the **architectural decision** (archive vs delete) is not addressed. This is a design fork:
- Option A: Keep archive semantics, rename GOAL-24 to "archive + cleanup references"
- Option B: Actually delete, as currently specified

**This matters because**: GOAL-27 (compact) says it "proposes a deletion list." If `forget()` already archives (soft delete), then compact should be the hard delete. But if `forget()` hard deletes, compact is redundant.

**Suggested fix**: Clarify the two-tier model explicitly:
- `forget()` = soft delete (archive) + cleanup stale references
- `compact()` = hard delete of archived memories past retention period
- Add a GOAL for the archive→delete lifecycle

---

### FINDING-2 [Important] — GOAL-17 (entity co-occurrence) not implemented, but §6 says "✅ Fully implemented"

**GOAL-17**: "The system MUST maintain entity co-occurrence relationships."

**Actual code**: `entity_relations` table exists in schema. `upsert_entity_relation` exists in `storage.rs`. But there is **zero code** that automatically records co-occurrence when two entities appear in the same memory. The `extract_and_store_entities` flow in `entities.rs` stores entity↔memory links but does NOT create entity↔entity co-occurrence links.

The co-occurrence links that exist in `entity_relations` are only created by explicit API calls (e.g., from external callers), not automatically during memory writes.

**As-is status** says "✅ Fully implemented" for C6, but GOAL-17 is NOT implemented.

**Suggested fix**: Update the as-is status for §6 to: "✅ Mostly implemented. **Missing**: automatic co-occurrence link creation on memory write (GOAL-17)."

---

### FINDING-3 [Important] — GOAL-18 entity_recall 1-hop scoring issue is worse than documented

**GOAL-18** specifies: "Direct entity match = score 1.0; 1-hop related entity = score 0.5"

**Actual code** (`memory.rs:1097-1138`): The scoring adds raw values (1.0 for direct, 0.5 for 1-hop), then normalizes by dividing by max. This means:

- If a memory is found ONLY via 1-hop (score = 0.5), after normalization it becomes 1.0 (max = 0.5, 0.5/0.5 = 1.0). This is **indistinguishable from a direct match**.
- If a memory has both direct + 1-hop (score = 1.5), after normalization it becomes 1.0 too.
- A memory with only 1 direct match (1.0) and another with 3 direct matches (3.0) get normalized to 0.33 and 1.0 respectively — the single-match memory gets unfairly penalized.

The normalization strategy destroys the intended signal differentiation. The Known Issues section mentions this but understates it ("may produce unexpected results for edge cases") — it's a systematic problem, not an edge case.

**Suggested fix**: Add a GOAL or sub-requirement specifying the normalization strategy:
- "Scores MUST preserve relative ordering between direct matches and 1-hop matches"
- Suggest: normalize by a fixed maximum (e.g., max possible = number_of_query_entities * 1.5) rather than by actual max in results

---

### FINDING-4 [Important] — Missing GOAL for interaction between `forget()` and insights

**GOAL-24** says: "Forget MUST NOT delete insights (synthesized memories)."

But what about the **provenance links**? When `forget()` removes/archives a source memory that was used to generate an insight, the insight's provenance now points to a non-existent record.

No GOAL addresses:
- Should insights whose ALL source memories have been forgotten also be candidates for forgetting?
- Should insight provenance be updated when source memories are forgotten (e.g., mark as "source forgotten")?
- Can you forget a memory that is the sole source of a still-active insight?

**Suggested fix**: Add GOAL-24a:
"When forgetting a memory that is referenced as a source in insight provenance, the system MUST update the insight's provenance to mark that source as archived. If ALL source memories of an insight have been forgotten, the insight MUST be flagged for review but NOT automatically deleted."

---

### FINDING-5 [Important] — GUARD-5 references FEAT-002 event bus that's out of scope

**GUARD-5**: "All lifecycle operations MUST emit events via the event bus for observability."

But the Overview says: "Event bus / notification system (FEAT-002)" is **out of scope**.

If FEAT-002 isn't built yet, this guard can't be satisfied. Either:
- Define a minimal event interface within FEAT-003 (trait with no-op default impl)
- Make GUARD-5 conditional: "MUST emit events IF event bus is available"
- Add FEAT-002 as a dependency

**Suggested fix**: Change GUARD-5 to: "All lifecycle operations MUST support event emission via a configurable callback or trait. If no event bus is configured, operations proceed silently. Event types include: `MemoryMerged`, `InsightCreated`, `MemoryForgotten`, `RebalanceCompleted`."

This makes FEAT-003 implementable independently of FEAT-002.

---

### FINDING-6 [Minor] — GOAL-3 performance target (50ms dedup) not testable as written

**GOAL-3**: "Dedup lookup MUST complete within 50ms for databases up to 100k memories."

How is this tested? The doc says "index-driven" but doesn't specify:
- What hardware/conditions (CI vs dev machine)?
- Is this p50, p95, or p99?
- Is this a hard requirement (test fails if exceeded) or a benchmark target?

Performance requirements without test methodology are aspirational, not requirements.

**Suggested fix**: Change to: "Dedup lookup SHOULD complete within 50ms (p95) for databases up to 100k memories on commodity hardware. Benchmark test MUST exist to verify."

Similarly for GOAL-22 (100ms fusion recall).

---

### FINDING-7 [Minor] — Component dependency graph has an arrow direction inconsistency

The dependency graph uses `←` which reads as "is used by" in some lines and "depends on" in others:

```
C6 (Entity Extraction) ← C2 (Dedup) uses entity overlap
```

Does this mean C2 depends on C6? Or C6 depends on C2? The annotation says "C2 uses entity overlap" which means C2 → C6 (C2 depends on C6). But the `←` arrow suggests C6 is the dependent.

**Suggested fix**: Use consistent notation:
```
C2 (Dedup) → depends_on → C6 (Entity Extraction) — dedup uses entity overlap
C3 (Merge) → depends_on → C2 (Dedup) — dedup triggers merge
...
```

---

### FINDING-8 [Minor] — GOAL-11 "≥80% reduction" is not verifiable

**GOAL-11**: "Incremental synthesis MUST reduce redundant LLM calls by ≥80% on stable databases."

80% compared to what? What defines "stable"? How many cycles must pass before the database is "stable"? This is unmeasurable as written.

**Suggested fix**: "After an initial full synthesis pass, running sleep_cycle() again with no new memories added MUST result in zero LLM calls. Running sleep_cycle() after adding N new memories MUST only synthesize clusters containing those new memories."

---

### FINDING-9 [Minor] — GOAL-4 content replacement rule ("30% longer") is fragile

**GOAL-4**: "If new content is >30% longer than existing, replace content with new version."

What if the new content is shorter but more accurate? What if the existing content is already a merged result of 5 prior versions (and thus very long), making it impossible for a new single memory to be 30% longer?

**Suggested fix**: Consider a more nuanced rule: "Replacement criteria: new content is longer, OR new content was created more recently (within 1 hour) and has higher importance. If neither applies, append metadata noting the alternative phrasing."

---

### FINDING-10 [Minor] — Missing explicit GOAL for `sleep_cycle` integration of decay + forget

**GOAL-12** defines sleep_cycle as: "discover clusters → gate each → synthesize passing clusters → store insights"

But there's no mention of running decay (GOAL-25) or forget (GOAL-24) as part of sleep_cycle. The as-is status notes `decay_hebbian_links` runs in consolidate, but the requirements don't specify the ordering.

**Suggested fix**: Expand GOAL-12's pipeline definition:
"Sleep cycle = discover clusters → gate each → synthesize → store insights → decay Hebbian links → optionally run forget(). The pipeline order ensures synthesis uses pre-decay link strengths."

---

## Phase Summary

| Phase | Checks Run | Findings |
|-------|-----------|----------|
| Phase 0: Size | ✅ 9 components, within limit | — |
| Phase 1: Structural Completeness | ✅ All GOALs defined, cross-refs valid | FINDING-7 |
| Phase 2: Logic Correctness | ⚠️ Archive vs delete ambiguity, provenance gap | FINDING-1, FINDING-4 |
| Phase 3: Type Safety & Edge Cases | ⚠️ Normalization bug, fragile replacement rule | FINDING-3, FINDING-9 |
| Phase 4: Architecture Consistency | ⚠️ Event bus dependency, as-is accuracy | FINDING-2, FINDING-5 |
| Phase 5: Doc Quality | ⚠️ Unmeasurable performance/reduction targets | FINDING-6, FINDING-8, FINDING-10 |

---

## Stats

- Critical: 1 (FINDING-1)
- Important: 4 (FINDING-2, 3, 4, 5)
- Minor: 5 (FINDING-6, 7, 8, 9, 10)
- Total: 10
