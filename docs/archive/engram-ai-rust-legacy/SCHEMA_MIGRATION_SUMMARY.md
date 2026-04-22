# Schema Migration Summary - Rust Crate

## Overview
Updated the Rust crate to conform to the canonical schema defined in the Python project's design doc.

## Changes Made

### 1. Timestamp Format Change (TEXT → REAL)
All timestamp columns now store Unix float (seconds since epoch) instead of ISO 8601 strings.

**Tables updated:**
- `memories.created_at` — TEXT → REAL
- `memories.last_consolidated` — TEXT → REAL
- `access_log.accessed_at` — TEXT → REAL
- `hebbian_links.created_at` — TEXT → REAL
- `engram_acl.created_at` — TEXT → REAL
- `behavior_log.timestamp` — TEXT → REAL (consistency update)
- `subscriptions.created_at` — TEXT → REAL (consistency update)
- `notification_cursor.last_checked` — TEXT → REAL (consistency update)
- `emotional_trends.last_updated` — TEXT → REAL (consistency update)

### 2. New Tables Added

**`engram_meta`** - Schema versioning table:
```sql
CREATE TABLE IF NOT EXISTS engram_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO engram_meta VALUES ('schema_version', '1');
```

**`entities`** - Entity storage:
```sql
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    namespace TEXT NOT NULL DEFAULT 'default',
    metadata TEXT,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
```

**`entity_relations`** - Entity relationships:
```sql
CREATE TABLE IF NOT EXISTS entity_relations (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relation TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    source TEXT,
    namespace TEXT NOT NULL DEFAULT 'default',
    created_at REAL NOT NULL,
    metadata TEXT
);
```

**`memory_entities`** - Memory-entity links:
```sql
CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'mention',
    PRIMARY KEY (memory_id, entity_id)
);
```

### 3. Code Changes

**Files Modified:**
- `src/storage.rs` - Core schema and storage operations
- `src/bus/feedback.rs` - Behavior logging timestamps
- `src/bus/subscriptions.rs` - Subscription and notification timestamps
- `src/bus/accumulator.rs` - Emotional trend timestamps

**Helper Functions Added:**
Each file now includes:
```rust
fn datetime_to_f64(dt: &DateTime<Utc>) -> f64
fn f64_to_datetime(ts: f64) -> DateTime<Utc>
fn now_f64() -> f64
```

**In-Memory Representation:**
- Types remain as `DateTime<Utc>` in Rust structs (no breaking changes to API)
- Only the database storage format changed (TEXT → REAL)

### 4. Column Name Verification
✅ Confirmed: The Rust code already uses `source` (not `source_file`), matching the Python schema.

## Migration Notes

### For New Databases
- Schema creates tables with REAL timestamps automatically
- No migration needed

### For Existing Databases
- SQLite doesn't support `ALTER COLUMN` type changes
- Existing databases will need manual migration or recreation
- The code can read REAL timestamps (forward compatible)
- **Not implemented**: Automatic migration from TEXT to REAL for existing DBs

### Test Updates
- Updated test helper functions to create tables with REAL timestamps
- Changed `datetime('now')` to `strftime('%s','now')` in test INSERT statements

## Compatibility

**Forward Compatible:**
- New code reads REAL timestamps correctly
- Old DBs with TEXT timestamps will need migration

**API Compatibility:**
- No breaking changes to public API
- All methods still accept/return `DateTime<Utc>`
- Internal storage format is abstracted away

## Verification

Run these checks to verify the migration:
```bash
# No RFC3339 string operations remain
grep -rn "rfc3339" src/ --include="*.rs"  # Should return 0 results

# Verify schema contains REAL timestamps
sqlite3 test.db ".schema memories"  # Should show created_at REAL

# Check entity tables exist
sqlite3 test.db ".tables"  # Should list entities, entity_relations, memory_entities
```

## Status
✅ Schema migration complete
✅ All timestamp operations updated
✅ Entity tables added
✅ Tests updated
✅ No logic changes made
🔲 Data migration script (not implemented - out of scope)
