---
title: Single-hop LoCoMo conv-26 stuck at 0.22 after BM25 wired — diagnose & lift
status: open
priority: P1
severity: degradation
category: retrieval
created: 2026-05-24
relates: [ISS-144, ISS-145, ISS-146, ISS-147]
---

## Summary

After ISS-147 wired BM25 into the fusion adapters (Factual / Episodic / Affective),
LoCoMo conv-26 single-hop only climbed from **0.156 → 0.219 (+6.25pp)** — well
short of the AC-5 target of ≥0.40. BM25 is now firing in production
(0.4408 → 0.4671 overall, +2.63pp) but single-hop remains the
dominant accuracy hole on conv-26.

This issue tracks the diagnosis and the second lever needed to close
the gap.

## Evidence

### Conv-26 ISS-147 BM25-wired result vs ISS-144 L1-only baseline

| Category | Baseline | ISS-147 | Δ |
|---|---|---|---|
| overall | 0.4408 | 0.4671 | +2.63pp |
| single-hop (32q) | 0.1562 | 0.2188 | +6.25pp |
| multi-hop (37q) | 0.6216 | 0.5946 | -2.70pp |
| open-domain (13q) | 0.3077 | 0.3846 | +7.69pp |
| temporal (70q) | 0.5000 | 0.5286 | +2.86pp |

Run: `benchmarks/runs/ISS147-BM25-conv26-l0.7-20260524T033206Z/`

### Failure mode: "I don't know"

Of the 25 zero-score single-hop fails, the predicted-answer column
shows the LLM saying "I don't know" or "the memories don't specify"
on most of them. Sample fails:

- **q3** gold="Adoption agencies" → "I don't know. The memories only mention that Caroline was 'off to go do some research'..."
- **q7** gold="Single" → "I don't know."
- **q11** gold="Sweden" → "I don't know."
- **q15** gold="pottery, camping, painting, swimming" → "Based on the memories, Melanie signed up for a pottery class..." (only pottery — list-question)
- **q18** gold="beach, mountains, forest" → "I don't know."

The pattern is **the relevant memory never reaches the LLM's
context**, not "the LLM was given the right text and misread it."
BM25 alone cannot fix what's not in the candidate set.

## Hypotheses for the recall gap

1. **Single-hop questions reference attributes mentioned once, deep
   in the conversation.** If the embedding model collapses the query
   into a generic intent and BM25 can't find a strong lexical hook
   (the gold token may not appear verbatim in the question), neither
   channel surfaces the right memory.

2. **The dedup/canonical-memory pass over-collapses.** Multiple
   episodes mentioning the same entity may merge into one canonical
   memory whose surface text doesn't preserve the specific attribute
   (e.g., the canonical memory says "Caroline mentioned her research"
   but loses "about adoption agencies").

3. **Single-hop list questions (q15, q18)** require recalling
   multiple separate episodes. BM25 may surface one of them but the
   model has to **enumerate all** to score full credit. Top-K=10
   may be undersized for list-question recall.

4. **K_seed too narrow.** ISS-147 uses K_seed = max(K\*4, 40) = 40
   for K=10. If the right memory ranks #41+ on either channel, it
   never enters the fusion pool.

## Diagnosis path (next session)

1. Add a `topk_snippets` dump to the LoCoMo bench output (or use
   `embed_rank_diag.py` against the failing q-ids) — answer: is
   the gold-supporting memory in the top-40 candidate pool?
2. If yes → fusion weighting / saturation tuning. Try
   `BM25_DEFAULT_SATURATION` sweep, or per-plan text weight
   sweep (factual 0.40 → 0.55).
3. If no → recall is the bottleneck. Options:
   a. Increase K_seed to 100+ for single-hop plans
   b. Re-rank cross-encoder stage (filed as ISS for ISS-141)
   c. HyDE / query expansion (ISS-141)
   d. Per-namespace dedup tuning (multiple canonical memories
      preserving distinct attributes)

## Acceptance Criteria

- [ ] AC-1: Diagnosis written up — answers "is gold in top-40 pool?"
       for all 25 single-hop zero-score fails on conv-26
- [ ] AC-2: Conv-26 single-hop ≥ 0.35 (Stretch ≥ 0.40, the original
       ISS-147 AC-5 target)
- [ ] AC-3: Overall conv-26 ≥ 0.50 (current 0.4671)
- [ ] AC-4: Full LoCoMo 1540q regression: no category regresses
       more than 1pp vs ISS-147 baseline

## Relates

- **ISS-147**: BM25 wired into fusion (resolved cbddac9 + 5ed5dc0).
  This issue is the follow-up after BM25 alone was insufficient.
- **ISS-144**: L1-only baseline used for comparison.
- **ISS-141**: HyDE / query expansion (separate lever).
- **ISS-145**: GraphEntityResolver visibility (separate dedup angle).
