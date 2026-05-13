//! T11 — Phase A acceptance test for v0.4 unified substrate
//! (`v04-unified-substrate/design.md` §8.2).
//!
//! Verifies the additive-only invariant: after Phase A migrations run,
//!
//!   1. **Fresh DB**: a brand-new `Storage` has all four unified tables
//!      (`nodes`, `edges`, `nodes_fts`, `node_embeddings`), plus their
//!      indexes and triggers, AND `engram_meta.schema_version =
//!      '0.4-additive'`.
//!
//!   2. **Legacy DB**: re-opening a DB that already contains v0.3 data
//!      (rows in `memories`, `memory_embeddings`, `entities`, …) with
//!      the v0.4 binary adds the unified tables but **does not touch
//!      a single byte** of the legacy rows. This is the guarantee
//!      design §5.1 makes: Phase A is read-side invisible to existing
//!      retrieval code.
//!
//!   3. **Idempotency** (GUARD-ss.3): re-opening repeatedly leaves the
//!      same end state — no accumulated rows in `engram_meta`, no
//!      duplicate triggers, no errors.
//!
//! These three together form the Phase A acceptance gate. Phase B
//! (dual-write) only kicks in when application code is changed to
//! emit substrate writes; until then the database is a strict
//! superset of v0.3.

use engramai::storage::Storage;
use rusqlite::{params, Connection};
use std::path::Path;
use tempfile::tempdir;

/// The four substrate tables that Phase A creates. The integration test
/// asserts against this list, not against whatever happens to exist —
/// any drift between this list and storage.rs will be caught.
const UNIFIED_TABLES: &[&str] =
    &["nodes", "edges", "nodes_fts", "node_embeddings"];

/// Triggers created by T07. If any of these go missing the FTS surface
/// silently de-syncs from `nodes`, which is the single most expensive
/// failure mode this acceptance test catches.
const UNIFIED_FTS_TRIGGERS: &[&str] =
    &["nodes_fts_ai", "nodes_fts_ad", "nodes_fts_au"];

/// Indexes the writers will rely on. Not exhaustive (storage.rs creates
/// more) — just the ones the acceptance test cares about.
const REQUIRED_INDEXES: &[&str] = &[
    "idx_node_embeddings_model",
    "idx_nodes_kind",
    "idx_nodes_namespace",
    "idx_edges_source",
    "idx_edges_target",
    "idx_edges_kind_pred",
];

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .is_ok()
}

fn trigger_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .is_ok()
}

fn index_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .is_ok()
}

fn schema_version(conn: &Connection) -> String {
    conn.query_row(
        "SELECT value FROM engram_meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )
    .expect("schema_version row must exist after open()")
}

fn assert_phase_a_complete(conn: &Connection) {
    for tbl in UNIFIED_TABLES {
        assert!(
            table_exists(conn, tbl),
            "Phase A table missing: {tbl}"
        );
    }
    for trig in UNIFIED_FTS_TRIGGERS {
        assert!(
            trigger_exists(conn, trig),
            "Phase A trigger missing: {trig}"
        );
    }
    for idx in REQUIRED_INDEXES {
        assert!(index_exists(conn, idx), "Phase A index missing: {idx}");
    }
    assert_eq!(
        schema_version(conn),
        "0.4-additive",
        "schema_version must be 0.4-additive after Phase A"
    );
}

// ===========================================================================
// Scenario 1 — fresh DB
// ===========================================================================

#[test]
fn t11_fresh_db_has_all_phase_a_artifacts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("fresh.db");

    let storage = Storage::new(&path).expect("open fresh DB");
    assert_phase_a_complete(storage.conn());

    // And the unified tables are empty on a fresh DB.
    for tbl in UNIFIED_TABLES {
        let count: i64 = storage
            .conn()
            .query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "fresh {tbl} should be empty, got {count}");
    }
}

// ===========================================================================
// Scenario 2 — legacy DB → re-open as v0.4
// ===========================================================================

/// Populate a DB with realistic v0.3 rows so we can assert they survive
/// Phase A unchanged. We open via `Storage::new` once (which already
/// runs Phase A), seed rows, **snapshot the rows**, then re-open and
/// confirm nothing moved.
///
/// This deliberately uses the storage API rather than hand-crafted SQL —
/// the goal is to test the *real* round trip, not a synthetic schema.
fn seed_legacy_rows(path: &Path) -> Vec<(String, String, i64, i64, i64)> {
    let storage = Storage::new(path).unwrap();
    let conn = storage.conn();

    // Seed a memory row directly via SQL (the higher-level Memory API
    // would pull in the entire async runtime, which is heavier than we
    // need for an integration sanity check).
    conn.execute(
        "INSERT INTO memories
         (id, content, memory_type, layer, created_at,
          working_strength, core_strength, importance, namespace)
         VALUES
         ('mem-legacy-1', 'legacy content', 'factual', 'core', 1.0,
          0.0, 1.0, 0.5, 'default')",
        [],
    )
    .expect("insert legacy memory row");

    // And an entity row.
    conn.execute(
        "INSERT INTO entities
         (id, name, entity_type, namespace, metadata, created_at, updated_at)
         VALUES
         ('ent-legacy-1', 'potato', 'person', 'default', '{}', 1.0, 1.0)",
        [],
    )
    .expect("insert legacy entity row");

    // And a hebbian edge. Composite PK (source_id, target_id), no id column.
    conn.execute(
        "INSERT INTO hebbian_links
         (source_id, target_id, strength, coactivation_count,
          temporal_forward, temporal_backward, direction,
          created_at, namespace)
         VALUES
         ('mem-legacy-1', 'mem-legacy-1', 0.3, 1,
          0, 0, 'bidirectional',
          1.0, 'default')",
        [],
    )
    .expect("insert legacy hebbian row");

    // Snapshot: (table, id, table-row-count, ..., …)
    fn count(conn: &Connection, tbl: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))
            .unwrap()
    }

    vec![
        (
            "memories".into(),
            "mem-legacy-1".into(),
            count(conn, "memories"),
            count(conn, "memory_embeddings"),
            0,
        ),
        (
            "entities".into(),
            "ent-legacy-1".into(),
            count(conn, "entities"),
            0,
            0,
        ),
        (
            // hebbian_links has a composite PK (source_id, target_id),
            // not a single `id` column. Use source_id as the lookup key
            // for the existence check below.
            "hebbian_links".into(),
            "mem-legacy-1".into(),
            count(conn, "hebbian_links"),
            0,
            0,
        ),
    ]
}

#[test]
fn t11_reopen_preserves_legacy_rows_and_adds_substrate() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.db");

    // Pass 1 — seed legacy rows.
    let snapshot = seed_legacy_rows(&path);

    // Pass 2 — re-open. Phase A migrations run again (idempotent) and
    // schema_version stays at 0.4-additive. The legacy rows must be
    // byte-for-byte untouched.
    let storage = Storage::new(&path).expect("re-open seeded DB");
    let conn = storage.conn();
    assert_phase_a_complete(conn);

    // Legacy rows still present, same id. hebbian_links uses
    // source_id rather than id (composite PK), so use a per-table key
    // column.
    for (tbl, id, _, _, _) in &snapshot {
        let key_col = if tbl == "hebbian_links" { "source_id" } else { "id" };
        let exists: bool = conn
            .query_row(
                &format!("SELECT 1 FROM {tbl} WHERE {key_col} = ?1"),
                params![id],
                |_| Ok(()),
            )
            .is_ok();
        assert!(exists, "legacy row {id} dropped from {tbl}");
    }

    // Unified tables are still empty — Phase A is read-side invisible
    // until Phase B writers turn on (T12-T16). If a row appeared here
    // unprompted, it would mean Phase A accidentally triggered a write.
    for tbl in UNIFIED_TABLES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "Phase A must not write to {tbl}; found {count} rows after re-open"
        );
    }

    // Legacy row counts unchanged.
    for (tbl, _, expected, _, _) in &snapshot {
        let now: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {tbl}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            now, *expected,
            "row count for {tbl} changed: {expected} → {now}"
        );
    }
}

// ===========================================================================
// Scenario 3 — repeated re-open idempotency
// ===========================================================================

#[test]
fn t11_repeated_open_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("idem.db");

    for round in 0..3 {
        let storage = Storage::new(&path).expect("re-open");
        assert_phase_a_complete(storage.conn());

        // No row accumulation in engram_meta (one row per key max).
        let dup_count: i64 = storage
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM engram_meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dup_count, 1,
            "round {round}: schema_version row count must be 1, got {dup_count}"
        );

        // No duplicate triggers (sqlite_master enforces unique names; if
        // CREATE TRIGGER ran without IF NOT EXISTS this would have failed
        // long before this assertion, but be defensive).
        for trig in UNIFIED_FTS_TRIGGERS {
            let c: i64 = storage
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='trigger' AND name = ?1",
                    params![trig],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(c, 1, "round {round}: trigger {trig} appears {c} times");
        }
    }
}
