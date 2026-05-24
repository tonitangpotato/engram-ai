# ISS-152 sweep results — pool sizing + MMR λ

> **Status: draft skeleton — numbers populated once `/tmp/iss152_sweep.sh` (PID 93487) completes.**

## Setup

- Started: 2026-05-24T04:58:18Z
- Engram commit: `894dcb1` (`api.rs` + `orchestrator.rs` — 2 new GraphQuery override fields, 5 wire sites)
- Engram-bench commit: `df3c8d1` (`drivers/locomo.rs` — `ENGRAM_BENCH_K_SEED` + `ENGRAM_BENCH_BM25_POOL` env vars)
- Fixture: conv-26 (152 questions, K=10, temp=0 via ISS-137 default)
- Each run: ~12 min wall-clock (single conv, OAuth Haiku per-query judge)
- Output dirs: `/Users/potato/clawd/projects/engram-bench/benchmarks/runs/ISS152-{A,B,C,D,E}-*`

## Baselines (immediate predecessors)

| label | commit | overall | single | multi | open | temporal | notes |
|---|---|---|---|---|---|---|---|
| ISS-144 L1-only | `7eee30e` | 0.4408 | 0.1562 | 0.6216 | 0.3077 | 0.5000 | conv-26 |
| ISS-147 BM25 | `5ed5dc0` | 0.4671 | 0.2188 | 0.5946 | 0.3846 | 0.5286 | conv-26, +BM25 fusion |
| ISS-150 BM25-Assoc | `3253d49` | 0.4408 | 0.2188 | 0.6216 | 0.3077 | 0.5000 | conv-26, +Associative BM25 (no judge-score movement) |

**ISS-148 AC-5 target: single-hop ≥ 0.40**

## Grid

| run | K_seed | bm25_pool | MMR λ | hypothesis under test |
|---|---|---|---|---|
| A | unset (= K=10 default) | unset (= 40 default) | 0.7 | baseline reproduce of ISS-150 — same code path, same defaults |
| B | 100 | 100 | 0.7 | "pool too narrow" — recall-miss bucket shrinks |
| C | 200 | 200 | 0.7 | upper-bound pool effect |
| D | 100 | 100 | 0.5 | pool + diversity combined |
| E | 100 | 100 | 0.3 | aggressive diversity (list-question hot fix) |

## Results

> _Populated by `/tmp/iss152_compare.py` after sweep completes._

| run | overall | single | multi | open | temporal | Δ single vs A |
|---|---|---|---|---|---|---|
| A | — | — | — | — | — | — |
| B | — | — | — | — | — | — |
| C | — | — | — | — | — | — |
| D | — | — | — | — | — | — |
| E | — | — | — | — | — | — |

## Decision

> _Filled in after results populate. Decision tree from ISS-152 issue body:_

- **If B or C single-hop ≥ 0.35:** commit wider pool default, deprioritise ISS-153.
- **If D or E recovers open-domain coverage:** commit lower MMR λ, file ISS-154 anyway.
- **If nothing moves:** ISS-153 (HyDE) is the right next move.

## Mechanistic confirmation (TBD)

If a pool-widening run wins, re-run **once** with `ENGRAM_BENCH_DUMP_CANDIDATES=1` to enable Mode-B dump, then run `recall_diag.py` against the winning config. Expected:
- Recall-miss bucket shrinks from **14** (ISS-150 baseline) → < 14
- Partial-list bucket may grow if pool fills with near-duplicates from the same conversational thread
- Wrong-fact bucket stable at 2

This is the mechanistic proof that pool widening actually surfaced gold episodes that were previously below top-K.

## Decision log

_(to be filled in after analysis)_
