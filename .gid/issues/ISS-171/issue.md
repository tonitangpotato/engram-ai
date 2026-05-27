---
title: 'Plan classifier never routes single-fact LoCoMo questions to Factual plan (0/152, all go to Associative)'
status: open
priority: P1
severity: root-cause-suspected
category: retrieval
created: 2026-05-27
relates:
- ISS-148
- ISS-164
- ISS-149
- ISS-162
- ISS-165
discovered_in: ISS-164 Phase 2 RE-RUN (engram-bench:f28b41d, sweep STAMP 20260527T051146Z)
---

## Summary

In the ISS-164 Phase 2 re-run (post-ISS-165/166 fix, full
substrate, K=10 temp=0 HyDE=off), the plan classifier routed
**0 of 152 LoCoMo conv-26 queries to the Factual plan**. The
distribution was:

```
121 associative
 18 abstract
  6 affective
  5 hybrid
  2 episodic
  0 factual
```

Source: `grep "execute_plan ENTER" /tmp/iss164-bench/iss164-A.log
| awk -F'plan_kind=' '{print $2}' | sort | uniq -c`.

This includes **all 9 ISS-161 single-fact questions** (q3, q7,
q11, q37, q40, q43, q71, q75, q76) — all of which ask for one
specific entity/fact as gold ("Sweden", "Becoming Nicole",
"abstract art", "sunset", "Adoption agencies", etc.). These are
the textbook Factual-plan use case.

## Why this matters

ISS-164's entity-channel design assumed Factual plan would
consume the resolved anchors. The Phase 2 re-run produced
single-fact 0/9 → 0/9 with Δ=0 because **the Factual plan never
ran**. The anchors landed in the Associative plan's seed_entities
instead, where they feed an aggregation pipeline that washes
single-fact retrieval signal.

This may be the real ISS-148 AC-5a (single-fact ≥ 0.60)
bottleneck. The entity_channel + resolver fixes (ISS-164,
ISS-165, ISS-166, ISS-167) were all necessary but wrong-layer —
the classifier needs to route these questions to Factual first,
then the anchor work can carry weight.

## Hypotheses (need investigation)

**H1**: Classifier is heuristic / embedding-based and LoCoMo's
question phrasing ("What did Caroline research?", "Where did
Caroline move from?") doesn't match the Factual intent cluster
the classifier was trained/tuned on. Possibly tuned on QA
templates ("What is the capital of X?", "Who wrote Y?") and
LoCoMo's conversational tone routes elsewhere.

**H2**: Classifier confidence thresholds are mis-set — Factual
plan requires high confidence to override the default
Associative path, and LoCoMo single-fact questions never hit
that threshold.

**H3**: There IS no Factual plan code path being exercised here
at all, only the enum variant. The retrieval pipeline has
collapsed to Associative-by-default since some earlier change.

## Acceptance criteria

- [ ] **AC-1**: Find the classifier — locate the code that
  decides `plan_kind` per query. Likely
  `crates/engramai/src/retrieval/plans/classifier.rs` or
  similar.
- [ ] **AC-2**: Determine why the 9 single-fact LoCoMo questions
  route to Associative. Dump classifier scores per plan_kind
  for those 9 questions.
- [ ] **AC-3**: Categorize the failure: heuristic mismatch (H1),
  threshold mis-set (H2), or path-dead (H3).
- [ ] **AC-4**: If H1: propose a fix (re-tune intent embeddings
  / add LoCoMo-style training examples / use LLM classifier).
- [ ] **AC-5**: If H2: surface and document the threshold; A/B
  on tweaked threshold.
- [ ] **AC-6**: A/B sweep on conv-26: classifier-fixed vs
  current. Measure single-fact bucket lift. If Factual plan
  now fires on single-fact AND entity_channel is on, we should
  see real anchor utilization.

## Cross-references

- ISS-148: AC-5a single-fact ≥ 0.60 target — likely blocked by
  this classifier issue, not by the anchor work
- ISS-149: previously suspected classifier death; this issue is
  the empirical confirmation
- ISS-164: entity_channel falsified because anchors fed wrong
  plan (Associative instead of Factual)
- ISS-162: extraction context was queued behind ISS-164; same
  re-evaluation applies
- ISS-165: resolver fix is correct and ships, just wasn't
  enough on its own

## Suggested first move

`grep -rn "plan_kind\|classify\|PlanKind::Factual" crates/engramai/src/retrieval/`
then dump per-query classifier scores during a 9-question probe.
Cheap, no API spend, points at the root cause directly.

## References

- Sweep log: `/tmp/iss164-bench/iss164-A.log`
- Per-query: `engram-bench/benchmarks/runs/ISS164-A-conv26-20260527T051146Z/locomo_per_query.jsonl`
- ISS-164 Phase 2 verdict: `.gid/issues/ISS-164/issue.md` (2026-05-27 entry)
