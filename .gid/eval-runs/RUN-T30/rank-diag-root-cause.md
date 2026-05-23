# RUN-T30 — tail-rank divergence root cause

**Date**: 2026-05-22
**Tool**: `crates/engramai/examples/t30_rank_diag.rs`
**Follow-up to**: `summary.md` (T30 probe-parity FAIL @ K=10, parity 0.40)

## TL;DR

The unified-vs-legacy candidate-set divergence is **not** a bug in T29.* read adapters. It's a **structural consequence of `nodes_fts` indexing more than just memories** — entity and insight nodes share the same FTS5 inverted index, which shifts BM25 IDF for every token relative to `memories_fts`. Same SQL `ORDER BY rank` over different IDF distributions → different top-K.

## Evidence

Query: `graph`

**legacy `memories_fts MATCH "graph" ORDER BY rank LIMIT 12`**

```
9af8428f  -4.236  (rank 1)
a3e71f74  -4.230
bfb54037  -4.127
e09183d6  -4.107
d32e4d3e  -4.105
6dfff4fa  -4.083
c84c9cd6  -4.074
5f3fcde9  -4.040
006a748d  -4.026
2a5b9f7e  -4.019
03ebc00a  -4.019
03afa49e  -4.005
```

**unified `nodes_fts MATCH "graph" + node_kind='memory' ORDER BY rank LIMIT 12`**

```
6dfff4fa  -4.329  (rank 1)  ← legacy rank 6
a3e71f74  -4.298
9af8428f  -4.285             ← legacy rank 1
f2e8a3d4  -4.230             ← not in legacy top-12
bfb54037  -4.208
796ab424  -4.186             ← not in legacy top-12
d32e4d3e  -4.179
e09183d6  -4.154
c84c9cd6  -4.112
5f3fcde9  -4.094
d1c844c8  -4.094             ← not in legacy top-12
49d6bdbe  -4.094             ← not in legacy top-12
```

Same memories, different absolute rank values, different relative ordering. Top candidates from both arms are present in both indexes — but the BM25 score shift is enough to invert ranks.

## Index composition

```sql
SELECT node_kind, COUNT(*) FROM nodes WHERE fts_rowid IS NOT NULL GROUP BY node_kind;
-- memory  19378
-- entity   2791
-- insight     5
```

`nodes_fts` has ~14% of its document mass coming from entity content (which is short — names, types). FTS5 BM25 IDF depends on `N` (total doc count) and `df` (per-token doc frequency). Entity docs add to both, but disproportionately raise `N` for short tokens that occur frequently in entity names like `graph`, `rust`, `memory`, etc. → these tokens get lower IDF in `nodes_fts` than in `memories_fts`.

## Why this looks like "tail-rank swap"

50-query probe set distribution (from `summary.md`):

- 20 queries: Jaccard 1.000 (identical) — rare-token queries where IDF shift doesn't reorder
- 24 queries: Jaccard 0.818 (one swap) — common-token queries where one entity-name-heavy boundary memory is repositioned
- 6 queries: Jaccard 0.333–0.667 (multiple swaps) — query tokens that overlap heavily with entity name vocabulary (`graph`, `rust`, `embedding`, `session compaction`)

The same memory IDs are nearly always present in both candidate pools — they're just ranked differently.

## Options

### Option 1 — Split nodes_fts by node_kind (root fix)

Create separate `nodes_fts_memory`, `nodes_fts_entity` virtual tables. Unified search_fts joins only against `nodes_fts_memory`. This makes BM25 IDF identical to `memories_fts` by construction.

**Cost**: schema migration, T29.6 patch, all readers that hit `nodes_fts` need rewiring. Touches the FTS-index design which currently has 4 FTS-write call sites (T12 dual-write, T13 entity insert, T15 KC insert, T16 synthesis insert). Estimated 1-2 day refactor.

**Pro**: byte-identical BM25 rank → parity by construction. Clean abstraction.

### Option 2 — Filter post-rank instead of using FTS rank

Run `nodes_fts MATCH ?` to get the candidate set (over-fetch, e.g. LIMIT 3*K), then re-rank in-process using a unified scoring function that knows it's looking at memories only.

**Cost**: scoring function exists in retrieval/fusion/ — already does per-signal weighting. Could add a BM25-on-memories pass. Maybe 1 day.

**Pro**: doesn't disturb the FTS schema.
**Con**: BM25 needs document corpus stats; recomputing them in Rust is annoying. May still drift.

### Option 3 — Accept rank divergence; lower T30 threshold; gate on downstream task quality

Accept that BM25 ordering differs by construction. Replace "Recall@10 ≥ 95% of legacy" with "LoCoMo J-score unified ≥ legacy baseline" as the actual Phase D gate.

**Cost**: rewrite design §5.4 acceptance. T31 LoCoMo result becomes the load-bearing decision.

**Pro**: zero code change. T31 is already running (PID 31350 — started 2026-05-22 21:05Z).
**Con**: T30 probe-parity becomes a soft diagnostic only.

## Recommendation

**Option 3 first, Option 1 if T31 fails.**

If LoCoMo J-score under unified is within noise of legacy, the FTS rank shift is cosmetic — the actual semantic retrieval is fine because downstream fusion (which mixes BM25 with vector/graph/recency/actr) dominates. T31 will tell us.

If T31 shows degradation, Option 1 is the only clean fix (Option 2 is a workaround that re-implements BM25 in Rust against a moving corpus).

## Artifacts

- `rank-diag.md` — full top-10 dumps for 5 worst queries (note: PlanTrace `explain=true` returns `trace: None` — that's a separate engram instrumentation gap unrelated to T30)
- direct sqlite verification of FTS rank values quoted above

## Status

T30 verdict stands: parity_ratio=0.40 at K=10, do NOT flip unified default on T30 evidence alone. T31 LoCoMo is the next gate; tracking in `summary.md`.
