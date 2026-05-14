---
title: Storage::delete (hard) skips nodes — orphan unified row survives hard-delete
status: fixed
priority: P1
labels:
- v04-unified-substrate
- phase-b
- dual-write
relates_to:
- ISS-121
- ISS-124
- ISS-125
fixed_by:
- _pending_commit_sha_
---

## Problem

`Storage::delete(id)` in `crates/engramai/src/storage.rs:2706` hard-deletes from legacy `memories` (and `memories_fts`) but **does not** delete from the unified `nodes` table (or trigger `nodes_fts` cleanup). After a hard delete:

- Legacy path: `get(id)` returns `None`. ✅
- Unified path: `nodes` still has the row. The unified read path returns the deleted memory. ❌

Same family as ISS-121 (soft_delete), ISS-124 (update family), ISS-125 (delete_embedding).

## Surface

Searching for production callers:

```
$ grep -rn "\.delete(" crates/engramai/src/ | grep -v "embeddings\|fts\|deleted_at"
```

Hard delete is used by:
- Test fixtures (cleanup)
- `decay_hebbian_links` purge path (?)
- `consolidation/` paths that purge low-strength memories
- Cascade tests

Even if production usage is rare (soft_delete is preferred), the bug still violates the v04 substrate contract — the two substrates must agree on what exists.

## Fix plan

Add `DELETE FROM nodes WHERE id = ?` (and cascade to `edges` via FK if defined, else explicit DELETE FROM edges) inside `delete_inner`, in the same transaction as the existing legacy DELETE:

```rust
fn delete_inner(&self, id: &str) -> Result<(), rusqlite::Error> {
    // ... existing FTS cleanup + memories DELETE ...

    // ISS-126: dual-DELETE on nodes. The nodes_fts_ad trigger
    // cleans up nodes_fts automatically. Edges with this id as
    // src/dst must also be removed to avoid dangling refs — this
    // mirrors the legacy delete cascade.
    self.conn.execute(
        "DELETE FROM edges WHERE src_id = ? OR dst_id = ?",
        params![id, id],
    )?;
    self.conn.execute("DELETE FROM nodes WHERE id = ?", params![id])?;
    Ok(())
}
```

Open question: does `nodes` have ON DELETE CASCADE for `edges`? Let me check.

## Contract tests

Under `crates/engramai/tests/iss126_hard_delete_dual_write.rs`:

1. **iss126_delete_clears_nodes_row**: add memory, hard delete, assert `nodes` row gone.
2. **iss126_delete_clears_nodes_fts**: add memory with searchable content, delete, assert unified search_fts returns no hit.
3. **iss126_delete_clears_inbound_edges**: add memory + linked entity (creates structural edge in unified), delete memory, assert edge gone.
4. **iss126_delete_on_missing_id_noop**: idempotency check.

## Out of scope

- Soft delete already fixed (ISS-121).
- `decay_hebbian_links` deletes from hebbian table not memories — different surface, T14 covers it.

## Acceptance criteria

- [ ] `delete_inner` mirrors DELETE onto `nodes` + cascading edges
- [ ] 4 contract tests pass
- [ ] 1902/1902 lib tests still pass
- [ ] Phase B/D peer tests all still pass
