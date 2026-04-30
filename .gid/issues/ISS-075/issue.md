---
id: "ISS-075"
title: "Resolution pipeline never writes alias rows or embeddings — search_candidates returns empty, every entity becomes CreateNew"
status: open
priority: P0
labels: [resolution, dedup, root-cause, v0.3, locomo]
relates_to: [ISS-033, ISS-072, ISS-074]
---

# ISS-075: Resolution pipeline never produces alias rows or embeddings

## TL;DR

The v0.3 resolution pipeline's `search_candidates` always returns empty in production because **no production code path ever writes to `graph_entity_aliases` or populates `graph_entities.embedding`**. Every entity mention therefore short-circuits to `CreateNew`. Fusion and decision stages run but operate on an empty candidate set — they have never actually merged anything in any production substrate.

This is the structural root cause of the 27 duplicate `Caroline` rows in RUN-0007 (and 29 in RUN-0005/0006).

## Evidence

RUN-0007 substrate (`.gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db`):

- 187 entities total, **0** with non-null `embedding`
- **0** rows in `graph_entity_aliases`
- 27 rows for `canonical_name='Caroline'`, all in same namespace, all distinct UUIDs, none have alias rows, none have embeddings

Same pattern in RUN-0005-substrate and RUN-0006-substrate (29 Caroline rows each, 0 aliases, 0 embeddings).

## Root cause (code path audit)

`crates/engramai/src/resolution/`:

1. **`stage_extract.rs`** — extracts triples from LLM, creates `Entity` rows. Does not touch `embedding` field. Does not call `upsert_alias`. Confirmed by `grep -n "embedding\|upsert_alias\|alias" stage_extract.rs` returning zero matches.

2. **`stage_persist.rs`** — persists resolved entities. Does not touch `embedding`. Does not call `upsert_alias`. Same `grep` returns zero matches.

3. **`pipeline.rs::resolve_entities`** — calls `retrieve_candidates` (candidate_retrieval.rs:127) which calls `store.search_candidates(query)`. The query has `mention_embedding=None` (extractor never populated it) and `mention_text="Caroline"` (or similar).

4. **`SqliteGraphStore::search_candidates`** (graph/store.rs) requires either:
   - a row in `graph_entity_aliases` matching the normalized mention (ISS-033 NFKC normalize), **or**
   - a non-null embedding for cosine search.
   
   Both conditions fail unconditionally → returns `[]`.

5. **`pipeline.rs:951` comment confirms this is a known production behavior**:
   > `// search_candidates → empty → all entities CreateNew`
   
   That comment is in test scaffolding (`StubStore`), but it accurately describes what production does too.

6. **Only `upsert_alias` call site outside `#[cfg(test)]` is in `retrieval/adapters/graph_entity_resolver.rs:188`** — and that file is also gated by `#[cfg(test)]` (line 165). Production code has zero calls to `upsert_alias`.

## Why fusion / decision stages don't help

Fusion (`fusion.rs`) computes weighted scores over `Vec<ScoredCandidate>`. When that vec is empty (CreateNew short-circuit), fusion is not called at all. Decision (`decision.rs`) likewise sees an empty pool.

So fusion thresholds, decision thresholds, embedding cosine cutoffs — **none of it is reachable from production**. Every entity creates a new row.

## Test coverage gap

`graph_entity_resolver.rs:188` — the test helper that does call `upsert_alias` — has a comment that explicitly says:

> `search_candidates does not match by canonical_name alone — it requires a row in graph_entity_aliases. Mirror the production path by upserting a self-alias.`

The author of the test knew production was supposed to do this. Tests mocked it. Production never implemented it. Classic test-passes-production-broken pattern.

## Secondary issue (won't fix here, but flag)

`Entity::new` defaults `identity_confidence = 0.0` (entity.rs:166). Even if alias rows were written, `search_candidates` may filter / down-rank low-confidence anchors. The test helper at line 178 explicitly bumps to 1.0 with the comment "search_candidates path treats it as a high-confidence anchor." Production extraction also never sets identity_confidence > 0.0. Tracked for follow-up after dedup is fixed.

## Impact

- **Dedup completely non-functional** in v0.3. Every mention of "Caroline" / "Joanna" / etc. becomes a fresh entity row. LoCoMo benchmarks see entity-cardinality bloat that hides retrieval quality regressions and confounds spreading-activation experiments.
- **Spreading activation can't traverse**: 27 Caroline rows × 0 outgoing edges per row (separately tracked as ISS-074) means even queries that find Caroline-as-anchor have nowhere to spread.
- **Hebbian links between entities are wrong**: links are made between mention-instance entities, not the canonical person.

## Acceptance criteria

A fix lands when, on a fresh ingest of LoCoMo conv-26:

- [AC-1] `SELECT COUNT(*) FROM graph_entity_aliases WHERE namespace='locomo-conv26-iss075'` > 0 (some alias rows exist)
- [AC-2] `SELECT COUNT(*) FROM graph_entities WHERE canonical_name='Caroline'` ≤ 2 (1 ideal; ≤2 if first-mention race is acceptable). Current: 27.
- [AC-3] Pipeline trace logs confirm `Decision::MergeInto` outcomes, not just `Decision::CreateNew`. Current: no MergeInto in any production substrate.
- [AC-4] Existing tests still pass (stage_extract / stage_persist / fusion / decision unit tests).

## Out of scope (separate issues)

- ISS-074: Person entities have 0 outgoing edges (independent — even after dedup, edges still missing).
- Schema-level `identity_confidence` defaults / extraction-time confidence assignment.
- Embedding model choice / dimension / backfill of historical entities.

## Suggested approach (sketch only — implementer chooses)

Two minimum changes to make pipeline functional:

1. **`stage_persist.rs`** — when persisting a `CreateNew` decision, immediately call `store.upsert_alias(canonical_name.to_lowercase(), canonical_name, entity_id, None)`. This makes the entity discoverable on the *next* mention.
2. **`stage_extract.rs`** or **a new stage between extract and resolve** — populate `mention.embedding` from the same model used for memory embeddings, so `search_candidates` has a cosine path even before any aliases exist.

Either alone would unblock dedup; both together is the proper fix.

## Verification commands

```bash
# Confirm root-cause data state on any current substrate:
sqlite3 .gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db \
  "SELECT (SELECT COUNT(*) FROM graph_entity_aliases) AS aliases,
          (SELECT COUNT(*) FROM graph_entities WHERE embedding IS NOT NULL) AS with_embed,
          (SELECT COUNT(*) FROM graph_entities WHERE canonical_name='Caroline') AS caroline_rows;"
# Expected (current): 0 | 0 | 27
# Expected (fixed):   >0 | >0 | 1 or 2
```

```bash
# Confirm production has no upsert_alias calls outside tests:
grep -rn "upsert_alias" crates/engramai/src --include="*.rs" | grep -v "#\[cfg(test)\]" | grep -v "test_helpers" | grep -v "/tests/"
# Expected (current): only definitions in graph/store.rs, no call sites
```
