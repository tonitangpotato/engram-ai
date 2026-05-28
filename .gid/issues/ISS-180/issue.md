---
title: Stack-test ISS-164 entity_channel × ISS-175 combine_factual_v2 — additive lift or interaction?
status: open
priority: P2
severity: lever-composition
category: retrieval-fusion
created: 2026-05-28
relates: [engram:ISS-164, engram:ISS-175, engram:ISS-177]
---

## Summary

ISS-164 (entity_channel) was falsified standalone in its Phase 2 bench
(conv-26: sf 3/12=3/12 +0, overall -3.29pp, multi-hop -10.81pp).
ISS-175 / ISS-177 (combine_factual_v2) shipped as canonical opt-in.

Open question: does entity_channel become a positive lever when
*stacked on top of* combine_factual_v2? The two changes touch different
parts of the pipeline:

- combine_factual_v2: post-fusion weight reshape (text/vector/graph/recency)
- entity_channel: pre-fusion candidate-set expansion via entity overlap

In principle they're orthogonal: entity_channel widens the candidate
pool, combine_factual_v2 then re-ranks. The Phase 2 falsification was
against the **v1 fusion default**, which may have not known how to
score the entity-channel-recovered candidates correctly. v2 fusion's
new graph-channel weight (0.30 vs v1) could rehabilitate them.

## Methodology

Within-STAMP A/B on conv-26, identical envelope to ISS-175:

- Arm B' (control): `FACTUAL_REWEIGHT=on  ENTITY_CHANNEL=off`
- Arm D (stack):    `FACTUAL_REWEIGHT=on  ENTITY_CHANNEL=on`

K=10, temp=0, HyDE=off, MMR=off, cross-encoder=off, force_intent=off,
pipeline_pool=1.

Why re-run Arm B' instead of using ISS-175 Arm B existing data:
LLM-judge run-to-run noise ~0.66pp stdev (ISS-137). Within-STAMP A/B
controls for that.

Harness: `/tmp/iss180_stacktest_sweep.sh`.

## ACs

- [ ] AC-1: Both arms complete end-to-end, locomo_summary.json + locomo_per_query.jsonl land for both
- [ ] AC-2: Per-query flip ledger computed, by-category Δ table appended to issue
- [ ] AC-3: Plan-kind histogram verified — Factual ~113/152 in both arms (per ISS-166 pipeline_pool=1 fix); entity-channel candidates non-zero in Arm D
- [ ] AC-4: Decision rule applied:
  - D − B' ≥ +5pp on overall AND no >2pp regression on any axis → stack confirmed, file follow-up to bake both as default-on
  - D − B' in [+2pp, +5pp] → marginal, keep both as separate opt-ins
  - D − B' < +2pp → no additive lift, file follow-up on whether to remove entity_channel code (it failed standalone AND stacked, evidence for removal)
- [ ] AC-5: Findings written to `.gid/issues/ISS-180/artifacts/stacktest-conv26-{STAMP}-findings.md`

## Why this matters

If stack lifts overall: justification for cleaning up FusionConfig
defaults and shipping both v2 fusion + entity channel together.
If stack doesn't lift: ISS-164 entity_channel code is dead code in
both standalone and stacked configurations — clear ground for removal.
Either way the outcome unblocks ISS-164's open status.

## Related

- ISS-164: entity_channel original ticket (falsified standalone Phase 2)
- ISS-175: combine_factual_v2 conv-26 ship
- ISS-177: combine_factual_v2 conv-44 confirm
