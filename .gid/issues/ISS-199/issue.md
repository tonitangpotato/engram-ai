---
title: 'T34a read-path cutover: route soft_delete/get_deleted_at/find_entity_overlap/consolidation/append_merge_provenance off `memories` to `nodes`, then re-attempt T34a'
status: resolved
priority: P0
severity: blocker
labels:
- v04-unified-substrate
- phase-e
- read-cutover
- t34a
feature: v04-unified-substrate
created: 2026-05-31
relates_to:
- ISS-196
- ISS-197
- ISS-198
blocks:
- ISS-197
depends_on:
- ISS-198
fixed_by:
- 22333ad
- e6fd8a3
---

# Summary

T34a (`delete the `INSERT INTO memories` write in `Storage::add` under unified
mode) cannot land standalone. ISS-198 closed the FK-on-write hazard (all child
tables that are *written* during `add`/enrichment now reference `nodes`), and
the suite is green at **2084/0** with those re-points in place — **but only
because T34a itself was reverted out of the tree** (OPT2).

When T34a *is* applied (the `if !unified` gate around the `memories`/
`memories_fts` writes), **5 `lifecycle::tests` fail** — not with
`FOREIGN KEY constraint failed`, but with `QueryReturnedNoRows` /
`assert!(result.is_some())`. They fail because they **read / UPDATE / RMW the
`memories` table**, which T34a now leaves empty. T34a removes a *write* that
these paths still implicitly depend on.

# The 5 coupled paths (must be cut over to `nodes` first)

1. **`soft_delete` + `get_deleted_at`** (`test_forget_targeted_soft`)
   - `soft_delete` (storage.rs:4620) `UPDATE memories SET deleted_at ...` → 0
     rows when the row was never inserted. It already dual-writes
     `nodes.deleted_at`, so the data is in `nodes`.
   - `get_deleted_at` (storage.rs:4785) `SELECT deleted_at FROM memories` →
     `QueryReturnedNoRows`. **Deliberately pinned to `memories` until T39**
     because of the `deleted_at` type mismatch: `memories.deleted_at` is TEXT
     (RFC3339) but `nodes.deleted_at` is REAL (epoch f64). Cutting it over
     forces the ISS-197 §8.6 reconciliation (return-type change or
     epoch→RFC3339 conversion) earlier than planned.

2. **`find_entity_overlap`** (`test_find_entity_overlap`, storage.rs:7378)
   - `JOIN memories m` for the `namespace` + `deleted_at IS NULL` filter →
     empty `memories` → returns `None`. Cut the JOIN over to `nodes`
     (`node_kind='memory'`, namespace via attributes, `deleted_at` REAL).

3. **`run_consolidation_cycle`** (`test_enhanced_sleep_cycle_phases`,
   `test_iss103_occurred_at_does_not_trigger_decay_on_historical_ingest`,
   lifecycle.rs) — iterates `memories` rows for decay/forget; a `query_row`
   returns no rows on empty `memories` ("Consolidation failed, attempting FTS
   rebuild: Query returned no rows"). Iterate `nodes WHERE node_kind='memory'`.

4. **`append_merge_provenance`** (`test_append_merge_provenance`,
   storage.rs:7460) — RMW path that reads `memories.metadata`. Tracked as a
   Phase B dual-write candidate in ISS-197 §8.1. Cut the RMW over to
   `nodes.attributes`.

# Why this is its own issue (not folded into ISS-198)

- ISS-198 is mechanical FK re-pointing (`rebuild_table_fk_to_nodes`), separable
  and green. This is **substantive read-path work** — each path needs its own
  contract test, and #1 drags a TEXT/REAL type reconciliation (§8.6) along.
- Folding a type-format change into a "delete a write" commit muddies both and
  violates the one-read-path-at-a-time §5.4 discipline.

# Plan

Cut each path over to `nodes` one at a time, each with a contract test proving
unified-mode parity, in this order (cheapest → forced-reconciliation last):

1. `find_entity_overlap` JOIN → `nodes`
2. `run_consolidation_cycle` iteration → `nodes WHERE node_kind='memory'`
3. `append_merge_provenance` RMW → `nodes.attributes`
4. `soft_delete` UPDATE → `nodes` only + `get_deleted_at` read → `nodes`
   (resolve §8.6 TEXT/REAL `deleted_at`: change return type or convert)
5. Re-apply T34a (`if !unified` gate around `memories`/`memories_fts` writes in
   `Storage::add`) → full lib suite green (≥2084, 0 failed) = ISS-197 AC-3
   unblock.

# Acceptance

- [x] AC-1: all 5 read/UPDATE/RMW paths cut over to `nodes` with per-path
      contract tests (unified-mode parity vs the legacy-mode arm). Paths:
      soft_delete/get_deleted_at, find_entity_overlap (committed e6fd8a3),
      consolidation update (get_unenriched_memory_ids + increment_extraction_
      attempts), append_merge_provenance, plus the update_content_inner /
      update_inner / get_embeddings_in_namespace / get_all_embeddings readers
      surfaced once T34a was applied. Contract tests: iss199_* (5 files, 17
      tests). [22333ad, e6fd8a3]
- [x] AC-2: `deleted_at` TEXT/REAL reconciliation resolved (§8.6).
      `get_deleted_at` under unified mode reads `nodes.deleted_at` (REAL epoch)
      and converts epoch→RFC3339 via `f64_to_datetime(...).to_rfc3339()`,
      preserving the `Option<String>` contract losslessly. [22333ad]
- [x] AC-3: T34a confirmed applied (storage.rs L2317 `if !self.unified_substrate`
      gate on the `memories`/`memories_fts` writes). FULL suite green with T34a
      live: 2084 lib + 2694 integration = 4778 tests, 0 failed, 0 panics. The
      gate exposed additional readers beyond the original 4 (embedding-dedup
      JOINs, update_content_inner), all cut over in the same pass.
- [x] AC-4: ISS-197 §8.7 + PHASE-E-PLAN.md updated; ISS-197 AC-3 unblocked.

# Scope note (resolved larger than originally framed)

The "out of scope" 3 `graph_*` tables (`graph_edges` /
`graph_memory_entity_mentions` / `graph_pipeline_runs`) WERE cut over in this
issue after all: their `memory_id` FK was re-pointed `memories(id)`→`nodes(id)`
because they are written inside the T34a→T39 window when the row exists only in
`nodes`, and the FK-787 broke the *integration* suite (the original scope was
lib-only). `migrate_graph_tables_fk_to_nodes` migrates existing DBs;
`GRAPH_DDL` was updated for fresh DBs.

# Out of scope (original framing — superseded above)

- The 3 `graph_*` tables (`graph_edges` / `graph_memory_entity_mentions` /
  `graph_pipeline_runs`) — they are bootstrapped in
  `src/graph/storage_graph.rs`, not written in the lib-test window, and their
  FK re-point (if a prod FK fires) is tracked separately under ISS-198's
  graph-init call-site note.
- T39 table DROP — a separate phase after this cutover.
