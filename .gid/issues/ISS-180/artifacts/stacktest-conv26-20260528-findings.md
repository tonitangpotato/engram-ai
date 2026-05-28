# ISS-180 stack-test verdict — ISS-164 entity_channel × ISS-175 combine_factual_v2

**Date**: 2026-05-28
**STAMP**: 20260528T155831Z
**Envelope**: conv-26 (152 q), K=10, temp=0, HyDE=off, MMR=off, cross-encoder=off, pipeline_pool=1
**Arms** (both with FACTUAL_REWEIGHT=on):
- B': ENTITY_CHANNEL=off (canonical opt-in baseline, ISS-175 Arm B re-run)
- D : ENTITY_CHANNEL=on (stack test)

## Headline

| Axis        | B'    | D     | Δ          |
|-------------|-------|-------|------------|
| overall     | 0.270 | 0.289 | **+1.9pp** |
| multi-hop   | 0.216 | 0.378 | **+16.2pp** |
| open-domain | 0.308 | 0.462 | **+15.4pp** |
| single-hop  | 0.063 | 0.094 | +3.1pp     |
| temporal    | 0.386 | 0.300 | **-8.6pp** ⚠️ |

Per-query ledger: **10 D gains / 7 D regressions / 135 ties on 152 q**.

## Per-category gain/loss decomposition

| Category    | n   | D gains | D regressions | net |
|-------------|-----|---------|----------------|-----|
| multi-hop   | 37  | +6      | 0              | **+6** |
| open-domain | 13  | +2      | 0              | **+2** |
| single-hop  | 32  | +2      | 1              | **+1** |
| temporal    | 70  | 0       | 6              | **-6** |

**The gains and losses are surgically category-separated**: entity_channel
delivers clean wins on multi-hop + open-domain, and clean losses on
temporal. No category has both gains AND losses simultaneously (the only
exception: single-hop, +2/-1, the conv-26-q3 loss is itself notable —
the question ISS-178 was designed to fix).

## Mechanism hypothesis

Entity_channel pre-fusion candidate-set expansion via entity-overlap
appears to:

1. **Help multi-hop / open-domain**: questions that benefit from broader
   entity-anchored context. The +6 multi-hop wins are mostly
   "give me everything about person X" or "what did Y do" — entity_channel
   pulls in surrounding context the seed-vector retrieval missed.
2. **Hurt temporal**: questions like "when did X happen", "what date",
   "how long ago" need date-bearing memories. Entity_channel widens the
   candidate pool with entity-matched memories that crowd out date
   anchors when downstream fusion has fixed K=10. The 6 lost temporals
   are all clean 1→0 flips, suggesting the right memory existed in B'
   top-10 but was bumped out by entity_channel candidates in D.

## Single-fact sub-bucket (ISS-161 audit set)

| qid | B' | D | Δ |
|---|---|---|---|
| q3 (adoption agencies) | 1.00 | 0.00 | **-1** |
| q70 | 0.00 | 1.00 | **+1** |
| (11 others) | tied | tied | 0 |

Net SF: 2/13 → 2/13 (no change). The q3 regression here is the most
notable — q3 was ISS-178's primary target (prev-turn-fixable noun-phrase
drop). Entity_channel apparently *disrupted* the v1-fusion path that
was previously getting q3 right.

## Decision rule applied

Original (from ISS-180 issue body):
- D − B' ≥ +5pp on overall AND no >2pp regression on any axis → ship stacked
- D − B' in [+2pp, +5pp] → marginal, keep both as separate opt-ins
- D − B' < +2pp → no additive lift, file follow-up to remove entity_channel

Observed: D − B' = **+1.9pp** on overall (under +2pp), but the
per-category picture is asymmetric:
- multi-hop +16.2pp (massive)
- open-domain +15.4pp (massive)
- temporal -8.6pp (clean regression)

The decision rule was a single-axis proxy and doesn't capture this case
well — multi-hop+open-domain combined +8 wins is real lift, balanced
by -6 temporal losses, netting +1.9pp overall but masking strong
underlying signal.

## Honest verdict

**Falsifies the simple "additive stack" hypothesis** — entity_channel is
not a free additive lever. But it's not dead code either:

- Standalone entity_channel was falsified in Phase 2 (overall -3.29pp on
  conv-26 K=10 HyDE=off MMR=off, against v1 fusion default)
- Stacked entity_channel on v2 fusion shows +1.9pp overall on conv-26,
  with clean multi-hop +16.2pp and open-domain +15.4pp signals BUT
  -8.6pp temporal regression

The lift mechanism is **real but category-conditional**. The honest
options:

### Option 1: Gate entity_channel on plan-kind

Enable entity_channel only when the planner selects Factual/Multi-hop
plans, disable for Temporal plans. Requires plumbing plan-kind through
fusion config. Medium scope.

### Option 2: Keep entity_channel opt-in standalone, document trade-off

Add to README: "ENGRAM_BENCH_ENTITY_CHANNEL=on with FACTUAL_REWEIGHT=on
boosts multi-hop +16pp / open-domain +15pp but costs ~9pp on temporal.
Use when your workload is multi-hop-heavy; avoid for temporal-heavy."
Low scope, but doesn't help any real consumer.

### Option 3: Remove entity_channel from code

Standalone falsified, stacked it net-loses 6 temporal questions, no
clear consumer for "multi-hop boost at temporal cost". 78 LoC + 6 tests
to remove. Lowest maintenance, but discards a real signal.

### Option 4: Investigate temporal regression mechanism, fix root cause

The clean -6 temporal regression suggests a specific failure mode: K=10
fixed budget gets crowded by entity-anchor candidates that displace
date-bearing memories. Could be fixable by raising K to 15-20 for
temporal-classified queries, or by giving date-bearing memories a
recency boost in the fusion weighting. Largest scope, but if it works
unlocks the full +16/+15 lift without the temporal cost.

## Recommendation (not a decision)

**Option 1 (plan-kind-gated entity_channel)**: closest to the data.
The category-clean gain/loss separation strongly suggests the right
move is to enable entity_channel for the plans that benefit and skip
it for the plans that don't. Multi-hop +6 + open-domain +2 + single-hop +1
= +9 wins without paying the -6 temporal cost would net to **+5.9pp
overall**, putting it firmly in the "ship as default" band.

This needs design work to verify plan-kind is actually the right gating
signal (vs question-content classifier, vs intent), but the data
endorses the approach.

## ACs

- [x] AC-1: Both arms complete end-to-end, both summaries land
- [x] AC-2: Per-query flip ledger computed, by-category Δ table here
- [ ] AC-3: Plan-kind histogram NOT yet checked — TODO when prioritized
- [x] AC-4: Decision rule applied — falls in "<+2pp" band on overall,
  but per-category picture is asymmetric. Original rule didn't anticipate
  category-clean separation; honest verdict requires the per-category
  analysis above.
- [x] AC-5: This file

## Files

- engram-bench/benchmarks/runs/ISS180-{Bprime,D}-conv26-20260528T155831Z/locomo_{summary,per_query}.{json,jsonl}
- /tmp/iss180_stacktest_sweep.sh
- /tmp/iss180-stacktest/{master,iss180-Bprime,iss180-D}.log

---

## Addendum 2026-05-28 — mechanism re-analysis (per-prediction text inspection)

Spot-checked the predicted-answer text for all 7 D-losses and all 10 D-gains.
The original "temporal" framing in this findings file was **wrong about the
mechanism** (right about the category aggregates, wrong about why).

### Loss/gain decomposition

| Question | gold | mis-labeled as "temporal" but actually... |
|---|---|---|
| q86 | "LGBTQ+ individuals" | who/why question about adoption agency choice |
| q87 | "because of their inclusivity..." | why question |
| q88 | "creating a family for kids..." | what question |
| q120 | "Melanie's daughter" | who question |
| q129 | "Brave by Sara Bareilles" | what (song title) |
| q142 | "An ongoing adventure of..." | summary question |
| q3 | "Adoption agencies" | what (single-hop, NOT temporal) |

The 6 "temporal" losses are **all about Caroline / Melanie / adoption** —
not actual date-grounding questions. They get the temporal label from the
LoCoMo dataset because they sit in time-stamped conversation; the gold is
not a date.

### "I don't know" frequency tells the real story

|              | Losses (7q) | Gains (10q) |
|--------------|-------------|-------------|
| B' says IDK  | **0/7**     | 4/10        |
| D says IDK   | **6/7**     | 0/10        |

- **Losses**: B' answered all 7 confidently with the right answer. D had the
  same retrieval pool widened by entity_channel and **the LLM couldn't find
  the answer in the candidate set** — said "I don't know" 6 times.
- **Gains**: B' said IDK 4 times (insufficient recall). D's wider pool gave
  the LLM enough context to answer — 0 IDKs.

### Real mechanism

**It's a recall-vs-precision tradeoff at the LLM-judge generation stage,
not a retrieval-quality difference.** K=10 is fixed in both arms:

- Where B' top-10 misses the answer-bearing memory → entity_channel
  recovers it → D wins (gains: q2, q6, q10, q15, q35, q50, q57, q63, q67, q70).
- Where B' top-10 already has the precise answer-bearing memory cleanly →
  entity_channel dilutes the top-10 with entity-overlap candidates → the
  precise memory drops out of top-K *or* drops to a low rank where the
  generation LLM gets confused → D's LLM hallucinates "I don't know"
  (losses: q3, q86, q87, q88, q120, q129, q142).

The "temporal" aggregate Δ = -8.6pp is a coincidence of which questions
fall on which side of this recall/precision boundary, not a property of
temporal questions specifically.

### Implications for ISS-180 options

This **changes the recommendation**:

- **Option 1 (plan-kind gating) is now WEAKER** — plan-kind almost
  certainly doesn't separate the gains and losses, because the failure
  mode is generation-stage, not plan-routing. Both wins and losses are
  likely Factual-plan. Would need plan-kind histogram to confirm, but
  the per-prediction text already disproves the "temporal questions are
  different" narrative.
- **Option 4 (root-cause via higher K or reranker) is now STRONGER** —
  the right fix is either (a) raising K for entity_channel-on so the
  precise memory survives the dilution, or (b) running a reranker
  (ISS-159 cross-encoder) so the right memory floats back to the top.
  Both directly target the generation-confusion failure mode.

### Specifically for ISS-159 (cross-encoder reranker)

This finding is **strong evidence to prioritize ISS-159 Step 5 full bench**:
the failure mode here (right memory in candidate set, wrong rank order,
LLM gives up) is exactly what cross-encoder reranking is supposed to fix.
ISS-159 cross-encoder + ISS-164 entity_channel + ISS-175 combine_factual_v2
could be the ship-band stack — wider recall, then precise re-ranking.

### Suggested next step (still requires potato direction)

Run a 3-arm sweep:
- Arm B': FACTUAL_REWEIGHT=on, ENTITY_CHANNEL=off (today's baseline)
- Arm D : FACTUAL_REWEIGHT=on, ENTITY_CHANNEL=on (today's stack arm)
- Arm E : FACTUAL_REWEIGHT=on, ENTITY_CHANNEL=on, CROSS_ENCODER=on (the
  hypothesis-test arm — if E recovers the temporal losses without losing
  the multi-hop gains, the stack story is confirmed)

This is a new ticket (ISS-181 stack-test with cross-encoder).

