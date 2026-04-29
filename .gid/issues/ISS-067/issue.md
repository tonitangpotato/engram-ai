---
id: ISS-067
title: "Hybrid: decide fallback contract when ALL sub-plans downgrade-to-empty (ISS-061 follow-up)"
kind: issue
status: open
severity: medium
discovered: 2026-04-29
discovered_by: rustclaw
relates_to: [ISS-061, ISS-063, ISS-060]
---

# ISS-067: Hybrid plan — decide fallback contract when all sub-plans downgrade-to-empty

## Context

ISS-061 asked: *"if Hybrid sub-plans all return 0, should the plan fall
back to a working sibling (e.g. Factual) instead of surfacing
`empty_result_set`?"* That decision was deferred to a follow-up issue
once ISS-063 landed and we could **confirm sub-plans genuinely return
0 items** (not a `hybrid_to_scored` ID-mapping bug).

This is that follow-up issue.

## Confirmation (RUN-0003, 2026-04-29)

LoCoMo conv-26 sessions 1-3, post-ISS-063, smoke substrate
`.gid/issues/_smoke-locomo-2026-04-28/locomo-conv26-s1-3-iss058.{db,graph.db}`,
ns=`locomo-conv26-iss058`. 14/25 hits, identical to RUN-0002.

The two Hybrid queries (q13, q21) both produce `empty_result_set` and
hit=false. Per-sub-plan trace **at the source** — i.e. before any
fusion/scoring — shows genuine 0 items:

```
[hybrid] q13 "What did Melanie realize after the charity race?"
  hybrid_sub_plan EXIT sub_kind=Episodic   items=0  outcome=DowngradedFromEpisodic
  hybrid_sub_plan EXIT sub_kind=Abstract   items=0  outcome=DowngradedL5Unavailable
  execute_plan   EXIT plan_kind=hybrid     candidates=0  outcome=empty_result_set

[hybrid] q21 "What did Caroline realize after her charity race?"
  (identical pattern)
```

So the question ISS-061 deferred is now answerable: **sub-plans
genuinely returned 0 items.** `hybrid_to_scored` is exonerated. The
choice is between Option A (Hybrid fallback) and Option B (fix the
upstream sub-plans).

Gold for both queries is `D2:3`, which is in the substrate (sessions
1–3 range). Both queries pattern: "after the charity race" — relative
time phrase, no absolute window → Episodic downgrades. Abstract L5 is
unimplemented → second downgrade. Result: 0+0 → empty.

## Options (carried over from ISS-061, scoped to *post-confirmation*)

### Option A — Hybrid sub-plan fallback

If all selected sub-plans downgrade-to-empty, Hybrid runs Factual (or
some configured "always-on" fallback plan) over the same query and
returns those candidates with a clear `outcome=hybrid_fellback_to_X`.

- Pro: hits @ 5 likely +2/25 immediately (relative-time queries on the
  same conv stand to gain).
- Con: blurs the per-plan-kind diagnostic signal — a Hybrid query that
  fell back to Factual will look like Factual hit-rate. Mitigation:
  keep `outcome` distinct so per-outcome breakdown still attributes
  correctly.
- Risk: Factual on q13/q21 may also miss (gold D2:3 is "she felt
  fulfilled after the run" — not literal-keyword material).

### Option B — Upstream sub-plan fixes

Two sub-issues:
- **Episodic relative-time-phrase support** ("after the charity
  race", "before the wedding") — non-trivial; requires either time-
  expression resolution against ingested events or a separate plan.
- **Abstract L5** — currently flagged `DowngradedL5Unavailable`; a
  real implementation is its own multi-week issue.

- Pro: clean architecturally. Each plan does one thing well.
- Con: blocks LoCoMo Hybrid recovery on two unrelated big features.

### Option C (rejected) — keep current behavior

`empty_result_set` is the honest answer when nothing matches. But
ISS-061 already chose this for the **observability** question. The
LoCoMo-recovery question is separate and Option C means accepting
2/25 perma-loss on Hybrid.

## Recommendation

**Option A, with a flag (`hybrid.fallback_plan`) defaulted to
`Factual`.** Reasoning:

1. Cheap (one branch in `HybridDispatchExecutor`).
2. Reversible (flag flip).
3. Doesn't block on B's two large features.
4. Diagnostic signal preserved via distinct outcome label.

If A lands and the conv-26 numbers don't move (because Factual also
misses on those gold docs), then we know B is the only path and we
escalate.

## Acceptance

- Option chosen and recorded.
- If A: code change lands; conv-26 RUN-NNNN logged with new hit count
  for q13 and q21 specifically; per-plan breakdown shows the new
  `hybrid_fellback_to_factual` outcome distinct from native Factual.
- If B: this issue is closed and replaced with two specific issues
  ("Episodic relative-time" + "Abstract L5 implementation"), and a
  meta-issue tracking the joint LoCoMo recovery.

## Reference

- RUN-0003 log: `.gid/issues/_smoke-locomo-2026-04-28/RUN-0003.log`
- RUN-0003 doc: `.gid/eval-runs/RUN-0003.md`
- Hybrid plan source: `crates/engramai/src/retrieval/plans/hybrid.rs`
- Predecessor: ISS-061 (resolved by ISS-063), ISS-060 (Abstract).
