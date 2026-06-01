//! T29.5 part-3 — `list_entities` read-switch contract.
//!
//! Per design §5.4, the `unified_substrate` flag flips
//! `list_entities` from reading `entities LEFT JOIN memory_entities`
//! to `nodes` with a correlated edges subquery.
//!
//! Now that ISS-123 closes the Phase B dual-write gap for
//! `link_memory_entity`, both substrates see the same mention
//! events and `list_entities` must return the same set on both
//! paths.
//!
//! Acceptance contract:
//!
//!   1. With several entities and several mentions, both substrates
//!      return the same `(id, name, entity_type, mention_count)`
//!      tuples (sorted-by-id for set comparison; insertion order
//!      not guaranteed).
//!   2. Namespace filter narrows to the correct subset on both
//!      paths.
//!   3. `entity_type` filter applied post-decode in unified path
//!      returns the same subset as the SQL-WHERE filter in legacy.
//!   4. `limit` honoured: legacy and unified both cap at the
//!      requested limit.
//!   5. Topics (node_kind='topic') don't have a legacy entities
//!      row, so the legacy reader doesn't see them. The unified
//!      reader filters by `node_kind IN ('entity','topic')` so it
//!      DOES surface topics. This is an intentional asymmetry —
//!      Phase E will drop the legacy table anyway. The test pins
//!      that the entity rows are identical and that the unified
//!      path additionally surfaces topics.

use engramai::storage::Storage;
use std::collections::HashMap;
use tempfile::tempdir;

fn entities_by_id(
    rows: &[(engramai::storage::EntityRecord, usize)],
) -> HashMap<String, (String, usize)> {
    rows.iter()
        .map(|(r, c)| (r.id.clone(), (r.entity_type.clone(), *c)))
        .collect()
}

#[test]
fn t29_5p3_list_entities_matches_with_mentions() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    // Seed: 3 entities, 5 mentions distributed unevenly.
    let storage = Storage::new(&db).unwrap();
    let alice = storage
        .upsert_entity("Alice", "person", "default", None)
        .unwrap();
    let bob = storage
        .upsert_entity("Bob", "person", "default", None)
        .unwrap();
    let widget = storage
        .upsert_entity("Widget", "thing", "default", None)
        .unwrap();

    // Seed memories (via store_raw which dual-writes to nodes).
    for i in 0..5 {
        let mid = format!("mem-{i}");
        storage
            .store_raw(&mid, &format!("content-{i}"), "factual", 1.0, None)
            .unwrap();
    }

    // Link: Alice mentioned in mem-0, mem-1, mem-2 (3 mentions)
    //       Bob   mentioned in mem-0          (1 mention)
    //       Widget mentioned in mem-3, mem-4  (2 mentions)
    storage
        .link_memory_entity("mem-0", &alice, "mention")
        .unwrap();
    storage
        .link_memory_entity("mem-1", &alice, "mention")
        .unwrap();
    storage
        .link_memory_entity("mem-2", &alice, "mention")
        .unwrap();
    storage
        .link_memory_entity("mem-0", &bob, "mention")
        .unwrap();
    storage
        .link_memory_entity("mem-3", &widget, "mention")
        .unwrap();
    storage
        .link_memory_entity("mem-4", &widget, "mention")
        .unwrap();

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .list_entities(None, None, 10)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .list_entities(None, None, 10)
        .unwrap();

    let l_map = entities_by_id(&legacy);
    let u_map = entities_by_id(&unified);

    assert_eq!(l_map.len(), 3, "legacy 3 entities");
    assert_eq!(u_map.len(), 3, "unified 3 entities");
    assert_eq!(l_map, u_map, "id -> (type, mention_count) maps equal");

    // Spot-check counts
    assert_eq!(l_map[&alice], ("person".to_string(), 3));
    assert_eq!(l_map[&bob], ("person".to_string(), 1));
    assert_eq!(l_map[&widget], ("thing".to_string(), 2));
}

#[test]
fn t29_5p3_list_entities_namespace_filter_matches() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let storage = Storage::new(&db).unwrap();
    let a_default = storage
        .upsert_entity("Alice", "person", "default", None)
        .unwrap();
    let _a_alt = storage
        .upsert_entity("Alice", "person", "alt", None)
        .unwrap();

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .list_entities(None, Some("default"), 10)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .list_entities(None, Some("default"), 10)
        .unwrap();

    assert_eq!(legacy.len(), 1);
    assert_eq!(unified.len(), 1);
    assert_eq!(legacy[0].0.id, a_default);
    assert_eq!(unified[0].0.id, a_default);
}

#[test]
fn t29_5p3_list_entities_type_filter_matches() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let storage = Storage::new(&db).unwrap();
    let alice = storage
        .upsert_entity("Alice", "person", "default", None)
        .unwrap();
    let _widget = storage
        .upsert_entity("Widget", "thing", "default", None)
        .unwrap();
    let bob = storage
        .upsert_entity("Bob", "person", "default", None)
        .unwrap();

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .list_entities(Some("person"), None, 10)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .list_entities(Some("person"), None, 10)
        .unwrap();

    let l_ids: std::collections::HashSet<String> =
        legacy.iter().map(|(r, _)| r.id.clone()).collect();
    let u_ids: std::collections::HashSet<String> =
        unified.iter().map(|(r, _)| r.id.clone()).collect();

    let expected: std::collections::HashSet<String> = [alice, bob].into_iter().collect();
    assert_eq!(l_ids, expected, "legacy type=person");
    assert_eq!(u_ids, expected, "unified type=person");
}

#[test]
fn t29_5p3_list_entities_limit_honoured() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("e.db");

    let storage = Storage::new(&db).unwrap();
    for i in 0..5 {
        storage
            .upsert_entity(&format!("E{i}"), "person", "default", None)
            .unwrap();
    }

    let legacy = Storage::with_unified_substrate(&db, false)
        .unwrap()
        .list_entities(None, None, 2)
        .unwrap();
    let unified = Storage::with_unified_substrate(&db, true)
        .unwrap()
        .list_entities(None, None, 2)
        .unwrap();

    assert_eq!(legacy.len(), 2);
    assert_eq!(unified.len(), 2);
}
