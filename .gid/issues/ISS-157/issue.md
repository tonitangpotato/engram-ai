---
title: AC-5 weapon selection — lift single-hop conv-26 0.22 → 0.40 (design)
status: open
priority: P1
severity: design
category: retrieval
created: 2026-05-25
relates:
- ISS-148
- ISS-145
- ISS-149
- ISS-150
- ISS-156
depends_on: ''
---

## TL;DR

ISS-148 AC-5 target: **single-hop ≥ 0.40 on conv-26 K=10 MMR 0.7**. Current
state after ISS-147 (BM25 wired) + ISS-150 (BM25 in Associative) + ISS-156
(per-category HyDE) + ISS-155 (extractor temp=0):

- single-hop: **0.2188** (post-fix substrate, ISS-156 PerCategory arm)
- Gap: **+18.12pp** to AC-5 target

HyDE is no longer the lever (ISS-156 PerCategory turns it off for single-hop;
ISS-153 retest showed HyDE-all single-hop only 0.25). This issue is a
**design-only** weapon-selection exercise. Implementation tickets get spun
out per-weapon once we pick.

## Three candidate weapons

| Weapon | Cost (impl) | Cost (per query) | Expected lift | Risk |
|--------|------------:|------------------|--------------:|------|
| A. Cross-encoder re-ranker (K=50 → K=10) | 3–5 days | +30–80ms local CPU, or ~$0.0002 hosted | +5–15pp single-hop, possibly +2-3pp overall | High up-front, low recurring |
| B. Stronger query/doc embedder (nomic → bge-large or mxbai) | 1–2 days | +1.5× embed time (768d → 1024d), one-shot re-index | +3–8pp single-hop, possibly regressions elsewhere | Medium, full re-index needed |
| C. Triple-extraction yield (turn on `triple.enabled`) | 2–4 days | Haiku per ingest (~$0.0001/memory), one-shot backfill | +0–10pp single-hop *iff* Factual plan reachable | High — gated on ISS-145+149 |

Sections below: what each is, what evidence backs the lift estimate,
what blocks it, what the empirical test plan looks like.

---

## Weapon A: Cross-encoder re-ranker

### What

After top-K fusion (currently K=10 after MMR), feed (query, candidate) pairs
through a cross-encoder model that produces a single relevance score per pair.
Re-sort. Truncate to final K.

Typical pipeline:
1. Retrieval returns K_fusion = 50 (today K=10 → expand the fusion-pool)
2. Cross-encoder scores all 50 pairs
3. Sort by cross-encoder score
4. Truncate to K_final = 10 → returned to caller

### Why it should help single-hop

Single-hop = "find the one episode that contains the answer". The current
fusion uses BM25 + dense embedding + (sometimes) entity score, linearly
combined via `FusionConfig` weights. A cross-encoder reads **query and
candidate jointly** and learns relevance — strictly more expressive than any
weighted sum of independent channels.

LoCoMo single-hop questions (per conv-26 spot-check) often need lexical
overlap with **specific phrasings** ("did Caroline ever mention X about her
mom?") that BM25 misses on stop-word-heavy queries and dense embeddings
average-pool away. Cross-encoder is the textbook fix.

Lift estimate **+5–15pp** based on published BEIR / MS-MARCO results:
- BM25 + cross-encoder typically lifts MRR@10 by 5–10pp on single-fact QA
- Hybrid (BM25 + dense) + cross-encoder typically +3–8pp on top of hybrid
- On small, in-domain corpora (LoCoMo conv = ~50 episodes), the lift trends
  higher because cross-encoder has less noise to fight through

### What blocks it

1. **Model choice**: `ms-marco-MiniLM-L-6-v2` (22MB, ~30ms/pair on M1) is the
   typical default. `bge-reranker-base` (110MB, ~60ms/pair) is stronger. Both
   are HuggingFace. Need to decide if we want local (Candle / ONNX runtime)
   or hosted (Voyage / Cohere reranker — adds $).
2. **Integration point**: `Reranker` trait already exists at
   `crates/engramai/src/retrieval/fusion/reranker.rs` with `MmrReranker` as
   the first concrete impl (ISS-139). New reranker would be a second impl,
   wired at the same `api.rs:584-602` Stage C.5 hook. **The scaffold is
   already done.**
3. **Pool expansion**: Currently fusion returns K=10. Cross-encoder wants
   K_fusion ≥ 30 (ideally 50) to have room to re-order. Need to add a
   `cross_encoder_pool` knob analogous to `bm25_pool`.
4. **Inference runtime**: Candle (Rust-native) or ort (ONNX). Candle is the
   project's existing direction (no new dep). ort is more battle-tested for
   transformer inference. ~3 days build either way.
5. **Score interaction with MMR**: MMR currently runs *before* truncation.
   If cross-encoder also runs before truncation, ordering matters: probably
   `cross-encode → MMR → truncate` (re-rank by relevance, then diversify).

### Empirical test plan

1. Build minimal cross-encoder reranker (local Candle, MiniLM-L-6)
2. Bench conv-26 K=10 MMR 0.7 K_fusion=50, sweep cross_encoder on/off
3. Spot-check 10 single-hop wins/losses to confirm it's relevance not noise
4. If +5pp+: full LoCoMo 152q to check no other category regresses
5. If conv-26 wins but full LoCoMo regresses: revert to opt-in flag

Pass criterion: single-hop ≥ 0.30 (halfway to AC-5) on conv-26. If we
get less than +5pp on single-hop, kill it — not worth the recurring latency
budget for marginal lift.

---

## Weapon B: Stronger embedder

### What

Swap `nomic-embed-text` (768d, current default) for either:
- **`bge-large-en-v1.5`** (1024d, ~340MB, top-of-MTEB English)
- **`mxbai-embed-large-v1`** (1024d, ~330MB, MTEB-leader, Apache-2.0)

Both are local Ollama-compatible. Re-embed every memory + every query.

### Why it should help single-hop

Better semantic match between query and candidate episode. Single-hop wins
or loses by whether the right episode is in top-K — a +5% MTEB lift on the
embedder translates roughly to +2–5pp recall@10 on in-domain QA.

`nomic-embed-text` MTEB avg: **62.4**.
`bge-large-en-v1.5` MTEB avg: **64.2** (+1.8).
`mxbai-embed-large-v1` MTEB avg: **64.7** (+2.3).

Modest. Embedder upgrade is the smallest-lift candidate, but also the cheapest
to try because the swap is one config line + a re-index.

### What blocks it

1. **Re-index cost**: All existing memories need re-embedding. For LoCoMo
   conv-26 (~50 episodes), trivial. For real production memory DBs (10k+
   memories), need a backfill driver — analogous to T26a triple backfill
   but for embeddings. Probably 1 day to build, ~1h to run for 10k memories.
2. **Dimension change** (768 → 1024): touches `node_embeddings` table schema,
   FAISS index (if any), HNSW params. Migration story is non-trivial.
   *Alternative*: pick a 768d upgrade (e.g., `bge-small-en-v1.5` is 384d;
   none of the bge-large variants are 768d). The 768d → 1024d schema change
   is the real cost.
3. **Possible regression on other categories**: multi-hop / open-domain
   currently work OK; a different embedder could regress them. Need a full
   LoCoMo bench, not just conv-26.
4. **Determinism**: ISS-155-style determinism issues on the new embedder
   (Ollama version, model precision) — need to verify temp/seed before
   trusting deltas.

### Empirical test plan

1. Pull `bge-large-en-v1.5` and `mxbai-embed-large-v1` via Ollama
2. Write small benchmark: re-embed conv-26, re-run K=10 MMR 0.7, compare
   single-hop / multi-hop / overall against current `nomic-embed-text`
3. If either lifts single-hop by ≥3pp without regression on other categories,
   build the production embedder swap (config + migration + tests)
4. If neither helps, kill — embedder isn't the bottleneck on this corpus

Pass criterion: single-hop ≥ 0.28 (+6pp) without regression on multi-hop
(currently 0.5946). Smaller lift than weapon A but cheaper to try first.

---

## Weapon C: Triple-extraction yield

### What

Today `TripleConfig::default().enabled == false`. Triple extraction (S-P-O
fact tuples via Haiku/Sonnet per memory) is built but dormant.

Turn it on. Triples populate `graph_entities` + `entity_relations` tables.
The Factual retrieval plan uses these for fact-anchored retrieval. The
hypothesis: if Factual plan becomes reachable and useful, single-hop
factual questions ("did X say Y") route through it and win.

### Why it should help single-hop

Single-hop on LoCoMo is mostly **factual recall**: "what did Caroline
tell Melanie about her mom?" → the answer is one S-P-O triple in one
episode. Factual plan is designed exactly for this query shape.

**But** — this only matters if the Factual plan is *reachable*. Today it
isn't (ISS-149: classifier uses NullEntityLookup). So triples alone don't
help unless the upstream blockers fall.

### What blocks it

1. **ISS-145 (L1b)**: ingest path doesn't populate `graph_entity_aliases`.
   `GraphEntityResolver` (Factual's resolver) reads only from there → returns
   0 anchors → Factual plan degrades. Need to either:
   a. Fix ingest to write aliases (probably 1–2 days)
   b. Switch `GraphEntityResolver` to read from `entities` + `memory_entities`
      (the path ingest actually writes)
2. **ISS-149 (L2)**: classifier uses `NullEntityLookup` → never selects
   Factual plan even if anchors *were* resolvable. Need a real entity lookup
   bound at api.rs:496. 1 day fix.
3. **Cost of running triples**: Haiku ~$0.0001 per memory. For LoCoMo
   conv-26 (50 episodes) = $0.005. For 10k production memories = $1.
   Negligible at this scale, but adds ingest latency (~500ms per memory
   for Haiku roundtrip).
4. **Triple quality**: ISS-155 confirmed Anthropic non-determinism. Triples
   extracted at temp=0 should be stable, but quality on conversational text
   (LoCoMo-style "I told her about my mom") is unmeasured. Could be noisy.
5. **Causal chain length**: weapon C needs **three** unrelated fixes to
   land before we see any single-hop signal. Highest risk of "spent 4
   days and learned nothing".

### Empirical test plan

1. Fix ISS-145 + ISS-149 first (separate tickets, both already filed)
2. Turn on `triple.enabled = true`, run extractor backfill on conv-26
3. Verify Factual plan now gets selected by checking `execute_plan` logs
   (currently 0/152 → target ≥30/152 on conv-26)
4. Bench conv-26 K=10 MMR 0.7 with triples on vs off (substrate identical
   apart from triple tables)
5. If Factual plan fires ≥20% AND single-hop ≥0.28, ship; else triage:
   is the Factual *adapter* underperforming, or is the *classifier* picking
   wrong queries?

Pass criterion: single-hop ≥ 0.28 AND Factual plan selected on ≥20% of
single-hop queries. If Factual fires but doesn't win, the adapter needs
work (separate issue).

---

## Recommendation

**Try B first, then A, defer C.**

Reasoning:
- **B (embedder)** is the smallest commitment (1–2 days) and tests a clean
  hypothesis ("is the embedder the bottleneck?"). If it lifts ≥3pp, big
  win for trivial work. If it doesn't, we've eliminated one suspect cheaply.
- **A (cross-encoder)** is the highest-EV bet (largest expected lift, also
  the most mainstream solution for this exact problem). The scaffold is
  already done (ISS-139 Reranker trait + MMR impl as template). Should
  be the main attack if B disappoints.
- **C (triples)** is gated on ISS-145 + ISS-149 — two separate fixes neither
  of which is glamorous. If we land A or B first and AC-5 is hit, we may
  not need C at all. If we don't hit AC-5, C becomes the third move and
  the dependency cost is justified.

Order: B (1–2 days, decide) → A (3–5 days, decide) → C (4+ days dependency
chain) only if A+B fail to clear AC-5.

## Acceptance criteria

- [x] **AC #1 — partial (B + C only, A is the baseline arm)** —
      Weapon B benched on conv-26 K=10 MMR 0.7 HyDE=per_category with three
      embedders (nomic 768d / bge-large 1024d / mxbai-embed-large 1024d).
      Driver: `.gid/issues/ISS-157/artifacts/iss157_weapon_b.sh`. Pass/fail
      decisions recorded below. **Weapons A and C still pending.**

## Weapon B empirical (2026-05-25)

Three-arm head-to-head, conv-26 K=10 MMR 0.7 HyDE=per_category, otherwise
identical config. Embedder is the only variable.

| arm                    | overall | multi-hop | open-domain | **single-hop** | temporal |
|------------------------|--------:|----------:|------------:|---------------:|---------:|
| A: nomic 768d (baseline)   | 0.4605  | 0.5405    | 0.5385      | **0.2188**     | 0.5143   |
| B: bge-large 1024d         | 0.4737  | 0.5946    | 0.4615      | **0.2188**     | 0.5286   |
| C: mxbai-embed-large 1024d | 0.4737  | 0.6216    | 0.4615      | **0.1875**     | 0.5286   |

**Reference (ISS-156 PerCategory baseline, nomic, separate run):**
overall 0.4737 / multi 0.5946 / open 0.4615 / single 0.2188 / temporal 0.5286
— note Arm A above (also nomic, identical config) scored 0.4605 overall vs
ISS-156's 0.4737. That 1.32pp delta is pure LLM-judge variance, same code
path. Confirms ISS-137-style judge-determinism work is still incomplete.

### Verdict: weapon B FAILS the single-hop gate

- B-bge single-hop **0.2188** — **identical to A baseline 0.2188**, no
  movement at all. Target was ≥0.28 (+6pp).
- C-mxbai single-hop **0.1875** — *regression* of −3.13pp vs A. The
  "stronger MTEB score → better single-hop" hypothesis is falsified.

Per-AC scoring:

- AC #2 (single-hop ≥ 0.40): **FAIL** — best of three arms is 0.2188.
- AC #3 (no regression ≥ 2pp): Weapon B passes; C regresses single-hop.
- AC #4 (production wiring): N/A — weapon B isn't the winner.
- AC #5 (bench-design.md doc): see Findings below; weapon-B section will
  go into bench-design.md alongside weapon-A results once that ships.

### Why single-hop didn't move — root cause confirmed

Plan distribution is identical across all three embedders:

| plan_kind   | A-nomic | B-bge | C-mxbai |
|-------------|--------:|------:|--------:|
| associative |     238 |   242 |     242 |
| abstract    |      36 |    36 |      36 |
| affective   |      16 |    12 |      12 |
| hybrid      |      10 |    10 |      10 |
| episodic    |       4 |     4 |       4 |
| **factual** | **0**   | **0** | **0**   |

**The Factual retrieval plan — the one that would actually use BM25-anchored
fact retrieval to win single-hop — is selected ZERO times on all three
embedders.** This is exactly ISS-149: `HeuristicClassifier` runs with
`NullEntityLookup`, so entity-anchor scores are 0.0 and Factual is never
the highest-scoring plan. Embedder swap doesn't fix this — the entity
lookup is structurally absent, not just weak.

Multi-hop *did* move (+5.4pp on B, +8.1pp on C) because cross-episode
semantic similarity is the lever there, and the stronger embedders give
sharper relevance gradients along Associative's vector channel.
Single-hop needs lexical-precise anchor retrieval, which is structurally
gated upstream of the embedder.

### Implication for weapon A (cross-encoder reranker)

This sweep is also evidence **for** weapon A. The argument:

1. Single-hop bottleneck is **plan routing**, not retrieval-channel
   quality. ISS-149 is the structural fix.
2. Even with ISS-149 fixed, single-hop wins require **answer-relevant
   reranking** of the candidate set. The current pipeline returns top-K
   by fusion-of-channels score; nothing reads (query, candidate) jointly.
3. A cross-encoder reranker at the Stage-C.5 hook (already scaffolded by
   ISS-139) reads query and candidate jointly and reorders. It does this
   *regardless of which plan was selected* — so it sidesteps ISS-149.
4. The Reranker trait already exists; MMR is the first concrete impl.
   Adding cross-encoder is a second impl at the same hook.

Weapon A is now the clear next move. Filing implementation issue.

## Acceptance criteria (continued)

- [ ] **AC #2 — FAIL on weapon B** — best single-hop across 3 embedders
      is 0.2188. Target ≥0.40. Move to weapon A.
- [ ] AC #3 — see weapon-A results when they land.
- [ ] AC #4 — see weapon-A results when they land.
- [ ] AC #5 — `bench-design.md` retrieval-improvements section deferred
      to weapon-A completion (single combined doc-update at the end).

## Empirical artifacts (weapon B)

- Driver: `.gid/issues/ISS-157/artifacts/iss157_weapon_b.sh`
- Summary JSON per arm: `.gid/issues/ISS-157/artifacts/iss157-{A-nomic,B-bge,C-mxbai}-summary.json`
- Run dirs:
  - `engram-bench/benchmarks/runs/ISS157-A-nomic-conv26-20260525T035221Z/`
  - `engram-bench/benchmarks/runs/ISS157-B-bge-conv26-20260525T035221Z/`
  - `engram-bench/benchmarks/runs/ISS157-C-mxbai-conv26-20260525T035221Z/`
- Implementation commits:
  - engram `008808e` — ISS-158 graph-store dim threading (blocker fix)
  - engram-bench `f02d2e4` — ENGRAM_BENCH_EMBED_MODEL / EMBED_DIM env override

## Open questions

1. **Cross-encoder runtime**: Candle vs ort? Both work; Candle is project
   direction but ort is faster for transformer inference. Defer to weapon-A
   implementation issue.
2. **Embedder migration story**: if we swap to 1024d, do we re-embed all
   existing engram DBs (potato's `engram-memory.db` has months of data) or
   gate behind a config flag with dual-write during transition?
3. **Triple extraction cost ownership**: Haiku $$ is small but per-ingest
   latency (~500ms) is real. Does that violate any p99 latency budget for
   real-time ingest paths?

## Related

- ISS-148 — owns the AC-5 target. This issue is the design that decides
  how to hit it.
- ISS-145 / ISS-149 — blockers for weapon C
- ISS-139 — Reranker trait scaffold (precondition for weapon A, already done)
- ISS-155 — substrate determinism (relevant for trusting any future deltas)
- ISS-156 — confirmed HyDE is NOT the AC-5 lever
