---
id: ISS-093
title: Hybrid/Episodic plan recency-dumps top-5 = last session, ignoring content signal
status: open
priority: P1
severity: high
tags: [retrieval, hybrid-plan, episodic-plan, locomo, recency-bias, fusion]
created: 2026-05-01
relates_to: [ISS-070, ISS-083, ISS-084]
---

# Hybrid/Episodic plan recency-dumps top-5 = last session

## Symptom

In RUN-0012 (locomo-conv26-full hit@5 retrieval, 197 QAs), 12 misses exhibit
the exact same pattern: **top-5 is entirely composed of the last session
(D19) regardless of what the query asks**. No content match, no session
diversity — just "the most recent stuff".

```
[ 12] cat=1 plan=Episodic Q="Where did Caroline move from 4 years ago?"
      gold=[D3:13, D4:3]   top=[D19:2, D19:1, D19:5, D19:11, D19:7]

[ 82] cat=4 plan=Hybrid    Q="What did Melanie realize after the charity race?"
      gold=[D2:3]           top=[D19:1, D19:2, D19:5, D19:11, D19:7]

[150] cat=4 plan=Hybrid    Q="What did Melanie do after the road trip to relax?"
      gold=[D18:17]         top=[D19:1, D19:2, D19:5, D19:11, D19:7]

[151] cat=5 plan=Hybrid    Q="What did Caroline realize after her charity race?"
      gold=[D2:3]           top=[D19:1, D19:2, D19:5, D19:11, D19:7]
```

Total: 12 cases — **5 cat=4 (all Hybrid), 5 cat=5 (all Hybrid), 2 cat=1 (Episodic)**.
All 12 share the identical top-5 ordering `[D19:1, D19:2, D19:5, D19:11, D19:7]`,
which is also a tell: this is exactly what an episodic working-memory leg
returns when fed a generic activation seed and asked for "most recently
salient items".

The substrate has the gold turns (other queries hitting D2:3, D3:13, D18:17
from `plan=Factual` succeed). The retrieval engine has them. The Hybrid /
Episodic dispatch path discards them in favor of recency.

## Why it matters

- **Distinct from ISS-083.** ISS-083 was about Hybrid emitting *empty*
  results when sub-plans had nothing. That's fixed (Hybrid now downgrades
  to Factual on empty). This issue is about Hybrid (and Episodic) emitting
  *non-empty but content-blind* results — a different failure mode that
  ISS-083's downgrade path doesn't trigger because the episodic leg
  technically returns 5 candidates.
- **Silent quality loss.** No outcome marker fires. The engine reports
  `outcome=ok`, the metric framework counts the query as "served", and
  the wrong content slips through.
- **Hits hardest on multi-hop and adversarial.** 10/12 occurrences are
  cat=4/5 (LoCoMo's reasoning-heavy categories). Fixing this alone
  recovers ~10pp on cat=4 hit@5 and cat=5 hit@5 if the associative leg's
  content matches are restored.

## Root cause hypothesis

`HybridPlan` in `crates/engramai/src/retrieval/orchestrator.rs` fuses
candidate sets from multiple sub-plans. When the **episodic** leg returns
high-activation working-memory items (last session by construction —
working memory's whole point is recency), and the **associative** leg
returns embedding-matched items with normal cosine scores (~0.3-0.6), the
episodic scores dominate fusion because:

1. Episodic activation scores are not normalized to the same range as
   embedding cosine.
2. Score fusion is likely additive or weighted-sum without rank-based
   normalization (RRF).
3. There's no diversity floor — top-5 can be 5/5 same session.

Same dynamic explains why Episodic plan alone (no Hybrid wrapper) also
recency-dumps on the 2 cat=1 cases: Episodic plan straight-up returns
working memory unless the query has a strong temporal anchor that
re-targets the activation.

## Reproduction

All 12 reproducer queries are in
`.gid/eval-runs/RUN-0012-iss091/RUN-0012-records.json` (filter `hit=false`,
`top` matches regex `^\[D19:`). Per-query trace available in
`RUN-0012-full-conv26.log`.

For a single repro:

```bash
cd /Users/potato/clawd/projects/engram
RUST_LOG=engramai::retrieval=debug \
  cargo run --release --example locomo_conv26_retrieval -- \
  --db   .gid/eval-runs/RUN-0012-iss091/locomo-conv26-full.db \
  --graph-db .gid/eval-runs/RUN-0012-iss091/locomo-conv26-full.graph.db \
  --dataset /Users/potato/clawd/projects/cogmembench/datasets/locomo/data/locomo10.json \
  --max-session 19 --limit 5 --ns locomo-conv26-full \
  --query-only "What did Melanie realize after the charity race?"
```

Expected (with fix): top-5 contains `D2:3` (the charity-race turn).
Actual: top-5 = `[D19:1, D19:2, D19:5, D19:11, D19:7]`.

## Proposed investigation steps

1. **Confirm fusion math.** Read `HybridPlan::execute` and the score-merge
   call site. Capture episodic-leg raw scores vs associative-leg raw
   scores for query 82. Check whether the issue is (a) raw score scale
   mismatch, (b) rank-only fusion missing, or (c) episodic leg is
   returning *too many* candidates and crowding out associative.
2. **Confirm Episodic standalone behavior.** Run the 2 cat=1 Episodic
   misses through `EpisodicPlan` directly (not via Hybrid). If they also
   recency-dump, the bug is in EpisodicPlan, not Hybrid fusion.
3. **Inspect dispatcher classification.** Why does cat=1 single-hop ever
   classify as Episodic? Cases like "Where did Caroline move from 4 years
   ago?" — temporal phrase "4 years ago" might be triggering Episodic
   routing. If so, classification needs adjustment for past-temporal vs
   present-recency.

## Proposed fix (after investigation)

Likely a combination of:

- **Reciprocal Rank Fusion (RRF)** for combining episodic and associative
  legs, replacing score-additive fusion. RRF naturally normalizes across
  scoring scales.
- **Diversity / session-coverage penalty** — once 2 candidates from the
  same session are in top-K, subsequent same-session candidates take
  a downweight. This breaks single-session dumps.
- **Episodic leg should be off** unless the query has a recency intent.
  "What did Melanie realize after the charity race" is not a
  working-memory query; routing it through episodic at all is the
  upstream mistake.

## Acceptance criteria

- [ ] Investigation: log episodic-leg vs associative-leg raw scores +
  fusion output for query 82 (`What did Melanie realize after the
  charity race?`); diagnosis written into a comment on this issue.
- [ ] Decide between (a) fusion fix (RRF + diversity), (b) classifier
  fix (don't route to Episodic for non-recency queries), (c) both.
- [ ] After fix: re-run RUN-0012 retrieval on the same substrate; the
  12 reproducer queries no longer have `top ⊂ {D19:*}`.
- [ ] cat=4 hit@5 ≥ 50% (currently 48.6%). cat=5 hit@5 ≥ 50% (currently
  48.9%). Both should move ~5-7pp from this fix alone if the 10
  Hybrid recency-dumps recover.
- [ ] No regression in cat=2 (currently 86.5%) — temporal queries
  legitimately use recency and should not be over-penalized by
  diversity.
- [ ] Test: substrate with multi-session content, query with content
  match in early session, classified as Hybrid → assert top-5 has at
  least one candidate outside the most-recent session.

## Out of scope

- Real episodic-layer wiring for L5 inference (that's ISS-083's L5 work).
- Cross-encoder reranking (separate, in ISS-085 / ISS-084 Path B).
- Multi-query expansion (separate, ISS-084 Path B).

## References

- RUN-0012 RESULTS: `.gid/eval-runs/RUN-0012-iss091/RESULTS.md` §2 Mode B.
- Records: `.gid/eval-runs/RUN-0012-iss091/RUN-0012-records.json`.
- Raw log: `.gid/eval-runs/RUN-0012-iss091/RUN-0012-full-conv26.log`.
- Related: ISS-083 (Hybrid empty-downgrade — different failure mode, both
  in Hybrid), ISS-070 (multi-hop dispatcher), ISS-084 (SA path decision).
