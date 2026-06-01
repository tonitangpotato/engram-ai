//! ISS-117 — single canonical row contract for `hebbian_links`.
//!
//! After ISS-117, every formed hebbian link is stored as ONE row in
//! `hebbian_links` keyed by `(min(source,target), max(source,target))`.
//! Readers OR-match on `(source = ? OR target = ?)` and the writer
//! never emits a reverse-direction row. This makes the legacy row
//! shape match the unified `edges` row shape — a prerequisite for
//! T29.4's Phase D read-switch.
//!
//! Acceptance contract:
//!
//!   1. After `record_coactivation` fires the threshold-form branch
//!      enough times to form a link, `hebbian_links` contains exactly
//!      ONE row for the pair (not two).
//!   2. The row's source/target order is canonical (min, max), even
//!      when the caller passes the ids in the opposite order.
//!   3. `get_hebbian_neighbors(a)` and `get_hebbian_neighbors(b)`
//!      both return the other endpoint (OR-match works).
//!   4. `get_hebbian_links_weighted` returns exactly ONE entry per
//!      neighbour — no duplicates from reverse rows.
//!   5. `record_coactivation_ns` and `record_cross_namespace_coactivation`
//!      follow the same single-canonical-row contract.
//!   6. `migrate_hebbian_canonical_rows` collapses pre-existing
//!      double-direction rows into one canonical row, merging
//!      strength (max), coactivation_count (sum), temporal counters
//!      (sum), created_at (min).
//!   7. The migration is idempotent — running it twice on an
//!      already-canonical table is a no-op.

use chrono::{TimeZone, Utc};
use engramai::storage::Storage;
use engramai::types::{MemoryLayer, MemoryRecord, MemoryType};
use rusqlite::params;
use tempfile::tempdir;

/// Helper: insert a minimal memory row so FK-bearing dual-writes
/// don't roll back. Uses the canonical Storage::add path so all
/// substrate writes happen in lockstep.
fn seed_memory(storage: &mut Storage, id: &str) {
    let rec = MemoryRecord {
        id: id.into(),
        content: format!("content-{id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss117-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, "default").expect("seed memory");
}

/// Like `seed_memory`, but stores the memory in a specified namespace.
/// Used by ISS-118 cross-axis regression tests where we need to put
/// endpoints into namespaces that don't agree with their id ordering.
fn seed_memory_ns(storage: &mut Storage, id: &str, ns: &str) {
    let rec = MemoryRecord {
        id: id.into(),
        content: format!("content-{id}"),
        memory_type: MemoryType::Factual,
        layer: MemoryLayer::Core,
        created_at: Utc.with_ymd_and_hms(2026, 5, 13, 11, 0, 0).unwrap(),
        occurred_at: None,
        access_times: vec![],
        working_strength: 0.4,
        core_strength: 0.9,
        importance: 0.7,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "iss117-test".into(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: None,
    };
    storage.add(&rec, ns).expect("seed memory in ns");
}

/// Helper: insert a raw `hebbian_links` row directly. Used by the
/// migration tests to simulate pre-ISS-117 legacy databases that
/// contain double-direction rows.
fn insert_raw_link(
    storage: &Storage,
    source: &str,
    target: &str,
    strength: f64,
    count: i32,
    created_at: f64,
) {
    storage
        .connection()
        .execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES (?1, ?2, ?3, ?4, 0, 0, 'bidirectional', ?5, 'default')",
            params![source, target, strength, count, created_at],
        )
        .expect("insert raw link");
}

fn count_links_for_pair(storage: &Storage, a: &str, b: &str) -> i64 {
    storage
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM hebbian_links \
             WHERE (source_id = ?1 AND target_id = ?2) \
                OR (source_id = ?2 AND target_id = ?1)",
            params![a, b],
            |row| row.get(0),
        )
        .expect("count links")
}

// ---------------------------------------------------------------
// Single-canonical-row writer contract
// ---------------------------------------------------------------

#[test]
fn iss117_record_coactivation_forms_single_canonical_row() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "alpha");
    seed_memory(&mut storage, "beta");

    // Threshold defaults to 3 in the engine, but record_coactivation
    // takes it as a parameter. Drive 3 invocations to cross the
    // formed-link branch.
    for _ in 0..3 {
        storage
            .record_coactivation("alpha", "beta", 3)
            .expect("record_coactivation");
    }

    // Strengthen once more (formed branch) to exercise that path too.
    storage
        .record_coactivation("alpha", "beta", 3)
        .expect("record_coactivation post-form");

    // Exactly one row for the pair.
    let n = count_links_for_pair(&storage, "alpha", "beta");
    assert_eq!(
        n, 1,
        "ISS-117: expected one canonical row, found {n} (reverse-row regression?)"
    );
}

#[test]
fn iss117_record_coactivation_canonicalizes_id_order() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "zzz_high");
    seed_memory(&mut storage, "aaa_low");

    // Caller passes high, low. The writer should still store
    // (low, high) canonical order.
    for _ in 0..3 {
        storage
            .record_coactivation("zzz_high", "aaa_low", 3)
            .unwrap();
    }

    let (s, t): (String, String) = storage
        .connection()
        .query_row(
            "SELECT source_id, target_id FROM hebbian_links \
             WHERE (source_id = ?1 AND target_id = ?2) \
                OR (source_id = ?2 AND target_id = ?1)",
            params!["aaa_low", "zzz_high"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(s, "aaa_low", "canonical source should be min");
    assert_eq!(t, "zzz_high", "canonical target should be max");
}

#[test]
fn iss117_get_neighbors_works_in_either_direction() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "a");
    seed_memory(&mut storage, "b");

    for _ in 0..3 {
        storage.record_coactivation("a", "b", 3).unwrap();
    }

    let neighbors_of_a = storage.get_hebbian_neighbors("a").unwrap();
    let neighbors_of_b = storage.get_hebbian_neighbors("b").unwrap();

    assert_eq!(neighbors_of_a, vec!["b".to_string()]);
    assert_eq!(neighbors_of_b, vec!["a".to_string()]);
}

#[test]
fn iss117_get_hebbian_links_weighted_no_duplicates() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "a");
    seed_memory(&mut storage, "b");
    seed_memory(&mut storage, "c");

    for _ in 0..3 {
        storage.record_coactivation("a", "b", 3).unwrap();
        storage.record_coactivation("a", "c", 3).unwrap();
    }

    let links = storage.get_hebbian_links_weighted("a").unwrap();
    assert_eq!(
        links.len(),
        2,
        "expected 2 distinct neighbours, got {} (duplicate rows?)",
        links.len()
    );

    let mut targets: Vec<String> = links.into_iter().map(|(t, _)| t).collect();
    targets.sort();
    assert_eq!(targets, vec!["b".to_string(), "c".to_string()]);
}

#[test]
fn iss117_record_coactivation_ns_forms_single_canonical_row() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "a");
    seed_memory(&mut storage, "b");

    for _ in 0..3 {
        storage
            .record_coactivation_ns("a", "b", 3, "tenant1")
            .unwrap();
    }

    let n = count_links_for_pair(&storage, "a", "b");
    assert_eq!(n, 1, "namespaced writer should produce single row");
}

#[test]
fn iss117_record_cross_namespace_coactivation_forms_single_canonical_row() {
    let dir = tempdir().unwrap();
    let mut storage = Storage::new(dir.path().join("h.db")).unwrap();

    seed_memory(&mut storage, "x_in_ns1");
    seed_memory(&mut storage, "y_in_ns2");

    for _ in 0..3 {
        storage
            .record_cross_namespace_coactivation("x_in_ns1", "ns1", "y_in_ns2", "ns2", 3)
            .unwrap();
    }

    let n = count_links_for_pair(&storage, "x_in_ns1", "y_in_ns2");
    assert_eq!(n, 1, "cross-NS writer should produce single row");
}

// ---------------------------------------------------------------
// Migration: collapse pre-existing double-direction rows
// ---------------------------------------------------------------

#[test]
fn iss117_migration_collapses_double_direction_rows() {
    // Seed a database with double-direction rows directly via SQL,
    // simulating a pre-ISS-117 production database. Storage::new
    // runs the migration on open, so we open twice: once to seed
    // (which already runs migrations — harmless on empty), then
    // close, reopen via raw rusqlite to insert dupes bypassing the
    // canonical writer, then reopen via Storage::new to trigger
    // migration.

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    // Open once to bring schema up + seed memories.
    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
    }

    // Now inject reverse rows directly. Using raw rusqlite so we
    // bypass record_coactivation and force the legacy double-row
    // shape onto disk. We must also bypass the migration that ran on
    // Storage::new — so insert AFTER reopening with a raw connection.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        // Canonical row: strength=1.0, count=5, created=1000.
        conn.execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES ('a', 'b', 1.0, 5, 2, 1, 'forward', 1000.0, 'default')",
            [],
        )
        .unwrap();
        // Reverse row: strength=0.8, count=3, created=2000.
        conn.execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES ('b', 'a', 0.8, 3, 0, 0, 'backward', 2000.0, 'default')",
            [],
        )
        .unwrap();
    }

    // Reopen via Storage::new — the migration should run and collapse.
    let storage = Storage::new(&db_path).unwrap();

    // Exactly one row remains.
    let n: i64 = storage
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM hebbian_links \
             WHERE (source_id = 'a' AND target_id = 'b') \
                OR (source_id = 'b' AND target_id = 'a')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "migration should collapse double rows");

    // Surviving row is canonical (a, b), with merged metrics.
    let (s, t, strength, count, tf, tb, created): (String, String, f64, i32, i32, i32, f64) =
        storage
            .connection()
            .query_row(
                "SELECT source_id, target_id, strength, coactivation_count, \
                    temporal_forward, temporal_backward, created_at \
             FROM hebbian_links WHERE source_id = 'a' AND target_id = 'b'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();

    assert_eq!(s, "a");
    assert_eq!(t, "b");
    assert_eq!(strength, 1.0, "strength = max(1.0, 0.8)");
    assert_eq!(count, 8, "count = 5 + 3");
    assert_eq!(tf, 2, "temporal_forward = 2 + 0");
    assert_eq!(tb, 1, "temporal_backward = 1 + 0");
    assert_eq!(created, 1000.0, "created_at = min(1000, 2000)");
}

#[test]
fn iss117_migration_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    // Seed via the canonical writer — table is already canonical.
    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
        for _ in 0..3 {
            storage.record_coactivation("a", "b", 3).unwrap();
        }
    }

    let before = {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM hebbian_links", [], |row| row.get(0))
            .unwrap()
    };

    // Reopen → migration runs again on already-canonical table.
    let _ = Storage::new(&db_path).unwrap();

    let after = {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM hebbian_links", [], |row| row.get(0))
            .unwrap()
    };

    assert_eq!(before, after, "migration must be a no-op when canonical");
    assert_eq!(before, 1, "single canonical row from writer");
}

#[test]
fn iss117_migration_leaves_single_direction_rows_alone() {
    // A row with source_id < target_id (canonical) and NO mirror —
    // migration must leave it untouched.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory(&mut storage, "a");
        seed_memory(&mut storage, "b");
    }

    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES ('a', 'b', 0.5, 2, 1, 0, 'forward', 1500.0, 'default')",
            [],
        )
        .unwrap();
    }

    // Trigger migration.
    let storage = Storage::new(&db_path).unwrap();

    let (strength, count, created): (f64, i32, f64) = storage
        .connection()
        .query_row(
            "SELECT strength, coactivation_count, created_at \
             FROM hebbian_links WHERE source_id = 'a' AND target_id = 'b'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    assert_eq!(strength, 0.5, "untouched");
    assert_eq!(count, 2, "untouched");
    assert_eq!(created, 1500.0, "untouched");

    // Suppress unused-helper warning on this test path.
    let _ = insert_raw_link;
}

// ---------------------------------------------------------------
// ISS-118 cross-axis coverage: when id-ordering disagrees with
// (ns, id)-ordering, the migration's canonical-row rule must agree
// with the writer's canonical-row rule. Pre-ISS-118 the migration
// used raw `source_id > target_id` and silently DELETEd cross-NS
// rows on every reopen whenever the lower-ns endpoint had the
// higher id. These tests pin the ns-aware DELETE.
// ---------------------------------------------------------------

#[test]
fn iss118_cross_ns_row_survives_reopen_when_id_order_inverts_ns_order() {
    // Writer canonicalises by (ns, id) tuple. `ns_aaa < ns_zzz`,
    // so writer stamps source = ("ns_aaa", "hub"), target =
    // ("ns_zzz", "apple"). Raw id ordering is "hub" > "apple", so a
    // raw-id migration would DELETE this row. ns-aware migration
    // must keep it.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_ns(&mut storage, "hub", "ns_aaa");
        seed_memory_ns(&mut storage, "apple", "ns_zzz");

        for _ in 0..3 {
            storage
                .record_cross_namespace_coactivation("hub", "ns_aaa", "apple", "ns_zzz", 3)
                .unwrap();
        }

        let n = count_links_for_pair(&storage, "hub", "apple");
        assert_eq!(n, 1, "writer produces one canonical row");
    }

    // Reopen — Storage::new runs migrate_hebbian_canonical_rows.
    // Pre-ISS-118 this DELETEd the row because "hub" > "apple".
    // Post-ISS-118 the row survives because ("ns_aaa","hub") <
    // ("ns_zzz","apple").
    {
        let storage = Storage::new(&db_path).unwrap();
        let n = count_links_for_pair(&storage, "hub", "apple");
        assert_eq!(
            n, 1,
            "ISS-118: cross-NS row must survive reopen when id-order \
             inverts ns-order (pre-fix this was 0)"
        );
    }
}

#[test]
fn iss118_cross_ns_row_with_multiple_neighbours_survives_reopen() {
    // Stress: hub in lower ns has higher id than several neighbours
    // in higher ns. All rows would have been wiped by the raw-id
    // DELETE. Verify the whole fan survives reopen.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_ns(&mut storage, "hub", "ns_hub");
        seed_memory_ns(&mut storage, "a", "ns_other");
        seed_memory_ns(&mut storage, "b", "ns_other");
        seed_memory_ns(&mut storage, "c", "ns_other");

        for neighbour in &["a", "b", "c"] {
            for _ in 0..3 {
                storage
                    .record_cross_namespace_coactivation("hub", "ns_hub", neighbour, "ns_other", 3)
                    .unwrap();
            }
        }

        let before: i64 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM hebbian_links \
                 WHERE source_id = 'hub' OR target_id = 'hub'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 3, "writer produced one row per pair");
    }

    {
        let storage = Storage::new(&db_path).unwrap();
        let after: i64 = storage
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM hebbian_links \
                 WHERE source_id = 'hub' OR target_id = 'hub'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            after, 3,
            "ISS-118: all cross-NS rows survive reopen (pre-fix all 3 were deleted)"
        );
    }
}

#[test]
fn iss118_same_ns_with_inverted_id_order_still_canonicalises() {
    // Within a single namespace, the (ns, id) tuple collapses to
    // pure id comparison. Verify the migration still collapses
    // double-direction rows correctly when both endpoints share a
    // namespace — this is the original ISS-117 happy-path under the
    // new SQL, must remain green.
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("h.db");

    {
        let mut storage = Storage::new(&db_path).unwrap();
        seed_memory_ns(&mut storage, "alpha", "shared_ns");
        seed_memory_ns(&mut storage, "beta", "shared_ns");
    }

    // Seed double-direction rows directly (simulating a pre-ISS-117 db).
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES ('alpha', 'beta', 0.5, 2, 1, 0, 'forward', 1000.0, 'shared_ns')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO hebbian_links \
             (source_id, target_id, strength, coactivation_count, \
              temporal_forward, temporal_backward, direction, \
              created_at, namespace) \
             VALUES ('beta', 'alpha', 0.3, 1, 0, 1, 'backward', 2000.0, 'shared_ns')",
            [],
        )
        .unwrap();
    }

    // Trigger migration.
    let storage = Storage::new(&db_path).unwrap();
    let n = count_links_for_pair(&storage, "alpha", "beta");
    assert_eq!(n, 1, "same-ns double-direction collapses to one row");

    let (src, tgt): (String, String) = storage
        .connection()
        .query_row(
            "SELECT source_id, target_id FROM hebbian_links \
             WHERE (source_id = 'alpha' AND target_id = 'beta') \
                OR (source_id = 'beta'  AND target_id = 'alpha')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(src, "alpha", "canonical = min(id) under same ns");
    assert_eq!(tgt, "beta");
}
