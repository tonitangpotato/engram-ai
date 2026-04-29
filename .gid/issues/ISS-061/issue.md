---
id: ISS-061
title: Hybrid plan returns 0 candidates despite outcome=ok (2/25 LoCoMo conv-26 queries)
kind: issue
status: todo
severity: high
discovered: 2026-04-28
discovered_by: rustclaw
relates_to:
- ISS-049
- ISS-055
- ISS-056
- ISS-058
- ISS-060
superseded_by: .gid/issues/ISS-063/issue.md
writeup: .gid/docs/retrieval-downgrade-contract-problem.md
---

# ISS-061: Hybrid plan returns 0 candidates with outcome=ok (silent stub)

## Symptom

In LoCoMo conv-26 retrieval run (post ISS-055/056/058 fixes):

```
plan=Hybrid hit=false cat=5 got=0 outcome=ok
plan=Hybrid hit=false cat=4 got=0 outcome=ok
```

2/25 queries classified as Hybrid → outcome=ok (no error/downgrade signal)
yet candidates=0. This is worse than Abstract's `downgraded_from_abstract`
because it's silent — the planner reports success while delivering nothing.

## Why this matters

- Hybrid is conceptually the easiest non-Factual plan to wire up: it's
  Factual + an additional signal (associative or graph-walk). Factual already
  works at 65% on its own.
- A correctly-implemented Hybrid should approximately match or beat Factual
  on the queries it owns (2-3 of 2 = 100%-ish, since Hybrid was chosen for
  multi-hop questions).
- Ceiling impact: 11/25 → 12-13/25 if Hybrid lands a real adapter.

## Actual root cause (2026-04-28 investigation)

**Hybrid plan itself is fully implemented and behaves correctly.** It is an
aggregator: it picks 1-2 strong sub-plans via `select_subplans()` (tau_high=0.7,
HYBRID_SUBPLAN_CAP=2), runs them via `HybridDispatchExecutor`, and fuses with RRF.
The orchestrator path (`PlanKind::Hybrid` arm) is wired and runs.

The 2 conv-26 queries that hit candidates=0 are:
- "What did Melanie realize after the charity race?" → Hybrid picks Episodic + Abstract
- "What did Caroline realize after her charity race?" → Hybrid picks Episodic + Abstract

For both:
- `hybrid_sub_plan EXIT sub_kind=Episodic items=0` — Episodic has no
  `time_window` ("after the charity race" is a relative phrase, no absolute
  date) → Episodic correctly downgrades to empty per design.
- `hybrid_sub_plan EXIT sub_kind=Abstract items=0` — same bug as ISS-060
  (Abstract downgrade chain returns 0).

Hybrid is a **downstream victim of two upstream issues**:
1. ISS-060 (Abstract returns 0 via `downgraded_from_abstract`)
2. Episodic-without-time-window correctly returns 0, but Hybrid has no
   fallback when ALL selected sub-plans return empty.

## Decision needed

This is no longer "implement Hybrid" — Hybrid works as designed. It's a
design choice:

**Option A: Empty-sub-plan fallback to Factual.** When every selected
sub-plan returns 0, Hybrid runs a Factual sub-plan as a last-resort signal
to avoid a silent empty result. Pros: cheap (~30 LOC), covers ISS-060
spillover. Cons: changes Hybrid's contract (design §4.7 lists fixed sub-plan
set), may mask real upstream bugs.

**Option B: Don't change Hybrid.** Fix the upstream causes:
- Fix ISS-060 (Abstract downgrade)
- Add Episodic relative-time-phrase support (separate issue), OR
- Tune classifier so queries without time_window get routed to Factual not Hybrid

**Option C: Mark `outcome=stub_no_subplan_candidates`** when all selected
sub-plans empty out, so the silent `outcome=ok` is replaced with a signal
that something's wrong upstream — no behavior change, but observability
improves and the metric reflects reality.

Recommendation: **Option C now (cheap, no semantic change), Option B as the
real fix.** Option A is tempting but blurs the diagnostic signal.

## Acceptance

- Decision recorded above.
- If A or C: code change lands, conv-26 re-run logged.
- If B: this issue blocked-by ISS-060 + new "episodic-relative-time" issue.

## Acceptance

- Root cause confirmed (stub vs. real-but-buggy).
- Either:
  (a) Real Hybrid lands, conv-26 hits non-zero on those 2 queries, OR
  (b) Outcome explicitly marked `stub` so downstream tooling can distinguish
      "Hybrid failed to implement" from "Hybrid genuinely had no candidates".
- conv-26 run logged with new numbers.

## Reference

- Run log: `/tmp/conv26-run-fix-1917/v03.log`
- DB pair: `.gid/issues/ISS-055/locomo-conv26-iss055.{db,graph.db}`, ns=`conv26`
- Driver: `crates/engramai/examples/locomo_conv26_retrieval.rs`
