//! ISS-044 — End-to-end Phase 4 backfill integration tests.
//!
//! Validates `MigrationOrchestrator::run_backfill` actually wires
//! `PipelineRecordProcessor` to v0.2 → v0.3 conversion (replacing the
//! pre-ISS-044 stub). Coverage:
//!
//! - **Smoke**: a populated v0.2 DB → migrate completes Phase 4 without
//!   the stub error → `BackfillReport.records_processed == memories
//!   row count` → graph DB's `graph_entities` is populated proportional
//!   to memories. (ISS-058 root fix: writes now land in `--graph-db
//!   <path>`, not the v0.2 main DB.)
//! - **Idempotency**: re-running migrate (after the first one finishes)
//!   on a v0.3 DB does not double-write graph rows. Relies on
//!   `Memory::with_pipeline_pool`'s no-op-on-v0.3 short-circuit.
//! - **Stop-on-failure**: not covered here (no fixture for failing
//!   pipeline; `processor.rs` unit tests cover the failure path).
//!
//! Acceptance criteria mapped from `.gid/issues/ISS-044/issue.md`:
//!   - [x] `engram migrate --accept-forward-only` against populated
//!         v0.2 DB completes Phase 4 successfully (no stub error).
//!   - [x] After migration, entities/edges tables populated proportional
//!         to ingested memories.
//!   - [x] Idempotent: running migrate twice doesn't double-write edges.
//!
//! Uses the noop triple extractor (entity-only) so the test does not
//! hit any external LLM and runs deterministically in CI.

use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;

use engramai_migrate::{migrate, MigrateOptions};

/// Seed a richer v0.2 DB than the `resume.rs` minimal fixture: memories
/// have content with entity patterns the default `EntityExtractor` can
/// recognise via its built-in regex set (ISS-NNN, file paths, URLs,
/// @handles, *-rs project names). Plain capitalised proper nouns
/// (Alice/Bob/Paris) are NOT extracted by default — the extractor only
/// matches *known* entities (Aho-Corasick) plus structural regex hits.
/// Resume/idempotency tests use a thinner fixture because they never
/// reach Phase 4.
fn seed_v02_db_with_entities(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        // v0.2 `memories` schema: the `metadata` column is part of the
        // base v0.2 shape (the backfill cursor SELECTs id, content,
        // metadata, created_at).
        "CREATE TABLE memories (\
             id TEXT PRIMARY KEY,\
             content TEXT,\
             metadata TEXT,\
             created_at TEXT\
         );\
         INSERT INTO memories VALUES ('m1', 'Filed ISS-100 against gid-rs to track src/main.rs refactor.', NULL, '2026-01-01T00:00:00Z');\
         INSERT INTO memories VALUES ('m2', 'See https://example.com/issue/200 for ISS-200 details from @alice_dev.', NULL, '2026-01-02T00:00:00Z');\
         INSERT INTO memories VALUES ('m3', 'Updated GOAL-3.1 in design.md and tracked GUARD-7 against engramai-rs.', NULL, '2026-01-03T00:00:00Z');\
         CREATE TABLE hebbian_links (a TEXT, b TEXT);\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
    )
    .unwrap();
}

fn options_for(db: &Path) -> MigrateOptions {
    let mut opts = MigrateOptions::new(db);
    opts.tool_version = "0.1.0-iss044-test".to_string();
    opts.accept_forward_only = true;
    opts.no_backup = true;
    opts.accept_no_grace = true;
    // Default extractor = None (Noop) — entity-only path. No external calls.
    opts
}

fn count_rows(db: &Path, table: &str) -> i64 {
    let conn = Connection::open(db).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap_or(0)
}

fn table_exists(db: &Path, table: &str) -> bool {
    let conn = Connection::open(db).unwrap();
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .map(|_| true)
    .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// test_backfill_completes_against_populated_v02_db
// ---------------------------------------------------------------------------

/// Acceptance criterion #1: `migrate --accept-forward-only` against a
/// populated v0.2 DB completes Phase 4 successfully (no stub error).
///
/// Acceptance criterion #2: After migration, the graph DB is populated
/// proportional to ingested memories.
#[test]
fn test_backfill_completes_against_populated_v02_db() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("populated.db");
    let graph_db = dir.path().join("populated.graph.db");
    seed_v02_db_with_entities(&db);

    assert_eq!(count_rows(&db, "memories"), 3, "fixture seeded 3 memories");

    let mut opts = options_for(&db);
    opts.graph_db_path = Some(graph_db.clone());

    let report = migrate(&opts).expect("migrate must succeed on populated v0.2 DB");

    // Top-level: phase 4 ran, finalize was reached.
    assert!(
        report.migration_complete,
        "migration_complete must be true; got report = {report:?}"
    );
    assert_eq!(
        report.backfill.records_total, 3,
        "records_total must equal memories row count"
    );
    assert_eq!(
        report.backfill.records_processed, 3,
        "all 3 memories must be processed; report.backfill = {:?}",
        report.backfill
    );
    assert_eq!(
        report.backfill.records_failed, 0,
        "no failures expected with regex extractor on well-formed inputs; \
         report.backfill = {:?}",
        report.backfill
    );

    // Graph DB file is created and carries the graph-layer schema.
    assert!(graph_db.exists(), "graph DB file must be created");
    // ISS-058 root fix: graph schema lives ONLY in the graph DB now.
    // The pre-ISS-058 code redundantly initialised graph tables on the
    // main DB to mask a split-brain bug; that's gone.
    assert!(
        table_exists(&graph_db, "graph_entities"),
        "graph DB must have graph_entities table"
    );

    // Entity count: with the fixture's structural patterns (ISS-NNN,
    // GOAL-X.Y, GUARD-N, src/*.rs, https://, @handle, *-rs project)
    // the default `EntityExtractor` produces several entities per row.
    // We don't pin an exact count because the extractor's regex set is
    // allowed to evolve — the contract is "non-empty proportional to
    // memories".
    //
    // ISS-058 root fix: entities/edges/mentions land in `graph_db`
    // (the file passed via `--graph-db <path>`), not the v0.2 main DB.
    // Pre-fix this assertion targeted `&db` because the processor
    // wrapped a fresh SqliteGraphStore over the main-DB conn — a
    // known split-brain. The processor now shares the same
    // `Arc<Mutex<SqliteGraphStore>>` the pipeline reads from, so reads
    // and writes both target `graph_db`.
    let entity_count = count_rows(&graph_db, "graph_entities");
    assert!(
        entity_count > 0,
        "graph_entities table must be non-empty in graph DB after backfill; \
         got {entity_count}"
    );
}

// ---------------------------------------------------------------------------
// test_backfill_idempotent_on_v03_db
// ---------------------------------------------------------------------------

/// Acceptance criterion #6: running migrate twice does NOT double-write.
///
/// Mechanics: the second `migrate()` call sees `SchemaState::V03` and
/// short-circuits with a "already at schema_version=3; nothing to do"
/// warning before Phase 4 runs again. So the contract here is observed
/// via the *second* report being marked complete with empty backfill
/// counters, AND graph entity count not changing between runs.
#[test]
fn test_backfill_idempotent_on_v03_db() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("idem.db");
    let graph_db = dir.path().join("idem.graph.db");
    seed_v02_db_with_entities(&db);

    let mut opts = options_for(&db);
    opts.graph_db_path = Some(graph_db.clone());

    // First run.
    let r1 = migrate(&opts).expect("first migrate");
    assert!(r1.migration_complete);
    assert_eq!(r1.backfill.records_processed, 3);
    // ISS-058: query graph DB, not main DB.
    let entities_after_first = count_rows(&graph_db, "graph_entities");
    assert!(entities_after_first > 0);

    // Second run (DB now at schema_version=3).
    let r2 = migrate(&opts).expect("second migrate must not error");
    assert!(
        r2.migration_complete,
        "v0.3 DB second run must report complete"
    );
    // Phase 4 should NOT have run a second time on a v0.3 DB.
    assert_eq!(
        r2.backfill.records_processed, 0,
        "second run on v0.3 DB must skip Phase 4 entirely"
    );

    // Entity rows didn't grow.
    let entities_after_second = count_rows(&graph_db, "graph_entities");
    assert_eq!(
        entities_after_first, entities_after_second,
        "running migrate twice must not double-write graph rows"
    );
}

// ---------------------------------------------------------------------------
// test_backfill_dry_run_does_not_write
// ---------------------------------------------------------------------------

/// Sanity: dry-run still works (preserves the pre-ISS-044 dry-run
/// behaviour). The graph DB must not be created.
#[test]
fn test_backfill_dry_run_does_not_write() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("dry.db");
    let graph_db = dir.path().join("dry.graph.db");
    seed_v02_db_with_entities(&db);

    let mut opts = options_for(&db);
    opts.graph_db_path = Some(graph_db.clone());
    opts.dry_run = true;

    let report = migrate(&opts).expect("dry-run must not error on populated v0.2 DB");
    assert!(report.dry_run);
    assert_eq!(
        report.backfill.records_total, 3,
        "dry-run still reports projected row count"
    );
    // Dry-run reports projection but does not actually process any
    // records. records_processed == 0.
    assert_eq!(
        report.backfill.records_processed, 0,
        "dry-run does not process records"
    );
    assert!(
        !graph_db.exists(),
        "dry-run must not create the graph DB file"
    );
}
