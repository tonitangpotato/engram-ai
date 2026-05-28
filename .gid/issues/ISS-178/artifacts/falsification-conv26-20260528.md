# ISS-178 falsification — slim prev-turn ExtractionContext is HARMFUL on conv-26

**Date:** 2026-05-28
**Verdict:** ACTIVELY HARMFUL → all 5 implementing commits reverted
**Sweep STAMP:** `20260528T204811Z`
**Envelope:** conv-26, K=10, temp=0, HyDE=off, MMR=off, entity_channel=off, FACTUAL_REWEIGHT=on, pipeline_pool=1 (ISS-177 canonical)

## Arms

| Arm | `ENGRAM_BENCH_PREV_TURN_CONTEXT` | Output dir |
|---|---|---|
| A (baseline) | off | `benchmarks/runs/ISS178-A-conv26-20260528T204811Z` |
| B (lever)    | on  | `benchmarks/runs/ISS178-B-conv26-20260528T204811Z` |

## Headline numbers (Arm B − Arm A)

| Bucket | A | B | Δ |
|---|---|---|---|
| **Overall (n=152)** | 0.2961 | 0.2763 | **−1.97 pp** |
| multi-hop (n=37)    | 0.2973 | 0.2973 |  +0.00 pp  |
| **open-domain (n=13)** | 0.4615 | 0.3077 | **−15.38 pp** |
| **single-hop (n=32)**  | 0.1250 | 0.0625 |  **−6.25 pp** (4/32 → 2/32) |
| temporal (n=70)     | 0.3429 | 0.3571 |  +1.43 pp  |

## Primary / secondary targets

- **conv-26-q3 (PRIMARY):** 0 → 0 — no flip. Both arms return "I don't know."
- **conv-26-q7 (secondary):** 0 → 0 — no flip. Both arms return "I don't know."

The fix-surface census (ISS-179) identified q3 (+ maybe q7) as the highest-confidence
recovery target for the slim prev-turn lever. The lever fails to recover **either**.

## Flip ledger

- B gains: **14** queries
- B regressions: **17** queries
- Ties: 121
- **Regression rate: 11.2 %** (AC-4 guard ≤ 10 % → FAIL)

### Per-category flips

| Category | Gains | Regressions | Ties |
|---|---|---|---|
| multi-hop   | +5 | −5 | 27 |
| open-domain | +1 | −3 |  9 |
| single-hop  | +1 | −3 | 28 |
| temporal    | +7 | −6 | 57 |

## Are the regressions real or judge noise?

Real. Sample of single-hop A=1 → B=0 cases shows Arm B literally returned
**less complete answers** because the slim prev-turn context pruned facts the
extractor needed to keep:

- **conv-26-q15** (Melanie's hobbies, gold = "pottery, camping, painting, swimming")
  - A: "Reading / Painting / Camping … " (full list)
  - B: "Melanie does pottery and plays clarinet." (incomplete — camping/painting/swimming dropped)
- **conv-26-q13** (Caroline's counseling focus, gold = "Transgender people")
  - A: "exploring counseling … to work with trans people"
  - B: "decided to pursue a career in counseling and mental health" (trans-specificity dropped)
- **conv-26-q39** (Caroline's LGBTQ activism list)
  - A: "Speaking up for the trans community and advocating …"
  - B: "Giving talks at schools about her transgender …" (different subset, partial)

Open-domain regressions show the same pattern — Arm B falls back to "I don't know"
where Arm A had the supporting episode (q22 children's-book library, q42
Melanie's nature/national-park preference).

This is consistent with the mechanism: a slim prev-turn-only context **discards**
co-occurring entities that the long-window extractor would otherwise have linked
into the relation. Net effect on conv-26 is fact loss, not denoising.

## Decision rule application

From the compaction header / iss178_analyse.py:

- Δsh ≥ +1 AND q3 flipped AND reg ≤ 10 % → STRONG KEEP
- Δsh ≥ +1 AND reg ≤ 10 %               → KEEP
- q3 flipped AND Δsh = 0                 → MARGINAL KEEP
- Δsh = 0                                → FALSIFIED
- **Δsh < 0                              → ACTIVELY HARMFUL** ← we are here

Δsh = **−2** (4/32 → 2/32). Verdict: **revert**.

## Reverts (in order applied)

In `engram` (`/Users/potato/clawd/projects/engram`):

| Reverted commit | Subject | Revert commit |
|---|---|---|
| `5352739` | ISS-178 AC ticks + status flip | `0123b1c` |
| `989025c` | Step 4 StorageMeta + ingest_with_meta | `76faa8c` |
| `670bc41` | Steps 2+3 backend overrides | `aff3868` |
| `fdac0a4` | Step 1 ExtractionContext type + trait | `645be52` |

In `engram-bench` (`/Users/potato/clawd/projects/engram-bench`):

| Reverted commit | Subject | Revert commit |
|---|---|---|
| `cf6e859` | Step 5 LoCoMo driver wiring | `ac193ca` |

Post-revert tree builds clean (`cargo build --release -p engramai` OK).

## Implication for ISS-162 and ISS-179

- **ISS-162** (semantic UPDATE phase for cosine `[0.80, 0.95)` band, LLM ADD/UPDATE/DELETE/NOOP)
  was framed in part as "needs prev-turn context to disambiguate UPDATE vs NOOP."
  ISS-178's falsification removes that justification for the slim variant.
  ISS-162 is **downgraded to P3** until a different context source (e.g. a small
  retrieval pre-pass over the current namespace) is proposed and shown not to
  prune useful facts.
- **ISS-179** (AC-5a target redefine, Options A/B/C/D) is still blocked on potato's
  decision — but the empirical finding here strengthens the case for
  **Option C** (move SF target off conv-26 entirely) because none of the
  context-injection levers tested so far (ISS-164 entity_channel, ISS-178
  prev_turn) move the conv-26 single-fact bucket.

## Disk artifacts

- Arm A: `benchmarks/runs/ISS178-A-conv26-20260528T204811Z/` (intact)
- Arm B: `benchmarks/runs/ISS178-B-conv26-20260528T204811Z/`
- Analysis script: `/tmp/iss178_analyse.py`
- Original sweep script: `/tmp/iss178_bench_sweep.sh` (Arm A) + `/tmp/iss178_arm_b_rerun.sh` (Arm B re-run after OAuth expiry)
- Bench logs: `/tmp/iss178-bench/iss178-A.log`, `/tmp/iss178-bench/iss178-B-rerun.log`
