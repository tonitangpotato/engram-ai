---
title: Memory metadata UPDATEs skip nodes mirror — Storage::update, update_content, update_importance
status: fixed
priority: P1
labels:
- v04-unified-substrate
- phase-b
- dual-write
relates_to:
- ISS-119
- ISS-120
- ISS-121
- ISS-122
- ISS-123
fixed_by:
- 80d17a4
---

## Problem

Three `Storage` methods mutate columns on the legacy `memories` table without mirroring those changes to the corresponding `nodes` row, even though every insert path (T12 `add`, T16 `store_raw`) dual-writes the same columns. This is a Phase B dual-write gap that **silently breaks** the unified-substrate read contract for any record that is mutated after its initial insert.

Affected methods in `crates/engramai/src/storage.rs`:

- **`update`** (line 2477) — full `MemoryRecord` overwrite. Updates `content`, `memory_type`, `layer`, `working_strength`, `core_strength`, `importance`, `pinned`, `consolidation_count`, `last_consolidated`, `source`, `contradicts`, `contradicted_by`, `metadata` on `memories` only. Refreshes `memories_fts` rowid index, **but does not touch `nodes` or `nodes_fts`**.
- **`update_content`** (line 2604) — replaces `content` + `metadata` (typically used by consolidation rewrites). Refreshes `memories_fts`. **Does not touch `nodes` or `nodes_fts`**.
- **`update_importance`** (line 6229) — single-column `UPDATE memories SET importance` (used by synthesis auto-bump path — see `1555a26`). **Does not touch `nodes`**.

## How this manifests under `unified_substrate=true`

After any of these UPDATEs lands, the two substrates diverge:

| Field | `memories` row | `nodes` row | Risk |
|---|---|---|---|
| `content` (after `update` / `update_content`) | new | **stale** | Unified FTS search misses the new content; topic clustering uses old text |
| `importance` (after `update_importance`) | new | **stale** | Salience-weighted retrieval rank differs between substrates |
| `metadata` JSON | new | **stale** | Attribute-based filters return wrong results |
| `last_consolidated` (after `update`) | new | **stale** | Lifecycle/decay path may double-consolidate |

ISS-119/120/121/122/123 collectively prove the pattern: every writer that touches `memories` needs a parallel write to `nodes`. These three are the last unaudited ones in the memory-substrate writer set.

## Root cause analysis

Same family as ISS-121/122/123: the Phase B writers were added incrementally, and the existing UPDATE paths weren't refactored to dual-write because the early v04 design treated them as out-of-scope ("Phase D readers will reveal the gaps"). T29.5/T29.6 are now revealing them. T12/T16 dual-write the **inserts**; nothing patches the **mutations**.

## Fix plan

For each method, wrap the existing `memories` UPDATE in a transaction (most already do via `needs_tx` / `update_inner`), then add a parallel `nodes` UPDATE before the FTS index refresh:

```rust
// Inside update_inner / update_content_inner / update_importance:
self.conn.execute(
    r#"
    UPDATE nodes SET
        content = ?, memory_type = ?, layer = ?, importance = ?, ...
    WHERE id = ?1 AND node_kind = 'memory'
    "#,
    params![...],
)?;
```

Triggers on `nodes` (`nodes_fts_au` on `UPDATE OF content, summary`) will keep `nodes_fts` in sync automatically — no manual FTS refresh needed for the nodes side.

**Order**: legacy UPDATE first, then nodes UPDATE, then legacy FTS refresh (mirrors the insert order in T12 helper). Both UPDATEs must be in the same transaction.

**Skip-if-missing**: under Phase B production state (pre-T26c backfill), some legacy rows have no corresponding `nodes` row yet. The nodes UPDATE should silently no-op (UPDATE on missing PK = 0 rows affected = no error). Backfill will fill the gap. No FK guard needed because nodes has no FK back to memories on the data path.

**Important — `update_importance` namespace**: this method uses `Box<dyn Error>` return type and is called from the synthesis path. The fix should keep that signature.

## Contract tests (per method)

Each method gets a contract test under `crates/engramai/tests/iss124_*.rs`:

1. **iss124_update_dual_writes_to_nodes**: seed via `add`, mutate via `update`, query both substrates, assert content/importance/metadata match.
2. **iss124_update_content_dual_writes_to_nodes**: seed via `add`, call `update_content("new")`, assert `nodes.content = "new"` AND `nodes_fts` matches new content (search returns the row under the new query, not the old one).
3. **iss124_update_importance_dual_writes_to_nodes**: seed via `add` (importance=0.3), call `update_importance(0.9)`, assert both substrates show 0.9.
4. **iss124_update_idempotent**: re-call same `update` twice, no second-update divergence.
5. **iss124_update_on_missing_node_noop**: corner case — legacy row exists but `nodes` row missing (pre-backfill state); the nodes UPDATE should silently no-op without erroring.

## Out of scope

- `delete` (hard delete) — separate issue if needed; current behavior is intentional for cascade tests.
- `record_access` — only touches `access_log` table, not metadata columns.
- `supersede` / `unsupersede` — already T12 dual-write (verified in t12_dual_write_superseded_by_root_fix).
- `soft_delete` — already fixed (ISS-121).

## Acceptance criteria

- [ ] `update`, `update_content`, `update_importance` each dual-write to `nodes` in the same transaction as the legacy UPDATE
- [ ] 5 contract tests under iss124_* all pass
- [ ] 1902/1902 lib tests still pass
- [ ] Phase B/D peer tests (v04_phase_b_dual_write, all T29.5 + T29.6 + ISS-121/122/123 tests) all still pass
- [ ] Design.md §8.4 (Phase B writer audit) adds a note documenting that the UPDATE family was closed in this issue
