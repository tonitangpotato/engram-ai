---
title: MMR default λ=1.0 → 0.7 — ISS-139 ship completion (L1+MMR validation)
status: resolved
priority: P1
severity: enhancement
category: retrieval
created: 2026-05-24
resolved: 2026-05-24
fixed_by: [b16b243]
related: [ISS-139, ISS-143, ISS-144]
---

## Summary

ISS-139 shipped the MMR reranker code but kept the default at λ=1.0
(MMR off / byte-identical to pre-ISS-139). That was the right ship
gate for the mechanism — but it left the parameter unvalidated, so
production / benchmarks were running the no-op default. This issue
closes the loop: validate that MMR with a non-trivial λ actually
helps LoCoMo, pick a default, and flip.

## Result

Flipped `FusionConfig::default_mmr_lambda()` from `1.0` → `0.7`.
Commit: `b16b243`.

## Evidence

LoCoMo conv-26 (152q, K=10, temp=0), full sweep this session:

| config                           | overall | single-hop | multi-hop | temporal | open  |
|----------------------------------|--------:|-----------:|----------:|---------:|------:|
| baseline (no L1, no MMR)         |  0.3947 |     0.0625 |    0.5135 |   0.4857 | 0.3846 |
| L1 only                          |  0.4408 |     0.1562 |    0.6216 |   0.5000 | 0.3077 |
| MMR λ=0.7 only (no L1)           |  0.4474 |     0.1562 |    0.6216 |   0.5000 | 0.3846 |
| MMR λ=0.7 smoke (no L1)          |  0.4605 |     0.2188 |    0.5405 |   0.5429 | 0.3846 |
| **L1 + MMR λ=0.7**               |  **0.4671** | **0.2188** | 0.5676 | 0.5429 | 0.3846 |

ISS-137 measured run-to-run stdev = 0.66pp at temp=0, so all deltas
≥ +3.95pp are clearly above noise. The +7.24pp overall gain is 11×
that floor.

L1 and MMR are partly additive: each alone adds ~5pp overall, combined
adds +7.24pp. They target different failure modes (L1 = dedup/entity
recall, MMR = list-style cluster-collapse), so partial additivity is
expected.

## Why 0.7 specifically

ISS-143 already swept λ ∈ {0.3, 0.5, 0.7, 0.9, 1.0} on conv-26 25q
smoke (memory note `iss139_sweep`). λ=0.7 was the sweet spot:

- Strong enough to break same-cluster repetition (single-hop wins)
- Conservative enough to preserve top-1 relevance
- Matches Carbonell-Goldstein original MMR paper default

## Rollback

Callers needing legacy no-op behaviour pass
`GraphQuery::with_mmr_lambda(Some(1.0))`. The env-override path
(`ENGRAM_BENCH_MMR_LAMBDA=1.0`) in engram-bench still works.

## What this is NOT

This issue is **the easy single-toggle win**. It does NOT address the
deeper retrieval failures discovered during the same diagnostic
session — embedding paraphrase failures (q11 "Sweden" ranks 319/419
on pure cosine) and BM25 channel dead code. Those are tracked in
ISS-147.

## Run artifacts (disk)

- `engram-bench/benchmarks/runs/ISS137-temp0-run3-20260523T040751Z/` — baseline
- `engram-bench/benchmarks/runs/ISS144-L1-only-20260524T000937Z/` — L1 only
- `engram-bench/benchmarks/runs/ISS139-mmr-l0.7-20260523T204431Z/` — MMR only (full)
- `engram-bench/benchmarks/runs/ISS143-smoke-conv26-k10-mmr0.7-20260523T214232Z/` — MMR smoke
- `engram-bench/benchmarks/runs/ISS146-L1+MMR-l0.7-20260524T023928Z/` — combined
