---
title: Extend vector_score to Hybrid sub-plan Factual path (ISS-172 follow-up, mirror Strategy A)
status: deferred
priority: P2
severity: incomplete-fix
category: retrieval
created: 2026-05-27
relates:
- engram:ISS-148
- engram:ISS-172
- engram:ISS-173
blocked_by: engram:ISS-175
deferred_until: iss-179-ac-5a-redefine
---

## Summary

ISS-172 fix (commit `engram:ae4a2be`) threaded `query_embedding` into
`factual_to_scored`, emitting per-candidate cosine as `vector_score`.
But the fix only touched the **standalone Factual path** —
`orchestrator.rs:execute_plan` and `run_factual_fallback_for_hybrid`.

The **Hybrid sub-plan Factual path** (`HybridDispatchExecutor::run` at
`orchestrator.rs:798`, `SubPlanKind::Factual` arm) was deliberately left
alone per the Strategy A "minimal blast radius" decision.

## Why this is currently P2, not P1

Looking at the Hybrid sub-plan code more carefully (orchestrator.rs:798-870):

```rust
SubPlanKind::Factual => {
    let inputs = FactualPlanInputs { ... };
    let plan = FactualPlan::new();
    let exec_result = plan.execute(&inputs, resolver, self.graph, &mut budget);
    // ... converts exec_result.memories to HybridItem::Memory(id)
}
```

**The Hybrid sub-plan only takes `FactualPlanResult.memories` (a `Vec`
of `FactualMemoryRow { memory_id, seen_via }`) and projects to
`HybridItem::Memory(id)` — discarding any scores entirely.** Hybrid then
applies RRF over rank positions, not scores.

So even if we mirrored the Strategy A change into the Hybrid sub-plan
path, the vector_score would immediately be discarded — RRF only sees
rank position. **`factual_plan::execute` returns memories sorted by
`memory_id` ascending** (UUID lexicographic order, per
`memories_into_rows` comment "tests rely on this"), which means the
"rank" Hybrid feeds into RRF is **arbitrary lexicographic**.

## Real fix needed

For Hybrid sub-plan Factual to contribute meaningful rank, the
FactualPlan itself must rank its output by something more useful than
UUID order before Hybrid sees it. Options:

- **A**: Have FactualPlan accept an optional `query_embedding` and
  pre-sort `memories_into_rows` by cosine descending. Mirrors ISS-172
  Strategy A inside the plan rather than at the adapter boundary.
- **B**: Move the cosine computation up to the Hybrid sub-plan adapter,
  attaching scores before converting to `HybridItem::Memory`. Then
  Hybrid's RRF works on rank-by-cosine.

**Both A and B are blocked on ISS-175** — if Factual's standalone path
(which IS scored now) doesn't recover overall to ≥0.34 once fusion
weights are fixed, then making Hybrid match it doesn't help. ISS-175
must land first; then we measure whether the 4 SF qids that route
through Hybrid still suffer; then decide on Option A vs B.

## AC

- [ ] AC-1: After ISS-175 lands and a fresh sweep is run, re-classify
      the 9 SF qids by plan kind on the fixed substrate. If ≥4 still
      route Hybrid AND still miss gold, A/B test Option A.
- [ ] AC-2: Implement chosen option behind a feature flag (default off
      until cross-validated on conv-44).

## Status

**2026-05-28 — deferred.** ISS-175 reached `falsified-on-AC-5a`
(verdict: `kept-as-opt-in`) — combine_factual_v2 did NOT lift
single-fact AC-5a on conv-26 (+0.66pp overall, +1 SF flip out of
27). The premise of ISS-174 ("once ISS-175 lands productively,
extend the same wiring to Hybrid sub-plan") is invalidated:
extending a floor-fix to a second code path that consumes the
same falsified signal can't recover anything.

Re-promotion criteria:
- ISS-179 AC-5a redefines a target that ISS-175's combine_factual_v2
  actually improves (e.g. multi-hop axis where ISS-177 saw +18.9pp),
  AND
- A fresh per-query trace shows the 4 Hybrid-routed SF qids would
  benefit from cosine-based pre-ranking inside FactualPlan.

Until then, ISS-174 is dead weight — closing the loop honestly.
