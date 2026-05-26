# ISS-161 — real failure-mode taxonomy (2026-05-26 ~12:40 EDT)

After L2/L7 falsification, hand-checked all 7 stable single-fact misses
against Arm C K=30 retrieved candidates AND raw conv-26 turns.

**Earlier classification was wrong.** I had labelled q3/q7/q71 as
"generation failure" and q11/q75/q76 as "pool-recall miss". The truth
is more mixed and L7 v2 prompt was only fitting noise.

| qid | gold | real failure mode | fix surface |
|-----|------|-------------------|------|
| q3  | Adoption agencies | **extractor lossy** — D2:8 "Researching adoption agencies" became "go do some research" (noun phrase dropped) | L3 extraction (preserve key noun phrases) |
| q7  | Single | gold turn D2:14 "single parent" not in top-30; D3:13 "tough breakup" extracted to vague "significant life change" | L3 extraction + ranking |
| q11 | Sweden | cross-session synthesis (D3:13 "moved from home country" + D4:3 "home country, Sweden") — no single episode contains both | graph linking / multi-episode composition |
| q37 | sunset | image-caption (BLIP) attribution — gold from D8:5 BLIP "sunset" caption on Melanie's message | L3 BLIP extraction |
| q40 | 2 | counting (beach mentioned in D10:8 + D6:16, requires aggregation) | inference, not retrieval |
| q43 | abstract art | aggregation across 3 painting episodes; none individually says "abstract" | L3 with style attribute extraction |
| q71 | "Becoming Nicole" | **generation failure** — fact present at rank 8 in K=30 (Arm C), LLM said "title not mentioned" | generation prompt, but v2 already falsified |
| q75 | 3 | inference from "my son", "the kids/brother" — requires counting children | inference, not retrieval |
| q76 | 19 October 2023 | temporal anchor — "did it yesterday" needs date binding | temporal grounding |

## What this means for the next lever

L3 has a real target: **extractor's lossy summarisation drops key noun
phrases.** q3 is the cleanest example — "Researching adoption agencies"
→ "go do some research". If the extractor is told to preserve concrete
nouns and named entities, q3 should become retrievable.

But the gain ceiling is small. Even if L3 fixes q3 + q7 (best case),
that's +2 single-fact → 10/27 = 0.370. Still below AC-5a 0.629 by
~7 questions.

Q71 is the most frustrating — fact IS in top-K, LLM ignored it. v2
prompt that tried "scan ALL memories" lost more than it gained because
in K=10 the strategy hurts. Maybe a fix-only-for-K=30 prompt would
work, but the design space gets cramped fast.

**Honest read for the AC-5a target:** 0.60 single-fact on conv-26 looks
unreachable without two architectural changes:

1. extractor: stop summarising lossy (q3, q7)
2. multi-episode composition / graph link layer (q11)

Both are real engineering, not benchmark gaming. Whether to pursue
them as ISS-148 prerequisites or accept AC-5a 0.60 is **unrealistic
for this corpus shape** is the open product question.

## Next action options

A. L3 extractor preservation probe — one re-ingest of conv-26 with a
   modified extraction prompt that says "preserve concrete nouns / named
   entities verbatim". Re-run Arm A at K=10 K=30. ~$8 ~45min.
   Expected lift: +1 or +2 single-fact (q3, possibly q7).

B. Skip retrieval levers. Accept that conv-26 has ~5-8 hard inference
   questions (q40, q75, q76, partially q43) that no retrieval/generation
   lever lifts. File AC-5a redefine ticket (Lever 6).

C. Tackle q71-style generation failures with a different prompt
   strategy — not "scan ALL" but "if top candidate is a meta-reference
   (e.g. 'previously recommended a book'), check K+ candidates for the
   referent". Narrow, fact-anchored.

I lean A → if it lifts even +1 (q3), the substrate is meaningfully
better and we have hard evidence; if 0, file Lever 6 with conviction.
B without trying A is a premature surrender.
