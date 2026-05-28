---
title: B-bucket activation — Dimensions: only abstract_l5 plan reads dimension signal; 7 other plans ignore
status: open
priority: P3
severity: feature-inert
category: cognitive-substrate
created: 2026-05-28
relates: [engram:ISS-181, engram:ISS-158]
relates_to: .gid/issues/ISS-181/issue.md
discovered_in: ISS-181 cognitive feature coverage matrix
---

## Summary

`dimensions.rs` ships a per-memory dimension vector
(factual / episodic / emotional / procedural / relational / opinion /
causal) populated by the LLM extractor. Integration test
`dimensional_integration_test` and ISS-158 dim threading pass.

But of the 8 retrieval plans (factual, associative, abstract_l5,
hybrid, affective, bitemporal, narrative, episodic), **only
abstract_l5 reads the dimension signal** in its scoring path.
The other 7 plans treat dimensions as inert metadata.

This means:
- Dimensions affect ≤5% of LoCoMo conv-26 queries (abstract_l5
  routing share)
- ISS-158 fix (`Memory::with_graph_store` respects
  `config.embedding.dimensions`) was correct but the downstream
  consumers haven't been built
- Substrate-level claim "dimensional memory representation" in
  README has narrow practical reach

## What it would take to make dimensions production-active

Two paths, pick one:

### Path A — Dimension-aware fusion channel

Add a `dimension_match` signal to `FusionSignals` that scores
candidate-vs-query dimension overlap:

1. Classify query into a dimension distribution at
   `HeuristicClassifier::classify` (1 LLM call OR a fast keyword
   classifier — the latter avoids per-query LLM cost).
2. In `combine()` / `combine_factual_v2()`, add a `dimension`
   weight channel with a default low weight (0.05–0.10) to
   avoid disturbing the locked envelope.
3. A/B sweep on conv-26 + conv-44.
4. Default OFF until proven; flag-gated like ISS-175.

Risk: low (additive, weight-bounded), but adds an LLM call to
the query hot path unless the query classifier is keyword-based.

### Path B — Dimension-conditional plan routing

Use dimension distribution to bias plan dispatch:
- Factual-heavy queries → Factual plan boost
- Emotional-heavy queries → affective plan (if ISS-182 lands)
- Procedural-heavy queries → procedural plan (would need to be
  filed as a new plan)

Risk: high (rewires `HeuristicClassifier`), depends on per-plan
quality being good — which today it isn't (see ISS-149).

**Recommend Path A** when this issue is taken on.

## Acceptance criteria

- AC-1: A `dimension_match` channel exists in fusion (Path A) OR
  dimension distribution affects plan selection (Path B).
- AC-2: A/B sweep on conv-26 K=10 with dimension channel off vs
  on. Decision rule:
  - Path A: any axis Δ ≥ +2pp with regression rate ≤ 10% →
    ship as opt-in. Δ ≥ +5pp → eligible for default-ON pending
    conv-44 cross-validation.
  - Path B: same gates as ISS-149 (no net SH regression on conv-26).
- AC-3: ISS-181 matrix updated — Dimensions moves from B to A
  if AC-2 passes, stays B otherwise.

## Why P3

- Path A is medium-effort (~200 LoC + new fusion channel + LLM
  call OR keyword classifier).
- Path B is high-effort and high-risk (plan dispatch rewrite).
- Neither has strong evidence of being the next-most-EV lever.
  ISS-179 census doesn't list "dimension awareness" as a fix
  surface for any of the 9 single-fact misses.
- LoCoMo's question distribution doesn't reward dimensional
  routing — most questions are factual+temporal, not
  procedural+emotional.

Reopen at P2 if/when ISS-179 redefine resolves toward a more
dimension-diverse corpus.

## Linkages

- Parent audit: ISS-181 §B bucket
- Companion: ISS-158 (dim threading — kept dimensions queryable
  but didn't activate the signal)
