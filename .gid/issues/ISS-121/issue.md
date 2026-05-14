---
title: soft_delete does not dual-write deleted_at to nodes; Phase D liveness filters diverge
status: open
priority: P1
severity: degradation
filed_by: rustclaw (overnight session 2026-05-14)
relates_to:
  - v04-unified-substrate
  - T29
  - ISS-119
  - ISS-120
---

## Symptom

`Storage::soft_delete` (storage.rs:3611) is a single-table UPDATE:

```rust
pub fn soft_delete(&self, id: &str) -> Result<(), rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    self.conn.execute(
        "UPDATE memories SET deleted_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}
```

`nodes.deleted_at` for the same row is **not updated**. The two columns diverge for every soft-delete that runs after T12 dual-write shipped.

Concretely: after `soft_delete("m1")`:
- `memories.deleted_at = '2026-05-14T04:35:00+00:00'` (TEXT, RFC3339)
- `nodes.deleted_at = NULL` (REAL, never written)

## Discovery context

Caught during the same 2026-05-14 overnight Phase D scoping pass that found ISS-119 and ISS-120. While inventorying ✅ "trivial-safe" readers for the safe-readers batch commit, I checked dual-write coverage for `deleted_at` and found the soft_delete writer is single-table.

`hard_delete_cascade` (the harder cousin) **does** correctly hit both substrates per its comments — explicitly documented as "closes ISS-115 — Phase B dual-WRITE writers". soft_delete was not migrated alongside.

## Why this matters

Every Phase D reader that filters on liveness (`deleted_at IS NULL`) will diverge between substrates the moment any soft-delete happens. Specifically affects:

- `count_memories_in_namespace` (both branches filter `deleted_at IS NULL`)
- `count_soft_deleted` (filters `IS NOT NULL`, the inverse — would *under-count* on unified)
- `list_deleted`
- `list_namespaces`
- `search_fts` / `search_fts_ns` / `fetch_recent` / `search_by_type*` (all have `deleted_at IS NULL` in WHERE)
- The full Phase D hot-retrieval cohort (T29.7) once ISS-119 lands

In production: soft-deleted memories would re-surface in unified reads. Confidence/ACT-R penalties might also fire on already-deleted items because the reader thinks they're alive.

## Production DB audit (2026-05-14)

Running on `/Users/potato/rustclaw/engram-memory.db` to estimate blast radius is **safe** (read-only). Not running tonight — operational task for daylight.

## Fix

```rust
pub fn soft_delete(&self, id: &str) -> Result<(), rusqlite::Error> {
    let tx = self.conn.transaction()?;
    let now_rfc = chrono::Utc::now().to_rfc3339();
    let now_epoch = chrono::Utc::now().timestamp() as f64
        + (chrono::Utc::now().timestamp_subsec_nanos() as f64) / 1e9;
    tx.execute(
        "UPDATE memories SET deleted_at = ?1 WHERE id = ?2",
        params![now_rfc, id],
    )?;
    tx.execute(
        "UPDATE nodes SET deleted_at = ?1, updated_at = ?1 \
         WHERE id = ?2 AND node_kind = 'memory'",
        params![now_epoch, id],
    )?;
    tx.commit()?;
    Ok(())
}
```

Caveats:
- Two timestamps (RFC3339 string for `memories`, epoch f64 for `nodes`) come from `Utc::now()` called twice — should be computed once and converted. Minor cleanup in the patch.
- Need a Phase C backfill pass: walk `memories WHERE deleted_at IS NOT NULL`, parse RFC3339 to epoch, write `nodes.deleted_at` for matching ids. Idempotent.

## Acceptance criteria

- [ ] `soft_delete` wraps both UPDATEs in a single transaction.
- [ ] After `soft_delete(id)`, `nodes.deleted_at` is non-NULL and equals the epoch form of `memories.deleted_at`.
- [ ] Backfill driver `backfill_soft_delete_into_nodes` walks legacy soft-deletes and patches `nodes.deleted_at`. Idempotent (re-run = 0 updates).
- [ ] Tests: `tests/iss121_soft_delete_dual_write.rs`
  - Round-trip: ingest, soft_delete, assert both columns set.
  - Liveness filter parity: ingest 3 memories, soft-delete one, run a count query on both substrates, assert equal.
  - Backfill idempotency.

## Out of scope

- `restore_deleted` / `undelete` — search showed none exist. If they are added later, they need the same dual-write treatment.
- Hard delete — already dual-writes per `hard_delete_cascade` docs.

## Notes

This issue plus ISS-119 + ISS-120 are the **three** Phase B dual-write gaps that the original T12–T18 acceptance criteria did not catch. Recommend adding an explicit per-column round-trip parity test suite as a permanent gate (something like `tests/phase_b_dual_write_round_trip.rs`) so the next category of gap is caught at the writer-side milestone instead of the reader-side scoping pass.
