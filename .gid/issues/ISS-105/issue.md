---
id: ISS-105
title: Hybrid sub-plans hardcode K, ignore query.limit (ISS-104 fix is incomplete)
kind: issue
status: in_progress
priority: high
labels:
- retrieval
- bug
- k-sweep
relates_to:
- ISS-104
- ISS-106
---

# ISS-105: Hybrid sub-plans hardcode K, ignore `query.limit`

## TL;DR

ISS-104 ("`query.limit` ignored in retrieval") was filed and "fixed" — but the
fix only patched the **Associative path** (1 plan) and the **fuse_rrf
truncation** in Hybrid (final stage). The **4 Hybrid sub-plans
(Factual / Episodic / Abstract / Affective) still hardcode their internal K**
and never read `query.limit`.

This means RUN-0023 (claimed K=50) was actually:

- **Sub-plan fanout**: still K=5 / K=10 / default (per-plan hardcoded)
- **Fuse stage**: now correctly takes top-50 of fused candidates ✅ (this is
  the only thing ISS-104 fix changed for the Hybrid path)

So the +6.6pp J-score in RUN-0023 (0.467 → 0.533) comes from
`take(top_k)` extension at fuse stage, NOT from larger sub-plan recall.

The K-sweep experiment we wanted to run (compare sub-plan recall at K=15 vs
K=50) is **still impossible** without this fix.

## Evidence (verified 2026-05-06)

File: `crates/engramai/src/retrieval/orchestrator.rs`

### Factual sub-plan (3 instantiation sites)
Lines 567–578, 805–816, 1159–1170:
```rust
let inputs = FactualPlanInputs {
    ...
    max_anchors: 5,                    // ← hardcoded, ignores query.limit
    memory_limit_per_entity: 50,       // ← hardcoded
    ...
};
```

### Episodic sub-plan (line 614–618)
```rust
let inputs = EpisodicPlanInputs {
    query: self.query,
    time_window: self.query.time_window.clone(),
    budget: BudgetController::with_defaults(),  // ← test default, ignores query.limit
};
```

### Abstract sub-plan (line 635+)
Same pattern — `BudgetController::with_defaults()`.

### Affective sub-plan (line 677+)
Same pattern — `BudgetController::with_defaults()`.

`BudgetController::with_defaults()` documentation (`budget.rs:336`):
> "Construct a controller with all defaults: no outer cap, no stage caps,
> design §7.3 cost caps. **Useful for tests.**"

This is being used in **production code paths** for Hybrid sub-plans.

## Impact

1. **K-sweep results are misleading.** Anything claiming "RUN-NNNN was K=50"
   for Hybrid path is technically wrong — only the fuse-stage truncation
   honors that K.
2. **Sub-plan recall ceiling is fixed**, regardless of `query.limit`. If a
   query needs >5 anchors (Factual) or >10 episodes (Episodic), the relevant
   memory is silently dropped before fusion ever sees it.
3. **multi-hop -18.9pp regression in RUN-0023** is unexplained but consistent
   with this bug: fuse-stage now keeps 50 candidates, but sub-plans still
   produce the same ~10–15 each, so the extra slots fill with lower-relevance
   distractors that RRF promotes inconsistently.

## Acceptance Criteria

- [ ] Factual sub-plan: `max_anchors` and `memory_limit_per_entity` derive
      from `query.limit` (or an explicit Hybrid budget config), not
      hardcoded `5` / `50`.
- [ ] Episodic / Abstract / Affective sub-plans: replace
      `BudgetController::with_defaults()` with a budget controller seeded
      from `query.limit` or a Hybrid-aware config.
- [ ] Verification: run the same query with `query.limit=15` vs
      `query.limit=50`; trace logs must show sub-plan candidate counts
      changing accordingly (NOT identical).
- [ ] Re-run LoCoMo K-sweep (K=15 vs K=50) post-fix; this is the first run
      where the K label is meaningful for Hybrid path.

## Out of Scope

- Adaptive / dynamic K (per-query). This issue is about plumbing the
  existing `query.limit` through. Smarter K selection is a separate concern.
- Generator context budget (separate truncation layer).

## References

- ISS-104 (parent fix; only addressed Associative path + fuse truncation)
- RUN-0020 (claimed K=15) and RUN-0023 (claimed K=50) eval-runs — neither
  measured what it claimed for Hybrid path
- `crates/engramai/src/retrieval/orchestrator.rs` lines 567, 614, 635, 677,
  805, 1159
- `crates/engramai/src/retrieval/budget.rs:337` (`with_defaults` docstring)
