---
title: Generation answers 'I don't know' even when dated gold is at rank 0 — date-asking prompt hardening
status: todo
priority: 2
labels: generation, synthesis, prompt, conv-26-q0, retrieval-ok
relates_to: ISS-211, ISS-210, ISS-205, ISS-204
---

# ISS-212: generation drops the dated gold line even at rank 0 (distractor saturation)

## Summary

ISS-211 (reserved-first re-rank, v2 = relevance tiebreak) drove the gold
dated episode for conv-26-q0 to **rank 0** of the fused top-10 context.
Verified by the delivery probe on the v2 confirmation run. Yet the
generator **still** answers "I don't know." The remaining defect is
entirely in the generation/synthesis layer: the model ignores an explicit
dated line that directly answers the question, distracted by the other
nine same-subject episodes in the window.

This is the residual that ISS-211's *title* predicted but its *fix*
(ranking) could not address — ranking is now provably maxed out (rank 0),
so the only remaining lever is the prompt / synthesis instruction.

## Evidence (v2 confirmation arm — STAMP 20260602T210128Z, binary 0c8886bc, DB .tmpjTs3bm)

Delivery probe (`iss207_q0_delivery_probe`, GOLD_PREFIX=641e2014):

```
plan_used: Factual
  [ 0] score=0.7942  641e2014  [2023-05-07] Caroline attended a LGBTQ support group  <== GOLD
  [ 1] score=0.7765  cc519a6c  [2023-06-09] Caroline gave a talk ...
  [ 2] score=0.6604  3ac33027  [2023-06-23] Caroline attended an LGBTQ+ counseling workshop ...
  [ 3] score=0.6529  7691b5a9  [2023-06-17] Caroline and her transgender teen mentee attended ...
  [ 4] score=0.6968  7bb57219  [2023-05-25] Caroline is researching ... adoption ...
  ... (ranks 5-9: more Caroline advocacy / support episodes)
GOLD in top-10: YES (rank 0). Carries resolved date 2023-05-07: YES.
```

Bench verdict for conv-26-q0:

```
gold:      7 May 2023
predicted: I don't know.
           "The memories mention Caroline's involvement with the LGBTQ
            community ... but they don't specify when she went to an LGBTQ
            support group."
score:     0.0
```

The dated answer is the **first line** of context and the model still
claims the date is unspecified. This is a synthesis/prompt failure, not a
retrieval failure.

## Root cause hypotheses

1. **Subject-match blindness.** The window has 5+ "Caroline attended/went
   to an LGBTQ {support group, counseling workshop, pride parade, talk}"
   lines. The model treats them as near-duplicates and refuses to pick
   one, rather than matching the exact subject phrase ("support group")
   to its dated line.
2. **Date-line under-weighting.** The synthesis prompt does not instruct
   the model to prefer/scan explicit `[YYYY-MM-DD]` lines when the question
   is date-asking ("when did …").
3. **Over-conservative IDK bias.** The prompt likely encourages "I don't
   know" when uncertain; with several similar episodes the model defaults
   to IDK instead of answering from the best subject match.

## Proposed levers (in order of cheapness)

1. **Date-asking prompt clause** (cheapest): when the query asks "when",
   instruct the model to scan the context for `[YYYY-MM-DD]` lines whose
   subject phrase matches the question, and answer with that date; only
   say "I don't know" if no dated line matches the subject. The
   `query_classifier::asks_for_date` flag is already computed and could be
   threaded to the synthesis prompt builder.
2. **Rank-0 anchoring hint**: tell the model the first context line is the
   most relevant retrieved memory for the question.
3. **Subject-disambiguation instruction**: when multiple same-actor
   episodes appear, match on the full predicate phrase ("support group" ≠
   "counseling workshop" ≠ "pride parade").

Start with lever 1 — it is the direct counter to the observed failure and
reuses the existing date-asking classifier signal. Bench the conv-26-q0
flip plus the temporal/single-hop aggregate as a regression gate.

## Acceptance criteria

- [ ] AC-1: conv-26-q0 flips 0→1 (model answers ~2023-05-07 from the
      rank-0 dated line).
- [ ] AC-2: no regression on conv-26 aggregate (overall ≥ 0.3092 within
      ingest-noise band; temporal category does not drop).
- [ ] AC-3: the prompt change is gated to date-asking queries (reuses
      `asks_for_date`), leaving non-temporal synthesis byte-identical.

## Why this is separate from ISS-211

ISS-211 was a **retrieval/ranking** fix (deliver the dated episode to the
head of the window). It is done and proven (rank 0). ISS-212 is a
**generation/synthesis** fix (make the model *use* the delivered line).
Different layer, different fix, independently testable.
