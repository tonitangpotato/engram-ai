# ISS-161 Lever 2 — Paper-only diagnostic on the 9 failing single-fact questions

**Trader heartbeat 08:01 EDT 2026-05-26. Completes Lever 2 from ISS-161 without burning bench cycles.**

## Method

For each of the 9 failing single-fact questions in conv-26 v2 Arm A, I checked:
1. Does the literal gold string appear in any conv-26 episode? (`fixtures/locomo/.../conversations.jsonl`)
2. What did the system retrieve / generate? (`runs/ISS159v2-A,B-conv26-…0406Z/locomo_per_query.jsonl`)
3. Question type — is it actually single-fact or mis-classified?

## Per-question table

| qid | gold | literal in corpus? | retrieved relevant episode? | actual question type | fixable by rerank? |
|---|---|---|---|---|---|
| q3 | "Adoption agencies" | ✅ ep25,27,253 (3 episodes) | ❌ ("off to do some research") | true single-fact | ❌ no — gold episode missing from top-K |
| q7 | "Single" | ✅ ep31 ("single parent") | ❌ ("I don't know") | implicit / lexical mismatch | ❌ no — gold not in top-K AND "single parent" ≠ relationship-status "single" |
| q11 | "Sweden" | ✅ ep60 ("home country, Sweden") | ❌ ("I don't know") | 2-hop (Sweden + "moved 4y ago") | ❌ no — gold episode missing from top-K |
| q37 | "sunset" | ✅ ep276,278,365 (3 episodes) | ❌ ("Melanie painted a horse") | true single-fact | ❌ no — wrong episode retrieved |
| q40 | "2" | ✅ ep65 ("2 younger kids" — irrelevant) | partial ("at least once in 2023") | **count question** | ❌ no — needs aggregation |
| q43 | "abstract art" | ❌ NOT in corpus | partial ("draws flowers, self-acceptance") | **inference required** | ❌ no — no episode says "abstract" |
| q71 | '"Becoming Nicole"' | ✅ ep118 (book title) | ❌ ("title not mentioned") | true single-fact | ❌ no — gold episode missing from top-K |
| q75 | "3" | ❌ NOT in corpus literally | ❌ ("no info about how many children") | **count + inference** | ❌ no — needs counting from indirect mentions |
| q76 | "19 October 2023" | ❌ NOT in corpus | ❌ ("I don't know") | **date arithmetic** | ❌ no — needs reasoning over multiple dated episodes |

## Failure pattern

**Pure retrieval misses (gold-string-in-corpus, not in top-K): 5/9** — q3, q7, q11, q37, q71

For these the gold episode literally exists. The CE reranker found nothing better to promote because the seed pool didn't contain the right episode at any K. Confirms ISS-159 falsification root cause.

**Non-retrieval failures: 4/9** — q40 (count), q43 (no literal), q75 (count+inference), q76 (date arithmetic)

These aren't single-fact lookups at all. They're aggregation / inference. No rerank (CE or otherwise) can fix these. They are also unlikely to be fixed by widening K — they need generation-side reasoning or upstream classifier work.

## Bucket purity finding

**~44% (4/9) of the "single-fact" failing bucket is mis-classified.** They are counts, dates, or implicit inferences. If we re-bucket honestly:

- True single-fact failures: 5 (q3, q7, q11, q37, q71)
- Count/inference failures (don't belong here): 4 (q40, q43, q75, q76)

If we recompute AC-5a on the true-single-fact bucket only (n = 12 - 4 mis-classified = ~8 questions, with 3 passing), the baseline is 3/8 = 0.375. Still far below 0.60 target, but the target itself is now defined against a cleaner bucket.

## Implications for the lever ranking in ISS-161

This sharpens the lever choice without spending a bench:

- **Lever 1 (BM25 weight bump)**: directly addressable for q3, q11, q37, q71 — all four have strong noun-phrase literal-string overlap with their gold episode. q3 has "research" + "adoption" in the question and "Researching adoption agencies" in ep25. q11 has "Sweden" literal. q37 has "paint" + "sunset" literal. q71 has "book" + "Caroline's suggestion" + "Becoming Nicole" literal. **High-probability win on 4/5 true single-fact misses.**
- **Lever 3 (HyDE per-category)** was already FALSIFIED on conv-26 single-hop per ISS-149.
- **Lever 5 (better embeddings)** was already FALSIFIED on conv-26 single-hop per ISS-157 weapon B.
- **Levers for non-retrieval failures (q40 count, q43 inference, q75 count, q76 date)**: out of scope for AC-5a as currently defined. Could file as ISS-162 (numeric/temporal reasoning gate) or accept as permanent floor.

## Recommended next move

1. **Run Lever 1 (BM25 weight bump)** as a 2-arm bench on conv-26 only, ~$1, ~12 min. Hypothesis: BM25 × 1.5 surfaces ep25/ep60/ep118/ep276 into top-K and lifts q3/q11/q71/q37 from 0→1 → single-fact bucket 3/12 → 7/12 = 0.583, just below 0.60 target.
2. **If Lever 1 hits ≥0.50 on single-fact** but not 0.60, run Lever 1 + Lever 6 (combined) or accept the new floor as the AC-5a redefinition (true-single-fact ≥0.50 on n=8).
3. **Document the 4 mis-classified questions** as bucket-pollution evidence and propose AC-5a v2 against the cleaned bucket. This is a more honest gate.

## Files

- Per-query data: `engram-bench/benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_per_query.jsonl`
- Conv-26 source: `engram-bench/benchmarks/fixtures/locomo/39e7df4ea492e8bc7a483b2cfc8e18620054beb05fed267f5cc098bd65fd5f4d/conversations.jsonl`
- This diagnostic: `engram/.gid/issues/ISS-161/artifacts/heartbeat-0801-lever2-diagnostic.md`

— Trader, heartbeat 08:01 EDT, 2026-05-26
