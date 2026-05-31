---
relates_to: .gid/issues/ISS-196/issue.md
---
# .gid/issues/ISS-198/issue.md (issue)
project: engram
---
title: "T34a-pre: re-point write-active retained child-table FKs (hebbian_links/memory_entities/synthesis_provenance) from memories to nodes"
status: open
priority: P0
severity: blocker
labels: [v04-unified-substrate, phase-e, phase-ordering, fk, schema]
feature: v04-unified-substrate
created: 2026-05-31
relates_to: [ISS-196, ISS-197]
blocks: [ISS-197]
---

# Summary

**T34a (delete `INSERT INTO memories` in `Storage::add`) is blocked by three
child-table FKs that still `REFERENCES memories(id)` and are written during
`add`/enrichment under the unified substrate.** When the `memories` write is
removed, those child inserts have no FK parent (`memories` is empty) and fail
with `FOREIGN KEY constraint failed` — *before* the table is ever dropped.

This is the exact `access_log` hazard ISS-196 fixed, but for three more tables
ISS-196 deliberately excluded.

# Why this is a NEW issue and not a re-open of ISS-196

ISS-196 is **resolved and correct for what it claimed**. Its AC-2 made an
explicit scoping decision:

> AC-2: ... of the 7 tables with `REFERENCES memories(id)`, only `access_log`
> is retained; `hebbian_links`/`memory_entities`/`synthesis_provenance`/
> `memory_embeddings`/`memory_embeddings_v2`/`triples` are all in the §5.6.1
> drop set, so their FKs vanish with the tables.

That reasoning is sound **for the DROP (T39)**: a table on the drop list takes
its FK with it, so no re-point is needed *to survive the drop*. What AC-2
**missed** is the **write-time** hazard: those three tables are still
**written during `add`/enrichment under unified mode in the window between
T34a (memories-write deletion) and T39 (table drop)**. In that window:

- `memories` is empty (T34a removed the write), but
- `hebbian_links` / `memory_entities` / `synthesis_provenance` still exist with
  their FK into `memories`, and
- `record_coactivation*` / entity enrichment / synthesis still INSERT into them.

So the FK fires at **write time**, long before the drop. ISS-196 reasoned about
the drop edge (correct) and never reasoned about this write edge (the gap).
Re-opening a resolved P0 to bolt on contradicting work would corrupt its
resolution record; this issue corrects the AC-2 assumption explicitly instead.

# Evidence (empirical, 2026-05-31)

T34a re-attempted with correct narrow scope (gate `INSERT INTO memories` +
`memories_fts` in `Storage::add` behind `if !unified`, keep
`insert_memory_node_row`, leave RMW UPDATE paths + `soft_delete` writing
`memories`). Result:

```
cargo test -p engramai --lib
test result: FAILED. 2052 passed; 23 failed; 4 ignored
```

All 23 failures are uniform `FOREIGN KEY constraint failed`. Representative:

```
lifecycle::tests::test_list_namespaces  panicked … lifecycle.rs:557
  Result::unwrap() on Err: "storage error: FOREIGN KEY constraint failed"
```

Failure clusters and the FK each one trips:

- `lifecycle::tests` (hebbian/sleep/forget/health) + `memory::confidence_tests::
  test_broadcast_*` → **`hebbian_links`** (`source_id`/`target_id`
  `REFERENCES memories(id)`, storage.rs L1087-1088). Written by
  `record_coactivation*` during `add` co-activation. These tests don't touch
  entities, so hebbian is the one firing.
- entity-touching tests (`test_find_entity_overlap`, health/cluster) →
  **`memory_entities`** (`memory_id REFERENCES memories(id)`, L1139). Written
  during entity enrichment.
- synthesis-insight tests → **`synthesis_provenance`** (`insight_id`/`source_id`
  `REFERENCES memories(id)`, L1168-1169). Written when synthesis insights land.

`access_log` does NOT appear — ISS-196 already re-pointed it (L1185+
`migrate_access_log_fk_to_nodes`).

Note: these tables *do* dual-write to unified `edges`/`nodes` (T14/T16), but
with `PRAGMA foreign_keys=ON` (storage.rs:447) the legacy child FK check fires
first, before the unified write matters.

# Root cause

Same as ISS-196's: a table designed as a child of legacy `memories`. In the
unified substrate a memory is `nodes(node_kind='memory')`, so these child
tables should reference `nodes(id)`. ISS-196 re-pointed only `access_log`
because its AC-2 audit classified the other three as "drops, no re-point
needed" — true for the drop, false for the pre-drop write window.

# Fix (root, not patch) — mirror ISS-196 exactly

Add a Phase E precursor sub-task **T34a-pre / write-active FK re-point**,
landing **before** T34a's legacy-write deletion:

1. `migrate_hebbian_links_fk_to_nodes` — table-rebuild re-pointing
   `source_id` + `target_id` from `memories(id)` to `nodes(id) ON DELETE
   CASCADE`. Copy only rows whose endpoints `IN (SELECT id FROM nodes)` to stay
   FK-valid. Recreate the table's indices.
2. `migrate_memory_entities_fk_to_nodes` — re-point `memory_id` →
   `nodes(id) ON DELETE CASCADE`. (Leave `entity_id REFERENCES entities(id)` —
   `entities` is not on the drop set.)
3. `migrate_synthesis_provenance_fk_to_nodes` — re-point `insight_id` +
   `source_id` → `nodes(id)`.
4. Each migration **idempotent** via stored-DDL inspection (no-op once
   `REFERENCES memories` is gone), wired into `with_unified_substrate` AFTER
   `migrate_unified_nodes` (so `nodes` exists as the new parent), alongside
   `migrate_access_log_fk_to_nodes`.

This follows `migrate_access_log_fk_to_nodes` (storage.rs L1185+) verbatim —
the only differences are the table name, the FK column(s), and the
endpoint-validity SELECT filter.

# Acceptance criteria

- [ ] AC-1: `hebbian_links`, `memory_entities`, `synthesis_provenance` schemas
      re-pointed to `REFERENCES nodes(id) ON DELETE CASCADE` via idempotent
      table-rebuild migrations (DDL-inspection guard; no-op once `REFERENCES
      memories` is gone).
- [ ] AC-2: re-audit ALL `REFERENCES memories(id)` sites in storage.rs and
      classify each as (a) re-pointed here, (b) drops with table at T39 AND is
      never written under unified before the drop, or (c) RMW-write-coupled and
      tracked elsewhere (ISS-197 §8.1: dedup L6276, append_merge_provenance
      L7216, soft_delete). No write-active retained child FK left targeting
      `memories`.
- [ ] AC-3: regression tests pin behaviour — for each re-pointed table, an
      `iss198_<table>_insert_succeeds_without_legacy_memories_row` test
      (nodes-only parent, no `memories` row → insert succeeds) + an idempotency
      test.
- [ ] AC-4: with these migrations in place, re-run T34a (delete `INSERT INTO
      memories` in `Storage::add`) → `cargo test -p engramai --lib` green
      (≥2075, 0 failed). This is the unblock proof.
- [ ] AC-5: PHASE-E-PLAN.md §8.7 + design §5.5.3 updated — note that ISS-196
      AC-2's "drops ⇒ no re-point" reasoning held only for the drop edge, not
      the pre-drop write window; T34a-pre closes that gap.

# Blocks

- ISS-197 AC-3 (T34a write-deletion) — blocked until AC-4 here passes.

# Out of scope

- `memory_embeddings` / `memory_embeddings_v2` / `triples` — verify under AC-2
  whether they are written under unified mode before T39. If NOT written in the
  T34a→T39 window, they stay (drops with the table, ISS-196 AC-2 logic holds).
  If they ARE written, fold them into AC-1.
- The RMW-write-coupled paths (dedup L6276, append_merge_provenance L7216,
  soft_delete) still WRITE `memories` — they are NOT removed by T34a, so their
  FK parents still exist. Tracked in ISS-197 §8.1; not this issue.
- `deleted_at` type mismatch (ISS-197 §8.6) and hebbian-dedup migration-ordering
  (§8.5) — T39 prerequisites, independent of this FK re-point.
