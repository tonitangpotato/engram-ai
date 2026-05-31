---
relates_to: .gid/issues/ISS-196/issue.md
fixed_by: 1b0d703
status: resolved
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

---

# Batch 2 findings (2026-05-31, runtime-probed)

## What landed (FK re-points, in scope)

A runtime `sqlite_master` probe (entity_config OFF + dedup OFF, inside
`Storage::add`) found **5** tables still `REFERENCES memories(id)` at runtime:
`memory_embeddings`, `triples`, `graph_edges`,
`graph_memory_entity_mentions`, `graph_pipeline_runs`.

ROOT CAUSE of the original 23 T34a FK-on-write failures was confirmed to be
**`store_embedding()` dual-writing `memory_embeddings` (FK→memories) on every
`add` when an embedding exists** (production default when Ollama is reachable),
NOT the access_log path (already →nodes via ISS-196).

Two re-points were added in `Storage::new` (storage.rs ~L575, right after the
existing 3 ISS-198 calls — placed there because `migrate_embeddings`/
`migrate_triples` create those tables before `migrate_unified_nodes`, so both
child + `nodes` parent exist):

- `migrate_memory_embeddings_fk_to_nodes` (fk_col `memory_id`, idx on `model`)
- `migrate_triples_fk_to_nodes` (fk_col `memory_id`, 3 `idx_triples_*`)

`memory_embeddings_v2` needs NO re-point — it is migration-scratch
(CREATE→populate→RENAME), never persists on a fresh DB, not in the runtime scan.

The 3 `graph_*` tables are bootstrapped in `src/graph/storage_graph.rs`
(`init_graph_tables`→`migrate_v04_substrate`), NOT in `Storage::new`. `test_memory()`
wires no graph_store/job_queue, so they are not written in the lib-test
`add`→enrichment window and did NOT fire. Production resolution pipeline does
write them — if a prod-path FK fires later, a `migrate_*_fk_to_nodes` must be
wired at the graph-init call site (deferred; out of scope for the lib-suite
unblock).

## VERDICT: T34a CANNOT land standalone — 5 residual failures are read-path coupling, not FK

After the 2 re-points + T34a (`if !unified` gate) applied:
`cargo test -p engramai --lib` → **2076 passed, 5 failed** (down from 23).
Build clean. **All 5 remaining failures are `lifecycle::tests` and are NOT
`FOREIGN KEY constraint failed`** — they are `QueryReturnedNoRows` /
`assert!(result.is_some())` failures caused by methods that **read / UPDATE /
RMW the `memories` table** which T34a now leaves empty:

- `test_forget_targeted_soft` (L157) → `soft_delete` UPDATEs
  `memories.deleted_at` (0 rows, row never inserted), then `get_deleted_at`
  `SELECT deleted_at FROM memories` → `QueryReturnedNoRows`. **`get_deleted_at`
  is documented (storage.rs:4785) to deliberately stay on `memories` until T39
  because of the TEXT(RFC3339)/REAL(epoch) `deleted_at` type mismatch** — the
  exact ISS-197 §8.6 reconciliation that is a T39 prereq.
- `test_find_entity_overlap` (L208) → `find_entity_overlap` JOINs
  `memory_entities … JOIN memories m` for namespace + `deleted_at IS NULL`
  filter → empty `memories` → `None` → `assert!(result.is_some())` fails.
- `test_append_merge_provenance` (L311) → `append_merge_provenance` is an
  **RMW path** that reads `memories` (already flagged ISS-197 §8.1 out-of-scope);
  test then `SELECT metadata FROM memories` directly.
- `test_enhanced_sleep_cycle_phases` (L536) + `test_iss103_…historical_ingest`
  (L480) → `sleep_cycle`→`run_consolidation_cycle` iterates `memories` rows
  (decay/forget); a `query_row` returns no rows on empty `memories`
  ("Consolidation failed, attempting FTS rebuild: Query returned no rows").

**These are exactly the "residual `FROM memories` reads" my working-memory and
ISS-197 §8.1/§8.6 already enumerated as OUT OF SCOPE for the mechanical
read-cutover.** T34a's `if !unified` gate is correct in isolation, but it
removes a write that these read/UPDATE/RMW paths still implicitly depend on.
Per the "不确定先停记 issue, 不硬干" rule I did **NOT** force a fix.

## Two clean ways forward (potato to choose — NOT auto-applied)

1. **Cut the 4 read/UPDATE/RMW paths over to `nodes` first, then land T34a.**
   - `soft_delete`: UPDATE `nodes.deleted_at` only (already dual-writes it);
     `get_deleted_at`: read `nodes.deleted_at` (REAL) → return type / epoch→RFC
     reconciliation (ISS-197 §8.6 — the T39 prereq, now forced earlier).
   - `find_entity_overlap`: JOIN `nodes` instead of `memories` for namespace +
     `deleted_at` filter.
   - `run_consolidation_cycle`: iterate `nodes WHERE node_kind='memory'`.
   - `append_merge_provenance`: RMW against `nodes.attributes` (Phase B
     dual-write candidate, ISS-197 §8.1).
   This is substantive read-path work, not mechanical — each needs its own
   contract test. Effectively pulls ISS-197 §8.1+§8.6 forward as a T34a prereq.
   **→ Now tracked as ISS-199.**

2. **Keep T34a deferred; land ONLY the batch-2 FK re-points now (green at
   2081/0 without T34a, per /tmp/iss198_full.log).** Then sequence the
   read-path cutover (option 1) as its own issue before re-attempting T34a.
   This keeps the suite green and the commits atomic. **Recommended** — it
   matches the one-read-path-at-a-time §5.4 discipline and avoids folding a
   type-reconciliation (§8.6) into a "delete a write" change.

## AC status after batch 2

- [x] AC-1 (extended): `memory_embeddings` + `triples` re-pointed (they ARE
      written under unified mode before T39 — `store_embedding` + triple
      extraction); `graph_*` flagged for graph-init call site if a prod FK fires.
- [x] AC-2: idempotency + nodes-only-parent tests for the 2 new migrations
      shipped — `iss198_memory_embeddings_insert_succeeds_without_legacy_memories_row`,
      `iss198_triples_insert_succeeds_without_legacy_memories_row`,
      `iss198_batch2_fk_repoint_is_idempotent` (storage.rs ~L11490). All 9
      `iss198_*` tests green.
- [ ] AC-4: **NOT met with T34a applied** — 5 read-path failures remain (above).
      Met at **2081/0 WITHOUT T34a** (batch-2 FK re-points alone). **DECISION:
      OPT2 taken — T34a reverted out of this issue's working tree; only the
      batch-2 FK re-points (+ their 3 tests) land here. T34a re-attempt is
      deferred to a new issue after the read-path cutover (OPT1 work).**
