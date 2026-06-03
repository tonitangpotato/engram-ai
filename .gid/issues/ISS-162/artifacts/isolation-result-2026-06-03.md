# ISS-162/178 confound isolation — windowing is a REAL portable lever

**Date:** 2026-06-03
**Verdict:** ISS-178 falsification was a CONFOUND, not a refutation of windowing.
**Sweeps:** ISS-201 A/B (STAMP 20260603T141337Z) + ISS-162 C/D (STAMP 20260603T171256Z)

## Full matrix (conv-26, same binary)

| arm | window | envelope | overall | single-hop | open-domain | multi-hop | SEM-GAP |
|---|---|---|---|---|---|---|---|
| A | 0 | mine (REWEIGHT off, RES=5, SURFACE on) | 0.2697 | 0.031 | 0.077 | 0.297 | 18 |
| B | 4 | mine | **0.3882** | 0.156 | 0.308 | 0.378 | 13 |
| C | 1 | mine | 0.3421 | 0.125 | 0.231 | 0.351 | 17 |
| D | 4 | ISS-178 (REWEIGHT on, no RES) | 0.3421 | **0.219** | 0.385 | 0.189 | **11** |

## Confound 1 — window size (C vs B, envelope held = mine)

- window=1 → 0.3421 (+7.2pp). window=4 → 0.3882 (+11.85pp).
- **Both lift; N=4 strictly better (+4.6pp).** window=1 is NOT harmful under
  this envelope — it helps. ISS-178's −1.97pp is therefore NOT explained by
  "1 turn is too few"; it points to the envelope.

## Confound 2 — envelope (D vs B, window held = 4)

- Under ISS-178's `FACTUAL_REWEIGHT=on` envelope, window=4 STILL lifts +7.2pp,
  and gives the **best single-hop (0.219) and lowest SEMANTIC-GAP (11)** of all
  four arms.
- **The windowing lift survives REWEIGHT=on.** Not an artifact of REWEIGHT-off.

## Why ISS-178 falsified

ISS-178 = window **1** × REWEIGHT **on** — the weakest window combined with the
envelope where `combine_factual` reranking dominates. The small window=1 signal
was masked/overridden by REWEIGHT's own candidate reordering, netting −1.97pp.
The windowing MECHANISM was never the problem; the falsification measured the
worst corner of the matrix.

## Decision

- **Build ISS-162 in the engramai library.** Lever is real and portable.
- Window default **N=4**.
- ISS-162 reprioritised P3 → P1. Downgrade reason cleared.
