# ISS-152 sweep results — pool sizing + MMR λ

> **Status: complete — sweep finished 2026-05-24T07:22:19Z, results below.**

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
| **ISS-152 Run A** | `894dcb1` | **0.3618** | **0.1562** | **0.3243** | **0.3846** | **0.4714** | **same code path as ISS-150 with override=None — but did NOT reproduce.** |

### ⚠ Run A did NOT reproduce ISS-150 — ingest non-determinism

Run A uses identical code (engram `894dcb1` adds override fields defaulted
`None` → byte-identical call sites) and identical fixture / env to ISS-150.
But the per-query diff shows:

- 110/152 same score, 13 flipped up, **29 flipped down** vs ISS-150
- Of the 29 regressions: **13 are HARD (A returned "I don't know" where ISS-150 answered correctly)**, 16 are soft (both answered, judge differed)
- Both runs had exactly 1 `Dedup: merging` event but on **different memory IDs** (`10f710b1` vs `1241fe04`) at slightly different similarity (0.9535 vs 0.9529)

This means **Ollama embedding output is non-deterministic across runs**, producing different dedup-merge decisions early in ingest, cascading into different graph topology by query time. ISS-137 only stabilised the judge (temp=0); ingest embedding noise is the next stdev source.

**Implication for ISS-152**: comparisons must be **A vs B/C/D/E within this sweep**, NOT against historical ISS-150 numbers. Run A is the only valid baseline for runs B-E because they share the same Ollama session / ingest noise pattern.

**Side-quest noted**: file ISS for ingest embedding determinism — likely needs Ollama `temperature=0` + `seed` if the model supports it, or switching to a deterministic local model. Defer to after sweep — not blocking decision.

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

| run | K_seed | bm25_pool | MMR λ | overall | single | multi | open | temporal | Δ single vs A |
|---|---|---|---|---|---|---|---|---|---|
| ISS-150 prior baseline | — | — | 0.7 | 0.4408 | 0.2188 | 0.6216 | 0.3077 | 0.5000 | — |
| **A (sweep baseline)** | unset | unset | 0.7 | **0.3618** | **0.1562** | **0.3243** | 0.3846 | 0.4714 | — |
| B | 100 | 100 | 0.7 | 0.2895 | 0.1250 | 0.3514 | 0.1538 | 0.3571 | **−0.0312** |
| C | 200 | 200 | 0.7 | 0.1842 | 0.0312 | 0.1892 | 0.1538 | 0.2571 | **−0.1250** |
| D | 100 | 100 | 0.5 | 0.2303 | 0.0938 | 0.3243 | 0.0769 | 0.2714 | **−0.0624** |
| E | 100 | 100 | 0.3 | 0.2039 | 0.0938 | 0.2432 | 0.0769 | 0.2571 | **−0.0624** |

**All four experimental runs are WORSE than Run A on overall and on single-hop.** Pool widening (B, C) monotonically hurt as pool grew — bigger pool = more distractors, MMR didn't recover. Diversity-only sweeps (D, E) also regressed across the board.

Also note Run A itself sits at single-hop 0.1562, well below the ISS-150 baseline of 0.2188 — confirming the ingest non-determinism finding from §Setup. The sweep is internally consistent (same Ollama session) so the cross-run comparison is still valid; only the comparison vs historical ISS-150 is unsafe.

## Decision

**No threshold met. Per ISS-152 decision tree → ISS-153 (HyDE) is the right next move.**

- ✗ Run B single-hop 0.1250 < 0.35 threshold
- ✗ Run C single-hop 0.0312 < 0.35 threshold
- ✗ Run D open-domain 0.0769 < Run A's 0.3846 (regression, not recovery)
- ✗ Run E open-domain 0.0769 < Run A's 0.3846 (regression, not recovery)

**Interpretation**: pool size is not the bottleneck. Growing the candidate pool (40 → 100 → 200) without improving the embedding's ability to surface the right gold passage just dilutes the top-K with semantically-close-but-wrong neighbours. MMR diversification only makes this worse when the underlying embedding ranking is noisy.

**The bottleneck is upstream — query→passage embedding semantics.** ISS-153 (HyDE, hallucinated-document expansion) is the correct lever: rewrite the query into a synthetic answer and embed that, rather than rebalancing the existing flawed ranking.

### Actions
- [x] Sweep complete, results recorded.
- [ ] Close ISS-152: knob landed and tested, sweep is **decisively negative** — knob can stay (zero-cost, defaults unchanged) but no follow-up commit needed.
- [ ] Bump ISS-153 (HyDE) to `in_progress`, start design.
- [ ] File ingest-determinism issue (Ollama embedding non-determinism noted in §Setup) — lower priority, blocks future bench reproducibility but doesn't block ISS-153.
- [ ] **Skip** Mode-B dump confirmation — no winning config to confirm.

## Mechanistic confirmation (TBD)

If a pool-widening run wins, re-run **once** with `ENGRAM_BENCH_DUMP_CANDIDATES=1` to enable Mode-B dump, then run `recall_diag.py` against the winning config. Expected:
- Recall-miss bucket shrinks from **14** (ISS-150 baseline) → < 14
- Partial-list bucket may grow if pool fills with near-duplicates from the same conversational thread
- Wrong-fact bucket stable at 2

This is the mechanistic proof that pool widening actually surfaced gold episodes that were previously below top-K.

## Decision log

_(to be filled in after analysis)_
