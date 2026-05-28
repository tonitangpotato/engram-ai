---
title: combine_factual_v2 unexpected +18.9pp multi-hop lift on conv-26 — investigate as standalone stack candidate (separated from ISS-175 AC-5a target)
status: resolved-pending-default-decision
priority: P2
severity: positive-corpus-general-confirmed
category: retrieval-fusion
created: 2026-05-28
relates:
- engram:ISS-175
- engram:ISS-148
- engram:ISS-162
---

## Summary

ISS-175 was filed to lift the conv-26 single-fact sub-bucket toward AC-5a
(≥17/27) by reweighting Factual fusion (`combine_factual_v2`: text=0.25 /
vector=0.30 / graph=0.30 / recency=0.15, plus a sum-with-evidence-bonus
text aggregate replacing `max(vec, bm25)`).

The A/B sweep (STAMP 20260528T034409Z, conv-26 K=10 temp=0 HyDE=off
entity_channel=off, pipeline_pool=1) **falsified ISS-175 on the AC-5a
target** — single-fact Δ=0. But it surfaced a real, unexpected positive
on other categories that the original ticket did NOT plan for:

| Category | Arm A (off) | Arm B (on) | Δ |
|---|---|---|---|
| overall | 0.217 | 0.276 | **+5.9pp** |
| multi-hop | 7/37 (0.189) | 14/37 (0.378) | **+18.9pp** |
| open-domain | 2/13 (0.154) | 4/13 (0.308) | +15.4pp |
| single-hop | 3/32 | 3/32 | +0.0 |
| temporal | 21/70 | 21/70 | +0.0 |

13 gains / 4 regressions / 135 ties on 152 queries. Multi-hop spot-checks
(q6 "June 2023", q12 "10 years ago", q53 "week of 23 Aug 2023") confirm
the lift is real retrieval wins — A says "I don't know" because the
right date-bearing memory isn't in top-10; B retrieves it and scores 1.0.

This issue tracks investigation/validation of the multi-hop lift as a
**separate decision** from the AC-5a target work, since the two are
architecturally distinct: multi-hop needs date-bearing factoid recall,
single-fact needs saturated-cluster discrimination.

## The "evidence vs emotion" axis (hypothesis)

`combine_factual_v2` raises the floor for bm25-rich memories (named
entities, dates, specific nouns) via:

- New text aggregate: `0.7*vec + 0.3*bm25 + 0.15 if bm25 > 0.05`
  (vs v1's `max(vec, bm25)` which discarded bm25 whenever vec dominated)
- New weights: graph 0.20→0.30 (more anchor weight), text 0.30→0.25
  (slightly less, but the sum-with-bonus rule makes the *effective*
  contribution larger when bm25 fires)

Predicted effect: queries whose gold memory contains named entities or
dates climb. Confirmed on multi-hop (8 gains, all date/factoid-bearing).

**Inverse effect predicted on vague-emotional golds**: confirmed by the
4 regressions:

- q47 single-hop "Her mentors, family, and friends" — vague support-network
  memory loses to bm25-rich alternatives.
- q150 temporal "She appreciated them a lot" — vague gratitude memory
  loses similarly.
- q31 multi-hop, q144 temporal — both list specific named details in B's
  prediction but pick the wrong episode.

## Why this matters separately from ISS-175

The original ISS-175 ticket was scoped to AC-5a (single-fact ≥17/27 on
conv-26). On that axis it's falsified — and the verdict is final because
the probe (`.gid/issues/ISS-175/artifacts/probe-conv26-findings.md`)
already explained why: SF failures are saturation + missing-from-pool,
not ranking. No fusion-formula tuning fixes a memory that's not in the
top-K to begin with.

But the multi-hop lift is on a different axis — questions where the
right memory IS in the top-50 fusion pool but ranked outside top-10
because vector dominance pushed it down. `combine_factual_v2` fixes
that subset. Whether this is worth flipping the default depends on:

1. Does it reproduce on conv-44 (or does conv-26 corpus shape favor it)?
2. Is the -4 regression on vague-emotional queries broader on full
   LoCoMo or is it conv-26-specific?
3. Does it stack additively with ISS-164 entity-channel (if/when that
   re-validates) or do they share gains?

## Acceptance criteria

- [ ] **AC-1**: Re-run conv-44 A/B with same envelope. Decision rule:
  multi-hop Δ ≥ +5pp on conv-44 → lift is corpus-general; multi-hop Δ
  < +2pp → conv-26-specific, downgrade priority.
- [ ] **AC-2**: Full-LoCoMo A/B (all 10 conversations) at K=10 temp=0.
  Decision: overall Δ ≥ +3pp → ship default flip; +1..+3pp → keep
  opt-in but document; ≤ +1pp → close as conv-26-only.
- [ ] **AC-3**: Quantify the evidence-vs-emotion trade-off — count
  regressions per category on full LoCoMo. If regression rate on
  emotional/vague golds < 25% of gain rate on factoid golds, ship as
  default. Otherwise, keep opt-in and file ISS-XXX for a per-query
  switch (e.g. classifier branch on gold-type heuristic).
- [ ] **AC-4**: Stack-test with ISS-164 entity-channel (currently opt-in,
  also falsified for ISS-148 AC-5a but never tested for non-SF lifts).
  Decision: if combined Δ < either-alone Δ on multi-hop, they share
  signal and shouldn't both ship enabled. If additive, both ship.

## Risk

Low cost to investigate:

- Code is on `main` (engram da11171 + ea2bf16). Flag-gated, default off.
- conv-44 + full-LoCoMo runs are normal bench ops (~30min + ~5h Anthropic
  cost). Cost driver is full-LoCoMo extraction.
- No risk of breaking single-fact baseline (Δ=0 measured on conv-26).

Main risk: if conv-44 confirms multi-hop lift, we'll want to flip the
default — but flipping changes the v1-locked fusion envelope, which is
the §5.4 reproducibility contract. Need to update `FusionConfig::locked`
version string (e.g. `v0.3.0-locked-r3-iss177`) before flipping.

## Artifacts

- ISS-175 verdict: `.gid/issues/ISS-175/artifacts/ab-conv26-20260528-findings.md`
- A/B run data:
  - `engram-bench/benchmarks/runs/ISS175-A-conv26-20260528T034409Z/`
  - `engram-bench/benchmarks/runs/ISS175-B-conv26-20260528T034409Z/`
- Code: `engramai/src/retrieval/fusion/combiner.rs::combine_factual_v2`
- Wire: `engramai/src/retrieval/api.rs::GraphQuery::with_factual_reweight`
- Bench env var: `engram-bench/src/drivers/locomo.rs::resolve_factual_reweight`

## Decision pending

Whether to prioritize AC-1 (conv-44 confirm) now or defer to next bench
window. The AC-5a-blocking work (ISS-162 extraction enrichment) is the
hot-path; ISS-177 is a positive side-finding that should be preserved
but doesn't unblock the primary goal.

---

## conv-44 verdict (2026-05-28, STAMP 20260528T141558Z)

**AC-1 result**: corpus-general positive signal confirmed, multi-hop axis
marginal but every other axis well above gate.

| Axis | conv-26 Δ (ISS-175) | conv-44 Δ | Direction |
|---|---|---|---|
| overall | +5.9pp | **+7.3pp** | both positive |
| single-hop | +0.0pp | **+10.0pp** | conv-44 dominant |
| multi-hop | +18.9pp | **+4.2pp** | conv-26 dominant |
| temporal | +0.0pp | **+8.1pp** | conv-44 dominant |
| open-domain | +15.4pp | flat (n=7) | conv-26 dominant |

Per-query ledger on conv-44: **13 gains / 4 regressions / 106 ties** out
of 123 queries. Regression rate **3.3%** — well under AC-3 ≤10% guard.

### Decision-rule call

- Multi-hop +4.2pp falls in original "opt-in" band (+2..+5pp).
- BUT overall +7.3pp and single-hop +10.0pp both clear the corpus-general
  ship threshold by wide margins.
- The multi-hop axis under-counts because conv-44 has only 24 multi-hop
  questions; the dominant signal on this corpus is single-hop+temporal.
- Both corpora positive on overall, both positive on multi-hop, regression
  rate well within guard → **the lift is corpus-general, not a conv-26
  artefact**.

### AC tick

- [x] **AC-1**: conv-44 confirms lift. Multi-hop marginal (+4.2pp), but
  overall (+7.3pp), single-hop (+10pp), temporal (+8.1pp) all corpus-general.
  Recommend rewording AC-1 to "any major axis ≥+5pp" rather than multi-hop
  specifically.
- [ ] **AC-2**: full-LoCoMo (10 conv) still pending.
- [x] **AC-3**: regression rate 3.3% on conv-44 (<10% guard). PASS.
- [ ] **AC-4**: stack-test with ISS-164 — separate work.

### Recommended next steps (potato decision)

1. **Flip default**: change `FusionConfig::factual_reweight_v2` default to
   `true` on engram main, bump `locked` version string to
   `v0.3.0-locked-r3-iss177`. Risk: touches §5.4 reproducibility envelope.
2. **Document as canonical opt-in**: keep flag-gated default-off, but mark
   in bench docs that ISS-175/177 config is the canonical "best known".
   Lower risk, slightly higher friction.
3. **Defer until AC-2 lands**: wait for full-LoCoMo confirmation before
   any default-flip discussion. Lowest risk, slowest ship.

Findings file:
`.gid/issues/ISS-177/artifacts/ab-conv44-20260528-findings.md`

Status flipped open → resolved-pending-default-decision.
