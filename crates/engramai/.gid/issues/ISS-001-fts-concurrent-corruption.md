# ISS-001: FTS5 Index Corruption Under Multi-Process Concurrent Writes

## Status: FIXED (2026-04-14)

## Severity: Critical (data corruption)

## Summary

When two or more processes (e.g., two RustClaw instances) share the same engram SQLite database, the FTS5 full-text search index (`memories_fts`) becomes corrupted during concurrent writes. This manifests as repeated `[engram] FTS corruption detected during update, rebuilding index...` errors in stderr, with 43+ occurrences observed in a single log period.

## Root Cause

Several storage methods perform **non-atomic FTS operations** — a DELETE followed by an INSERT on the `memories_fts` table without transaction wrapping. When two processes interleave these operations, the FTS index enters an inconsistent state.

### Affected Methods (no transaction wrapping)

| Method | Operation | Risk |
|--------|-----------|------|
| `update()` | DELETE fts + INSERT fts | **High** — most common write path |
| `delete()` | SELECT rowid + DELETE fts + DELETE memory | **High** — FTS delete not atomic with memory delete |
| `update_content()` | UPDATE memory + DELETE fts + INSERT fts | **High** — used by memory updates |
| `store_raw()` | INSERT memory + INSERT fts | **Medium** — no transaction, but comment says "caller manages transaction" (not enforced) |
| `rebuild_fts_if_needed()` | DELETE all fts + bulk INSERT | **Medium** — runs at startup only |

### Already Safe

| Method | Why |
|--------|-----|
| `add()` | Uses `self.conn.transaction()` — FTS insert is inside the tx |
| `begin_transaction()` / `commit_transaction()` | Exist but are manual — callers must remember to use them |

### Corruption Sequence (concrete example)

```
Process A: update() → DELETE FROM memories_fts WHERE rowid = 42
  ← context switch, Process B acquires write lock →
Process B: update() → DELETE FROM memories_fts WHERE rowid = 99
Process B: update() → INSERT INTO memories_fts(rowid, content) VALUES (99, "...")
  ← Process A resumes →
Process A: INSERT INTO memories_fts(rowid, content) VALUES (42, "...")
  ← FTS internal B-tree now has inconsistent state from interleaved ops →
```

## Environment

- **Database**: `/Users/potato/rustclaw/engram-memory.db` (82MB, WAL mode)
- **Processes**: Two RustClaw instances (PIDs observed: 94611, 99065) sharing same DB
- **engramai version**: v0.2.2 (local path dep with busy_timeout=5000 already added)
- **busy_timeout**: Already present in local source (not in published crate)
- **First observed**: 2026-03-29 (`.bak-malformed` files from that date)
- **Workaround applied**: Manual `INSERT INTO memories_fts(memories_fts) VALUES('rebuild')` — temporary, corruption recurs

## Fix

Wrap all FTS write operations in `BEGIN IMMEDIATE` transactions. This ensures:

1. **Atomicity**: DELETE+INSERT on FTS is a single atomic unit
2. **Serialization**: `BEGIN IMMEDIATE` acquires a write lock upfront; other processes wait (up to busy_timeout) rather than interleaving
3. **Crash safety**: If a process crashes mid-transaction, SQLite WAL auto-rollbacks

### Changes Required (all in `storage.rs`)

1. **`update()`** — wrap entire body in `self.conn.transaction()`
2. **`delete()`** — wrap rowid lookup + FTS delete + memory delete in transaction
3. **`update_content()`** — wrap memory update + FTS delete + FTS insert in transaction
4. **`rebuild_fts_if_needed()`** — wrap bulk DELETE + INSERT loop in transaction
5. **`store_raw()`** — add explicit transaction (don't rely on caller)

## Verification

After fix:
1. Rebuild FTS index once: `INSERT INTO memories_fts(memories_fts) VALUES('rebuild')`
2. Run both RustClaw instances simultaneously
3. Monitor stderr for FTS corruption messages — should be zero
4. Run `PRAGMA integrity_check` — should return "ok"

## History

- 2026-03-29: First corruption observed, manual FTS rebuild applied
- 2026-04-14: 43 corruption errors in current log, root cause identified as missing transactions
