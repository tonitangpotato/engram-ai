---
title: Factual fusion graph_score collapses to 1.0 on 1-anchor queries — vector_score has no independent channel
status: falsified-on-AC-5a
priority: P1
severity: ranker-floor-too-low
category: retrieval-fusion
created: 2026-05-27
relates:
- engram:ISS-148
- engram:ISS-172
- engram:ISS-173
blocks:
- engram:ISS-148
- engram:ISS-174
verdict: kept-as-opt-in
---

## Summary

ISS-172 shipped vector_score wiring in Factual but AC-6 sweep stayed
at overall=0.230 / 0/9 SF qids. Tracing through `FusionConfig::locked()`
+ `combine()` + `factual_to_scored` reveals **two interlocking bugs**
that make the vector_score signal essentially impotent on the most
common conv-26 SF failure mode:

### Bug 1: `graph_score` normalization collapses on 1-anchor queries

`factual_to_scored` (orchestrator.rs:401):
```rust
let graph_score = (row.seen_via.len() as f64) / total_anchors;
```
where `total_anchors = result.anchors.len()`.

On a 1-anchor query (e.g. q40 "How many times has Melanie been to the
beach in 2023?" → resolver picks `[Melanie]` only), **every candidate
in the pool entered via that single anchor** → `seen_via.len() == 1`
for all 151 candidates → **graph_score = 1.0 for everyone**.

The 0.45-weight graph channel — the *largest* weight in
`FusionWeights::factual` — produces **zero ranking signal** on
1-anchor queries. It's a constant.

### Bug 2: `vector_score` has no independent weight channel

`FusionConfig::locked().factual`:
```rust
FusionWeights {
    text: 0.40,
    vector: 0.0,     // ← unused entirely
    graph: 0.45,
    recency: 0.15,
    ...
}
```

vector_score only contributes via `text = max(vector, bm25)` in
`combine()` (combiner.rs:299). If the query has any common term that
fires BM25 high on noise candidates, `text` takes the bm25 value
*instead* of cosine — silently dropping the semantic signal on
exactly the queries where it's needed most.

## Combined effect on conv-26 SF qids

For a 1-anchor query with 151 Factual candidates:
- 0.45 * 1.0 (graph, flat across pool)           = +0.45 constant
- 0.40 * max(cosine, bm25) (text, only signal)   = 0.0 to +0.40
- 0.15 * recency_score                            = 0.0 to +0.15
- 0.0 * vector_score (dead channel)              = +0.0

So **all 151 candidates score within 0.45-1.0**, ranked entirely by
text+recency. Gold needs not just a higher cosine than every noise
candidate, but a higher `max(cosine, bm25)` — and if many recent
memories about Melanie+beach mention common terms, bm25 trumps cosine.

## Proposed fix (two options)

### A. Reweight: give vector its own channel

```rust
let factual = FusionWeights {
    text:    0.25,   // ↓ from 0.40 (text is now bm25-leaning)
    vector:  0.30,   // ↑ from 0.0 (independent cosine signal)
    graph:   0.30,   // ↓ from 0.45 (curb collapsed channel)
    recency: 0.15,
    ...
};
```
Sum still 1.0. Vector gets a stable channel that bm25 can't drown.

Risk: changes byte-identity of historical bench runs that pinned to
`v0.3.0-locked-r3`. Must bump version label to `v0.3.0-locked-r4`
and gate the new weights behind a `FusionConfig::iss175()` constructor
until cross-validated.

### B. Log-decay graph_score on small anchor sets

```rust
// In factual_to_scored, replace flat graph_score with:
let raw = (row.seen_via.len() as f64) / total_anchors;
let graph_score = if total_anchors == 1.0 {
    // single anchor: collapse to constant signal; force vector to lead
    0.0
} else {
    raw
};
```
Keeps weights frozen; zeros graph contribution when anchor set is too
small to discriminate. Vector now dominates by virtue of being the
only non-zero signal.

Risk: silent semantic change to `graph_score` definition. Tests in
`factual_to_scored` mod will break unless updated.

## Recommendation

Start with **A** (reweight). It's the clearer fix — vector gets a
real channel, all downstream code stays the same shape. B is a hack
that papers over the symptom.

Cross-validate on conv-44 (ISS-160 inverted ratio protocol) before
flipping the default.

## AC

- [ ] AC-1: Implement Option A behind `FusionConfig::iss175()` (don't
      mutate `locked()` until validated).
- [ ] AC-2: Add unit test: 1-anchor query with 5 candidates, 1 with
      cosine=0.7 + 4 with cosine=0.1, must rank cosine=0.7 first
      under iss175() weights but NOT under locked() weights (pinning
      the bug).
- [ ] AC-3: Run AC-6 sweep with `ENGRAM_BENCH_FUSION_CONFIG=iss175` on
      conv-26, target overall ≥0.34 (recover baseline) AND 9 SF qids
      ≥3/9 (meaningful lift over 0/9).
- [ ] AC-4: Cross-validate on conv-44 (different SF/list ratio per
      ISS-160). If recovery holds, flip `locked()` default and bump
      label to `v0.3.0-locked-r4`.
- [ ] AC-5: Add ENGRAM_BENCH_FUSION_CONFIG env var to engram-bench so
      future weight changes are A/B-able without redeploy.

## Probe verification (2026-05-27)

Empirical probe on conv-26, all 4 Factual-routed SF qids (q40/q43/q71/q75).
Full findings: `artifacts/probe-conv26-findings.md`. Highlights:

### `graph_score` distribution per qid (confirms Bug 1)

| qid | n   | g=1.0     | g=0.67 | g=0.5     | g=0.33    |
|-----|-----|-----------|--------|-----------|-----------|
| q40 | 161 | 30 (19%)  | —      | 131 (81%) | —         |
| q43 | 249 | 110 (44%) | —      | 139 (56%) | —         |
| q71 | 268 | 18 (7%)   | 84 (31%) | —     | 166 (62%) |
| q75 | 192 | **192 (100%)** | — | —     | —         |

q75 is the textbook 1-anchor collapse case predicted by Bug 1. q40/q43
saturate the upper half. Only q71 has natural spread.

### Aggregate signal separation (5 gold rows, 865 non-gold)

| field         | gold mean | non mean | Δ      | sep ratio |
|---------------|-----------|----------|--------|-----------|
| graph_score   | 0.79      | 0.79     | +0.00  | **0× — dead signal** |
| bm25_score    | 0.10      | 0.013    | +0.09  | **7.5× — strongest predictor** |
| vector_score  | 0.57      | 0.52     | +0.05  | mild |

bm25_score is the **strongest gold discriminator** but the `max(vec,bm25)`
operator silently discards it: when vec > bm25 (true for all 4 gold rows
where vec=0.55-0.68 vs bm25=0.0-0.30), bm25's contribution is **zero**.

This adds a **Bug 3** to the analysis:

### Bug 3: `text = max(vector, bm25)` discards the discriminating signal

`combiner.rs:~150` (factual fusion):
```rust
let text = max_or_zero(scores.vector_score, scores.bm25_score);
```
On gold candidates for q40/q71, the rare lexical hit ("beach",
"Nicole") fires bm25 to 0.25-0.30 — well above the non-gold noise
floor of 0.013. But because the gold's vector_score is also high
(0.62), `max()` returns vector and the +7.5× bm25 signal is erased.

This compounds with Bug 1 (graph saturated) and Bug 2 (vector has no
own channel): the only signal that genuinely separates gold from
noise is gated behind an aggregation that throws it away.

### Recommendation update

Option A (reweight) **alone** addresses Bugs 1+2 but not Bug 3. Recommend
extending it to also replace `text = max(vec, bm25)` with a
sum-with-evidence-bonus aggregate for Factual:

```rust
fn factual_text_score(vec: Option<f64>, bm25: Option<f64>) -> f64 {
    let v = vec.unwrap_or(0.0);
    let b = bm25.unwrap_or(0.0);
    let base = 0.7 * v + 0.3 * b;
    let bonus = if b > 0.05 { 0.15 } else { 0.0 };  // rare-hit signal
    (base + bonus).min(1.0)
}
```

This preserves the §5.2 "avoid double-counting correlated signals"
intent in the dense case (when bm25 < 0.05, behaves like 0.7×vec),
but adds an evidence bonus when bm25 fires on rare query tokens.

Predicted lifts under combined fix (reweight + new text aggregate):
- q40: ~35% relative score deficit reduction (rank ~32 → ~20)
- q71: ~18% deficit reduction (rank ~107 → ~80) — still drowned
- q43: 0% (gold and top-1 both have g=1.0, both have bm25=0; needs ISS-174)
- q75: 0% (no episode states "three kids" — generator failure, not fusion)

**Conclusion: ISS-175 alone won't hit AC-5a.** Must stack with:
- ISS-174 (reranker for q43-style intra-cluster ties)
- Entity-channel re-enable (ISS-164 was inert in default-off mode)
- Possibly bm25_pool expansion (ISS-152 sweep was negative pre-Bug 3 fix)

### ISS-174 architecture decision (option b)

Probe confirms reweighting belongs in `combiner::fuse_factual()`, not
upstream (would affect all plans) or downstream (would re-fuse already-
fused output). File ISS-174 with scope:

1. `FusionConfig.factual_reweight: bool` (serde-default false → byte-
   identity preserved)
2. `combiner::fuse_factual_v2()` with new weights AND new text aggregate
3. 3 unit tests: byte-identity at flag=false, lift verification at flag=true
4. Engram-bench `ENGRAM_BENCH_FUSION_CONFIG=iss175` env var

## Implementation plan (single PR, ISS-175 scope)

> Scaffolding flag + formula ship together in one ticket. ISS-174 is
> a separate already-filed issue about Hybrid sub-plan vector_score
> threading — unrelated to this reweighting work.

### Files touched

1. `crates/engramai/src/retrieval/fusion/combiner.rs`
   - Add `FusionConfig.factual_reweight: bool` (serde-default false)
   - Add `combine_factual_v2(sub, weights) -> f64` (new fn, NOT a modification of `combine`)
   - Branch in `fuse_and_rank` (or in the Factual call site): if
     `cfg.factual_reweight && intent == Intent::Factual` → call v2,
     else current `combine`
   - Bump `version` field on FusionConfig when flag is on
     (e.g. `"v0.3.0-locked-r3-iss175"`)

2. `crates/engramai/src/retrieval/orchestrator.rs`
   - `GraphQuery::with_factual_reweight(Option<bool>)` builder (mirrors
     `with_mmr_lambda` and `with_entity_channel` pattern)
   - Plumb override into the FusionConfig used by Factual standalone path

3. `engram-bench` (separate commit)
   - `ENGRAM_BENCH_FACTUAL_REWEIGHT=on` env var → sets the override

### `combine_factual_v2` formula

```rust
/// ISS-175 — Factual-only reweighted fusion (gated by
/// `FusionConfig.factual_reweight`).
///
/// Three changes from `combine` for Factual:
/// 1. New text aggregate: sum-with-evidence-bonus instead of max.
///    Preserves §5.2 "avoid double-counting" intent in the dense case
///    (when both signals are similar) but rescues rare-lexical-hit
///    cases (when bm25 fires high on gold tokens absent from query).
/// 2. Rebalanced weights: graph 0.30 (was 0.45, curbs saturation),
///    text 0.25 (was 0.40, freed by giving vector its own channel),
///    vector 0.30 (was 0.0), recency 0.15 (unchanged).
/// 3. Renormalization unchanged (existing logic in combine handles
///    missing-signal redistribution).
fn combine_factual_v2(sub: &SubScores) -> f64 {
    // New weights (sum to 1.0):
    const W_TEXT: f64 = 0.25;
    const W_VECTOR: f64 = 0.30;
    const W_GRAPH: f64 = 0.30;
    const W_RECENCY: f64 = 0.15;

    // New text aggregate: 0.7×vec + 0.3×bm25 + 0.15 bonus if bm25>0.05
    let text_score: Option<f64> = match (sub.vector_score, sub.bm25_score) {
        (None, None) => None,
        (v, b) => {
            let v = v.unwrap_or(0.0);
            let b = b.unwrap_or(0.0);
            let base = 0.7 * v + 0.3 * b;
            let bonus = if b > 0.05 { 0.15 } else { 0.0 };
            Some((base + bonus).min(1.0))
        }
    };

    // ... same renormalization loop as combine(), with components:
    //   (W_TEXT, text_score),
    //   (W_VECTOR, sub.vector_score),
    //   (W_GRAPH, sub.graph_score),
    //   (W_RECENCY, sub.recency_score)
}
```

### Tests (unit, in combiner.rs)

1. `factual_reweight_default_off_byte_identical_to_combine`
   — pin: with `factual_reweight=false`, fuse_and_rank produces
   bit-identical output to current path on a 5-candidate fixture.

2. `factual_reweight_on_lifts_gold_with_rare_bm25_hit`
   — fixture: 3 candidates, gold has vec=0.62 bm25=0.25, leader has
   vec=0.77 bm25=0.0. Assert gold ranks above leader under v2 (vs
   below under current).

3. `factual_reweight_on_lifts_gold_with_low_graph_score`
   — fixture: gold has graph=0.33 vec=0.65 bm25=0.25, leader has
   graph=1.0 vec=0.55 bm25=0.0. Assert gold climbs (specific
   delta numbers from probe q71 case).

4. `factual_reweight_renormalizes_when_recency_missing`
   — fixture: candidate with recency=None. Assert score = weighted
   sum / (W_TEXT + W_VECTOR + W_GRAPH) = 0.85-normalised.

5. `factual_reweight_does_not_affect_other_intents`
   — fixture: Episodic candidate. Assert fuse_and_rank with
   `factual_reweight=true` produces same output as `false`.

### Bench validation (after code lands)

1. Re-run conv-26 with flag on, same envelope as ISS-161 Arm A
   (entity_channel off, mmr=0.7, k=10). Compare SF pass rate:
   - Baseline (ISS-161 Arm A): SF 5/27
   - This probe run (single, w/ dump hook overhead): SF 1/13
   - Target with flag on: SF ≥7/27 (≥+2 from Arm A baseline)
2. Cross-validate on conv-44 (no regression on multi-hop ≥0.39).
3. If lift confirmed: flip `factual_reweight` default to true,
   bump `version` permanently, document in §5.2.

### Stacking gates

ISS-175 lifts Factual SF modestly. To hit AC-5a (≥0.60), need:
- ISS-175 alone: expected SF ~7-9/27 (rough; probe predicts q40+q71 climb but stay outside top-10)
- + ISS-164 entity-channel: separate enable test
- + reranker (cross-encoder, separate ticket): for q43-style intra-cluster ties

ISS-175 is **one of three required pieces**, not the silver bullet.

## Status

Probe complete (2026-05-27, commit 785fb34). Findings at
`artifacts/probe-conv26-findings.md`. Issue extended with Bug 3 +
revised recommendation + implementation plan.

P1 because it directly blocks ISS-148 AC-5a recovery.

Next: implementation per plan above (single PR, ~150 LoC + 5 tests).

---

## A/B Verdict — 2026-05-28 (STAMP 20260528T034409Z)

**Sweep**: conv-26 K=10 temp=0 HyDE=off entity_channel=off pipeline_pool=1.
Arm A = `FACTUAL_REWEIGHT=off`, Arm B = `on`. engram-bench commit b51ee58.

**Plan-kind histogram** (proves the formula engaged):
- A: factual=113, hybrid=31, associative=8
- B: factual=113, hybrid=30, associative=8, abstract=1

`combine_factual_v2` ran on **113/152 queries in both arms** — the
formula did fire on the intended Factual plan.

**Single-fact sub-bucket (the AC-5a gate)**:
- Single-hop raw: A=3/32 B=3/32 **Δ=0**
- Two single-hop flips cancel: q32 gain (list), q47 loss (list).
- Under any SF/list partition (comma-heuristic or strict): SF Δ=0.

**Ship gate (B sf ≥ A sf + 2) NOT met. FALSIFIED on AC-5a target.**

**Unexpected positive — overall +5.9pp, multi-hop +18.9pp**:
- Multi-hop: A=7/37 → B=14/37 (+7 net, 8 gains, 1 regression)
- Open-domain: A=2/13 → B=4/13 (+2 net, 2 gains, 0 regressions)
- Total: 13 gains, 4 regressions, 135 ties = +9 net on 152 queries.
- Spot-checked gains are real retrieval wins (date-bearing factoids
  surfaced from outside A's top-10), not LLM-judge noise.
- Spot-checked regressions follow the inverse pattern: vague-emotional
  golds (q47, q150) get bumped by bm25-rich rivals under v2 weights.

**Verdict**: combine_factual_v2 surfaces a real "evidence vs emotion"
ranking axis but doesn't lift the AC-5a-relevant single-fact bucket
(consistent with probe predictions — q40/q43/q71 stayed misses).

**Recommendation**:
1. Keep code on `main` as opt-in (flag default `false`). Don't flip.
2. Multi-hop +18.9pp is a meaningful lift on its own — file follow-up
   to investigate whether it's a stack candidate for non-AC-5a goals.
3. AC-5a target (single-fact ≥17/27) remains unreachable via this
   lever; consistent with ISS-161 verdict that current architecture
   tops out around 8/27 without extraction enrichment (ISS-162) or
   a stronger reranker than the cross-encoder weapon (which already
   falsified).

**Artifacts**:
- `.gid/issues/ISS-175/artifacts/ab-conv26-20260528-findings.md` — full ledger
- `engram-bench/benchmarks/runs/ISS175-A-conv26-20260528T034409Z/` — Arm A data
- `engram-bench/benchmarks/runs/ISS175-B-conv26-20260528T034409Z/` — Arm B data
