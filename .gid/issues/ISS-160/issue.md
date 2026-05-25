---
title: LoCoMo single-hop list-question failure mode — generation/judge, not retrieval
status: open
priority: P2
severity: design
category: bench
created: 2026-05-25
relates:
- ISS-148
- ISS-159
depends_on: ''
---

## TL;DR

Half of conv-26 single-hop failures (16/32) are **list questions** where
the gold answer is a multi-item set ("pottery, camping, painting,
swimming") and the failure mode is **incomplete enumeration in the
generated answer**, not retrieval miss. K=10 vs K=30 changes nothing
on this bucket (both 4/20 = 0.20). This is split out from ISS-148 to
keep that issue focused on the retrieval-layer fixes ISS-159 controls.

## Evidence (from ISS-148 deep-read, 2026-05-25)

Bucketed all 32 conv-26 single-hop questions:

| sub-bucket | gold shape | K=10 | K=30 |
|---|---|---|---|
| **list** (this issue) | multi-item: "X, Y, Z" or '"A", "B"' | 4/20 = 0.200 | 4/20 = 0.200 |
| **single-fact** (ISS-159 target) | one specific value | 3/12 = 0.250 | 5/12 = 0.417 |

K-expansion lifted single-fact +16.7pp; lifted list **0.0pp**. This
isolates the failure mode cleanly.

### Two failure modes inside the list bucket

**Mode A — gold items live in 1-of-N episodes (retrieval-adjacent but
not retrieval-fixable in practice):**

- q52 "pets" gold="Oliver, Luna, Bailey" — Oliver 4 episodes, Luna 1
  episode, Bailey 1 episode (out of 419). Predicted: "Luna, Oliver".
  Model correctly enumerated the items it had context for; the third
  needle wasn't surfaced.

**Mode B — gold list is not unique; multiple valid lists exist
(annotation subjectivity):**

- q15 "activities Melanie partakes in" gold="pottery, camping,
  painting, swimming". Predicted: "hiking, pottery, running". All three
  predicted items ARE Melanie's real activities in the corpus, just a
  different valid subset than the annotator chose. Judge marks the
  entire answer wrong because it's not a strict-equality match.

The two modes are inseparable inside the current judge design.

## Why this is generation/judge, not retrieval

1. K=10 → K=30 lifts list bucket exactly 0.0pp. More candidates ≠
   better list enumeration.
2. The model's predictions *are* lists. The issue is **completeness**,
   not whether it knows to enumerate.
3. The judge uses strict-superset (any missing gold item → No).
4. Mode B (gold-non-unique) is fundamentally a dataset annotation
   issue; even an oracle retriever can't fix it.

## Levers to investigate

1. **Generation prompt change** — current prompt ends with "Answer (be
   concise, just the key fact):". On list-shaped queries, this biases
   toward terse single-item answers. Try a variant: "If the question
   asks for multiple items, enumerate all you can find."
2. **List-question detection + K=100 for list queries only** — for the
   subset of queries that look like enumerations ("what X has Y done?"),
   widen the candidate pool aggressively. Cheap to try.
3. **Judge partial-credit scoring** — current judge: binary Yes/No on
   strict-equality. Try: weighted F1 over gold-item set. This is a
   dataset/judge contract change, separate sub-task.
4. **Annotation review** — for Mode B cases, the dataset has multiple
   valid answers but only one is annotated. Either drop those questions
   or expand `gold` to a set of acceptable answers.

## Acceptance criteria

- [ ] **AC-1:** Reproduce the bucketing on a 2nd conversation (not just
      conv-26) — verify list/single-fact ratio and 0-pp-K-expansion-on-list
      generalises. If not, this issue's scope is conv-26-specific.
- [ ] **AC-2:** Try generation-prompt variant (lever 1) on conv-26 list
      bucket. Measure: list pass-rate on K=30 with new prompt. Target
      ≥ 6/20 = 0.30 (current 0.20).
- [ ] **AC-3:** Decide on judge partial-credit scoring or annotation
      review (levers 3 + 4). Out-of-scope to *implement* in this issue;
      in-scope to document the trade-off.
- [ ] **AC-4:** Document final list-bucket ceiling on conv-26 after
      levers 1-3 — this is the realistic upper bound for AC-5b in
      ISS-148.

## Out of scope

- Cross-encoder reranker (ISS-159 / weapon A) — won't help list bucket.
- Retrieval pipeline changes (BM25 tuning, embedder swap, etc.) — proven
  not to move this bucket.

## References

- ISS-148 deep-read section (2026-05-25)
- ISS-149 K-expansion probe artifacts
- engram-bench: `src/scorers/locomo_judge.rs:render_generate_prompt`
- engram-bench: `src/scorers/locomo_judge.rs:judge_answer`
