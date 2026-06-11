---
title: 'PPR ablation arm: HippoRAG2-style Personalized PageRank over unified entity+memory graph to replace BFS+fixed-weight fusion ranking'
status: open
priority: P1
severity: major
labels: [v04-unified-substrate, locomo, retrieval, ranking, ppr, ablation]
feature: v04-unified-substrate
created: 2026-06-11
relates_to: [ISS-201, ISS-159, ISS-186]
depends_on: [ISS-209]
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

# Prerequisite — canonicalization status (updated 2026-06-11)

PPR on a fragmented entity graph measures the fragmentation, not the
algorithm. Status of the known defects:

- ✅ **`caroline`/`Caroline` base-name split — FIXED & VERIFIED**
  (ISS-209, root fix ce9075fd `graph::canonical_entity_id`). Verified
  2026-06-11 by SQL on the LEVER2 run's substrate DB: 1 node, 47
  occurred_on edges, zero duplicate base-names across 778 entity nodes.
  (Original filing wrongly pointed at ISS-203, which was already
  resolved/falsified — the live carrier was ISS-209.)
- ⚠️ **Phrase-entity fragmentation — STILL OPEN** (ISS-203 defect (b)):
  ~45 `Caroline's X` nodes on the verified conv-26 graph ("Caroline's
  art", "Caroline's journey", ...). These strand PPR mass on junk nodes.
  No working fix exists (V2 extraction prompt was falsified). Options:
  fix before the PPR arm, OR run PPR anyway and let the ablation
  quantify how much fragmentation costs — decide at design time.

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

# Design (2026-06-11, decided with potato: option (a) fusion channel)

## Decisions

- **D1 — Integration shape: new fusion channel**, NOT graph_score
  replacement. `SubScores` (api.rs:576) gains `ppr_score: Option<f64>`;
  `FusionWeights` (combiner.rs:67) gains `ppr: f64` (default **0.0** =
  inert; `combine()`'s live-weight renormalization makes a missing/
  zero-weight channel byte-identical — same pattern every prior knob
  used). Replacement of graph_score becomes a follow-up only if the
  channel shows lift.
- **D2 — Defect (b) measure-first**: run PPR on the current graph
  (phrase-entity nodes included). The ablation itself quantifies the
  dilution; do NOT build a head-noun-stripping fix speculatively.
- **D3 — Algorithm**: standard PPR power iteration, damping `d = 0.85`,
  `tol = 1e-6`, `max_iters = 50`. Personalization vector: uniform over
  query-resolved **anchor entity nodes** (same anchors the Factual/
  Associative plans already resolve). Phase 1 does NOT seed memory
  nodes from vector hits (HippoRAG2's passage seeding) — keep the arm
  minimal; note as phase-2 knob.
- **D4 — Graph**: unified `nodes`/`edges`, entity + memory nodes
  jointly, **undirected** walk (edge direction is extraction artifact,
  not semantic for proximity), uniform edge weights across edge kinds
  in phase 1 (kind-weighting = phase-2 knob). conv-26 scale ≈ 1.2k
  nodes / few-k edges → power iteration is sub-ms; load adjacency once
  per query after anchor resolution.
- **D5 — Score extraction**: candidates are memory nodes; a candidate's
  `ppr_score` = its steady-state mass **max-normalized within the
  candidate pool** → [0,1]. Memories absent from the graph get `None`
  (renormalization handles it; no penalty-by-zero).
- **D6 — Determinism**: iterate nodes in sorted-id order; fixed seeds →
  bit-identical output (AC-2).
- **D7 — Config**: `FusionConfig.ppr_weight` (serde-default 0.0) +
  `GraphQuery::with_ppr_weight` override + bench env
  `ENGRAM_BENCH_PPR_WEIGHT`. Initial arm-B weight: **0.15** carved
  proportionally from existing Associative/Factual weights (exact split
  recorded in the bench script, not hardcoded as a new matrix).

## Known risk — CE masking (record up front)

The envelope runs CE with k_in=250, and CE **overwrites** fused scores
for the head it rescores. So a pre-CE fusion channel mostly affects
*which* candidates enter the CE head, not the final top-10 order when
pool ≤ 250. If arm B shows ~0 delta, that is NOT proof PPR carries no
signal — it may be CE masking. Mitigation already planned as sub-arm
**B2**: blend post-CE (`final = α·CE + (1−α)·PPR_norm`, α=0.7) behind
`ENGRAM_BENCH_PPR_CE_BLEND`. Run B1 (fusion channel) first; only run B2
if B1 is flat AND per-query inspection shows gold's *fused* rank
improved but CE re-buried it.

## Module plan

- `retrieval/ppr.rs` — pure: `fn personalized_pagerank(adj: &Adjacency,
  seeds: &[NodeId], cfg: &PprConfig) -> HashMap<NodeId, f64>` + unit
  tests (convergence, dangling nodes, disconnected components,
  determinism, empty seeds → None short-circuit).
- `graph/store.rs` — `load_adjacency(namespace)` read helper (one SQL
  pass over `edges`, both endpoints; reuse existing read patterns).
- Wire point: orchestrator, after anchor resolution, before plan
  fusion — compute scores map once, thread into plan→`SubScores`
  annotation. Plans without anchors (Abstract/Affective) skip entirely.
