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

- [ ] AC-1: factual/episodic-plan candidates carry populated
  `embedding` (from Storage fallback) at the C.5 hook, gated by a
  serde-default-false config knob. Off = byte-identical to current.
- [ ] AC-2: unit test — factual plan with embedding-population ON +
  λ<1.0 reorders a synthetic redundant-cluster candidate set so a
  distant relevant item enters the head; OFF = unchanged.
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

## Status

Open 2026-05-29 — root fix identified by ISS-187 diagnostic.
Implementation not started.
