---
id: ISS-060
title: Abstract plan downgrade chain returns 0 candidates (4/25 LoCoMo conv-26 queries)
kind: issue
status: todo
severity: high
discovered: 2026-04-28
discovered_by: rustclaw
relates_to:
- ISS-049
- ISS-054
- ISS-055
- ISS-056
- ISS-058
superseded_by: .gid/issues/ISS-063/issue.md
writeup: .gid/docs/retrieval-downgrade-contract-problem.md
---

# ISS-060: Abstract plan returns 0 candidates via "downgraded_from_abstract"

## Symptom

In LoCoMo conv-26 retrieval run (post ISS-055/056/058 fixes, namespace=`conv26`,
graph-db has 192 entities + 140 edges):

```
plan=Abstract hit=false cat=4 got=0 outcome=downgraded_from_abstract  (×2)
plan=Abstract hit=false cat=5 got=0 outcome=downgraded_from_abstract  (×1)
plan=Abstract hit=false cat=1 got=0 outcome=downgraded_from_abstract  (×1)
```

4/25 queries (16%) classified as Abstract → 0 hits. Outcome `downgraded_from_abstract`
indicates the planner went through its downgrade path but the downgrade target
also produced 0 candidates.

## Why this matters

- Factual plan (the only one connected to a real adapter) hits 11/17 = 65%
  on the queries it owns.
- If Abstract degrades into Factual or Hybrid cleanly, those 4 queries should
  have a non-zero hit rate (probably 1-3 hits).
- Current ceiling = 11/25 (44%). Fixing this could push to 12-14/25.

## Hypotheses (to verify)

1. **Downgrade target is also a stub.** Abstract → ??? where ??? is one of
   Hybrid/Episodic/Associative/Affective, all of which are also unimplemented.
   If so the downgrade is cosmetic — there's no real fallback.
2. **Downgrade target is Factual but planner doesn't re-run.** Abstract decides
   "I can't handle this, use Factual" but the actual Factual adapter call is
   skipped (returns 0 instead of executing).
3. **Abstract-specific filter applied before downgrade.** Query gets pre-filtered
   to abstract-only candidates (which don't exist yet), and the downgrade step
   inherits the empty pre-filter result.

## Investigation steps

1. `grep -rn "downgraded_from_abstract\|DowngradedFromAbstract" crates/engramai/src/retrieval/`
   to find the emit site.
2. Trace from emit site backwards: what does the Abstract executor call before
   emitting this outcome?
3. Check whether the downgrade actually executes a different plan or just
   relabels and returns the original (empty) result.
4. If it's hypothesis 1 (downgrade-to-stub): block this on whichever plan
   adapter gets implemented first (probably Hybrid via ISS-061).
5. If it's hypothesis 2 (skipped re-run): one-line fix in the downgrade path
   to actually invoke the target plan's adapter.

## Acceptance

- Root cause documented (which hypothesis holds).
- Either:
  (a) Fix lands and 4 Abstract queries get non-zero candidates, OR
  (b) Issue marked blocked-by with a concrete dependency (e.g. "blocked by ISS-061
      until Hybrid is real").
- Re-run conv-26 retrieval and record the new hit count.

## Reference

- Run log: `/tmp/conv26-run-fix-1917/v03.log` (timestamp 2026-04-28 ~20:10)
- DB pair: `.gid/issues/ISS-055/locomo-conv26-iss055.{db,graph.db}`, ns=`conv26`
- Driver: `crates/engramai/examples/locomo_conv26_retrieval.rs`
