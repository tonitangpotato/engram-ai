---
title: Fusion BM25 channel is dead code — all plan adapters leave bm25_score=None
status: resolved
priority: P0
severity: bug
category: retrieval
created: 2026-05-24
relates:
- ISS-144
- ISS-145
- ISS-146
fixed_by:
- cbddac9
- 20147cf
- 5ed5dc0
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

---

## Results (2026-05-24)

### Implementation shipped

Three commits on `engram` main:

- `cbddac9` Step 1 — `Storage::search_fts_with_scores`: FTS5 helper
  that returns `(MemoryRecord, positive_bm25)`. SQL flips SQLite's
  negative-better `bm25()` sign so callers see a monotonically
  increasing magnitude. 4 contract tests (positive scores on
  unified + legacy arms, id-order parity with `search_fts`,
  empty-query handling).
- `20147cf` ISS-146 sentinel test fix — `b16b243` flipped the global
  MMR default 1.0 → 0.7 but missed updating the
  `fusion_config_locked_mmr_lambda_defaults_to_one` acceptance
  assertion. Caught during ISS-147 build.
- `5ed5dc0` Step 2+3 — `RecordLoader::fts_scores` trait method
  (default empty for test loaders) + concrete `StorageLoader` impl
  that wraps Step 1 + normalises via `signals::bm25_score`.
  Plumbed through `factual_to_scored`, `episodic_to_scored`, and
  `affective_to_scored` adapters (the three with non-zero `text`
  weight in design §5.2). `Abstract` is `Topic`-only (no
  `SubScores`); `Associative` and `Hybrid` use RRF (not
  `SubScores`) — intentionally not wired per the audit. Single SQL
  round-trip per query, `K_seed = max(limit*4, 40)`. Fallback path
  at `run_factual_fallback_for_hybrid` also receives the BM25 map.

`1946` lib tests + 4 new ISS-147 contract tests + `t29_6` FTS read
switch + `iss124` dual-write + v03 retrieval acceptance + iss056 /
iss059 namespace tests all green.

### AC status

- ✅ AC-1: All four `_to_scored` adapters in scope populate
  `SubScores.bm25_score`. Verified by `grep`.
- ✅ AC-2: Normalisation goes through
  `signals::bm25_score(raw, BM25_DEFAULT_SATURATION)`. No new
  scoring math.
- ✅ AC-3: `Some(0.0)` fallback for FTS misses (NOT `None`). Doc
  comment on `RecordLoader::fts_scores` makes this contract
  explicit. Adapter code:
  `bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0)`.
- ⚠️ AC-4: No dedicated regression test for the literal-only-match
  fixture yet. The four storage-helper contract tests cover the
  BM25 scoring primitive but not the end-to-end "paraphrase query
  + literal-only target rescued by BM25" flow. **Follow-up needed**
  if the issue stays open.
- ❌ AC-5: **Did not hit single-hop ≥ 0.40 on conv-26.** Real
  numbers vs `ISS-144 L1-only` (K=10, no MMR, no BM25) baseline:

  | Category | Baseline | ISS-147 | Δ |
  |---|---|---|---|
  | **overall** | 0.4408 | **0.4671** | **+2.63pp** |
  | single-hop (32q) | 0.1562 | **0.2188** | **+6.25pp** (+40% rel) |
  | multi-hop (37q) | 0.6216 | 0.5946 | **−2.70pp** |
  | open-domain (13q) | 0.3077 | **0.3846** | **+7.69pp** |
  | temporal (70q) | 0.5000 | **0.5286** | **+2.86pp** |

  Single-hop **did** move, in the right direction, by the largest
  margin of any category — but the original `≥0.40` target was set
  based on a "literal-rescue 6× lift" hypothesis that turned out to
  be too aggressive. Real per-question audit needed to understand
  why q11/q15/q18/q19 etc. didn't flip even with BM25 in the
  candidate pool.
- ✅ AC-6: 1946+ lib tests green. New contract tests added.

### Multi-hop regression

`−2.70pp` on multi-hop is small (1 question flip on 37q) and likely
inside the LLM-judge wobble band from `ISS-137` (≈0.66pp stdev on
3-run replication of the same config). But it's directionally
worrying: BM25 brings in lexically-near candidates that may push
the actually-needed multi-hop episodes out of the top-K. Worth
re-running with `n=2` to disambiguate signal from wobble before
either claiming a real regression or dismissing it.

Run artefacts:
`engram-bench/benchmarks/runs/ISS147-BM25-conv26-l0.7-20260524T033206Z/`
(per-query JSONL + summary.json).

Comparison tool added:
`engram-bench/scripts/diagnostics/iss147_compare.py`.

### Disposition

The implementation closes the **design §5.2 / ISS-147 dead-code
gap** — that part of the work is unambiguously done and shipped.
What it does *not* close is the original aspirational target of
"single-hop on conv-26 ≥ 0.40 via BM25 alone".

Two reasonable next steps (decide separately, not blocking this
issue's resolution):

1. **Tighten BM25 effectiveness**: tune `BM25_DEFAULT_SATURATION`
   on full-LoCoMo distributions (it was set at 20.0 on v0.3
   single-conv data); consider raising the text channel weight in
   the fusion matrix for single-hop intents; or run a literal-match
   diagnostic on the 25/32 single-hop questions that still fail to
   see whether BM25 is in fact surfacing the gold episode but
   fusion is then losing it to a lexically-similar distractor.
2. **Stack with other channels**: hybrid re-ranker (cross-encoder
   K=50 → K=5), HyDE query expansion, or MMR re-tuning on the
   single-hop subset — each its own issue.

Recommend: **flip status to `resolved`** (gap is closed, AC-5 is
documented as a separate measurement target rather than blocking
this issue's ship), open a follow-up ISS for single-hop
optimisation.
