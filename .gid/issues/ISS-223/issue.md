---
title: MMR λ re-sweep with live vector channel — ISS-143-era MMR conclusions measured a dead channel (ISS-222)
status: resolved
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

- [x] AC-1: harness can measure λ arms over a shared ingestion (either MMR_AB mode or documented same-DB workaround). — `ENGRAM_BENCH_MMR_AB` shipped engram-bench `be420f9` (arm A = override None → λ=1.0 passthrough, arm B = env λ < 1.0 hard-validated; 4-way *_AB mutual exclusivity; per-arm dump labels `<qid>-a/-b`; `locomo_mmr_ab_diff.json`; 215 lib tests green).
- [x] AC-2: conv-26 sweep λ ∈ {1.0, 0.7, 0.5} with live vector channel; per-category deltas recorded (see Verdict).
- [x] AC-3: MMR confirmed active in λ<1.0 arms. conv-26 L05 reorder probe + conv-44 L05 probe (pairs=123 identical=2 pure_reorder=20 set_changed=101) show MMR is live-reshuffling candidates, not score noise. The vector channel is alive (ISS-222 fix holds).
- [x] AC-4: **FALSIFIED — keep default λ=1.0.** λ<1.0 does not beat λ=1.0 on a cross-validated basis. Prod `FusionConfig::mmr_lambda` stays 1.0; λ=0.5 remains opt-in only.
- [x] AC-5: ISS-143 / ISS-188 annotated with dead-channel invalidation note pointing here.

## Notes

- ISS-139 shipped MMR as reorder-only (scores preserved) with FusionConfig.mmr_lambda default 1.0 = byte-identical passthrough.
- MMR hook is Stage C.5 in api.rs (after CE per D5 ordering decision).
- Missing-embedding candidates get 0 diversity penalty — under the dead channel this meant ALL candidates, hence relevance-only.

## Verdict (2026-06-13) — λ<1.0 FALSIFIED on cross-validation

MMR-with-live-vectors was measured for the first time (ISS-222 fixed the dead channel). Three same-DB A/B arms:

- **λ=0.7, conv-26** (`ISS223-MMRAB-L07-conv26-20260612T134643Z`): 0.500 → 0.500, **0 flips, Δ=0**. Completely inert. CE k_in=250 head-rerank leaves λ=0.7 nothing meaningful to reorder.
- **λ=0.5, conv-26** (`ISS223-MMRAB-L05-conv26-20260612T134643Z`): 0.5066 → 0.5263 (**+1.97pp**). single-hop +6.25pp, multi-hop +2.7pp, temporal +1.4pp, **open-domain −7.7pp**. 13 flips (8 gain / 5 loss). Gains concentrated on list-style single-hop (diversity surfaces missing list items).
- **λ=0.5, conv-44** (`ISS223-MMRAB-L05-conv44-20260613T005734Z`): 0.5447 → 0.5366 (**−0.81pp**). single-hop/multi-hop/open-domain all Δ=0, temporal −1.6pp. 7 flips (3 gain / 4 loss). **The conv-26 gain does NOT replicate.**

**Decision (AC-4):** The conv-26 +2pp is corpus-specific — it comes from conv-26's list-question density, and conv-44 (inverted list/single-fact ratio, sparser graph) shows no gain and a slight regression. Cross-validation gate ("≥+2pp on conv-44 with no category regression") is NOT met. **Default `FusionConfig::mmr_lambda` stays 1.0.** λ=0.5 remains available as an opt-in knob for list-heavy workloads but is not the default.

This supersedes any ISS-143-era "MMR helps/doesn't help" conclusion — all of those measured the dead vector channel and are void. The real finding: with a live channel, diversity reordering is at best corpus-specific and at worst mildly harmful once CE k_in=250 already reranks the head.
