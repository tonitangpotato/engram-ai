---
id: ISS-115
title: "Phase B dual-DELETE gap — hard_delete_cascade only deletes legacy tables, not unified nodes/edges"
status: open
priority: P1
severity: degradation
labels: [v04-unified-substrate, phase-b, dual-write, root-fix-pending]
created: 2026-05-13
---

# Phase B dual-DELETE gap

## Summary

Phase B (T12–T18) introduced **dual-write writers** that mirror legacy
table writes into the unified `nodes` / `edges` / `node_embeddings`
tables, so that Phase D's read-switch can serve traffic from the
unified substrate. But the **delete path was never dualized**: every
DELETE goes only to legacy tables. As Phase D reads progressively
switch on (`unified_substrate=true`), this gap will surface as stale
unified-substrate rows that legacy reads correctly ignore.

## Where it lives

`Storage::hard_delete_cascade` (`crates/engramai/src/storage.rs:3257`)
calls:

- `DELETE FROM memory_embeddings WHERE memory_id = ?` — no companion
  `DELETE FROM node_embeddings WHERE node_id = ?`
- `DELETE FROM hebbian_links WHERE source_id = ?1 OR target_id = ?1`
  — no companion `DELETE FROM edges WHERE … edge_kind='associative'`
- `DELETE FROM memory_entities WHERE memory_id = ?1` — no companion
  edges delete (provenance/mentions + structural subject_of/object_of)
- `DELETE FROM synthesis_provenance WHERE source_id = ?1 OR insight_id = ?1`
  — no companion edges delete (provenance/derived_from)
- `DELETE FROM memories WHERE id = ?1` — no companion
  `DELETE FROM nodes WHERE id = ?` (node row + cascades survive)

`Storage::delete_all_embeddings` (`storage.rs:3330`) has the same
asymmetry for the embeddings path.

`Storage::add` followed by `soft_delete` is consistent on the read
path (the liveness predicate `m.deleted_at IS NULL AND superseded_by
IS NULL` is JOINed identically by both legacy and unified readers per
T29.3) — only **hard** deletes leak.

## Why it didn't blow up earlier

Phase B + Phase C only require that **writes** stay in sync (T17 row-
count parity verifier covers post-backfill steady state). The hard-
delete path was implicitly assumed to be Phase E's problem (legacy
retirement). T29.3 (Phase D embeddings read-switch, commit landing
today) is the first read-switch where unified-substrate divergence
becomes observable in production traffic.

## Symptoms when `unified_substrate=true`

- `hard_delete_cascade(m)` removes `m` from `memories`/`memory_embeddings`,
  so legacy reads correctly omit it. But the `nodes` row and its
  `node_embeddings` row stay alive. Unified reads of `get_embedding`,
  `get_all_embeddings`, `embedding_stats` will surface a phantom row.
- The `JOIN memories m ON e.node_id = m.id` predicates in
  `get_all_embeddings` / `get_embeddings_in_namespace` happen to mask
  the embedding phantom because the parent memory row is gone — so
  T29.3 readers degrade to "stale row hidden behind missing JOIN
  match" rather than visible regression. But `get_embedding` and
  `get_embedding_for_memory` query `node_embeddings` directly without
  the JOIN — those will return the phantom blob.
- T18+ Hebbian / entity / provenance readers (future T29.4-T29.8)
  will see analogous phantoms once they switch.

## Fix (Phase B closure)

Add dual-DELETE counterparts inside `hard_delete_cascade` and
`delete_all_embeddings`. Wrap the whole `hard_delete_cascade` in a
transaction (currently per-statement autocommit — already a latent
issue, partial-failure can leave half-deleted state).

Specifically, mirror each legacy DELETE with its unified counterpart:

- `memory_embeddings` → `node_embeddings WHERE node_id = ?`
- `hebbian_links` → `edges WHERE edge_kind='associative' AND
   ((source_id=? AND target_id IS NOT NULL) OR (target_id=? AND
   source_id IS NOT NULL))` (canonical-pair direction packed; matches
   T14/T24 writer)
- `memory_entities` → `edges WHERE source_id=? AND edge_kind IN
   ('provenance','structural') AND predicate IN ('mentions',
   'subject_of','object_of')` (matches T23 writer's three role splits)
- `synthesis_provenance` → `edges WHERE edge_kind='provenance' AND
   predicate='derived_from' AND (source_id=? OR target_id=?)`
   (matches T16/T25 writer)
- `memories` → `nodes WHERE id=?` (cascades to `node_embeddings`,
   `edges` via FK ON DELETE CASCADE — but we delete `edges` rows
   explicitly above for clarity and to keep the dual-DELETE matching
   the dual-WRITE one-for-one)

Tests: parity test per delete kind — pre-delete state visible to
both legacy and unified readers, post-delete invisible to both.

## Why this is **not** done in T29.3 today

T29.3 is a read-switch task. Adding 5 dual-DELETE branches across
two methods, three new dual-DELETE helpers (mirroring T14/T16/T23/T25
writers), and a transaction-wrapping refactor of
`hard_delete_cascade` is a separate concern. T29.3 documents the
asymmetry in `store_embedding`'s helper comment and pins this issue.

Suggested ordering: land ISS-115 before T29.4 (Hebbian read-switch)
to keep each read-switch's blast radius bounded. If the LoCoMo /
manual probe campaign (T30/T31) flips on `unified_substrate=true`
ahead of ISS-115 closing, we're flying with phantom-deletion risk
for embeddings — currently low because hard-delete is rare in prod,
but tracked here.

## References

- design.md §3.3 (closed edge-kind taxonomy — drives which legacy
  table maps to which `(edge_kind, predicate)` tuple for DELETE)
- design.md §5.3 T13–T16 (Phase B dual-WRITE writers — the symmetric
  ones)
- design.md §5.4 T29 (Phase D read-switch — discovers the gap)
- Commit `ac1c9f0` + T29.3 commit (this surfaces the issue)
