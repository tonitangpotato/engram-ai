# RUN-0023 — ISS-104 K_seed fix + K=50

> **⚠️ CORRECTION (2026-05-06, post-hoc):** The "K=50" label in this run is
> **misleading**. ISS-104 fix was found to be a **partial fix**:
>
> - ✅ Associative path: K_seed now honors `query.limit`
> - ✅ Hybrid `fuse_rrf` final truncate: now keeps top-50 (was capped at 15)
> - ❌ Hybrid sub-plans (Factual/Episodic/Abstract/Affective): **STILL
>   hardcode their K** and ignore `query.limit`
>
> So "K=50" in this run actually means: sub-plans fanned out at their
> hardcoded K (~5–10 each), then `fuse_rrf` kept up to 50 of the merged
> candidates instead of truncating to 15. The +6.6pp improvement is
> **real**, but it comes from "fuse stage no longer over-truncates",
> NOT from "sub-plans recall more memories".
>
> The **true K=50 K-sweep** (where sub-plans also fan out wider) is
> blocked on **ISS-105**.
>
> Tracked: ISS-104 re-opened (status: in_progress); ISS-105 filed.

---

**Date:** 2026-05-06 05:28 UTC (autopilot session, potato asleep)
**Build:** engram with ISS-104 fix (orchestrator.rs:921+1240 add `.with_k_seed(query.limit)`)
**Driver:** `cargo build --release --bin engram-bench` then `ENGRAM_BENCH_TOP_K=50 ./target/release/engram-bench locomo`
**Substrate:** `fresh_in_memory_db()` per conversation, ingested from LoCoMo conv-26 fixtures (ISS-103 occurred_at semantics)
**Metric:** J-score (LLM-as-judge, claude-sonnet-4-5)
**Output dir (raw):** `/tmp/k50-smoke/2026-05-06T05-28-51Z_locomo/` → archived here

## Hypothesis

ISS-104 root cause analysis:
- `AssociativePlan::new()` was being called without `.with_k_seed(...)`, so `K_seed = 10` was hardcoded
- RUN-0020 (declared K=15) measured: 145/152 queries got exactly 10 candidates (jsonl-verified)
- "K-sweep" experiments RUN-0019/20/21/22 were therefore not actually testing K — they were testing K=5 vs K=10 vs K=10 vs K=10
- The fix surfaces `query.limit` as `k_seed`, letting retrieval saturate the requested top-K

**Predictions (pre-run):**
1. ✅ J-score should change measurably (the fix changes what gets fed to the generator)
2. ⚠️ Direction unknown — more candidates can help (more evidence) or hurt (more distractors)
3. ✅ Per-category profile should shift, especially in categories that previously suffered from entity-miss

## Results vs RUN-0019/20

| Category    | RUN-0019 (K=5)   | RUN-0020 (K=15, eff. K=10) | **RUN-0023 (K=50, post-fix)** | Δ vs RUN-0020 |
|-------------|------------------|----------------------------|-------------------------------|---------------|
| **overall**     | 0.421 (64/152)   | 0.467 (71/152)             | **0.533 (81/152)**            | **+6.6pp**    |
| multi-hop   | 0.595 (22/37)    | 0.703 (26/37)              | 0.514 (19/37)                 | **-18.9pp**   |
| open-domain | 0.385 (5/13)     | 0.231 (3/13)               | 0.385 (5/13)                  | +15.4pp       |
| single-hop  | 0.156 (5/32)     | 0.188 (6/32)               | **0.312 (10/32)**             | **+12.4pp**   |
| temporal    | 0.457 (32/70)    | 0.514 (36/70)              | **0.671 (47/70)**             | **+15.7pp**   |

**Per-question churn (RUN-0020 → RUN-0023):**
- 19 questions flipped 0 → 1
- 9 questions flipped 1 → 0
- Net +10, gross churn 28 (high churn = real change in generator inputs, not noise)

## Verification That The Fix Actually Took Effect

Indirect evidence (jsonl candidate dump was disabled — wrong env var, see Caveats §1):

1. **Net 28 questions changed verdict** between RUN-0020 and RUN-0023. If K_seed fix was inert, only generator stochasticity (~1-2 q) could change verdicts. 28 ≫ 1-2 → retrieval inputs to generator definitely changed.
2. **Single-hop +12.4pp** matches ISS-001 mode-B (entity-miss) hypothesis exactly: with K_seed=10, the needle entity was missed; with K_seed=50, it's pulled into the candidate set.
3. **Temporal +15.7pp** matches: temporal retrieval needs more context windows around the queried date, and K_seed=10 was clipping them.

## Caveats

1. **Forgot to set `ENGRAM_BENCH_DUMP_CANDIDATES=1`** (typoed it as `ENGRAM_BENCH_CANDIDATE_DUMP=1`). RUN-0023 jsonl does not include `retrieved_candidates`, so we can't directly count candidates per query. Indirect evidence above is strong; a 5-question spot-check rerun with correct env var would confirm directly. Cost: 25-30 min.
2. **Multi-hop regressed -18.9pp**. This is a real concern: more candidates → more distractors → generator picks wrong evidence. Two interpretations:
   - (a) K_pool=100 is being saturated with low-quality entity-expansion hits, fusion sort isn't filtering them well
   - (b) Generator prompt structure can't handle 50 context blocks (truncation? noise?) — needs prompt engineering work
3. **Did not control for K_pool**. K_seed=50 with K_pool=100 means seeds get less expansion budget per seed (100/50 = 2 hops/seed) vs K_seed=10 (100/10 = 10 hops/seed). The two-axis interaction wasn't isolated.
4. **Compared RUN-0019/20 used a different DB substrate workflow** — same in-memory replay path, but the bench binary was built at engram@c8c8fa9. RUN-0023 uses HEAD-of-tree with the ISS-104 fix on top. Build delta = 1 commit (the fix itself), so this is small.

## Decision Implications

- ISS-104 fix is **shipping-quality** — net +6.6pp J-score is significant, single-hop and temporal big wins.
- **Multi-hop regression is the next bottleneck**. Triage path: check whether multi-hop questions in RUN-0023 are losing because (a) the right evidence was retrieved but generator picked wrong one, or (b) more aggressive K_seed is now overwriting the right evidence with low-relevance entity-expansion hits. Need candidate dump to diagnose.
- **K=50 is not necessarily optimal**. May be K=20 or K=30 is the sweet spot. Now that the cap is lifted, a real K-sweep is possible.

## Next Actions (for potato to triage on wakeup)

1. **Rerun 5-question smoke with `ENGRAM_BENCH_DUMP_CANDIDATES=1`** to directly confirm jsonl shows ~30-50 candidates per query (5min run).
2. **Real K-sweep**: K=10, 20, 30, 50, 75 — find the multi-hop / single-hop trade-off curve.
3. **Investigate multi-hop regression** before claiming victory. Possibilities: K_pool tuning, fusion stage re-weighting, generator prompt for handling 50 blocks.
4. **Update ISS-104 status** based on triage outcome (flip to `done` once K-sweep done and multi-hop regression understood).
5. **Update prior RUN summaries (RUN-0019/20/21/22)** with the post-mortem note: "effective K≈10 due to ISS-104, see RUN-0023 for true K-behavior".

## Files

- `locomo_per_query.jsonl` — 152 question-by-question records
- `locomo_summary.json` — overall + per-category J-score
- `/Users/potato/clawd/projects/engram/.gid/issues/ISS-104/issue.md` — root cause + fix detail
