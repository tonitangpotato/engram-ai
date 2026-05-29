---
id: ISS-190
title: Generation refuses temporal arithmetic — 'owned N years as of DATE' not derived to adoption year (conv-44-q29, conv-26-q71)
status: open
priority: P1
severity: degradation
tags:
- generation
- temporal-reasoning
- llm-behavior
- locomo
- single-fact
created: 2026-05-29
relates_to:
- ISS-189
- ISS-179
- ISS-148
- .gid/issues/ISS-189/issue.md
---

# ISS-190: Generation refuses temporal arithmetic even when the fact is in the prompt

> Split out of **ISS-189**. ISS-189 fixed the *recall* defect (the gold
> evidence episode now reaches generation). The residual failure on
> conv-44-q29 is **provably not retrieval** — the temporal fact is in the
> top-K context and the model still answers "I don't know". This is the
> "Generation failure (fact in top-K)" bucket that ISS-179's census flagged
> as `no lever filed`.

## The canonical failure: conv-44-q29

- **Query:** "Which year did Audrey adopt the first three of her dogs?"
- **Gold:** `2020`
- **Category:** single-hop. **Score:** 0.0 (baseline AND post-ISS-189).

### Why this is a generation defect, not a recall defect

After ISS-189 (commit `2ec7e3c`, traverse incoming edges + seed from their
memory_id), the gold evidence episode `a8b823f4` **does** reach generation.
SQL-verified two episodes carry the temporal fact:

- `a8b823f4` (gold, named dogs): *"Audrey has three pets named Pepper,
  Precious, and Panda that she has owned for 3 years as of March 2023."*
- `35ad6494` (paraphrase): *"multiple dogs all 3 years old."*

The **baseline** prediction already matched `35ad6494` — so the temporal
fact `owned for 3 years as of 2023` reached generation **before** ISS-189
too. Both pre- and post-fix predictions quote the "3 years as of March
2023" fact and then say **"I don't know"** the adoption year.

The model has every term it needs (`3 years`, `as of March 2023`) and
refuses to compute `2023 − 3 = 2020`.

## Root cause (D2 in the ISS-189 decomposition)

The answer-generation prompt does not instruct the model to perform
relative-to-absolute temporal derivation. When evidence states a duration
relative to a reference date ("owned N years as of DATE"), the model treats
the absolute answer (DATE − N) as "not stated" rather than derivable.

This is distinct from:
- **D0/D1 (ISS-189, FIXED):** answer episode never reached the pool.
- **temporal grounding (ISS-179 q76):** resolving "yesterday" → a date at
  *ingestion/extraction* time. ISS-190 is at *generation* time, deriving an
  absolute year from a stated duration + reference date.

## Acceptance criteria

- [ ] **AC-1** Reproduce: confirm conv-44-q29 post-fix prediction contains
      the "3 years as of March 2023" fact but answers "I don't know" /
      omits 2020. (Already observed in run
      `ISS189-fix-conv44-20260529T131853Z`.)
- [ ] **AC-2** Design a generation-prompt change that instructs the model to
      derive absolute dates from `duration + reference-date` evidence
      ("X for N years as of DATE" → start ≈ DATE − N), without
      hallucinating dates when no reference is present.
- [ ] **AC-3** A/B on conv-26 (locked envelope: K=10 temp=0 HyDE=off MMR=off
      entity_channel=off pipeline_pool=1). Arm A = current prompt, Arm B =
      temporal-derivation prompt. Target: q71 + any other duration-relative
      SF question flips 0→1 with **no regression** (regression rate ≤10%).
- [ ] **AC-4** Cross-validate on conv-44: q29 flips 0→1, overall ≥ baseline
      0.2439.
- [ ] **AC-5** Guard against the failure mode: when evidence has NO temporal
      reference, the model must still say "I don't know" rather than
      fabricate a year. Add a negative test case.

## Risk / scope notes

- **Prompt-only change** — no retrieval or schema work. Blast radius is the
  answer-generation prompt + bench A/B. Low risk relative to the falsified
  retrieval levers.
- This is one of the few remaining SF levers in ISS-179's census with a
  *tractable, in-pipeline* fix surface (vs BLIP / multi-hop / aggregation
  which need infrastructure). It also covers q71 ("Becoming Nicole").
- **Honesty flag:** unverified whether a prompt change alone is sufficient —
  the model may need the reference date made explicit by a pre-generation
  temporal-derivation step rather than relying on in-context arithmetic.
  AC-2 should evaluate both before committing.

## Evidence artifacts

- Run: `engram-bench/benchmarks/runs/ISS189-fix-conv44-20260529T131853Z/`
  (`locomo_per_query.jsonl` q29 row, `locomo_summary.json` overall 0.2439)
- Baseline: `engram-bench/benchmarks/runs/CONV44-baseline-20260529T060701Z/`
- ISS-189 issue.md (recall fix, root-cause decomposition D0/D1/D2/D3)

## Related

- **ISS-189** — fixed the recall half (incoming-edge traversal). This issue
  is the generation half that ISS-189 surfaced once recall was no longer the
  blocker.
- **ISS-179** — census that flagged "Generation failure (fact in top-K)" as
  an unfiled lever; this is that ticket.
- **ISS-148** — AC-5a single-fact ship gate.
