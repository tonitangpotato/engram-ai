//! Phase 2 — schema transition (§4.2 / §4.3 / §4.4 of
//! `.gid/features/v03-migration/design.md`).
//!
//! This module owns three responsibilities, kept separate so each can be
//! tested in isolation:
//!
//! 1. **§4.2 additive ALTERs** on the existing `memories` table (5 columns
//!    enumerated in the design) and the conditional pure rename
//!    `entities.valence → entities.agent_affect`. Idempotent via SQLite's
//!    `duplicate column` and `no such column` error tolerance.
//! 2. **§4.3 schema_version handshake** — Phase 2 commit: insert/upsert
//!    `(3, now)` into `schema_version` as the atomic switch from "v0.2 DB"
//!    to "v0.3 DB".
//! 3. **§4.4 idempotency wrapper** — the entire Phase 2 batch (caller-
//!    supplied table-creation DDL + this module's ALTERs + the version
//!    bump) runs inside a single `BEGIN IMMEDIATE … COMMIT`. If any
//!    statement fails, the whole batch rolls back and `schema_version`
//!    stays at 2 — the database is unchanged (§4.2 last paragraph).
//!
//! ## Why DDL is injected, not embedded
//!
//! §4.2 explicitly defers the per-column DDL for new tables (`graph_*`,
//! `episodes`, `affect_mood_history`, `knowledge_topics`) to v03-graph-
//! layer §4.1, which is the *single source of truth* for those schemas.
//! Hard-coding a copy of those `CREATE TABLE` statements inside this crate
//! would create exactly the dual-source-of-truth drift the design warns
//! against.
//!
//! Instead, [`run_phase2`] accepts a `table_ddl: &str` chunk that the
//! caller (CLI integration layer / future build task) sources from the
//! engramai crate (or a generated `schema.sql`). The migration module owns
//! the **protocol** (transaction, ALTER guards, conditional rename, version
//! bump); the **DDL text** stays where it belongs.
//!
//! Tests in this module use a small fixture DDL string to verify the
//! protocol — they don't try to reproduce the full v0.3 schema.

use rusqlite::{Connection, OptionalExtension};

use crate::error::MigrationError;
use crate::preflight::TARGET_SCHEMA_VERSION;

/// SQL chunk creating the `schema_version` tracking table. Idempotent.
///
/// Public so other phases (notably preflight on a Fresh DB after Phase 0
/// classification) can ensure the row infrastructure exists before
/// inserting the bootstrap `(2, now)` row per §4.3 step 2.
pub const SCHEMA_VERSION_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version    INTEGER PRIMARY KEY,
    updated_at TEXT    NOT NULL
);
"#;

/// §4.2 additive columns on the existing `memories` table.
///
/// Each tuple is `(column_name, full_ddl)`. The column name is used only
/// for error messages — SQLite has no `IF NOT EXISTS` for `ADD COLUMN`,
/// so we run the statement and tolerate the "duplicate column" error to
/// keep the operation idempotent (§4.4 layer 1).
const MEMORIES_ALTERS: &[(&str, &str)] = &[
    (
        "episode_id",
        "ALTER TABLE memories ADD COLUMN episode_id INTEGER",
    ),
    (
        "entity_ids",
        "ALTER TABLE memories ADD COLUMN entity_ids TEXT NOT NULL DEFAULT '[]'",
    ),
    (
        "edge_ids",
        "ALTER TABLE memories ADD COLUMN edge_ids TEXT NOT NULL DEFAULT '[]'",
    ),
    (
        "confidence",
        "ALTER TABLE memories ADD COLUMN confidence REAL",
    ),
    (
        "hebbian_links_entity_pair", // descriptive label; column is on hebbian_links
        "ALTER TABLE hebbian_links ADD COLUMN entity_pair TEXT",
    ),
];

/// Returns `true` if `error` represents the SQLite "duplicate column"
/// condition that we want to swallow for ALTER idempotency.
fn is_duplicate_column_error(e: &rusqlite::Error) -> bool {
    e.to_string().contains("duplicate column name")
}

/// Returns `true` if `error` represents "table/column not found", which
/// we tolerate for the conditional rename of legacy `entities.valence`.
fn is_missing_object_error(e: &rusqlite::Error) -> bool {
    let s = e.to_string();
    s.contains("no such table") || s.contains("no such column")
}

/// Apply the §4.2 additive ALTERs on `memories` and `hebbian_links`.
///
/// Idempotent: re-running on a v0.3 DB is a no-op because every ALTER
/// statement that would already have run produces "duplicate column"
/// which we explicitly tolerate. Any other SQLite error surfaces as
/// `MigrationError::DdlFailed` with the column name in the message.
///
/// **Pre-condition:** the `memories` and `hebbian_links` tables must
/// exist. On a Fresh DB they don't — but Fresh DBs go through a different
/// init path (no ALTERs needed) and never call this function.
pub fn apply_additive_columns(conn: &Connection) -> Result<(), MigrationError> {
    for (col, ddl) in MEMORIES_ALTERS {
        match conn.execute(ddl, []) {
            Ok(_) => {}
            Err(e) if is_duplicate_column_error(&e) => {
                // Column already added by a prior run — expected on retry/resume.
            }
            Err(e) => {
                return Err(MigrationError::DdlFailed(format!(
                    "ALTER TABLE … ADD COLUMN {col}: {e}"
                )));
            }
        }
    }
    Ok(())
}

/// Apply the §4.2 conditional `entities.valence → entities.agent_affect`
/// rename (master DESIGN §8.1).
///
/// Behaviour matrix:
///
/// | Source state                                           | Action                          |
/// |--------------------------------------------------------|---------------------------------|
/// | `entities` table missing                               | no-op (Fresh DB has no v0.2 entities) |
/// | `entities.valence` column present                      | RENAME to `agent_affect`        |
/// | `entities.valence` already absent (already renamed)    | no-op (idempotent re-run)       |
/// | `entities.agent_affect` already present                | no-op (rename already done)     |
///
/// Uses `ALTER TABLE … RENAME COLUMN` (SQLite ≥ 3.25 — the bundled
/// `rusqlite` crate ships ≥ 3.40, so this is always available).
pub fn rename_entities_valence_if_present(conn: &Connection) -> Result<(), MigrationError> {
    // Cheap guard: if `entities` doesn't exist, nothing to rename.
    let entities_exists = table_exists(conn, "entities")?;
    if !entities_exists {
        return Ok(());
    }

    // Inspect columns. We need this both for "is `valence` present" and
    // "is `agent_affect` already present".
    let mut has_valence = false;
    let mut has_agent_affect = false;
    let mut stmt = conn
        .prepare("PRAGMA table_info(entities)")
        .map_err(|e| MigrationError::DdlFailed(format!("PRAGMA table_info(entities): {e}")))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| MigrationError::DdlFailed(format!("PRAGMA table_info query: {e}")))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| MigrationError::DdlFailed(format!("PRAGMA table_info next: {e}")))?
    {
        let name: String = row
            .get(1)
            .map_err(|e| MigrationError::DdlFailed(format!("PRAGMA table_info col name: {e}")))?;
        match name.as_str() {
            "valence" => has_valence = true,
            "agent_affect" => has_agent_affect = true,
            _ => {}
        }
    }

    if !has_valence {
        // Already renamed (or never existed) — idempotent no-op.
        return Ok(());
    }
    if has_agent_affect {
        // Both columns present — ambiguous state. The design treats this
        // as an unexpected schema drift; we surface it loudly rather than
        // pick a winner.
        return Err(MigrationError::DdlFailed(
            "entities table has BOTH valence and agent_affect columns; \
             refusing to rename (schema drift — manual intervention required)"
                .to_string(),
        ));
    }

    match conn.execute(
        "ALTER TABLE entities RENAME COLUMN valence TO agent_affect",
        [],
    ) {
        Ok(_) => Ok(()),
        // Defensive: if the column vanished between PRAGMA and ALTER (TOCTOU),
        // treat it as already done.
        Err(e) if is_missing_object_error(&e) => Ok(()),
        Err(e) => Err(MigrationError::DdlFailed(format!(
            "ALTER TABLE entities RENAME COLUMN valence: {e}"
        ))),
    }
}

/// Bump `schema_version` to 3, recording `now` as `updated_at`.
///
/// Implemented as `INSERT OR REPLACE` so it is safe under both fresh-row
/// and resume scenarios:
///
/// - First Phase 2 run: row `(3, …)` does not yet exist → INSERT.
/// - Resumed Phase 2 run after partial completion: row `(3, …)` may
///   exist → REPLACE refreshes `updated_at` (cheap, harmless).
///
/// `now` is supplied by the caller (RFC 3339 string) so this function
/// stays pure and unit-testable without pulling `chrono` into a hot path.
pub fn record_schema_version_v3(
    conn: &Connection,
    now_rfc3339: &str,
) -> Result<(), MigrationError> {
    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, updated_at) VALUES (?1, ?2)",
        rusqlite::params![TARGET_SCHEMA_VERSION as i64, now_rfc3339],
    )
    .map_err(|e| {
        MigrationError::DdlFailed(format!("INSERT INTO schema_version (version=3): {e}"))
    })?;
    Ok(())
}

/// Phase 2 transactional wrapper.
///
/// Executes, in order, inside a single `BEGIN IMMEDIATE … COMMIT`:
///
/// 1. Caller-supplied `table_ddl` (CREATE TABLE / CREATE INDEX statements
///    for new v0.3 tables — sourced from v03-graph-layer §4.1).
/// 2. [`apply_additive_columns`] (ALTERs on `memories` / `hebbian_links`).
/// 3. [`rename_entities_valence_if_present`] (legacy column rename).
/// 4. Ensure [`SCHEMA_VERSION_DDL`] (the row table itself).
/// 5. [`record_schema_version_v3`] — atomic version bump.
///
/// On any error in steps 1-5, the transaction is rolled back and
/// `schema_version` remains at 2 — the database is unchanged from the
/// caller's point of view (§4.2 final paragraph).
///
/// **Idempotency** (§4.4): all DDL is `IF NOT EXISTS`-guarded or
/// duplicate-tolerant; the rename is conditional; the version bump is
/// `INSERT OR REPLACE`. Re-running Phase 2 on a v0.3 DB is a no-op.
///
/// **Why `IMMEDIATE`:** acquires the SQLite reserved lock at `BEGIN` time
/// rather than on first write, eliminating a class of "BUSY" failures
/// when another reader is open. The migration lock (Phase 0) prevents
/// concurrent migrators, but other read-only consumers may be present.
pub fn run_phase2(
    conn: &mut Connection,
    table_ddl: &str,
    now_rfc3339: &str,
) -> Result<(), MigrationError> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| MigrationError::DdlFailed(format!("BEGIN IMMEDIATE: {e}")))?;

    // 1. Caller-supplied table DDL (graph_*, episodes, etc.).
    if !table_ddl.trim().is_empty() {
        tx.execute_batch(table_ddl)
            .map_err(|e| MigrationError::DdlFailed(format!("table-creation DDL batch: {e}")))?;
    }

    // 2. Additive columns on existing v0.2 tables.
    apply_additive_columns(&tx)?;

    // 3. Conditional rename.
    rename_entities_valence_if_present(&tx)?;

    // 4. schema_version row infrastructure.
    tx.execute_batch(SCHEMA_VERSION_DDL)
        .map_err(|e| MigrationError::DdlFailed(format!("schema_version DDL: {e}")))?;

    // 5. Atomic version bump.
    record_schema_version_v3(&tx, now_rfc3339)?;

    tx.commit()
        .map_err(|e| MigrationError::DdlFailed(format!("COMMIT: {e}")))?;

    Ok(())
}

/// Local copy of preflight's `table_exists` helper (private there).
///
/// Duplication is intentional and minimal: the function is 4 lines, and
/// re-exporting it from preflight would force schema to depend on
/// preflight's other internals. Keep schema's interface clean.
fn table_exists(conn: &Connection, name: &str) -> Result<bool, MigrationError> {
    conn.query_row(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![name],
        |_row| Ok::<_, rusqlite::Error>(()),
    )
    .optional()
    .map(|opt| opt.is_some())
    .map_err(|e| MigrationError::DdlFailed(format!("sqlite_master probe for {name}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorTag, ExitCode};

    /// Minimal v0.2-shaped fixture: `memories` + `hebbian_links` +
    /// `entities` tables. We intentionally use a subset of the real
    /// schema — just enough columns to exercise the ALTERs and the
    /// conditional rename. The full v0.3 schema is graph-layer's
    /// concern, not this module's.
    fn make_v02_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE hebbian_links (a TEXT, b TEXT);\
             CREATE TABLE entities (id TEXT PRIMARY KEY, name TEXT, valence REAL);",
        )
        .unwrap();
        conn
    }

    fn make_v02_fixture_no_entities() -> Connection {
        // Some v0.2.2 deployments never wrote to `entities`; the design
        // says rename is conditional on table presence.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE hebbian_links (a TEXT, b TEXT);",
        )
        .unwrap();
        conn
    }

    fn column_exists(conn: &Connection, table: &str, col: &str) -> bool {
        let sql = format!("PRAGMA table_info({table})");
        let mut stmt = conn.prepare(&sql).unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(1).unwrap();
            if name == col {
                return true;
            }
        }
        false
    }

    fn read_max_version(conn: &Connection) -> Option<i64> {
        conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .unwrap()
    }

    // ---------- apply_additive_columns ----------

    #[test]
    fn additive_columns_added_on_v02_db() {
        let conn = make_v02_fixture();
        apply_additive_columns(&conn).unwrap();
        assert!(column_exists(&conn, "memories", "episode_id"));
        assert!(column_exists(&conn, "memories", "entity_ids"));
        assert!(column_exists(&conn, "memories", "edge_ids"));
        assert!(column_exists(&conn, "memories", "confidence"));
        assert!(column_exists(&conn, "hebbian_links", "entity_pair"));
    }

    #[test]
    fn additive_columns_idempotent_on_second_run() {
        let conn = make_v02_fixture();
        apply_additive_columns(&conn).unwrap();
        // Second call must not error — proves "duplicate column" tolerance.
        apply_additive_columns(&conn).unwrap();
        // And columns are still there exactly once.
        assert!(column_exists(&conn, "memories", "episode_id"));
    }

    #[test]
    fn additive_columns_propagate_unrelated_errors_as_ddl_failed() {
        // No memories table → first ALTER fails with "no such table"
        // which is NOT the duplicate-column error we tolerate, so it
        // surfaces as DdlFailed (exit code 7).
        let conn = Connection::open_in_memory().unwrap();
        let err = apply_additive_columns(&conn).unwrap_err();
        match err {
            MigrationError::DdlFailed(msg) => {
                assert!(msg.contains("episode_id"), "msg: {msg}");
            }
            other => panic!("expected DdlFailed, got {other:?}"),
        }
    }

    // ---------- rename_entities_valence_if_present ----------

    #[test]
    fn rename_renames_legacy_valence_to_agent_affect() {
        let conn = make_v02_fixture();
        rename_entities_valence_if_present(&conn).unwrap();
        assert!(!column_exists(&conn, "entities", "valence"));
        assert!(column_exists(&conn, "entities", "agent_affect"));
    }

    #[test]
    fn rename_idempotent_when_already_renamed() {
        let conn = make_v02_fixture();
        rename_entities_valence_if_present(&conn).unwrap();
        // Second call must be a no-op (valence is now agent_affect).
        rename_entities_valence_if_present(&conn).unwrap();
        assert!(column_exists(&conn, "entities", "agent_affect"));
    }

    #[test]
    fn rename_no_op_when_entities_table_missing() {
        let conn = make_v02_fixture_no_entities();
        rename_entities_valence_if_present(&conn).unwrap();
        // Nothing to assert beyond "did not error" — the table doesn't exist.
    }

    #[test]
    fn rename_no_op_when_only_agent_affect_present() {
        // Already-migrated DB: only agent_affect, no valence.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE hebbian_links (a TEXT, b TEXT);\
             CREATE TABLE entities (id TEXT PRIMARY KEY, agent_affect TEXT);",
        )
        .unwrap();
        rename_entities_valence_if_present(&conn).unwrap();
        assert!(column_exists(&conn, "entities", "agent_affect"));
    }

    #[test]
    fn rename_errors_on_schema_drift_with_both_columns() {
        // Drift: someone manually added agent_affect without dropping valence.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE hebbian_links (a TEXT, b TEXT);\
             CREATE TABLE entities (\
                id TEXT PRIMARY KEY, valence REAL, agent_affect TEXT);",
        )
        .unwrap();
        let err = rename_entities_valence_if_present(&conn).unwrap_err();
        match err {
            MigrationError::DdlFailed(msg) => assert!(msg.contains("schema drift")),
            other => panic!("expected DdlFailed, got {other:?}"),
        }
    }

    // ---------- record_schema_version_v3 ----------

    #[test]
    fn version_bump_inserts_3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_VERSION_DDL).unwrap();
        record_schema_version_v3(&conn, "2026-04-27T08:00:00Z").unwrap();
        assert_eq!(read_max_version(&conn), Some(3));
    }

    #[test]
    fn version_bump_idempotent_via_insert_or_replace() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_VERSION_DDL).unwrap();
        record_schema_version_v3(&conn, "2026-04-27T08:00:00Z").unwrap();
        // Second call refreshes updated_at without erroring.
        record_schema_version_v3(&conn, "2026-04-27T09:00:00Z").unwrap();
        assert_eq!(read_max_version(&conn), Some(3));
        let updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM schema_version WHERE version = 3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, "2026-04-27T09:00:00Z");
    }

    // ---------- run_phase2 ----------

    /// Tiny stand-in for v03-graph-layer's GRAPH_DDL — just one table so we
    /// can prove the caller-supplied chunk is executed inside the txn.
    const FIXTURE_TABLE_DDL: &str = r#"
        CREATE TABLE IF NOT EXISTS graph_entities_test (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_graph_entities_test_name
            ON graph_entities_test(name);
    "#;

    #[test]
    fn phase2_creates_tables_alters_renames_and_bumps_version() {
        let mut conn = make_v02_fixture();
        run_phase2(&mut conn, FIXTURE_TABLE_DDL, "2026-04-27T08:00:00Z").unwrap();

        // Table from caller-supplied DDL.
        assert!(table_exists(&conn, "graph_entities_test").unwrap());
        // Additive columns on memories.
        assert!(column_exists(&conn, "memories", "entity_ids"));
        // Rename happened.
        assert!(!column_exists(&conn, "entities", "valence"));
        assert!(column_exists(&conn, "entities", "agent_affect"));
        // Version is 3.
        assert_eq!(read_max_version(&conn), Some(3));
    }

    #[test]
    fn phase2_idempotent_on_replay() {
        let mut conn = make_v02_fixture();
        run_phase2(&mut conn, FIXTURE_TABLE_DDL, "2026-04-27T08:00:00Z").unwrap();
        // Second run on the same DB must succeed (proves §4.4 layer 1).
        run_phase2(&mut conn, FIXTURE_TABLE_DDL, "2026-04-27T09:00:00Z").unwrap();
        assert_eq!(read_max_version(&conn), Some(3));
    }

    #[test]
    fn phase2_rolls_back_on_ddl_error_leaving_version_at_2() {
        // Pre-seed schema_version with 2 to confirm rollback truly leaves
        // it untouched.
        let mut conn = make_v02_fixture();
        conn.execute_batch(SCHEMA_VERSION_DDL).unwrap();
        conn.execute(
            "INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Inject a syntactically-broken DDL chunk to force a mid-phase failure.
        let bad_ddl = "CREATE TABLE not_valid_sql_$$$$ (";
        let err = run_phase2(&mut conn, bad_ddl, "2026-04-27T08:00:00Z").unwrap_err();
        assert!(matches!(err, MigrationError::DdlFailed(_)));

        // Critical post-condition: version stayed at 2, not 3.
        assert_eq!(read_max_version(&conn), Some(2));
        // Critical post-condition: ALTERs that preceded the bad DDL were
        // rolled back too — `memories.entity_ids` must NOT exist.
        // (FIXTURE_TABLE_DDL is fine — but with bad_ddl injected, the
        // batch fails before any ALTER would have run inside this txn.)
        assert!(!column_exists(&conn, "memories", "entity_ids"));
    }

    #[test]
    fn phase2_atomic_alter_rollback_when_version_step_fails_due_to_corrupt_pre_state() {
        // Construct a scenario where the additive ALTERs succeed but the
        // rename hits the "drift" guard (both columns present) → entire
        // batch must roll back including the ALTERs.
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE hebbian_links (a TEXT, b TEXT);\
             CREATE TABLE entities (\
                id TEXT PRIMARY KEY, valence REAL, agent_affect TEXT);",
        )
        .unwrap();

        let err = run_phase2(&mut conn, "", "2026-04-27T08:00:00Z").unwrap_err();
        assert!(matches!(err, MigrationError::DdlFailed(_)));
        // ALTERs that ran before the rename guard tripped must be rolled back.
        assert!(!column_exists(&conn, "memories", "entity_ids"));
        // schema_version row was never inserted because txn rolled back.
        let sv_exists = table_exists(&conn, "schema_version").unwrap();
        assert!(!sv_exists, "schema_version should not exist after rollback");
    }

    #[test]
    fn phase2_empty_table_ddl_still_runs_alters_and_bumps_version() {
        // Caller may legitimately pass empty DDL when all v0.3 tables
        // already exist (Fresh DB upgraded earlier through another path).
        let mut conn = make_v02_fixture();
        run_phase2(&mut conn, "", "2026-04-27T08:00:00Z").unwrap();
        assert!(column_exists(&conn, "memories", "entity_ids"));
        assert_eq!(read_max_version(&conn), Some(3));
    }

    // ---------- error contract ----------

    #[test]
    fn ddl_errors_map_to_internal_error_exit_code() {
        let conn = Connection::open_in_memory().unwrap();
        let err = apply_additive_columns(&conn).unwrap_err();
        // Per error.rs: DdlFailed → ExitCode::InternalError (=7), tag InternalError.
        assert_eq!(err.exit_code(), ExitCode::InternalError);
        assert_eq!(err.error_tag(), ErrorTag::InternalError);
    }
}
