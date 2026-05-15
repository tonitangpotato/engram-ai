---
title: v0.3 KC EmbeddingInfomapClusterer collapses to 1 super-cluster on dense single-domain corpora
status: in_review
priority: P1
severity: degradation
labels:
- kc
- v0.3
- clustering
- locomo
- benchmark-regression
relates_to:
- ISS-106
created: 2026-05-12
fixed_by: 7f41bf7
---

# Problem

The v0.3 Knowledge Compiler clusterer (`EmbeddingInfomapClusterer`, default `similarity_threshold=0.5`) degenerates into a **single super-cluster** when fed a dense, semantically homogeneous corpus (e.g. one LoCoMo conversation of ~440 episodes about two people).

This is not a hypothetical concern — it caused a **-22pp J-score regression** in RUN-0026 (LoCoMo 152q) compared to the pre-ISS-106-patch baseline (RUN-0025: J=0.559 → RUN-0026: J=0.342).

## Concrete evidence (RUN-0026, conv-26)

After ingesting conv-26 (441 episodes, single conversation between Caroline and Melanie) and running `Memory::compile_knowledge("default")`:

- `knowledge_topics` table: **1 row** (expected ≥ ~20 for a corpus of this size/length)
- `topic.title`: `"Caroline and Melanie's Journey..."` (one super-topic that matches everything)
- `topic.source_memories`: 441/441 (the topic absorbed the entire corpus)
- `topic.contributing_entities`: 0 (entity layer never populated under degenerate clustering)

## Downstream impact

During retrieval, this super-topic matches every **Abstract** sub-plan query (because it semantically covers the whole conversation). The fuse stage then over-weights this single topic and **squeezes Factual / Episodic candidates out of the top-K**, causing:

- Abstract category: appears to "improve" (super-topic always returns something)
- Factual / Episodic / Temporal: regress sharply
- Net J-score: **-22pp vs no-KC baseline**

This is why ISS-106's "compile_knowledge between ingest and query loops" patch, when applied in RUN-0026, made things *worse* instead of better — the patch was correct in intent (populate `knowledge_topics`), but the clusterer turned that population into a poison pill.

## Root cause hypothesis

`similarity_threshold=0.5` is too permissive for dense single-domain corpora. When all episodes are about the same two people in the same conversation, their embeddings cluster tightly above 0.5 cosine — Infomap then merges everything into one community.

Threshold was likely tuned on multi-domain general corpora where 0.5 produces ~10-50 clusters. LoCoMo (and any single-conversation benchmark) violates that assumption.

## Why this is split out from ISS-106

ISS-106 ("v0.3 KC integration into LoCoMo harness") is about wiring `compile_knowledge` into the benchmark flow. That wiring works correctly. The bug is one layer down, in the clusterer algorithm itself — orthogonal to the harness integration. Mixing them caused the working-memory misattribution caught during the 2026-05-12 verify task.

## Acceptance criteria

- [x] Reproduce the 1-super-cluster collapse on conv-26 in a unit test (or integration test against a fixture corpus)
- [x] Decide on a fix strategy: adaptive threshold, density-aware Infomap parameters, two-pass clustering, or fallback to a different algorithm for low-diversity corpora
- [x] Implement the fix
- [ ] Verify on RUN-0026 fixture: `knowledge_topics` row count > 1 (target ~10-30 for conv-26)
- [ ] Re-run LoCoMo 152q with `compile_knowledge` enabled — J-score must recover to ≥ RUN-0025 baseline (0.559), ideally exceed it on Abstract category
- [x] Add a regression test: ingest a dense single-domain fixture, assert clusterer produces > 1 cluster

## Out of scope

- v0.2 KC (already deprecated, see v04-unified-substrate design.md §4.16)
- The v04 substrate migration itself — clusterer is orthogonal to storage layer
- Cross-conversation clustering (multi-conv LoCoMo runs may not exhibit this — verify separately)

## References

- RUN-0026 archive: `engram-bench/benchmarks/runs/RUN-0026-*`
- RUN-0027 archive (rollback baseline): `engram-bench/benchmarks/runs/RUN-0027-*`
- v04 design §4.16.3 (feature debt): references this ISS as the clusterer-tuning track
- ISS-106 (parent integration ticket, kept open until this is fixed and the patch can land cleanly)

## Resolution (2026-05-15)

**Status:** Code fix shipped in commit `7f41bf7`. AC #4 (RUN-0026 fixture verification) and AC #5 (LoCoMo J-score recovery) deferred — see "Remaining" section below.

### Fix strategy chosen: Mutual k-NN edge filter

Replaced the global absolute-threshold edge strategy with **mutual k-nearest-neighbors** as the new default. An edge `(i, j)` exists iff `j` is among `i`'s top-k most-similar nodes **and** vice versa. Bounds each node's degree by `k` regardless of corpus density, so Infomap can find substructure even on homogeneous data.

Why this over the alternatives in the AC list:

- **Adaptive threshold**: still global, still collapses if the whole corpus passes the bar. Doesn't fix the K_n problem.
- **Density-aware Infomap parameters**: Infomap doesn't expose the knobs we'd need; the algorithm sees a clique and gives a clique answer.
- **Two-pass clustering**: extra complexity for the same effect mutual k-NN gives in one pass.
- **Fallback algorithm for low-diversity corpora**: requires a corpus-diversity oracle. We'd build the same kNN structure to measure diversity, so just use it for clustering directly.

Mutual k-NN is the standard fix for this exact failure mode in graph-clustering literature (e.g. scikit-learn's `connectivity='kneighbors'` Ward, UMAP's connectivity graph). Implementation in `MutualKnnEdges` (clusterer.rs).

### k sizing

New `KNeighbors` enum:
- `Auto`: `clamp(sqrt(n), 3, 10)` — sensible across corpus sizes (50 candidates → k=7; 1000 → k=10; 16 → k=4).
- `Fixed(usize)`: pinned k for advanced callers.

Default is `Some(KNeighbors::Auto)`. Legacy absolute-threshold mode preserved as `k_neighbors: None` for callers that explicitly want it (the two pre-existing struct-literal tests pin this).

### Verification done

- New test `iss111_dense_single_domain_collapses_to_one_supercluster` pins legacy mode, asserts 50 near-identical 16-D vectors collapse to exactly 1 cluster of size 50 — **passes**, confirming the bug.
- New test `iss111_dense_single_domain_does_not_collapse_after_fix` uses the new default (mutual k-NN), asserts the same fixture splits into > 1 cluster — **passes**, confirming the fix.
- Lib regression: 1904/1904 pass.
- Integration regression: synthesis_integration_test (7), v04_phase_c_backfill, v04_phase_c_backfill_atomicity — all green.

### Remaining (AC #4 + #5)

LoCoMo end-to-end verification needs either:

1. A recorded fixture from RUN-0026's ingest pass (441 OpenAI-embedded episodes), replayed offline through `compile_knowledge` to check `knowledge_topics` row count, OR
2. A live re-ingest of conv-26 + full 152q run, costing API budget.

Neither was done in this session. Synthetic-fixture evidence (50 near-identical 16-D vectors → splits cleanly) gives strong confidence the fix works on the underlying graph problem, but cannot prove the J-score number until real embeddings are exercised. Flagged to potato for the API-budget call.

**Tracking:** Reopen this issue or file a follow-up before the v0.4 `unified_substrate=true` default flip if the LoCoMo recovery hasn't been measured by then.
