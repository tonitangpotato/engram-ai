---
id: ISS-139
title: 'Retrieval: add MMR (Maximal Marginal Relevance) diversity step in top-K selection for list-style questions'
status: open
priority: P1
severity: degradation
labels:
- retrieval
- ranking
- mmr
- locomo
relates_to:
- ISS-069
- ISS-138
filed: 2026-05-23
filed_by: rustclaw
depends_on: .gid/issues/ISS-138/issue.md
---

## Problem

For "list" questions (gold = 2-4 distinct items across separate dialog
turns from different sessions), the current top-K returns near-duplicate
candidates clustered around the query embedding. This systematically
misses list items mentioned in other contexts.

### Concrete failure (conv-26 q15)

- **Question:** "What activities does Melanie partake in?"
- **Gold:** "pottery, camping, painting, swimming" (4 items)
- **Evidence:** D5:4 (pottery), D9:1 (camping), D1:12 (painting), D1:18 (swimming)
- **Run-1 prediction:** "Melanie signed up for a pottery class"
  (1/4 items mentioned) — judge scored 0.0

The retriever ranked 4 pottery-related turns from D5 in the top-5 because
they cluster tightly around the embedding of the word "activities" + the
single most-discussed activity (pottery). The other 3 items (camping,
painting, swimming) were each in less-talked-about sessions and ranked
lower than the pottery cluster's tail.

### Pattern across single-hop list failures (2026-05-23, conv-26)

13/29 single-hop failures match this structure:

| qid | gold list items | evidence turns from N sessions | pred items |
|-----|-----------------|--------------------------------|-----------|
| q15 | 4 | 4 sessions | 1 |
| q18 | 3 | 3 sessions | 0 |
| q19 | 2 | 2 sessions | 0 |
| q23 | 2 | 2 sessions | 0 |
| q24 | 2 | 2 sessions | 1 |
| q32 | 3 | 4 sessions | 1 |
| q34 | 2 | 2 sessions | 1 |
| q39 | 4 | 4 sessions | 2-3 |
| q43 | 1 | 3 turns same session | 0 |
| q48 | 2 | 3 sessions | 1 |
| q51 | 3 | 2-3 sessions | 2 |
| q60 | 2 | 2 sessions | 1 |
| q70 | 2 | 2 sessions | 0 |

Across these 13 queries, gold evidence is spread across **2-4 distinct
sessions** in 11/13 cases. Top-K is concentrated in 1-2 sessions because
those sessions have the highest discussion density of the query topic.

## Proposed fix: MMR re-ranking in fuse_rrf or post-fuse

Replace plain top-K with **MMR top-K**:

```
score_mmr(c) = λ · sim(c, query) - (1-λ) · max_{c' ∈ selected} sim(c, c')
```

- λ ∈ [0.5, 0.8] empirically; start at 0.7 (balanced)
- `sim(c, c')` = cosine on memory embeddings (already available)
- Implemented inside `fuse_rrf` after candidate-pool assembly, before
  truncation to top-K

The effect: candidates that look semantically distinct from already-
selected ones get a score boost, so list items from different sessions
surface even if their raw fusion score is lower.

## Implementation sketch

Touchpoint: `engramai/src/retrieval/fuse.rs` (or wherever fuse_rrf lives).

```rust
fn mmr_select(
    candidates: Vec<ScoredCandidate>,  // sorted desc by score
    embeddings: &EmbeddingStore,
    k: usize,
    lambda: f32,
) -> Vec<ScoredCandidate> {
    let mut selected: Vec<ScoredCandidate> = Vec::with_capacity(k);
    let mut remaining: Vec<ScoredCandidate> = candidates;
    while selected.len() < k && !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_score = f32::MIN;
        for (i, c) in remaining.iter().enumerate() {
            let max_sim_to_sel = selected.iter()
                .map(|s| cosine(&embeddings[c.id], &embeddings[s.id]))
                .fold(0.0_f32, f32::max);
            let mmr = lambda * c.score - (1.0 - lambda) * max_sim_to_sel;
            if mmr > best_score { best_score = mmr; best_idx = i; }
        }
        selected.push(remaining.swap_remove(best_idx));
    }
    selected
}
```

Cost: O(K · pool_size · D) where D=embedding dim. At K=10, pool=200,
D=1536 → 3M FLOPs per query, ~1ms. Negligible.

## Acceptance criteria

1. MMR implemented behind feature flag or config knob `mmr_lambda`
   (default 1.0 = current behavior, i.e. no MMR)
2. With `mmr_lambda=0.7` and K=10, single-hop recall@K rises from 17.3%
   (K=10 plain) to ≥ 25% (target: visible diversity lift on list
   questions)
3. Multi-hop recall does NOT regress (currently 70.3% @ K=10) — MMR can
   hurt focused questions if λ too low
4. Three temp=0 LoCoMo runs with MMR confirm overall J-score ≥ 0.42
5. Re-check ISS-100 envelope at K=10 + MMR=0.7

## Risk

- λ tuning is the main risk. Too low (0.5) → diversity dominates,
  multi-hop regresses. Too high (0.9) → barely any diversity.
- Need to validate on conv-25 / conv-27 too, not just conv-26 (avoid
  overfitting).

## Order in roadmap

After ISS-138 (K=10 baseline). ISS-138 is the floor; this is the
ceiling for list-question recall. Cannot evaluate MMR honestly until
we have the K=10 baseline measured.
