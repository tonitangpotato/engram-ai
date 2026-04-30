# RUN-0007 Pre-Fix Baseline (broken graph)

**Date:** 2026-04-30 10:20 EDT
**Substrate:** RUN-0007 (locomo-conv26 sessions 1-3, ingested 04:12 with ISS-072 kind_hint code)
**Graph state:** Broken — ISS-076 dangling mention→entity edges, ISS-075 absent embeddings, ISS-074/078 deferred work
**Backup:** `RUN-0007-substrate-pre-fix/` (preserves this state forever)

## Headline numbers

| Metric | Value |
|---|---|
| **Total hit@5** | **12/25 (48.0%)** |
| **Headline hit@5 (cat 1-4)** | **10/20 (50.0%)** |
| Empty result sets | 2 (8.0%) |

## Per-category

| Category | n | hits | rate | Notes |
|---|---|---|---|---|
| 1 Multi-hop | 3 | 0 | 0.0% | spreading activation expected to be broken by ISS-076 |
| 2 Temporal | 7 | 6 | 85.7% | mostly factual lookup, less affected |
| 3 Open-ended | 1 | 1 | 100.0% | n=1, noise floor |
| 4 Single-hop | 9 | 3 | 33.3% | direct entity match still failing 6/9 |
| 5 Adversarial | 5 | 2 | 40.0% | hit ≠ correctness (gold = unanswerable) |

## Per-plan

| Plan | n | hits | rate | empty | Notes |
|---|---|---|---|---|---|
| Abstract | 4 | 1 | 25.0% | 0 | all downgraded |
| Affective | 2 | 0 | 0.0% | 0 | no_cognitive_state |
| Factual | 17 | 11 | 64.7% | 0 | strongest plan |
| Hybrid | 2 | 0 | 0.0% | 2 | **full collapse** — empty_result_set on both |

## Per-outcome

| Outcome | n |
|---|---|
| ok | 17 |
| downgraded_from_abstract | 4 |
| no_cognitive_state | 2 |
| empty_result_set | 2 |

## Predictions for post-fix

If ISS-076 (dangling mention→entity) and ISS-075 (missing embeddings) are real causes:
- Multi-hop should rise from 0% (currently zero spreading)
- Single-hop should rise from 33% toward 60-70%
- Hybrid empty rate should drop (entity nodes resolvable for graph walks)

If post-fix numbers don't move on these slices → the fixes aren't the binding constraint.

## Source files
- Log: `.gid/eval-runs/RUN-0007-substrate/RUN-0007-baseline-pre-fix.log`
- Script: `.gid/eval-runs/RUN-0007-substrate/02_retrieve_baseline_pre_fix.sh`
- DB at time of measurement: also frozen in `RUN-0007-substrate-pre-fix/`
