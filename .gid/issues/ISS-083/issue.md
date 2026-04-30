---
id: ISS-083
title: Hybrid plan emits empty_result_set instead of downgrading to Factual
status: open
priority: P1
severity: medium
tags: [retrieval, plan-dispatcher, locomo, hybrid]
created: 2026-04-30
relates_to: [ISS-070]
---

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
