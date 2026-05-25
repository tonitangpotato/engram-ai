---
title: Weapon A — cross-encoder reranker at Stage C.5 (ISS-157 next step)
status: open
priority: P1
severity: feature
category: retrieval
created: 2026-05-25
relates:
- ISS-148
- ISS-157
- ISS-149
- ISS-139
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
2. It runs at the Stage-C.5 post-fusion pre-truncate hook (api.rs:584-602),
   so it reorders the candidate pool *regardless of which plan was
   selected*. This sidesteps the ISS-149 classifier deadlock.
3. The `Reranker` trait scaffold already exists
   (`crates/engramai/src/retrieval/fusion/reranker.rs`); MMR (ISS-139)
   was the first concrete impl, cross-encoder is the second at the
   same wiring point.
4. Published BEIR / MS-MARCO results: cross-encoder typically lifts
   single-fact MRR@10 by 5-10pp on top of hybrid retrieval. LoCoMo
   conv-26 is in-domain dense corpus → expected lift trends higher.

## Acceptance criteria

- [ ] **AC #1 — Model + runtime choice** — Pick local cross-encoder.
      Decide Candle vs ort (or hosted via Voyage/Cohere as a Tier-2
      fallback). Default candidate: `ms-marco-MiniLM-L-6-v2` (22MB,
      ~30ms/pair on M1). Stretch: `bge-reranker-base` (110MB, ~60ms/pair,
      stronger). Document choice + rationale in this issue body.
- [ ] **AC #2 — Implementation** — `CrossEncoderReranker` struct
      implementing `Reranker` trait in
      `crates/engramai/src/retrieval/fusion/cross_encoder.rs`. Constructor
      takes model path / config. Inference path is sync (matches
      `Reranker::rerank` signature).
- [ ] **AC #3 — Pool expansion knob** — `FusionConfig.cross_encoder_pool`
      (default = `top_k`, i.e. disabled) controls K_fusion before
      reranking. When > top_k, the fusion pipeline returns the larger
      candidate set, cross-encoder rescores, then truncates to top_k.
      Wire env-var override `ENGRAM_BENCH_CROSS_ENCODER_POOL` analogous
      to ISS-152's `ENGRAM_BENCH_BM25_POOL`.
- [ ] **AC #4 — MMR interaction** — Decide order:
      `cross-encoder → MMR → truncate` vs `MMR → cross-encoder → truncate`.
      Default recommendation: **cross-encoder first** (relevance pass,
      reorders all 50 candidates), then MMR (diversity pass, picks the
      top 10 with anti-redundancy). Document and test.
- [ ] **AC #5 — Empirical conv-26 K=10 MMR 0.7 HyDE=per_category** —
      Two sub-arms:
      - A: cross-encoder OFF (= ISS-156 PerCategory baseline)
      - B: cross-encoder ON, pool=50
      Target: single-hop ≥ 0.30 (halfway to AC-5 target). If hit, proceed
      to AC #6. If miss: kill cross-encoder, file ISS-149 directly as
      next move (no more retrieval-channel weapons left).
- [ ] **AC #6 — Full LoCoMo 152q bench** — no regression ≥ 2pp on
      multi-hop, open-domain, temporal vs ISS-156 PerCategory baseline.
      If hit, ship.
- [ ] **AC #7 — Production wiring** — `MemoryConfig.retrieval.cross_encoder`
      config knob (default off, opt-in via config). Users can pin a
      model path + enable. Bench env-var stays as the dev-time override.
- [ ] **AC #8 — Test coverage** — Property tests via existing
      `assert_reranker_contract` harness (4 properties: idempotent on
      empty, preserves total count, score-finite, deterministic). Unit
      tests on `CrossEncoderReranker::new` with both real model file
      and missing-file error paths.

## Open design questions

### Q1: Candle vs ort

- **Candle** (Rust-native, project direction since v0.3):
  - ✅ no new C++/Python dep
  - ✅ pure Rust → simpler deploy
  - ❌ transformer inference is younger codebase; some operators
    less mature than ort
  - ❌ MiniLM-L-6 requires manual model conversion from HF (.safetensors
    → Candle-loadable format)
- **ort** (ONNX runtime bindings):
  - ✅ battle-tested for BERT-family inference
  - ✅ direct HF ONNX support (`Xenova/ms-marco-MiniLM-L-6-v2-ONNX`)
  - ✅ faster end-to-end (typically 1.5-2× Candle for sentence-pair)
  - ❌ adds onnxruntime native dep (~50MB)

**Initial recommendation: ort** for the implementation experiment because
faster + direct HF model loading. If it lifts single-hop ≥0.30, decide
production whether to port to Candle for dep-cleanliness.

### Q2: Local vs hosted

Local inference (above) is the default. Hosted options exist as
fallback if local doesn't lift:
- **Voyage rerank-2-lite**: $0.05/M tokens, ~50ms latency
- **Cohere Rerank 3**: similar pricing, similar latency

Hosted moves the weapon from "always on" to "$$ per query" — which
violates the spirit of an opt-out-able production knob. Treat as
Tier-2 fallback only.

### Q3: Pool size

K_fusion=50 is the conventional default (gives the reranker 5× headroom
over K=10). On conv-26 the total corpus is ~50 episodes, so K_fusion=50
basically reranks the whole corpus for each query — that's fine for
benching, will need to lower for production with larger DBs.

Recommend `cross_encoder_pool: 50` default, knob to override.

## Step plan

1. **Step 1 — Pick runtime**: Add `ort` or `candle-transformers` dep
   to engramai. Spike-test on a single (query, candidate) pair to
   verify model loads + scores produce sensible output. ~1-2h.

2. **Step 2 — Reranker impl**: Implement `CrossEncoderReranker`. Mirror
   `MmrReranker` skeleton from `reranker.rs`. Property tests pass.
   ~3-4h.

3. **Step 3 — Pool expansion**: Add `cross_encoder_pool` to
   `FusionConfig`. Plumb through hybrid plan candidate count (the
   places where K=10 is hardcoded today). ~2-3h.

4. **Step 4 — Wire at Stage C.5**: Add a second reranker hook after
   MMR runs (or replace ordering — see AC #4). Default off via config
   gate. ~1-2h.

5. **Step 5 — Bench**: Smoke test on conv-26 K=10 MMR 0.7
   HyDE=per_category, cross-encoder ON vs OFF. ~30min wall.

6. **Step 6 — Decision point**: If single-hop ≥ 0.30 → continue to
   full LoCoMo bench + production wiring. If < 0.30 → kill weapon A,
   move to ISS-149 direct fix.

7. **Step 7 — Full LoCoMo + production wiring + doc update**. ~half day.

Total: ~2 days end-to-end if AC #5 passes; ~1 day to learn-and-kill if
it doesn't.

## Risk register

- **R1 — Inference latency**: 50 candidates × 30ms = 1.5s per query.
  conv-26 has 152 queries → +4 min wall added per bench run. Acceptable
  for offline bench, may need batching for prod p99.
- **R2 — Model size in repo**: Don't commit the model file. Either lazy
  download on first use (cached under `~/.cache/engram/models/`) or
  document as a deploy-time setup step.
- **R3 — Cross-encoder might also not move single-hop** — possible if
  the candidate pool fundamentally doesn't contain the right episode
  (i.e. retrieval recall, not ranking, is the bottleneck). The smoke
  test in Step 5 is exactly the falsification probe.
- **R4 — Determinism**: cross-encoder inference should be deterministic
  on fixed model + fp32. If we see ISS-137-style judge wobble at this
  stage too, that's a separate issue (ISS-155 family).

## Related

- ISS-148 — owns the AC-5 target (single-hop ≥ 0.40 on conv-26)
- ISS-157 — parent design issue; weapon B failed, this is weapon A
- ISS-139 — Reranker trait scaffold + MMR impl (precondition, done)
- ISS-149 — classifier deadlock; cross-encoder sidesteps it. If cross-
  encoder doesn't move single-hop, ISS-149 is the next direct attack.
