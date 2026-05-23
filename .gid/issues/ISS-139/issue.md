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

> **NOTE (2026-05-23):** the original sketch below predates code-layer
> investigation. The actual hook location and the existing `Reranker`
> trait are documented in "Hook location & architecture" further down.
> Keep this section for intent; ignore the file path and signature.

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

## Hook location & architecture (2026-05-23 code-layer pass)

### Existing infrastructure — the `Reranker` trait

`engramai/src/retrieval/fusion/reranker.rs` already defines a
`Reranker` trait with a 4-property contract (pure, bounded latency,
score-preserving, no-drop-only-reorder). A `NullReranker` ships as
the v0.3 default. Today **no plan invokes the trait** — it exists
ahead of any concrete reranker. MMR will be the first.

```rust
pub trait Reranker: Send + Sync {
    fn rerank(
        &self,
        query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError>;
}
```

Property contract (must satisfy all four for MMR to pass
`assert_reranker_contract`):

1. **Pure** — same `(query, candidates)` → same output. No `rand`,
   no clock, no globals. (MMR is naturally pure given fixed
   candidate set + λ.)
2. **Bounded latency** — implementation cooperates with
   `BudgetController::Stage::Rerank` (already wired in
   `retrieval/budget.rs:72`). For K=10 over a 200-candidate pool,
   MMR is ~1ms, well below any sane budget.
3. **Score preservation** — output scores MUST stay in `[0.0, 1.0]`
   and MUST NOT be NaN. MMR adjusts scores via the λ-blend formula;
   the blended score stays in `[0,1]` iff both `sim(c, query)` and
   `max sim(c, selected)` are in `[0,1]`. Cosine on unit-norm
   embeddings is in `[-1, 1]` — must clamp to `[0,1]` (negative
   means orthogonal-or-worse, treat as zero similarity).
4. **No-drop / reorder-only** — output multiset == input multiset.
   MMR satisfies trivially: it permutes; it never filters.

### Hook location — `api.rs:553` post-fusion truncate

The actual chokepoint is `retrieval/api.rs:545-553`:

```rust
let mut ranked = match plan_kind {
    crate::retrieval::dispatch::PlanKind::Hybrid => candidates,
    _ => crate::retrieval::fusion::fuse_and_rank(intent, &cfg, candidates),
};

// Top-K cutoff.
if ranked.len() > limit {
    ranked.truncate(limit);
}
```

All 7 plans (`Hybrid`, `Factual`, `Episodic`, `Associative`,
`Affective`, `Bitemporal`, `Abstract`) funnel through this one
truncate. **Reranker invocation goes between the two blocks** — after
`fuse_and_rank` produces a scored pool, before `truncate(limit)`
clips it. Wiring once here serves every plan.

The existing `.take(top_k)` in `plans/hybrid.rs:462` is downstream
of plan-level fusion (RRF over sub-plan results) and is **not** the
right hook — that's plan-internal limit, not the API boundary.
Leave hybrid.rs alone; do the rerank call in api.rs.

### Where do candidate embeddings come from?

`ScoredResult::Memory` carries a `MemoryRecord` which does **not**
include the embedding vector (see `crates/engramai/src/types.rs:162`
— no embedding field). MMR needs `sim(c, c')` per candidate pair,
which means looking up embeddings during rerank.

Lookup path already exists: `Storage::get_embedding(memory_id,
model) -> Result<Option<Vec<f32>>>` (storage.rs:3971) and
`Storage::get_embeddings_in_namespace` (storage.rs:4040, batched).

Two implementation strategies:

- **Strategy A: batched fetch inside rerank.** MMR reranker holds a
  `&Storage` ref via its constructor, calls
  `get_embedding(mem_id, model)` for each unique memory ID in the
  pool. One-shot SQL `WHERE memory_id IN (...)` is preferable, so
  expose a batched helper if not present. Adds ~5-15ms for a 200-
  candidate pool over a warm SQLite — likely within the rerank
  budget.
- **Strategy B: thread candidate embeddings through ScoredResult.**
  Add an optional `embedding: Option<Vec<f32>>` field to
  `ScoredResult::Memory` (or a parallel `EmbeddedScoredResult`
  wrapper). Set by the seed_recaller adapter when it builds
  candidates from vector search (it already has the embeddings in
  hand at that point). Reranker reads from the candidate.

**Recommend Strategy B for v0.3 MMR.** Embeddings the seed_recaller
already loaded from the vector index would otherwise get re-fetched
on the rerank hot path. Cost: one field added to a hot type; benefit:
zero extra SQL calls during rerank. Risk: ~1.5KB per candidate × 200
candidates = ~300KB of transient memory per query — acceptable.

If Strategy B is unacceptable (e.g. `ScoredResult` is part of a
public API that can't be extended without breaking changes), fall
back to A.

### Config knob — `FusionConfig` extension, not feature flag

`retrieval::fusion::FusionConfig` (used at api.rs line ~540) already
holds the live config. Add:

```rust
pub struct FusionConfig {
    // … existing fields …

    /// MMR diversity λ ∈ [0.0, 1.0]. 1.0 = pure relevance
    /// (NullReranker behavior, current default). 0.5 = balanced.
    /// 0.0 = pure diversity (don't use). See ISS-139.
    pub mmr_lambda: f32,
}

impl FusionConfig {
    pub const fn locked() -> Self {
        Self {
            // … existing …
            mmr_lambda: 1.0,  // default off
        }
    }
}
```

Then in `api.rs`:

```rust
let reranker: Box<dyn Reranker> = if cfg.mmr_lambda < 1.0 {
    Box::new(MmrReranker::new(self.storage(), cfg.mmr_lambda))
} else {
    Box::new(NullReranker::new())
};
let ranked = reranker.rerank(&query_text, &ranked)?;

if ranked.len() > limit {
    ranked.truncate(limit);
}
```

`mmr_lambda = 1.0` is the existing-behavior preserve case (formula
collapses to `score_mmr = sim(c, query)` = original RRF score, no
reorder). This means the default config produces byte-identical
output, preserving the ISS-100 cross-validate envelope (AC #5
above).

### Module layout

New file: `engramai/src/retrieval/fusion/mmr.rs` (parallel to
`reranker.rs`, exports `MmrReranker`). Avoids growing `reranker.rs`
beyond its current "trait + contract test helper" role.

### Test plan

- Unit: `MmrReranker` satisfies `assert_reranker_contract` at
  λ ∈ {0.0, 0.5, 0.7, 0.9, 1.0}.
- Unit: λ=1.0 produces byte-identical output to input (regression
  guard for default-off behavior).
- Property: candidate `(c1, c2)` highly-similar pair → at λ=0.5
  exactly one of them appears in top-K (diversity bites).
- Integration: synthetic 4-cluster pool, K=4 → λ=0.7 picks one
  per cluster; λ=1.0 picks 4 from densest cluster.
- LoCoMo: 3 temp=0 runs at λ=0.7, K=10 vs K=10 baseline (AC #2-4
  above).

### Out of scope

- λ auto-tuning per query (could be a follow-up — heuristic on
  query length / list-marker words).
- MMR on `Topic` results (`ScoredResult::Topic`). Topic similarity
  needs a different embedding strategy; skip for v0.3.
- Cross-encoder rerank (ISS-140) — orthogonal concern; would compose
  with MMR by either chaining rerankers or running MMR on the
  cross-encoded top-50.

## Acceptance criteria

1. `MmrReranker` implemented in
   `engramai/src/retrieval/fusion/mmr.rs`, satisfies
   `assert_reranker_contract` at λ ∈ {0.0, 0.5, 0.7, 0.9, 1.0}.
   Wired through `FusionConfig::mmr_lambda` (default 1.0 = current
   behavior, no MMR). See "Hook location & architecture" below for
   why this lives at `api.rs:553` (the single API-boundary truncate),
   not inside each plan.
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
