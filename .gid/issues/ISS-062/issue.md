---
title: 'Strengthen retrieval observability: distinguish stub/downgrade/empty outcomes across plans and sub-plans'
status: todo
priority: P1
labels:
- retrieval
- observability
- logging
relates_to:
- ISS-060
- ISS-061
writeup: .gid/docs/retrieval-downgrade-contract-problem.md
---

# Strengthen retrieval observability

## Problem

The current retrieval logging conflates three very different states under a
single `outcome=ok candidates=0` line:

1. **Stub-empty** — the plan/sub-plan is not implemented and silently
   returns `Ok(vec![])`. Looks identical to a real run that found nothing.
2. **Designed-downgrade-empty** — the plan is implemented but a precondition
   isn't met (e.g. Episodic without `time_window`, Abstract without topic),
   so it correctly returns 0 by design.
3. **Real-empty** — the plan ran fully and the corpus genuinely has no
   matches.

This conflation directly caused two recent diagnostic mistakes:

- ISS-061 was filed assuming Hybrid was a silent stub; investigation showed
  Hybrid is fully implemented and was a downstream victim of empty
  sub-plans. Logs gave no signal.
- ISS-060 (Abstract returns 0) reads in logs as `outcome=ok candidates=0`
  with no indication that the downgrade chain bottomed out.

This is a core principle violation per SOUL.md: *root fix not patch* and
*想清楚再写* — we cannot reason about retrieval correctness when the logs lie.

## Scope

Audit and strengthen logging across the retrieval module. At minimum:

### Plans (`crates/engramai/src/retrieval/plans/*.rs`)

For every plan and sub-plan executor, the EXIT log line MUST distinguish:

- `outcome=ok items=N` — ran fully, returned N (N may be 0 if corpus genuinely empty)
- `outcome=downgraded reason=<why>` — precondition missing, returned 0 by design
- `outcome=stub` — adapter is not implemented (must NEVER look like `ok`)
- `outcome=error reason=<why>` — failure path

Affected plans (each one needs explicit categorization at every empty-return site):
- `factual.rs` — already returns real results, but add downgrade reasons
- `episodic.rs` — `DowngradedFromEpisodic` paths (no time_window, etc.)
- `abstract_l5.rs` — downgrade chain in ISS-060
- `affective.rs` — `DowngradedNoSelfState`
- `bitemporal.rs` — currently a stub? verify
- `associative.rs` — currently a stub? verify
- `hybrid.rs` — already logs sub-plan ENTER/EXIT; add aggregate outcome
  (`stub_no_subplan_candidates` when all sub-plans empty; `ok` otherwise)

### Hybrid sub-plan adapters (`orchestrator.rs::HybridDispatchExecutor`)

Currently logs `hybrid_sub_plan EXIT sub_kind=X items=N` — extend to
`items=N outcome=<ok|downgraded|stub> reason=<...>`.

### Dispatch (`dispatch.rs`)

When the classifier picks `PlanKind::Hybrid` but the underlying signal
strengths suggest a single dominant intent, log the routing decision with
signal scores so we can see *why* a query went to Hybrid.

### Eval harness (`crates/engramai-eval/`)

Per-query output line should include the outcome category, not just
`outcome=ok`. Today: `[N/M] ✗ cat=4 hit=false plan=Hybrid outcome=ok got=0`.
After: `[N/M] ✗ cat=4 hit=false plan=Hybrid outcome=stub_no_subplan_candidates got=0`.

## Non-goals

- This issue does NOT change retrieval behavior. Pure observability.
- Does NOT add new metrics aggregation infra (out of scope; separate issue
  if wanted).

## Acceptance

- Every `Ok(vec![])` return path in `retrieval/plans/*.rs` has an explicit
  outcome category in its log.
- A grep for `outcome=ok candidates=0` after a conv-26 run shows ONLY
  queries where the corpus is genuinely empty (or proves none exist for
  conv-26).
- conv-26 re-run produces logs that, by inspection alone, distinguish
  ISS-060-style downgrades from real-empty from stubs.
- Test: a unit test per plan asserting the outcome string for an
  empty-return case.

## Why P1 not P0

Doesn't block functionality, but every retrieval debugging session from now
on benefits. Worth doing before chasing more `candidates=0` mysteries.

## Related

- ISS-060 — Abstract returns 0; logs misleading
- ISS-061 — Hybrid sub-plan empty cascade; misdiagnosed initially because logs lied
