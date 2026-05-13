---
id: ISS-108
title: Micro-benchmark for retrieval/clustering changes (decouple from LoCoMo to prevent overfit)
kind: issue
status: todo
priority: high
labels: [benchmark, methodology, prerequisite]
relates_to: [ISS-106, ISS-107]
---

# ISS-108: Micro-benchmark for retrieval / clustering changes

## Why this exists

Every retrieval / fuse / clustering change in 2026-04 / 2026-05 has
been validated by **re-running the full LoCoMo benchmark and watching
J-score**. This methodology has visible failure modes:

1. **LoCoMo is a degenerate data shape.** Single conversation between
   two parties over weeks. Real engram users have multi-conversation,
   multi-author, multi-topic data. Tuning a clustering threshold to
   make conv-26 work *will* break the production case (see ISS-107).

2. **J-score is summative, not diagnostic.** When J moves
   −0.22 (RUN-0026), we know "something is worse" but not "which
   sub-plan, which query type, which mechanism". Forensic work
   (read substrate.db, count topics, eyeball candidates) consumed
   most of the RUN-0026 investigation budget. A diagnostic harness
   would have given the answer in one query.

3. **Cost & latency.** Full LoCoMo = 152 queries × LLM-judge ≈ 30–50
   minutes per run + Anthropic spend. Iterating on a clustering
   threshold sweep at 6 values = 3–5 hours and ~$5. A micro-bench
   should run in <60s offline.

4. **Sonnet-4.5 alignment trap.** All judging uses sonnet-4.5. As we
   tune the system to LoCoMo + sonnet-4.5, we drift toward "what
   sonnet thinks is a good answer on conv-26", not "what is
   genuinely good retrieval".

This issue is the **prerequisite** for ISS-107 clustering work and
any future retrieval tuning.

## What it is (spec)

A small, hand-crafted, locally-scoring benchmark suite covering the
internal correctness of retrieval primitives — not end-to-end QA.

### Suite 1 — Sub-plan precision (~40 queries)

For each sub-plan (Factual / Episodic / Abstract / Affective):

- Hand-craft 10 queries with known-correct memory IDs in a fixture
  substrate.db.
- For each query, run *only that sub-plan* and assert top-K returns
  the expected memory IDs.
- Output: precision@K per sub-plan, regression diff vs previous run.

This catches "did the change break Factual while improving
Abstract" — invisible in J-score because they cancel.

### Suite 2 — Fuse stability (~20 queries)

- Hand-craft 20 queries where the *correct* memory is unambiguously
  in one sub-plan's output.
- Run full fuse pipeline, assert correct memory survives to top-3.
- Output: fuse-survival rate per sub-plan-of-origin.

This catches "fuse is dropping good candidates because of α / weight
miscalibration".

### Suite 3 — Clustering shape (~10 corpora)

- 10 fixture substrates with *known* cluster ground truth:
  - 1 single-conversation (LoCoMo-shape) — expect ~5–15 clusters
  - 1 multi-conversation (chat-app-shape) — expect 1 cluster per topic
  - 1 mixed (journal + email + slack) — expect cluster-per-source
  - 1 all-similar (same topic, different days) — expect few clusters
  - 1 all-different (random topics) — expect many clusters
  - …etc
- Run `compile_knowledge`, assert cluster count is in expected
  range AND no cluster covers >40% of memories.

This catches **exactly** the ISS-107 failure mode without needing to
re-run LoCoMo.

### Suite 4 — Temporal grounding (~15 queries)

- Hand-craft 15 queries with explicit temporal anchors ("when did X
  happen", "what came before Y").
- Assert temporal layer returns memory in correct time order.
- Catches ISS-103-style regressions.

## What it is NOT

- **Not** an LLM-judge benchmark. All scoring is exact-match on
  memory IDs (or rank position). No Anthropic calls. Runs offline.
- **Not** a replacement for LoCoMo. LoCoMo stays as the end-to-end
  gate. Micro-bench is a *precondition* — passes before LoCoMo runs.
- **Not** statistically large. ~85 queries total. Goal is *coverage
  of mechanisms*, not statistical power.

## Cost / effort estimate

- Fixture authoring: 4–6 hours (largest cost)
- Harness wiring: 2–3 hours (reuse engram-bench infra)
- Total: ~1 day of focused work

This is small relative to the cost of one mis-tuning incident
(RUN-0026 + investigation = ~half a day already).

## Acceptance criteria

- [ ] Spec finalized (this issue is a draft of it; iterate)
- [ ] Fixture substrates committed under
      `engram-bench/benchmarks/microbench/fixtures/`
- [ ] Harness runs all 4 suites in <60s on a Mac mini
- [ ] CI hook (or `make microbench`) runs the suite
- [ ] First green baseline recorded
- [ ] ISS-107 clustering work is verified against Suite 3 *before*
      re-running LoCoMo

## Discovered

Triggered by ISS-107 root-cause investigation (RUN-0026, 2026-05-06).
Pattern visible across earlier work too: every retrieval change in
RUN-0017 → RUN-0024 was validated only on LoCoMo, with no
mechanism-level test. Several of those changes may be partially
LoCoMo-overfit; we currently have no way to know.

## Out of scope for this issue

- Implementing the suites (separate tasks — this issue is the spec).
- Redesigning LoCoMo. LoCoMo stays as-is.
- Replacing sonnet-4.5 as judge. Separate concern.
