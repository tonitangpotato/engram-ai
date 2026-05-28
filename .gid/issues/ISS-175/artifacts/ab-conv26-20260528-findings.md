# ISS-175 A/B verdict — conv-26 K=10 temp=0

**Date**: 2026-05-28 STAMP=20260528T034409Z
**Sweep**: `/tmp/iss175_bench_sweep.sh` (engram-bench b51ee58 / engram da11171)
**Envelope**: K=10, temp=0, HyDE=off, MMR off (λ=1.0), entity_channel=off,
pipeline_pool=1, conv-26 only. Matches ISS-161 Arm A baseline envelope.

## Results

| Metric | Arm A (off) | Arm B (on) | Δ |
|---|---|---|---|
| overall | 0.2171 | 0.2763 | **+5.9pp** |
| multi-hop | 0.1892 (7/37) | 0.3784 (14/37) | **+18.9pp** ← big |
| open-domain | 0.1538 (2/13) | 0.3077 (4/13) | +15.4pp |
| single-hop | 0.0938 (3/32) | 0.0938 (3/32) | **+0.0pp** ← flat |
| temporal | 0.300 (21/70) | 0.300 (21/70) | +0.0pp |

**Caveat — envelope drift vs ISS-161 Arm A.** ISS-161 Arm A reported overall=0.362
single-hop=6/32 on the same nominal envelope (HyDE=per_category vs HyDE=off here).
HyDE was OFF in this sweep (per the ISS-175 ship-gate spec), so the absolute numbers
sit lower than ISS-161's HyDE-pc baseline. The A/B delta is the only number that
matters for the ship gate — both arms ran HyDE=off, so HyDE cannot confound the Δ.

## Single-fact sub-bucket (the AC-5a gate)

**Single-hop totals**: A=3/32 vs B=3/32 — **zero net delta**.

Per-query single-hop flips (only two, and they cancel):
- conv-26-q32 (gold "Pride parade, school speech, support group"): A=0 → B=1 (gain)
- conv-26-q47 (gold "Her mentors, family, and friends"): A=1 → B=0 (loss)

Under either bucketing heuristic (comma-list or strict-list-of-items):
- Comma-heuristic (n=13 SF / n=19 list): SF A=2/13 B=2/13 Δ=0
- Strict-list (n=12 SF / n=20 list): SF A=2/12 B=2/12 Δ=0
- Single-hop raw: A=3/32 B=3/32 Δ=0

The two flipped questions are both list-shaped under any reasonable bucketing.
**The single-fact sub-bucket is unmoved by `combine_factual_v2`.**

## Ship gate verdict — FALSIFIED

ISS-175 ship gate per AC-5a: **B sf ≥ A sf + 2** (baseline 5/27, target ≥7/27).

Measured Δ = 0. **Gate not met. Falsified for the AC-5a target.**

## Unexpected positive — multi-hop +18.9pp

Spot-check confirms multi-hop gains are real retrieval wins, NOT LLM-judge noise:
- q6 ("June 2023" camping date) — A says "I don't know"; B retrieves the correct
  June 2023 camping episode and gets it right.
- q12 ("10 years ago" 18th birthday) — A says "I don't know"; B finds the
  birthday-bowl memory with the right date.
- q53 ("week of 23 August 2023" adoption) — A says "I don't know"; B retrieves
  the 2023-08-21 adoption-application episode.

The pattern: A's top-10 is dominated by vague/emotional memories; B's
sum-with-evidence-bonus text aggregate elevates bm25-rich memories with named
entities and dates. Multi-hop questions that require date-bearing factoids
benefit; single-fact questions don't, because the SF failures aren't about
top-K composition — they're already saturated (see ISS-175 probe Bug 1:
graph_score saturation on q43-shape).

Regressions (n=4) follow the inverse pattern: where the right episode is
vague-emotional (q47 "Her mentors, family, and friends"; q150 "She appreciated
them a lot"), B's bm25-bias bumps it out of top-10.

## Total flip ledger

| Category | gains (A=0→B=1) | regressions (A=1→B=0) | net |
|---|---|---|---|
| multi-hop | 8 | 1 | +7 |
| open-domain | 2 | 0 | +2 |
| single-hop | 1 | 1 | 0 |
| temporal | 2 | 2 | 0 |
| **total** | **13** | **4** | **+9** |

135/152 ties = 88.8% stability. Verdict signal-to-noise is high.

## Recommendation

1. **Do NOT flip `factual_reweight` default.** Ship gate not met on the
   AC-5a target (single-fact).
2. **Keep the flag and `combine_factual_v2` code** (commit da11171) on
   `main` as opt-in. It's a real multi-hop/open-domain lift, but the
   trade-off (-4 in vague-emotional questions) means it's not a free win.
3. **Re-bench under HyDE=per_category** to check whether the lift stacks
   with HyDE before stacking with ISS-164 entity-channel or pursuing
   ISS-148 AC-5a recovery via a different lever.
4. **File follow-up**: ISS-175's combine_factual_v2 surfaces a real
   "evidence vs emotion" axis in fusion. The right next move is not flag
   tuning but investigating whether multi-hop date-bearing questions are
   in scope for AC-5a-adjacent goals (probably not — AC-5a is strictly
   single-fact on conv-26).
5. **Probe was right, gate was wrong**: the probe predicted q40/q43/q71
   would narrow gap but stay outside top-10. Result: q40/q43/q71 all
   stayed misses in B. The probe accurately diagnosed that the
   `combine_factual_v2` formula moves the right knobs for evidence-rich
   queries but doesn't address the SF failures (which are saturation or
   missing-from-pool, not ranking).

## Decision pending

- Keep ISS-175 as opt-in on main, file ISS-176 for the
  "vague-emotional regression" trade-off, mark ISS-175 falsified-on-AC-5a
  but kept-as-opt-in?
- Or merge into a single ISS-175 "shipped as opt-in, AC-5a falsified"
  closure?

