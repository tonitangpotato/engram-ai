---
title: Phase B dual-write missing for Storage::upsert_entity; blocks T29.5 entity-reader switch
status: open
priority: P1
severity: degradation
filed_by: rustclaw (overnight session 2026-05-14)
relates_to:
  - v04-unified-substrate
  - T29.5
  - ISS-119
  - ISS-120
---

## Symptom

`Storage::upsert_entity` (storage.rs:4687) writes into the legacy `entities` table but does **not** dual-write to `nodes`. T13 dual-write covers writes from `graph_entities` (the v0.3 resolution-pipeline `Entity` type, via `SqliteGraphStore::insert_entity`), but the v0.2 `Storage::upsert_entity` path — still used by direct callers and tests — has no `nodes` projection.

```rust
pub fn upsert_entity(&self, name: &str, entity_type: &str, namespace: &str, ...) {
    self.conn.execute(
        "INSERT INTO entities (id, name, entity_type, namespace, ...) ...",
        ...
    )?;
    // ← no insert_entity_node_row, no dual-write
}
```

T21 backfill closes the *historical* gap (legacy `entities` → `nodes`), but new writes after `unified_substrate` flips on still go to legacy-only. A Phase D reader switch for `get_entity` / `find_entities` / `list_entities` / `entity_stats` would silently lose any post-flip `upsert_entity` row.

This is the entity-side analogue of T12 (memory dual-write into `add`/`store_raw`). The PHASE-D-READER-AUDIT.md flagged the *reader-side* `entity_type` gap (ISS-120) but missed the writer-side asymmetry.

## Discovery context

Found 2026-05-14 ~05:00 EDT during overnight Phase D push, while scoping the T29.5 entity-reader switch. Same audit pass that surfaced ISS-119 / ISS-120.

## Fix

Mirror the T12 pattern: wrap `upsert_entity` in a transaction and call `Storage::insert_entity_node_row` after the legacy INSERT, using the same `attributes_json` projection T21 backfill uses (`{"entity_type": <column>, ...metadata-keys (existing-wins)}`). Pre-empt ISS-120 by also stamping `_legacy_kind` via the canonical-label → `EntityKind` mapping from `wrap_legacy_entity_type` — but since the v0.2 path only ever produces flat string labels (not `EntityKind`), the simpler approach is to leave `attributes.entity_type` as the legacy stamp and let the T29.5 reader use the `entity_type` fallback in `extract_legacy_entity_kind` (already implemented in ISS-120).

## Acceptance criteria

- AC1: `Storage::upsert_entity` writes 1 row to `entities` AND 1 row to `nodes` in the same transaction.
- AC2: ON CONFLICT update path on `entities` propagates to a corresponding nodes update (or stays consistent via T21's existing-wins merge semantics).
- AC3: Reading the nodes row through `extract_legacy_entity_kind` reconstructs the original `entity_type` exactly (canonical labels round-trip, non-canonical → `Other(s)`).
- AC4: Regression test in `tests/iss122_upsert_entity_dual_write.rs` covering: fresh insert, re-upsert (idempotency), metadata merge, namespace round-trip.

## Why it wasn't caught earlier

T12 acceptance tests covered `Storage::add` (the memory path). T13 acceptance tests covered `SqliteGraphStore::insert_entity` (the v0.3 graph path). Nobody audited `Storage::upsert_entity` (the v0.2 entity path) because the audit doc focused on reader-side gaps. T17 row-count parity invariants only check post-backfill state — they don't catch writer-side asymmetries that arise after a partial cutover.

## Refs

- `.gid/features/v04-unified-substrate/PHASE-D-READER-AUDIT.md`
- ISS-119 (memory-side variant)
- ISS-120 (entity-side reader gap)
- T12 (memory dual-write pattern)
- T21 (entity backfill — provides the projection template)
