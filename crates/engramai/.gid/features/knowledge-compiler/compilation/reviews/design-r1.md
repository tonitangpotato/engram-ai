# Design Review R1: Topic Compilation & Feedback

**Reviewer**: RustClaw  
**Date**: 2026-04-17  
**Document**: compilation/design.md  
**Depth**: standard (Phase 0-5)  
**Score**: 8/10

---

## Summary

The compilation design doc is well-structured with 6 components (within limit), clear data flow, and a solid traceability matrix. The GOAL mappings have been corrected and now accurately reflect the requirements. Main issues: some GOAL semantics are shifted from the requirements spec, missing GUARD coverage, and a few design gaps.

---

## Findings

### FINDING-1: GOAL-comp.1 scope mismatch [✅ Applied]

**Phase 1 — Check 5 (Completeness)**

Requirements `GOAL-comp.1` is "Topic Page Generation" (compilation), while `GOAL-comp.2` is "Automatic Topic Discovery". But in the design:
- §3.1 TopicDiscovery traces to GOAL-comp.1
- §3.2 CompilationPipeline traces to GOAL-comp.2, GOAL-comp.3

This is backwards. TopicDiscovery should satisfy GOAL-comp.2 (discovery), and CompilationPipeline should satisfy GOAL-comp.1 (page generation) + GOAL-comp.3 (provenance).

The §1.2 goals list and §6 traceability matrix need to be updated accordingly.

**Fix**: Swap GOAL annotations:
- §3.1 TopicDiscovery → GOAL-comp.2
- §3.2 CompilationPipeline → GOAL-comp.1, GOAL-comp.3
- Update §1.2 and §6 to match

### FINDING-2: GOAL-comp.3 is "Incremental Compilation", not "Provenance" [✅ Applied]

**Phase 3 — Check 12 (Internal consistency)**

Requirements `GOAL-comp.3` = "Incremental Compilation" (detect stale pages, recompile only affected). But the design maps comp.3 to CompilationPipeline's provenance tracking.

Provenance is covered by GOAL-comp.1's pass/fail criteria ("each insight traces to at least one source memory") and GUARD-4 ("Provenance Traceability").

GOAL-comp.3 should map to §3.3 IncrementalTrigger.

**Fix**: 
- §3.2 traces → GOAL-comp.1 (generation + provenance via GUARD-4)
- §3.3 traces → GOAL-comp.3 (incremental), GOAL-comp.4 (merge triggers), GOAL-comp.5 (split triggers)

Wait — that's also wrong. Let me re-check:
- GOAL-comp.4 = "Topic Merging" → §3.4 TopicLifecycle ✅
- GOAL-comp.5 = "Topic Splitting" → §3.4 TopicLifecycle ✅

So §3.3 IncrementalTrigger should trace to GOAL-comp.3 only. Currently it traces to comp.4 and comp.5 which are merge/split goals.

**Fix**:
- §3.3 IncrementalTrigger → GOAL-comp.3
- §3.4 TopicLifecycle → GOAL-comp.4, GOAL-comp.5, GOAL-comp.6
- Update §1.2 component table and §6 matrix

### FINDING-3: GOAL-comp.6 (Cross-Topic Linking) has no dedicated component [✅ Applied]

**Phase 2 — Check 7 (Coverage)**

Requirements `GOAL-comp.6` = "Cross-Topic Linking" — automatic links between topics based on shared entities, memories, and embedding similarity.

The design maps comp.6 to TopicLifecycle (§3.4), but TopicLifecycle only handles merge/split. Cross-topic linking is a distinct capability — it doesn't change topic structure, it creates relationship edges.

**Options**:
A. Add cross-topic linking logic to CompilationPipeline (compute links during compilation)
B. Create a 7th component "CrossTopicLinker" (still within ≤8 limit)
C. Add it to TopicLifecycle's `analyze()` method

**Recommendation**: Option A — linking is a post-compilation step, fits naturally in the compilation flow. Add a `compute_links()` step after `compile_new()` / `recompile_*()`.

### FINDING-4: GUARD coverage incomplete [✅ Applied]

**Phase 4 — Check 13 (Architecture consistency)**

The design mentions GUARDs in passing but doesn't explicitly show how each GUARD is satisfied:

- **GUARD-1** (Engram-Native): Implicitly satisfied — all code in `src/compiler/`. Not stated.
- **GUARD-2** (Incremental, Not Batch): Satisfied by §3.3 IncrementalTrigger. Not explicitly stated.
- **GUARD-3** (LLM Cost Awareness): **NOT addressed**. No component tracks or budgets LLM token costs. The `CompilationRecord` tracks `model_used` but not cost. No cost threshold enforcement.
- **GUARD-4** (Provenance Traceability): Satisfied by §3.2's citation parsing. ✅
- **GUARD-5** (Non-Destructive): Not explicitly addressed (should state that compilation never mutates source memories).
- **GUARD-6** (Offline-First): Not addressed — should note that compiled pages are queryable without LLM.

**Fix**: Add a "§ GUARD Compliance" section mapping each GUARD to how this feature satisfies it. At minimum, add GUARD-3 compliance (token cost tracking + budget threshold) to CompilationPipeline.

### FINDING-5: GOAL-comp.10 (Failure Handling) design is thin [✅ Applied]

**Phase 2 — Check 8 (Error/edge case coverage)**

GOAL-comp.10 requires: "preserve previous version, log failure, mark still-stale, don't roll back successful pages in same batch, record LLM costs for failed compilations."

The design mentions failure handling implicitly in §3.2 (citation parsing fallback) and §3.6 (QualityScorer traces to comp.10), but there's no explicit failure handling flow. QualityScorer is about quality assessment, not failure handling.

**Fix**: Add explicit failure handling to §3.2 CompilationPipeline:
- `compile_new()` / `recompile_*()` returns `Result<..., CompilationError>`
- On failure: previous page version retained, page marked stale, error logged with cost
- Batch orchestrator continues compiling other pages after one failure

### FINDING-6: FeedbackProcessor conflates comp.7 and comp.8 [✅ Applied]

**Phase 1 — Check 4 (Atomicity)**

§3.5 FeedbackProcessor traces to GOAL-comp.8 (point-level feedback) and GOAL-comp.9 (dry run). But the GID graph has a separate task `kc-manual-edit` for GOAL-comp.7 (manual topic edit), which is a P0 requirement.

The design doesn't have a component for manual editing. FeedbackProcessor handles feedback (ratings, corrections) but not direct editing of topic page content.

**Fix**: Either:
A. Add manual edit handling to FeedbackProcessor (user edits = special feedback type)
B. Add it to CompilationPipeline (edit preservation during recompilation)

The requirements say "user edits are preserved across recompilation" — this is a CompilationPipeline concern (recompile_partial must detect and preserve user-edited sections).

### FINDING-7: GOAL-comp.9 (Dry Run) mapped to FeedbackProcessor incorrectly [✅ Applied]

**Phase 3 — Check 12 (Internal consistency)**

GOAL-comp.9 = "Compilation Dry Run" — preview what would change + estimated cost. This is not a feedback feature. It's a compilation orchestration feature (preview mode).

Should be in CompilationPipeline or a top-level orchestrator, not FeedbackProcessor.

**Fix**: Move dry-run to CompilationPipeline as `compile_dry_run()` method, or create an orchestrator component.

### FINDING-8: No cross-reference to existing engram code [✅ Applied]

**Phase 5 — Check 17 (Technology assumptions)**

The design references `discover_clusters()` from `synthesis::cluster` and `SynthesisLlmProvider` but doesn't specify:
- Whether these already exist or need to be created
- The exact module path in the engram crate
- Whether `SynthesisLlmProvider` is the same as or different from the platform's `LlmProvider` trait (§2.1 of platform design)

**Fix**: Add a brief "§ Integration with Existing Code" section noting which symbols already exist vs need creation, and clarify the relationship between `SynthesisLlmProvider` and `LlmProvider`.

---

## Score Breakdown

| Area | Score | Notes |
|------|-------|-------|
| Structure & completeness | 8/10 | Good component split, within limits |
| GOAL traceability | 6/10 | Multiple mappings still wrong after rewrite |
| GUARD compliance | 5/10 | GUARD-3 not addressed, others implicit |
| Logic correctness | 9/10 | Algorithms are sound |
| Edge cases | 7/10 | Failure handling thin, manual edit missing |
| Trade-offs | 9/10 | §5 is well-reasoned |

**Overall: 8/10** — Architecturally sound but traceability needs another pass.
