//! ISS-199 (Phase E read-cutover) — `find_entity_overlap` read-switch
//! contract.
//!
//! `find_entity_overlap` filters candidate memories by namespace and
//! soft-delete. Before this cutover the filter JOINed the legacy
//! `memories` table. Under `unified_substrate = true` it JOINs `nodes`
//! (`node_kind = 'memory'`) instead, because T34a removes the
//! `memories` write under unified mode and the `nodes` row is the only
//! one guaranteed present (via T12 dual-write).
//!
//! The `memory_entities ⋈ entities` half of the join is unchanged —
//! those tables key on `memory_id` which equals `nodes.id` for memory
//! nodes — so the overlap/Jaccard math is identical on both substrates.
//!
//! Acceptance contract:
//!
//!   1. Empty input: both substrates return `None`.
//!   2. Full overlap: both return `Some((id, jaccard))` with the same
//!      id and jaccard for a memory whose entity set equals the query.
//!   3. No overlap: both return `None`.
//!   4. Namespace isolation: a query in namespace "default" does not
//!      match a memory whose node lives in namespace "alt", on the
//!      unified substrate (proves the JOIN reads `nodes.namespace`).
//!   5. Soft-delete exclusion: a soft-deleted memory (its `nodes`
//!      row carries `deleted_at`) is excluded on the unified substrate
//!      (proves the JOIN honours `nodes.deleted_at IS NULL`).

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use tempfile::tempdir;

fn rec(id: &str, content: &str) -> MemoryRecord {
    let created = Utc.with_ymd_and_hms(2026, 5, 31, 4, 30, 0).unwrap();
    MemoryRecord {
        id: id.into(),
        content: content.into(),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: created,
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
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

/// Seed one memory (real `node_kind='memory'` dual-write via
/// `Storage::add`) with two linked entities, in the given namespace.
fn seed_memory_with_entities(storage: &mut Storage, mem_id: &str, ns: &str) {
    storage
        .add(&rec(mem_id, "John works at Google"), ns)
        .expect("add memory");
    let e1 = storage.upsert_entity("john", "person", ns, None).unwrap();
    let e2 = storage
        .upsert_entity("google", "organization", ns, None)
        .unwrap();
    storage.link_memory_entity(mem_id, &e1, "mention").unwrap();
    storage.link_memory_entity(mem_id, &e2, "mention").unwrap();
}

#[test]
fn iss199_empty_input_returns_none_on_both() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let mut storage = Storage::new(&db).unwrap();
    seed_memory_with_entities(&mut storage, "mem-1", "default");

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entity_overlap(&[], "default", 0.5)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&[], "default", 0.5)
        .unwrap();

    assert!(legacy.is_none());
    assert!(unified.is_none());
}

#[test]
fn iss199_full_overlap_matches_on_both() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let mut storage = Storage::new(&db).unwrap();
    seed_memory_with_entities(&mut storage, "mem-1", "default");

    let query = vec!["john".to_string(), "google".to_string()];

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();

    let (lid, ljac) = legacy.expect("legacy should match");
    let (uid, ujac) = unified.expect("unified should match");

    assert_eq!(lid, "mem-1");
    assert_eq!(uid, "mem-1");
    // Perfect overlap: |{john,google} ∩ {john,google}| / |union| = 2/2 = 1.0
    assert!((ljac - 1.0).abs() < 1e-9, "legacy jaccard = {ljac}");
    assert!((ujac - 1.0).abs() < 1e-9, "unified jaccard = {ujac}");
    assert!((ljac - ujac).abs() < 1e-9, "substrates must agree on jaccard");
}

#[test]
fn iss199_no_overlap_returns_none_on_both() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let mut storage = Storage::new(&db).unwrap();
    seed_memory_with_entities(&mut storage, "mem-1", "default");

    let query = vec!["nonexistent_entity".to_string()];

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();

    assert!(legacy.is_none());
    assert!(unified.is_none());
}

#[test]
fn iss199_namespace_isolation_on_unified() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let mut storage = Storage::new(&db).unwrap();
    // Memory + entities live entirely in the "alt" namespace.
    seed_memory_with_entities(&mut storage, "mem-alt", "alt");

    let query = vec!["john".to_string(), "google".to_string()];

    // Querying the "default" namespace must NOT see the alt-ns memory.
    let unified_default = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();
    assert!(
        unified_default.is_none(),
        "alt-ns memory must not match a default-ns query"
    );

    // Sanity: querying the correct namespace DOES match on unified.
    let unified_alt = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&query, "alt", 0.5)
        .unwrap();
    let (uid, _) = unified_alt.expect("alt-ns query should match alt-ns memory");
    assert_eq!(uid, "mem-alt");
}

#[test]
fn iss199_soft_deleted_excluded_on_unified() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let mut storage = Storage::new(&db).unwrap();
    seed_memory_with_entities(&mut storage, "mem-1", "default");

    // Soft-delete dual-writes nodes.deleted_at (epoch) under ISS-121.
    storage.soft_delete("mem-1").unwrap();

    let query = vec!["john".to_string(), "google".to_string()];

    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();
    assert!(
        unified.is_none(),
        "soft-deleted memory (nodes.deleted_at set) must be excluded on unified"
    );

    // Legacy substrate must agree (memories.deleted_at also set).
    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entity_overlap(&query, "default", 0.5)
        .unwrap();
    assert!(
        legacy.is_none(),
        "soft-deleted memory must be excluded on legacy too"
    );
}
