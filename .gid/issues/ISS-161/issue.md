---
title: AC-5a next lever — single-fact sub-bucket ≥0.60 on conv-26 (post-weapon-A)
status: open
priority: P1
severity: planning
category: retrieval
created: 2026-05-26
relates:
- ISS-148
- ISS-149
- ISS-150
- ISS-155
- ISS-159
- ISS-160
---

## Summary

ISS-148 AC-5a target: conv-26 **single-fact sub-bucket** (12 of 32
single-hop questions) ≥ 0.60. Current best measured (ISS-148 K=30
anchor): **5/12 = 0.417**. Gap to gate: need to convert **at least 3
more single-fact questions to pass**.

Two top retrieval levers have now been empirically falsified on this
sub-bucket:

| Lever | Result | Verdict |
|---|---|---|
| ISS-149 NullEntityLookup → real EntityLookup (forced-Factual probe) | -1 pass, 0 new passes (-3.13pp single-hop) | NOT the lever |
| ISS-159 weapon A cross-encoder ms-marco-MiniLM-L-6 k_in=50 (v2 sweep) | 3/12 single-fact → 3/12, **delta = 0** | NOT the lever |

This issue is a **planning ticket**: enumerate remaining candidate
levers for AC-5a, rank them by expected lift × cost, and pick the next
one to probe.

## Evidence carry-over

From ISS-159 falsification sub-bucket analysis (commit b48ba46 +
this issue's hand-classification, 2026-05-26, paper-only):

Conv-26 K=10 HyDE=per_category temp=0:

| Sub-bucket (single-hop) | A no-CE | B CE k_in=50 | K=30 anchor (ISS-148) |
|---|---|---|---|
| **single-fact (n=12, AC-5a)** | 3/12 = 0.250 | 3/12 = 0.250 | 5/12 = 0.417 |
| list (n=20, AC-5b territory) | 3/20 = 0.150 | 3/20 = 0.150 | 4/20 = 0.200 |

The single-fact bucket has 12 questions; 5 pass at the K=30 anchor.
The 7 currently-failing single-fact questions (by id, from v2 Arm A
gold-strings):

- q3  "Adoption agencies"
- q11 "Sweden"
- q37 "sunset"
- q40 "2"
- q43 "abstract art"
- q7  "Single"
- q71 "\"Becoming Nicole\""
- q75 "3"
- q76 "19 October 2023"

(IDs from `benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_per_query.jsonl`.)

Each is a discrete-fact lookup against a 675-episode dense chat
corpus. Diagnostic per-question (which arm of the pipeline drops the
right episode out of top-K) has not yet been done.

## Candidate levers (proposed, not selected)

### Lever 1 — BM25 weight bump on Factual / Associative adapters

Status: untested. ISS-150 wired BM25 into Associative, ISS-147 wired
it into Factual; weights remain at defaults. Bump BM25's relative
weight in fusion when query carries strong noun-phrase signal.

Hypothesis: discrete-fact queries ("Sweden", "abstract art", "Becoming
Nicole") have strong literal-string overlap with the gold episode but
get drowned out by Hebbian / dense-embedding signal on adjacent
content. Increasing BM25 weight should surface the literal-match
episode into top-K.

Cost: 1-2 LoC config, ~1 sweep (~12min, ~$1).
Risk: regresses paraphrase-shaped queries (multi-hop, open).
Probe shape: A baseline / B bm25_weight × 1.5 / C bm25_weight × 2.

### Lever 2 — Per-question diagnostic: where is the right episode?

Status: not yet done. For each of the 7 failing single-fact questions,
inspect the top-K candidates and check (a) is the gold episode in the
candidate pool, (b) at what rank, (c) which adapter ranked it where.
Cheap (paper-only on the existing per-query.jsonl + the candidate
dump).

Outcome routes:
- Gold episode **never in top-50 candidate pool**: pool-recall issue
  → look at indexing (extraction, embedding quality, FTS analyzer).
  This is the ISS-155-class fix (extraction-time fact density).
- Gold episode **in pool at rank 11-50**: reranker / fusion-weight
  issue. ISS-159 cross-encoder didn't move it → either wrong reranker
  features (cross-encoder uses raw episode text; maybe entity-aware
  reranker needed) or wrong scoring target (cross-encoder optimised
  for paraphrase, not literal lookup).
- Gold episode **in top-10 but generation answers wrong**: not a
  retrieval problem — punt to AC-5b (generation prompt).

This diagnostic MUST run before another sweep. ~30min paper-only.

### Lever 3 — Extraction-time fact density (ISS-155-class)

Status: ISS-155 fixed extractor temp=0 determinism; broader question
of whether the extractor is producing high-density single-fact
episodes for needles like "Sweden", "19 October 2023" remains open.

If Lever 2 diagnostic shows the gold episode is **missing from the
candidate pool**, this is the lever. Inspect extracted episodes for
the 7 failing questions: is the literal fact in any episode? If yes
but not surfaced — embedder/FTS issue. If no — extraction misses it.

Cost: medium (extraction is ~$5 per re-ingest of conv-26).

### Lever 4 — Query expansion targeted at single-fact recall

Status: ISS-153 HyDE re-tested post-ISS-155-fix, found multi-hop
regression; per_category routing (ISS-156) shipped. Could narrow
further: literal-keyword expansion for single-fact intent (e.g.,
classifier predicts Factual → generate alternate phrasings of the
noun-phrase target, not full hypothetical answers).

Cost: design + impl + sweep; ~4h + $5.
Risk: same multi-hop regression we saw with HyDE-on-everything.

### Lever 5 — Different reranker family (entity-aware, not cross-encoder)

Status: cross-encoder falsified for single-fact. Entity-aware reranker
(re-score candidates by how well they cover the query's named
entities) is a different family. Closer to the ISS-149 entity-aware
classifier path but applied at rerank stage instead of plan-dispatch.

Cost: similar to weapon A (~1 week impl).
Risk: high — same family-mismatch failure could recur if root cause
is pool-recall not pool-ordering.

### Lever 6 — Punt AC-5a, redefine ISS-148

If diagnostic (Lever 2) shows 5+ of the 7 failing single-fact
questions are unrecoverable (gold episode never extracted, or factual
content is in multi-episode context the generator can't compose), the
honest answer is AC-5a ≥0.60 is unreachable on conv-26 with current
extraction+retrieval architecture. Reframe ISS-148 to ≥0.42 (no
regression from current best) and ship.

This is the "honesty option" if Levers 1-5 are all dead.

## Decision rule

1. **Run Lever 2 first** (paper-only diagnostic, no LLM calls). This
   is non-negotiable — every sweep done without this diagnostic so
   far has been blind, and the result has been falsification.
2. Based on Lever 2 outcome, pick ONE of Levers 1, 3, 4, 5 to probe.
3. **Do not run any new bench sweep until Lever 2 is complete.**
4. If two consecutive levers falsify, escalate to Lever 6.

## Acceptance Criteria

- [ ] **AC-1:** Per-question diagnostic for the 7 failing single-fact
       questions: gold episode in pool? at what rank? from which
       adapter? Output: artifact `iss161-diagnostic.md` with one row
       per question.
- [ ] **AC-2:** Based on AC-1, select ONE candidate lever (1, 3, 4,
       or 5) and write a 1-paragraph implementation sketch.
- [ ] **AC-3:** Implement + probe selected lever (separate ISS or
       continuation of this one).
- [ ] **AC-4:** Single-fact sub-bucket B-A delta ≥ +3 questions
       (+25pp), or escalate to Lever 6.

## Out of scope

- AC-5b (list sub-bucket, generation/judge fixes): separate work
  stream tracked by ISS-160.
- General multi-hop / temporal / open improvements: this ticket is
  AC-5a-only.
- Variance bracketing of ISS-159 falsification: ISS-159 is closed-out
  on its own evidence; no re-litigation here.

## References

- ISS-148 — root AC-5a definition (single-fact sub-bucket ≥0.60)
- ISS-149 — NullEntityLookup, falsified as AC-5a lever 2026-05-25
- ISS-159 — weapon A cross-encoder, falsified as AC-5a lever 2026-05-26
- ISS-150 — BM25 wired into Associative adapter
- ISS-147 — BM25 wired into Factual adapter
- ISS-155 — extractor temp=0 determinism (related extraction-side fix)
- ISS-160 — list-question generation/judge (AC-5b sibling)
- `benchmarks/runs/ISS159v2-A-conv26-20260526T040634Z/locomo_per_query.jsonl` — source for failing-question list
