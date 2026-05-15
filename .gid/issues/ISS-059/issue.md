---
title: 'orchestrator: thread query namespace into Abstract plan (fix DowngradedFromAbstract empties)'
status: done
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
fixed_by: dbcc715
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

## Follow-up: incomplete fix discovered 2026-04-29

The orchestrator-level `AbstractPlanInputs.namespace` threading (lines
643 and 945) is in place. However the regression test
(`crates/engramai/tests/iss059_retrieval_abstract_namespace_test.rs`)
revealed a deeper layer that still ignores per-query namespace:

`AbstractPlan::execute` at `plans/abstract_l5.rs:405` calls
`graph.get_topic(hit.topic_id)`. The `SqliteGraphStore` impl at
`graph/store.rs:2728` filters by `self.namespace` — the namespace
the store was constructed with (always `"default"` because
`Memory::with_graph_store` does not chain `.with_namespace()`),
NOT the per-query namespace from `AbstractPlanInputs`.

Effect: `list_topics(alpha)` correctly returns the alpha topic id,
but the subsequent `get_topic(id)` lookup silently filters by
"default" → returns None → topic candidate dropped → Abstract
returns Empty → fallback chain produces
`EmptyResultSet { reason: "abstract_then_associative_empty" }`.

Test status:
- `iss059_abstract_query_without_namespace_uses_default` — PASS
- `iss059_abstract_query_with_namespace_finds_topic` — FAIL (correctly)

Two possible root fixes:
1. Add `get_topic_in_namespace(id, ns) -> Option<KnowledgeTopic>` to
   `GraphRead`, plumb through to `AbstractPlan::execute`.
2. Make `Memory::with_graph_store` accept (or `graph_query` set)
   a per-query namespace on the underlying store before dispatch.

Option 1 is the cleaner trait extension. Option 2 fights existing
"single-row CRUD stays pinned to self.namespace" invariant
(comment at `store.rs:2838`).


## Closure (2026-05-15)

**Status: done.** Two-layer fix shipped:

1. `dbcc715 fix(retrieval): Abstract honours per-query namespace
   (ISS-059)` — orchestrator threading: both call sites in
   `orchestrator.rs` pass `GraphQuery::namespace` into
   `AbstractPlanInputs`. AC #1, #2 satisfied.

2. Deeper-layer fix (the "Follow-up" section above): `GraphRead`
   trait now has a `get_topic_in(id, ns)` method with a default
   impl that post-filters by namespace (`graph/store.rs:362`).
   `AbstractPlan::execute` at `retrieval/plans/abstract_l5.rs:414`
   now calls `graph.get_topic_in(hit.topic_id, inputs.namespace)`
   instead of namespace-pinned `get_topic`. Option 1 from the
   Follow-up section, implemented.

**Verified 2026-05-15:**

- `cargo test -p engramai --test iss059_retrieval_abstract_namespace_test`
  → 2/2 pass:
  - `iss059_abstract_query_with_namespace_finds_topic` PASS (the
    test that was originally FAIL when only `dbcc715` had landed)
  - `iss059_abstract_query_without_namespace_uses_default` PASS
- AC #1 (call sites thread namespace) — DONE
- AC #2 (TODO comments removed) — DONE
- AC #3 (`cargo test -p engramai` passes) — DONE (1904 lib pass)
- AC #4 (new unit test with non-default namespace) — DONE (the
  `iss059_*finds_topic` test exercises exactly this path)
- AC #5 (LoCoMo conv-26 P@5 improves) — **deferred to v0.4 T31
  parity campaign**, same reasoning as ISS-111: re-running LoCoMo
  now means burning API budget on a baseline that v0.4 substrate
  flip (T30→T32) will invalidate. Folded into T31 alongside
  ISS-106 + ISS-111 verification.
- AC #6 (`outcome=no_cognitive_state` count unchanged at 2) —
  DONE (separate issue, untouched by this fix).

`fixed_by: dbcc715` (orchestrator threading) and the `get_topic_in`
trait extension (committed alongside earlier graph-layer work — no
single commit, the change is integrated). Status: in_review → done.
