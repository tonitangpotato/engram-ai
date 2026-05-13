---
title: v0.3 KC EmbeddingInfomapClusterer collapses to 1 super-cluster on dense single-domain corpora
status: open
priority: P1
severity: degradation
labels: [kc, v0.3, clustering, locomo, benchmark-regression]
relates_to: [ISS-106]
created: 2026-05-12
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

- [ ] Reproduce the 1-super-cluster collapse on conv-26 in a unit test (or integration test against a fixture corpus)
- [ ] Decide on a fix strategy: adaptive threshold, density-aware Infomap parameters, two-pass clustering, or fallback to a different algorithm for low-diversity corpora
- [ ] Implement the fix
- [ ] Verify on RUN-0026 fixture: `knowledge_topics` row count > 1 (target ~10-30 for conv-26)
- [ ] Re-run LoCoMo 152q with `compile_knowledge` enabled — J-score must recover to ≥ RUN-0025 baseline (0.559), ideally exceed it on Abstract category
- [ ] Add a regression test: ingest a dense single-domain fixture, assert clusterer produces > 1 cluster

## Out of scope

- v0.2 KC (already deprecated, see v04-unified-substrate design.md §4.16)
- The v04 substrate migration itself — clusterer is orthogonal to storage layer
- Cross-conversation clustering (multi-conv LoCoMo runs may not exhibit this — verify separately)

## References

- RUN-0026 archive: `engram-bench/benchmarks/runs/RUN-0026-*`
- RUN-0027 archive (rollback baseline): `engram-bench/benchmarks/runs/RUN-0027-*`
- v04 design §4.16.3 (feature debt): references this ISS as the clusterer-tuning track
- ISS-106 (parent integration ticket, kept open until this is fixed and the patch can land cleanly)
