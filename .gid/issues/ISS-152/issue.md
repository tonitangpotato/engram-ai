---
title: Retrieval pool sizing + MMR λ sweep for list-question coverage (cheap first try)
priority: P1
severity: degradation
status: resolved
tags:
- retrieval
- pool-sizing
- mmr
- locomo
- conv-26
- cheap-win
relates_to:
- ISS-148
- ISS-150
- ISS-151
- ISS-153
- ISS-154
blocks: ''
fixed_by: 10f2295
resolution: negative-result
---

# ISS-152 — Pool sizing + MMR λ sweep

## TL;DR

Cheapest possible attack on the ISS-151 root cause. Two knobs, no
new code paths:

1. **K_seed pool** — currently `(K * 4).max(40)` = **40** for K=10.
   Widen to test whether the 14 recall-miss queries find their gold
   when the post-fusion truncate sees a deeper pool.
2. **MMR λ** — currently 0.7. Drop toward 0.5 to force more
   diversity, attacking the 9 partial-list cases where top-K is
   dominated by 1 item from the gold list.

If pool widening alone moves the needle, we get a free win without
implementing HyDE (ISS-153) or query expansion (ISS-154).

## Plan

### Sweep grid

| run | K_seed pool | MMR λ | hypothesis under test |
|---|---|---|---|
| A | 40 (current) | 0.7 (current) | baseline (= ISS-150 result) |
| B | 100 | 0.7 | "pool too narrow" → recall miss bucket shrinks |
| C | 200 | 0.7 | upper-bound pool effect |
| D | 100 | 0.5 | pool + diversity combined |
| E | 100 | 0.3 | aggressive diversity (list-question hot fix) |

Each run = full 152q conv-26, K=10, temp-NA (use ISS-137 default).
~12 min per run on the current box → ~1 hour wall-clock total.

### Implementation

`K_seed` is computed inside `execute_plan` as `(query.limit * 4).max(40)`
at orchestrator.rs:984. Make this an env-var override:

```rust
let bm25_pool = std::env::var("ENGRAM_BENCH_BM25_POOL")
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .unwrap_or_else(|| (query.limit * 4).max(40));
```

Same for `run_associative_fallback`. MMR λ already has env-var
override via `ENGRAM_BENCH_MMR_LAMBDA`.

### Decision tree

After all 5 runs:

- If **B or C** lifts single-hop ≥ 0.35 → **commit the wider pool**
  as new default, file follow-up for ISS-154 (list multi-query) but
  deprioritise ISS-153 (HyDE).
- If **D or E** lifts list-question coverage (per-q21/q15/q38/q52)
  but A-C don't → **commit lower λ** + still file ISS-154.
- If **none** materially move → fall through to ISS-153 (HyDE) as
  the next lever. This is the most-informative negative result:
  it proves embedding-similarity is the bottleneck regardless of
  pool size, which is exactly what HyDE attacks.

## Acceptance criteria

- [ ] Env-var override `ENGRAM_BENCH_BM25_POOL` shipped (both
      `execute_plan` and `run_associative_fallback` sites).
- [ ] 5 conv-26 runs completed, results in
      `.gid/issues/ISS-152/artifacts/sweep_results.md`.
- [ ] Per-bucket recovery measured via `ISS-151/artifacts/recall_diag.py`
      on each run (re-run the 14/9/2 split, see which bucket shrinks).
- [ ] Decision committed to ISS-148 update: which knob (if any) stays
      on as new default.

## Open questions

- **K_seed pool >> top-K**: does the MMR diversity hook degrade
  meaningfully when it's choosing from 200 candidates instead of 40?
  (MMR is O(N²) on candidate count for the in-Rust impl — 200 is
  still fine, 1000+ wouldn't be.)
- **λ very low** (≤ 0.3): risks throwing away the actually-relevant
  top candidate for marginal diversity. Worth measuring but I'd be
  surprised if it wins outright.

## Non-goals

- Does NOT implement HyDE (ISS-153).
- Does NOT implement list-question sub-query expansion (ISS-154).
- Does NOT touch the upstream classifier/resolver blindness
  (ISS-149 / ISS-145).

## Resolution (2026-05-24 07:22Z)

Sweep complete (commit `10f2295`). **Negative result — hypothesis falsified.**

Pool widening is *monotonically harmful* across A→B→C:
- overall 0.36 → 0.29 → 0.18
- single-hop 0.16 → 0.13 → **0.03**

Lower MMR λ (B→D→E) is also monotonically harmful. Open-domain takes
the worst hit (0.38 → 0.08–0.15).

The 14 "recall-miss" single-hop failures identified in ISS-151 are NOT
caused by pool size. Widening the candidate pool just lets short
conversational reactions ("Wow!", "Cool!") with high embedding cosine
similarity dilute the top-K. MMR diversity pressure makes it worse by
pushing noise candidates *into* top-K.

The override fields `k_seed_override` and `bm25_pool_override` remain
in code (engram `894dcb1` + engram-bench `df3c8d1`) as diagnostic
levers. **Defaults are unchanged**: `K_seed = query.limit`, `bm25_pool = (limit*4).max(40)`, `MMR λ = 0.7`.

Next move: **ISS-153 (HyDE)** bumped to P1 in_progress. The bottleneck
is upstream — query→passage embedding semantics. Rewriting the query
into a hypothetical answer (HyDE) attacks that directly, where pool
sizing cannot.

See: `.gid/issues/ISS-152/artifacts/sweep_results.md` for full table
+ per-query diff analysis + side-issue (Ollama embed non-determinism).
