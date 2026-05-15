//! T21 — Phase C backfill driver: entities → nodes(kind=entity).
//!
//! Acceptance per design.md §5.3:
//!
//!   1. Every legacy `entities` row gets a matching
//!      `nodes(node_kind='entity')` row.
//!   2. `entities.entity_type` lands inside `nodes.attributes` JSON
//!      under the `"entity_type"` key.
//!   3. `entities.metadata` (JSON object) keys are merged into
//!      `nodes.attributes`; legacy metadata wins over the synthetic
//!      `entity_type` key (so a metadata edit can override the
//!      type), but loses to any pre-existing T13 dual-write row's
//!      attributes.
//!   4. If a `nodes` row already exists for the same id (T13 path
//!      ran first), Pass 2 merges legacy metadata in **existing-wins**.
//!   5. Idempotent: re-running the driver leaves attributes
//!      unchanged (existing-wins is convergent under repeated
//!      application). **ISS-112 §D**: `updated_at` also stays
//!      byte-identical on idempotent rerun, because Pass-2 now
//!      diffs the merge result against existing attributes and
//!      skips the UPDATE entirely on no-op merges. Regression test:
//!      `iss112_d_idempotent_rerun_does_not_bump_updated_at`.
//!   6. Namespace filter respected.
//!   7. Malformed `metadata` JSON does not fail the row — entity
//!      lands with just `{"entity_type": "..."}`, count surfaced in
//!      audit notes.

use engramai::storage::Storage;
use engramai::substrate::backfill::backfill_entities_to_nodes;
use rusqlite::params;
use tempfile::tempdir;

/// Seed a legacy `entities` row directly via SQL. We bypass any
/// `Storage` API because the goal is to simulate pre-Phase-B legacy
/// data — Phase B's resolution-pipeline path writes a *different*
/// shape (T13).
fn seed_legacy_entity(
    storage: &Storage,
    id: &str,
    name: &str,
    entity_type: &str,
    namespace: &str,
    metadata_json: Option<&str>,
) {
    storage
        .conn()
        .execute(
            r#"INSERT INTO entities (id, name, entity_type, namespace, metadata, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, 1700000000.0, 1700000000.0)"#,
            params![id, name, entity_type, namespace, metadata_json],
        )
        .expect("seed entities row");
}

#[test]
fn t21_backfill_projects_legacy_entity_into_nodes() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_entity(
        &storage,
        "ent-1",
        "Alice",
        "person",
        "default",
        Some(r#"{"alias":"al","note":"founder"}"#),
    );

    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);
    assert_eq!(run.rows_skipped_existing, 0);

    let (kind, content, attrs, ns): (String, String, String, String) = storage
        .conn()
        .query_row(
            "SELECT node_kind, content, attributes, namespace FROM nodes WHERE id = ?",
            params!["ent-1"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(kind, "entity");
    assert_eq!(content, "Alice");
    assert_eq!(ns, "default");

    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["entity_type"], "person");
    assert_eq!(parsed["alias"], "al");
    assert_eq!(parsed["note"], "founder");
}

#[test]
fn t21_column_entity_type_wins_over_metadata() {
    // Per design §5.3: `entities.entity_type` → `nodes.attributes.entity_type`
    // is the FIRST projection; `entities.metadata` keys are merged
    // in afterward with "existing keys win on collision". So if the
    // legacy `entities.metadata` ALSO has an `entity_type` key, the
    // column value (seeded first) MUST win.
    //
    // This test pins that contract. Earlier review (t21-r1.md
    // FINDING-1) caught the implementation reversed.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    seed_legacy_entity(
        &storage,
        "ent-shadow",
        "Bob",
        "person",
        "default",
        Some(r#"{"entity_type":"organization","other":"keep_me"}"#),
    );

    backfill_entities_to_nodes(&mut storage, None).expect("backfill");

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'ent-shadow'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(
        parsed["entity_type"], "person",
        "design §5.3: column entity_type seeded first wins over metadata.entity_type"
    );
    assert_eq!(
        parsed["other"], "keep_me",
        "non-colliding metadata keys still merged in"
    );
}

#[test]
fn t21_existing_nodes_row_wins_on_collision() {
    // Case 2 from the module docs: a T13-style nodes row already
    // exists for this entity id. Backfill Pass 2 merges legacy
    // metadata in, but existing keys must win.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Plant a fake "T13-shaped" nodes row first.
    storage
        .conn()
        .execute(
            r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
               VALUES ('ent-collide', 'entity', 'default', 'Alice Canonical',
                       '{"entity_type":"PERSON","alias":"canonical_al","extra":"t13_value"}',
                       1700000000.0, 1700000000.0,
                       (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
            [],
        )
        .unwrap();
    // Bump the FTS counter manually to reserve a slot.
    storage
        .conn()
        .execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        )
        .unwrap();

    // Now seed a legacy entities row for the same id with
    // overlapping + non-overlapping metadata.
    seed_legacy_entity(
        &storage,
        "ent-collide",
        "Alice Legacy",
        "person", // collision: existing has "PERSON"
        "default",
        Some(r#"{"alias":"legacy_al","new_key":"legacy_value"}"#),
    );

    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(
        run.rows_inserted, 0,
        "Pass 1 INSERT OR IGNORE should be a no-op when nodes row exists"
    );
    assert_eq!(run.rows_skipped_existing, 1);

    // Verify merge semantics.
    let (content, attrs): (String, String) = storage
        .conn()
        .query_row(
            "SELECT content, attributes FROM nodes WHERE id = 'ent-collide'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();

    // content MUST stay the T13-shaped one (we don't overwrite it).
    assert_eq!(content, "Alice Canonical");
    // entity_type: existing "PERSON" wins (not legacy "person").
    assert_eq!(parsed["entity_type"], "PERSON");
    // alias: existing "canonical_al" wins.
    assert_eq!(parsed["alias"], "canonical_al");
    // extra: only existing has it — preserved.
    assert_eq!(parsed["extra"], "t13_value");
    // new_key: only legacy has it — added.
    assert_eq!(parsed["new_key"], "legacy_value");
}

#[test]
fn t21_idempotent_rerun_keeps_attributes_stable() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(
        &storage,
        "ent-idem",
        "Eve",
        "person",
        "default",
        Some(r#"{"k1":"v1"}"#),
    );

    backfill_entities_to_nodes(&mut storage, None).expect("first");
    let attrs1: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='ent-idem'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let r2 = backfill_entities_to_nodes(&mut storage, None).expect("second");
    assert_eq!(r2.rows_inserted, 0);
    // The second run hits Pass 2 (rows already exist), which merges.
    // Merge of (existing) with (legacy projection) yields the same JSON
    // because every key already matched.
    let attrs2: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='ent-idem'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // JSON key order can differ across serde runs; parse + compare.
    let p1: serde_json::Value = serde_json::from_str(&attrs1).unwrap();
    let p2: serde_json::Value = serde_json::from_str(&attrs2).unwrap();
    assert_eq!(p1, p2, "attributes should be content-stable across runs");
}

#[test]
fn t21_malformed_metadata_does_not_fail_row() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(
        &storage,
        "ent-bad",
        "Mallory",
        "person",
        "default",
        Some("not-a-json-object"),
    );

    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_inserted, 1, "malformed metadata must not fail the row");

    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id='ent-bad'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["entity_type"], "person");

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed_notes: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(parsed_notes["rows_malformed_metadata"], 1);
}

#[test]
fn t21_namespace_filter() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-a", "A", "person", "ns-a", None);
    seed_legacy_entity(&storage, "ent-b", "B", "person", "ns-b", None);

    let run = backfill_entities_to_nodes(&mut storage, Some("ns-a")).unwrap();
    assert_eq!(run.rows_read, 1);
    assert_eq!(run.rows_inserted, 1);

    let a_present: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM nodes WHERE id='ent-a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let b_present: i64 = storage
        .conn()
        .query_row("SELECT COUNT(*) FROM nodes WHERE id='ent-b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(a_present, 1);
    assert_eq!(b_present, 0, "ns-b row must not be backfilled");
}

#[test]
fn t21_existing_non_entity_node_is_not_touched() {
    // Defence-in-depth: if a legacy entities.id happens to collide
    // with an existing nodes row whose node_kind is something other
    // than 'entity' (topic, memory, insight), Pass 2 must NOT merge
    // attributes — the legacy projection has no business mutating a
    // different node-kind's attribute shape.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Plant a topic node with the same id we'll seed as an entity.
    storage
        .conn()
        .execute(
            r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
               VALUES ('collide-id', 'topic', 'default', 'Topic Title',
                       '{"cluster_size":5,"topic_specific_key":"abc"}',
                       1700000000.0, 1700000000.0,
                       (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
            [],
        )
        .unwrap();
    storage
        .conn()
        .execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        )
        .unwrap();

    seed_legacy_entity(
        &storage,
        "collide-id",
        "Entity Name",
        "person",
        "default",
        Some(r#"{"alias":"e_alias"}"#),
    );

    backfill_entities_to_nodes(&mut storage, None).expect("backfill");

    let (kind, content, attrs): (String, String, String) = storage
        .conn()
        .query_row(
            "SELECT node_kind, content, attributes FROM nodes WHERE id='collide-id'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "topic", "kind must NOT be rewritten");
    assert_eq!(content, "Topic Title", "content must NOT be overwritten");
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["cluster_size"], 5);
    assert_eq!(parsed["topic_specific_key"], "abc");
    assert!(
        parsed.get("alias").is_none(),
        "legacy entity alias must NOT leak into the topic node's attributes"
    );
    assert!(
        parsed.get("entity_type").is_none(),
        "legacy entity_type must NOT leak into the topic node's attributes"
    );
}

#[test]
fn t21_empty_table_completes_cleanly() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill empty");
    assert_eq!(run.rows_read, 0);
    assert_eq!(run.rows_inserted, 0);
}

#[test]
fn t21_null_metadata_lands_with_entity_type_only() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();
    seed_legacy_entity(&storage, "ent-null", "N", "place", "default", None);

    backfill_entities_to_nodes(&mut storage, None).expect("backfill");

    let attrs: String = storage
        .conn()
        .query_row("SELECT attributes FROM nodes WHERE id='ent-null'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    let obj = parsed.as_object().unwrap();
    assert_eq!(obj.len(), 1, "NULL metadata should leave only entity_type");
    assert_eq!(parsed["entity_type"], "place");
}

// ===================================================================
// ISS-112 §D — idempotent rerun should not bump updated_at on
// no-op Pass-2 merges. This was FINDING-5 in the T21 round-1 review.
// ===================================================================

#[test]
fn iss112_d_idempotent_rerun_does_not_bump_updated_at() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Plant a T13-shaped nodes row that already has the legacy
    // metadata key (existing-wins keeps it). When T21 runs, Pass 2
    // computes a merge that equals existing attributes byte-for-byte
    // → no UPDATE → no updated_at bump.
    storage
        .conn()
        .execute(
            r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
               VALUES ('ent-stable', 'entity', 'default', 'Stable Entity',
                       '{"entity_type":"PERSON","note":"existing_value"}',
                       1700000000.0, 1700000000.0,
                       (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
            [],
        )
        .unwrap();
    storage
        .conn()
        .execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        )
        .unwrap();

    // Legacy entities row with the *same* metadata key — merge is a
    // no-op under existing-wins.
    seed_legacy_entity(
        &storage,
        "ent-stable",
        "Stable Entity",
        "PERSON",
        "default",
        Some(r#"{"note":"existing_value"}"#),
    );

    // Run 1: establishes the (already-correct) post-merge state.
    let run1 = backfill_entities_to_nodes(&mut storage, None).expect("backfill 1");
    assert_eq!(run1.rows_inserted, 0);
    assert_eq!(run1.rows_skipped_existing, 1);

    // Snapshot updated_at after run 1.
    let updated_at_after_run1: f64 = storage
        .conn()
        .query_row(
            "SELECT updated_at FROM nodes WHERE id = 'ent-stable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // The T13 seed timestamp was 1700000000.0 — unchanged because the
    // merge is a no-op.
    assert!(
        (updated_at_after_run1 - 1700000000.0).abs() < 1e-6,
        "first run's merge should be byte-identical, no UPDATE: got {}",
        updated_at_after_run1
    );

    // Sleep a tick so wall-clock advances; if a buggy implementation
    // did UPDATE, updated_at would change to "now" which is > seed.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Run 2: should also be a no-op.
    let run2 = backfill_entities_to_nodes(&mut storage, None).expect("backfill 2");
    assert_eq!(run2.rows_inserted, 0);
    assert_eq!(run2.rows_skipped_existing, 1);

    let updated_at_after_run2: f64 = storage
        .conn()
        .query_row(
            "SELECT updated_at FROM nodes WHERE id = 'ent-stable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        updated_at_after_run1, updated_at_after_run2,
        "second idempotent rerun must not bump updated_at"
    );

    // Sanity: attributes content also unchanged.
    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'ent-stable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["entity_type"], "PERSON");
    assert_eq!(parsed["note"], "existing_value");
}

#[test]
fn iss112_d_actual_merge_does_bump_updated_at() {
    // Counter-test: when the merge IS a real change, updated_at
    // should bump. Confirms the diff-and-skip doesn't suppress
    // legitimate updates.
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    storage
        .conn()
        .execute(
            r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
               VALUES ('ent-newkey', 'entity', 'default', 'New-Key Entity',
                       '{"entity_type":"PERSON"}',
                       1700000000.0, 1700000000.0,
                       (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
            [],
        )
        .unwrap();
    storage
        .conn()
        .execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        )
        .unwrap();

    // Legacy adds a new key not in existing — merge SHOULD change attributes.
    seed_legacy_entity(
        &storage,
        "ent-newkey",
        "New-Key Entity",
        "PERSON",
        "default",
        Some(r#"{"new_key":"freshly_added"}"#),
    );

    let _run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    let updated_at_after: f64 = storage
        .conn()
        .query_row(
            "SELECT updated_at FROM nodes WHERE id = 'ent-newkey'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        updated_at_after > 1700000000.0,
        "real merge must bump updated_at past seed; got {}",
        updated_at_after
    );

    // Sanity: the new key is in there.
    let attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'ent-newkey'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert_eq!(parsed["new_key"], "freshly_added");
}

// ===================================================================
// ISS-112 §E — counter naming: rows_skipped_kind_mismatch (was
// rows_kind_mismatch). Subset signaling.
// ===================================================================

#[test]
fn iss112_e_kind_mismatch_emits_under_skipped_prefix() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Plant a foreign-kind nodes row (topic) that collides with our
    // legacy entity id — Pass 2 should skip and count as kind_mismatch.
    storage
        .conn()
        .execute(
            r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
               VALUES ('ent-foreign', 'topic', 'default', 'A topic',
                       '{"some":"topic_attr"}', 1700000000.0, 1700000000.0,
                       (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
            [],
        )
        .unwrap();
    storage
        .conn()
        .execute(
            "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
            [],
        )
        .unwrap();

    seed_legacy_entity(
        &storage,
        "ent-foreign",
        "Same id, different kind",
        "PERSON",
        "default",
        None,
    );

    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 1);

    // Inspect audit notes JSON.
    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&notes).unwrap();
    // ISS-112 §E: counter renamed.
    assert_eq!(parsed["rows_skipped_kind_mismatch"], 1);
    // And: the foreign topic row was not mutated.
    let foreign_attrs: String = storage
        .conn()
        .query_row(
            "SELECT attributes FROM nodes WHERE id = 'ent-foreign'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(foreign_attrs, r#"{"some":"topic_attr"}"#);
}

// ===================================================================
// ISS-112 §C — when existing nodes.attributes is corrupt
// ('null', '[]', scalar), legacy keys are silently dropped (defensive)
// but the counter surfaces the drop.
// ===================================================================

#[test]
fn iss112_c_corrupt_existing_attributes_surfaced_in_counter() {
    let tmp = tempdir().unwrap();
    let mut storage = Storage::new(tmp.path().join("engram.db")).unwrap();

    // Plant THREE T13-shaped nodes rows with progressively corrupt
    // attributes payloads. Pass 2 should refuse to merge each, count
    // each in `rows_existing_attrs_not_object`, and leave the
    // existing (corrupt) attributes alone.
    for (id, bad_attrs) in [
        ("ent-null", "null"),
        ("ent-array", "[]"),
        ("ent-scalar", r#""string""#),
    ] {
        storage
            .conn()
            .execute(
                r#"INSERT INTO nodes (id, node_kind, namespace, content, attributes, created_at, updated_at, fts_rowid)
                   VALUES (?, 'entity', 'default', 'corrupt-existing',
                           ?, 1700000000.0, 1700000000.0,
                           (SELECT next_value-1 FROM fts_rowid_counter WHERE singleton=0))"#,
                params![id, bad_attrs],
            )
            .unwrap();
        storage
            .conn()
            .execute(
                "UPDATE fts_rowid_counter SET next_value = next_value + 1 WHERE singleton = 0",
                [],
            )
            .unwrap();

        // Use unique (name, entity_type, namespace) per row to
        // satisfy the legacy table's unique index. We're testing
        // corrupt EXISTING `nodes` rows, the legacy seed data is
        // just a vehicle to reach Pass 2.
        seed_legacy_entity(
            &storage,
            id,
            &format!("Name-for-{id}"),
            "PERSON",
            "default",
            Some(r#"{"some":"legacy_key"}"#),
        );
    }

    let run = backfill_entities_to_nodes(&mut storage, None).expect("backfill");
    assert_eq!(run.rows_inserted, 0);
    assert_eq!(run.rows_skipped_existing, 3);

    let notes: String = storage
        .conn()
        .query_row(
            "SELECT notes FROM backfill_runs WHERE run_id = ?",
            params![&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&notes).unwrap();
    assert_eq!(
        parsed["rows_existing_attrs_not_object"], 3,
        "all three corrupt-existing rows should be counted"
    );
    // Defensive guarantee: existing corrupt attributes were not
    // overwritten.
    for (id, expected) in [
        ("ent-null", "null"),
        ("ent-array", "[]"),
        ("ent-scalar", r#""string""#),
    ] {
        let actual: String = storage
            .conn()
            .query_row(
                "SELECT attributes FROM nodes WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actual, expected, "row {id} attributes mutated unexpectedly");
    }
}
