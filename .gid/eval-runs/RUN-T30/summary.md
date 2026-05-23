# RUN-T30 — Phase D probe-parity dry run

**Date**: 2026-05-22
**Driver**: `crates/engramai/examples/t30_probe_parity.rs`
**Backfill runner**: `crates/engramai/examples/t30_phase_d_backfill_runner.rs`
**Source DB**: snapshot of `/Users/potato/rustclaw/engram-memory.db` (12,756 active memories, 2,791 entities, 0 topics) → `/tmp/t30-probe.db`

## Backfill pre-step

All 8 Phase C drivers ran cleanly on the snapshot (5.29s total, 0 rows_failed):

| Driver | read | inserted | skipped | failed |
|---|---:|---:|---:|---:|
| T19 memories→nodes | 19,387 | 18,882 | 505 | 0 |
| T20 embeddings→node_embeddings | 19,015 | 18,864 | 151 | 0 |
| T21 entities→nodes | 2,791 | 2,478 | 313 | 0 |
| T22 entity_relations→edges | 8,533 | 8,533 | 0 | 0 |
| T23 memory_entities→edges | 10,193 | 9,702 | 491 | 0 |
| T24 hebbian_links→edges | 43,224 | 43,110 | 114 | 0 |
| T25 synthesis_provenance→edges | 928 | 901 | 27 | 0 |
| T26 soft_delete projection | 6,631 | 6,630 | 1 | 0 |

Post-backfill: 22,174 nodes, 62,771 edges, 19,011 node_embeddings.

`skipped_existing` > 0 across the board is **expected** — Phase B dual-write has been live, so many of these projections already existed from runtime writes.

## Probe-parity results

50 queries (20 broad-topic + 30 production-specific from entity-frequency table), per-query Jaccard@K between `unified_substrate=false` and `unified_substrate=true` arms.

| top-K | queries_passing (jac ≥ 0.95) | parity_ratio | gate (≥ 0.95) |
|---|---:|---:|---|
| 3 | 32 / 50 | 0.6400 | **FAIL** |
| 5 | 29 / 50 | 0.5800 | **FAIL** |
| 10 | 20 / 50 | 0.4000 | **FAIL** (design spec) |

At a much weaker threshold (Jaccard ≥ 0.5, i.e. set-majority agreement):

| top-K | queries_passing | parity_ratio |
|---|---:|---:|
| 10 | 49 / 50 | 0.98 |

## Interpretation

The dominant divergence pattern is **9-of-11 shared, 1-different-each-side** (Jaccard = 0.818). 24 of 50 queries land exactly there. This means: top-K result *sets* are largely the same, but the **last-rank candidate differs systematically**. Worst cases (Jaccard 0.333–0.538) are still set-majority overlap with 3–5 swaps, not catastrophic divergence.

Concrete example — query `"session compaction"`:
- legacy top-10: `55db8d5d, 9f72da84, f95e27e1, 468727f6, 997a7903, 4ab519b2, 1c6ceae5, ce3ce403, 62314124, f9a341e0`
- unified top-10: `55db8d5d, 9f72da84, f95e27e1, 468727f6, 997a7903, 85405b0c, 4ab519b2, a11222f8, 061b4fca, 62314124`

First 5 ranks **byte-identical, in order**. Divergence starts at rank 6 and 3 candidates differ at the tail.

## Decision

**Do NOT flip `unified_substrate` default to true on this evidence.**

Cause is **not** missing data — backfill is clean, both arms see the same underlying corpus. Cause **is** a ranker / scoring divergence in the read-side adapters (T29.1–T29.6) that shows up at the tail of the candidate list.

## Follow-up (file as ISS-NNN, separate work unit)

1. Diagnose tail-rank divergence: take 3 worst queries, log the candidate-score breakdown from both arms, find where scores diverge (likely fusion weights, source-of-truth ordering for tied scores, or FTS rank tiebreaker).
2. Once that's fixed, re-run T30 against the same snapshot. Target Recall@10 ≥ 0.95.
3. Then run T31 LoCoMo + probe set side-by-side. ISS-111 real-embedding verification rides on T31, not T30.

## Artifacts (this run)

- `t30-backfill.log` — backfill summary stdout
- `t30-probe-parity-k10.json` — full per-query report (Recall@10)
- `t30-probe-parity-k5.json`, `t30-probe-parity-k3.json` — sensitivity analysis

## Reproducibility

```bash
# Snapshot
cp /Users/potato/rustclaw/engram-memory.db /tmp/t30-probe.db

# Phase C backfill (one-shot, ~5s on 12k memories)
cargo run --release -p engramai --example t30_phase_d_backfill_runner -- \
  --source /tmp/t30-probe.db

# Probe parity (~8s)
cargo run --release -p engramai --example t30_probe_parity -- \
  --source /tmp/t30-probe.db \
  --top-k 10 \
  --out /tmp/t30-probe-parity.json
```
