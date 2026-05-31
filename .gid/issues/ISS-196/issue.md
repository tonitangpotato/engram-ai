---
title: Phase E/F deletion plan breaks retained-table FKs into legacy `memories` (access_log et al.)
status: resolved
priority: P0
severity: blocker
tags:
- v04-unified-substrate
- phase-e
- phase-f
- fk
- schema
feature: v04-unified-substrate
---

# Phase E/F deletion plan breaks retained-table FKs into legacy `memories`

## Summary

The Phase E "pure deletion" pattern (§5.5.2) and the Phase F drop set
(§5.6.1) are **not executable as written** because the retained table
`access_log` holds a `FOREIGN KEY ... REFERENCES memories(id)` and
`PRAGMA foreign_keys=ON` is set on the prod connection
(`storage.rs:447`).

Two distinct failures fall out of this:

### Failure 1 — T34a (`Storage::add`) breaks immediately on legacy-write deletion

`Storage::add` (storage.rs:1834) currently does, in ONE transaction:

1. `INSERT INTO memories ...`            (legacy — T34 says delete)
2. `INSERT INTO access_log (memory_id ...)`  (RETAINED — stays)
3. FTS roundtrip on `memories`            (legacy — T34 says delete)
4. `Self::insert_memory_node_row(...)`     (Phase-B unified — survivor)

`access_log.memory_id` is `TEXT NOT NULL REFERENCES memories(id) ON
DELETE CASCADE` (storage.rs:1076). The moment T34 deletes step 1, the
step-2 `access_log` insert references a parent row that no longer
exists in `memories` → **`FOREIGN KEY constraint failed`** at runtime,
on every single `add()`.

The §5.5.2 worked example (delete the legacy write, keep the unified
write) silently assumes the legacy `memories` row is not the FK parent
of any sibling insert in the same tx. It is.

### Failure 2 — T39 `DROP TABLE memories` collides with retained `access_log` FK

§3.5 / design L1151 explicitly **retains** `access_log` ("audit/
observability table"). §5.6.1 explicitly **drops** `memories`. With
`foreign_keys=ON`, `DROP TABLE memories` while a retained child table
still declares `REFERENCES memories(id)` leaves a dangling FK
definition; any subsequent `INSERT INTO access_log` then fails the
constraint check against a missing parent. The drop-sequence in §5.6.2
orders *legacy-to-legacy* FKs but never addresses the
**retained→legacy** edge.

## Evidence (empirical, this session)

Minimal repro (`/tmp/fk_probe.py`), foreign_keys=ON, post-T34 shape
(legacy `memories` INSERT removed, only `nodes` + `access_log` remain):

```
INSERT INTO nodes (id, content) VALUES ('m1','hello');        -- ok
INSERT INTO access_log (memory_id, accessed_at) VALUES ('m1',1.0);
RESULT: FK VIOLATION as predicted -> FOREIGN KEY constraint failed
```

FK inventory — every table that `REFERENCES memories(id)`
(`grep -n "REFERENCES memories(id)" storage.rs`):

- L1076 `access_log`            — **RETAINED** → blocker
- L1081/1082 `hebbian_links`     — dropped (FK vanishes with table)
- L1133 `memory_entities`        — dropped
- L1162/1163 `synthesis_provenance` — dropped
- L1251/1276 `memory_embeddings` — dropped
- L1308 `memory_embeddings_v2`   — dropped
- L1625 `triples`                — dropped

Only `access_log` is both retained AND FK-bound to `memories`.

## Root cause

`access_log` was designed as a child of the legacy `memories` table.
In the unified substrate, a memory is `nodes(node_kind='memory')`, so
`access_log` should reference `nodes(id)` — but no Phase B/E task
re-pointed that FK. The migration plan treats `access_log` as
"untouched / out of scope" (§3.5) without noticing it is structurally
coupled to a table on the drop list.

## Proposed fix (root, not patch)

Add a Phase E precursor task (call it **T34-pre / FK re-point**):

1. Migrate `access_log.memory_id` FK from `REFERENCES memories(id)` to
   `REFERENCES nodes(id) ON DELETE CASCADE`. SQLite cannot `ALTER` an
   FK in place → requires table rebuild:
   `CREATE access_log_new (... REFERENCES nodes(id) ...)` →
   `INSERT INTO access_log_new SELECT * FROM access_log` →
   `DROP access_log` → `ALTER access_log_new RENAME TO access_log` →
   recreate indices. Bump `user_version`.
2. Verify every retained table is audited for `REFERENCES memories(id)`
   (only `access_log` found this session, but the audit must be part of
   the task, not assumed).
3. Re-point must land **before** any T34 legacy `INSERT INTO memories`
   deletion, because `Storage::add` writes `access_log` in the same tx.

Then the §5.5.2 deletion pattern becomes valid for T34a, and the
Phase F `DROP TABLE memories` no longer collides with a retained FK.

## Acceptance criteria

- [x] AC-1: `access_log` schema re-pointed to `REFERENCES nodes(id) ON DELETE CASCADE`, via table-rebuild migration, idempotent (DDL-inspection guard; no-op once `REFERENCES memories` is gone). `user_version` not used — engram migrations are idempotent functions, not version-sequenced; the DDL guard is the canonical idempotency mechanism in this codebase.
- [x] AC-2: audit confirms no OTHER retained table FK-references any dropped legacy table. Of the 7 tables with `REFERENCES memories(id)`, only `access_log` is retained; `hebbian_links`/`memory_entities`/`synthesis_provenance`/`memory_embeddings`/`memory_embeddings_v2`/`triples` are all in the §5.6.1 drop set, so their FKs vanish with the tables.
- [x] AC-3: regression tests pin the behaviour — `iss196_access_log_insert_succeeds_without_legacy_memories_row` (simulated post-T34: nodes-only parent, no `memories` row → insert succeeds), `iss196_add_writes_access_log_against_nodes_parent`, `iss196_access_log_fk_repointed_to_nodes`, `iss196_migration_idempotent`. All 4 pass.
- [x] AC-4: `cargo test -p engramai --lib` green — 2075 passed, 0 failed.
- [x] AC-5: design.md §5.5.3 updated with the T33b FK re-point precursor task and the retained→legacy FK hazard note.

## Resolution (2026-05-31)

**Fix shipped (storage.rs):**
1. `migrate_access_log_fk_to_nodes` — table-rebuild migration re-pointing
   `access_log.memory_id` from `memories(id)` to `nodes(id)`. Idempotent
   via stored-DDL inspection. Wired into `with_unified_substrate` after
   `migrate_unified_nodes` (so `nodes` exists as the new parent). The
   rebuild copies only rows whose `memory_id IN (SELECT id FROM nodes)`
   to stay FK-valid, and recreates `idx_access_log_mid`.
2. `Storage::add` reordered — `insert_memory_node_row` now runs FIRST,
   before the `access_log` insert, so the `nodes` parent exists.
3. `Storage::store_raw` reordered — the `nodes` (insight) dual-write
   moved ahead of the `access_log` insert for the same reason.

**Test fixture pulled forward (§5.6.4 work):**
`iss019_backfill_test::insert_v1_row` now also seeds the matching
`nodes` row (was seeding `memories` only), because `merge_enriched_into`
writes `access_log` which now FK-requires a `nodes` parent. This
surfaced a real latent assumption — "a `memories` row can exist with no
`nodes` row and still merge" — which the unified substrate makes false.

**Empirical FK proof:** `/tmp/fk_probe.py` (pre-fix shape) →
`FOREIGN KEY constraint failed`; post-fix `iss196_access_log_insert_succeeds_without_legacy_memories_row` →
passes.

**Note (out of scope, pre-existing):** `v04_phase_b_dual_write::t13_insert_edge_dual_writes_to_unified_edges`
and `t17_phase_b_parity_invariants_across_namespaces` fail on the clean
tree (before ISS-196) — `edge_kind` is `"structural"` where the test
expects `"assertion"`. Verified by stashing the ISS-196 diff and
re-running: both still fail. Likely a T37g read-path-era classification
drift; tracked separately, NOT introduced by this fix.

## Blocks

- T34 (Phase E memory-core legacy-write deletion) — **UNBLOCKED** by AC-1/AC-3.
- T39 (Phase F DROP legacy tables) — `DROP TABLE memories` no longer collides with the retained `access_log` FK.
