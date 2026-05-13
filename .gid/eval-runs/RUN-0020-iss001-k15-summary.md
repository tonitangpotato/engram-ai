# RUN-0020 — ISS-001 K-sweep (K=15 vs K=5)

**Date:** 2026-05-05 22:22 EDT
**Build:** engram @ c8c8fa9
**Driver:** engram-bench `cargo run --release --bin engram-bench -- locomo` (env `ENGRAM_BENCH_TOP_K=15`)
**Substrate:** RUN-0018 DB (ISS-103 occurred_at fix applied)
**Conversation:** conv-26, 152 queries
**Metric:** **J-score** (LLM-as-judge, claude-sonnet-4-5-20250929, binary 0/1)

## Hypothesis

ISS-001 single-hop failures triaged as ~mode-A (列举型, ~11/24) + mode-B (entity-miss, ~13/24). K=5 → K=15 should:

- ✅ improve multi-hop (more candidates → better reasoning chain)
- ✅ partially improve single-hop mode-A (列举更全)
- ❌ NOT improve single-hop mode-B (candidate set still missing the needle)

## Results vs RUN-0019 (K=5, same DB, same build)

| Category    | K=5 (RUN-0019) | K=15 (RUN-0020) | Δ          |
|-------------|----------------|------------------|------------|
| **overall**     | 0.421 (64/152) | **0.467** (71/152) | **+4.6pp** |
| multi-hop   | 0.595 (22/37)  | **0.703** (26/37)  | **+10.8pp** |
| temporal    | 0.457 (16/35)  | **0.514** (18/35)  | +5.7pp     |
| single-hop  | 0.156 (10/64)  | **0.188** (12/64)  | +3.1pp     |
| open-domain | 0.385 (5/13)   | **0.231** (3/13)   | **−15.4pp** ⚠️ |

## Per-question deltas

- **Multi-hop new wins (5):** q8, q31, q45, q49, q73 — all temporal-grounding-heavy ("the week before...", year/month answers). Confirms K-expansion helps when answer requires assembling multiple fragments.
- **Single-hop new wins (2 net):** small movement, hypothesis confirmed.
- **Open-domain regressions (2):** q14, q27 — both flipped 1→0. Inspection:
  - q14 gold "Likely no". K=5 confidently extrapolated → correct by luck. K=15 said "I don't know" because expanded context contained more conflicting signals.
  - q27 same pattern. Generator hedged with more context.

**Open-domain regression is judge/generator behavior, not retrieval failure.** With 15 candidates, the generator becomes more cautious ("I don't know"); judge marks "I don't know" as wrong even when gold is "Likely no/yes". Could be inflated 5/13 baseline (lucky guesses) being corrected, not true regression.

## Hypothesis Verdict

| Prediction | Result | Verdict |
|---|---|---|
| Multi-hop big improvement | +10.8pp | ✅ confirmed |
| Single-hop small improvement | +3.1pp | ✅ confirmed (mode-B dominates as predicted) |
| Single-hop mode-B unchanged | yes | ✅ confirmed by stagnation |
| Open-domain stable/improve | −15.4pp | ❌ unanticipated regression |

ISS-001 entity-miss diagnosis (mode-B ~13/24 single-hop fails) **stands** — K-expansion didn't move the needle there, as predicted.

## Standard / non-standard

**Non-standard run.** `DEFAULT_TOP_K = 5` is the committed value (`engram-bench/src/drivers/locomo.rs:489`); the source comment explicitly says `ENGRAM_BENCH_TOP_K` override is "not for committed runs — only used when triaging which subsystem is the bottleneck."

K=5 is the conventional benchmark setting for comparable systems (Mem0, Graphiti, etc. typically report at K=5). RUN-0019 (K=5) remains the canonical reportable number. RUN-0020 is **diagnostic only**.

## Decision

- **Don't change `DEFAULT_TOP_K`.** Reportable number is RUN-0019 K=5 = 42.1%.
- **Open ISS for K=15 open-domain regression** — unexpected, deserves separate investigation (generator/judge behavior under long context, not retrieval).
- **Confirm ISS-001 entity-miss is the next lever.** Single-hop mode-B can't be solved by K-expansion → needs better retrieval (entity grounding, query rewriting, or graph-based fetch).

## Files

- Per-query results: `engram-bench/benchmarks/runs/2026-05-06T02-22-59Z_locomo/locomo_per_query.jsonl`
- Summary JSON: `engram-bench/benchmarks/runs/2026-05-06T02-22-59Z_locomo/locomo_summary.json`
- Run logs: `engram-bench/.gid/eval-runs/RUN-0020.{stdout,stderr}`
- K=5 baseline: RUN-0019 (`2026-05-05T21-27-39Z_locomo`)

## P0 Gate Status (informational)

- GOAL-5.1 LOCOMO ≥ 0.685: 0.467 — **FAIL** (still 22pp below target)
- GOAL-5.2 baseline data: ERROR (Graphiti baseline data not loaded)
