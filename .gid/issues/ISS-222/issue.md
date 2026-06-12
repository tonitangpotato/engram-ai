---
title: get_embeddings_for_ids unified branch JOINs empty legacy memories table — vector signal dead in Factual/MMR since T32
status: resolved
fixed_by: [2cc72375]
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

- [x] AC-1: unified branch of `get_embeddings_for_ids` JOINs `nodes` (node_kind='memory'), not `memories`. Legacy branch byte-identical. — **PASS** (commit `2cc72375`, storage.rs ~:4937: `JOIN nodes m ON e.node_id = m.id WHERE m.node_kind = 'memory' AND m.deleted_at IS NULL AND (m.superseded_by IS NULL OR m.superseded_by = '')`; legacy branch untouched).
- [x] AC-2: contract test — **PASS**. `iss222_get_embeddings_for_ids_unified_works_with_empty_legacy_memories` covers the exact T34a failure mode (rows in nodes + node_embeddings, legacy `memories` empty): N rows returned, deleted_at exclusion, superseded_by exclusion. 2169/2169 lib tests green.
- [x] AC-3: bench validation — **PASS, fix confirmed live e2e**. Run `ISS222-LEVER2-conv26-20260612T002433Z` (LEVER2 envelope, no PPR blend, 152q, exit=0). Dump probe: Factual rows **57292/57292 vector_score nonzero** (was 0/all before fix), abstract 400/400. Overall **0.5329 vs 0.5197 baseline (+1.3pp)** — within ±2pp re-ingest noise but directionally consistent: single-hop 0.469 (+3.1pp), open-domain 0.615 (**+15.4pp**, biggest beneficiary), multi-hop 0.351 (+2.7pp), temporal 0.643 (−2.9pp, noise-range). Modest overall lift explained: CE k_in=250 already rescued most Factual queries in baseline; real gains are in Hybrid pool construction + open-domain.
- [x] AC-4: q3/q11 Hybrid pool recall — **PASS (recall restored; q11 remains a ranking problem)**. q3 flipped 0→1: gold 'adoption' memories now in pool at ranks 18/20/23/33. q11 gold ('moved from home country') **entered the pool at rank 41** (was absent entirely) but still scores 0.0 — outside top-10 = the old ISS-201 A-bucket ranked-out class, not a recall failure anymore. Note: Hybrid dump rows have all-None subscores by design (Hybrid routes via RRF, bypasses fusion) — that is expected, not a fix failure.
- [x] AC-5: audit remaining `JOIN memories` sites — **PASS, no other missed cutover**. Verified each (2026-06-11): :3743/:3812/:4667/:4705 memories_fts joins = legacy-branch only (unified branch uses nodes_fts); :5020/:5077 = get_all_embeddings / get_embeddings_in_namespace legacy branches (unified branches already fixed by ISS-199); :5911/:6006/:6090 hebbian cross-link readers = legacy branches of T29.4 read-switch (unified branches JOIN nodes); :7968 memory_entities = legacy branch of ISS-199 cutover. `get_embeddings_for_ids` was the only unified branch reading legacy `memories`.

## Verdict (2026-06-11)

**Resolved.** Root fix shipped in `2cc72375` (+62/−4, 1 file): unified branch mirrors the ISS-199 read-cutover pattern from its sibling functions. The vector channel in the Factual plan is live again for the first time since T32 — confirmed end-to-end by the 57292/57292 nonzero vector_score dump probe.

**Follow-ups (recorded, not blocking close):**
1. **MMR λ re-sweep needed** — every MMR A/B since T32 (including the ISS-143 sweep) measured a dead channel; MMR was relevance-only. This run kept λ=1.0 (MMR inert by config), so MMR-with-live-vectors is still unmeasured. ISS-143 conclusions are invalidated and need re-running.
2. **q11-class 'gold in pool but rank>10'** — ranking lever, the ISS-201 A-bucket problem (RRF tie-breaking / Hybrid scoring / cap). Recall is no longer the blocker.
3. **PPR re-test (ISS-221 AC-6 deferral)** — the ISS-221 B2 null result was confounded by this bug (golds never entered the Hybrid pool). With vector signal live and pool recall restored, the PPR-CE blend arm can be re-run if the q11-class ranking work wants a graph-signal lever.
4. **Possible audit**: associative adapter vector_score path (orchestrator.rs ~:737/:811) uses a different loader path — quick check that it doesn't share the pattern (AC-5 grep says no other unified-branch `JOIN memories` exists, so likely clean).

## Notes

- Discovered while root-causing ISS-221 B2 PPR-CE blend null result: blend was mechanically correct but inert because golds never entered the Hybrid pool; pool truncation was caused by degenerate Factual ranking; ranking was degenerate because vector channel was dead; vector channel was dead because of THIS bug.
- MMR re-evaluation after fix may warrant reopening ISS-143 conclusions.
