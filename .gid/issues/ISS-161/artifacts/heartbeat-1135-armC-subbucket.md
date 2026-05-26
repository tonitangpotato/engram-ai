# ISS-161 Arm C K=30 result + headline (RustClaw2 heartbeat 11:35 EDT)

**TL;DR: K=30 widens the pool but does NOT lift the AC-5a single-fact bucket. Arm C single-fact = 4/13 = 0.308 (FLAT vs Arm B at K=10). The +15.6pp single-hop headline is all from the LIST sub-bucket. The 5 stubborn single-fact blockers (q3, q7, q11, q71, q37) still at 0 across all 3 arms. Trader's pool-recall-miss diagnosis is confirmed.**

## Sub-bucket table (sweep STAMP=20260526T121230Z, all conv-26 K=10/30)

| Sub-bucket | n | A: HYDE pc K=10 | B: HYDE pc_v2 K=10 | **C: HYDE pc_v2 K=30** |
|---|---|---|---|---|
| **single-fact (AC-5a)** | 13 | 3/13 = 0.231 | 4/13 = 0.308 | **4/13 = 0.308** ⚠️ FLAT |
| list (AC-5b territory) | 19 | 3/19 = 0.158 | 5/19 = 0.263 | **7/19 = 0.368** |
| single-hop overall | 32 | 6/32 = 0.188 | 9/32 = 0.281 | **11/32 = 0.344** |

Cross-bucket (Arm C):

| Category | A | B | C |
|---|---|---|---|
| multi-hop | 0.351 | 0.405 | 0.405 |
| open-domain | 0.385 | 0.231 | 0.462 |
| temporal | 0.443 | 0.443 | 0.500 |
| overall | 0.362 | 0.382 | **0.441** |

## 9 ISS-161 target failing single-fact questions

| ID | Gold | A | B | C | Trader's diagnostic predicted |
|---|---|---|---|---|---|
| q3 | Adoption agencies | 0 | 0 | 0 | recoverable (14 needle eps) |
| q7 | Single | 0 | 0 | 0 | recoverable |
| q11 | Sweden | 0 | 0 | 0 | recoverable (ep60) |
| q37 | sunset | 0 | 0 | 0 | recoverable |
| q40 | 2 | 0 | 1 | 1 | unrecoverable (numeric) — flipped (likely artifact, see 0935 file) |
| q43 | abstract art | 0 | 1 | 1 | recoverable |
| q71 | "Becoming Nicole" | 0 | 0 | 0 | recoverable |
| q75 | 3 | 0 | 0 | 0 | unrecoverable (numeric) |
| q76 | 19 October 2023 | 0 | 0 | 0 | unrecoverable (date) |

**Net: 2 of 9 flipped in Arm C (q40, q43) — same as Arm B. Widening K=10 → K=30 added zero single-fact passes.**

## Why this matters

This **confirms Trader's diagnostic** with empirical data. The 5 recoverable-in-principle single-fact questions (q3, q7, q11, q71, q37) did NOT respond to:
1. ✗ Cross-encoder rerank (ISS-159 Arm B): 0pp
2. ✗ HyDE per_category_v2 expansion: 0pp on these 5
3. ✗ K=30 widening: 0pp on these 5

The needle exists in the corpus (Trader verified for q11 "Sweden" → ep60 contains it). It is NOT entering the post-fusion top-K pool even at K=30. This is a **fusion/adapter-weighting problem**, not a rerank problem and not a K-size problem.

## Implications for next move

**ISS-161 Lever 1 (BM25 weight bump) becomes the highest-EV remaining probe.** The pattern of failures is exactly what BM25 should fix:
- q11 "Sweden" — single-needle proper noun, dense embed gets drowned by ambient chat content
- q71 "Becoming Nicole" — proper noun book title
- q7 "Single" — keyword-matchable adjective
- q3 "Adoption agencies" — compound noun phrase

BM25 IDF weighting on Factual adapter should rocket-rank these because they're exactly the rare-token cases where TF-IDF outperforms dense retrieval.

**Note: A 4th experiment (ISS161-L7-D) just launched at 11:34 EDT with `GEN_PROMPT=v2` added — testing whether the issue is generator, not retrieval, on the remaining failures.** Will report results next cycle. This is orthogonal to the BM25 probe and worth running in parallel.

## Headline number caveat

`master.log` shows Arm C `overall_accuracy: None` because the parser failed (extension issue — summary.json exists fine, just not parsed). Actual overall is 0.441 — significant headline win, but driven by list + cross-category, not by AC-5a target.

— RustClaw2 heartbeat 11:35 EDT
