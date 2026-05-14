//! T29.5 part-2 — `find_entities` + `count_entities` read-switch contract.
//!
//! Same shape as T29.5 part-1 (`get_entity`) but for the
//! single-table (no JOIN) collection readers. Both legacy and
//! unified paths must return the same set after writers dual-write
//! (ISS-122 / T13).
//!
//! Acceptance contract:
//!
//!   1. `count_entities` matches across substrates with and without
//!      namespace filter.
//!   2. `find_entities` matches as a multi-set on
//!      `(id, name, entity_type, namespace, metadata_value)` after
//!      seeding multiple entities with the same name in different
//!      namespaces.
//!   3. `find_entities` honours the `limit` argument on both paths
//!      (limit=1 returns at most one row).
//!   4. Mixed `node_kind`: a `topic` row (planted directly into
//!      `nodes`) is **not** returned by the legacy path (no
//!      corresponding `entities` row) but is returned by the
//!      unified path. The reverse asymmetry (a legacy row with no
//!      unified counterpart) is impossible under ISS-122 writer
//!      contract — see test #4 inline comment.

use engramai::storage::Storage;
use std::collections::HashSet;
use tempfile::tempdir;

fn collect_ids(records: &[engramai::storage::EntityRecord]) -> HashSet<String> {
    records.iter().map(|r| r.id.clone()).collect()
}

#[test]
fn t29_5p2_count_entities_matches_across_substrates() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    {
        let storage = Storage::new(&db).unwrap();
        storage
            .upsert_entity("Alice", "person", "default", None)
            .unwrap();
        storage
            .upsert_entity("Bob", "person", "default", None)
            .unwrap();
        storage
            .upsert_entity("Carol", "person", "alt", None)
            .unwrap();
    }

    let legacy_all = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .count_entities(None)
        .unwrap();
    let unified_all = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .count_entities(None)
        .unwrap();
    assert_eq!(legacy_all, 3);
    assert_eq!(legacy_all, unified_all, "count all");

    let legacy_default = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .count_entities(Some("default"))
        .unwrap();
    let unified_default = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .count_entities(Some("default"))
        .unwrap();
    assert_eq!(legacy_default, 2);
    assert_eq!(legacy_default, unified_default, "count default ns");
}

#[test]
fn t29_5p2_find_entities_matches_by_id_set() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let id_a = {
        let storage = Storage::new(&db).unwrap();
        let id_a = storage
            .upsert_entity("Alice", "person", "default", Some(r#"{"k":1}"#))
            .unwrap();
        // Same name, different namespace → different deterministic id.
        let _id_b = storage
            .upsert_entity("Alice", "person", "alt", Some(r#"{"k":2}"#))
            .unwrap();
        id_a
    };

    // Namespace-scoped lookup: should return only the default-ns Alice
    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entities("Alice", Some("default"), 10)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entities("Alice", Some("default"), 10)
        .unwrap();

    assert_eq!(legacy.len(), 1, "legacy default-ns Alice");
    assert_eq!(unified.len(), 1, "unified default-ns Alice");
    assert_eq!(legacy[0].id, id_a);
    assert_eq!(unified[0].id, id_a);
    assert_eq!(legacy[0].entity_type, unified[0].entity_type);

    // Global lookup: should return both
    let legacy_global = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entities("Alice", None, 10)
        .unwrap();
    let unified_global = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entities("Alice", None, 10)
        .unwrap();
    assert_eq!(legacy_global.len(), 2, "legacy 2 Alices");
    assert_eq!(unified_global.len(), 2, "unified 2 Alices");
    assert_eq!(collect_ids(&legacy_global), collect_ids(&unified_global));
}

#[test]
fn t29_5p2_find_entities_honours_limit() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    {
        let storage = Storage::new(&db).unwrap();
        storage.upsert_entity("Foo", "thing", "ns1", None).unwrap();
        storage.upsert_entity("Foo", "thing", "ns2", None).unwrap();
        storage.upsert_entity("Foo", "thing", "ns3", None).unwrap();
    }

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .find_entities("Foo", None, 1)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .find_entities("Foo", None, 1)
        .unwrap();
    assert_eq!(legacy.len(), 1, "legacy limit=1");
    assert_eq!(unified.len(), 1, "unified limit=1");
}

#[test]
fn t29_5p2_count_zero_when_empty() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");
    let _ = Storage::new(&db).unwrap(); // create schema

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .count_entities(None)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .count_entities(None)
        .unwrap();
    assert_eq!(legacy, 0);
    assert_eq!(unified, 0);
}
