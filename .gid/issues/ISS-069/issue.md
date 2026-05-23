---
id: ISS-069
title: "Retrieval ranking instability: candidate-pool growth pushes correct gold turns out of top-5"
status: open
priority: P1
filed: 2026-04-29
filed_by: rustclaw
labels: [retrieval, ranking, evaluation, locomo]
relates_to: [ISS-068]
---

# Retrieval ranking instability under candidate-pool growth

## Summary

When the substrate gains additional turns (e.g. after the ISS-068 fix
admitted 19 previously-dropped conversational turns into LoCoMo
conv-26), retrieval **regressed on queries that previously passed** —
not because the previously-correct gold turn disappeared, but because
the enlarged candidate pool surfaced semantically-similar distractors
that ranked above it in top-5.

This is a structural ranking problem, not a data problem. Adding more
correct content **demoted** previously-correct answers. That should not
happen.

## Repro (RUN-0005, conv-26, 2026-04-29)

Both queries had the gold turn present in the substrate before AND
after the ISS-068 fix. Top-5 retrieval changed because newly-admitted
turns crowded the candidates list.

**Source data:** RUN-0004 log (`engram/.gid/issues/_smoke-locomo-2026-04-28/RUN-0004.log`)
for pre-fix; RUN-0005 log
(`engram/.gid/eval-runs/RUN-0005-substrate/RUN-0005.log`) for post-fix.

**Caveat:** The retrieval harness logs only the top-2 turn IDs per query,
not the full top-5 ordering. So we can confirm the gold turn entered /
exited the top-5 (because `hit` flips ✓→✗ when gold is at any rank ≤ 5
vs. > 5), but we cannot give the exact rank within top-5. Capturing
full top-5 (or full top-K with scores) is itself a task — see "Open
data gap" below.

### q14 — REGRESSED ✓ → ✗

- **Gold:** `D2:5`
- **Plan:** Factual, outcome=ok, got=5 (in both runs)
- **Pre-fix (RUN-0004 line 14):** `hit=true`, top-2 = `[D2:3, D2:1]` →
  gold present somewhere in top-5 ranks 3..5 (top-2 didn't show it but
  hit was true)
- **Post-fix (RUN-0005 line 14):** `hit=false`, top-2 = `[D2:4, D2:3]`
  → gold dropped out of top-5 entirely
- **Likely culprit:** `D2:4` is one of the 19 newly-admitted turns
  (post-ISS-068 it's now visible at rank 1) — it displaced `D2:5` from
  top-5 by ranking higher despite `D2:5` still being the gold answer.

### q22 — REGRESSED ✓ → ✗

- **Gold:** `D2:8`
- **Plan:** Abstract → downgraded_from_abstract, got=5 (in both runs)
- **Pre-fix (RUN-0004 line 22):** `hit=true`, top-2 = `[D2:10, D2:7]` →
  gold (`D2:8`) present at one of ranks 3..5
- **Post-fix (RUN-0005 line 22):** `hit=false`, top-2 = `[D2:7, D2:10]`
  → top-2 only swapped order, but the remaining 3 slots turned over
  enough to push `D2:8` out of top-5
- **Likely culprit:** Newly-admitted D2 turns (D2:11–D2:18 are session 2
  turns; ISS-068 admitted at minimum D2:15) displaced the rank-3..5
  positions in this Abstract→downgraded plan, where score margins are
  already thin.

### Net Hit@5 delta

- RUN-0004 (pre-fix): 14/25 (56.0%)
- RUN-0005 (post-fix): 12/25 (48.0%)
- 0 queries that previously failed now pass; 2 queries (q14, q22) that
  previously passed now fail. So the net −2 is entirely structural
  regression on previously-passing queries — there are no offsetting
  gains from the newly-admitted gold turns (q2/q19/q20 never enter
  top-5; that's a separate problem out of scope here).

### Open data gap

The smoke harness only emits top-2 in the per-query log line. To prove
out the monotonicity acceptance criterion (and to give a fix something
concrete to verify against) we need the harness to dump the full top-K
with scores, ideally as JSON per-query. **This is a prerequisite**, not
part of the ranking fix itself. Filing this as a sub-task is reasonable
once eval-stability work begins (see Status notes).

## Why this matters

1. **It blocks treating ISS-068 as a retrieval improvement.** Data-layer
   fixes that should help can't be evaluated honestly while ranking is
   this fragile.
2. **It implies the retrieval layer has no separation margin.** Top-5 is
   determined by very small score deltas, so any candidate-set
   perturbation reshuffles the cutoff. This will keep happening every
   time the substrate changes (more conversations ingested, embedding
   model swap, threshold tweak, etc.).
3. **It makes hit@k metrics non-monotonic in substrate size.** That's a
   fundamental property violation — adding correct content should never
   strictly hurt.

## Hypotheses (not for this issue to resolve — for scoping)

- FTS+embedding hybrid score has no separation gap; ties broken by
  arbitrary order
- No diversity / MMR step in top-k selection — near-duplicates of the
  query crowd out the actual answer
- Embedding model is too generic for conversational content; query and
  gold often share zero lemmas
- No re-ranker stage to verify candidate↔query semantic match before
  trimming to k=5

## Out of scope (intentionally)

- **Query rewriting / HyDE.** Related but different problem (q2/q19/q20
  in RUN-0005 — gold present, never enters top-5 because query↔content
  embedding gap is too large from the start). File separately if it
  remains a problem after ranking is stabilized.
- **Evaluation methodology.** n=25 is too noisy to detect ranking
  improvements (5 runs span 29%-56% hit@5). Need a stabler harness
  before measuring fixes here. Not blocking this issue, but blocking
  any verification of fixes proposed here.

## Acceptance criteria

A fix for this issue should produce, on a held-out repro set including
q14 and q22:

1. **Monotonicity:** adding correct turns to the substrate must not
   reduce hit@5 on queries whose gold turn was already present.
2. **q14 and q22 specifically pass** under the post-ISS-068 substrate.
3. **No regression** on the pre-fix passing set (the 14/25 baseline).

## Status notes

- **Not starting work yet.** Per 2026-04-29 decision: file the issue
  with repro, then stop. Do not chase hit@5 with the current eval
  harness — the n=25 noise floor is too high to attribute movements to
  any single change. Stabilize evaluation first, or accept this issue
  will sit until that's done.

## References

- `.gid/eval-runs/RUN-0005.md` — the run that surfaced the regression
- `.gid/issues/ISS-068/issue.md` — data-layer fix; post-fix correction
  note explains the retrieval non-lift in detail

---

## 2026-05-23 update — full-152q evidence + recall@K ceiling

After ISS-137 (temp=0) pinned LoCoMo variance to 0.66pp stdev, we can
now read retrieval quality directly off the J-score with confidence.
Using `engram-bench/examples/iss136_candidate_histogram.rs` (no-LLM
probe) on conv-26 full 152q at git `82e26d6`, the retrieval recall@K
curve is:

| K | ALL | single-hop | multi-hop | open-domain | temporal |
|---|-----|-----------|-----------|-------------|----------|
| 1 | 18.7% | 2.7% | 40.5% | 10.0% | 26.8% |
| 3 | 25.6% | 4.0% | 54.1% | 15.0% | 36.6% |
| **5** (LoCoMo K) | **31.5%** | **9.3%** | **62.2%** | 20.0% | 42.3% |
| 10 | 39.4% | 17.3% | **70.3%** | 30.0% | 49.3% |
| 20 | 41.9% | 18.7% | 70.3% | 40.0% | 52.1% |
| 30 | 43.8% | 22.7% | 70.3% | 40.0% | 53.5% |
| 50 | 45.3% | 26.7% | 70.3% | 40.0% | 53.5% |

This decomposes into two failure modes:

### Mode A — ranking problem (recall keeps climbing past K=5)
- **single-hop**: 9.3% @ K=5 → 26.7% @ K=50 (2.9× lift from ranking
  alone). Evidence is in the candidate pool, just buried at ranks
  10-50.
- **open-domain**: 20% @ K=5 → 40% @ K=50 (2× lift).

This is the original ISS-069 pathology, now sharper.

### Mode B — recall ceiling (retrieval truly can't find it)
- **multi-hop**: saturates at **70.3% by K=10**, no improvement at K=50.
  29.7% of multi-hop evidence is **not in top-50 at all**. This is not
  a ranking problem — these evidence turns aren't being retrieved by
  any combination of fusion components.
- **temporal**: saturates at 53.5% by K=30.

### Failure-mode correlation with LoCoMo J-score

| Category | recall@5 | LoCoMo run-1 score | gap |
|----------|----------|---------------------|-----|
| single-hop | 9.3% | 9.4% | ~0 — generator is bottlenecked by retrieval |
| multi-hop | 62.2% | 51.4% | 10pp — generator drops some recalled evidence |
| open-domain | 20.0% | 38.5% | +18pp — generator synthesizes correctly w/o full evidence |
| temporal | 42.3% | 48.6% | +6pp |

**Single-hop score = single-hop recall**. Generator is doing its job
honestly: it says "I don't know" 55% of failures because retrieval
returned no evidence, and gives partial-list answers the rest because
retrieval returned 1 of 3-4 evidence turns. This is not a generator
problem.

### List-question failure pattern

The 13 "partial-list" failures (q15, q24, q32, q34, q37, q39, q43, q48,
q51, q60, q65, q70 etc.) all share the structure: gold = comma-separated
list of 2-4 items, evidence = 2-4 dialog turns from different sessions.
With K=5, even if 2 of 4 evidence turns rank in top-5, the generator
emits a partial answer and LLM-judge scores it 0.0.

Possible fix directions (not committing to one):
1. **Raise K from 5 to 10** — would lift overall recall 31.5% → 39.4%
   (+8pp). LoCoMo published K=5 to match mem0; we can re-baseline.
2. **MMR / diversity in top-K** — list questions need spread, not
   concentration. Current fusion concentrates near the query embedding.
3. **Re-ranker stage** — score top-50 candidates against query semantic
   role, not lexical similarity. Would target Mode A specifically.
4. **HyDE / query expansion** — expand "What did Caroline research?"
   into hypothetical-document form before embedding. Targets Mode B
   (multi-hop ceiling).

### Concrete failure trace (q3 "What did Caroline research?")

- gold: "Adoption agencies" / evidence D2:8 = "Caroline: Researching
  adoption agencies — it's been a dream to have a family..."
- top-5 retrieved (verbatim from probe k=10):
  1. "caroline: totally agree, mel. relaxing and expressing ourselves"
  2. "caroline: cool! what did it look like?"
  3. "melanie: wow, what an experience! how did it make you feel?"
  4. "melanie: thanks for the tip, caroline. doing research and readyi..."
  5. "melanie: that sounds awesome! what did you take away from it..."
- Rank 4 contains the word "research" — but as a verb in Melanie's
  reply context, not Caroline's adoption research statement. Lexical
  match works; semantic role doesn't.

### Run artifacts

- `/tmp/iss136-histogram-current2.json` (K=10, git 82e26d6)
- `/tmp/iss136-histogram-k50.json` (K=50, git 82e26d6, 2026-05-23)
- `engram-bench/benchmarks/runs/ISS137-temp0-20260523T034444Z/` (run 1)

### Status update

Still open. **Now blocking** any further LoCoMo signal extraction —
overall stuck at 0.40 ± 0.66pp until retrieval gets a real lift.
