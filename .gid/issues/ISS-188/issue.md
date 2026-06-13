---
status: resolved
---
# .gid/issues/ISS-188/issue.md (issue)
project: engram
---
title: Populate factual/episodic-plan candidate embeddings so MMR diversity reranking works on list-questions
category: retrieval-foundation
discovered_in: ISS-187 resolved 2026-05-29 — drop_CD 22/32, SF-subset 10/13 LIST-type all scoring 0. Root cause = factual-plan candidates carry embedding:None so MMR gives 0 diversity penalty and degenerates to no-op.
priority: P0
severity: defect
status: open
relates: [engram:ISS-187, engram:ISS-186, engram:ISS-139]
---

## Why this issue exists

ISS-187 named the structural defect. List-questions (gold = "beach,
mountains, forest") fail because pure-relevance ranking stacks the
top-10 with one redundant semantic cluster (all mountains/forest),
pushing the other correct list items (beach) to rank 38-152. top-10
truncate drops them → LLM gives a partial answer → judge scores 0.

MMR (ISS-139) is exactly the fix for this — it breaks redundant top
clusters and surfaces relevant-but-distant items before truncation.
But `mmr.rs:58-70` documents that candidates with `embedding: None`
get a **0 diversity penalty**, so on the factual / episodic plans
(which carry no embeddings) MMR is a no-op. The diversity channel is
structurally dead on exactly the plans the list-questions route
through.

This is why ISS-139's λ-sweep saw no signal on factual plan, and why
9 relevance-tuning levers (ISS-159/164/175/178) all falsified:
list-questions don't lack relevance, they lack coverage, and the
coverage mechanism was inert.

## Root fix (NOT a K_seed bump)

Implement the "Future work" already noted in `mmr.rs:73-74`: an
opt-in Storage-backed embedding fallback that populates
`ScoredResult::Memory.embedding` for factual/episodic-plan
candidates before the C.5 MMR hook, so MMR can compute real
cosine-diversity on them.

Cheap K_seed/pool widening is explicitly rejected: the gold is
already in a 186-deep pool. Widening doesn't change ordering within
the pool; only diversity reranking does.

## Implementation surface (to be confirmed during impl)

- `crates/engramai/src/retrieval/fusion/mmr.rs` — diversity calc
  (already correct; just starved of embeddings).
- Stage C.5 hook in `crates/engramai/src/retrieval/api.rs:~895` —
  where MMR runs, post-fusion pre-truncate. Embedding population must
  happen BEFORE this point.
- `Storage::get_embeddings_for_ids` already exists (ISS-139 Strategy
  A wired it for the hybrid plan) — reuse for factual/episodic.
- Gate behind a config knob (default off) to preserve §5.4
  reproducibility envelope, same pattern as `mmr_lambda`.

## Acceptance criteria

- [x] AC-1: factual/episodic-plan candidates carry populated
  `embedding` (from Storage fallback) at the C.5 hook, gated by a
  serde-default-false config knob. Off = byte-identical to current.
  Shipped: `FusionConfig::populate_embeddings_for_diversity` (serde
  default false) + `GraphQuery::with_populate_embeddings_for_diversity`
  override + Stage C.4 backfill in api.rs before the cross-encoder/MMR
  hooks, via `mmr::populate_missing_embeddings` (pure fn, one batched
  `get_embeddings_for_ids` SQL round-trip).
- [x] AC-2: unit test — factual plan with embedding-population ON +
  λ<1.0 reorders a synthetic redundant-cluster candidate set so a
  distant relevant item enters the head; OFF = unchanged.
  Shipped: `populate_then_low_lambda_diversifies_previously_dead_cluster`
  (proves dead-channel baseline = relevance order, post-backfill
  λ=0.7 surfaces the diverse `car`) + 3 fn-contract tests
  (backfill / unreturned-ids-stay-None / no-overwrite) + 3 api builder
  tests. 26/26 mmr + 2027/2027 lib green.
- [ ] AC-3: λ-sweep on the **10 LIST-type SF queries** (q13/q15/q18/
  q19/q24/q32/q34/q38/q39/q47), NOT the diluted full conv-26 set.
  Find λ maximizing list coverage.
- [ ] AC-4: no regression on single-value SF queries (q4/q7/q43) and
  no regression on conv-26 overall vs ISS-161 Arm A baseline.
- [ ] AC-5: cross-validate the winning λ on conv-44 (inverted
  list/single ratio) to confirm corpus-general, not conv-26 artefact.

## Decision rule (the discipline that's been missing)

- list-SF coverage lift ≥ +3/10 AND no single-value regression AND
  conv-44 confirms → ship embedding population + winning λ as default.
- lift +1..+2/10 → opt-in only, keep default off.
- lift ≤0 → falsified; the partial-answer problem is in the JUDGE
  (penalizes incomplete lists) or GENERATION (LLM not synthesizing
  across retrieved items), not retrieval. Pivot to ISS-179 (SF axis
  redefinition) per its existing recommendation.

## AC-3 verdict — FALSIFIED (2026-05-29)

λ-sweep ran on conv-26, ISS-161 Arm A envelope (K=10, temp=0, HyDE off,
entity_channel off, FACTUAL_REWEIGHT off, pipeline_pool=1). STAMP
`20260529T041125Z`. Artifacts in `artifacts/ac3-sweep-verdict-*.txt`
plus the three sweep scripts.

LIST-type SF coverage (q13/15/18/19/24/32/34/38/39/47, pass ≥ 0.5):

  arm                          list-SF   overall
  A  populate=off (baseline)   2/10      0.237
  B  populate=on  λ=0.7        2/10      0.250
  C  populate=on  λ=0.5        3/10      0.217

Letter of the rule: Arm C lift = +1/10 → "opt-in only". But the +1 does
NOT survive scrutiny — it is judge/reorder wobble, not a structural fix:

- **Only q13 passes in all 3 arms (3/3).** Every other LIST-SF "pass"
  — q34, q38, q39, q47 — passes in exactly **1 of 3** arms. Each arm
  wins a *different* question and loses one it had:
    q34: 0/0/**1** (C only)   q38: 0/**1**/0 (B only)
    q39: 0/0/**1** (C only)   q47: **1**/0/0 (A only)
  If embedding population genuinely surfaced list content, gains would
  be consistent across the LIST set, not a shuffle.
- Overall score-flip A→C = 23/152 (15.1%), net **−3** (10 gained, 13
  lost). The "+1 list-SF" rides on top of an overall regression
  (0.217 < 0.237 baseline) — AC-4 no-regression guard **FAILS**.
- Single-value SF (q4/q7/q43) flat 1/3 across all arms — neither
  helped nor hurt.

**Conclusion:** feeding factual/episodic-plan candidate embeddings +
diversity reranking (λ=0.7 and λ=0.5) does NOT lift list-question
recall. Effective lift ≤ 0 once wobble is discounted. Per the decision
rule, the partial-answer problem is in the **JUDGE** (penalizes
incomplete lists) or **GENERATION** (LLM not synthesizing across
retrieved items), **not retrieval**. ISS-187's mechanism (MMR diversity
dead on factual plan) was real, but fixing it does not move the gold —
which means retrieval was never the binding constraint for these
list-questions. Pivot to ISS-179 (SF axis redefinition).

AC-5 (conv-44 cross-validation) **skipped** — no winning λ to validate.

## Disposition of the code

AC-1/AC-2 code (commit c683cc0) stays in tree but **default OFF**
(`FusionConfig::populate_embeddings_for_diversity = false`, serde
default false). It is inert at the locked v0.3 default (byte-identical),
fully tested (26/26 mmr + 2027/2027 lib green), and available as an
opt-in knob should a future SF redefinition (ISS-179) make diversity
reranking relevant again. No revert — the mechanism is correct, the
hypothesis that it would help list-questions is what was falsified.

## ACs

- [x] AC-1: code (Stage C.4 populate + serde-default-false knob) — c683cc0
- [x] AC-2: tests (26/26 mmr + 2027/2027 lib, 0 warnings) — c683cc0
- [x] AC-3: λ-sweep run — lift ≤0 after wobble discount → FALSIFIED
- [x] AC-4: no-regression check — FAILED (Arm C −3 overall)
- [~] AC-5: conv-44 cross-validation — SKIPPED (no winning λ)

## Status

**resolved (falsified) 2026-05-29.** AC-3 λ-sweep shows no real lift on
list-questions; bottleneck is judge/generation, not retrieval. Code
kept default-off as opt-in. Next: ISS-179 (SF axis redefinition).


---

## ⚠️ INVALIDATED measurement — dead vector channel (ISS-222, annotated 2026-06-13 per ISS-223 AC-5)

The populate-embeddings λ=0.7/0.5 arms in this issue reported **lift 0** — now explained: the vector channel was dead (empty-`memories` JOIN bug, T32→ISS-222), so the MMR diversity term had no embeddings to diversify against in the Factual path. The "lift 0" was a measurement artifact, not a real null result. **Do not cite the λ arms here as evidence about MMR.**

Corrected measurement: **ISS-223** (live channel). Verdict: λ<1.0 falsified on cross-validation, default `mmr_lambda` stays 1.0.
