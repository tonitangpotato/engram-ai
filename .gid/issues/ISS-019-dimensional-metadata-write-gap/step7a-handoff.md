# ISS-019 Step 7a — Handoff for next agent spawn

**Status:** NOT STARTED — previous spawn exhausted iteration budget on
investigation before writing code. Nothing committed. Repo clean at
HEAD `d58847c`.

**Workspace (CORRECT):** `/Users/potato/clawd/projects/engram-ai-rust`
(NOT `/Users/potato/rustclaw` — that's a different repo.)

**Why this doc exists:** the investigation was thorough but the budget
was blown on reads. All findings below are verified; start coding
immediately on respawn, do not re-investigate.

---

## Scope recap (from potato, unchanged)

v2 metadata write path + dual-path read compatibility. No new tables,
no new APIs, no new state machines. ~300 lines. Step 7b (legacy
classification + backfill_queue) is **deferred** — do not touch.

---

## Write points (need to produce v2 JSON)

### 1. `src/memory.rs:4801` — `fn build_legacy_metadata`

Currently writes **flat v1**:

```json
{
  "dimensions": {"participants": "...", "valence": 0.3, ...},
  "type_weights": {"episodic": 1.0, ...},
  "<user_key>": "<user_value>"   // merged at top level
}
```

Change to v2:

```json
{
  "engram": {
    "version": 2,
    "dimensions": {"participants": "...", "valence": 0.3, ...,
                   "type_weights": {"episodic": 1.0, ...}},
    "merge_count": 0,
    "merge_history": []
  },
  "user": { /* caller-supplied keys, unmodified */ }
}
```

Rename fn to `build_v2_metadata` (no callers outside memory.rs — grep
shows only `memory.rs:1798` calls it).

**Decision on `type_weights` location:** design §6 nests it inside
`engram.dimensions.type_weights`. Do that. It matches
`Dimensions::serialize` if you use `serde_json::to_value(&dims)`,
which is **cleaner than the hand-rolled map-building code**.

### 2. `src/enriched.rs:329` — `fn EnrichedMemory::to_legacy_metadata`

Same shape. Rename → `to_v2_metadata`. Update callers:

```
src/storage.rs:3240  (merge_enriched_into)
```

Grep for `to_legacy_metadata` to confirm — likely only one call site.

---

## Read points (need v2-first, v1-fallback)

### 1. `src/dimensions.rs:367` — `fn from_legacy_metadata`

Add a new sibling method `from_stored_metadata(metadata, core_fact)`:

```rust
pub fn from_stored_metadata(
    metadata: &serde_json::Value,
    core_fact: &str,
) -> Result<Self, EmptyCoreFactError> {
    // v2 path: metadata.engram.dimensions present?
    if let Some(engram) = metadata.get("engram") {
        if let Some(dims_val) = engram.get("dimensions") {
            // Try strict deserialize; fall through to v1 on failure.
            if let Ok(mut d) = serde_json::from_value::<Dimensions>(dims_val.clone()) {
                // Ensure core_fact matches the row's content column
                // (authoritative source of truth per design §6).
                if let Ok(core) = NonEmptyString::new(core_fact) {
                    d.core_fact = core;
                }
                return Ok(d);
            }
        }
    }
    // v1 fallback
    Self::from_legacy_metadata(metadata, core_fact)
}
```

Keep `from_legacy_metadata` public — it's the v1-only path and tests
can exercise it directly. Add a doc note that new code should prefer
`from_stored_metadata`.

### 2. Merge tracking reads in `src/storage.rs`

Three sites read `merge_history` / `merge_count` from stored metadata.
All currently assume top-level keys (v1 layout).

- **Line 3088-3106** (`merge_memory_into` — legacy untyped path)
- **Line 3268-3282** (`merge_enriched_into`)
- **Line 3655-3662** (another `merge_history` touch — verify exact fn)

Add a module-private helper:

```rust
// In storage.rs, near the other merge helpers.

/// Read (merge_history, merge_count) from stored metadata,
/// checking v2 (`engram.*`) first and falling back to v1 (top-level).
fn read_merge_tracking(metadata: &serde_json::Value) -> (Vec<serde_json::Value>, i64) {
    // v2
    if let Some(engram) = metadata.get("engram") {
        let history = engram.get("merge_history")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let count = engram.get("merge_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        // Only treat as v2 if engram object exists at all
        return (history, count);
    }
    // v1
    let history = metadata.get("merge_history")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let count = metadata.get("merge_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    (history, count)
}

/// Write (merge_history, merge_count) into metadata at the v2 location
/// (`engram.merge_history`, `engram.merge_count`). Caller ensures the
/// blob is already v2-shaped (i.e. written via build_v2_metadata or
/// to_v2_metadata).
fn write_merge_tracking(
    metadata: &mut serde_json::Value,
    history: Vec<serde_json::Value>,
    count: i64,
) {
    if let Some(obj) = metadata.as_object_mut() {
        let engram = obj.entry("engram".to_string())
            .or_insert_with(|| serde_json::json!({"version": 2}));
        if let Some(e_obj) = engram.as_object_mut() {
            e_obj.insert("merge_history".into(), serde_json::Value::Array(history));
            e_obj.insert("merge_count".into(), serde_json::json!(count));
        }
    }
}
```

Replace the three inline blobs with calls to these helpers.

### 3. Any other stored-metadata deserialization → `EnrichedMemory`

Grep for `EnrichedMemory::from_stored` / `from_record` /
`from_legacy_metadata` callers. The merge path rebuilds
`existing_em` from the DB record somewhere before line 3200. That
path must route through `from_stored_metadata`, not
`from_legacy_metadata` directly.

```
grep -n "from_legacy_metadata\|from_stored_metadata" src/
```

Any call site using `from_legacy_metadata` outside of `from_stored_metadata`
itself and explicit v1 tests is a bug — switch to `from_stored_metadata`.

---

## Tests

### Unit tests in `src/dimensions.rs`

Add to the existing `#[cfg(test)] mod tests` block (after line ~1226
where test helpers already exist):

1. `test_from_stored_metadata_v2_roundtrip` — build a `Dimensions`,
   serialize via `to_v2_metadata`-shaped JSON by hand, read back via
   `from_stored_metadata`, assert equal.
2. `test_from_stored_metadata_v1_fallback` — hand-craft a v1 JSON
   (flat `dimensions` + `type_weights`), call `from_stored_metadata`,
   assert fields come through correctly.
3. `test_from_stored_metadata_empty` — empty object, core_fact only,
   returns `Dimensions::minimal`-equivalent.
4. `test_v1_and_v2_produce_identical_dimensions` — same logical
   signature expressed in both layouts → equal `Dimensions` output.

### Integration test — **HARD REQUIREMENT, do not skip**

New file: `tests/iss019_v2_metadata_compat.rs`

```rust
//! ISS-019 Step 7a — read-path backward compatibility for v1 metadata.
//!
//! Constructs a v1 metadata JSON row **by hand** (simulating an old
//! DB entry written before the v2 layout existed), inserts it directly
//! into the memories table, and verifies the read path parses it
//! correctly without going through the write path.

use engramai::storage::MemoryStorage;
use rusqlite::params;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn manual_v1_metadata_parses_correctly() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("test.db");
    let mut storage = MemoryStorage::new(db.to_str().unwrap()).unwrap();

    // Hand-built v1 blob — mimics what pre-Step-7a engram wrote.
    let v1_metadata = json!({
        "dimensions": {
            "participants": "alice, bob",
            "temporal": "2026-04-22",
            "causation": "kickoff meeting",
            "valence": 0.4,
            "domain": "coding",
            "confidence": "likely",
            "tags": ["standup", "planning"]
        },
        "type_weights": {
            "episodic": 1.2, "factual": 1.0, "procedural": 0.8,
            "semantic": 1.0, "emotional": 0.5
        },
        "merge_count": 2,
        "merge_history": [
            {"ts": 1700000000, "sim": 0.93, "content_updated": false,
             "prev_content_len": 50, "new_content_len": 48}
        ]
    });

    // Direct INSERT bypasses write path — this is the whole point.
    let id = "v1_legacy_row_001";
    storage.conn().execute(
        "INSERT INTO memories (id, content, importance, metadata, created_at)
         VALUES (?, ?, ?, ?, ?)",
        params![
            id,
            "Alice and Bob had a kickoff meeting",
            0.6_f64,
            serde_json::to_string(&v1_metadata).unwrap(),
            1700000000_i64
        ],
    ).unwrap();

    // Read via public API → Dimensions must reflect v1 content.
    let record = storage.get_by_id(id).unwrap().expect("row exists");
    let dims = engramai::dimensions::Dimensions::from_stored_metadata(
        record.metadata.as_ref().unwrap(),
        &record.content,
    ).unwrap();

    assert_eq!(dims.participants.as_deref(), Some("alice, bob"));
    assert_eq!(dims.causation.as_deref(), Some("kickoff meeting"));
    assert_eq!(dims.valence.get(), 0.4);
    assert_eq!(dims.domain, engramai::dimensions::Domain::Coding);
    assert_eq!(dims.tags.len(), 2);
    assert!(dims.tags.contains("standup"));
}

#[test]
fn v1_and_v2_equivalent_layouts_yield_identical_dimensions() {
    // Same logical signature in both layouts.
    let v1 = json!({
        "dimensions": {
            "participants": "alice",
            "valence": 0.3,
            "domain": "research",
            "confidence": "confident",
            "tags": ["idea"]
        },
        "type_weights": {"episodic": 1.0, "factual": 1.0, "procedural": 1.0,
                         "semantic": 1.0, "emotional": 1.0}
    });
    let v2 = json!({
        "engram": {
            "version": 2,
            "dimensions": {
                "core_fact": "note",
                "participants": "alice",
                "valence": 0.3,
                "domain": "research",
                "confidence": "confident",
                "tags": ["idea"],
                "type_weights": {"episodic": 1.0, "factual": 1.0,
                                 "procedural": 1.0, "semantic": 1.0,
                                 "emotional": 1.0}
            },
            "merge_count": 0,
            "merge_history": []
        },
        "user": {}
    });

    let d1 = engramai::dimensions::Dimensions::from_stored_metadata(&v1, "note").unwrap();
    let d2 = engramai::dimensions::Dimensions::from_stored_metadata(&v2, "note").unwrap();

    assert_eq!(d1.participants, d2.participants);
    assert_eq!(d1.valence.get(), d2.valence.get());
    assert_eq!(d1.domain, d2.domain);
    assert_eq!(d1.confidence, d2.confidence);
    assert_eq!(d1.tags, d2.tags);
}
```

**Expose** any private helpers the test needs (e.g. `storage.conn()`
accessor — check if one exists; if not, prefer a test-only
`#[cfg(test)] pub fn conn_for_test(&self)` method on MemoryStorage).
Do not widen public API for production.

---

## Verification checklist (must pass before commit)

1. `cargo build` — no warnings (CI is strict).
2. `cargo test --lib` — ≥925 passing (baseline before this change).
3. `cargo test --test '*'` — all 9 existing integration tests + 2
   new tests in `iss019_v2_metadata_compat.rs` pass.
4. `cargo clippy --all-targets -- -D warnings`.
5. Verify no commits to Step 7b territory (no `LegacyClassification`
   enum, no `backfill_queue` table, no `backfill_dimensions` API).

---

## Commit message template

```
feat(metadata): ISS-019 Step 7a — v2 namespaced layout + v1 read compat

Writes now produce:
  {"engram": {"version": 2, "dimensions": {...},
              "merge_count": N, "merge_history": [...]},
   "user": {...}}

Reads try v2 (engram.dimensions) first, fall back to v1 (flat) layout.
No data migration — v1 rows remain readable; the explicit backfill
job is Step 7b.

Read points updated in storage.rs:
- merge_memory_into (legacy untyped merge)
- merge_enriched_into (typed merge)
- <any third site found via grep>

Write points:
- memory::build_v2_metadata (was build_legacy_metadata)
- EnrichedMemory::to_v2_metadata (was to_legacy_metadata)

New: Dimensions::from_stored_metadata (v2-first, v1-fallback).
Keeps Dimensions::from_legacy_metadata for explicit v1-only callers
(tests).

Tests:
- dimensions.rs unit tests for v1/v2 equivalence
- tests/iss019_v2_metadata_compat.rs — hand-built v1 JSON inserted
  directly via SQL, read path verified (proves real backward compat,
  not just self-write self-read)
```

---

## Files modified (expected)

- `src/dimensions.rs` — add `from_stored_metadata` + unit tests
- `src/memory.rs` — rewrite `build_legacy_metadata` → `build_v2_metadata`
- `src/enriched.rs` — rewrite `to_legacy_metadata` → `to_v2_metadata`
- `src/storage.rs` — 3 merge-tracking read sites → helpers
- `tests/iss019_v2_metadata_compat.rs` — new integration test

Target diff size: ~300 LOC (+200 net, -100 replaced).

---

## What this spawn actually produced

- Full investigation (above).
- This handoff doc.
- Engram memory entry tagged `ISS-019 Step 7a status at iter 22/25`.

**Nothing in `src/` was modified. No new commits. Repo clean.**
