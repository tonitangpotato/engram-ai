//! Exclusive migration lock (§3.5, GUARD-10).
//!
//! Migration must hold an application-level lock for its entire run. SQLite's
//! file-level locking serializes individual writes but does NOT prevent two
//! migrators from interleaving phases on the same DB. This module owns the
//! `migration_lock` table and the protocols around it.
//!
//! Scope (per design §3.5):
//!
//! * Lock acquisition uses `INSERT OR FAIL` — atomic, races resolved by
//!   SQLite at the constraint layer (`SQLITE_CONSTRAINT` on the singleton
//!   `id = 1`).
//! * On conflict, **stale-lock detection** classifies the holder:
//!   - same hostname + dead PID → `LockStale` (operator can `--force-unlock`)
//!   - different hostname → always `LockStale` (cannot prove remote liveness)
//!   - same hostname + live PID → `LockHeld` (another migrator IS running)
//! * Phase transitions update `migration_lock.phase` in the same transaction
//!   as the checkpoint advance — `engramai migrate --status` reads from here.
//! * Clean release deletes the row in the final transaction. Crash leaves the
//!   row; the next run's stale-lock detection cleans up.
//!
//! What this does NOT cover (per design):
//!
//! * Migration-vs-normal-traffic races. Operators must stop consumers/writers
//!   before migrating. The CLI's preflight banner is responsible for warning
//!   about this; the lock itself is migration-vs-migration only.
//! * Liveness check correctness across PID reuse. `kill(pid, 0)` answers "is
//!   *some* process with this PID alive?", not "is the *original* process
//!   alive?". Reuse is rare on the timescale of a migration; the design
//!   accepts this risk and fronts the operator's `--force-unlock` confirmation
//!   ("--i-know-what-im-doing") as the recovery escape hatch.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::MigrationError;
use crate::progress::MigrationPhase;

// ---------------------------------------------------------------------------
// DDL
// ---------------------------------------------------------------------------

/// `CREATE TABLE` for the migration lock (§3.5).
///
/// Created idempotently in Phase 0 *before* the `schema_version` handshake,
/// so even a brand-new v0.2.2 source DB can carry a lock row during its first
/// migration attempt.
pub const LOCK_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS migration_lock (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    pid          INTEGER NOT NULL,
    hostname     TEXT    NOT NULL,
    started_at   TEXT    NOT NULL,
    phase        TEXT    NOT NULL,
    tool_version TEXT    NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// LockHolder row
// ---------------------------------------------------------------------------

/// Decoded row of `migration_lock` — the migrator currently holding the lock.
///
/// Carries hostname + phase so callers (CLI status output, error messages)
/// don't need to re-query the table. Public-error mapping (§10.4) loses the
/// hostname/phase fields because `MigrationError::LockHeld { pid }` is a
/// stable contract — the rich detail stays inside this module's types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockHolder {
    pub pid: u32,
    pub hostname: String,
    pub started_at: String,
    /// Most recent phase the holder reported (`MigrationPhase::tag()` value).
    pub phase: String,
    pub tool_version: String,
}

// ---------------------------------------------------------------------------
// Liveness check abstraction (test-injectable)
// ---------------------------------------------------------------------------

/// Predicate "is this PID a live process on the local machine?".
///
/// Production uses [`real_pid_alive`] (a `kill(pid, 0)` syscall on Unix);
/// tests inject a closure to deterministically simulate "live" / "dead".
pub trait PidAliveCheck {
    fn is_alive(&self, pid: u32) -> bool;
}

impl<F> PidAliveCheck for F
where
    F: Fn(u32) -> bool,
{
    fn is_alive(&self, pid: u32) -> bool {
        (self)(pid)
    }
}

/// Production liveness check — `kill(pid, 0)` returns:
///
/// * `Ok` ⇒ process exists and we have permission to signal it (alive).
/// * `Err(EPERM)` ⇒ process exists but we lack permission (still alive).
/// * `Err(ESRCH)` ⇒ no such process (dead).
///
/// We treat anything that isn't `ESRCH` as "alive", erring on the side of
/// "don't break a real migrator" when the syscall is ambiguous.
#[cfg(unix)]
pub fn real_pid_alive(pid: u32) -> bool {
    // SAFETY: `kill` is async-signal-safe; signal 0 is the documented
    // "existence check" mode and never delivers a signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno != libc::ESRCH
}

#[cfg(not(unix))]
pub fn real_pid_alive(_pid: u32) -> bool {
    // Non-Unix support is not in scope for v0.3 — the CLI is documented as
    // Linux/macOS only. Conservatively report "alive" so we never wrongly
    // claim a remote-host lock is stale.
    true
}

// ---------------------------------------------------------------------------
// Hostname
// ---------------------------------------------------------------------------

/// Read the local hostname via `gethostname(2)`. Returns the literal string
/// `"unknown"` if the syscall fails — the lock still works (we just can't do
/// same-host stale detection, so all conflicts fall through to "foreign host"
/// → `LockStale` requiring `--force-unlock`, which is the safe behavior).
pub fn local_hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: writing into a fixed-size local buffer; length is correct.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return "unknown".to_string();
    }
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..nul])
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// MigrationLock
// ---------------------------------------------------------------------------

/// Outcome of a successful acquisition — wraps the holder row that was just
/// inserted (or re-inserted, after `--force-unlock`). Drop-on-clean-exit is
/// NOT implemented here: callers must explicitly call [`MigrationLock::release`]
/// inside their final commit transaction (per design §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredLock {
    pub holder: LockHolder,
}

/// API surface for the `migration_lock` table.
///
/// Like [`crate::checkpoint::CheckpointStore`], this is a stateless façade —
/// methods take `&Connection` so callers can pass a `&Transaction` to satisfy
/// the design's "phase update happens inside the checkpoint advance"
/// invariant (§3.5).
pub struct MigrationLock;

impl MigrationLock {
    /// Create the DDL on a fresh DB (idempotent).
    pub fn init(conn: &Connection) -> Result<(), MigrationError> {
        conn.execute_batch(LOCK_DDL).map_err(map_sqlite)?;
        Ok(())
    }

    /// Try to acquire the lock. Returns `Ok(AcquiredLock)` on success.
    ///
    /// On `SQLITE_CONSTRAINT`, classify the existing holder:
    ///
    /// 1. Same hostname + dead PID → `MigrationError::LockStale { pid }`
    ///    (operator's recovery: `--force-unlock`).
    /// 2. Different hostname → `MigrationError::LockStale { pid }` (we cannot
    ///    prove remote liveness; force-unlock with explicit confirmation is
    ///    the only escape).
    /// 3. Same hostname + live PID → `MigrationError::LockHeld { pid }` (a
    ///    real concurrent migrator).
    ///
    /// `pid` / `hostname` / `tool_version` / `now_rfc3339` are passed in so
    /// the caller controls them (and tests can pin them).
    pub fn acquire<P: PidAliveCheck>(
        conn: &Connection,
        pid: u32,
        hostname: &str,
        tool_version: &str,
        now_rfc3339: &str,
        liveness: &P,
    ) -> Result<AcquiredLock, MigrationError> {
        let phase_tag = MigrationPhase::PreFlight.tag();
        match conn.execute(
            "INSERT INTO migration_lock \
             (id, pid, hostname, started_at, phase, tool_version) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            params![pid, hostname, now_rfc3339, phase_tag, tool_version],
        ) {
            Ok(_) => {
                let holder = Self::load(conn)?.expect("lock row just inserted");
                Ok(AcquiredLock { holder })
            }
            Err(e) if is_constraint(&e) => {
                let existing = Self::load(conn)?.ok_or_else(|| {
                    // Constraint fired but the row is gone? That's a real
                    // race we don't model — surface it as a generic
                    // database error so the operator sees the rusqlite
                    // detail.
                    MigrationError::DdlFailed(format!(
                        "migration_lock constraint fired but row missing: {e}"
                    ))
                })?;
                Err(classify_conflict(&existing, hostname, liveness))
            }
            Err(e) => Err(map_sqlite(e)),
        }
    }

    /// Forcibly drop the existing lock row and re-acquire as the caller. Used
    /// only behind operator confirmation (`--force-unlock --i-know-what-im-doing`).
    /// Logged into `migration_state.provenance` is the *caller's* responsibility
    /// — this function only does the table mutation.
    pub fn force_unlock_and_acquire<P: PidAliveCheck>(
        conn: &Connection,
        pid: u32,
        hostname: &str,
        tool_version: &str,
        now_rfc3339: &str,
        liveness: &P,
    ) -> Result<AcquiredLock, MigrationError> {
        conn.execute("DELETE FROM migration_lock WHERE id = 1", [])
            .map_err(map_sqlite)?;
        Self::acquire(conn, pid, hostname, tool_version, now_rfc3339, liveness)
    }

    /// Read the current holder, if any.
    pub fn load(conn: &Connection) -> Result<Option<LockHolder>, MigrationError> {
        conn.query_row(
            "SELECT pid, hostname, started_at, phase, tool_version \
             FROM migration_lock WHERE id = 1",
            [],
            row_to_holder,
        )
        .optional()
        .map_err(map_sqlite)
    }

    /// Update the recorded phase (§3.5: this is run inside the same
    /// transaction as the checkpoint advance, so a partial failure rolls back
    /// both writes together).
    pub fn update_phase(
        conn: &Connection,
        new_phase: MigrationPhase,
    ) -> Result<(), MigrationError> {
        let n = conn
            .execute(
                "UPDATE migration_lock SET phase = ?1 WHERE id = 1",
                params![new_phase.tag()],
            )
            .map_err(map_sqlite)?;
        if n != 1 {
            return Err(MigrationError::InvariantViolated(
                "update_phase called without an active migration_lock row".to_string(),
            ));
        }
        Ok(())
    }

    /// Clean release on success / `--gate` / `--pause`. Idempotent: deleting
    /// an already-released lock is not an error (the migrator may retry
    /// release after a partial commit failure).
    pub fn release(conn: &Connection) -> Result<(), MigrationError> {
        conn.execute("DELETE FROM migration_lock WHERE id = 1", [])
            .map_err(map_sqlite)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn classify_conflict<P: PidAliveCheck>(
    existing: &LockHolder,
    our_hostname: &str,
    liveness: &P,
) -> MigrationError {
    if existing.hostname != our_hostname {
        // Different host: cannot prove liveness. Always require explicit
        // operator action (LockStale → --force-unlock --i-know-what-im-doing).
        return MigrationError::LockStale {
            pid: existing.pid,
        };
    }
    if liveness.is_alive(existing.pid) {
        MigrationError::LockHeld {
            pid: existing.pid,
        }
    } else {
        MigrationError::LockStale {
            pid: existing.pid,
        }
    }
}

fn is_constraint(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn row_to_holder(row: &Row<'_>) -> rusqlite::Result<LockHolder> {
    Ok(LockHolder {
        pid: row.get::<_, i64>(0)? as u32,
        hostname: row.get(1)?,
        started_at: row.get(2)?,
        phase: row.get(3)?,
        tool_version: row.get(4)?,
    })
}

fn map_sqlite(e: rusqlite::Error) -> MigrationError {
    // Lock-table I/O failures are unrecoverable schema problems — same
    // routing as checkpoint.rs. The catalog has no dedicated lock-IO row,
    // so DdlFailed (→ ExitCode::InternalError) is the closest stable home.
    MigrationError::DdlFailed(format!("migration_lock: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    const HOST: &str = "test-host";
    const OTHER_HOST: &str = "other-host";
    const T1: &str = "2026-04-27T10:00:00Z";
    const T2: &str = "2026-04-27T10:01:00Z";
    const TOOL: &str = "engramai-migrate/0.1.0";

    fn alive_always() -> impl PidAliveCheck {
        |_pid: u32| true
    }
    fn dead_always() -> impl PidAliveCheck {
        |_pid: u32| false
    }
    fn alive_if(set: Vec<u32>) -> impl PidAliveCheck {
        move |pid: u32| set.contains(&pid)
    }

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        MigrationLock::init(&c).unwrap();
        c
    }

    // ---- DDL --------------------------------------------------------------

    #[test]
    fn init_is_idempotent() {
        let c = Connection::open_in_memory().unwrap();
        MigrationLock::init(&c).unwrap();
        MigrationLock::init(&c).unwrap();
    }

    #[test]
    fn init_creates_lock_table() {
        let c = fresh();
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='migration_lock'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    // ---- acquire happy path ----------------------------------------------

    #[test]
    fn acquire_inserts_row_with_preflight_phase() {
        let c = fresh();
        let acq = MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        assert_eq!(acq.holder.pid, 100);
        assert_eq!(acq.holder.hostname, HOST);
        assert_eq!(acq.holder.started_at, T1);
        assert_eq!(acq.holder.phase, "Phase0");
        assert_eq!(acq.holder.tool_version, TOOL);
    }

    #[test]
    fn load_returns_none_when_unlocked() {
        let c = fresh();
        assert!(MigrationLock::load(&c).unwrap().is_none());
    }

    #[test]
    fn load_returns_holder_after_acquire() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.pid, 100);
    }

    // ---- conflict classification -----------------------------------------

    #[test]
    fn second_acquire_same_host_live_pid_is_lock_held() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        let err =
            MigrationLock::acquire(&c, 200, HOST, TOOL, T2, &alive_if(vec![100])).unwrap_err();
        match err {
            MigrationError::LockHeld { pid } => assert_eq!(pid, 100),
            other => panic!("expected LockHeld, got {:?}", other),
        }
    }

    #[test]
    fn second_acquire_same_host_dead_pid_is_lock_stale() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        let err = MigrationLock::acquire(&c, 200, HOST, TOOL, T2, &dead_always()).unwrap_err();
        match err {
            MigrationError::LockStale { pid } => assert_eq!(pid, 100),
            other => panic!("expected LockStale, got {:?}", other),
        }
    }

    #[test]
    fn second_acquire_foreign_host_is_lock_stale_regardless_of_liveness() {
        let c = fresh();
        // Acquire as OTHER_HOST first (so the row is owned by a different host).
        MigrationLock::acquire(&c, 100, OTHER_HOST, TOOL, T1, &alive_always()).unwrap();
        // Even though we report "alive_always", the foreign-host branch wins
        // before the liveness check (we cannot prove remote liveness).
        let err =
            MigrationLock::acquire(&c, 200, HOST, TOOL, T2, &alive_always()).unwrap_err();
        match err {
            MigrationError::LockStale { pid } => assert_eq!(pid, 100),
            other => panic!("expected LockStale (foreign host), got {:?}", other),
        }
    }

    #[test]
    fn failed_acquire_does_not_overwrite_existing_holder() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        let _ = MigrationLock::acquire(&c, 200, HOST, TOOL, T2, &alive_always()).unwrap_err();
        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.pid, 100, "loser must not steal the lock");
        assert_eq!(h.started_at, T1);
    }

    // ---- force unlock -----------------------------------------------------

    #[test]
    fn force_unlock_replaces_holder() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        let acq =
            MigrationLock::force_unlock_and_acquire(&c, 200, HOST, TOOL, T2, &alive_always())
                .unwrap();
        assert_eq!(acq.holder.pid, 200);
        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.pid, 200);
        assert_eq!(h.started_at, T2);
    }

    #[test]
    fn force_unlock_on_unlocked_db_just_acquires() {
        let c = fresh();
        let acq =
            MigrationLock::force_unlock_and_acquire(&c, 200, HOST, TOOL, T2, &alive_always())
                .unwrap();
        assert_eq!(acq.holder.pid, 200);
    }

    // ---- update_phase / release ------------------------------------------

    #[test]
    fn update_phase_changes_phase_only() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        MigrationLock::update_phase(&c, MigrationPhase::Backfill).unwrap();
        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.phase, "Phase4");
        // Other fields preserved:
        assert_eq!(h.pid, 100);
        assert_eq!(h.started_at, T1);
    }

    #[test]
    fn update_phase_without_lock_is_invariant_violation() {
        let c = fresh();
        let err = MigrationLock::update_phase(&c, MigrationPhase::Backfill).unwrap_err();
        match err {
            MigrationError::InvariantViolated(_) => {}
            other => panic!("expected InvariantViolated, got {:?}", other),
        }
    }

    #[test]
    fn release_drops_the_row() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        MigrationLock::release(&c).unwrap();
        assert!(MigrationLock::load(&c).unwrap().is_none());
    }

    #[test]
    fn release_when_unlocked_is_idempotent() {
        let c = fresh();
        MigrationLock::release(&c).unwrap();
        MigrationLock::release(&c).unwrap();
    }

    #[test]
    fn release_then_reacquire_works() {
        let c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();
        MigrationLock::release(&c).unwrap();
        let acq = MigrationLock::acquire(&c, 200, HOST, TOOL, T2, &alive_always()).unwrap();
        assert_eq!(acq.holder.pid, 200);
    }

    // ---- transactional release (atomicity with checkpoint advance) -------

    #[test]
    fn release_inside_rolled_back_tx_preserves_lock() {
        // §3.5 invariant: clean release happens in the SAME transaction as
        // the final checkpoint write. If that transaction rolls back, the
        // lock must remain so the next run can detect it.
        let mut c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();

        let tx = c.transaction().unwrap();
        MigrationLock::release(&tx).unwrap();
        // Sanity: gone inside the tx.
        assert!(MigrationLock::load(&tx).unwrap().is_none());
        tx.rollback().unwrap();

        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.pid, 100, "rollback restored the lock");
    }

    #[test]
    fn update_phase_inside_committed_tx_persists() {
        let mut c = fresh();
        MigrationLock::acquire(&c, 100, HOST, TOOL, T1, &alive_always()).unwrap();

        let tx = c.transaction().unwrap();
        MigrationLock::update_phase(&tx, MigrationPhase::TopicCarryForward).unwrap();
        tx.commit().unwrap();

        let h = MigrationLock::load(&c).unwrap().unwrap();
        assert_eq!(h.phase, "Phase3");
    }

    // ---- liveness helper sanity ------------------------------------------

    #[test]
    fn closure_pid_alive_check_works() {
        let f = alive_if(vec![1, 2, 3]);
        assert!(f.is_alive(1));
        assert!(!f.is_alive(99));
    }

    // Note: we don't test real_pid_alive(0) etc. — its correctness is
    // delegated to libc::kill(2), and exercising it portably is not in
    // scope (it would couple the test to the runner's PID space).

    // ---- hostname --------------------------------------------------------

    #[test]
    fn local_hostname_is_non_empty() {
        let h = local_hostname();
        assert!(!h.is_empty(), "hostname must never be empty");
        // No \0 in the middle:
        assert!(!h.contains('\0'));
    }
}
