# ISS-159 Step 4 — Cross-encoder bench analysis (Trader heartbeat 04:01 EDT)

**TL;DR: Weapon A appears to have hit a retrieval ceiling, not a rerank ceiling. The CE is running and changing predictions, but the seed pool doesn't contain the gold-relevant memories at any K. This means CE targets the wrong pipeline stage.**

## Data

conv-26, both arms, n=152:

| arm | overall | single-hop | multi-hop | temporal |
|---|---|---|---|---|
| A (control) | 0.349 | 0.188 | 0.324 | 0.471 |
| B (CE K=50, post-MMR) | 0.362 | **0.188** | **0.432** (+10.8pp) | 0.443 |

Single-hop sub-bucketed by gold structure (comma/semicolon = list, else single-fact):

| bucket | n | A | B | Δ |
|---|---|---|---|---|
| list | 19 | 0.158 | 0.158 | **+0.000** |
| single-fact | 13 | 0.231 | 0.231 | **+0.000** |

## Confirmation that CE is actually running

- 23/32 single-hop predictions are DIFFERENT in B vs A
- 26/37 multi-hop predictions are different
- 107/152 total predictions different
- retrieve_ms median: A=52.6 → B=96.0 (+43ms, consistent with K=50 × 1.5ms ≈ 75ms minus overlap)

So CE is fetching, scoring, and reordering. Not a wiring bug.

## Why scores don't move (the key finding)

Sample of 5 single-hop cases where prediction changed but score=0 in both arms:

- **q3** gold="Adoption agencies" — A and B both say "I don't know, memories don't specify"
- **q15** gold="pottery, camping, painting, swimming" — A: "reads books and paints", B: "reading a book and painting" (same source, rephrased)
- **q18** gold="beach, mountains, forest" — A: "camping but locations not mentioned", B: "I don't know. mentions camping trips, marshmallows..."
- **q23** gold="\"Nothing is Impossible\", \"Charlotte's Web\"" — A: "doesn't specify which book", B: "specific not... [truncated]"

In all cases the seed pool does not contain the memory holding the gold answer. CE has no winning candidate to promote.

This is **retrieval ceiling**, not reranking failure.

## Where CE DOES help: multi-hop +10.8pp

multi-hop had 26/37 different predictions and overall lifted +10.8pp. So CE rerank IS producing wins — just not in the bucket weapon A was scoped for (single-fact single-hop).

Hypothesis: multi-hop has more candidates in the seed pool that approximate the right chain — CE picks better stitching candidates. Single-hop list/single-fact questions in conv-26 hit topical gaps where no candidate is right.

## Caveat: bench config not captured

The run dir doesn't store env vars / k_seed value. Latency math is consistent with K_seed=50, but I cannot 100% verify Arm B was actually configured with K_seed=50 from disk alone. potato should confirm via shell history or by re-running with explicit ENGRAM_BENCH_K_SEED=50 + a config dump in the run dir.

## Recommendation (for potato to decide)

1. **Verify Arm B actually ran K_seed=50** — if k_seed defaulted to 10, CE rerank only reorders 10 items and the experiment was masked. Check shell history / re-run with logged config.
2. **If K_seed=50 was used**: weapon A has limited single-hop ceiling on conv-26 (the bucket type and corpus shape ISS-160 already flagged as problematic). Multi-hop unexpectedly benefits — consider whether AC-5a is the right gate, or if a new lower target makes sense given the retrieval ceiling.
3. **Either way, before declaring weapon A FAILED**: run conv-44 Arm B. ISS-160 showed single-fact lift on K=30 was STRONGER on conv-44 (+23.5pp vs +16.7pp). CE rerank should follow the same pattern. If conv-44 also flat → weapon A confirmed limited. If conv-44 lifts → conv-26 was the corpus-shape problem from ISS-160.
4. **Consider weapon next**: better embeddings or HyDE expansion at retrieval (lift the seed pool ceiling), since CE rerank can't promote what isn't fetched.

## Files

- A: `engram-bench/benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/`
- B: `engram-bench/benchmarks/runs/ISS159v2-B-conv26-20260526T040634Z/`
- This analysis: `engram/.gid/issues/ISS-159/artifacts/heartbeat-0401-ce-analysis.md`

— Trader, heartbeat 04:01 EDT, 2026-05-26
