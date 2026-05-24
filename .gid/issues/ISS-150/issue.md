---
title: Wire BM25 channel into Associative plan adapter (ISS-147 follow-up, blocks ISS-148 AC-5)
blocks: ISS-148
priority: P0
relates_to:
  - ISS-147
  - ISS-148
  - ISS-149
  - ISS-145
severity: degradation
status: open
tags:
  - retrieval
  - fusion
  - bm25
  - locomo
  - iss-148-blocker
---

# ISS-150 — Wire BM25 into Associative adapter

## TL;DR

ISS-147 wired BM25 into Factual / Episodic / Affective adapters but
**explicitly excluded** Associative on the assumption that
"Associative + Hybrid use RRF (not SubScores)". That assumption is
wrong for the Associative path. The combiner runs Associative results
under the **dispatched intent** (Factual for L1, Episodic for L2,
etc.) and `FusionConfig::locked()` gives Factual a `text = 0.40`
weight. With `bm25_score = None`, the text channel collapses to
`max(vector, None) = vector` — i.e. BM25 is dead code for ~80% of
LoCoMo conv-26 queries that fall back to Associative.

This blocks **ISS-148 AC-5** (`single-hop ≥ 0.40` on conv-26). The
ISS-147 BM25 wiring only touched ~5% of conv-26 queries (the ones
that successfully ran a Factual plan). The other 95% — every
classifier-blind query (ISS-149) and every resolver-blind query
(ISS-145) that downgrades to Associative — sees zero BM25 lift.

## Evidence

### 1. ISS-147 wired only the easy 5%

`crates/engramai/src/retrieval/orchestrator.rs:543` (post-ISS-147
comment):

```
// Abstract returns Topic variants (no SubScores), Associative + Hybrid
// use RRF (not SubScores) — intentionally not wired per the issue
// audit.
```

This is true for **standalone** Associative + Hybrid plans, but
Associative-as-fallback is dispatched through `combine()` not RRF.
See `run_associative_fallback` at orchestrator.rs:1471 — it calls
`associative_to_scored(&result, loader)` then returns into the same
`fuse_and_rank` path as any other plan.

### 2. Combiner weights `text` for the dispatched intent

`crates/engramai/src/retrieval/fusion/combiner.rs:179-189`
(Factual weights from `FusionConfig::locked()`):

```rust
let factual = FusionWeights {
    text: 0.40,
    vector: 0.0,
    graph: 0.45,
    recency: 0.15,
    actr: 0.0, affect: 0.0,
};
```

And combiner.rs:268-274:

```rust
let text_score: Option<f64> = match (sub.vector_score, sub.bm25_score) {
    (Some(v), Some(b)) => Some(v.max(b)),
    (Some(v), None) => Some(v),  // ← Associative path today
    (None, Some(b)) => Some(b),
    (None, None) => None,
};
```

With `bm25_score = None` (the current Associative SubScores), the
text channel is `Some(vector)` — i.e. the 0.40 text weight is
silently re-routed to vector-only ranking. Populating `bm25_score`
flips this to `max(vector, bm25)`, which is exactly the lift ISS-147
delivered for Factual / Episodic / Affective.

### 3. Three-stage failure traced

ISS-148 root cause is a three-stage failure, not two:

1. **Classifier blind** (ISS-149) — `Classifier::with_entity_lookup`
   uses `NullEntityLookup` in bench, so anchored queries dispatch as
   plain `Factual` even when they should be `Anchored`.
2. **Resolver blind** (ISS-145) — `GraphEntityResolver::search_candidates`
   reads from `graph_entity_aliases` which the bench never writes to.
   Result: `FactualPlanResult.anchors = []` → `EntityFoundNoEdges`.
3. **Factual fallback** — even when (1) and (2) work, `graph_edges`
   is empty (no `ResolutionPipeline` running in the bench harness),
   so `FactualPlan::execute` returns `EntityFoundNoEdges` →
   `run_associative_fallback` → Associative adapter → `bm25_score = None`.

ISS-145 Option D (read-switch `search_candidates` to `nodes`) was the
plan but hit a Uuid type-mismatch blocker (nodes.id is 16-hex FNV-1a,
CandidateMatch.entity_id is Uuid). Even if ISS-149 + ISS-145 ship,
stage 3 still drops BM25.

**This issue fixes stage 3 directly** — the 80% bucket — and is
independent of ISS-145 / ISS-149.

## Plan

Mirror ISS-147 commit 5ed5dc0 exactly:

### Step 1: Thread `bm25_by_id` into `associative_to_scored`

`crates/engramai/src/retrieval/orchestrator.rs`:

- Add `bm25_by_id: &HashMap<String, f64>` as 3rd parameter.
- Populate `SubScores.bm25_score = Some(bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0))`
  per AC-3 (Some(0.0) for FTS misses, NOT None — None triggers
  missing-signal renormalisation which would defeat the lexical
  channel).
- Update the call at orchestrator.rs:1140 (`PlanKind::Associative`
  arm in `execute_plan`) to pass `&bm25_by_id` — `bm25_by_id` is
  already in scope, computed once per query at line 984.

### Step 2: Recompute BM25 inside `run_associative_fallback`

`run_associative_fallback` is called 4× from `execute_plan` (Factual
/ Episodic / Abstract / Affective downgrade arms). The cleanest shape
is the one `run_factual_fallback_for_hybrid` already uses
(orchestrator.rs:1343): compute a fresh `bm25_by_id` inside the
fallback using `loader.fts_scores(&query.text, (query.limit * 4).max(40))`.

Rationale: threading the outer `bm25_by_id` down 4 call sites is
noisier than 1 extra SQL roundtrip per fallback (same pool sizing
as the primary path). The K_seed pool is the same `(K*4).max(40)`
ISS-147 picked.

### Step 3: Update the misleading comment

Remove or correct the orchestrator.rs:543 comment claiming
"Associative + Hybrid use RRF (not SubScores)". Associative-as-fallback
is `combine()` not RRF.

### Step 4: Bench

Conv-26 only smoke (already the ISS-148 baseline grid):

```bash
ENGRAM_BENCH_TOP_K=10 ENGRAM_BENCH_MMR_LAMBDA=0.7 \
  ENGRAM_BENCH_LOCOMO_CONVS=conv-26 \
  nohup engram-bench ...
```

Compare via `engram-bench/scripts/diagnostics/iss147_compare.py`
against ISS-144 L1-only baseline (overall=0.4408, single-hop=0.1562,
multi-hop=0.6216, open-domain=0.3077, temporal=0.5000).

## Acceptance criteria

- [ ] `associative_to_scored` populates `SubScores.bm25_score`
      (never `None` for present records, `Some(0.0)` for FTS misses).
- [ ] `run_associative_fallback` runs an FTS pass with
      pool=`(limit*4).max(40)` and threads the scores into
      `associative_to_scored`.
- [ ] Single SQL roundtrip per primary `PlanKind::Associative`
      query (shares `bm25_by_id` already computed in `execute_plan`).
      Fallback adds 1 roundtrip per downgrade (acceptable per
      `run_factual_fallback_for_hybrid` precedent).
- [ ] `cargo test -p engramai --lib` green.
- [ ] Conv-26 LoCoMo K=10 λ=0.7 single-hop ≥ 0.40 (this is the
      ISS-148 AC-5 gate this issue unblocks). If short of 0.40 but
      ≥ 0.30, log the gap and reassess: ISS-145/149 stages may
      still be required.
- [ ] **No regression** on conv-26 multi-hop (currently 0.5950 at
      L1-only baseline; 0.6486 at K=10 baseline). Associative is
      also the multi-hop hot path — wiring BM25 must not hurt the
      good direction.

## Open questions for potato

1. **K_seed for fallback** — ISS-147 uses `(K*4).max(40)` for the
   primary path. Should fallback use the same, or wider given it's
   the dominant 80% bucket on conv-26? Recommend: same as ISS-147,
   measure, adjust if conv-26 underperforms.
2. **Risk of multi-hop regression** — Associative is the path
   multi-hop takes too. Mitigation: bench compares both single-hop
   and multi-hop deltas; rollback if multi-hop drops > 2pp.

## Non-goals

- Does NOT fix the classifier blindness (that's ISS-149).
- Does NOT fix the resolver blindness (that's ISS-145).
- Does NOT touch Hybrid's RRF path (separate concern; Hybrid
  `hybrid_to_scored` ID-mapping has its own bug per ISS-061).
