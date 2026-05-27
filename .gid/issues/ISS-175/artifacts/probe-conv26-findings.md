# ISS-175 Probe Findings — conv-26 Factual subscore analysis

**Date**: 2026-05-27
**Probe**: PID 72391, STAMP=20260527T220553Z (third attempt; first two died on Anthropic transients before ISS-176 retry shipped)
**Dumps**: `/tmp/iss175-probe/dumps/conv-26-q{40,43,71,75}-factual.jsonl`
**Pool sizes**: q40=161, q43=249, q71=268, q75=192 (matches ISS-173 H5 sweep range 151-263)
**Analysis script**: `/tmp/iss175_analyse.py`

## TL;DR

The Factual fusion is failing because **`graph_score` saturates and provides ~zero discrimination on dense, single-domain conversations like LoCoMo conv-26**. With graph weighted at 0.45 (largest single weight), saturation translates directly into rank noise where the gold candidate is "drowned" among the 1.0-graph cluster.

Three of four qids confirm; q43 confirms a related failure mode (gold IS in the saturated cluster but the only tie-break is `vector_score` which doesn't separate enough).

## Raw evidence

### `graph_score` distribution per qid

| qid | n   | g=1.0     | g=0.67 | g=0.5     | g=0.33    | discrimination |
|-----|-----|-----------|--------|-----------|-----------|----------------|
| q40 | 161 | 30 (19%)  | —      | 131 (81%) | —         | binary; 1.0 cluster wins by graph weight alone |
| q43 | 249 | 110 (44%) | —      | 139 (56%) | —         | top-14 all in 1.0 cluster; intra-cluster tie-break needed |
| q71 | 268 | 18 (7%)   | 84 (31%) | —       | 166 (62%) | gold sits in g=0.33 cluster (rank 107) |
| q75 | 192 | 192 (100%)| —      | —         | —         | **complete saturation — graph signal is dead weight** |

### Gold candidate rank + subscore breakdown

#### q43: "What kind of art does Caroline create?" (gold = "abstract painting")
Gold at **rank 15**, score 0.852 vs top-1 0.893 (Δ=0.04 — drowned within 4% of leader)

```
rank  score   g     bm    v      content
   1  0.893  1.00  0.20  0.77   Melanie creates paintings as a form of self-expression  (non-gold)
   2  0.882  1.00  0.00  0.75   Melanie expressed admiration for Caroline's art         (non-gold)
   3  0.879  1.00  0.00  0.74   Caroline creates art that represents inclusivity        (non-gold)
   ...
  15  0.852  1.00  0.00  0.69   Caroline has been trying out abstract painting          <- GOLD
```
**Failure mode**: graph=1.0 plateau collapses to vector_score for ranking. Gold's vec=0.685 vs leader vec=0.773; the lexically-relevant "abstract" token isn't in the query so bm25=0 cannot rescue.

#### q71: "What book did Caroline read recently?" (gold = "Becoming Nicole")
Gold at **rank 107**, score 0.468 vs top-1 0.844 (Δ=0.38 — catastrophically drowned)

```
rank  score   g     bm    v      content
   1  0.844  1.00  0.25  0.67   Melanie is reading a book that Caroline recommended  (non-gold; topical drift)
   ...
 107  0.468  0.33  0.25  0.62   Caroline read and loved 'Becoming Nicole' by Amy Ellis Nutt  <- GOLD
```
**Failure mode**: gold has bm25=0.25 (high signal) AND vec=0.62 (high) — but graph penalty (0.33 vs 1.00) at 45% weight buries it. The 0.67 hop-step penalty (= 0.45 × 0.67) overwhelms the +0.16 BM25 lift (= 0.40 × 0.25).

#### q40: "How often does Melanie take her kids to the beach?" (gold = 'beach trips')
Gold at **rank 32**, score 0.565 vs top-1 0.799 (drowned by ~0.23)

```
rank  score   g     bm    v      content
   1  0.799  1.00  0.00  0.57   Melanie enjoyed a recent gathering ...  (non-gold)
   ...
  32  0.565  0.50  0.30  0.64   Melanie values beach trips with her kids ...  <- GOLD
  34  0.543  0.50  0.28  0.59   Melanie viewed artwork inspired by the beach   <- (also relevant)
```
**Failure mode**: same as q71 — gold has the highest bm25 in the pool but graph penalty (0.5 vs 1.0) at 0.45 weight = 0.225 deficit, against +0.12 bm25 lift.

#### q75: "How many children does Melanie have?" (gold = "three kids")
Gold candidates at ranks 11, 135, 164 — but **all 192 candidates have graph=1.0**. So graph contributes the same 0.45 to every score → fusion collapses to `0.40 × text + 0.15 × recency`. Failure is **not** the formula; it's that no episode explicitly states the count. **q75 is the predicted IDK-by-construction case from the compaction summary — generator-not-fusion problem.** ISS-175 cannot help q75; it stays in the residual.

### Aggregate signal separation (4 qids, 5 gold rows, 865 non-gold rows)

| field           | gold mean | non mean | Δ      | sep ratio |
|-----------------|-----------|----------|--------|-----------|
| graph_score     | 0.79      | 0.79     | +0.00  | **near zero — signal is dead** |
| bm25_score      | 0.10      | 0.013    | +0.09  | **+7.5× — strongest gold predictor** |
| vector_score    | 0.57      | 0.52     | +0.05  | mild |

**bm25_score is the strongest gold discriminator across the pool**, but at fusion weight 0.40 (shared with vector via `text = max(bm25, vec)`), it's being neutralised by the `max()` operator: when vec > bm25, the bm25 signal is discarded entirely. For gold rows where bm25=0.25 but vec=0.62, `text = max(0.25, 0.62) = 0.62` — bm25 contribution is **erased**.

## Root cause synthesis

Two compounding bugs:

1. **B1 — graph_score saturation drowning**: On dense single-domain conversations (LoCoMo conv-26 has only 2 speakers, 6 months of dialogue, ~700 episodes all mentioning Caroline/Melanie), the entity-anchor extraction puts most candidates at 0-1 hops from query entities → graph_score ≥ 0.5 for nearly all candidates, with 1.0 being a plateau holding 19-100% of the pool. With graph_score at the largest fusion weight (0.45), this means **fusion is dominated by a signal with zero variance** on the bottom half of the pool.

2. **B2 — `text = max(vec, bm25)` discards the discriminating signal**: The `max()` aggregate (combiner.rs §5.2 "avoids double-counting") was designed to be conservative against highly-correlated vec/bm25, but for Factual queries the correlation is **anti-correlated** at the gold position — bm25 spikes on rare gold tokens ("Nicole", "abstract", "beach") that the vector embedding under-represents. `max()` then throws away the very signal that would lift the gold.

## Proposed reweighting formula (ISS-175 scope)

**Two-part change**, both Factual-only, both reversible via FusionConfig:

### Change A: replace `text = max(vec, bm25)` with **sum-with-evidence-bonus** for Factual

```rust
// Current (combiner.rs:~140 in fuse_factual):
let text = max_or_zero(scores.vector_score, scores.bm25_score);
final_score = 0.45 * graph + 0.40 * text + 0.15 * recency;

// Proposed:
let text = sum_with_evidence_bonus(scores.vector_score, scores.bm25_score);
// where:
fn sum_with_evidence_bonus(vec: Option<f64>, bm25: Option<f64>) -> f64 {
    let v = vec.unwrap_or(0.0);
    let b = bm25.unwrap_or(0.0);
    // 0.7×vec + 0.3×bm25 floor, plus +0.15 bonus if bm25 > 0.05
    // (= "rare lexical hit detected" signal, capped at 1.0)
    let base = 0.7 * v + 0.3 * b;
    let bonus = if b > 0.05 { 0.15 } else { 0.0 };
    (base + bonus).min(1.0)
}
```

**Why this shape**: it preserves the vec/bm25 anti-correlation case (gold gets the +0.15 evidence bonus because bm25 fires) without double-weighting in the dense case (gold AND many non-gold all have low bm25, bonus doesn't fire). 0.7/0.3 split keeps vector as the primary text signal (matches §5.2 intent).

### Change B: log-compress `graph_score` saturation for Factual

```rust
// Replace graph signal with:
fn compress_graph_factual(g: f64) -> f64 {
    // log(1 + g) / log(2)  — maps [0,1] -> [0,1] but flattens the top:
    //   g=1.0  -> 1.000  (unchanged)
    //   g=0.67 -> 0.738
    //   g=0.50 -> 0.585
    //   g=0.33 -> 0.412
    //   g=0.25 -> 0.322
    //   g=0.0  -> 0.000
    (1.0 + g).ln() / std::f64::consts::LN_2
}
```

**Why log not boost**: we want to **shrink** the dominance of g=1.0 plateau without inverting the ordering. Boosting low-hop candidates would help q40/q71 but break the existing assumption that "graph distance = topical relevance". Log compression keeps the ordering, just compresses the top.

### Predicted lift on q43/q40/q71

- **q43**: gold and top-1 both have g=1.0 → graph compression doesn't change anything → BUT change A gives gold +0 bonus (bm25=0). **No predicted lift.** This is a separate failure (graph + vec both saturated; needs ISS-174 reranker — not ISS-175.)
- **q40**: gold g=0.5 → compressed 0.585 (vs leader's compressed 1.0). Old deficit at gold: `0.45 × (1.0 - 0.5) = 0.225`. New deficit: `0.45 × (1.0 - 0.585) = 0.187`. Combined with text bonus (gold bm25=0.30 → +0.15 bonus + 0.7×0.64 + 0.3×0.30 = 0.538 vs old `max(0.30, 0.64) = 0.64` → text loss = -0.102 × 0.40 = -0.041). Net: 0.187 - 0.041 = 0.146 deficit (was 0.225). **~35% relative lift, but probably not enough to break top-10.**
- **q71**: gold g=0.33 → compressed 0.412. Old deficit: `0.45 × (1.0 - 0.33) = 0.302`. New: `0.45 × (1.0 - 0.412) = 0.265`. Combined with text bonus (gold bm25=0.25 → +0.15 bonus + 0.7×0.62 + 0.3×0.25 = 0.659 vs old `max(0.25, 0.62) = 0.62` → text gain +0.039 × 0.40 = +0.0156). Net: 0.265 - 0.016 = 0.249 deficit (was 0.302). **~18% relative lift.** Still drowned at rank 107 → would need entity-channel + reranker stacking to recover.

### Conclusion: ISS-175 alone is **necessary but not sufficient**

The probe confirms ISS-173 H5 (scoring drowns gold) but also reveals the lift from reweighting alone is modest — single-digit-pp on conv-26 best case. To hit AC-5a (single-fact ≥0.60), ISS-175 must stack with:

1. **ISS-174** (reranker arch) — for q43-style cases where graph+vec both saturate, only a cross-encoder can break ties.
2. **Entity-channel re-enable** (ISS-164 was inert — needs re-eval after ISS-175 lands)
3. Possibly **higher Factual k_seed** (currently producing 150-270 candidates; bm25 pool expansion didn't help in ISS-152 sweep, but might now that bm25 has real weight)

## ISS-174 architecture decision

The data argues for **option (b) — reweighting happens at the `HybridItem` variant emission**, NOT pre-fusion or in HybridDispatchExecutor:

- **Not (a) pre-fusion**: subscores are computed per-candidate before fusion. Reweighting at the subscore stage would require modifying `signals::compute_subscores` and would affect ALL plans (would break Episodic/Associative which currently rely on graph_score working). We want Factual-only.
- **Not (c) HybridDispatchExecutor**: dispatch executes a list of sub-plans (Factual/Episodic/Associative/Abstract/Affective). Reweighting at dispatch would mean re-fusing the already-fused outputs, double-work.
- **Yes (b) HybridItem emission**: the existing `combiner::fuse_factual()` function is the natural home — it already owns the per-plan weight matrix (§5.2). Add a `FusionConfig.factual_reweight: bool` knob (serde-default false → locked old behaviour). When true, fuse_factual uses the new formulas. This keeps the change local, auditable, and reversible.

**Recommendation**: file ISS-174 with scope = "FusionConfig.factual_reweight + combiner::fuse_factual_v2 + 3 unit tests pinning byte-identity at flag=false". Then ISS-175 ships the actual formula implementation behind that flag.

## Open issues for follow-up

- **q43 won't lift from ISS-175 alone** — needs ISS-174 reranker (cross-encoder) to break the saturated plateau. AC-5a will need both.
- **q75 is genuinely unanswerable from the corpus** — no episode says "three kids" explicitly. This is generator-not-fusion. Don't count q75 in ISS-175 success metric.
- **Retry layer did NOT fire** during this probe (counter stayed 0; Anthropic was clean). ISS-176 unit tests cover the logic but real-network validation is deferred to first transient blip.
- **Probe took 21min wall** (vs ~12min for normal LoCoMo conv-26 run) — the `factual_dump_hook` instrumentation adds ~70% overhead. Acceptable for one-off diagnostics; gate behind compile feature flag if we keep it long-term.

## Provenance

- Probe sweep script: `/tmp/iss175_probe_sweep.sh`
- Dumps: `/tmp/iss175-probe/dumps/conv-26-q{40,43,71,75}-factual.jsonl`
- Analysis script: `/tmp/iss175_analyse.py`
- Analysis output: `/tmp/iss175-probe/analysis-output.txt`
- Engram-bench build: 2026-05-27 17:46 EDT, includes ISS-176 retry layer (engram commit 0d61039)
- Engram-bench label-thread hook: commit 8d2ba46
- Engram fusion::dump hook: commit 7e088d4
