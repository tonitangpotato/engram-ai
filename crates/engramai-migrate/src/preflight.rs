//! Phase 0 pre-flight checks for engramai v0.3 migration.
//!
//! Implements §3.1 (Phase 0 responsibility), §3.5 (lock acquisition),
//! §4.3 (`schema_version` handshake), and the disk-space pre-check from
//! `.gid/features/v03-migration/design.md`.
//!
//! Phase 0 is the **gate-keeper**: it must run before any migration work and
//! is responsible for proving — to operator-observable certainty — that:
//!
//! 1. The database is in a state migration can act on (Fresh / V02 / V03).
//! 2. Sufficient free disk space exists to safely complete the run
//!    (1.1× DB size — surfaced *before* any write so the operator can free
//!    space and retry without `--resume`).
//! 3. No other migrator holds the lock (§3.5 stale-lock detection applies).
//!
//! All phase-0 ops are idempotent and fail closed: any failure surfaces as
//! a `MigrationError` with a stable [`ExitCode`] and `ErrorTag`.

use rusqlite::{Connection, OptionalExtension};

use crate::error::MigrationError;
use crate::lock::{AcquiredLock, MigrationLock, PidAliveCheck};

/// Map a rusqlite error to a `MigrationError`.
///
/// Local copy to avoid widening `error::map_sqlite`'s visibility from the
/// crate-private helpers in `lock.rs` / `checkpoint.rs`. Phase 0 errors that
/// fall through here are infrastructure-level (DDL/PRAGMA reads on the
/// already-open connection) — surfacing them as `DdlFailed` matches their
/// semantic category (§10.4 "internal error" mapping).
fn map_sqlite(e: rusqlite::Error) -> MigrationError {
    MigrationError::DdlFailed(e.to_string())
}

/// Outcome of the `schema_version` handshake (§4.3).
///
/// The migrator's behavior diverges sharply on this value, so it is exposed
/// rather than hidden behind boolean flags:
///
/// - [`Fresh`] — empty database. No tables exist. The CLI prints
///   "no migration needed" and exits 0.
/// - [`V02`] — `schema_version = 2` (or implied by the presence of a
///   v0.2 `memories` table without a populated `schema_version`). The
///   migrator proceeds to Phase 1.
/// - [`V03`] — already at the v0.3 terminal version. Either an idempotent
///   no-op (default invocation) or a `--resume` jump to the last checkpoint.
///
/// Unsupported versions (0, 1, ≥ 4) are *not* represented here — they are
/// signalled directly via `MigrationError::UnsupportedVersion` from
/// [`detect_schema_version`].
///
/// [`Fresh`]: SchemaState::Fresh
/// [`V02`]: SchemaState::V02
/// [`V03`]: SchemaState::V03
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaState {
    /// Empty database — `memories` table absent, no `schema_version` row.
    Fresh,
    /// v0.2.2 source database — `schema_version = 2` (explicit or implied).
    V02,
    /// v0.3 terminal database — `schema_version = 3`.
    V03,
}

/// The expected schema version after migration completes.
///
/// Per §4.3, v0.3.x is the terminal version `3`; v0.3.1+ does not bump until
/// a future v0.4 migration. Pinned as a `const` so the `UnsupportedVersion`
/// error reports a stable `expected` value across the v0.3.x series.
pub const TARGET_SCHEMA_VERSION: u32 = 3;

/// Disk-space safety multiplier (§3.1 Phase 0 gate, design rationale).
///
/// Migration writes a `.bak` copy of the source DB plus all v0.3 additive
/// tables (graph entities, edges, mentions, knowledge_topics, audit). The
/// 1.1× factor is the design's documented headroom — large enough to hold
/// the backup (≈1×) plus the additive structures (~5–10%) without blowing
/// out small partitions.
pub const DISK_SPACE_MULTIPLIER_NUM: u64 = 11;
pub const DISK_SPACE_MULTIPLIER_DEN: u64 = 10;

/// Detect the source database's schema version (§4.3 handshake).
///
/// Implementation notes:
///
/// - Reads `MAX(version)` from `schema_version` (may be missing → Fresh /
///   pre-v0.2 case).
/// - If `schema_version` is missing/empty *but* the v0.2 `memories` table
///   exists, treat as v0.2.2 (per §4.3 step 2).
/// - Returns `MigrationError::UnsupportedVersion` for `v ∈ {0, 1}` or
///   `v > TARGET_SCHEMA_VERSION`. The migrator never auto-upgrades from
///   pre-v0.2 (skip-version upgrades unsupported, §4.3 version policy).
///
/// This function does **no writes**: it does not insert the implied
/// `(2, now)` row that the legacy v0.2 path eventually needs. That insert
/// is the responsibility of Phase 1's first transaction so it is part of
/// the same atomic checkpoint advance.
pub fn detect_schema_version(conn: &Connection) -> Result<SchemaState, MigrationError> {
    let schema_version_table_exists = table_exists(conn, "schema_version")?;
    let memories_table_exists = table_exists(conn, "memories")?;

    if !schema_version_table_exists {
        // No schema_version table at all.
        return Ok(if memories_table_exists {
            // v0.2.2 pre-dates schema_version tracking (§4.3 step 2).
            SchemaState::V02
        } else {
            SchemaState::Fresh
        });
    }

    // schema_version table exists — read MAX(version).
    let version: Option<i64> = conn
        .query_row(
            "SELECT MAX(version) FROM schema_version",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(map_sqlite)?;

    match version {
        None => {
            // Empty schema_version table.
            Ok(if memories_table_exists {
                SchemaState::V02
            } else {
                SchemaState::Fresh
            })
        }
        Some(2) => Ok(SchemaState::V02),
        Some(3) => Ok(SchemaState::V03),
        Some(v) if v < 0 || v > i64::from(u32::MAX) => {
            // Outside u32 range — corrupt/garbage value.
            Err(MigrationError::UnsupportedVersion {
                found: 0,
                expected: TARGET_SCHEMA_VERSION,
            })
        }
        Some(v) => Err(MigrationError::UnsupportedVersion {
            // Safe cast: bounded check above.
            found: v as u32,
            expected: TARGET_SCHEMA_VERSION,
        }),
    }
}

/// Check whether a SQLite table exists (used for `schema_version` /
/// `memories` probing during the handshake).
fn table_exists(conn: &Connection, name: &str) -> Result<bool, MigrationError> {
    conn.query_row(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![name],
        |_row| Ok::<_, rusqlite::Error>(()),
    )
    .optional()
    .map(|opt| opt.is_some())
    .map_err(map_sqlite)
}

/// Compute the required free space (bytes) for a safe migration given the
/// current DB size. Fallible only via integer overflow (impossible for any
/// DB that fits on a real disk; checked anyway for paranoia).
///
/// Uses the documented `1.1× DB size` headroom (§3.1) — encoded as
/// `DISK_SPACE_MULTIPLIER_NUM/DEN` to avoid floating-point drift in the
/// reported error message.
pub fn required_free_bytes(db_size_bytes: u64) -> u64 {
    db_size_bytes
        .saturating_mul(DISK_SPACE_MULTIPLIER_NUM)
        / DISK_SPACE_MULTIPLIER_DEN
}

/// Verify the disk has at least 1.1× DB-size free.
///
/// Pre-checks the operator's filesystem before any write so a disk-full
/// abort surfaces with a clean exit (code `10`, tag `MIG_DISK_FULL`) rather
/// than as a partial Phase 4 corruption (which would require `--resume`).
///
/// `db_size_bytes` is the size of the source DB file (caller measures it
/// via `std::fs::metadata` — kept out of this fn so tests can stub it).
/// `available_bytes` is the free space on the partition holding the DB
/// (caller measures via `statvfs` / equivalent — same rationale).
pub fn check_disk_space(
    db_size_bytes: u64,
    available_bytes: u64,
) -> Result<(), MigrationError> {
    let needed = required_free_bytes(db_size_bytes);
    if available_bytes < needed {
        return Err(MigrationError::InsufficientDiskSpace {
            needed,
            available: available_bytes,
        });
    }
    Ok(())
}

/// Result of a successful Phase 0 pre-flight.
///
/// Carries forward the two pieces of state the rest of the migrator needs:
/// the detected schema state (which dictates the next phase's behavior) and
/// the acquired lock holder (release deadline = end of run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightOutcome {
    pub state: SchemaState,
    pub lock: AcquiredLock,
}

/// Inputs for a Phase 0 pre-flight run.
///
/// Grouped into a struct so call-sites at the CLI layer (which collect this
/// state from argv + `std::fs` + `gethostname` + `getpid`) compose cleanly,
/// and so unit tests can build a `PreflightInputs` without touching the
/// real filesystem or process table.
#[derive(Debug, Clone)]
pub struct PreflightInputs<'a> {
    /// Source DB size in bytes (caller reads from `std::fs::metadata`).
    pub db_size_bytes: u64,
    /// Free space on DB's partition (caller reads via `statvfs`).
    pub available_bytes: u64,
    /// Current process PID (for the lock's `pid` field).
    pub pid: u32,
    /// Local hostname (for the lock's `hostname` field).
    pub hostname: &'a str,
    /// Migration tool semver (for the lock's `tool_version` field).
    pub tool_version: &'a str,
    /// RFC3339 timestamp captured at start (for the lock's `started_at`).
    pub now_rfc3339: &'a str,
}

/// Run the full Phase 0 sequence: lock-table init → disk-space check →
/// schema detection → lock acquisition.
///
/// Order matters:
///
/// 1. **Init the lock table first** — even on a fresh DB the `migration_lock`
///    DDL is needed before acquisition.
/// 2. **Disk-space check next** — before lock acquisition so a DB that
///    can't fit a backup never advertises a held lock to other tools.
/// 3. **Schema detection** — done before lock acquisition is committed so a
///    `Fresh` or `V03`-already-done detection can short-circuit without
///    holding a lock the caller never needed (CLI handles "Fresh → exit 0"
///    and "V03 + no --resume → exit 0" without needing the lock).
/// 4. **Lock acquisition** — last step inside Phase 0; subsequent phases
///    inherit the lock and update `migration_lock.phase` at each
///    transition (§3.5).
///
/// `liveness` is the `PidAliveCheck` impl used during stale-lock detection
/// (§3.5 step 2) — wired through so tests can pin alive/dead PIDs.
pub fn run_preflight<P: PidAliveCheck>(
    conn: &Connection,
    inputs: &PreflightInputs<'_>,
    liveness: &P,
) -> Result<PreflightOutcome, MigrationError> {
    // (1) Lock-table DDL is idempotent (`CREATE TABLE IF NOT EXISTS`), so it
    // is safe to run unconditionally on every preflight invocation, including
    // on a Fresh DB where the lock will never be acquired.
    MigrationLock::init(conn)?;

    // (2) Disk space — surface failure *before* lock acquisition so we don't
    // strand a lock row on a DB the caller cannot complete a migration on.
    check_disk_space(inputs.db_size_bytes, inputs.available_bytes)?;

    // (3) Schema-version handshake — also done *before* lock acquisition so
    // the "nothing to do" cases (Fresh, V03-clean) don't churn the lock
    // table. The CLI inspects `outcome.state` and may skip the rest of the
    // pipeline.
    let state = detect_schema_version(conn)?;

    // (4) Lock acquisition — failures here surface as LockHeld / LockStale,
    // both of which carry a precise enough operator action that no further
    // Phase 0 work is meaningful.
    let lock = MigrationLock::acquire(
        conn,
        inputs.pid,
        inputs.hostname,
        inputs.tool_version,
        inputs.now_rfc3339,
        liveness,
    )?;

    Ok(PreflightOutcome { state, lock })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ExitCode;
    use crate::lock::LOCK_DDL;

    /// Test stub: every PID is dead. Used to verify the "stale lock"
    /// classification path.
    struct AllDead;
    impl PidAliveCheck for AllDead {
        fn is_alive(&self, _pid: u32) -> bool {
            false
        }
    }

    /// Test stub: every PID is alive — exercises the "real concurrent
    /// migrator" path.
    struct AllAlive;
    impl PidAliveCheck for AllAlive {
        fn is_alive(&self, _pid: u32) -> bool {
            true
        }
    }

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Always seed the lock DDL; preflight is allowed to call
        // `MigrationLock::init` again — it is idempotent.
        conn.execute_batch(LOCK_DDL).unwrap();
        conn
    }

    fn seed_v02_db(conn: &Connection, with_schema_version: bool) {
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT)",
        )
        .unwrap();
        if with_schema_version {
            conn.execute_batch(
                "CREATE TABLE schema_version (\
                 version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
                 INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }
    }

    fn seed_v03_db(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
             version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (3, '2026-04-27T00:00:00Z');",
        )
        .unwrap();
    }

    // ---------- detect_schema_version ----------

    #[test]
    fn detect_fresh_db() {
        let conn = fresh_conn();
        // No memories, no schema_version table — only the lock table seed.
        assert_eq!(detect_schema_version(&conn).unwrap(), SchemaState::Fresh);
    }

    #[test]
    fn detect_v02_explicit_version() {
        let conn = fresh_conn();
        seed_v02_db(&conn, true);
        assert_eq!(detect_schema_version(&conn).unwrap(), SchemaState::V02);
    }

    #[test]
    fn detect_v02_implied_by_memories_only() {
        // §4.3 step 2: memories table exists but schema_version table is
        // absent — v0.2.2 pre-dates the tracking table.
        let conn = fresh_conn();
        seed_v02_db(&conn, false);
        assert_eq!(detect_schema_version(&conn).unwrap(), SchemaState::V02);
    }

    #[test]
    fn detect_v02_implied_by_empty_schema_version_table() {
        // schema_version table exists but is empty + memories present
        // → v0.2.2 (treat MAX(version)=NULL as missing tracking).
        let conn = fresh_conn();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
             version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);",
        )
        .unwrap();
        assert_eq!(detect_schema_version(&conn).unwrap(), SchemaState::V02);
    }

    #[test]
    fn detect_v03() {
        let conn = fresh_conn();
        seed_v03_db(&conn);
        assert_eq!(detect_schema_version(&conn).unwrap(), SchemaState::V03);
    }

    #[test]
    fn detect_unsupported_v01() {
        let conn = fresh_conn();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
             version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (1, '2024-01-01T00:00:00Z');",
        )
        .unwrap();
        let err = detect_schema_version(&conn).unwrap_err();
        match err {
            MigrationError::UnsupportedVersion { found, expected } => {
                assert_eq!(found, 1);
                assert_eq!(expected, TARGET_SCHEMA_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn detect_unsupported_future_version() {
        let conn = fresh_conn();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
             version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (99, '2030-01-01T00:00:00Z');",
        )
        .unwrap();
        let err = detect_schema_version(&conn).unwrap_err();
        assert!(matches!(
            err,
            MigrationError::UnsupportedVersion { found: 99, .. }
        ));
        // Stable error contract: exit code 4 / MIG_UNSUPPORTED_VERSION.
        assert_eq!(err.exit_code(), ExitCode::UnsupportedVersion);
    }

    // ---------- disk-space ----------

    #[test]
    fn required_free_bytes_uses_11_over_10() {
        // 100 bytes → 110 needed.
        assert_eq!(required_free_bytes(100), 110);
        // 0 → 0 (degenerate but valid: empty DB needs no headroom).
        assert_eq!(required_free_bytes(0), 0);
        // Large values do not overflow (saturating math).
        assert_eq!(required_free_bytes(1_000_000), 1_100_000);
    }

    #[test]
    fn check_disk_space_passes_with_headroom() {
        check_disk_space(1_000, 2_000).unwrap();
    }

    #[test]
    fn check_disk_space_passes_at_exact_required() {
        check_disk_space(1_000, 1_100).unwrap();
    }

    #[test]
    fn check_disk_space_fails_just_below_required() {
        let err = check_disk_space(1_000, 1_099).unwrap_err();
        match err {
            MigrationError::InsufficientDiskSpace { needed, available } => {
                assert_eq!(needed, 1_100);
                assert_eq!(available, 1_099);
            }
            other => panic!("expected InsufficientDiskSpace, got {other:?}"),
        }
    }

    #[test]
    fn check_disk_space_error_maps_to_disk_full_exit_code() {
        let err = check_disk_space(1_000, 0).unwrap_err();
        assert_eq!(err.exit_code(), ExitCode::DiskFull);
    }

    // ---------- run_preflight (integration) ----------

    fn baseline_inputs<'a>() -> PreflightInputs<'a> {
        PreflightInputs {
            db_size_bytes: 1_000,
            available_bytes: 10_000,
            pid: 12_345,
            hostname: "test-host",
            tool_version: "0.3.0-test",
            now_rfc3339: "2026-04-27T07:00:00Z",
        }
    }

    #[test]
    fn run_preflight_fresh_db_acquires_lock() {
        let conn = Connection::open_in_memory().unwrap();
        // Note: do NOT pre-seed the lock DDL — preflight must init it itself.
        let outcome = run_preflight(&conn, &baseline_inputs(), &AllDead).unwrap();
        assert_eq!(outcome.state, SchemaState::Fresh);
        assert_eq!(outcome.lock.holder.pid, 12_345);
        assert_eq!(outcome.lock.holder.hostname, "test-host");
    }

    #[test]
    fn run_preflight_v02_db_acquires_lock_and_reports_v02() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v02_db(&conn, true);
        let outcome = run_preflight(&conn, &baseline_inputs(), &AllDead).unwrap();
        assert_eq!(outcome.state, SchemaState::V02);
    }

    #[test]
    fn run_preflight_v03_db_acquires_lock_and_reports_v03() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v03_db(&conn);
        let outcome = run_preflight(&conn, &baseline_inputs(), &AllDead).unwrap();
        assert_eq!(outcome.state, SchemaState::V03);
    }

    #[test]
    fn run_preflight_disk_full_does_not_acquire_lock() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v02_db(&conn, true);
        let inputs = PreflightInputs {
            db_size_bytes: 1_000_000,
            available_bytes: 100, // way too small
            ..baseline_inputs()
        };
        let err = run_preflight(&conn, &inputs, &AllDead).unwrap_err();
        assert_eq!(err.exit_code(), ExitCode::DiskFull);
        // Critical: lock row must NOT have been inserted.
        let holder = MigrationLock::load(&conn).unwrap();
        assert!(
            holder.is_none(),
            "lock should not be acquired when disk-space check fails"
        );
    }

    #[test]
    fn run_preflight_unsupported_version_does_not_acquire_lock() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
             version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (1, '2024-01-01T00:00:00Z');",
        )
        .unwrap();
        let err = run_preflight(&conn, &baseline_inputs(), &AllDead).unwrap_err();
        assert!(matches!(err, MigrationError::UnsupportedVersion { .. }));
        // Critical: lock row must NOT have been inserted.
        let holder = MigrationLock::load(&conn).unwrap();
        assert!(holder.is_none());
    }

    #[test]
    fn run_preflight_lock_held_by_live_migrator_fails() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v02_db(&conn, true);
        // First call seeds the lock.
        run_preflight(&conn, &baseline_inputs(), &AllAlive).unwrap();
        // Second call (same conn → same DB) sees the existing live holder.
        let inputs2 = PreflightInputs {
            pid: 99_999,
            ..baseline_inputs()
        };
        let err = run_preflight(&conn, &inputs2, &AllAlive).unwrap_err();
        match err {
            MigrationError::LockHeld { pid } => assert_eq!(pid, 12_345),
            other => panic!("expected LockHeld, got {other:?}"),
        }
    }

    #[test]
    fn run_preflight_stale_lock_classified_correctly() {
        let conn = Connection::open_in_memory().unwrap();
        seed_v02_db(&conn, true);
        // Seed a "previous run" lock as if a process died.
        run_preflight(&conn, &baseline_inputs(), &AllAlive).unwrap();
        // Same hostname, but the alive-checker now reports the PID dead.
        let inputs2 = PreflightInputs {
            pid: 99_999,
            ..baseline_inputs()
        };
        let err = run_preflight(&conn, &inputs2, &AllDead).unwrap_err();
        match err {
            MigrationError::LockStale { pid } => assert_eq!(pid, 12_345),
            other => panic!("expected LockStale, got {other:?}"),
        }
    }

    #[test]
    fn run_preflight_idempotent_lock_init() {
        // Calling preflight on a DB that already has a lock table seeded
        // (e.g. a re-entry after a partial init) must not error.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(LOCK_DDL).unwrap();
        seed_v02_db(&conn, true);
        let outcome = run_preflight(&conn, &baseline_inputs(), &AllDead).unwrap();
        assert_eq!(outcome.state, SchemaState::V02);
    }
}
