---
title: Weapon A — cross-encoder reranker at Stage C.5 (ISS-157 next step)
status: open
priority: P1
severity: feature
category: retrieval
created: 2026-05-25
updated: 2026-05-25
relates:
- ISS-148
- ISS-157
- ISS-149
- ISS-139
- ISS-160
depends_on: ''
---

## Summary

ISS-157 weapon B (embedder swap) **failed** to lift conv-26 single-hop
(0.2188 → still 0.2188). Root cause confirmed: Factual plan selected
0/152 across nomic/bge/mxbai — ISS-149 NullEntityLookup deadlock,
structural not statistical.

Weapon A (cross-encoder reranker at the Stage C.5 hook) is now the
designated next move. The reasoning:

1. Cross-encoder reads **(query, candidate) jointly** — strictly more
   expressive than any weighted sum of independent retrieval channels.
2. It runs at the Stage-C.5 post-fusion pre-truncate hook (api.rs:680),
   so it reorders the candidate pool *regardless of which plan was
   selected*. This sidesteps the ISS-149 classifier deadlock.
3. The `Reranker` trait scaffold already exists
   (`crates/engramai/src/retrieval/fusion/reranker.rs`); MMR (ISS-139)
   was the first concrete impl, cross-encoder is the second at the
   same wiring point.
4. Published BEIR / MS-MARCO results: cross-encoder typically lifts
   single-fact MRR@10 by 5-10pp on top of hybrid retrieval. LoCoMo
   conv-26 is in-domain dense corpus → expected lift trends higher.

ISS-160 falsification (2026-05-25) confirmed the single-fact bucket
lift reproduces on conv-44 (+23.5pp at K=10→K=30). That's the durable
signal weapon A targets.

## Decision (locked 2026-05-25)

After ISS-160 cross-check + read of `retrieval/fusion/reranker.rs` +
`retrieval/api.rs:670-710` + `retrieval/budget.rs` (k_seed default = 10).

### Spike verified 2026-05-25

Freestanding spike (`/tmp/ce_spike/`) built and run end-to-end:
ort 2.0.0-rc.12 + tokenizers 0.23 + ndarray 0.17 + MiniLM-L-6-v2 ONNX
(87MB from Xenova HF mirror) on M1.

```
[ highly relevant] score= +7.6135  (1.5ms)
[      irrelevant] score=-11.1293  (1.6ms)
[         partial] score= -0.5790  (1.6ms)
50-pair batch: 76ms total = 1.5ms per pair
model load: 71ms
```

Ranking sane (relevant >> partial >> irrelevant). Latency 20× better
than the 30ms/pair design assumption (probably because Xenova's ONNX
export is already fused for inference). No system `onnxruntime` install
needed — ort downloads prebuilts at `cargo build` time on first run.

### D1. Runtime: **`ort`** (ONNX Runtime via `ort` crate)

- HF cross-encoder models ship as ONNX directly. No format conversion
  pain (Candle path requires manual `.safetensors` → Candle conversion
  for MiniLM, which is brittle for production).
- ort on M1 uses Accelerate / BLAS kernels → 2-5× faster than Candle
  for encoder-only transformer inference. Encoder-only BERT-family is
  not Candle's strong suit; Candle shines for diffusion/LLM.
- Downside (~50MB dylib dependency) is acceptable: engramai is not a
  single-binary crate, and the model file itself is already a
  deploy-time download.
- **Fallback path:** feature-flag Candle as alt-backend if ort dylib
  portability becomes a problem on Linux deployments. Not now.

### D2. Model: **`ms-marco-MiniLM-L-6-v2`** (22MB, ~30ms/pair on M1)

NOT bge-reranker-base. Reasons:

- MiniLM-L-6 is the BEIR/MS-MARCO published baseline. Every paper in
  the space compares against it → results are most trustworthy.
- At K_fusion=50 (see D3) latency budget is 50 × 30ms = 1.5s per query.
  bge-reranker-base at 60ms/pair = 3s, which pushes past the §5.4
  reproducibility envelope without proven extra quality on in-domain
  dense corpora.
- If MiniLM-L-6 doesn't hit AC-5a (single-fact ≥ 0.60 on conv-26),
  upgrading to bge-base is one config flag (same trait impl). Don't
  pre-optimize.

### D3. K_fusion: **50** (not 10)

This is the critical design call.

- Current default `k_seed=10` (verified at `retrieval/budget.rs:229`)
  means fusion outputs ~10 candidates per channel. Cross-encoder at
  K=10 would just rerank the same 10 fusion already chose → degenerates
  to "shuffle MMR's output", wasting the entire reason to ship a
  cross-encoder.
- ISS-152 sweep proved `bm25_pool=100/200` alone doesn't help conv-26.
  That's expected: without a second-stage reranker to *use* the extra
  candidates, more pool = more noise. Weapon A is exactly that second
  stage.
- Published cross-encoder lift numbers (BEIR papers) assume
  K_retrieve=50-100, K_rerank=10.
- Latency math at K_fusion=50: 1.5s/query × 152 queries = +3.8min on
  a ~12min run. Acceptable for quality bench.

### D4. Wire point: **same Stage C.5 hook as MMR**, AFTER MMR if both active

- Trait already enforces purity / no-drops / bounded latency (the four
  properties in `assert_reranker_contract`). No architecture work
  needed — drop in alongside `MmrReranker`.
- Single chokepoint covers all 7 plans, mirrors ISS-139's reasoning.

### D5. Composition order (when both rerankers active): **cross-encoder FIRST, MMR SECOND**

Key insight: MMR must run *after* cross-encoder, not before.

- Cross-encoder first = quality reorder on raw fusion candidates.
- MMR second = diversity pick on the quality-reordered list.
- Reverse order means MMR picks "diverse mediocre" from raw fusion
  scores instead of "diverse top" from cross-encoder scores. That
  wastes the cross-encoder's signal.
- Document this clearly in the C.5 hook comment.

### D6. Config knobs (mirror ISS-139 MMR pattern)

- `GraphQuery::with_cross_encoder(Option<CrossEncoderConfig>)` —
  per-query override knob, same pattern as `with_mmr_lambda`.
- `CrossEncoderConfig { model_path: PathBuf, k_in: usize, enabled: bool }`
  where `k_in` is the cap on input pool (default 50).
- Default: **disabled**, opt-in via builder. Matches ISS-139's
  default-off pattern — we don't change retrieval behavior until
  benched.
- `MemoryConfig.retrieval.cross_encoder` for production config-file
  wiring (AC #7).

### D7. Bench plan: 3-arm on **both** conv-26 and conv-44

- **Arm A** — control: current pipeline (HyDE=per_category, MMR=0.7,
  K=10, cross-encoder OFF).
- **Arm B** — MMR off, cross-encoder on at K_fusion=50.
- **Arm C** — MMR on, cross-encoder on at K_fusion=50 (composition test
  for D5).

Running on conv-44 too because ISS-160 taught us "wins on conv-26"
≠ "wins universally". conv-44 cross-check guards against regression on
other corpus shapes.

### D8. NOT in scope this round

- bge-reranker-base — fallback if MiniLM doesn't hit AC-5a.
- Hosted reranker (Voyage / Cohere) — no network deps in eval path.
  Tier-2 fallback only if local doesn't move the needle.
- ColBERT-style late-interaction — different architecture, ship the
  classic cross-encoder first.

## Acceptance criteria

- [x] **AC #1 — Model + runtime choice** — Decided in D1 + D2.
      `ort` crate + `ms-marco-MiniLM-L-6-v2`. Stretch fallback
      `bge-reranker-base`.
- [ ] **AC #2 — Implementation** — `CrossEncoderReranker` struct
      implementing `Reranker` trait in
      `crates/engramai/src/retrieval/fusion/cross_encoder.rs`. Constructor
      takes `CrossEncoderConfig`. Inference path is sync (matches
      `Reranker::rerank` signature). Tokenization + model inference
      via `ort` (and `tokenizers` for the HF MiniLM tokenizer).
- [ ] **AC #3 — Pool expansion knob** — `CrossEncoderConfig.k_in`
      (default 50) controls K_fusion before reranking. Plumbed through
      the fusion pipeline so candidate set returned to Stage C.5 is
      ≥ k_in when the cross-encoder is enabled. Env-var override
      `ENGRAM_BENCH_CROSS_ENCODER_POOL` in engram-bench.
- [ ] **AC #4 — MMR interaction** — Decided in D5: cross-encoder first,
      MMR second. Document in C.5 hook comment + add an integration
      test that runs both and verifies the composition order produces
      the cross-encoder-ranked top before MMR's diversity selection.
- [ ] **AC #5a — Empirical conv-26 single-fact ≥ 0.60** — Primary gate.
      Arm B vs Arm A on conv-26 single-fact bucket (12 questions).
      Current baseline: 3/12 = 0.25 at K=10, 5/12 = 0.42 at K=30 (no
      cross-encoder). Target: 7+/12 = 0.58+ with cross-encoder. Sets
      AC-5a in ISS-148 to passing.
- [ ] **AC #5b — Empirical conv-26 single-hop ≥ 0.30** — Aggregate
      single-hop sanity check (informational, follows from #5a + list
      bucket unchanged).
- [ ] **AC #6 — Full LoCoMo 152q no-regression** — On both conv-26
      AND conv-44: no regression ≥ 2pp on multi-hop, open-domain,
      temporal vs ISS-156 PerCategory baseline. Composition test (Arm C)
      no worse than Arm B on aggregate.
- [ ] **AC #7 — Production wiring** — `MemoryConfig.retrieval.cross_encoder`
      config knob (default off, opt-in via config file). Users can pin
      a model path + enable. Bench env-var stays as the dev-time override.
- [ ] **AC #8 — Test coverage** — Property tests via existing
      `assert_reranker_contract` harness (4 properties: pure / bounded /
      score-preserved / no-drops). Unit tests on `CrossEncoderReranker::new`
      with both real model file and missing-file error paths. Tokenizer
      round-trip determinism test.

## Step plan

1. **Step 1 — Add `ort` + `tokenizers` deps** (feature-flagged to keep
   default build light). Spike-test: load MiniLM-L-6 ONNX, score one
   (query, doc) pair, verify sensible output. ~1-2h.

2. **Step 2 — Reranker impl** — `CrossEncoderReranker` in
   `retrieval/fusion/cross_encoder.rs`. Mirror `MmrReranker` skeleton.
   `assert_reranker_contract` passes. ~3-4h.

3. **Step 3 — Pool expansion** — Add `cross_encoder` field to
   `FusionConfig` (or, cleaner, a new `RerankerStageConfig`). Adjust
   the fusion pipeline to return ≥ k_in candidates when reranker is
   enabled. ~2-3h.

4. **Step 4 — Wire at Stage C.5** — Insert cross-encoder hook BEFORE
   the MMR hook in `api.rs:680-705`. Default off via config gate.
   Update the C.5 doc comment to reflect D5 composition order. ~1-2h.

5. **Step 5 — Bench setup** — `ENGRAM_BENCH_CROSS_ENCODER=1` +
   `ENGRAM_BENCH_CROSS_ENCODER_POOL=50` env vars in engram-bench.
   Smoke test on conv-26 K=10 MMR 0.7 HyDE=per_category. ~30min wall.

6. **Step 6 — Decision point** — If AC #5a passes (single-fact ≥ 0.60)
   → continue to AC #6 full bench + production wiring. If miss →
   try bge-reranker-base before killing weapon A; if THAT misses, file
   ISS-149 as direct attack.

7. **Step 7 — Full bench + production wiring + doc update + ISS-148
   AC-5a close** — half day.

Total: ~2.5 days end-to-end if AC #5a passes; ~1 day to falsify and
escalate to bge / ISS-149 if it doesn't.

## Risk register

- **R1 — Inference latency** ✅ **MUCH BETTER THAN PLANNED.** Spike
  measured (M1, ort 2.0.0-rc.12, MiniLM-L-6 fp32, single-thread): **1.5ms
  per (query, doc) pair**, not 30ms. At K_fusion=50: 76ms/query = +11.4s
  overhead per 152-query bench (not 3.8min). At K_fusion=100: ~23s. This
  is an **order-of-magnitude better** than the design assumption, leaving
  headroom to expand K_fusion if AC-5a is borderline. Production p99 fine
  even single-pair.
- **R2 — Model size in repo**: Don't commit the model file. Lazy
  download on first use into `~/.cache/engram/models/` (similar to
  HF cache). Document deploy-time setup step.
- **R3 — Recall, not ranking, might be the bottleneck**: Cross-encoder
  only helps if the right episode is *in* the K_fusion=50 pool. If
  fusion's recall@50 doesn't contain the gold episode, no reranker
  can save it. AC #5a is precisely the falsification probe.
- **R4 — Determinism**: cross-encoder inference should be deterministic
  on fixed model + fp32 + sequential execution. If we see ISS-137-style
  judge wobble at this stage, that's a separate issue (ISS-155 family).
- **R5 — ort dylib portability**: ort needs onnxruntime native lib.
  On macOS dev (this work) it's `brew install onnxruntime`. On Linux
  deploy it's a package or static link. Document in production setup.
  Fallback to Candle is the escape hatch if this becomes a real blocker.

## Related

- ISS-148 — owns the AC-5a target (single-fact ≥ 0.60 on conv-26)
- ISS-157 — parent design issue; weapon B failed, this is weapon A
- ISS-139 — Reranker trait scaffold + MMR impl (precondition, done)
- ISS-149 — classifier deadlock; cross-encoder sidesteps it. If cross-
  encoder doesn't move single-hop, ISS-149 is the next direct attack.
- ISS-160 — list-bucket failure mode (corpus-shape pathology). Cross-
  encoder NOT expected to help that bucket — out of scope for AC-5a.
- ISS-152 — k_seed/bm25_pool sweep proved more candidates without a
  second-stage reranker don't help. Weapon A is the missing reranker.
