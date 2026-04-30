---
id: ISS-086
title: Hybrid→Factual fallback returns empty even when direct Factual succeeds
status: open
priority: P1
severity: medium
tags: [retrieval, plan-dispatcher, hybrid, fallback, locomo]
created: 2026-04-30
relates_to: [ISS-083]
---

# Hybrid→Factual fallback returns empty even when direct Factual succeeds

## Symptom

The ISS-083 Hybrid→Factual fallback wiring (`run_factual_fallback_for_hybrid`
in `crates/engramai/src/retrieval/orchestrator.rs`) does not actually surface
results, even when the substrate clearly contains them.

Concretely, in the integration test
`crates/engramai/tests/iss083_hybrid_downgrade_test.rs::hybrid_downgrades_to_factual_when_subplans_empty`:

1. We ingest `"Alice met Bob in the lab today"` so the deterministic
   extractor produces an `Alice → RelatedTo → Bob` triple. Entity count
   in the graph DB > 0 by the time we query.
2. **Sanity check passes**: `GraphQuery::new("Alice").with_intent(Intent::Factual)`
   returns non-empty `results`. Substrate is fine, resolver is fine.
3. **Failure**: the same `GraphQuery::new("Alice").with_intent(Intent::Hybrid)`
   reaches the Hybrid arm, every sub-plan is empty (expected — no L5 / no
   time expression), `run_factual_fallback_for_hybrid` is invoked, and the
   Factual `plan.execute(...)` call returns `Ok(_)` with `factual_to_scored`
   producing an **empty** Vec. So we end up with
   `EmptyResultSet { reason: "hybrid_subplans_empty_factual_also_empty" }`
   rather than `DowngradedFromHybrid { reason: "subplans_empty_factual_recovered" }`.

## What we've ruled out

- **Substrate is OK.** The direct-Factual sanity-check assertion in the
  same test passes immediately above the failing assertion. Same DB,
  same `Memory` instance, same `EntityResolver`.
- **`plan.execute(...)` is not erroring.** We added an `Err`-branch log
  in `run_factual_fallback_for_hybrid`; it never fires. The plan is
  returning `Ok` with a non-error result whose contents `factual_to_scored`
  flattens to zero `ScoredResult`s.
- **Test is reaching the fallback.** The "fallback ENTER" `log::info!`
  fires; budget refresh + collaborators wiring all execute.

## Suspects (in order of likelihood)

1. **`factual_to_scored` shape mismatch.** The `FactualPlanResult`
   produced inside the fallback context may have a different shape (e.g.
   `anchors` populated but `evidence` / per-entity memory rows empty)
   than the one produced via the top-level `PlanKind::Factual` arm. The
   direct-Factual path may go through a different `factual_to_scored`
   call site or a slightly different `FactualPlanInputs` configuration
   that produces non-empty `evidence`.
2. **`collaborators.entity_resolver` mutated by prior Hybrid steps.**
   The `EntityResolver` is shared via `&PlanCollaborators<'_>`. The
   Hybrid sub-plan executor (`HybridPlan::execute`) runs first and may
   internally mutate resolver-side caches / budget state in a way that
   makes the second resolution call (inside `run_factual_fallback_for_hybrid`)
   miss. Direct-Factual runs cleanly; fallback-Factual runs after a
   no-op Hybrid pass.
3. **Budget short-circuit.** Even though we construct a fresh
   `BudgetController::with_defaults()` inside the fallback helper, some
   downstream component (resolver, loader) may consult budget state on
   the original `RetrievalContext` / collaborators struct, which by the
   time we hit the fallback has been drained by the Hybrid sub-plans.
   Result: silent zero-candidate return without an `Err`.

## Reproducer

`crates/engramai/tests/iss083_hybrid_downgrade_test.rs` —
`hybrid_downgrades_to_factual_when_subplans_empty`. The test is
`#[ignore]`'d on the ISS-083 branch with reason
`"reproducer for ISS-086: Hybrid→Factual fallback returns empty"`.

To reproduce:

```bash
cd /Users/potato/clawd/projects/engram
RUST_LOG=engramai::retrieval=debug \
  cargo test -p engramai --test iss083_hybrid_downgrade_test \
  -- --ignored --nocapture
```

Expected (after fix): both the direct-Factual sanity-check and the
Hybrid-with-fallback assertion pass; outcome is
`DowngradedFromHybrid { reason: "subplans_empty_factual_recovered" }`.

Current: sanity-check passes, Hybrid fallback assertion fails — outcome
is `EmptyResultSet { reason: "hybrid_subplans_empty_factual_also_empty" }`
even though Factual just succeeded against the same substrate.

## Suggested debug path

Compare, side by side, the state observed at:

- **Call site A — direct Factual** (`PlanKind::Factual` arm in
  `RetrievalEngine::execute_plan`, `crates/engramai/src/retrieval/orchestrator.rs`).
- **Call site B — fallback Factual** (`run_factual_fallback_for_hybrid`,
  same file).

Specifically dump:

1. `FactualPlanInputs` field-by-field (the inputs constructor in B
   mirrors A, but verify there's no drift — `max_anchors`,
   `memory_limit_per_entity`, `predicate_filter`, `entity_filter`).
2. `EntityResolver` internal state before the call (any cache /
   per-query state that A starts fresh with but B inherits from
   Hybrid).
3. `BudgetController` reference identity — confirm that the resolver /
   loader don't reach back to `collaborators.budget` (or whatever the
   shared struct exposes) instead of the local `&mut budget`.
4. `FactualPlanResult` returned by `plan.execute(...)` in both paths,
   serialized — look for empty `evidence` / `anchors` / `memory_rows`
   in B vs populated in A.
5. The intermediate values inside `factual_to_scored` (loader hits,
   row construction) — does the loader find rows in A but not in B?

A focused unit test exercising `run_factual_fallback_for_hybrid`
directly (bypassing the orchestrator wrapper, constructing a
`PlanCollaborators` that mimics post-Hybrid state) would isolate
suspect 2 vs 3 quickly.

## Why this is P1 (not P0)

- **Doesn't block 0.4.0**: ISS-083's main wiring (variant + metrics +
  helper + dispatcher arm + doc test) lands; the `EmptyResultSet`
  → `DowngradedFromHybrid` distinction is correctly modelled in the
  type system and metrics. The terminal-empty case is also correctly
  reached.
- **But the fallback is effectively dead.** Until this is fixed, every
  Hybrid query whose sub-plans are empty terminates as
  `hybrid_subplans_empty_factual_also_empty` even when Factual would
  recover. Net behaviour matches the ISS-063 placeholder (empty result
  with a reason string) — no regression, but no real win either.

Fix-before: 0.4.1 retrieval polish window, OR before any LoCoMo run
that needs Hybrid recall ≥ Factual recall as a precondition.

## References

- ISS-083 — parent issue (Hybrid arm wiring)
- `crates/engramai/src/retrieval/orchestrator.rs::run_factual_fallback_for_hybrid`
- `crates/engramai/tests/iss083_hybrid_downgrade_test.rs`
- ISS-070 — multi-hop dispatcher (related fallback pattern, different bug)
