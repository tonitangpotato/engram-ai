# RUN-0008 Post-Fix (ISS-076 Phase A only)

**Date:** 2026-04-30 ~10:55 EDT
**Substrate:** RUN-0008 (locomo-conv26 sessions 1-3, ingested with ISS-076 dangling-UUID fix)
**Compare to:** RUN-0007 baseline (`.gid/eval-runs/RUN-0007-substrate/`)

## Graph integrity (smoking gun for the bug)

| metric | RUN-0007 (broken) | RUN-0008 (fixed) |
|---|---|---|
| entities | 187 | 180 |
| edges | 125 | 121 |
| edges with dangling subject_id | **125 (100%)** | **0 (0%)** |
| edges with dangling object_entity_id | **125 (100%)** | **0 (0%)** |

The fix worked at the data layer: every edge now points at a real entity row.
ISS-076 root-cause confirmed and fixed.

## Retrieval hit@k (Phase A only)

| metric | RUN-0007 | RUN-0008 | delta |
|---|---|---|---|
| Total queries | 25 | 25 | — |
| Hits @ 5 | 12 (48.0%) | 12 (48.0%) | **0** |
| Empty results | 2 (8.0%) | 2 (8.0%) | 0 |
| Headline hit@5 (cat 1-4) | 10/20 = 50.0% | 10/20 = 50.0% | **0** |

Per-category: identical across all 5 categories.
Per-plan: identical across Factual/Abstract/Hybrid/Affective.
Per-outcome: identical (4 downgraded_from_abstract, 2 empty, 2 no_cognitive_state, 17 ok).

## Verdict

**Removing dangling edges alone moves nothing.**

The retrieval pipeline was never reading the broken edges in a way that
affected hit@k for these 25 queries. Either:

1. Retrieval barely uses graph edges in this query mix (Factual plan dominates, n=17),
   so edge integrity is not the binding constraint at hit@5.
2. The downstream plans (Abstract, Hybrid) are gated on other capabilities
   (L5 unavailable → 4 abstract downgrades, no L4 cognitive state → 2 affective
   no-ops). These never reach the edge-traversal step regardless of edge health.
3. The 25-query sample is too coarse to detect smaller deltas.

**ISS-076 alone is not the binding constraint on retrieval quality.**
This was exactly the hypothesis we wanted to falsify — and it was falsified.

## Implications for next steps

- ISS-076 still must ship (silent data corruption — 100% of CreateNew edges
  pointed at non-existent rows). The fix is correct and verified at the
  graph-integrity layer. It just doesn't move the retrieval needle on its own.
- The retrieval bottleneck is upstream of edge traversal:
  - 4/25 queries downgrade because L5 (abstract substrate) is unavailable
  - 2/25 queries no-op because L4 (cognitive state) is missing
  - 2/25 hybrid queries return empty because both sub-plans degrade
- Phase B (ISS-075 sync embedding + ISS-077 in-batch dedup) is now necessary,
  not optional, to see retrieval movement. Their effect can be measured against
  RUN-0008 as the new baseline (clean-edge graph, no embedding/dedup).

## Files

- Substrate: `.gid/eval-runs/RUN-0008-substrate/locomo-conv26-iss076.{db,graph.db}`
- Ingest log: `.gid/eval-runs/RUN-0008-substrate/ingest.log`
- Retrieve log: `.gid/eval-runs/RUN-0008-substrate/RUN-0008-post-fix.log`
- Scripts: `01_ingest.py`, `02_retrieve_post_fix.sh`
