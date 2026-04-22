# Requirements Review: ISS-016 — LLM Triple Extraction for Hebbian Link Quality

**Reviewer:** coder agent
**Date:** 2026-04-18
**Document:** `.gid/issues/ISS-016/requirements.md`
**Issue:** `.gid/issues/ISS-016-llm-triple-extraction.md`
**Depth:** Standard (Phase 0–5)

---

## Phase 0: Document Size

**CHECK: ≤15 GOALs?**

15 GOALs (8 P0 / 6 P1 / 1 P2) + 4 GUARDs (2 hard / 2 soft).

**PASS** — exactly at the 15-GOAL ceiling.

---

## Phase 1: Individual Requirement Quality

### GOAL-1.1 [P0] — Triple persistence and queryability
**PASS** — specific (survive restarts), testable (query by memory ID/subject/object/predicate), measurable, atomic.

### GOAL-1.2 [P0] — Triple fields (subject, predicate, object, confidence, source)
**PASS** — well-defined fields, "queryable independently" is testable via indexed columns. Matches the schema in the issue doc.

### GOAL-1.3 [P1] — Idempotent duplicate rejection
**PASS** — clear behavior (reject without error), directly maps to `UNIQUE(memory_id, subject, predicate, object)` constraint. Atomic and testable.

### GOAL-1.4 [P0] — Migration preserves existing data
**PASS** — testable (upgrade from pre-triple DB, verify all existing data intact). Existing codebase has migration precedent (`migrate_v2`, `migrate_embeddings`, `migrate_entities`, `migrate_hebbian_signals` in `storage.rs`).

### GOAL-2.1 [P0] — LLM extraction produces triples with confidence
**PASS** — clear input/output contract. "Zero or more" handles empty-content edge case.

### GOAL-2.2 [P1] — Predicate vocabulary constraint with fallback
**FINDING [minor]:** The requirement says "recognized predicate values" and "fallback relationship type" but does not name the fallback. The issue doc defines `related_to` as "Weak/fallback". The requirement should explicitly state that `related_to` is the fallback predicate, or reference the vocabulary table in the issue.

### GOAL-2.3 [P1] — Graceful handling of malformed LLM responses
**PASS** — clear behavior (zero triples + logged warning, never crash/panic). Testable. Aligns with GUARD-3.

### GOAL-2.4 [P1] — Few-shot examples in prompt
**FINDING [info]:** This is an implementation detail elevated to a requirement. It's testable (inspect prompt contains examples) but arguably leaks implementation. Acceptable at P1 since it constrains prompt quality rather than dictating architecture.

### GOAL-3.1 [P0] — Extraction runs during consolidation, not write path
**PASS** — clear constraint. Directly testable (verify `store()` does not call extraction). Aligns with GUARD-2.

### GOAL-3.2 [P0] — Batch processing of un-enriched memories
**PASS** — clear behavior (skip already-enriched, process un-enriched). Testable.

### GOAL-3.3 [P1] — Configurable batch size
**FINDING [minor]:** No default value specified. The requirement says "configurable" but doesn't state a sensible default or valid range. Implementor must guess. Recommend: specify default (e.g., 10) and minimum (1).

### GOAL-4.1 [P0] — Entity overlap includes triple-extracted entities
**PASS** — clear integration point. Maps directly to `SignalComputer::entity_jaccard()` in `src/association/signals.rs` and `Storage::get_entities_for_memory()` in `src/storage.rs`. The existing code takes `&[String]` entity slices — the union of Aho-Corasick + triple entities can be passed in.

### GOAL-4.2 [P0] — Graceful degradation without triples
**PASS** — clear fallback behavior. Existing memories continue working with Aho-Corasick only. Testable.

### GOAL-5.1 [P1] — Enable/disable triple extraction in config
**PASS** — clear behavior. Follows existing pattern (e.g., `AssociationConfig::enabled`, `EntityConfig::enabled`). Testable.

### GOAL-5.2 [P2] — Separate model config for triple extraction
**PASS** — clear, atomic. Low priority appropriately reflects it's an enhancement.

### GUARD-1 [hard] — No data loss
**PASS** — clear invariant. Testable via migration + round-trip tests.

### GUARD-2 [hard] — Hot path isolation
**PASS** — clear invariant. Testable by verifying no LLM calls in `store()` path. Redundant with GOAL-3.1 but appropriate as a guard (defense-in-depth).

### GUARD-3 [soft] — LLM failures are non-fatal
**FINDING [minor]:** States "memories that failed extraction are retried in future cycles" but no GOAL specifies retry mechanics. How are failed memories tracked? Is there a retry limit? Without this, a persistently failing memory could be retried infinitely every cycle.

### GUARD-4 [soft] — Backward compatibility with pre-migration DB
**PASS** — clear invariant. Testable. The condition "enabling extraction triggers migration automatically" is well-defined.

---

## Phase 2: Coverage & Gaps

### Happy Path
**PASS** — covered: store triple, extract from memory text, run during consolidation, feed into Hebbian signals, configure on/off.

### Error / Edge Cases
**FINDING [minor]:** No requirement covers what happens when a memory's text content is empty or extremely short (e.g., 1 word). GOAL-2.1 says "zero or more triples" which implicitly handles it, but an explicit edge case for minimal content would strengthen coverage.

**FINDING [major]:** No requirement addresses **concurrent consolidation safety**. The existing `run_consolidation_cycle()` in `src/models/consolidation.rs` uses transactions. If triple extraction (which makes slow LLM calls) runs inside the same transaction, it could hold the DB lock for extended periods. If it runs outside, there's a TOCTOU race between "identify un-enriched memories" and "store triples." The requirements should specify the concurrency/transaction model.

**FINDING [minor]:** No requirement covers **memory deletion cascading to triples**. The issue doc's schema uses `REFERENCES memories(id)` but doesn't specify `ON DELETE CASCADE`. If a memory is deleted, orphaned triples would remain. The existing `hebbian_links` table uses `ON DELETE CASCADE`. Requirements should specify cascade behavior.

### Non-Functional
**FINDING [info]:** No latency/throughput target for triple extraction itself (only that it doesn't affect the hot path). Acceptable since it runs in background consolidation, but a soft bound (e.g., "extraction should complete within consolidation cycle budget") could prevent surprise.

### Boundary Conditions
**FINDING [minor]:** No requirement specifies behavior when confidence score is outside [0.0, 1.0]. GOAL-1.2 says "confidence score" but doesn't constrain the range. The existing `ExtractedFact` uses `importance: f64` with 0.0-1.0 documented. Triple confidence should match.

### State Transitions
**PASS** — the state model is simple: memory goes from "no triples" → "has triples". GOAL-3.2 handles the transition detection (skip already-enriched).

---

## Phase 3: Consistency

### Internal Consistency
**PASS** — no contradictions between GOALs. GOAL-3.1 and GUARD-2 are redundant but consistent (GOAL = positive statement, GUARD = invariant).

### Terminology
**PASS** — "triple", "subject", "predicate", "object", "consolidation", "Aho-Corasick", "entity overlap" used consistently throughout and match the issue doc and codebase terminology.

### Priority Consistency
**PASS** — P0 covers core storage + extraction + integration. P1 covers robustness + config. P2 is purely cosmetic (model selection). Reasonable hierarchy.

### Cross-References
**PASS** — all GOALs have `(ref: ISS-016, ...)` cross-references to specific sections of the issue doc.

### GUARD vs GOAL Alignment
**FINDING [info]:** GUARD-3 mentions retry semantics ("retried in future cycles") that have no corresponding GOAL. This is acceptable as a guard-level constraint but creates an implicit requirement without explicit acceptance criteria. See Phase 1 GUARD-3 finding.

---

## Phase 4: Implementability

### Technology Assumptions
**PASS** — assumes SQLite (already in use), LLM backend via `MemoryExtractor` trait (already exists in `src/extractor.rs`). No new external technology required.

### External Dependencies
**PASS** — Dependencies section correctly identifies LLM provider, SQLite, Aho-Corasick, and association module. All exist in the codebase.

### Data Requirements
**FINDING [minor]:** The issue doc defines a 9-predicate vocabulary but the requirement (GOAL-2.2) doesn't specify the initial predicate set or where it's defined. Implementor needs to know: is the vocabulary hardcoded, config-driven, or defined in a separate file? Recommend: reference the issue doc's predicate table as the initial vocabulary, and specify whether it's extensible via config.

### Migration / Compatibility
**PASS** — GOAL-1.4 and GUARD-4 cover migration. The codebase has established migration patterns (`migrate_v2`, `migrate_embeddings`, `migrate_entities`, `migrate_hebbian_signals` in `storage.rs`) that can be followed.

### Scope Boundaries
**PASS** — Out of Scope section is explicit and matches the issue doc. Notably excludes "relation overlap as a fourth Hebbian signal" which the issue doc mentions as optional/future.

### Integration Point Specificity
**FINDING [minor]:** The requirements reference Hebbian entity_overlap but don't specify *where* the triple entities are merged with Aho-Corasick entities. Two plausible designs exist:
1. **Storage-level:** `get_entities_for_memory()` returns the union (triple subjects/objects are inserted into the `entities`/`memory_entities` tables).
2. **Compute-level:** `entity_jaccard()` caller merges the two entity sources before calling.

Option 1 is cleaner (transparent to callers) and matches the existing `memory_entities` pattern. The requirements should clarify, or at minimum the implementor should know this is a design decision.

---

## Phase 5: Traceability

### Unique IDs
**PASS** — all GOALs have unique IDs (GOAL-1.1 through GOAL-5.2), all GUARDs have unique IDs (GUARD-1 through GUARD-4).

### Grouping
**PASS** — GOALs are grouped into 5 logical categories: Triple Storage (1.x), LLM Extraction (2.x), Consolidation Integration (3.x), Hebbian Signal (4.x), Configuration (5.x).

### Dependency Graph
**FINDING [info]:** No explicit dependency ordering between GOALs, but the implicit order is clear:
- GOAL-1.x (storage) must be implemented before GOAL-2.x (extraction) and GOAL-3.x (consolidation integration)
- GOAL-2.x before GOAL-3.x (consolidation calls extraction)
- GOAL-1.x + GOAL-2.x before GOAL-4.x (Hebbian needs stored triples)
- GOAL-5.x (config) is cross-cutting

This is fine for a 15-GOAL document — explicit DAG would be overhead.

### Acceptance Criteria
**FINDING [minor]:** GOALs describe *what* but most lack explicit acceptance criteria or test scenarios. For example, GOAL-4.1 says "two memories sharing LLM-extracted entities produce a non-zero entity overlap score" — this is a good implicit acceptance criterion. Other GOALs (e.g., GOAL-2.4 "few-shot examples in the prompt") are harder to verify without explicit criteria. Acceptable at this document size but worth noting.

---

## Summary

| Phase | Pass | Findings |
|-------|------|----------|
| Phase 0: Document Size | ✅ | 0 |
| Phase 1: Requirement Quality | ✅ | 4 (1 info, 3 minor) |
| Phase 2: Coverage & Gaps | ⚠️ | 5 (1 info, 3 minor, 1 major) |
| Phase 3: Consistency | ✅ | 1 (1 info) |
| Phase 4: Implementability | ✅ | 2 (2 minor) |
| Phase 5: Traceability | ✅ | 2 (1 info, 1 minor) |
| **Total** | | **14 findings** (4 info, 9 minor, 1 major, 0 critical) |

---

## Recommendations

### Must Address (Major)

1. **Add concurrency/transaction model for consolidation-time extraction (Phase 2).** Triple extraction makes slow LLM calls. Specify whether extraction happens inside or outside the consolidation transaction. Recommended approach: identify un-enriched memories → release DB lock → make LLM calls → re-acquire lock → store triples. This avoids long-held locks and matches the existing pattern where `run_consolidation_cycle()` does its work in a bounded transaction.

### Should Address (Minor)

2. **Specify `related_to` as the explicit fallback predicate (GOAL-2.2).** The issue doc defines it; the requirement should name it.

3. **Add default and valid range for batch size (GOAL-3.3).** Suggest default: 10, minimum: 1, maximum: configurable but document a sensible upper bound.

4. **Add retry-limit or backoff for failed extractions (GUARD-3).** Without bounds, a memory with content that always fails extraction (e.g., binary data, extremely long text) will be retried every consolidation cycle forever. Suggest: max 3 retries, then mark as "extraction_failed" and skip.

5. **Specify ON DELETE CASCADE for triples → memories FK.** Match existing pattern from `hebbian_links` table. Add to GOAL-1.1 or create a sub-requirement.

6. **Constrain confidence score range to [0.0, 1.0] (GOAL-1.2).** Match existing `importance` range convention.

7. **Specify where triple entities merge with Aho-Corasick entities (GOAL-4.1).** Recommend storage-level integration: insert triple subjects/objects into `entities` + `memory_entities` tables with a `source = 'triple'` marker, so `get_entities_for_memory()` transparently returns the union.

8. **Specify predicate vocabulary location and extensibility (GOAL-2.2).** Reference the issue doc's 9-predicate table as the initial set. State whether it's hardcoded or config-driven.

### Nice to Have (Info)

9. **Consider a soft latency budget for extraction within a consolidation cycle.** Not critical since it's background work, but prevents a 1000-memory backlog from turning a consolidation cycle into a multi-hour operation.

10. **GOAL-2.4 (few-shot prompting) is implementation leakage** — acceptable at P1 but could be reframed as "extraction produces consistently structured output across diverse memory content" (the *what*) rather than "includes few-shot examples" (the *how*).

---

**Verdict:** The requirements document is well-structured, internally consistent, and closely aligned with the issue document and existing codebase. The single major finding (concurrency model) should be addressed before implementation begins. The minor findings are low-risk but would reduce ambiguity during implementation. **Approved with minor revisions recommended.**
