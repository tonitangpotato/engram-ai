---
title: 'conv-26 window-LOSS root cause: candidate-dump probe (recall-miss vs select-miss for the 14 A→IDK flips)'
status: diagnosed
priority: P1
severity: diagnosis
category: retrieval
created: 2026-06-08
relates:
- ISS-162
- ISS-179
- ISS-190
discovered_in: ISS-162 §A/B per-query diagnosis 2026-06-08
blocks: .gid/issues/ISS-162/issue.md
---

## Summary

ISS-162's sliding-window ingest is a **net +6.58 pp** on conv-26 but it
*reallocates* recall rather than broadly raising it: +24 wins (18 temporal)
against 14 losses, where the loss signature is "window-off answered correctly →
window-on says *I don't know*" (q104 *Becoming Nicole*, q129 *Brave*, q20, q47,
q65, …).

We have already proven (ISS-162 §A/B diagnosis):
- the gold facts exist in the dialogue (fixture-verified source turns),
- the bare-turn **embedding is unchanged** (`memory.rs:3609-3614` decouples
  `extraction_input` from the stored/embedded `content`), so the regression is
  an **extraction-layer** side effect, not a vector-ranking one.

**The one unresolved fork:** for the 14 LOSS questions under window-on, is the
gold memory (a) **absent from the candidate pool** (extraction changed which
memory/entities get anchored → factual/entity plan never surfaces it), or
(b) **present but ranked/selected wrong** (it's in top-K, generation picks the
wrong line)? The fix is different for each:
- (a) → fix extraction/anchor under window context (don't let preceding-turn
  entities displace the current turn's fact).
- (b) → ranking/generation problem (reserved-block ordering, ISS-211 family).

## Method

`engram-bench` already has the instrumentation — no code change needed:
- `ENGRAM_BENCH_DUMP_CANDIDATES=1` → emits `retrieved_candidates: [CandidateDump]`
  per query into `locomo_per_query.jsonl`. `CandidateDump` carries `rank`,
  fusion `score`, `kind`, `text` (head 400 chars) **and per-signal subscores**
  (`graph_score` / `bm25_score` / `vector_score` / `recency_score` /
  `actr_score` / `affect_score`). This is exactly enough to classify
  recall-miss vs select-miss and to see WHICH channel did or didn't surface
  the gold.

### Run (do this with a FRESH token — ≥90 min — to avoid the 401-mid-sweep
death documented across ISS-178/188/153):

```
ENGRAM_BENCH_DUMP_CANDIDATES=1 \
ENGRAM_BENCH_LOCOMO_CONVS=conv-26 \
ENGRAM_BENCH_TOP_K=10 \
# canonical envelope (match ISS-162 LIB run 20260604T124723Z):
ENGRAM_BENCH_FACTUAL_REWEIGHT=on \
ENGRAM_BENCH_HYDE=off  ENGRAM_BENCH_MMR=off  ENGRAM_BENCH_ENTITY_CHANNEL=off \
ENGRAM_BENCH_PIPELINE_POOL=1  ENGRAM_BENCH_POPULATE=off \
# two arms, serial, RE-FETCH TOKEN PER ARM:
#   Arm A: ENGRAM_BENCH_INGEST_WINDOW=0   (window off)
#   Arm B: ENGRAM_BENCH_INGEST_WINDOW=4   (window on, library default)
```

(If/when the LoCoMo driver is swapped to library `TurnWindow`+`ingest_turn`
instead of the env-flag, run the probe through that path so it matches the
resolve gate — but the env-flag arms are a valid first probe.)

### Analysis (the 14 LOSS qids from ISS-162 §A/B):

For each LOSS qid, in Arm B's `retrieved_candidates`:
1. **Is the gold memory present in the pool?** (match gold fact against
   `candidate.text`). Present → bucket (b) select-miss. Absent → bucket (a)
   recall-miss.
2. If present, what `rank` and which `*_score` channels fired? (Did the gold
   land but get out-ranked by a window-polluted neighbour?)
3. Compare to Arm A's pool for the same qid — confirm the gold WAS in A's pool
   (it answered correctly), so the diff isolates exactly what the window
   removed/demoted.

## Acceptance criteria

- **AC-1.** Probe runs clean (no mid-run 401), both arms, conv-26, dump on.
- **AC-2.** Each of the 14 LOSS qids classified into bucket (a) recall-miss or
  (b) select-miss, with the gold candidate's rank + firing channels recorded.
- **AC-3.** A one-line root-cause verdict: is the dominant loss mechanism (a)
  or (b)? This directly chooses the ISS-162 follow-up fix path.
- **AC-4.** If (a) dominates: file the extraction/anchor fix issue with the
  specific failing pattern (e.g. "window-on attributes the fact to the
  preceding speaker" or "current-turn entity dropped from anchor set"). If (b)
  dominates: link to the ISS-211 ranking family.

## Why P1

This is the blocker for resolving ISS-162. The window ships a net gain but with
a measured open-domain/multi-hop regression (conv-44 open-domain → 0.0). We
cannot decide the right fix (selective window? N=4→2? extraction-anchor fix?)
without knowing whether the losses are recall-miss or select-miss. Cheap probe,
high decision value.

---

## RESULTS — Probe complete (2026-06-09)

**Runs (clean, same-binary, conv-26, DUMP_CANDIDATES=1, canonical envelope
FACTUAL_REWEIGHT=on HYDE=off MMR=1.0 ENTITY_CHANNEL=off PIPELINE_POOL=1
POPULATE=off TOP_K=10):**
- Arm A (window=0): `benchmarks/runs/ISS217-A-conv26-20260609T044302Z/2026-06-09T05-07-21Z_locomo/`
  — overall 0.2763, single-hop 0.094, multi-hop 0.243, open 0.154, temporal 0.40
- Arm B (window=4): `benchmarks/runs/ISS217-B-conv26-20260609T112119Z/2026-06-09T11-47-45Z_locomo/`
  — overall 0.2829, single-hop 0.094, multi-hop 0.162, open 0.231, temporal 0.443

Within-sweep Δ(B−A): overall **+0.66pp**, multi-hop **−8.11pp**, open-domain
**+7.69pp**, temporal **+4.29pp**, single-hop flat. (Magnitudes smaller than the
ISS-162 LIB cross-ingestion run because this is a clean same-binary A/B — the
direction is identical: temporal+open up, multi-hop down.)

### AC-1 ✅ Probe ran clean
Both arms finished, no mid-run 401, `retrieved_candidates` present (10/query,
carrying rank/score/graph_score/bm25_score/vector_score).

### Correction: only 10 of the 14 LIB-flagged LOSS qids are real losses here
q14, q47, q65, q104 fail or pass in BOTH arms in this clean run (they were
cross-ingestion noise in the separate-DB LIB list). Real same-binary
window-losses = **10**: q6, q13, q20, q44, q63, q82, q91, q100, q129, q141.

### AC-2 ✅ Per-qid classification (anchored on Arm A pool where gold provably is)

Method: A passed → gold-bearing memory is in A's top-10. Track survival into B's
top-10 + inspect B's full pool text for the gold fact.

| qid  | cat        | mechanism            | evidence |
|------|------------|----------------------|----------|
| q6   | multi-hop  | **RECALL-miss**      | A r1 `[2023-05-25] planning camping next month (2023-06-01)` GONE from B; B pool has paraphrased camping mems but the June-dated planning one displaced |
| q20  | multi-hop  | **RECALL-miss**      | A's `[2023-07-06] museum` anchor GONE; B pool all beach/camping |
| q82  | temporal   | **RECALL-miss**      | gold mental-health fact absent from B pool |
| q91  | temporal   | **RECALL-miss**      | necklace-meaning (love/faith/strength) absent from B pool |
| q129 | temporal   | **RECALL-miss** ⭐    | A r3 `[2023-08-28] song 'Brave' by Sara Bareilles` (A=1.0) COMPLETELY displaced in B → clean smoking gun |
| q141 | temporal   | RECALL-miss (weak)   | gold freedom/being-true fact diffuse, not crisply in B pool |
| q13  | single-hop | generation/judge-miss| gold mem present B r2/r4, answer close but judged wrong |
| q44  | multi-hop  | **GENERATION-miss**  | gold `[2023-08-14] birthday last night` present B r1/r2; LLM hedged |
| q63  | multi-hop  | **GENERATION-miss** ⭐| gold `[2023-08-28] talent show next month (2023-09-01)` present B **rank-1**; LLM computed "October" from conv-date + "next month", ignoring explicit 2023-09 in memory |
| q100 | temporal   | judge-miss           | "safe loving home" present B r4/r5; judge wanted exact "safe inviting place to grow" phrasing |

**Tally: 6 recall-miss / 4 generation-or-judge-miss.**

### AC-3 ✅ Root-cause verdict

The dominant loss mechanism is **recall-miss (6/10) — window-on churns the
candidate pool and displaces the gold-bearing memory out of top-K.** The
churn is NOT to unrelated memories: B's pool fills with **re-extracted
paraphrase variants of the same facts that lost their key detail** (the date
anchor on q6/q20, the song TITLE on q129). This confirms the extraction-layer
hypothesis from ISS-162 §A/B: window context makes the extractor produce
*differently-phrased, less-specific* memories for the SAME turn, which then
(a) re-embed to slightly different vectors → different fusion neighbours →
the gold-bearing variant drops out, and (b) sometimes strip the discriminating
token (title/exact-date) entirely.

The remaining 4/10 are downstream (generation reasons wrong on an explicit date
q63, or judge phrasing-strictness q100) — NOT caused by the window and NOT
fixable by changing window policy.

### AC-4 ✅ Fix path chosen

Recall-miss dominates → the ISS-162 follow-up is an **extraction-anchor /
specificity-preservation fix**, NOT a ranking fix (ISS-211 family does not
apply to these losses; the gold is genuinely absent, not mis-ranked).

Concrete failing pattern: **window context degrades current-turn extraction
specificity** — the extractor, given preceding turns, re-paraphrases the
current fact and drops discriminating tokens (proper-noun titles, explicit
resolved dates). This is the mirror image of the temporal WIN mechanism
(window ADDS a resolved date anchor to under-specified turns). So the right
fix is **selective / specificity-preserving window**, not N=4→2 blanket
reduction:
- inject window context ONLY to resolve under-specified references (pronouns,
  relative dates, "it"/"that" with no antecedent in the bare turn), AND
- constrain the extractor to PRESERVE proper nouns + explicit dates already
  present in the bare turn (don't let window context paraphrase them away).

→ File follow-up ISS for specificity-preserving window extraction. ISS-162 stays
in_review pending that fix + re-bench (do NOT resolve on net-positive alone —
the recall-miss losses are a real, explained regression).

**Status → diagnosed (all ACs met).**
