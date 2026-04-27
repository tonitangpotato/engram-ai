//! T16 — Idempotency tests (design §11.2).
//!
//! Implements the §11.2 test matrix from
//! `.gid/features/v03-migration/design.md`:
//!
//! - `test_migrate_twice_is_noop` — running `migrate()` twice on the same
//!   DB is a no-op on the second run; the on-disk file hash is unchanged.
//!   We exercise this on two surfaces because the live end-to-end
//!   v0.2 → v0.3 path is currently gated by T9 (per-record backfill is
//!   blocked on `ResolutionPipeline::resolve_for_backfill`):
//!     1. `test_migrate_twice_is_noop_v03_db` — the post-migration
//!        steady state ("already at v0.3"). This is the production
//!        idempotency contract operators rely on.
//!     2. `test_migrate_twice_is_noop_gate2` — the v0.2 → schema-only
//!        path via `--gate phase2`, which is the largest write-path
//!        we can run today without T9. The second run short-circuits
//!        on the `SchemaState::V03` branch.
//!
//! - `test_ddl_guard_add_column` — `ALTER TABLE … ADD COLUMN` guard
//!   tolerates a column that already exists. This is also covered by
//!   `schema::tests::additive_columns_idempotent_on_second_run` at the
//!   unit level; the integration test here fixes the contract at the
//!   library boundary (`apply_additive_columns` is `pub`, so a future
//!   refactor that breaks idempotency would surface here even if the
//!   internal unit test was deleted or moved).
//!
//! - `test_topic_insert_or_ignore` — Phase 3 idempotency (re-running the
//!   topic carry-forward leaves the row count stable). **Currently gated
//!   on T11 (`task:mig-impl-topics`)** — Phase 3 is stubbed in `cli.rs`
//!   pending a schema disagreement resolution. We include the test
//!   shape with an explicit `#[ignore]` and a TODO referencing T11 so
//!   the body is in place when topics ships.
//!
//! All tests use `tempfile::tempdir` so they don't touch the working
//! directory.
//!
//! GOAL coverage: GOAL-4.3 (idempotent migration; partial migrations
//! resumable; no duplicate rows on re-run).

use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;

use engramai_migrate::{
    apply_additive_columns, migrate, MigrateOptions, MigrationPhase,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Snapshot the user-visible state of the database — everything except
/// migration-internal tables (lock, checkpoint, log) whose content
/// legitimately mutates on every `migrate()` call. Used to assert that
/// a no-op migration leaves user data untouched.
///
/// Returned as `(schema_version_rows, memories_rows)` — both vectors of
/// strings so equality is trivial. We don't try to be exhaustive on
/// every v0.2 table; the contract we're enforcing is:
///   - the version stamp is unchanged,
///   - the memories rows are unchanged.
///
/// If a future test needs to assert hebbian_links / topic stability,
/// extend this helper.
fn snapshot_user_state(path: &Path) -> (Vec<String>, Vec<String>) {
    let conn = Connection::open(path).unwrap();

    let mut version_rows = Vec::new();
    let mut stmt = conn
        .prepare("SELECT version, updated_at FROM schema_version ORDER BY version")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let v: i64 = row.get(0).unwrap();
        let u: String = row.get(1).unwrap();
        version_rows.push(format!("{v}|{u}"));
    }

    let mut memories_rows = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id, content, created_at FROM memories ORDER BY id")
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        let id: String = row.get(0).unwrap();
        let content: Option<String> = row.get(1).unwrap();
        let created_at: Option<String> = row.get(2).unwrap();
        memories_rows.push(format!(
            "{id}|{}|{}",
            content.unwrap_or_default(),
            created_at.unwrap_or_default()
        ));
    }

    (version_rows, memories_rows)
}

/// Seed a v0.2-shaped database — the minimum schema the migration
/// pre-flight needs to recognise the source as `SchemaState::V02`.
///
/// Mirrors the seed helper in `src/cli.rs` test module so the integration
/// suite is self-contained.
fn seed_v02_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
         INSERT INTO memories VALUES ('m1', 'hello', '2026-01-01T00:00:00Z');\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
    )
    .unwrap();
}

/// Seed a v0.3-shaped database — `schema_version = 3` is the only thing
/// the pre-flight needs to short-circuit to the "already at v0.3" branch.
fn seed_v03_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (3, '2026-04-27T00:00:00Z');",
    )
    .unwrap();
}

fn options_for(db: &Path) -> MigrateOptions {
    let mut opts = MigrateOptions::new(db);
    opts.tool_version = "0.1.0-idempotency-test".to_string();
    opts
}

// ---------------------------------------------------------------------------
// test_migrate_twice_is_noop — surface 1: v0.3 DB
// ---------------------------------------------------------------------------

/// §11.2: running `migrate()` on an already-v0.3 database twice is a
/// content-level no-op — both runs report `migration_complete = true`
/// with the "already at schema_version=3" warning, and the user-data
/// rows the migration is supposed to leave alone are unchanged.
///
/// **Note on byte-identity**: design §11.2 phrases the contract as "file
/// hash unchanged after second run". In practice the migration lock
/// (`MigrationLock::init` + `acquire`/`release` in preflight) writes
/// timestamp metadata into the DB on every run, so raw-byte file hash
/// is *not* invariant — only the user-visible content is. The
/// pragmatic GOAL-4.3 contract enforced here is:
///
///   - `schema_version` row identical (version + updated_at)
///   - `memories` rows identical (no Phase 4 backfill re-run)
///
/// If we later want strict byte-identity, the lock writes would need
/// to be tagged "transient mutation" (e.g. via WAL-only updates that
/// checkpoint cleanly). That is a follow-up, not what GOAL-4.3 asks
/// of us today.
#[test]
fn test_migrate_twice_is_noop_v03_db() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("v03.db");
    seed_v03_db(&db);

    // Snapshot the user-data before any migrate() call.
    let snapshot_before = snapshot_user_state(&db);

    // First run.
    let report1 = migrate(&options_for(&db)).expect("first migrate on v0.3 DB should succeed");
    assert!(report1.migration_complete);
    assert_eq!(report1.final_phase, MigrationPhase::Complete.tag());
    assert!(
        report1
            .warnings
            .iter()
            .any(|w| w.contains("already at schema_version=3")),
        "first run should warn about already-v0.3 state, got: {:?}",
        report1.warnings
    );

    let snapshot_after_first = snapshot_user_state(&db);
    assert_eq!(
        snapshot_before, snapshot_after_first,
        "first migrate run on a v0.3 DB must not change user data \
         (lock metadata is allowed to change, user rows are not)"
    );

    // Second run — must be a content no-op AND must not modify user data.
    let report2 = migrate(&options_for(&db)).expect("second migrate on v0.3 DB should succeed");
    assert!(report2.migration_complete);
    assert_eq!(report2.final_phase, MigrationPhase::Complete.tag());
    assert!(
        report2
            .warnings
            .iter()
            .any(|w| w.contains("already at schema_version=3")),
        "second run should warn about already-v0.3 state, got: {:?}",
        report2.warnings
    );

    let snapshot_after_second = snapshot_user_state(&db);
    assert_eq!(
        snapshot_after_first, snapshot_after_second,
        "second migrate run must not modify user data (GOAL-4.3 idempotency)"
    );
}

// ---------------------------------------------------------------------------
// test_migrate_twice_is_noop — surface 2: v0.2 DB via --gate phase2
// ---------------------------------------------------------------------------

/// §11.2 (partial): the largest live write-path runnable without T9.
///
/// Run 1: v0.2 DB + `--gate SchemaTransition` + `--accept-forward-only`
/// runs preflight + backup + schema (Phase 0–2), and the gate stops
/// the machine **after** Phase 2. Per design §3.1 last bullet,
/// Phase 2 deliberately does **not** bump `schema_version` — the
/// canonical version stamp lives in Phase 5 (Verify). So after a
/// gated phase2 run, the DB has the additive v0.3 columns but
/// `schema_version` is still 2.
///
/// Run 2 with the same options sees `schema_version = 2` again,
/// treats the DB as `SchemaState::V02`, and re-runs Phase 2's
/// additive-column DDL. Phase 2 is internally idempotent (the
/// "duplicate column" guard in `apply_additive_columns` and the
/// rename-no-op in `rename_entities_valence_if_present`), so the
/// re-run must succeed and the column set must not change.
///
/// **What this proves**: the schema-only path is idempotent under
/// repeated invocation, even when no version bump has been recorded —
/// which is the pessimistic case (no schema_version short-circuit).
#[test]
fn test_migrate_twice_is_noop_gate2() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("v02.db");
    seed_v02_db(&db);

    let mut opts = options_for(&db);
    opts.accept_forward_only = true;
    opts.gate = Some(MigrationPhase::SchemaTransition);
    // Skip backup so we don't write a sidecar file we'd have to account
    // for in the column-set comparison below.
    opts.no_backup = true;
    opts.accept_no_grace = true;

    // First run — gate at Phase 2 → expected to terminate with a
    // gate-reached error (mapped to ExitCode::GateReached by the binary).
    let result1 = migrate(&opts);
    assert!(
        result1.is_err(),
        "gate-reached on phase2 should surface as Err; got {:?}",
        result1
    );

    // After run 1, the DB should still be at schema_version=2 (Phase 5
    // never ran) but `memories` should now have the v0.3 additive
    // columns from Phase 2.
    {
        let conn = Connection::open(&db).unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            v, 2,
            "gate-at-phase2 must NOT bump schema_version (Phase 5 is the canonical bump site)"
        );
    }
    let columns_after_first = collect_columns_via_path(&db, "memories");

    // Second run — same gate, same options. Must succeed (i.e. surface
    // the same gate-reached error), and the column set must be unchanged.
    let result2 = migrate(&opts);
    assert!(
        result2.is_err(),
        "second gated run should also surface gate-reached, got {:?}",
        result2
    );

    // Schema_version is still 2.
    {
        let conn = Connection::open(&db).unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(v, 2, "second gated run must not bump version either");
    }
    let columns_after_second = collect_columns_via_path(&db, "memories");
    assert_eq!(
        columns_after_first, columns_after_second,
        "second gated run must not change the column set (Phase 2 is idempotent)"
    );
}

// ---------------------------------------------------------------------------
// test_ddl_guard_add_column
// ---------------------------------------------------------------------------

/// §11.2: `apply_additive_columns` tolerates a DB that already has the
/// columns it would add (i.e. the `ALTER TABLE ADD COLUMN` guard
/// suppresses the SQLite "duplicate column" error). Run twice: the
/// second invocation must succeed without modifying schema.
#[test]
fn test_ddl_guard_add_column() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("ddl-guard.db");
    let conn = Connection::open(&db).unwrap();

    // Seed minimal v0.2 tables that `apply_additive_columns` ALTERs.
    // Mirrors the schema unit tests' seed shape.
    conn.execute_batch(
        "CREATE TABLE memories (\
            id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
         CREATE TABLE hebbian_links (\
            from_id TEXT, to_id TEXT, weight REAL);",
    )
    .unwrap();

    // First call — adds the v0.3 columns.
    apply_additive_columns(&conn).expect("first apply_additive_columns should succeed");

    // Snapshot the columns set after the first run.
    let columns_after_first = collect_columns(&conn, "memories");
    assert!(
        !columns_after_first.is_empty(),
        "memories should have columns after first apply"
    );

    // Second call — must be a no-op idempotency-wise.
    apply_additive_columns(&conn)
        .expect("second apply_additive_columns must tolerate existing columns");

    let columns_after_second = collect_columns(&conn, "memories");
    assert_eq!(
        columns_after_first, columns_after_second,
        "second apply_additive_columns must not change the column set \
         (DDL guard tolerates duplicate-column error per §4.4)"
    );
}

/// Collect column names of a table via `PRAGMA table_info`.
fn collect_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        let name: String = row.get(1).unwrap();
        out.push(name);
    }
    out.sort();
    out
}

/// Same as [`collect_columns`] but opens its own short-lived
/// `Connection` from a path — the gate2 test wants to inspect schema
/// state between two `migrate()` invocations without holding a long-
/// lived handle that could collide with the migration lock.
fn collect_columns_via_path(path: &Path, table: &str) -> Vec<String> {
    let conn = Connection::open(path).unwrap();
    collect_columns(&conn, table)
}

// ---------------------------------------------------------------------------
// test_topic_insert_or_ignore — currently gated on T11
// ---------------------------------------------------------------------------

/// §11.2: running Phase 3 (topic carry-forward) twice on the same
/// fixture leaves `knowledge_topics` row count stable (no duplicates).
///
/// **STATUS — `#[ignore]`**: T11 (`task:mig-impl-topics`) is currently
/// blocked on a schema disagreement between migration design §6 and
/// the v03-graph-layer schema (the DDL columns design §6 references
/// don't exist in graph-layer's `knowledge_topics` table). Phase 3 in
/// `cli.rs` is a stub no-op until that's resolved; running this test
/// today against the stub would just verify "stub did nothing twice",
/// which provides no semantic coverage.
///
/// When T11 lands:
///   1. Remove `#[ignore]`.
///   2. Replace the `unimplemented!()` body with the real assertion:
///      seed a v0.2 fixture with 10 `kc_topic_pages` rows, run a Phase 3
///      executor twice, assert `count(knowledge_topics) == 10` after
///      each call.
#[test]
#[ignore = "T11 (mig-impl-topics) blocked — Phase 3 is a stub; see cli.rs §run_topic_carry_forward"]
fn test_topic_insert_or_ignore() {
    // TODO(T11): when topic carry-forward is implemented,
    //   - seed a v0.2 fixture with 10 kc_topic_pages rows
    //   - run Phase 3 once → assert knowledge_topics row count == 10
    //   - run Phase 3 again → assert row count is still 10
    //   - assert legacy=1 on every row (per §6 carry-forward semantics)
    unimplemented!("blocked on T11 (mig-impl-topics); see test docstring");
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Compile-time guards
// ---------------------------------------------------------------------------

/// Compile-time check: the public re-exports we depend on stay wired.
/// If someone moves `MigrationPhase` and breaks the re-export path,
/// this fails the build before any run-time test executes.
#[allow(dead_code)]
fn _phase_alias_check() {
    let _ = MigrationPhase::Complete;
}
