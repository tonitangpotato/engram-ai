---
id: ISS-083
title: Hybrid plan emits empty_result_set instead of downgrading to Factual
status: in-progress
priority: P1
severity: medium
tags: [retrieval, plan-dispatcher, locomo, hybrid]
created: 2026-04-30
updated: 2026-04-30
relates_to: [ISS-070, ISS-086]
---

## 2026-04-30 — Status update: main work landed, integration AC blocked on ISS-086

**Summary:** Type-system + dispatcher + metrics + unit-test work is complete.
Integration test is written and committed but `#[ignore]`'d because it
exposed a separate, deeper bug now tracked in **ISS-086**.

### What landed

- `RetrievalOutcome::DowngradedFromHybrid { reason }` variant
  (`crates/engramai/src/retrieval/outcomes.rs`).
- Metrics counter `downgraded_from_hybrid_total` + helper +
  `MetricsSnapshot` field (`crates/engramai/src/retrieval/metrics.rs`).
- `run_factual_fallback_for_hybrid` helper in
  `crates/engramai/src/retrieval/orchestrator.rs` — refreshes budget,
  calls `FactualPlan::execute`, reuses `factual_to_scored`, returns
  `Some(scored)` on non-empty / `None` on empty.
- `PlanKind::Hybrid` arm in `execute_plan` rewired: when every
  sub-plan returns empty, invoke fallback helper. Non-empty fallback →
  `DowngradedFromHybrid`. Empty fallback → terminal `EmptyResultSet`
  with reason `"hybrid_subplans_empty_factual_also_empty"`.
- Doc test in `crates/engramai/src/retrieval/api.rs` updated to cover
  the `DowngradedFromHybrid` outcome shape.
- Unit tests: 309 pre-existing retrieval tests still green; doc tests
  green.
- Reproducer integration test:
  `crates/engramai/tests/iss083_hybrid_downgrade_test.rs` —
  `#[ignore = "reproducer for ISS-086: Hybrid→Factual fallback returns empty"]`.

### Acceptance criteria — final state

- [x] No `empty_result_set` from Hybrid when sub-plans empty **and**
  Factual would recover. *(Covered structurally by the dispatcher arm
  + DowngradedFromHybrid emission; behavioural verification on real
  substrate blocked by ISS-086.)*
- [x] Outcome `downgraded_from_hybrid` emitted when fallback recovers.
  *(Type + metric + emission point all in place.)*
- [x] Doc test added covering the downgrade outcome shape.
- [x] Unit tests for outcome + metrics: 309 retrieval tests green.
- [ ] Integration test asserting non-empty results on a real substrate
  with a Hybrid-classified query → **blocked by ISS-086**, test is
  `#[ignore]`'d. Re-enable once ISS-086 is fixed.
- [ ] Re-run RUN-0009 retrieval; verify zero `Hybrid+empty_result_set`
  outcomes. *(Defer until ISS-086 resolved — without that fix the
  outcome would still be empty, just with a different reason string.)*

### Note on the integration test (→ ISS-086)

The reproducer (`hybrid_downgrades_to_factual_when_subplans_empty`)
ingests a triple, then asserts:

1. **Sanity**: direct `Intent::Factual` query for "Alice" returns
   non-empty. ✅ Passes.
2. **Goal**: same query under `Intent::Hybrid` triggers the new
   fallback path and returns non-empty. ❌ Fails — fallback's
   `plan.execute(...)` returns `Ok(_)` whose `factual_to_scored`
   output is empty, despite (1) succeeding moments earlier on the
   same substrate.

This is a behavioural bug in the fallback execution context (resolver
state? budget? result-shape mismatch?), **not** in the dispatcher
wiring this issue addresses. Tracked separately as ISS-086 with full
debug surface + suspect list.

### Resolution plan

Keep ISS-083 **in-progress** until ISS-086 is fixed and the integration
test can be un-ignored and asserted green. At that point:

- Remove `#[ignore]` on `hybrid_downgrades_to_factual_when_subplans_empty`.
- Mark the last two AC items.
- Close ISS-083.

---

## Original issue (filed 2026-04-30)


# Hybrid plan emits empty_result_set instead of downgrading to Factual

## Symptom

In RUN-0009 (full conv-26 substrate retrieval, 199 QAs), the `Hybrid` retrieval plan was selected by the classifier 10 times. **All 10 returned zero candidates** with `outcome=empty_result_set`:

```
[82/197]  cat=4 gold=D2:3   plan=Hybrid empty
[117/197] cat=4 gold=D10:14 plan=Hybrid empty
[144/197] cat=4 gold=D18:5  plan=Hybrid empty
[146/197] cat=4 gold=D18:5  plan=Hybrid empty
[150/197] cat=4 gold=D18:17 plan=Hybrid empty
[151/197] cat=5 gold=D2:3   plan=Hybrid empty
[174/197] cat=5 gold=D10:14 plan=Hybrid empty
[192/197] cat=5 gold=D18:5  plan=Hybrid empty
[194/197] cat=5 gold=D18:5  plan=Hybrid empty
[196/197] cat=5 gold=D18:17 plan=Hybrid empty
```

Five distinct gold dialogues (D2:3, D10:14, D18:5, D18:17, plus duplicate of D18:5) appear once in cat=4 and again in cat=5 — these are LoCoMo's paraphrased / adversarial pairs. Substrate has the gold evidence (other queries hitting the same memories from Factual plan succeed); the Hybrid plan just doesn't return anything.

For comparison, in the same run:
- `Abstract` plan: 25 selections, 9 hits, 0 empty (the 25 all `downgraded_from_abstract` because L5 substrate isn't built — **but Abstract still returns Factual candidates after downgrade**).
- `Hybrid`: 10 selections, 0 hits, 10 empty — **never downgrades**.

So this is a real bug: `Hybrid` is the only plan that, when its sub-substrate is unavailable, returns empty instead of falling back.

## Root cause (preliminary, needs binary verification with `RUST_LOG=engramai::retrieval=debug`)

`Hybrid` is supposed to combine multiple sub-plans (Factual + Abstract + multi-hop / temporal evidence). Its execution path inside `RetrievalEngine::execute_plan` depends on:

- Working **abstract substrate (L5)** — currently absent on this DB; Abstract plan itself only works by downgrading.
- Working **multi-hop traversal** — ISS-070, dispatcher has no traversal step.

When neither sub-component returns candidates, `HybridPlan` apparently emits `empty_result_set` directly rather than falling through to a working plan. The `Abstract` arm has a downgrade path; the `Hybrid` arm does not.

This needs to be confirmed by running:

```bash
RUST_LOG=engramai::retrieval=debug cargo run --release \
  --example locomo_conv26_retrieval -- \
  --db .../locomo-conv26-full.db \
  --graph-db .../locomo-conv26-full.graph.db \
  --dataset locomo10.json \
  --max-session 19 --limit 5 --ns locomo-conv26-full
```

The debug log should show the per-sub-plan dispatch sequence inside the Hybrid arm and where the chain terminates with zero candidates.

## Why it matters

- **Silent failure mode.** 10/199 QAs (5%) silently return empty without any actionable diagnostic. We only noticed because we read the per-outcome breakdown.
- **Wrong floor.** Even with all sub-substrates absent, `Hybrid` should be able to fall back to the same Factual retrieval path that 152 other queries used successfully. Returning empty is strictly worse than returning the Factual candidates.
- **Blocks proper measurement.** Until `Hybrid` downgrades, we cannot tell whether multi-modal queries would benefit from a real Hybrid implementation. Right now they get punished for being classified as Hybrid.

## Proposed fix

In `RetrievalEngine::execute_plan` Hybrid arm:

1. Try each sub-plan in priority order (Factual → Episodic → Abstract).
2. Union results.
3. If **all** sub-plans return zero candidates, emit `outcome=downgraded_from_hybrid` and re-dispatch the original query as `Factual` (the always-available best-effort plan).
4. **Never silently return `empty_result_set`** from Hybrid unless every sub-plan was actually attempted *and* the substrate has no relevant content at all (in which case Factual would also have returned empty, not Hybrid).

### Cheaper interim mitigation

In `query_classifier`, only classify `Hybrid` when at least one of {abstract_available, multi_hop_traversal_available} is true. When neither is true, classify as Factual directly. This makes classification substrate-aware and avoids the bad path entirely until the proper fix lands.

## Acceptance criteria

- [ ] On the same RUN-0009 substrate, re-running retrieval shows 0 occurrences of `outcome=empty_result_set` from `plan_used=Hybrid`.
- [ ] At least one of {downgraded_from_hybrid, ok} replaces the empty outcome for the 10 currently-empty queries.
- [ ] The 10 currently-empty Hybrid queries return at least the same candidates that `Factual` would have returned for the same query (verified by side-by-side run with classifier forced to Factual).
- [ ] Test added: `tests/retrieval/hybrid_downgrade.rs` — substrate with no abstract data, query that classifies as Hybrid, assert non-empty result and `downgraded_from_hybrid` outcome.

## Out of scope

- Building real L5 abstract substrate (separate, larger work).
- Wiring multi-hop traversal into Hybrid (depends on ISS-070 + anchor-resolution-v2 design).
- Improving Hybrid's classification (e.g., reducing false positives where Factual would suffice). This issue is only about the downgrade path when classification has already happened.

## References

- RUN-0009 report: `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv-report.md` §3
- Raw log: `.gid/eval-runs/RUN-0009-substrate/RUN-0009-full-conv26.log`
- Related: ISS-070 (multi-hop dispatcher has no traversal — different bug, similar surface)
