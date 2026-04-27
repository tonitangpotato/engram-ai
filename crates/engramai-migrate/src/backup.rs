//! Phase 1 — pre-migration backup (§8 of `.gid/features/v03-migration/design.md`).
//!
//! Phase 1 produces `{db_path}.pre-v03.bak` via SQLite's `VACUUM INTO`, which
//! gives a transactionally-consistent snapshot even on a live database. The
//! file is verified for integrity *and* for "schema-version equivalence"
//! before this phase's gate passes — if any check fails the source database
//! is **untouched** (no DDL has run yet), and the operator can re-run after
//! freeing space / fixing perms.
//!
//! Operator flags surfaced through this module:
//!
//! - `--no-backup` (handled in [`maybe_write_backup`]) — skip Phase 1, emit
//!   the "no rollback possible" warning. Implemented by passing
//!   [`BackupMode::Skip`] from the CLI layer.
//! - `--force-backup-overwrite` — allow overwriting an existing `.bak`
//!   file. Implemented by [`BackupMode::Force`].
//!
//! Errors funnel through [`MigrationError::BackupFailed`] (exit code 5,
//! tag `MIG_BACKUP_FAILED`). The `source: io::Error` is preserved so the
//! operator sees the underlying cause (`No space left on device`,
//! `Permission denied`, etc.).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::error::MigrationError;

/// Backup-file extension appended to the source DB path (§8.1).
///
/// The design pins this string ("pre-v03.bak") so that operators, CI
/// drills, and the rollback runbook all reference the *same* literal name.
/// Changing it is a breaking change to the operator contract.
pub const BACKUP_SUFFIX: &str = "pre-v03.bak";

/// Disk-space safety multiplier (§8.1, mirror of `preflight::DISK_SPACE_*`).
///
/// Re-checked at Phase 1 because the Phase 0 check may be minutes stale —
/// a parallel process could have eaten the partition.
pub const DISK_SPACE_MULTIPLIER_NUM: u64 = 11;
pub const DISK_SPACE_MULTIPLIER_DEN: u64 = 10;

/// Mode controlling Phase 1 behaviour.
///
/// CLI flags map to variants:
///
/// | Flag                         | Variant      |
/// |------------------------------|--------------|
/// | (default)                    | `Write`      |
/// | `--force-backup-overwrite`   | `Force`      |
/// | `--no-backup`                | `Skip`       |
///
/// `Skip` is the operator's "I have external snapshots" escape hatch
/// (§8.2). The CLI layer is responsible for the 5-second grace period
/// banner + provenance recording — this module just honours the
/// instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupMode {
    /// Default: write the backup. Refuse to overwrite an existing file.
    Write,
    /// `--force-backup-overwrite`: allow overwriting an existing `.bak`.
    Force,
    /// `--no-backup`: skip Phase 1 entirely.
    Skip,
}

/// Outcome of a Phase 1 invocation.
///
/// Carries forward the backup path (used by the CLI to surface in the
/// summary log + by Phase 2's checkpoint provenance blob), or an explicit
/// "skipped" flag when `--no-backup` was passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupOutcome {
    /// Backup file written and verified.
    Written(PathBuf),
    /// `--no-backup` honoured. No file produced.
    Skipped,
}

/// Compute the backup path for a given source database path.
///
/// `/path/to/engram.db` → `/path/to/engram.db.pre-v03.bak`.
///
/// We do *not* use [`Path::with_extension`] because that replaces the
/// extension (`.db` → `.bak`). The design (§8.1) says the backup keeps the
/// original name and *appends* `.pre-v03.bak`, so re-runs/operator scripts
/// can grep for the literal `pre-v03.bak` suffix.
pub fn backup_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_owned();
    s.push(".");
    s.push(BACKUP_SUFFIX);
    PathBuf::from(s)
}

/// Compute the required free space (bytes) for a safe backup write,
/// applying the §8.1 1.1× headroom rule.
///
/// Saturating-mul so a degenerate caller (huge DB on a 32-bit usize host)
/// does not wrap silently.
pub fn required_free_bytes(db_size_bytes: u64) -> u64 {
    db_size_bytes.saturating_mul(DISK_SPACE_MULTIPLIER_NUM) / DISK_SPACE_MULTIPLIER_DEN
}

/// Phase 1 free-space gate. Re-runs the §8.1 disk-space check immediately
/// before the `VACUUM INTO`.
///
/// Returns `BackupFailed { path: backup_path, source: ENOSPC }` so the
/// operator-visible error semantically matches the failure they would
/// have seen if the `VACUUM INTO` itself had run out of space mid-write.
fn check_free_space(
    backup_path: &Path,
    db_size_bytes: u64,
    available_bytes: u64,
) -> Result<(), MigrationError> {
    let needed = required_free_bytes(db_size_bytes);
    if available_bytes < needed {
        return Err(MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::OutOfMemory, // closest stable variant for ENOSPC pre-flight
                format!(
                    "insufficient free disk space: need {needed} bytes (1.1× DB), \
                     have {available_bytes} bytes"
                ),
            ),
        });
    }
    Ok(())
}

/// Trait abstracting "is path X present on disk" so tests can simulate an
/// existing backup file without touching the filesystem twice.
///
/// In production the [`RealFs`] impl is used; tests inject a stub that
/// drives the existence check from an in-memory flag.
pub trait FsExists {
    fn exists(&self, path: &Path) -> bool;
}

/// Production [`FsExists`] impl backed by `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealFs;

impl FsExists for RealFs {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// Inputs to a Phase 1 backup write.
///
/// `db_size_bytes` and `available_bytes` come from the caller (CLI →
/// `std::fs::metadata` + `statvfs`) so the module stays free of
/// platform-specific `statvfs` plumbing and is unit-testable from
/// `Connection::open_in_memory()`.
#[derive(Debug, Clone)]
pub struct BackupInputs<'a> {
    /// Path to the source DB on disk. Must be the same path the
    /// `Connection` was opened with — used both for naming the `.bak` file
    /// and for the operator-facing error message.
    pub db_path: &'a Path,
    /// Source DB size in bytes (caller reads from `std::fs::metadata`).
    pub db_size_bytes: u64,
    /// Free space on backup's partition (caller reads via `statvfs`).
    pub available_bytes: u64,
    /// Phase 1 mode (Write / Force / Skip).
    pub mode: BackupMode,
}

/// Phase 1 entry point — honours `--no-backup` opt-out, otherwise writes
/// and verifies the backup file.
pub fn maybe_write_backup<F: FsExists>(
    conn: &Connection,
    inputs: &BackupInputs<'_>,
    fs_exists: &F,
) -> Result<BackupOutcome, MigrationError> {
    if inputs.mode == BackupMode::Skip {
        return Ok(BackupOutcome::Skipped);
    }
    let path = write_backup(conn, inputs, fs_exists)?;
    Ok(BackupOutcome::Written(path))
}

/// Write `{db_path}.pre-v03.bak` and verify it (§8.1).
///
/// **Steps and rationale:**
///
/// 1. **Refuse to overwrite an existing backup** unless `BackupMode::Force`.
///    A surviving `.bak` from a previous run is the rollback artifact and
///    silently overwriting it would destroy the operator's escape route.
///    Force is the explicit "I know what I'm doing" lever.
/// 2. **Re-check free space.** Phase 0 already gated this, but minutes may
///    have passed; another tenant on the host could have filled the
///    partition. Cheaper to fail here than mid-`VACUUM INTO`.
/// 3. **VACUUM INTO.** Single-statement atomic snapshot. The path is
///    SQL-quoted with single-quote doubling per the SQLite docs; the
///    backup path lives under the same directory as the source so a
///    user-controlled DB name cannot inject a different write target.
/// 4. **Cleanup on partial write.** If the `VACUUM INTO` errors *after*
///    creating the file (rare — generally surfaces as the SQL error itself),
///    we attempt a best-effort `remove_file` so the next run is not
///    blocked by step 1's overwrite refusal.
/// 5. **Verify** (§8.1 post-conditions 1-4):
///    - file exists
///    - file size ≥ original size
///    - opens cleanly as a SQLite database
///    - `PRAGMA integrity_check` returns `ok`
///    - SHA-256 of the source's `schema_version` rows equals the
///      backup's (skipped — with explicit log — if neither side has a
///      `schema_version` table; v0.2.2 pre-dates the table, see §4.3)
fn write_backup<F: FsExists>(
    conn: &Connection,
    inputs: &BackupInputs<'_>,
    fs_exists: &F,
) -> Result<PathBuf, MigrationError> {
    let backup_path = backup_path_for(inputs.db_path);

    // (1) Existing-backup refusal (unless Force).
    if fs_exists.exists(&backup_path) {
        match inputs.mode {
            BackupMode::Write => {
                return Err(MigrationError::BackupFailed {
                    path: backup_path.clone(),
                    source: io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "backup file already exists; pass --force-backup-overwrite \
                         to replace it (current file is your rollback artifact)",
                    ),
                });
            }
            BackupMode::Force => {
                // Best-effort removal — if this fails the VACUUM INTO will
                // report its own error and we surface that instead.
                if let Err(e) = fs::remove_file(&backup_path) {
                    return Err(MigrationError::BackupFailed {
                        path: backup_path.clone(),
                        source: e,
                    });
                }
            }
            BackupMode::Skip => unreachable!("Skip handled in maybe_write_backup"),
        }
    }

    // (2) Disk-space re-check.
    check_free_space(&backup_path, inputs.db_size_bytes, inputs.available_bytes)?;

    // (3) VACUUM INTO.
    let quoted = backup_path.to_string_lossy().replace('\'', "''");
    let stmt = format!("VACUUM INTO '{quoted}'");
    if let Err(e) = conn.execute_batch(&stmt) {
        // (4) Cleanup on partial write — best-effort, ignore secondary errors.
        let _ = fs::remove_file(&backup_path);
        return Err(MigrationError::BackupFailed {
            path: backup_path,
            source: io::Error::other(e.to_string()),
        });
    }

    // (5) Verify.
    verify_backup(conn, &backup_path, inputs.db_size_bytes)?;

    Ok(backup_path)
}

/// Verify a freshly-written backup against §8.1 post-conditions.
fn verify_backup(
    source_conn: &Connection,
    backup_path: &Path,
    source_size_bytes: u64,
) -> Result<(), MigrationError> {
    // 5a. exists + non-empty + size sanity.
    let meta = fs::metadata(backup_path).map_err(|e| MigrationError::BackupFailed {
        path: backup_path.to_path_buf(),
        source: e,
    })?;
    let backup_size = meta.len();
    if backup_size == 0 {
        return Err(MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "backup file is empty (VACUUM INTO produced no data)",
            ),
        });
    }
    if backup_size < source_size_bytes {
        return Err(MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(format!(
                "backup smaller than source: backup={backup_size} bytes, \
                 source={source_size_bytes} bytes — refusing to trust it"
            )),
        });
    }

    // 5b. opens cleanly + integrity_check.
    let backup_conn = Connection::open(backup_path).map_err(|e| MigrationError::BackupFailed {
        path: backup_path.to_path_buf(),
        source: io::Error::other(format!("backup did not open as SQLite: {e}")),
    })?;
    let integrity: String = backup_conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(format!("integrity_check query failed: {e}")),
        })?;
    if integrity != "ok" {
        return Err(MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(format!(
                "integrity_check returned {integrity:?} (expected \"ok\")"
            )),
        });
    }

    // 5c. schema_version equivalence (skip when both sides lack the table —
    // v0.2.2 pre-dates schema_version per §4.3 step 2).
    let source_digest = schema_version_digest(source_conn).map_err(|e| {
        MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(format!("source schema_version digest failed: {e}")),
        }
    })?;
    let backup_digest = schema_version_digest(&backup_conn).map_err(|e| {
        MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(format!("backup schema_version digest failed: {e}")),
        }
    })?;
    if source_digest != backup_digest {
        return Err(MigrationError::BackupFailed {
            path: backup_path.to_path_buf(),
            source: io::Error::other(
                "schema_version digest mismatch between source and backup \
                 (VACUUM INTO produced a non-equivalent copy)"
                    .to_string(),
            ),
        });
    }

    Ok(())
}

/// Compute a deterministic SHA-256 digest over the contents of the
/// `schema_version` table (rows ordered by `version`). Returns `None` if
/// the table is missing.
///
/// Used as a sanity check that the source and backup contain the same
/// schema-tracking state. The digest format (`"v={version},u={updated_at}\n"`
/// per row) is internal — callers should only compare digests for equality.
pub fn schema_version_digest(conn: &Connection) -> Result<Option<String>, rusqlite::Error> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(None);
    }

    let mut stmt =
        conn.prepare("SELECT version, updated_at FROM schema_version ORDER BY version ASC")?;
    let mut hasher = Sha256::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let v: i64 = row.get(0)?;
        let u: String = row.get(1)?;
        hasher.update(format!("v={v},u={u}\n").as_bytes());
    }
    Ok(Some(hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorTag, ExitCode};
    use std::cell::Cell;
    use tempfile::TempDir;

    /// In-memory FsExists stub. The real filesystem still gets touched by
    /// the SQLite `VACUUM INTO` (we test the happy path against a real
    /// tempdir); this stub is for branch-specific tests of the
    /// "backup-already-exists" path that don't want to call `VACUUM INTO`
    /// at all.
    struct ExistsStub(Cell<bool>);
    impl ExistsStub {
        fn new(initial: bool) -> Self {
            Self(Cell::new(initial))
        }
    }
    impl FsExists for ExistsStub {
        fn exists(&self, _path: &Path) -> bool {
            self.0.get()
        }
    }

    fn make_v02_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT);\
             CREATE TABLE schema_version (\
                version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');\
             INSERT INTO memories VALUES ('m1', 'hello'), ('m2', 'world');",
        )
        .unwrap();
        conn
    }

    fn db_size(path: &Path) -> u64 {
        fs::metadata(path).unwrap().len()
    }

    // ---------- backup_path_for ----------

    #[test]
    fn backup_path_appends_suffix() {
        let p = backup_path_for(Path::new("/data/engram.db"));
        assert_eq!(p, PathBuf::from("/data/engram.db.pre-v03.bak"));
    }

    #[test]
    fn backup_path_handles_no_extension() {
        let p = backup_path_for(Path::new("/data/engram"));
        assert_eq!(p, PathBuf::from("/data/engram.pre-v03.bak"));
    }

    #[test]
    fn backup_path_handles_dotted_filename() {
        // foo.bar.db → foo.bar.db.pre-v03.bak (the suffix is appended,
        // not substituted — operator scripts grep for ".pre-v03.bak").
        let p = backup_path_for(Path::new("/data/foo.bar.db"));
        assert_eq!(p, PathBuf::from("/data/foo.bar.db.pre-v03.bak"));
    }

    // ---------- required_free_bytes ----------

    #[test]
    fn required_free_uses_11_over_10() {
        assert_eq!(required_free_bytes(0), 0);
        assert_eq!(required_free_bytes(100), 110);
        assert_eq!(required_free_bytes(1_000_000), 1_100_000);
    }

    #[test]
    fn required_free_saturates_on_overflow() {
        // Pathological caller; we just need to confirm we don't panic.
        let _ = required_free_bytes(u64::MAX);
    }

    // ---------- check_free_space ----------

    #[test]
    fn free_space_passes_with_headroom() {
        check_free_space(Path::new("/x"), 1_000, 2_000).unwrap();
    }

    #[test]
    fn free_space_passes_at_exact_required() {
        check_free_space(Path::new("/x"), 1_000, 1_100).unwrap();
    }

    #[test]
    fn free_space_fails_just_below_required() {
        let err = check_free_space(Path::new("/x.bak"), 1_000, 1_099).unwrap_err();
        match err {
            MigrationError::BackupFailed { path, source } => {
                assert_eq!(path, PathBuf::from("/x.bak"));
                let msg = format!("{source}");
                assert!(msg.contains("1100"), "msg should mention 1100 bytes: {msg}");
            }
            other => panic!("expected BackupFailed, got {other:?}"),
        }
    }

    #[test]
    fn free_space_failure_carries_disk_full_semantics() {
        let err = check_free_space(Path::new("/x"), 1_000, 0).unwrap_err();
        // Routing: a backup-time disk failure surfaces as BackupFailed
        // (not InsufficientDiskSpace) because Phase 1 owns it. The exit
        // code is 5 / MIG_BACKUP_FAILED.
        assert_eq!(err.exit_code(), ExitCode::BackupFailed);
        assert_eq!(err.error_tag(), ErrorTag::BackupFailed);
    }

    // ---------- write_backup happy paths ----------

    #[test]
    fn write_backup_produces_verified_file() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("engram.db");
        let conn = make_v02_db(&db_path);
        let size = db_size(&db_path);
        let inputs = BackupInputs {
            db_path: &db_path,
            db_size_bytes: size,
            available_bytes: size * 100,
            mode: BackupMode::Write,
        };

        let outcome = maybe_write_backup(&conn, &inputs, &RealFs).unwrap();
        let backup_path = match outcome {
            BackupOutcome::Written(p) => p,
            BackupOutcome::Skipped => panic!("expected Written"),
        };
        assert!(backup_path.exists());
        assert!(backup_path.to_string_lossy().ends_with(".pre-v03.bak"));

        // Verify backup content is openable + has the data.
        let bconn = Connection::open(&backup_path).unwrap();
        let count: i64 = bconn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let v: i64 = bconn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn write_backup_skip_returns_skipped_without_file() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("engram.db");
        let conn = make_v02_db(&db_path);
        let inputs = BackupInputs {
            db_path: &db_path,
            db_size_bytes: 1,
            available_bytes: 1,
            mode: BackupMode::Skip,
        };

        let outcome = maybe_write_backup(&conn, &inputs, &RealFs).unwrap();
        assert_eq!(outcome, BackupOutcome::Skipped);
        assert!(!backup_path_for(&db_path).exists());
    }

    // ---------- write_backup overwrite policy ----------

    #[test]
    fn write_backup_refuses_existing_backup_in_write_mode() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("engram.db");
        let conn = make_v02_db(&db_path);
        // Pretend the .bak already exists via the stub — keeps this test
        // pure (no need to seed a real .bak file just to check the gate).
        let stub = ExistsStub::new(true);
        let inputs = BackupInputs {
            db_path: &db_path,
            db_size_bytes: db_size(&db_path),
            available_bytes: u64::MAX / 2,
            mode: BackupMode::Write,
        };
        let err = maybe_write_backup(&conn, &inputs, &stub).unwrap_err();
        match err {
            MigrationError::BackupFailed { path, source } => {
                assert_eq!(path, backup_path_for(&db_path));
                assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected BackupFailed/AlreadyExists, got {other:?}"),
        }
    }

    #[test]
    fn write_backup_force_overwrites_existing_backup() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("engram.db");
        let conn = make_v02_db(&db_path);
        // Seed a real stale .bak file so Force has something to remove.
        let backup_path = backup_path_for(&db_path);
        fs::write(&backup_path, b"stale junk").unwrap();
        assert!(backup_path.exists());

        let inputs = BackupInputs {
            db_path: &db_path,
            db_size_bytes: db_size(&db_path),
            available_bytes: u64::MAX / 2,
            mode: BackupMode::Force,
        };
        let outcome = maybe_write_backup(&conn, &inputs, &RealFs).unwrap();
        assert_eq!(outcome, BackupOutcome::Written(backup_path.clone()));

        // Backup is now the SQLite snapshot, not stale junk.
        let bconn = Connection::open(&backup_path).unwrap();
        let count: i64 = bconn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    // ---------- write_backup disk-space gate ----------

    #[test]
    fn write_backup_aborts_when_disk_too_small() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("engram.db");
        let conn = make_v02_db(&db_path);
        let size = db_size(&db_path);
        let inputs = BackupInputs {
            db_path: &db_path,
            db_size_bytes: size,
            available_bytes: 0, // force the gate to trip
            mode: BackupMode::Write,
        };
        let err = maybe_write_backup(&conn, &inputs, &RealFs).unwrap_err();
        assert!(matches!(err, MigrationError::BackupFailed { .. }));
        // Critical: backup file must NOT have been written.
        assert!(!backup_path_for(&db_path).exists());
    }

    // ---------- schema_version_digest ----------

    #[test]
    fn schema_version_digest_returns_none_when_table_absent() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(schema_version_digest(&conn).unwrap(), None);
    }

    #[test]
    fn schema_version_digest_stable_for_same_content() {
        let c1 = Connection::open_in_memory().unwrap();
        c1.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z'),\
                                                (3, '2026-04-27T00:00:00Z');",
        )
        .unwrap();
        let c2 = Connection::open_in_memory().unwrap();
        c2.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (3, '2026-04-27T00:00:00Z'),\
                                                (2, '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        // ORDER BY version → same digest regardless of insert order.
        let d1 = schema_version_digest(&c1).unwrap();
        let d2 = schema_version_digest(&c2).unwrap();
        assert_eq!(d1, d2);
        assert!(d1.is_some());
    }

    #[test]
    fn schema_version_digest_distinguishes_different_content() {
        let c1 = Connection::open_in_memory().unwrap();
        c1.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        let c2 = Connection::open_in_memory().unwrap();
        c2.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (3, '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        assert_ne!(
            schema_version_digest(&c1).unwrap(),
            schema_version_digest(&c2).unwrap()
        );
    }

    // ---------- error contract sanity ----------

    #[test]
    fn backup_failures_map_to_exit_code_5() {
        let err = check_free_space(Path::new("/x"), 100, 0).unwrap_err();
        assert_eq!(err.exit_code() as u8, 5);
    }
}
