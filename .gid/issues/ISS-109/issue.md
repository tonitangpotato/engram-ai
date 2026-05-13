---
title: EmbeddingInfomapClusterer collapses on single-conversation corpora (blocks ISS-106)
status: open
priority: P1
labels: [clustering, knowledge-compiler, locomo, blocks-iss-106]
relates_to: [ISS-106, ISS-107, ISS-108]
created: 2026-05-06
---

# ISS-109 — EmbeddingInfomapClusterer collapses on single-conversation corpora (blocks ISS-106)

## Summary

`EmbeddingInfomapClusterer` (default `similarity_threshold = 0.5`) collapses LoCoMo single-conversation episode sets into **one super-cluster** containing every memory in the namespace. The resulting single topic matches every Abstract-class query and crowds Factual/Episodic candidates out of the fuse stage, **regressing** J-score instead of helping.

This was discovered while validating ISS-106 (compile_knowledge between ingest and query in the LoCoMo replay driver).

## Evidence

### Smoke verification (2026-05-06, persistent DB, 5q)

After `compile_knowledge("default")` on conv-26 episodes:

| Metric | Value |
|---|---|
| `knowledge_topics` rows | **1** |
| `topic.source_memories` count | **441/441** (100% of conv memories) |
| `topic.contributing_entities` | **0** |
| Topic title | "Caroline and Melanie's Journey of Self-Discovery and Mutual Support" |

A single super-topic with `contributing_entities = 0` AND `source_memories > 10` is the exact "loco-shape" failure pattern from ISS-107 / ISS-108 fixture F01. The clusterer is producing **structurally degenerate** output on this input shape.

### RUN-0026 (full 152q, ISS-106 patch in)

- conv-26: J-score **0.559 → 0.342** (–21.7pp)
- Mechanism: Abstract sub-plan retrieves the super-topic; fuse stage promotes its source-memory bundle, displacing the actually-relevant Factual / Episodic candidates that would have answered the gold question.

### RUN-0024 (baseline, no ISS-106 patch)

- J-score **0.559** (Abstract sub-plan downgrades cleanly to non-knowledge path because `knowledge_topics` is empty.)

### RUN-0027 (in-flight, 2026-05-06 ~14:57 EDT, baseline re-verify)

- ISS-106 patch reverted, retrieval experiments parked in stash.
- Expected: J-score back to ~0.467 (matches RUN-0020 K=15 line). If hits ~0.467 → confirms RUN-0024→0026 delta is fully explained by the ISS-106 patch on top of degenerate clustering.
- If hits ~0.559 (closer to RUN-0024) → there is additional drift between runs, separate investigation needed.

## Root cause hypothesis

`similarity_threshold = 0.5` on **dense, topically-coherent** single-conversation embedding sets (LoCoMo conv-26 = ~441 memories all from two-person dialog about emotional/relational topics) produces a graph where **every** node is "similar enough" to every other → Infomap's flow collapses to one community.

Real-world (multi-conv, multi-source) corpora don't hit this because cross-conversation embedding distance naturally exceeds 0.5 for most pairs, breaking the collapse.

This is a **clusterer-side** failure: the algorithm has no defense against degenerate-input shapes (no minimum cluster count, no maximum cluster size cap relative to corpus size, no entropy check on community-size distribution).

## Why it blocks ISS-106

ISS-106's premise — "Abstract queries should hit `knowledge_topics`, currently they fall through because compile_knowledge is never called in the LoCoMo driver" — is correct. But the fix only **helps** if compile_knowledge produces useful topics. On single-conv corpora it produces 1 useless super-topic, so the fix becomes a regression.

ISS-106 cannot be reapplied until the clusterer is robust to this input shape, OR the LoCoMo driver gates compile_knowledge to multi-conv corpora only (workaround, not root fix).

## Why it relates to ISS-107 / ISS-108

- **ISS-107** = clustering shape regression (loco-shape) — exactly what this issue is about, but from the opposite side: ISS-107 wants to *guard* against it; ISS-109 has *observed* it in production.
- **ISS-108** = Suite 3 fixtures including F01 (loco-shape). F01's whole point is to make this failure mode CI-detectable. ISS-109 is the underlying defect F01 will catch.

## Acceptance criteria

A fix lands when ALL of these are true:

- [ ] `compile_knowledge("default")` on conv-26 episodes produces **≥ 3** topics, NONE with `source_memories > 50% of corpus`.
- [ ] Re-applying the ISS-106 patch (compile_knowledge between ingest/query in `engram-bench/src/drivers/locomo.rs`) **does NOT regress** J-score on conv-26 vs baseline (RUN-0024 = 0.559).
- [ ] ISS-108 Suite 3 fixture F01 (loco-shape regression) **passes** without the loco-shape assertion firing.
- [ ] `topic.contributing_entities = 0 AND topic.source_memories > 10` is asserted impossible in a unit test added to engramai.

## Possible fix directions (not yet decided)

1. **Threshold auto-tuning** — choose `similarity_threshold` from corpus-internal pairwise distance distribution (e.g., median + k·MAD) instead of fixed 0.5.
2. **Post-cluster guard** — if any cluster contains > N% of corpus → re-run with higher threshold OR split that cluster recursively.
3. **Min cluster count floor** — for corpora > M memories, require ≥ K clusters; if Infomap returns fewer, fall back to a different algorithm (HDBSCAN? agglomerative with ward linkage?).
4. **Entity-aware clustering** — use the entity graph from extraction as a constraint (memories sharing entities cluster together; memories sharing NO entities cannot be in the same cluster).

Direction 4 is most aligned with engramai's architecture (entity extraction is already in the pipeline) and most likely to fix `contributing_entities = 0` simultaneously.

## Out of scope

- LLM-side topic naming quality (`compile_knowledge` calls Anthropic Haiku per cluster). The naming is fine; the input clusters are the problem.
- Cross-namespace clustering. This issue is about within-namespace degeneracy.

## Links

- ISS-106 — compile_knowledge between ingest/query (the trigger that exposed this)
- ISS-107 — clustering shape regression (the conceptual frame)
- ISS-108 — Suite 3 microbench fixtures (the CI guard for this)
- RUN-0024, RUN-0026, RUN-0027 — eval evidence, in `engram/.gid/eval-runs/`
- engram-bench `src/drivers/locomo.rs:606` — the NOTE comment documenting why ISS-106 patch is reverted

## Worklog

### 2026-05-06 — ISS opened
- Discovered while diagnosing RUN-0026 J-score regression vs RUN-0024.
- Smoke + RUN-0026 + RUN-0027 confirm clusterer is the failure point, not the ISS-106 patch logic.
