---
title: 'orchestrator: thread query namespace into Abstract plan (fix DowngradedFromAbstract empties)'
status: in_review
severity: high
priority: P1
filed: 2026-04-28
labels:
- retrieval
- orchestrator
- namespace
- root-fix
relates_to:
- ISS-049
- ISS-056
---

# ISS-059: Thread query namespace into Abstract plan inputs

## Summary

`HybridDispatchExecutor` and `execute_plan` hardcode
`AbstractPlanInputs.namespace = "default"` at two sites in
`crates/engramai/src/retrieval/orchestrator.rs` (lines ~622, ~839).
The hardcode was an interim from ISS-049 Phase 3 with a "Phase 4 wires
per-query namespace" TODO that never landed.

`Memory::graph_query` already extracts the per-query namespace from
`GraphQuery::namespace` (api.rs ~410) and constructs the real adapters
(`GraphTopicSearcher`, `StorageEpisodicStore`, `HybridAffectiveSeedRecaller`)
with the correct namespace. But the Abstract plan's *inputs* override the
adapter's namespace, so abstract retrieval always queries the literal
`"default"` namespace regardless of caller intent.

## Symptoms (LoCoMo conv-26, 2026-04-28 run)

7/25 EMPTY results in retrieval, P@5 = 0.04. Outcome breakdown of the EMPTYs:

- 4 × `outcome=downgraded_from_abstract` (Q4, Q12, Q15, Q22) — Abstract
  plan returns empty in `"default"` namespace, system downgrades to
  Associative which also has no seed → empty result.
- 2 × Hybrid `outcome=ok got=0` (Q13, Q21) — Hybrid sub-plan calls
  Abstract with the same hardcoded namespace; sub-result empty; fusion
  has nothing to rank.
- 2 × `outcome=no_cognitive_state` (Q18, Q25) — separate root cause
  (driver doesn't supply `self_state_override`); tracked in follow-up.

## Root cause

Two sites in `crates/engramai/src/retrieval/orchestrator.rs`:

```rust
// Line ~622 (HybridDispatchExecutor::dispatch_subplan, SubPlanKind::Abstract):
let inputs = AbstractPlanInputs {
    query: self.query,
    namespace: "default", // ← hardcoded
    budget: BudgetController::with_defaults(),
};

// Line ~839 (execute_plan, PlanLabel::Abstract):
let inputs = AbstractPlanInputs {
    query: query.text.as_str(),
    namespace: "default", // ← hardcoded
    budget: BudgetController::with_defaults(),
};
```

Both sites have access to the live `GraphQuery` (`self.query` and `query`
respectively), so `query.namespace.as_deref().unwrap_or("default")` is
the direct fix.

## Why this is root-fix not patch

- The TODO comment in the source (`Phase 4 wires per-query namespace`)
  explicitly documents this as deferred work, not a design choice.
- The real adapters are already wired — only the `inputs.namespace`
  argument is wrong.
- Plan classifier fallback or any "if abstract empty, skip" patch would
  paper over a wiring bug that has a 2-line root fix.

## Fix

Replace both `namespace: "default"` with
`namespace: query.namespace.as_deref().unwrap_or("default")`
(or `self.query` for the executor site).

Remove the `Phase 4 wires per-query namespace` TODO comments since
this issue closes them.

## Acceptance

- [x] Both call sites in `orchestrator.rs` thread `GraphQuery::namespace`
      into `AbstractPlanInputs`.
- [x] TODO comments referencing "Phase 4 wires per-query namespace" are
      removed from both sites.
- [x] `cargo test -p engramai` passes (1771 passed, 0 failed, 4 ignored).
- [ ] New unit test: Abstract plan called via `execute_plan` with a
      non-default namespace queries that namespace's `graph_topics` rows
      (not `"default"`). **Deferred** — orchestrator has no unit tests
      (per file-level comment at line 945; tests live in api.rs and
      need full `SqliteGraphStore` setup). Adding one requires either
      (a) building the api.rs end-to-end fixture with real graph data
      ingested under a custom namespace, or (b) introducing a new
      `RecordingTopicSearcher` test helper + minimal `GraphRead` stub.
      Both are tracked as the next step here, blocked on baseline LoCoMo
      data being re-ingested (see below).
- [ ] LoCoMo conv-26 re-run shows: 4 `downgraded_from_abstract` empties
      and 2 Hybrid `got=0` empties either return non-empty results or
      change outcome category. P@5 baseline (0.04) improves measurably.
      **Blocked**: the smoke graph DB at
      `.gid/issues/_smoke-locomo-2026-04-27/locomo-conv26-s1-3-postd991715.graph.db`
      currently has empty `graph_entities` / `knowledge_topics` tables
      (verified 2026-04-28 22:55Z). The 2026-04-28 v03-retrieval.log
      that showed P@5=0.04 must have been taken immediately after a
      ingest that has since been lost. Re-ingest is required before
      end-to-end validation can run. This is independent of ISS-059.
- [x] `outcome=no_cognitive_state` count unchanged at 2 (separate issue).

## Out of scope

- `NoCognitiveState` for affective queries (Q18, Q25) — needs driver to
  supply `self_state_override`. File as ISS-060 if not already tracked.
- Plan classifier improvements — separate concern.

## References

- ISS-049 (retrieval plan dispatch refactor — introduced the TODO)
- ISS-056 (per-query namespace on `GraphQuery` — the field this fix consumes)
- `crates/engramai/src/retrieval/orchestrator.rs:622,839`
- `crates/engramai/src/retrieval/api.rs:410` (where namespace is extracted but not threaded)
- /tmp/conv26-run-2026-04-28/v03-retrieval.log (symptom evidence)
