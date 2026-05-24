---
title: conv-26 single-hop retrieval recall miss — 14/25 fails have gold keywords absent from top-10 (root-cause analysis)
priority: P1
severity: degradation
status: open
tags:
  - retrieval
  - recall
  - locomo
  - conv-26
  - root-cause
relates_to:
  - ISS-148
  - ISS-150
  - ISS-152
  - ISS-153
  - ISS-154
---

# ISS-151 — conv-26 single-hop root-cause analysis

## TL;DR

ISS-148 set a target of single-hop ≥ 0.40 on conv-26. ISS-147 (BM25
into Factual/Episodic/Affective) and ISS-150 (BM25 into Associative)
both landed cleanly and produced byte-identical 0.2188 single-hop
accuracy. This issue documents WHY: of the 25 single-hop failures,
**14 (56%) are retrieval recall misses** (gold keywords entirely
absent from top-10), **9 (36%) are partial-list recall** (gold is a
multi-item list, only 1-2 items in top-10), and **2 (8%) are
reading-comprehension fails** (evidence present, generator answered
wrong). BM25 reranking can't fix any of these because it only
reorders an already-too-narrow pool.

This issue is the **diagnostic artifact**. The fix issues are:
- ISS-152 — cheap pool-sizing + MMR λ sweep (do first)
- ISS-153 — HyDE / hypothetical query expansion (if ISS-152 insufficient)
- ISS-154 — list-question multi-sub-query expansion (independent track)

## Diagnostic procedure

Mode-B dump enabled via `ENGRAM_BENCH_DUMP_CANDIDATES=1` env var.
Run record:

```
benchmarks/runs/ISS150-modeB-dump-conv26-20260524T042707Z/
  └── 2026-05-24T04-39-29Z_locomo/locomo_per_query.jsonl
```

`retrieved_candidates` field contains the 10 candidates that fed
`generate_answer`. Procedure: extract gold keywords (4+ char
alphanumeric tokens), join all top-10 candidate `text` fields,
substring-match keywords. Any hit = "evidence in pool". Zero hits
= recall miss.

Script archived at `.gid/issues/ISS-151/artifacts/recall_diag.py`
(reproduces the 14/9/2 split exactly).

## Failure breakdown (25 single-hop fails)

### A. Retrieval recall miss — 14 / 25 (56%)

Gold keywords entirely absent from top-10. Evidence DOES exist in
the source conversation. Example walked through below.

| qid | gold | gold keywords missing from top-10 |
|---|---|---|
| q3 | Adoption agencies | adoption, agencies |
| q7 | Single | single |
| q11 | Sweden | sweden |
| q18 | beach, mountains, forest | beach, mountains, forest |
| q19 | dinosaurs, nature | dinosaurs, nature |
| q23 | "Nothing is Impossible", "Charlotte's Web" | nothing, impossible, charlotte |
| q34 | Mentoring program, school speech | mentoring, program, school, speech |
| q43 | abstract art | abstract |
| q48 | bowls, cup | bowls |
| q55 | Sunsets | sunsets |
| q56 | Rainbow flag, transgender symbol | rainbow, flag, transgender, symbol |
| q66 | Roast marshmallows, tell stories | roast, marshmallows, tell, stories |
| q71 | "Becoming Nicole" | becoming, nicole |
| q76 | 19 October 2023 | october |

### B. Partial-list recall — 9 / 25 (36%)

Gold is a multi-item list, top-10 contains some items but not all.
LLM judge is all-or-nothing on lists → partial = 0 score.

| qid | gold | items hit | items needed |
|---|---|---|---|
| q15 | pottery, camping, painting, swimming | 1 | 4 |
| q24 | Running, pottery | 1 | 2 |
| q38 | Pottery, painting, camping, museum, swimming, hiking | 1 | 6 |
| q40 | (counting question) | partial | — |
| q51 | (various) | partial | — |
| q52 | Oliver, Luna, Bailey | 1 | 3 |
| q60 | clarinet and violin | 1 | 2 |
| q61 | Summer Sounds, Matt Patterson | partial | 2 |
| q78 | (items list) | partial | — |

### C. Reading-comprehension fail — 2 / 25 (8%)

Gold-related evidence IS in top-10, but generator answered wrong.
These are LLM-side failures, not retrieval problems.

## Walk-through: conv-26-q3 "What did Caroline research?"

**Gold**: `Adoption agencies` (LoCoMo evidence pointer: `D2:8`)

**Source episode #25** (Day 2, episode 8, 2023-05-25):
> "Caroline: Researching adoption agencies — it's been a dream to
> have a family and give a loving home to kids who need it."

**Top-10 candidates retrieved**:

```
[1] s=0.8845 "[2023-05-08] Caroline: Totally agree, Mel. ... Well, I'm
              off to go do some research."
[2] s=0.8581 "[2023-07-20] Caroline: Cool! What did it look like?"
[3] s=0.8272 "[2023-05-08] Melanie: Wow, that's cool, Caroline! What
              happened that was so awesome? ..."
[4] s=0.8293 "[2023-07-12] Melanie: That sounds awesome! What did you
              take away from it ..."
[5] s=0.8203 "[2023-08-28] Caroline: Wow! Did you see that band?"
[6] s=0.8476 "[2023-07-15] Melanie: Wow, what an experience! How did it
              make you feel?"
[7] s=0.8158 "[2023-08-25] Melanie: Wow, that's gorgeous! Where did you
              find it?"
[8] s=0.8295 "[2023-10-13] Melanie: Thanks for the tip, Caroline.
              Doing research and readying myself emotionally ..."
[9] s=0.8114 "[2023-07-15] Melanie: Wow, looks awesome! Did you join in?"
[10] s=0.8207 "[2023-08-25] Melanie: Wow, did you make that? It looks
               so real!"
```

**Pathology**: 9 of 10 candidates are **conversational reactions**
("Wow!", "Cool!", "What did it look like?"). The actual evidence
(#25, "Researching adoption agencies") was ranked **outside top-10**.
The #1 candidate (#16, "I'm off to go do some research") is a
semantically close but factually empty preamble from 17 days
earlier.

**Why embedding ranks reactions high**: a generic short utterance
in the same conversation has high cosine similarity to the query
embedding because (a) it shares speaker + temporal context, (b)
reactions are short, so their normalised embedding is dominated by
the few "important" tokens which often align with query intent
("research", "experience", "awesome"). The specific-fact episode
gets diluted by its richer content vocabulary.

## What ISS-150 results actually prove

| Metric | ISS-147 baseline | ISS-150 | Δ |
|---|---|---|---|
| single-hop accuracy | 0.2188 | 0.2188 | 0.00 |
| predicted answers identical | — | 108/152 | — |
| predicted answers different | — | 44/152 (29%) | — |
| judge score flips | — | 2/152 | — |

BM25 reordering IS reaching the generator (29% of predictions
changed), but the **pool itself is missing the gold evidence**, so
reranking can't help.

## Verified causal chain

1. K=10 top-K cap is binding for conv-26 single-hop.
2. Embedding model ranks conversational reactions ≥ specific-fact
   episodes when the query is short and the conversation is dense.
3. Reranking (vector / BM25 / graph) operates on the already-pooled
   top-K and CANNOT recover episodes that were never in the pool.
4. → Fix must (a) widen the pool before reranking [ISS-152],
   (b) generate richer queries that surface the right episodes
   [ISS-153], or (c) expand list-questions into sub-queries [ISS-154].

## Non-goals

This issue is diagnostic. No code change here. The fix is in 152/153/154.

## Acceptance criteria

- [x] Mode-B dump captured and archived
- [x] 25 fails categorised by failure mode (14 / 9 / 2)
- [x] Worked example documented (q3)
- [x] Fix-track issues filed (ISS-152, 153, 154)
- [ ] After ISS-152 lands: re-run Mode-B on conv-26, recompute the
      14/9/2 split, verify the pool-sizing fix moved the recall-miss
      bucket toward zero (or report the residual).
