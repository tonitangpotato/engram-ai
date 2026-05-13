---
id: ISS-107
title: Knowledge clustering collapses on single-conversation corpora (LoCoMo conv-26 441/441 → 1 super-topic)
kind: issue
status: todo
priority: high
labels: [knowledge-compile, clustering, retrieval, root-cause]
relates_to: [ISS-106]
---

# ISS-107: Knowledge clustering collapses on single-conversation corpora

## TL;DR

`Memory::compile_knowledge()` produces **one super-topic covering 441/441
memories** when run on LoCoMo conv-26. The single topic poisons the
Abstract sub-plan: it matches every query, crowds Factual/Episodic
candidates out of the fuse stage, and J-score drops by ~22pp.

This is the root reason ISS-106's patch (insert `compile_knowledge` into
the bench driver) **regressed J-score 0.559 → 0.342** in RUN-0026
instead of improving it. ISS-106 is now blocked on this issue.

## Evidence (RUN-0026 substrate.db, verified 2026-05-06)

```text
$ sqlite3 /var/folders/.../tmp7RP9O3/substrate.db
sqlite> SELECT COUNT(*) FROM memories;
452
sqlite> SELECT COUNT(*) FROM knowledge_topics;
1
sqlite> SELECT title, json_array_length(source_memories), contributing_entities
        FROM knowledge_topics;
"Caroline and Melanie's Journey of Self-Discovery and Mutual Support" | 441 | 0
```

- 441 of 452 memories collapse into one cluster.
- The remaining 11 memories were not clustered at all.
- `contributing_entities` = 0 — entity extraction produced nothing useful.

## Mechanism

`crates/engramai/src/knowledge/clusterer.rs::EmbeddingInfomapClusterer`:

- `similarity_threshold = 0.5` (default)
- LoCoMo conv-26 = single dialog between Caroline + Melanie over weeks
- All memories share dense semantic context → pairwise embedding cos-sim
  ≥ 0.5 across nearly all pairs
- Infomap sees a fully-connected graph → emits one community
- LLM summarizer compresses 441 memories into one abstract title

The configuration is **not wrong for cross-conversation corpora** (where
0.5 is reasonable to cluster related memories across distinct
contexts), but on a single long dialog it's pathological.

## Why a J-score regression, not just useless clustering

Once the super-topic exists, the Abstract sub-plan returns it as a
candidate for **every** query (it semantically matches everything
because its summary is universal). In the fuse stage:

- Before patch (RUN-0024): Abstract returns
  `DowngradedL5Unavailable` → fuse runs on Factual + Episodic +
  Affective → J = 0.559
- After patch (RUN-0026): Abstract returns the super-topic →
  fuse-rank promotes it → it occupies a slot that previously held a
  specific Factual or Episodic memory → J = 0.342

Net: −33 queries flipped correct → wrong. All 4 categories regress.

## Why this matters beyond LoCoMo

LoCoMo is a benchmark, but the failure mode generalizes to any
real-world data shape:

- A user's daily journal (one author, continuous narrative)
- An Agent's session log (one agent + one user, long arc)
- A single Slack channel's history

These are *exactly* the data shapes engram targets in production. If
clustering collapses on them, knowledge_compile is broken for the
primary use case, not just for a specific benchmark.

## Investigation needed

1. **Confirm threshold sensitivity.** Re-run `compile_knowledge` on
   conv-26 substrate.db with `similarity_threshold ∈ {0.6, 0.7, 0.75,
   0.8, 0.85, 0.9}`. Plot cluster count vs threshold. Find the elbow.

2. **Check whether higher thresholds give *useful* clusters or just
   *more* clusters.** Eyeball top topic titles at each threshold —
   does 0.85 produce "Career change", "Relationship support",
   "Mental health" (useful) or "Cluster A", "Cluster B", "Cluster C"
   of equally vague summaries (not useful)?

3. **Try different clustering algorithms** on the same memory graph:
   - HDBSCAN with min_cluster_size=5
   - Agglomerative with average-linkage + cut at distance N
   - Topic modeling (BERTopic-style) instead of graph-based
   Compare cluster quality on the same eyeball test.

4. **Decide: data-aware threshold vs algorithm change vs both.**
   - Data-aware: scale `similarity_threshold` based on graph density
     (if avg-cos-sim > 0.4, raise threshold)
   - Algorithm change: replace EmbeddingInfomap with something that
     handles dense graphs better
   - Both: probably the right answer

## Out of scope for this issue

- **Don't tune for LoCoMo.** The goal is "clustering produces
  meaningful groups on real data shapes," not "RUN-0027 beats
  RUN-0024 by tuning threshold to 0.85". If we tune to LoCoMo we'll
  break production. Verification needs an independent micro-benchmark
  (see ISS-NEW micro-bench, separate issue).
- **ISS-106 is blocked here** but stays open — once clustering is
  fixed, ISS-106's compile_knowledge insertion in the bench driver
  becomes valid again.

## Acceptance criteria

- [ ] Document mechanism with reproducer (script that loads conv-26
      substrate, runs compile_knowledge, prints cluster count + top
      titles)
- [ ] Run threshold sweep (6 values) + plot
- [ ] Compare ≥1 alternative algorithm on same data
- [ ] Decide on fix direction (data-aware threshold / new algorithm /
      both) with written justification
- [ ] Implement + add unit test that asserts: "single-conv corpus of
      ≥100 memories produces ≥3 clusters with avg cluster size
      ≤ 0.4 × total_memories"
- [ ] Verify on conv-26: re-run `compile_knowledge` after fix,
      confirm cluster count > 5 with thematically distinct titles
- [ ] Unblock ISS-106: re-apply bench driver patch + run RUN-0027,
      confirm J-score ≥ RUN-0024 baseline (0.559)

## Discovered

While investigating ISS-106 RUN-0026 regression — see
`engram/.gid/eval-runs/RUN-0026-iss106-investigation.md` for full
forensic trace and `engram/.gid/eval-runs/RUN-0026-substrate-evidence.md`
for the substrate.db queries that revealed this.

## Related

- ISS-106 (blocked by this) — Abstract sub-plan dead code in bench driver
- (forthcoming) ISS-NEW micro-bench — needed to verify clustering
  fixes without LoCoMo overfit risk
