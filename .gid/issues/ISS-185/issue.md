---
title: 'B-bucket activation — Metacognition: read/write paths exist, no e2e loop where signal triggers behavior change'
status: open
priority: P3
severity: feature-inert
category: cognitive-substrate
created: 2026-05-28
relates: [engram:ISS-181]
relates_to: .gid/issues/ISS-181/issue.md
discovered_in: ISS-181 cognitive feature coverage matrix
---

## Summary

The metacognition substrate ships:
- Write path: confidence / uncertainty / source-reliability fields
  populated on memories and on retrieval results.
- Read path: callers can inspect those fields after retrieval.
- Storage: persistence works, round-trips clean.

But there is **no end-to-end loop where a metacognitive signal
triggers a behavior change**. The signal is computed, stored,
and exposed, but nothing in the retrieval / generation pipeline
acts on it. Concretely: a low-confidence answer does not trigger
query reformulation, candidate re-fetch, plan re-routing, or
a "I don't know" abstention. A high-uncertainty memory does not
get deprioritized in fusion. A low-source-reliability cluster
does not get flagged for the user.

Coverage matrix bucket B: read + write paths real, behavior loop
inert.

## What it would take to make metacognition production-active

Pick one concrete loop and ship it end-to-end. Suggested first
loop (smallest scope, biggest signal):

### Concrete loop A — Low-confidence answer triggers query reformulation

1. After the generation step, parse the answer for confidence
   markers (LLM's own hedging language: "I think", "possibly",
   "I'm not sure" — or have the generator emit a structured
   confidence field).
2. If confidence below threshold AND the original query has
   reformulation potential (length > N tokens, contains
   anaphora, contains ambiguous referents), call
   `query_reformulator` to produce 1–3 expanded queries.
3. Re-run retrieval on the reformulated queries, merge candidates
   into the original candidate pool, re-rank, re-generate.
4. If second-pass confidence is still below threshold, emit
   "I don't know" rather than a hallucinated answer.
5. Flag-gated, default off. A/B vs current pipeline on conv-26.

### Concrete loop B — Source-reliability gates fusion weight

1. Compute per-memory source reliability from extraction
   metadata (LLM-extracted facts vs verbatim quotes vs
   structured field copies).
2. In `combine()` / `combine_factual_v2()`, multiply final
   score by a reliability factor (default 1.0 = no effect).
3. Low-reliability memories drop in rank without being
   dropped from the pool — preserves recall while improving
   precision.
4. Flag-gated, default off. A/B sweep.

### Concrete loop C — Uncertainty triggers candidate expansion

1. Compute query-level uncertainty (entity ambiguity, temporal
   ambiguity, plan classifier confidence).
2. High-uncertainty queries get K_seed bumped (e.g. 10 → 30)
   and fusion pool widened (mirrors the ISS-152 sweep but
   conditional rather than global).
3. Avoids the latency cost of always running wide pools.
4. Flag-gated, A/B.

## Acceptance criteria for activation

- [ ] AC-1 — One concrete metacog→behavior loop chosen and
  implemented end-to-end (not partial), with a flag gate.
- [ ] AC-2 — Bench harness shows the loop firing on a
  measurable fraction of queries (e.g. ≥10% trigger rate
  on conv-26).
- [ ] AC-3 — A/B sweep shows ≥+2pp lift on the targeted
  query subset (e.g. low-confidence-trigger queries for
  loop A, low-reliability-source queries for loop B), with
  no regression > 1pp on the un-triggered subset.

## Why P3 now

Three reasons this isn't worth promoting:

1. **No corpus signal yet that the loop matters**: every
   substrate-side improvement we've tried (entity channel,
   factual reweight, prev-turn context, HyDE, MMR, cross-
   encoder) has been falsified or moved <2pp on conv-26.
   The bottleneck is generation/extraction quality, not
   retrieval — and metacognition layered on top of broken
   retrieval just gives us more confident wrong answers.

2. **Concrete loop choice is corpus-dependent**: loop A
   needs queries where reformulation actually helps (multi-
   hop / anaphora-heavy); loop B needs corpora with mixed-
   reliability sources (LoCoMo is uniform Haiku extraction,
   no reliability variance); loop C needs uncertainty
   variance in the query classifier. Choosing the wrong
   loop wastes weeks.

3. **ISS-179 redefines the target**: same logic as
   ISS-182/183/184 — until we know what AC-5a measures,
   we can't tell which metacog loop is worth shipping.

Hold criteria for promotion to P2/P1:
- ISS-179 lands with a target axis that rewards metacog
  behavior (e.g. abstention accuracy, calibrated confidence,
  multi-pass retrieval lift), OR
- A retrieval-side fix lands that unblocks generation
  (then metacog becomes the next bottleneck and a concrete
  loop becomes obviously the right ship), OR
- A non-LoCoMo corpus arrives with mixed-reliability sources
  or strong anaphora structure that rewards loop A or B.
