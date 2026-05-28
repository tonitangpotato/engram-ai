---
title: Redefine ISS-148 AC-5a — best-case lever stack tops at ~14-17/27 on conv-26 SF; current 17/27 (0.63) target may be structurally unreachable
status: open
priority: P1
severity: target-feasibility
category: ship-gates
created: 2026-05-28
relates:
- engram:ISS-148
- engram:ISS-161
- engram:ISS-175
- engram:ISS-177
- engram:ISS-178
relates_to: .gid/issues/ISS-148/issue.md
updated: 2026-05-28
---

## Summary

The ISS-148 AC-5a target ("single-fact ≥17/27 on conv-26") was set in
early bench planning before we had the per-question failure-mode taxonomy
for conv-26. Today's evidence base (ISS-161 audit + ISS-175/177 lever
sweep) suggests the target may be structurally unreachable on
conv-26 with the lever set we have, and is worth a deliberate redefine
discussion before sinking more code into chasing it.

## Per-question fix-surface census

(Source: `.gid/issues/ISS-161/artifacts/heartbeat-1240-real-failure-modes.md`,
hand-mapped over conv-26 27 single-fact questions; 5 currently PASS.)

| Bucket | qids | n | Fix surface | Status |
|---|---|---|---|---|
| **Already PASS** | (5) | 5 | — | passing in ISS-175 Arm A baseline |
| Aggregation across episodes | q40, q43, q75 | 3 | counting/synthesis logic, not retrieval | no lever in current roadmap |
| Cross-session graph synthesis | q11 | 1 | ISS-070 multi-hop dispatcher | P0 graph work, separate project |
| BLIP image extraction | q37 | 1 | image-caption ingestion | infrastructure, not in retrieval |
| Generation failure (fact in top-K) | q71 | 1 | answer-generation prompt, LLM behavior | judge/generator work |
| Temporal grounding ("yesterday" → date) | q76 | 1 | temporal resolver, not extractor | needs ISS-? (not filed) |
| Prev-turn noun phrase | q3 | 1 | ISS-178 slim ExtractionContext | tractable |
| Maybe prev-turn helpful | q7 | 1 | ISS-178 ExtractionContext (partial) | maybe tractable |
| Embedder ranking (factual reweight benefited) | various | ~2-4 | ISS-175/177 combine_factual_v2 | shipped opt-in |
| Other (unclassified, smaller cases) | residual | ~10-12 | unknown | needs case-by-case audit |

## Lever-by-lever expected lift on SF

(Best-case per-lever, NOT additive — many overlap on the same qids.)

| Lever | Status | SF lift on conv-26 |
|---|---|---|
| ISS-175 combine_factual_v2 | shipped opt-in | +0 (single-fact axis was flat on conv-26) |
| ISS-178 prev-turn context | proposed slim | +1 to +2 (q3, maybe q7) |
| ISS-070 multi-hop dispatcher | open P0 | +1 (q11) |
| Aggregation/counting | no lever filed | +0 to +3 if filed (q40, q43, q75) |
| BLIP ingestion | infrastructure | +1 (q37) |
| Generation prompt rework | no lever filed | +0 to +1 (q71) |
| Temporal resolver | no lever filed | +0 to +1 (q76) |
| **Stack ceiling (best-case)** | — | **5 + 5..8 = 10-13/27** |

Even if every conceivable lever in the current pipeline lands optimally,
the best-case stack tops at **~13/27 = 0.48**, below the **17/27 = 0.63**
AC-5a target.

To reach 17/27 we would need 12 net gains from the 22 currently-failing
questions, but the audit shows only ~8 of those 22 have an identified
fix surface in the current roadmap. The remaining ~14 either need
infrastructure work not on the roadmap (BLIP, temporal, generation,
multi-hop) or are residual cases where the failure mode is unclassified.

## Why AC-5a was set at 0.63

(Speculative — needs potato confirmation.) Likely a round "two-thirds-of-SF"
target picked in early ISS-148 planning, before the per-question taxonomy
existed. ISS-138 (DEFAULT_TOP_K=10) and ISS-139 (MMR) were expected to
contribute baseline lift; subsequent levers (HyDE, entity_channel) all
falsified on SF specifically, leaving the ceiling much lower than
originally projected.

## Options

### Option A: Redefine AC-5a to "any major axis ≥+5pp Δ over default"

Track ship-gate quality at the *axis* level, not single-fact specifically.
ISS-175 (multi-hop +18.9pp) and ISS-177 (single-hop +10pp, temporal
+8.1pp) both already demonstrate this is achievable. **Pros**: matches
actual evidence; rewards real lift. **Cons**: weaker contract; "any
axis" gameable if not carefully phrased.

### Option B: Redefine AC-5a to "conv-26 overall ≥0.30" or similar

Shift target axis from single-fact to overall accuracy. ISS-175 Arm B
hits 0.276, ISS-177 Arm B on conv-44 hits 0.285 — within striking distance
with one more lever (ISS-178). **Pros**: less rigid axis-specific
constraint; tracks "user-visible" answer quality. **Cons**: open-domain
and synthesis questions are noisier; harder to attribute regressions.

### Option C: Keep AC-5a, change corpus

Move the SF target to conv-44 or full-LoCoMo. conv-26's question
distribution may not be representative — only 27 SF questions, with
several requiring infrastructure not on the roadmap. **Pros**:
broader signal. **Cons**: more expensive bench; conv-44 SF baseline
unknown.

### Option D: Keep AC-5a unchanged, accept it as long-horizon

Stop treating AC-5a as a near-term blocker; mark it as v0.4 goal.
Continue shipping levers; close ISS-148 when the structural work
(ISS-070, BLIP, aggregation) lands organically. **Pros**: avoids
target gymnastics. **Cons**: removes pressure to ship retrieval lifts.

### Option E: File the missing infrastructure tickets, keep AC-5a

If we genuinely want 17/27 on conv-26, the audit shows we need to
file: ISS-(BLIP), ISS-(aggregation), ISS-(temporal), ISS-(generation),
ISS-(multi-hop). Then chase the full stack. **Pros**: honest about
what 17/27 actually requires. **Cons**: 6-12 months of work.

## Recommendation (not a decision)

**Option A (axis-level)** combined with **Option C (move SF target to
conv-44 or full-LoCoMo)**. The current AC-5a was likely a placeholder
target; both ISS-175 and ISS-177 already cleared corpus-general lift
gates, which suggests the bench should reward what's actually
measurable. The conv-26 SF axis specifically has structural floors
the current lever set can't break through.

## Decision needed

Which option (or combination) does potato want? This issue exists to
surface the ceiling-vs-target gap and have the explicit conversation
before more levers chase a target that may be unreachable. No code
to land on ISS-179 itself; it's a planning artifact.

## Related

- ISS-148: AC-5a parent
- ISS-161 audit: per-question failure-mode taxonomy
- ISS-175 / ISS-177: lever sweeps that prompted this re-examination
- ISS-178: slim prev-turn lever (currently the highest-confidence remaining SF lever)

---

## 2026-05-28 update — ISS-178 falsification tightens the ceiling

ISS-178 (slim prev-turn ExtractionContext) shipped and was **falsified
as ACTIVELY HARMFUL** on conv-26:

- Overall Δ −1.97 pp
- single-hop (n=32) **4 → 2** (Δ −6.25 pp)
- single-fact bucket: q3 (PRIMARY) and q7 (secondary) **both no-flip**
- Regression rate **11.2 %** (AC-4 ≤10 % FAIL)
- Mechanism: slim prev-turn context **prunes** co-occurring entities
  the long-window extractor was keeping → net fact loss

See `.gid/issues/ISS-178/artifacts/falsification-conv26-20260528.md`
and `.gid/issues/ISS-178/issue.md` (status `falsified`).

### Revised lever-by-lever table

| Lever | Status | SF lift on conv-26 |
|---|---|---|
| ISS-175 combine_factual_v2 | shipped opt-in | +0 |
| ~~ISS-178 prev-turn context~~ | **falsified 2026-05-28** | **+0** (and harmful) |
| ISS-159 cross-encoder reranker | **falsified 2026-05-26** | +0 |
| ISS-164 entity_channel | falsified, locked off | +0 |
| ISS-149 force-Factual | de-prioritised — net-negative on conv-26 SH | +0 |
| ISS-070 multi-hop dispatcher | open P0 | +1 (q11) |
| Aggregation/counting | no lever filed | +0 to +3 (q40, q43, q75) |
| BLIP ingestion | infrastructure | +1 (q37) |
| Generation prompt rework | no lever filed | +0 to +1 (q71) |
| Temporal resolver | no lever filed | +0 to +1 (q76) |
| **Stack ceiling (best-case, updated)** | — | **5 + 3..6 = 8-11/27** |

Three independent retrieval-side levers have now falsified on the conv-26
single-fact axis (ISS-159, ISS-164, ISS-178). The remaining roadmap is
all *infrastructure work outside retrieval*: aggregation, BLIP,
generation, temporal, multi-hop graph. None of those are shipped or
budgeted near-term.

Updated best-case stack ceiling = **8-11/27 ≈ 0.30-0.41**, against AC-5a
target 17/27 = 0.63. Gap widened from "tough but tractable" to
"requires multiple unfiled infrastructure projects".

### Strengthened recommendation

**Option A + Option C combined**, now with higher confidence:

1. **Option A (axis-level AC-5a):** "Any major axis Δ ≥+5pp over locked
   default on conv-26 OR overall Δ ≥+3pp on conv-44 secondary corpus."
   Rewards real lift, drops the structurally-unreachable single-fact
   axis as the sole gate.

2. **Option C (move SF target off conv-26):** AC-5a single-fact gate
   moves to conv-44 or full-LoCoMo where the question-distribution
   floor isn't dominated by aggregation/temporal/BLIP cases. conv-26
   single-fact stays as a **diagnostic axis** (tracked but not gated).

Pure Option D ("accept long-horizon, treat as v0.4 goal") is also
defensible if you want to stop conversation-on-target and focus shipping
energy on whichever levers are next-most-EV regardless of AC-5a.

### What I'm NOT recommending

- **Option B (overall ≥0.30 as the SF target):** ISS-177 Arm B already
  hit 0.345 on conv-44 — this would be a near-trivial target on the
  wrong axis, defeats the point of AC-5a.
- **Option E (file all missing infrastructure tickets):** 6-12 months
  of pre-committed work without proof any individual ticket moves the
  user-facing number. Build evidence first.

### Decision still pending

Need potato to pick A / C / D / combination. No code on ISS-179 until
this is resolved — but the ceiling now sits at ~0.30-0.41 best-case
on conv-26 SF, and three retrieval-side levers in a row have failed,
so the question is no longer "can we hit 0.63" but "do we keep targeting
0.63 at all on this corpus."
