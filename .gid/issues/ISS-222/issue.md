---
title: get_embeddings_for_ids unified branch JOINs empty legacy memories table — vector signal dead in Factual/MMR since T32
status: open
priority: P0
labels: retrieval, regression, phase-e, read-cutover
relates_to: ISS-199, ISS-221, ISS-201, ISS-139
discovered_by: ISS-221 probe (2026-06-11)
---

# get_embeddings_for_ids unified branch JOINs empty legacy `memories` table — vector signal dead since T32

## Summary

`Storage::get_embeddings_for_ids` (storage.rs:4917) is a **missed ISS-199 Phase-E read-cutover site**. Its `unified_substrate` branch does:

```sql
SELECT e.node_id, e.embedding
FROM node_embeddings e
JOIN memories m ON e.node_id = m.id      -- ← legacy table, EMPTY under unified mode
WHERE e.model = ? AND e.node_id IN (...)
  AND m.deleted_at IS NULL ...
```

Under unified mode (default since T32, commit 887dc37), T34a removed the legacy `memories` write — the table has **0 rows**. The JOIN therefore returns zero rows for every call, and the function silently returns an empty map (GUARD-9 swallow in `StorageLoader::load_embeddings`, orchestrator.rs ~:264).

The sibling functions `get_all_embeddings` (:4987) and `get_embeddings_in_namespace` (:5040) were **already fixed** by ISS-199 to `JOIN nodes m ... WHERE m.node_kind = 'memory'` — their doc comments describe this exact failure mode. `get_embeddings_for_ids` was simply missed.

## Verified evidence (ISS-221 probe run, conv-26, 2026-06-11)

Bench DB `/var/folders/.../.tmpQHZ4vm/substrate.db`:
- `memory_embeddings`: 484 rows, model `ollama/nomic-embed-text`
- `node_embeddings`: 484 rows, same model
- `memories`: **0 rows** → `JOIN memories` yields **0** (verified by direct SQL)
- `JOIN nodes WHERE deleted_at IS NULL`: 484 rows (the correct query works)

Downstream proof: q32 Factual dump (267 rows) — ALL rows have `vector_score=Some(0.0)`, zero `None`, zero positive. Query embedding was present (`Some`), every per-memory lookup missed (`unwrap_or(0.0)`). Corpus-wide across 242 factual dumps: vector>0 on only 1400/57790 rows (~2.4%, from other adapter paths that don't use this fetch).

## Blast radius

Every consumer of `get_embeddings_for_ids` under unified mode (= all bench runs since T32, all prod defaults):

1. **Factual plan vector_score** (orchestrator.rs:512) — dead. `combine_factual_v2` (ISS-192) gives vector its own weight channel = pure noise-at-zero. Ranking degenerates to graph_score tie-tiers broken by memory_id hex order (q32: 80-row tie at 0.353, gold at rank 83 > 50-cap → ISS-221 q3/q11 Hybrid pool recall failure).
2. **MMR diversity** (ISS-139) — `load_embeddings` empty → MMR degenerates to relevance-only silently. Every MMR A/B since T32 (ISS-143 sweep included) measured a no-op.
3. Any other `StorageLoader::load_embeddings` caller.

**Interpretation caution for past benches:** all conv-26/conv-44 results since T32 ran with vector signal dead in the Factual path. Lever-2's 0.5197 was achieved WITHOUT vector scoring — fixing this is a potential standalone lift (and may re-rank conclusions about MMR λ and factual_reweight).

## Fix

Mirror the ISS-199 pattern from `get_all_embeddings`:

```sql
-- unified branch
SELECT e.node_id, e.embedding
FROM node_embeddings e
JOIN nodes m ON e.node_id = m.id
WHERE m.node_kind = 'memory'
  AND e.model = ?
  AND e.node_id IN (...)
  AND m.deleted_at IS NULL
  AND (m.superseded_by IS NULL OR m.superseded_by = '')
```

Legacy branch unchanged.

## Acceptance criteria

- [ ] AC-1: unified branch of `get_embeddings_for_ids` JOINs `nodes` (node_kind='memory'), not `memories`. Legacy branch byte-identical.
- [ ] AC-2: contract test — unified-mode store: add N memories with embeddings, `get_embeddings_for_ids` returns N rows (currently returns 0). Plus liveness: deleted/superseded rows excluded.
- [ ] AC-3: bench validation — conv-26 LEVER2 envelope (INGEST_WINDOW=4, K=10, FACTUAL_REWEIGHT=on, CE=1, CE_K_IN=250) re-run with fix; dump shows vector_score>0 on a substantial share of factual rows (expect ≫2.4%); compare overall vs 0.5197 baseline.
- [ ] AC-4: check whether q3/q11 golds now enter the Hybrid 50-pool (the ISS-221 recall failure) — vector signal may break the graph-tie tiers that buried them.
- [ ] AC-5: audit remaining `JOIN memories` sites in storage.rs for the same missed-cutover pattern (grep shows :3743/:3812/:4667/:4705 are memories_fts joins — legacy-branch only? verify each; :5902+ hebbian joins; :7959 memory_entities).

## Notes

- Discovered while root-causing ISS-221 B2 PPR-CE blend null result: blend was mechanically correct but inert because golds never entered the Hybrid pool; pool truncation was caused by degenerate Factual ranking; ranking was degenerate because vector channel was dead; vector channel was dead because of THIS bug.
- MMR re-evaluation after fix may warrant reopening ISS-143 conclusions.
