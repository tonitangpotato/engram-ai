---
title: 'LoCoMo overall accuracy regression on master (legacy reads): 0.467 → 0.395'
status: open
priority: P1
severity: degradation
tags: [locomo, regression, eval, retrieval]
relates_to: [ISS-001, ISS-106, ISS-111]
---

# LoCoMo overall accuracy regression on master (legacy reads)

## Summary

LoCoMo overall J-score on the legacy read path (memories + memories_fts,
unified_substrate=false) has dropped from ~0.467 (RUN-0027, 2026-05-06)
to 0.3947 (RUN-T31 legacy arm, 2026-05-23) on the same 152-query conv-26
benchmark. This is a master-side regression that predates the engram v04
T29.* unified-substrate read switches — the legacy arm uses the
unchanged read path.

Discovered while running T31 (unified-vs-legacy parity for Phase D).
Unified arm scored 0.4013 — within noise of legacy and ahead by +0.66pp
overall — so unified-substrate is NOT the cause; both arms regressed
together. Tracked separately so it does not block T32 (flip
unified_substrate default to true).

## Evidence

- RUN-0027 (2026-05-06): legacy reads, J-score 0.467
  - source: memory.md note `run0027_pid` and historical context
- RUN-T31 legacy (2026-05-23): legacy reads, J-score 0.3947
  - file: `.gid/eval-runs/RUN-T31/legacy-summary.json`
  - per-category:
    - multi-hop:    0.541
    - temporal:     0.486
    - single-hop:   0.125
    - open-domain:  0.154

For comparison RUN-T31 unified arm scored 0.4013, with:
- multi-hop:    0.595
- temporal:     0.486
- single-hop:   0.0625
- open-domain:  0.231

Net delta of legacy 0.467 → 0.3947 = **-7.2pp** absolute, well above
LLM-judge noise floor (T31 flip analysis shows ~5-9 query flips on
near-identical predictions = ~3-6pp noise on n=152).

## Hypotheses (unverified)

1. **Read path change post-RUN-0027**: even though T29.* read switches
   are flag-gated and not active in legacy arm, refactors in
   `memory.rs` / `retrieval/` since 2026-05-06 may have changed the
   legacy default path (e.g. helper extraction, ranker tweaks, default
   parameter changes).
2. **engram-bench harness change**: the engram-bench commit
   `c8c8fa9 Import engram-bench from engram@c3edd2c` (import from
   monorepo) may have shipped a slightly different LoCoMo harness than
   RUN-0027 was run against. Generator / judge model / prompt drift
   would also count.
3. **Compile_knowledge / KC clusterer state**: ISS-106 was reverted
   2026-05-06 (compile_knowledge call in locomo.rs removed). If
   anything KC-side regressed independently it would land here.
4. **LLM-judge / answer-generator API drift**: Anthropic model
   responses may have shifted since 2026-05-06 even with fixed model id.

## Reproduction

```bash
cd engram-bench
unset ENGRAM_BENCH_UNIFIED_SUBSTRATE
./target/release/engram-bench --output-dir /tmp/repro locomo --format json
# expect overall ≈ 0.395 (currently)
```

## Out of scope

- T32 (flip unified_substrate default) — not blocked, since unified ≥
  legacy on this regressed baseline.
- ISS-001 (single-hop entity-miss) — pre-existing, separate root cause.
- ISS-111 (KC clusterer collapse) — separate, but may interact if
  hypothesis #3 holds.
- ISS-106 root cause — separate; this issue is about regression on the
  reverted-state master, not the ISS-106 patch behaviour.

## Acceptance

- [ ] Bisect engram + engram-bench between RUN-0027 (2026-05-06) and
      RUN-T31 (2026-05-23) to identify the commit that dropped legacy
      overall J-score.
- [ ] Either restore prior behaviour or document the trade-off and
      update the P0 LoCoMo gate threshold accordingly.
- [ ] Confirm the fix on a fresh 152-query legacy LoCoMo run.

## References

- `.gid/eval-runs/RUN-T31/summary.md` — full T31 writeup
- `engram-bench/benchmarks/runs/T31-legacy-20260523T010504Z/`
- engram-bench commit `82e26d6` — harness change shipped in T31 timeframe
- engram commit `270fef4` — RUN-T31 archive
