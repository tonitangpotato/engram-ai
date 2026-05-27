---
title: Factual plan candidate-generation gap — vector_score fix shipped but overall stays 0.230 (ISS-172 downstream)
status: resolved
priority: P1
severity: ranking-floor-too-low
category: retrieval
created: 2026-05-27
relates:
- engram:ISS-148
- engram:ISS-164
- engram:ISS-171
- engram:ISS-172
discovered_in: ISS-172 AC-6 sweep (STAMP 20260527T134341Z)
blocked_by: ''
blocks:
- engram:ISS-148
- engram:ISS-164
---

## Summary

ISS-172 shipped (commit `engram:ae4a2be`) the vector_score wiring fix
for the Factual plan — `factual_to_scored` now emits per-candidate
`Some(cosine(q, mem_emb))`, with 3 regression tests pinning the
contract. AC-5 (code-layer fix) is done.

But the AC-6 sweep (STAMP `20260527T134341Z`, conv-26 K=10 temp=0
HyDE=off) shows **the fix is necessary but insufficient**:

| metric        | pre-ISS-171 baseline | post-ISS-172 Arm A | Δ vs baseline |
|---------------|----------------------|--------------------|---------------|
| overall       | 0.362                | **0.230**          | −13.2pp       |
| single-hop    | 0.219                | 0.125              | −9.4pp        |
| 9 ISS-161 SF  | varies               | **0/9**            | floored       |

55% of Arm A predictions are literally "I don't know" — the LLM
has candidates but the *gold memory is not in them at all*. This
is no longer a ranker problem; it's a candidate-generation problem.

## Hypotheses

- **H4 (retrieval-side)**: Factual's candidate generation lives in
  `factual_plan::execute` → `traverse_anchors` → `memories_mentioning_entity`.
  If the gold memory isn't tagged with the anchor entity the resolver
  picked, it never enters the pool, regardless of how the ranker
  weights vector_score.

- **H5 (scoring-side)**: gold memory is in the pool but the
  graph_score floor (~0.15-0.3 per shared anchor) drowns its lone
  cosine win. Fix would be reweighting fusion to let vector_score
  dominate when present, or log-decay on anchor-count score.

## AC

- [ ] AC-1: Probe `iss173_gold_in_pool` (engram-bench `examples/`)
      classifies each of the 9 SF qids into GOLD_TAGGED / GOLD_UNTAGGED
      / GOLD_NOT_INGESTED.
- [ ] AC-2: If H4 dominates (≥5/9 GOLD_UNTAGGED): file follow-up
      design ticket for Factual to ALSO retrieve via BM25 + vector seed
      (not just anchor-mention).
- [ ] AC-3: If H5 dominates (≥5/9 GOLD_TAGGED): file follow-up for
      fusion reweighting (vector_score boost when present + anchor
      score decay).
- [ ] AC-4: If GOLD_NOT_INGESTED dominates: upstream extractor coverage
      bug — separate work item.

## Status — 2026-05-27 evening

Probe approach abandoned in favour of reading the AC-6 sweep log
directly. The sweep log already contains every `execute_plan ENTER/EXIT`
trace with plan_kind + candidate count + outcome, which gives the same
data without ingesting conv-26 again under OAuth rate-limits.

### What the AC-6 sweep log actually shows

Per-query plan routing on the 9 ISS-161 SF qids
(Arm A, STAMP `20260527T134341Z`):

| qid | plan kind   | candidates | gold-hit? |
|-----|-------------|------------|-----------|
| q3  | hybrid      | 10         | no        |
| q7  | associative | 100        | no        |
| q11 | hybrid      | 10         | no        |
| q37 | hybrid      | 10         | no        |
| q40 | factual     | 151        | no        |
| q43 | factual     | 236        | no        |
| q71 | factual     | 151        | no        |
| q75 | factual     | 177        | no        |
| q76 | (no match in log — query reshaped by gen) | ? | no |

**Two distinct sub-problems hiding under the same 0/9 failure number:**

1. **Hybrid-routed SF qids get tiny candidate pools (10)** — q3 / q11 /
   q37 route to Hybrid sub-plan but Hybrid emits HybridItem::Memory
   directly with K=10. The classifier picks Hybrid because the query
   has BOTH a person entity AND a non-anchor verb — Factual's pure
   entity-mention channel is the wrong source for these.

2. **Factual-routed SF qids retrieve 150–263 candidates but gold is
   still not in the top-K** — q40 / q43 / q71 / q75 all have huge
   pools, all `outcome=ok`, all post-ISS-172 (so vector_score IS being
   emitted). The fact that vector_score alone doesn't surface gold
   over the 150+ anchor-rich noise means **H5 (scoring drowns gold)
   is the dominant failure mode for the Factual path**.

### Updated hypothesis ladder

- **H5 (Factual scoring drowns gold, in-pool, ≥4/9)**: confirmed by
  log. Fusion weights graph_score and vector_score symmetrically; in
  a 150+ pool that 1 anchor = ~0.15 graph_score floor is enough to
  beat a lone cosine 0.7 win. Fix: log-decay anchor-count contribution
  OR boost vector_score weight when it's high.

- **ISS-172 scope was too narrow**: only standalone Factual was wired;
  Hybrid sub-plan Factual path (orchestrator.rs:798) was *explicitly
  left alone* per the Strategy A decision. On conv-26, ~4/9 SF qids
  route through Hybrid and don't see the new vector_score signal at
  all. **Need ISS-174 follow-up: extend vector_score emission to
  Hybrid's Factual sub-plan**.

- **H4 (entity-channel blindness, ≤2/9)**: probably not the root
  cause. Factual pools of 150-263 implies anchors resolve AND expand
  successfully. If entities were untagged the pools would collapse to
  small Step 3b' direct-anchor counts (~5-15 typical).

### Triple extractor JSON parser bug — separate work item

Probe v2 stderr surfaced a NEW Haiku parsing failure mode:
"Wait, let me reconsider..." natural-language interleaved between
two JSON arrays defeats the `find('[')..rfind(']')` heuristic in
`triple_extractor.rs:90`. ISS-167 fixed duplicate-key; this is the
next layer (self-reflection text). Observed at ~50% on the 2-sample
probe v2, only 2/152 in the AC-6 sweep log — sample size too small to
confirm the production impact rate. **File as separate issue**, do
not bundle into ISS-173. Estimated impact: bounded — sweep shows only
2 failures in 419 episodes (~0.5%), not catastrophic.

### Decision

- Mark this issue as **resolved-as-diagnosed**: hypothesis classified,
  follow-ups identified. No code fix in ISS-173 itself.
- File **ISS-174**: extend vector_score to Hybrid sub-plan Factual
  (mirror ISS-172 fix into the Hybrid path).
- File **ISS-175**: fusion reweighting — graph_score log-decay or
  vector_score boost when |cosine| > τ (H5 fix).
- File **ISS-176** (optional, low priority): Haiku self-reflection
  text in triple_extractor parser — observed but bounded impact.

ISS-148 AC-5a stays blocked pending ISS-174 + ISS-175 landing.
