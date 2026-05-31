//! ISS-199 (Phase E read-cutover): contract tests for the triple
//! extraction attempt counter and the unenriched-id selection.
//!
//! Under unified mode (T34a) the legacy `memories` table is no longer
//! written. The attempt counter, which historically lived in
//! `memories.triple_extraction_attempts`, moves into the
//! `nodes.attributes` JSON under the reserved key
//! `$._triple_extraction_attempts`. These tests pin:
//!
//!   1. `increment_extraction_attempts` writes the unified location
//!      (and round-trips through `json_extract`) under unified mode.
//!   2. `get_unenriched_memory_ids` reads from `nodes` under unified
//!      mode and respects the `max_retries` cap against the JSON
//!      counter.
//!   3. Legacy mode still uses the `memories.triple_extraction_attempts`
//!      column (no behavioural regression for non-unified callers).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};

fn sample(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 31, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Working,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.0,
        importance: 0.5,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss199-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    }
}

/// Read the unified attempt counter straight from `nodes.attributes`.
fn unified_attempts(storage: &Storage, id: &str) -> i64 {
    storage
        .connection()
        .query_row(
            "SELECT COALESCE(json_extract(attributes, '$._triple_extraction_attempts'), 0) \
             FROM nodes WHERE id = ?1 AND node_kind IN ('memory', 'insight')",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .expect("query unified attempts")
}

/// Read the legacy attempt counter from the `memories` column.
fn legacy_attempts(storage: &Storage, id: &str) -> i64 {
    storage
        .connection()
        .query_row(
            "SELECT triple_extraction_attempts FROM memories WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .expect("query legacy attempts")
}

#[test]
fn unified_increment_writes_nodes_attributes_json() {
    let mut storage = Storage::with_unified_substrate(":memory:", true).unwrap();
    storage.add(&sample("mem-1", "rust is a language"), "default").unwrap();

    // Missing key reads as 0 (matches column DEFAULT 0).
    assert_eq!(unified_attempts(&storage, "mem-1"), 0);

    storage.increment_extraction_attempts("mem-1").unwrap();
    assert_eq!(unified_attempts(&storage, "mem-1"), 1);

    storage.increment_extraction_attempts("mem-1").unwrap();
    storage.increment_extraction_attempts("mem-1").unwrap();
    assert_eq!(unified_attempts(&storage, "mem-1"), 3);
}

#[test]
fn unified_get_unenriched_reads_nodes_and_respects_cap() {
    let mut storage = Storage::with_unified_substrate(":memory:", true).unwrap();
    storage.add(&sample("mem-1", "fresh memory needing triples"), "default").unwrap();
    storage.add(&sample("mem-2", "another memory"), "default").unwrap();

    // Both are unenriched (no triples, attempts 0 < max).
    let ids = storage.get_unenriched_memory_ids(10, 3).unwrap();
    assert!(ids.contains(&"mem-1".to_string()));
    assert!(ids.contains(&"mem-2".to_string()));

    // Exhaust mem-1's retries; it must drop out of the unenriched set.
    storage.increment_extraction_attempts("mem-1").unwrap();
    storage.increment_extraction_attempts("mem-1").unwrap();
    storage.increment_extraction_attempts("mem-1").unwrap();
    let ids = storage.get_unenriched_memory_ids(10, 3).unwrap();
    assert!(!ids.contains(&"mem-1".to_string()), "mem-1 at cap must be excluded");
    assert!(ids.contains(&"mem-2".to_string()), "mem-2 still under cap");
}

#[test]
fn legacy_mode_still_uses_memories_column() {
    // Default `Storage::new` is legacy read mode.
    let mut storage = Storage::new(":memory:").unwrap();
    storage.add(&sample("mem-1", "legacy path memory"), "default").unwrap();

    assert_eq!(legacy_attempts(&storage, "mem-1"), 0);
    storage.increment_extraction_attempts("mem-1").unwrap();
    assert_eq!(legacy_attempts(&storage, "mem-1"), 1);

    // Legacy get_unenriched reads the memories column.
    let ids = storage.get_unenriched_memory_ids(10, 3).unwrap();
    assert!(ids.contains(&"mem-1".to_string()));
    storage.increment_extraction_attempts("mem-1").unwrap();
    storage.increment_extraction_attempts("mem-1").unwrap();
    let ids = storage.get_unenriched_memory_ids(10, 3).unwrap();
    assert!(!ids.contains(&"mem-1".to_string()), "mem-1 at cap (3) excluded");
}
