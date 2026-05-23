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

## Bisect strategy

Window: engram master between RUN-0025 (2026-05-06, J=0.559) and RUN-T31
(2026-05-23, J=0.395). 91 engramai commits in the window; engram-bench
crate has been frozen since 2026-05-06 (all uncommitted-but-present
files dated May 2–6), so the regression is engram-side only.

The legacy arm of RUN-T31 used `Storage::with_unified_substrate(_,
false)`, which means it exercised ONLY the legacy read path. Any
commit that touched ONLY unified-substrate code (T29.* read switches,
ISS-12x dual-write fixes for `nodes`/`edges` tables) cannot be the
culprit, narrowing the suspect list to commits that touched the legacy
write/read code or the data layout of the legacy tables.

### Prioritised suspect list (highest priority first)

1. **`4163f36` ISS-117 — collapse `hebbian_links` to single canonical
   row (2026-05-13)**
   Most suspicious. Changed the writer to drop reverse-direction
   INSERTs/UPDATEs in `record_coactivation` /
   `record_coactivation_ns` / `record_cross_namespace_coactivation`.
   Reader (`get_hebbian_links_weighted`) uses `source_id = ?1 OR
   target_id = ?1` so a single row matches either direction — query
   itself is direction-agnostic. But any caller that COUNTed hebbian
   rows, or that indexed activations per `source_id` separately, would
   see half the rows post-ISS-117. Worth checking: are there any
   counting/aggregation queries on `hebbian_links` in the recall
   path that would be sensitive to row count vs canonical-pair count?

2. **`6f47e66` lock-free store API + consolidation split (2026-05-14)**
   New `consolidate_db_only()` skips synthesis + triple extraction.
   Commit claims existing public API unchanged. LoCoMo driver calls
   the standard `sleep_cycle` path, so this *should* be inert — but
   "consolidation split" plus the resolution-pipeline namespace
   threading nearby is exactly the kind of refactor that quietly
   re-orders a single SQL UPDATE and changes outcomes. Worth verifying
   the bench driver path.

3. **`5eff26b` ISS-118 — ns-aware canonical row migration (2026-05-13)**
   Migrated existing namespaced hebbian rows. LoCoMo fresh-DB runs
   don't trigger migration but the canonical-pair logic added by
   ISS-118 may have changed.

4. **`aca955b` ISS-131 — clamp working_strength to 1.0 (2026-05-15)**
   Reward layer clamp. Could affect ranker scores if any path used
   `working_strength > 1.0` to surface candidates.

5. **`0282d53` ISS-119 — round-trip contradicts/contradicted_by
   through `nodes.attributes`**
   Touched supersession semantics. Should only affect unified path,
   but worth a glance.

### Cheap pre-bisect signals (run before spending LoCoMo $/time)

- Diff conv-26 retrieval candidate lists between
  `git checkout d61471b` (pre-ISS-117) and current master.
  `t30_probe_parity.rs` style Jaccard against conv-26 question set.
  Cheap (no LLM judge) and isolates retrieval-set regressions from
  LLM-judge regressions.
- Count `hebbian_links` rows after a clean conv-26 ingest at
  d61471b vs master. If row counts halve, ISS-117 changed the data
  population the recall path sees.
- Diff `MemoryConfig::default()` between RUN-0025 commit and master —
  any default-value drift (decay coefficients, ranker weights,
  threshold tweaks) explains regressions without code-path bugs.

### Bisect execution

If pre-bisect signals don't pinpoint the cause, do a 3-step git bisect
on the suspect commits (revert one at a time on master, run a 152-q
LoCoMo, observe). Each LoCoMo run costs ~25 min wall-clock + LLM
credits; the entire bisect should be ≤ 3 runs (≤ 75 min wall) given
the prioritised list above.

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
