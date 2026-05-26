---
title: conv-26 single-hop list-question failures — corpus-shape-specific retrieval blockage
status: open
priority: P3
severity: design
category: bench
created: 2026-05-25
updated: 2026-05-25
relates:
- ISS-148
- ISS-159
depends_on: ''
---

## ⚠️ 2026-05-25 — AC-1 FALSIFIED, issue reframed

The original framing ("list-question failures are generation/judge, not
retrieval") **did not generalise**. AC-1 reproduction probe on conv-44
(inverted ratio: 13 list / 17 single-fact) shows the K-invariance claim
was conv-26-specific:

| bucket | conv-26 K=10→K=30 | conv-44 K=10→K=30 |
|---|---|---|
| list questions | +0.0pp (4/20 → 4/20) | **+7.7pp** (1/13 → 2/13) |
| single-fact | +16.7pp (3/12 → 5/12) | **+23.5pp** (6/17 → 10/17) |

**What this falsifies:** "list failures are retrieval-immune" is false
on conv-44 — K-expansion does lift the list bucket there.

**What survives:** the single-fact bucket lift reproduced (and is
larger on conv-44). ISS-159 weapon A thesis is **strengthened**, not
weakened. That work continues; it's not gated on this issue.

**The real failure mode (revised):** dense single-domain two-person
chat corpora (conv-26's shape — Caroline/Melanie ubiquitous, every
episode is one of two speakers) create a **retrieval pathology** for
list questions: gold needle episodes get crowded out by abundant
near-duplicate chatter from the same speakers. The pathology is a
**corpus-shape × retrieval-strategy interaction**, not a
generation/judge issue.

Implication: this is no longer an AC-5b blocker for ISS-148. It's
documented corpus shape sensitivity worth understanding, but action
on it is **deferred** behind weapon A landing.

P2 → P3. Status stays open as research debt.

Artifacts: `artifacts/conv44-K{10,30}-{summary.json,per-query.jsonl}`,
`artifacts/iss160_repro_conv44.sh`.

---

## Original framing (preserved for history — but see falsification above)

### TL;DR

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

- [x] **AC-1:** ~~Reproduce the bucketing on a 2nd conversation (not just
      conv-26) — verify list/single-fact ratio and 0-pp-K-expansion-on-list
      generalises. If not, this issue's scope is conv-26-specific.~~
      **FAILED 2026-05-25 on conv-44.** List bucket moved +7.7pp with K,
      not 0.0pp. The K-invariance claim is conv-26-specific. Issue reframed
      to corpus-shape-specific retrieval pathology (see top of file).
- [ ] **AC-2 (deferred):** Try generation-prompt variant (lever 1) on
      conv-26 list bucket. Demoted from gate to research probe — no
      longer load-bearing for ISS-148 AC-5b.
- [ ] **AC-3 (deferred):** Judge partial-credit / annotation review
      (levers 3 + 4). Same reasoning.
- [ ] **AC-4 (revised):** Document the corpus-shape dependency. What
      property of conv-26 (two-person density? episode count?
      speaker-tag uniformity?) blocks K-expansion on list bucket while
      conv-44 doesn't? Cheap to investigate, but not blocking.

## Out of scope

- Cross-encoder reranker (ISS-159 / weapon A) — won't help conv-26 list
  bucket per the original deep-read, but **does** help single-fact
  bucket on both conv-26 (+16.7pp at K=30 baseline) and conv-44
  (+23.5pp at K=30 baseline). ISS-159 proceeds independently.
- Retrieval pipeline changes for general list questions — conv-44
  evidence shows K-expansion does work outside the conv-26 corpus shape.

## References

- ISS-148 deep-read section (2026-05-25)
- ISS-149 K-expansion probe artifacts
- engram-bench: `src/scorers/locomo_judge.rs:render_generate_prompt`
- engram-bench: `src/scorers/locomo_judge.rs:judge_answer`
