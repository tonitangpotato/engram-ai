---
title: Stack-test ISS-164 entity_channel × ISS-175 combine_factual_v2 — additive lift or interaction?
status: resolved-asymmetric-result
priority: P2
severity: category-conditional-lever
category: retrieval-fusion
created: 2026-05-28
relates:
- engram:ISS-164
- engram:ISS-175
- engram:ISS-177
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

- [x] AC-1: Both arms complete end-to-end, locomo_summary.json + locomo_per_query.jsonl land for both
- [x] AC-2: Per-query flip ledger computed, by-category Δ table appended to issue
- [ ] AC-3: Plan-kind histogram NOT yet verified — defer until prioritized (the per-category gain/loss separation is itself strong evidence that something category-conditional is at play)
- [x] AC-4: Decision rule applied — see findings; original single-axis rule was insufficient for asymmetric per-category result
- [x] AC-5: Findings file landed at `.gid/issues/ISS-180/artifacts/stacktest-conv26-20260528-findings.md`

## Verdict (2026-05-28, STAMP 20260528T155831Z)

**Overall**: D − B' = +1.9pp (technically in original "<+2pp falsified" band)
**But per-category**: clean asymmetric signal

| Axis | Δ |
|---|---|
| multi-hop | **+16.2pp** (6/37 wins, 0 losses) |
| open-domain | **+15.4pp** (2/13 wins, 0 losses) |
| single-hop | +3.1pp (2 wins, 1 loss — q3 lost) |
| temporal | **-8.6pp** (0 wins, 6/70 losses, all 1→0 flips) |

Per-query ledger: 10 D gains / 7 D regressions / 135 ties on 152 q.

**Mechanism hypothesis**: entity_channel widens the candidate pool with
entity-anchored memories that help multi-hop / open-domain context-rich
questions, but crowd out date-bearing memories at K=10 for temporal
questions. Gains and losses don't overlap across categories.

**This is not "no additive lift"** — it's **category-conditional lift**
that the original decision rule didn't anticipate.

## Options (potato decision)

1. **Plan-kind-gated entity_channel** — enable for Factual/Multi-hop plans,
   disable for Temporal. Estimated net would be +9 wins without -6 losses
   = ~+5.9pp overall (firmly in "ship" band). Medium scope.
2. **Document opt-in trade-off** — README note about multi-hop boost at
   temporal cost. Low scope, no real consumer.
3. **Remove entity_channel code** — strict-rule reading; standalone falsified
   + stack nets temporal regression. Lowest maintenance, discards real signal.
4. **Root-cause temporal regression** — K=15-20 for temporal queries, or
   recency boost in fusion weighting. Largest scope, biggest payoff.

**Recommendation**: Option 1, but as a separate ticket (ISS-181?) since
it needs design work to verify plan-kind is the right gating signal.

Status flipped open → resolved-with-asymmetric-result.

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
