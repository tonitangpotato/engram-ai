---
title: Fusion BM25 channel is dead code — all plan adapters leave bm25_score=None
status: open
priority: P0
severity: bug
category: retrieval
created: 2026-05-24
relates: [ISS-144, ISS-145, ISS-146]
---

## Summary

Design §5.2 specifies fusion as `text = max(vector_score, bm25_score)`
across 4 of 5 plans (Factual / Episodic / Abstract / Affective),
with `text` weighted 0.40–0.60. The intent is hybrid lexical+semantic
retrieval: when the embedding model fails on a paraphrase query but
the answer episode contains a specific named entity (place name,
person, date), BM25 catches it.

**In production, `bm25_score` is permanently `None`.** No plan
adapter populates it. The fusion combiner reduces to embedding-only
ranking with proportional weight redistribution. This is a major
design/implementation gap that explains a large fraction of
single-hop failures observed in LoCoMo.

## Evidence

### 1. Code search (zero production writes to `bm25_score`)

```text
$ grep -rn 'bm25_score\s*:' crates/engramai/src/ --include='*.rs' | grep -v test
crates/engramai/src/retrieval/api.rs:297:    pub bm25_score: Option<f64>,
```

Only the type definition. The 4 `Some(...)` references in
`combiner.rs` are inside `#[cfg(test)]` blocks.

### 2. No FTS query in any plan module

```text
$ grep -rn 'search_fts\|fts_score\|bm25' crates/engramai/src/retrieval/plans/
(no matches)
```

All 7 plan modules (`factual.rs`, `episodic.rs`, `associative.rs`,
`abstract_l5.rs`, `affective.rs`, `bitemporal.rs`, `hybrid.rs`)
contain zero references to FTS, BM25, or `search_fts`. The
`search_fts` function exists in `storage.rs` and was switched to
read `nodes_fts` in T29.6 (commit `9ecb684`), but no caller invokes
it from the per-plan retrieval path.

### 3. Adapter audit (`orchestrator.rs`)

What each `<plan>_to_scored` adapter actually populates in `SubScores`:

| adapter                  | populates                              |
|--------------------------|----------------------------------------|
| `factual_to_scored`      | `graph_score` only                     |
| `episodic_to_scored`     | `recency_score` only                   |
| `associative_to_scored`  | `vector_score` + `graph_score`         |
| `abstract_to_scored`     | (Topic only — no SubScores at all)     |
| `affective_to_scored`    | `affect_similarity` + others           |
| `hybrid_to_scored`       | (RRF — no SubScores path)              |

The Factual case is the worst: design says
`Factual: final = 0.45 * graph + 0.40 * text + 0.15 * recency`,
but the adapter populates only `graph_score`. Combiner's missing-signal
renormalization (§5.2) then redistributes the 0.40 text and 0.15
recency weights proportionally to graph, so **Factual in production =
100% graph_score**. Similarly Episodic = 100% recency, Abstract =
100% actr (when Topic isn't used), etc.

### 4. Diagnostic that motivated finding the bug

Ran `iss146-embed-diag.py` (pure Ollama `nomic-embed-text` cosine sim
between query and every episode, no engram involvement) on 4
single-hop failures from conv-26 L1-only run:

| qid | question | gold | gold ep | rank on pure embedding |
|-----|----------|------|--------:|-----------------------:|
| q11 | Where did Caroline move from 4 years ago? | Sweden | ep#60 | **319 / 419** |
| q15 | What activities does Melanie partake in?  | pottery, camping, painting, swimming | 21 gold eps | only 3 in top-50 |
| q18 | Where has Melanie camped?                 | beach, mountains, forest | 4 gold eps | only 1 in top-10 |
| q19 | What do Melanie's kids like?              | dinosaurs, nature | ep#65, #97 | both >100 |

q11 is the cleanest case: "Sweden" appears literally **once** in 419
episodes (ep#60 "necklace from grandma in my home country, Sweden").
Any working BM25 ranks ep#60 at #1 with overwhelming margin. Pure
embedding ranks it 319/419 because the query "Where did Caroline
move from 4 years ago?" embeds close to generic move/relocation chat,
not to a necklace-and-grandma anecdote.

**This failure mode is unfixable by MMR, by re-ranking, or by L1b/L2
entity resolution.** It is unfixable by anything *downstream* of
candidate retrieval. The only fix is to put the literal-string-match
candidate (ep#60) into the candidate set in the first place — which
is exactly what BM25 is supposed to do.

## Acceptance criteria

- [ ] AC-1: Each of Factual / Episodic / Abstract / Affective plan
  adapters invokes `Storage::search_fts*` (or equivalent) on the
  query text and populates `SubScores.bm25_score` with the
  saturation-normalized BM25 result for each candidate.
- [ ] AC-2: New `bm25_score` is normalized via existing
  `signals::bm25_score(raw, BM25_DEFAULT_SATURATION)` helper (no
  new scoring math — reuse what's already designed).
- [ ] AC-3: Per-candidate `bm25_score` defaults to `Some(0.0)` for
  candidates that match in the plan's primary path but have no FTS
  hit (not `None` — `None` triggers weight redistribution and
  effectively penalises them).
- [ ] AC-4: Regression test: ingest a corpus with a single
  literal-string-only-match episode, query with a paraphrase that
  has zero embedding overlap, verify the target episode appears in
  top-K. The conv-26 q11 "Sweden" case is the natural fixture.
- [ ] AC-5: LoCoMo conv-26 single-hop accuracy crosses 0.40 (4×
  current 0.0625 baseline). Multi-conv full run shows ≥+5pp overall
  vs ISS-146 post-flip baseline (0.4671).
- [ ] AC-6: 1946+ existing lib tests stay green. New tests for
  BM25-aware fusion path added.

## Implementation sketch

For each affected plan adapter:

```rust
// Pseudo-code in orchestrator.rs adapter
let bm25_hits: HashMap<MemoryId, f64> = storage
    .search_fts(query_text, k_seed * 2)
    .into_iter()
    .map(|(id, raw_bm25)| (id, signals::bm25_score(raw_bm25, BM25_DEFAULT_SATURATION)))
    .collect();

// ... in the per-candidate loop:
let bm25 = bm25_hits.get(&record.id).copied().unwrap_or(0.0);
let sub_scores = SubScores {
    graph_score: Some(graph_score),
    bm25_score: Some(bm25),
    // ...
};
```

Key questions to resolve before implementation:

1. **Should FTS run for *every* query or be gated?** FTS is cheap
   on `nodes_fts` (already tested to handle the LoCoMo corpus size).
   Default to "always run" with a config knob for benchmark
   reproducibility.
2. **What about queries with no FTS hits at all?** Pass `Some(0.0)`
   for all candidates (uniform 0 contribution) rather than `None`
   (which would penalise via renormalization). See AC-3.
3. **Saturation constant tuning.** Current default
   `BM25_DEFAULT_SATURATION = 20.0` was set for v0.3 single-conv
   benchmarks. May need re-tuning for full LoCoMo corpus where IDF
   distributions differ. Track separately if AC-5 isn't met.

## Estimated effort

- ~150-300 LoC across 4 adapters + 1 storage helper
- ~30-50 LoC of new tests
- 2-4 hours implementation + 1 hour for AC-5 benchmark validation

## Expected impact

If the q11/q18/q19 hypothesis holds (specific-entity queries are
embedding-paraphrase-failures rescuable by literal match), expected
single-hop lift: **0.0625 → 0.40+** (6×). Possibly bigger — same
class of failure likely dominates other conversations too, not just
conv-26.

This is the highest-ROI lever currently identified for LoCoMo
single-hop accuracy.
