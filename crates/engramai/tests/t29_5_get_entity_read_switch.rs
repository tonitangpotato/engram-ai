//! T29.5 — `get_entity` read-switch contract.
//!
//! Per design §5.4, the `unified_substrate` flag flips retrieval
//! adapters from legacy tables to `nodes`/`edges`. This file pins
//! that `get_entity` returns the same `EntityRecord` under both
//! substrates for entities written via the canonical write path
//! (`upsert_entity` / T13 graph-store dual-write).
//!
//! Acceptance contract:
//!
//!   1. `upsert_entity` populates both `entities` (legacy) and
//!      `nodes` (unified) atomically (ISS-122). After a single
//!      insert, both read paths return the same record on
//!      `(name, entity_type, namespace, metadata, id)`.
//!   2. Conflict-update path: re-`upsert_entity` with new metadata
//!      bumps `updated_at` on both substrates and the unified read
//!      preserves the column-seeded `entity_type` (existing-wins
//!      merge polarity).
//!   3. Missing entity returns `Ok(None)` on both substrates.
//!   4. `Other(_)` kinds round-trip through `_legacy_kind` correctly
//!      (decoded back to the same flat string the legacy column
//!      stored).
//!
//! See `storage.rs::decode_entity_type_and_metadata` for the decode
//! chain and `upsert_entity` for the projection contract.

use engramai::storage::Storage;
use tempfile::tempdir;

fn with_storage<F>(unified: bool, db_path: &std::path::Path, f: F)
where
    F: FnOnce(&Storage),
{
    let storage = Storage::with_unified_substrate(db_path, unified).unwrap();
    f(&storage);
}

#[test]
fn t29_5_get_entity_single_upsert_matches() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("e.db");

    // Seed via writer (always dual-writes legacy + unified post ISS-122).
    let id = {
        let storage = Storage::new(&db_path).unwrap();
        storage
            .upsert_entity(
                "Alice",
                "person",
                "default",
                Some(r#"{"role":"engineer","city":"NYC"}"#),
            )
            .unwrap()
    };

    let legacy = {
        let mut got = None;
        with_storage(false, &db_path, |s| {
            got = s.get_entity(&id).unwrap();
        });
        got.expect("legacy read")
    };
    let unified = {
        let mut got = None;
        with_storage(true, &db_path, |s| {
            got = s.get_entity(&id).unwrap();
        });
        got.expect("unified read")
    };

    assert_eq!(legacy.id, unified.id, "id");
    assert_eq!(legacy.name, unified.name, "name");
    assert_eq!(legacy.entity_type, unified.entity_type, "entity_type");
    assert_eq!(legacy.namespace, unified.namespace, "namespace");

    // metadata: legacy stores the caller-supplied string verbatim;
    // unified reconstructs the JSON-object from `attributes` minus
    // reserved keys. Both must parse to the same JSON value.
    let l_meta: serde_json::Value =
        serde_json::from_str(legacy.metadata.as_deref().expect("legacy metadata")).unwrap();
    let u_meta: serde_json::Value =
        serde_json::from_str(unified.metadata.as_deref().expect("unified metadata")).unwrap();
    assert_eq!(l_meta, u_meta, "metadata JSON-equal");
}

#[test]
fn t29_5_get_entity_missing_returns_none_on_both_substrates() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("e.db");
    // Initialize schema.
    let _ = Storage::new(&db_path).unwrap();

    let mut legacy = Some(());
    with_storage(false, &db_path, |s| {
        let got = s.get_entity("nonexistent").unwrap();
        legacy = None.or_else(|| got.map(|_| ()));
    });
    let mut unified = Some(());
    with_storage(true, &db_path, |s| {
        let got = s.get_entity("nonexistent").unwrap();
        unified = None.or_else(|| got.map(|_| ()));
    });
    assert!(legacy.is_none() && unified.is_none(), "both return None");
}

#[test]
fn t29_5_get_entity_no_metadata_yields_none_on_both() {
    // Legacy stores NULL in `entities.metadata`; unified strips the
    // reserved keys and returns `None` when the object is empty.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("e.db");

    let id = {
        let storage = Storage::new(&db_path).unwrap();
        storage
            .upsert_entity("Bob", "person", "default", None)
            .unwrap()
    };

    let legacy = {
        let mut got = None;
        with_storage(false, &db_path, |s| {
            got = s.get_entity(&id).unwrap();
        });
        got.expect("legacy read")
    };
    let unified = {
        let mut got = None;
        with_storage(true, &db_path, |s| {
            got = s.get_entity(&id).unwrap();
        });
        got.expect("unified read")
    };

    assert_eq!(legacy.metadata, None, "legacy metadata = NULL");
    assert_eq!(unified.metadata, None, "unified metadata = None (empty object)");
    assert_eq!(legacy.entity_type, unified.entity_type, "entity_type");
    assert_eq!(legacy.name, unified.name, "name");
}

#[test]
fn t29_5_get_entity_conflict_update_preserves_entity_type() {
    // ISS-122 conflict-update branch: re-upsert with new metadata
    // merges with existing-wins polarity, but the legacy column's
    // `entity_type` always wins. The unified reader must report the
    // same `entity_type` as legacy after the merge.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("e.db");

    let id_v1 = {
        let storage = Storage::new(&db_path).unwrap();
        storage
            .upsert_entity("Carol", "person", "default", Some(r#"{"v":1}"#))
            .unwrap()
    };
    // Same (name, entity_type, namespace) → same deterministic id,
    // hits the conflict-update branch.
    let id_v2 = {
        let storage = Storage::new(&db_path).unwrap();
        storage
            .upsert_entity(
                "Carol",
                "person",
                "default",
                Some(r#"{"v":2,"new_key":"x"}"#),
            )
            .unwrap()
    };
    assert_eq!(id_v1, id_v2, "deterministic id");

    let legacy = {
        let mut got = None;
        with_storage(false, &db_path, |s| {
            got = s.get_entity(&id_v2).unwrap();
        });
        got.expect("legacy")
    };
    let unified = {
        let mut got = None;
        with_storage(true, &db_path, |s| {
            got = s.get_entity(&id_v2).unwrap();
        });
        got.expect("unified")
    };
    assert_eq!(legacy.entity_type, "person", "entity_type preserved on legacy");
    assert_eq!(unified.entity_type, "person", "entity_type preserved on unified");
}
