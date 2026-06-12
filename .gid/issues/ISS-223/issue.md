---
title: MMR λ re-sweep with live vector channel — ISS-143-era MMR conclusions measured a dead channel (ISS-222)
status: open
priority: P1
labels: retrieval, mmr, benchmark, locomo
relates_to: ISS-222, ISS-139, ISS-143, ISS-201
depends_on: ISS-222
filed: 2026-06-11
filed_by: rustclaw
---

# MMR λ re-sweep with live vector channel

## Problem

Every MMR A/B measurement since T32 (commit `887dc37`, unified substrate default-on) ran with the vector-embedding channel **silently dead**: `get_embeddings_for_ids` JOINed the empty legacy `memories` table, so `StorageLoader::load_embeddings` returned an empty map and MMR (ISS-139) degenerated to relevance-only on every query (GUARD-9 swallow, orchestrator.rs ~:264).

Concretely invalidated:
- The ISS-143-era λ sweep runs (conv-26, λ ∈ {1.0, 0.7, …}) — both arms were measuring `NullReranker`-equivalent behavior. Any "MMR doesn't help" conclusion from that period is void.
- ISS-188 populate-embeddings λ=0.7/0.5 arms (lift 0 — now explained: diversity term had no embeddings to diversify against in the Factual path).

ISS-222 fixed the JOIN (commit `2cc72375`, validated run `ISS222-LEVER2-conv26-20260612T002433Z`: Factual `vector_score` nonzero 57292/57292). But that run kept `ENGRAM_BENCH_MMR_LAMBDA=1.0` — MMR inert by config. **MMR-with-live-vectors has never been measured.**

## Hypothesis

With embeddings actually loading, λ<1.0 should produce real reordering. Expected beneficiaries per ISS-139's original motivation: list-style questions (conv-26 has 5–20 depending on bucket definition) and the D-bucket "fragmented list answers" class from the ISS-201 Step-2 autopsy (11/71).

Counter-risk: CE k_in=250 (now the bench-default envelope per ISS-201 lever-2) already reorders the head aggressively; MMR runs *after* CE at Stage C.5, so diversity may fight CE precision on single-answer questions.

## Plan

Same-DB sweep on conv-26, LEVER2 envelope (INGEST_WINDOW=4, K=10, FACTUAL_REWEIGHT=on, HyDE/entity off, PIPELINE_POOL=1, CE=1, CE_K_IN=250 — identical to `ISS222-LEVER2` except λ):

- Arm A: λ=1.0 (control — can reuse `ISS222-LEVER2-conv26-20260612T002433Z` ONLY if re-ingestion is shared; otherwise re-run in-sweep)
- Arm B: λ=0.7
- Arm C: λ=0.5

⚠️ Cross-ingestion noise is ±2pp overall / ±9pp per-category (ISS-201 lesson). There is currently **no same-DB MMR_AB harness mode** (only TEMPORAL_AB / GUIDANCE_AB / CROSS_ENCODER_AB). Options:
1. (cheap) 3 independent arms in one sweep script, accept noise, only trust ≥5pp deltas
2. (correct) add `ENGRAM_BENCH_MMR_AB` same-DB mode mirroring `CROSS_ENCODER_AB` (pools differ between arms → per-arm dumps with `-a`/`-b` labels; ~50 lines in locomo.rs following the ce_ab block at :1545)

Option 2 preferred — MMR effects are likely <5pp, below the noise floor of option 1.

## Acceptance criteria

- [ ] AC-1: harness can measure λ arms over a shared ingestion (either MMR_AB mode or documented same-DB workaround).
- [ ] AC-2: conv-26 sweep λ ∈ {1.0, 0.7, 0.5} with live vector channel; per-category + list-question sub-bucket deltas recorded.
- [ ] AC-3: verify MMR is actually active in λ<1.0 arms (dump or probe shows reordering vs λ=1.0 arm, not just score noise).
- [ ] AC-4: decision — pick default λ for bench envelope + prod `FusionConfig::mmr_lambda`; if no arm beats λ=1.0, record falsification and keep 1.0.
- [ ] AC-5: annotate ISS-143/ISS-188 issue docs with dead-channel invalidation note pointing here.

## Notes

- ISS-139 shipped MMR as reorder-only (scores preserved) with FusionConfig.mmr_lambda default 1.0 = byte-identical passthrough.
- MMR hook is Stage C.5 in api.rs (after CE per D5 ordering decision).
- Missing-embedding candidates get 0 diversity penalty — under the dead channel this meant ALL candidates, hence relevance-only.
