---
title: AssociativePlan k_seed=10 silently caps fused candidate pool at ~10, breaking K-sweep experiments
status: in_progress
priority: P1
opened: 2026-05-06
opened_by: rustclaw-autopilot
labels: retrieval, locomo, root-cause, partial-fix
relates_to: ISS-001, ISS-103, ISS-105
---

# Summary

`AssociativePlan` is constructed in `orchestrator.rs:921` and `:1240` via `AssociativePlan::new(seed_recaller)` with no `.with_k_seed(...)` override. The default `K_seed = 10` (associative.rs:60) is used regardless of the driver-supplied `query.limit`. This silently caps the fused candidate pool at ~10 items even when callers ask for top-K=15, 20, 50, etc.

**Verified empirically from RUN-0020 jsonl:** 145/152 LoCoMo queries returned exactly 10 candidates, only 7/152 returned the requested 15. Mean = 10.23.

This invalidates the K-sweep interpretation of RUN-0019 (K=5) → RUN-0020 (K=15) +4.6pp. The real difference was effective K=5 → K=10, not K=5 → K=15. RUN-0021 / RUN-0022 (K=50) **could not test K=50** at all — the pool ceiling stayed at ~10.

# Root Cause

`HybridSeedRecaller` already takes `k_seed` as a parameter and overfetches `k_seed * 3` (capped at 200) — the bottleneck is **not** in retrieval, it's in plan construction. `K_seed` is never passed through from `query.limit`.

# Fix Applied (2026-05-06)

`engramai/src/retrieval/orchestrator.rs:921` and `:1240` — added `.with_k_seed(query.limit)` to both `AssociativePlan::new()` call sites.

```rust
let plan = crate::retrieval::plans::associative::AssociativePlan::new(
    collaborators.seed_recaller,
)
.with_k_seed(query.limit);
```

`K_pool = 100` is wide enough to absorb K_seed up to ~33 without breaking the §4.3 expansion budget. Larger `query.limit` values are clamped naturally by `K_pool` rather than raising it (preserves §7.3 cost caps).

# Verification Plan

- [x] Edit applied
- [x] `cargo check -p engramai --release` passes
- [x] `cargo build --release --bin engram-bench` passes
- [ ] **Smoke (in progress, PID 46345 started 2026-05-06T05:07Z)**: run K=50 LoCoMo with new binary, verify jsonl `retrieved_candidates` length > 10 for majority of queries
- [ ] If smoke confirms: archive as RUN-0023, compare J-score vs RUN-0020 (K=15 effective K=10)
- [ ] If smoke shows candidates still capped at ~10: investigate `K_pool` or fusion-stage truncation as next bottleneck

# ⚠️ Partial Fix — Re-Opened 2026-05-06

The fix above only addresses the **Associative path** and the **Hybrid
fuse_rrf truncation**. It does NOT fix the 4 Hybrid sub-plans
(Factual / Episodic / Abstract / Affective), which still hardcode their
own K and ignore `query.limit`.

**Concretely** — verified by reading `orchestrator.rs` 2026-05-06:
- Factual sub-plan (lines 567, 805, 1159): `max_anchors: 5,
  memory_limit_per_entity: 50` — hardcoded.
- Episodic sub-plan (line ~614): uses `BudgetController::with_defaults()`,
  whose docstring (`budget.rs:337`) reads "Useful for **tests**".
- Abstract sub-plan (line ~635): same.
- Affective sub-plan (line ~677): same.

**What this means for RUN-0023 (claimed K=50):**
- Sub-plan fanout: still ~5–10 each (hardcoded / test defaults).
- fuse_rrf final truncate: now correctly takes top-50 of fused candidates ✅.
- So the +6.6pp J-score (0.467 → 0.533) comes from **the fuse stage no
  longer truncating to 15**, not from larger sub-plan recall.

**What this means for the K-sweep:**
- The K-sweep experiment we wanted (compare sub-plan recall at K=15 vs
  K=50) is **still impossible** without the broader fix.
- All RUN-NNNN summaries claiming "K=N" for Hybrid path are misleading and
  need a footnote.

**Tracked separately in ISS-105.** This issue stays `in_progress` until
ISS-105 lands and end-to-end K plumbing is verified.

# Why This Wasn't Caught Earlier

- The `with_k_seed` builder method was added during initial associative plan implementation but never wired through orchestrator.
- Tests use `AssociativePlan::default()` or `AssociativePlan::new()` and assert on candidate counts ≤ K_seed, so they pass by definition.
- No integration test asserts that `query.limit = N` produces approximately N candidates from the orchestrator's full pipeline.

# Follow-Ups

- Add an integration test in `engramai/tests/` that runs orchestrator end-to-end with `query.limit = 50` against a stub graph with ≥100 candidates and asserts `response.results.len() ≥ 40`.
- Reconsider whether `query.limit` should also raise `K_pool` when very large, or whether documenting the K_pool=100 ceiling is sufficient.
- Re-interpret RUN-0019/0020/0021/0022 results in light of this finding (they all ran effectively K=10, not the K-values claimed in summaries).
