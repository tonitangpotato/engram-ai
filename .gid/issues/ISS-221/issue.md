---
title: 'PPR ablation arm: HippoRAG2-style Personalized PageRank over unified entity+memory graph to replace BFS+fixed-weight fusion ranking'
status: open
priority: P1
severity: major
labels: [v04-unified-substrate, locomo, retrieval, ranking, ppr, ablation]
feature: v04-unified-substrate
created: 2026-06-11
relates_to: [ISS-201, ISS-159, ISS-186]
depends_on: [ISS-203]
---

# Summary

Add an **ablation arm** that ranks retrieval candidates with
**Personalized PageRank (PPR)** over the unified `nodes`/`edges` graph
(entity + memory nodes jointly), HippoRAG2-style, instead of the current
BFS 1-hop traversal + fixed-weight channel fusion
(`retrieval/plans/associative.rs` weights 0.40/0.35/0.25).

Direction confirmed by potato 2026-06-11 ("对的就是PPR") after the ISS-201
lever-1/lever-2 results showed depth-widening exhausts its gains: CE
k_in=250 lifted conv-26 to 0.5197, but the residual A-bucket
("ranked-out": gold in pool, CE can't discriminate at depth — q3/q11
family) needs a **structural ranking signal**, not more depth.

# Why PPR (mapping HippoRAG2 → engram)

HippoRAG2 (arxiv 2502.14802) mechanisms and where they land here:

1. **PPR over passage+entity joint graph** — engram's unified substrate
   already stores entities AND memories as `nodes` with typed `edges`
   (structural entity↔entity + provenance/mentions edges with
   `source_memory_id` populated since ISS-202). PPR personalization vector
   seeds from query-resolved anchor entities (+ optionally top vector-hit
   memory nodes); steady-state scores rank memory nodes by multi-hop graph
   proximity — replacing the hand-tuned 1-hop BFS bonus.
2. **Passage integration** — memory nodes participate as first-class graph
   nodes, so dense-retrieval evidence and graph evidence mix inside the
   random walk instead of post-hoc weight fusion.
3. **Recognition-memory filter** (follow-on, separate scope) — LLM filters
   retrieved candidates before generation; targets the ~35q generation
   bucket (IDK over-caution). NOT in this issue's scope; file separately
   if PPR lands.

# Targets

- Residual ranked-out misses after lever 1+2 (see ISS-201 "Residual
  misses" section): q3/q11 family — gold in 250-deep pool, CE rank
  insufficient.
- single-hop category (0.4375 after lever-2; PPR's multi-hop entity
  evidence aggregation is HippoRAG2's main reported single-hop+multi-hop
  win).
- Possibly some true-pool-miss qids where gold memory is graph-reachable
  from anchors but never surfaces in vector/BM25 channels (PPR adds a
  channel that doesn't depend on text similarity).

# Design questions (resolve before implementation)

1. **Channel vs replacement**: PPR score as a new fusion channel alongside
   vector/BM25 (safer, A/B-able via weight=0) vs full replacement of
   `graph_score` in Associative/Hybrid plans (cleaner, riskier). Lean:
   start as channel with config knob, ablate.
2. **Personalization seeds**: anchor entities only, or anchors + top-k
   vector memory hits (HippoRAG2 does both)?
3. **Damping/iteration budget**: graph is small (conv-26 ~22k nodes);
   exact power iteration fine. Pick α (0.5 HippoRAG default vs 0.85
   classic) by sweep.
4. **Edge weights**: uniform vs edge-kind-weighted (structural vs
   provenance vs associative) vs confidence-weighted.

# Prerequisite — ISS-203 (HARD BLOCKER)

PPR on a fragmented entity graph measures the fragmentation, not the
algorithm. Known defects that MUST be fixed first:

- `caroline` / `Caroline` split (two separate nodes, probabilistic merge
  failed at 0.85 threshold)
- ~20 phrase-entities per person ("Caroline's art", "support from
  Caroline") never stripped to head noun nor linked — these strand PPR
  mass on junk nodes and disconnect gold memories from anchors.

ISS-202 (source_memory_id on structural edges) is already fixed.

# Acceptance criteria

- [ ] AC-1: PPR ranking implemented behind config knob (default off),
      byte-identical retrieval when off.
- [ ] AC-2: Unit tests — PPR convergence, seed handling, disconnected
      components, deterministic given seed set.
- [ ] AC-3: Ablation A/B on conv-26, P2+CE envelope (INGEST_WINDOW=4,
      K=10, FACTUAL_REWEIGHT=on, CE=1, k_in=250): arm A = lever-2 config,
      arm B = +PPR. Report per-category deltas + flips.
- [ ] AC-4: Specific probe on residual ranked-out qids (q3/q11 family):
      does gold's final rank improve?
- [ ] AC-5: Cross-validate on conv-44 (no regression beyond wobble).
- [ ] AC-6: Decision recorded: channel weight / default on-off / drop.

# Bench context (2026-06-11)

- ISS201-LEVER2 overall **0.5197** (single-hop 0.4375, temporal 0.671,
  open 0.462, multi 0.324) — current best, the baseline this arm must
  beat.
- Envelope: conv-26, INGEST_WINDOW=4, TOP_K=10, FACTUAL_REWEIGHT=on,
  HyDE/MMR/entity off, PIPELINE_POOL=1, CE=1, **CE_K_IN=250**.
