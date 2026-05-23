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

## Update 2026-05-23 02:50 — Pre-bisect signal analysis points at ISS-105 (4943aea)

Performed cheap pre-bisect signal #3 (config drift) and code-inspection
based suspect refinement (no LoCoMo runs spent).

**Signal #3 (config drift)**: `git diff d54a3e1..HEAD --
crates/engramai/src/config.rs` shows the only `MemoryConfig` change is
addition of the `unified_substrate` field (T28, commit 7ee3898). No
default-value drift on retrieval weights, decay coefficients, or
ranker thresholds. **Clean — not the cause.**

**Code inspection — narrowed suspect list**:

Filtering the 78-commit window (d54a3e1..HEAD) by files-touched =
`retrieval/`, `recall/`, `memory.rs`, `ranker/`, `spreading/`,
`association/` yields only **5 commits**:

1. `4943aea` ISS-105 generalized overfetch wiring (2026-05-12 23:56)
2. `4163f36` ISS-117 hebbian canonical row collapse (2026-05-13 20:22)
3. `aca955b` ISS-131 working_strength clamp 2.0→1.0 (2026-05-15)
4. `ec45c92` ISS-103 separate occurred_at from created_at (~2026-05-09)
5. `6f47e66` lock-free store API + consolidation split

**New top suspect: `4943aea` ISS-105 generalized overfetch wiring.**

Why this jumps to #1 (above ISS-117):

- ISS-105 is **explicitly** a candidate-pool-sizing change. Pre-ISS-105
  the K_seed cap silently held the fused candidate pool at ~10
  regardless of requested top-K (commit msg: "RUN-0020 K=15 had
  145/152 queries returning exactly 10 candidates"). Post-ISS-105 the
  Factual sub-plan computes `effective_limit = min(α × requested_k /
  anchors.len(), memory_limit_per_entity)` with α=3 and the per-entity
  cap bumped 50→100. Associative sub-plan also gets `.with_k_seed(
  query.limit)`. For LoCoMo `requested_k=10`, this is a **15× increase
  in candidate pool** (10 → ~150) flowing into `fuse_rrf`.
- A 15× candidate-pool expansion fundamentally changes the ranker's
  job: pre-fix it was choosing top-K from ~10 candidates (so rank
  order ≈ identity, all 10 returned), post-fix it's choosing top-K
  from ~150 candidates (real ranker discrimination). If the ranker is
  noisy or the new candidates introduce semantically-distant memories
  that score artificially high on FTS/embedding channels, J-score
  drops even though "more candidates" sounds like a strict win.
- Timeline match: RUN-0027 (pre-regression, 0.467) was 2026-05-06.
  ISS-105 landed 2026-05-12 23:56 — between RUN-0027 and RUN-T31. ✓
- Per-category evidence from RUN-T31 supports this hypothesis:
  - `single-hop` 0.125 (legacy) / 0.0625 (unified) — both arms
    collapsed. Single-hop is where wide candidate pools hurt most
    because there's exactly one correct answer; surfacing 149 wrong
    candidates dilutes ranker signal.
  - `multi-hop` 0.541 / 0.595 — held up better because multi-hop
    benefits from richer candidate pools.
  - `open-domain` 0.154 / 0.231 — unified gained from broader pool,
    legacy didn't. Consistent with the ranker handling expansion
    differently per arm.

Why this displaces ISS-117 as top suspect:

- ISS-117 writer collapse halves `hebbian_links` row count, but the
  reader is `WHERE source_id = ? OR target_id = ?` post-ISS-117 — so
  it returns one row per pair instead of two, same distinct-neighbor
  count.
- `hebbian_channel_scores` normalizes to [0, 1] internally (divides
  by max) before fusion, so absolute halving is invisible to the
  final score.
- The commit msg explicitly notes a side-benefit: "memory.rs:4412
  recall scoring summed strength across the dup pair → 2× over-score
  on formed Hebbian neighbours. Now correct." — meaning ISS-117 may
  have lowered Hebbian channel **absolute weight** by 2×, but the
  channel weight `hebbian_recall_weight` is a config knob that was
  presumably tuned before ISS-117. Net effect on top-K: small.
- ISS-117 still belongs on the list (it changed Hebbian neighbour
  pool composition during decay/merge edge cases), just not #1.

**Refined cheap confirmation (no LoCoMo $)**:

Before running a 152q bisect, revert just the `requested_k` /
`memory_limit_per_entity` changes from 4943aea and compute the
**candidate-pool size histogram** on conv-26's 152 queries. If
pre-ISS-105 the histogram concentrated at ~10 and post-ISS-105 it
spreads to 30-150, the mechanism is confirmed and we know exactly
which knob to revert.

Alternatively, **A/B revert ISS-105 only** and run a 152q LoCoMo
(~25 min, ~$X). If J-score returns to ≥0.45, ISS-105 is the cause
and we either: (a) tune α down (3 → 1.5?), (b) keep ISS-105 but
add ranker calibration that handles wider pools, or (c) accept the
trade and re-baseline the P0 LoCoMo gate.

### Updated prioritised suspect list

1. **`4943aea` ISS-105 — generalized overfetch wiring (2026-05-12)** ← NEW #1
2. **`4163f36` ISS-117 — hebbian canonical row collapse (2026-05-13)** ← was #1
3. **`aca955b` ISS-131 — working_strength clamp (2026-05-15)**
4. **`ec45c92` ISS-103 — occurred_at separation (~2026-05-09)**
5. **`6f47e66` lock-free store + consolidation split**

---

## 2026-05-23 — Pre-bisect signal #4 (content-based top-K diff): ISS-105 **FALSIFIED**

Built a retrieval-only probe (`engram-bench/examples/iss136_candidate_histogram.rs`)
that replays conv-26 (419 episodes / 152 questions) through `graph_query_locked`
with `--top-k 10` and dumps per-query content fingerprints (first 64 chars of each
result's `MemoryRecord.content`, lowercased + trimmed). Memory IDs are random
UUIDs regenerated per ingest so they don't survive across builds — but text
fingerprints do, since the fixture is identical.

**A/B procedure:**
1. Current HEAD (`82e26d6`, `engram-bench` HEAD; engramai unchanged) → dump
   /tmp/iss136-histogram-current.json
2. `git revert --no-commit 4943aea` in engram (changes only
   `retrieval/orchestrator.rs` + `retrieval/plans/factual.rs`) → rebuild → dump
   /tmp/iss136-histogram-pre-iss105.json
3. Revert the revert, rebuild back to current → re-dump current
   (deterministic ingest still gives random UUIDs but identical fingerprints,
   so we re-ran current to confirm steady state)

**Result (content-fingerprint diff over 152 conv-26 queries):**

| metric                | value         |
| --------------------- | ------------- |
| identical_rank_order  | **150 / 152** |
| identical_set @ K=10  | **152 / 152** |
| jaccard@10 mean       | **1.000**     |
| jaccard@10 median     | 1.000         |
| jaccard@10 min        | 1.000         |
| queries_with_any_diff | 0 / 152       |
| top1_differs          | 2 / 152       |

The 2 rank-order diffs (q11, q76) are pure tie-break noise — every score is
**1.000** for all 10 positions, so the underlying ordering is undefined and
the sort just lands differently across the two builds. Set-Jaccard = 1.000
confirms the SAME 10 rows reach top-K in both builds for all 152 queries.

**Conclusion:** ISS-105's α=3 overfetch + per-entity cap bump (50→100) does
NOT alter the top-K@10 result set on conv-26. The expanded candidate pool
exists but gets thrown away by the truncate step — the rows that survive
fuse+rank's first 10 are unchanged. Therefore **ISS-105 cannot be the cause
of the master LoCoMo regression 0.467 → 0.395** observed at the same K=10.

**Suspect list revision:**

1. ~~`4943aea` ISS-105~~ **FALSIFIED** — top-K@10 identical pre/post
2. **`4163f36` ISS-117 — hebbian canonical row collapse** ← restored to #1
3. **`aca955b` ISS-131 — working_strength clamp**
4. **`ec45c92` ISS-103 — occurred_at separation**
5. **`6f47e66` lock-free store + consolidation split**

Note on ISS-117: earlier I reasoned the absolute-halving was invisible to
the final score because hebbian_channel_scores normalizes to [0,1]. That
reasoning still stands for the channel itself, but ISS-117 changed *which
rows* the channel sees (canonical pair collapse) — that's a set-level change,
not just a score-magnitude change, and could shift top-K membership. The
content-diff probe applies cleanly here too: run the A/B with `git revert
--no-commit 4163f36` and re-diff.

**Files:**
- `engram-bench/examples/iss136_candidate_histogram.rs` (190+ lines, no LLM, no judge)
- `/tmp/diff_hist_content.py` (content-fingerprint Jaccard@K diff)
- Output JSONs (per-query top_content + top_scores) at
  `/tmp/iss136-histogram-{current,pre-iss105}.json`

**Cost so far:** 2 builds × ~45s engramai + ~20s engram-bench, 4 probe runs ×
~15s = ~3 min compute, **$0 LLM**. The probe is reusable for the remaining
4 suspects.

