//! Error model for engramai v0.3 migrations.
//!
//! Implements the abort classes (§10.1), exit codes (§9.1 / §10.4), and the
//! user-facing error-tag taxonomy (§10.4) from
//! `.gid/features/v03-migration/design.md`.
//!
//! Stability contract (per design §10.4):
//! - Exit codes and error tags are stable within a minor-version series.
//! - Adding new variants/tags is a minor-version change.
//! - Renumbering or renaming is a major-version change.

use std::path::PathBuf;

use thiserror::Error;

/// Process exit codes emitted by the `engramai migrate` CLI.
///
/// Mirrors §10.4 of the design document. Each `MigrationError` variant maps
/// to exactly one `ExitCode` via [`MigrationError::exit_code`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// 0 — Migration completed successfully.
    Success = 0,
    /// 1 — Unexpected error (bug). Stack trace logged.
    InternalError = 1,
    /// 2 — Cooperative SIGINT pause; resumable.
    Paused = 2,
    /// 3 — Phase 4 finished with `records_failed > 0`.
    FailuresPresent = 3,
    /// 4 — Source DB `schema_version` is not 2.
    UnsupportedVersion = 4,
    /// 5 — Phase 1 backup could not be written / verified.
    BackupFailed = 5,
    /// 6 — Stopped at `--gate <phase>` as requested.
    GateReached = 6,
    /// 7 — `migration_lock` held by a live PID on this host.
    LockHeld = 7,
    /// 8 — `migration_lock` held by a dead PID or foreign host.
    LockStale = 8,
    /// 9 — Completed phase digest mismatch.
    CheckpointDigestMismatch = 9,
    /// 10 — Write failure due to insufficient disk space.
    DiskFull = 10,
    /// 11 — `PRAGMA integrity_check` failed on source DB.
    CorruptSource = 11,
    /// 12 — Backfill batch exceeded `--batch-timeout-secs` twice.
    BatchStuck = 12,
    /// 13 — `--mid-rollback` invoked on a completed migration.
    RollbackOnCompletedMigration = 13,
    /// 14 — Dry-run projected a failure.
    DryRunWouldFail = 14,
}

/// Stable user-facing error tag.
///
/// Operators key automation off either the exit code or the tag; the tag is
/// recommended (more expressive, survives exit-code renumbering in future
/// majors). The string form (via `Display`) is the verbatim
/// SCREAMING_SNAKE_CASE token from §10.4 of the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorTag {
    InternalError,
    Paused,
    FailuresPresent,
    UnsupportedVersion,
    BackupFailed,
    GateReached,
    LockHeld,
    LockStale,
    CheckpointDigestMismatch,
    DiskFull,
    CorruptSource,
    BatchStuck,
    RollbackCompletedMigration,
    DryRunWouldFail,
}

impl ErrorTag {
    /// Verbatim string form from §10.4.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ErrorTag::InternalError => "MIG_INTERNAL_ERROR",
            ErrorTag::Paused => "MIG_PAUSED",
            ErrorTag::FailuresPresent => "MIG_FAILURES_PRESENT",
            ErrorTag::UnsupportedVersion => "MIG_UNSUPPORTED_VERSION",
            ErrorTag::BackupFailed => "MIG_BACKUP_FAILED",
            ErrorTag::GateReached => "MIG_GATE_REACHED",
            ErrorTag::LockHeld => "MIG_LOCK_HELD",
            ErrorTag::LockStale => "MIG_LOCK_STALE",
            ErrorTag::CheckpointDigestMismatch => "MIG_CHECKPOINT_DIGEST_MISMATCH",
            ErrorTag::DiskFull => "MIG_DISK_FULL",
            ErrorTag::CorruptSource => "MIG_CORRUPT_SOURCE",
            ErrorTag::BatchStuck => "MIG_BATCH_STUCK",
            ErrorTag::RollbackCompletedMigration => "MIG_ROLLBACK_COMPLETED_MIGRATION",
            ErrorTag::DryRunWouldFail => "MIG_DRY_RUN_WOULD_FAIL",
        }
    }
}

impl std::fmt::Display for ErrorTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Top-level migration error type — every CLI exit non-zero path goes through
/// here. Variants correspond 1:1 with rows in §10.4 of the design.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// Source DB `schema_version` is not the expected v0.2.2 value.
    #[error(
        "source DB schema_version is {found}, but migration requires {expected}; \
         upgrade the source to v0.2.2 first (multi-hop chains are not supported)"
    )]
    UnsupportedVersion { found: u32, expected: u32 },

    /// Phase 1 backup could not be written / verified.
    #[error("backup to {path} failed: {source}")]
    BackupFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Phase 2 DDL batch failed; the IMMEDIATE transaction rolled back.
    #[error("DDL failed (Phase 2 transaction rolled back): {0}")]
    DdlFailed(String),

    /// Phase 5 verification failed — a structural invariant is wrong.
    #[error("invariant violated during verification: {0}")]
    InvariantViolated(String),

    /// `PRAGMA integrity_check` failed on source DB before any write.
    #[error("source database failed integrity check: {0}")]
    CorruptSource(String),

    /// Non-per-record fatal error during Phase 4 (e.g., disk full, read-only DB).
    /// Checkpoint is preserved so the operator can resume after fixing.
    #[error("backfill aborted (checkpoint preserved, --resume to continue): {0}")]
    BackfillFatal(String),

    /// Cooperative SIGINT pause.
    #[error("migration paused (SIGINT); run 'engramai migrate --resume' when ready")]
    Paused,

    /// Phase 4 finished with `records_failed > 0`.
    #[error(
        "phase 4 finished with {count} per-record failure(s); \
         inspect graph_extraction_failures and run --retry-failed"
    )]
    FailuresPresent { count: u64 },

    /// Stopped at `--gate <phase>` as requested.
    #[error("stopped at --gate {phase} as requested")]
    GateReached { phase: String },

    /// `migration_lock` held by a live PID on this host.
    #[error("another migration is running (pid {pid})")]
    LockHeld { pid: u32 },

    /// `migration_lock` held by a dead PID or a foreign host.
    #[error(
        "stale migration lock detected (pid {pid}); \
         verify no other migration is running, then run 'engramai migrate --force-unlock'"
    )]
    LockStale { pid: u32 },

    /// A completed phase's recomputed digest differs from the stored one.
    #[error(
        "checkpoint digest mismatch for phase '{phase}'; \
         restore from backup is the safe default (--ignore-digest-mismatch is an escape hatch)"
    )]
    CheckpointDigestMismatch { phase: String },

    /// Backfill batch exceeded `--batch-timeout-secs` twice.
    #[error(
        "backfill batch stuck after two timeouts; last attempted memories.id = {last_id}"
    )]
    BatchStuck { last_id: String },

    /// `--mid-rollback` invoked on a DB where `migration_complete = 1`.
    #[error(
        "--mid-rollback is invalid on a completed migration; use full rollback from backup instead"
    )]
    RollbackOnCompletedMigration,

    /// Dry-run projected a failure (constraint, schema, sampled-backfill failure).
    #[error("dry-run projected a failure: {0}")]
    DryRunWouldFail(String),
}

impl MigrationError {
    /// Map this error to its stable [`ExitCode`] per §10.4.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            MigrationError::UnsupportedVersion { .. } => ExitCode::UnsupportedVersion,
            MigrationError::BackupFailed { .. } => ExitCode::BackupFailed,
            // §10.4 has no dedicated row for DDL/invariant; design note in the
            // task spec routes these to MIG_INTERNAL_ERROR (exit 1).
            MigrationError::DdlFailed(_) => ExitCode::InternalError,
            MigrationError::InvariantViolated(_) => ExitCode::InternalError,
            MigrationError::CorruptSource(_) => ExitCode::CorruptSource,
            // Treated as MIG_DISK_FULL (exit 10) for now; refine into more
            // specific variants when needed (the design earmarks this).
            MigrationError::BackfillFatal(_) => ExitCode::DiskFull,
            MigrationError::Paused => ExitCode::Paused,
            MigrationError::FailuresPresent { .. } => ExitCode::FailuresPresent,
            MigrationError::GateReached { .. } => ExitCode::GateReached,
            MigrationError::LockHeld { .. } => ExitCode::LockHeld,
            MigrationError::LockStale { .. } => ExitCode::LockStale,
            MigrationError::CheckpointDigestMismatch { .. } => {
                ExitCode::CheckpointDigestMismatch
            }
            MigrationError::BatchStuck { .. } => ExitCode::BatchStuck,
            MigrationError::RollbackOnCompletedMigration => {
                ExitCode::RollbackOnCompletedMigration
            }
            MigrationError::DryRunWouldFail(_) => ExitCode::DryRunWouldFail,
        }
    }

    /// Map this error to its stable [`ErrorTag`] per §10.4.
    pub fn error_tag(&self) -> ErrorTag {
        match self {
            MigrationError::UnsupportedVersion { .. } => ErrorTag::UnsupportedVersion,
            MigrationError::BackupFailed { .. } => ErrorTag::BackupFailed,
            MigrationError::DdlFailed(_) => ErrorTag::InternalError,
            MigrationError::InvariantViolated(_) => ErrorTag::InternalError,
            MigrationError::CorruptSource(_) => ErrorTag::CorruptSource,
            MigrationError::BackfillFatal(_) => ErrorTag::DiskFull,
            MigrationError::Paused => ErrorTag::Paused,
            MigrationError::FailuresPresent { .. } => ErrorTag::FailuresPresent,
            MigrationError::GateReached { .. } => ErrorTag::GateReached,
            MigrationError::LockHeld { .. } => ErrorTag::LockHeld,
            MigrationError::LockStale { .. } => ErrorTag::LockStale,
            MigrationError::CheckpointDigestMismatch { .. } => {
                ErrorTag::CheckpointDigestMismatch
            }
            MigrationError::BatchStuck { .. } => ErrorTag::BatchStuck,
            MigrationError::RollbackOnCompletedMigration => {
                ErrorTag::RollbackCompletedMigration
            }
            MigrationError::DryRunWouldFail(_) => ErrorTag::DryRunWouldFail,
        }
    }

    /// Whether the operator can resume / retry after addressing the cause.
    ///
    /// Derived from the "Resumable?" column of §10.4:
    /// - `Yes`, `Partial`, `Maybe` → `true`
    /// - `No`, `n/a` → `false`
    pub fn is_resumable(&self) -> bool {
        match self {
            // n/a or No
            MigrationError::UnsupportedVersion { .. } => false,
            MigrationError::LockHeld { .. } => false,
            MigrationError::RollbackOnCompletedMigration => false,
            MigrationError::DryRunWouldFail(_) => false,

            // Yes / Partial / Maybe
            MigrationError::BackupFailed { .. } => true, // Yes (re-run from scratch)
            MigrationError::DdlFailed(_) => true,         // Maybe (MIG_INTERNAL_ERROR)
            MigrationError::InvariantViolated(_) => true, // Maybe (MIG_INTERNAL_ERROR)
            MigrationError::CorruptSource(_) => true,     // Yes (after repair)
            MigrationError::BackfillFatal(_) => true,     // Yes (DiskFull → resume)
            MigrationError::Paused => true,               // Yes
            MigrationError::FailuresPresent { .. } => true, // Partial (retry-failed)
            MigrationError::GateReached { .. } => true,   // Yes
            MigrationError::LockStale { .. } => true,     // Yes (after --force-unlock)
            MigrationError::CheckpointDigestMismatch { .. } => true, // Yes (with flag)
            MigrationError::BatchStuck { .. } => true,    // Yes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::path::PathBuf;

    /// Build one instance of every variant. Used by exhaustiveness tests.
    fn all_variants() -> Vec<MigrationError> {
        vec![
            MigrationError::UnsupportedVersion {
                found: 1,
                expected: 2,
            },
            MigrationError::BackupFailed {
                path: PathBuf::from("/tmp/x.bak"),
                source: io::Error::other("no space"),
            },
            MigrationError::DdlFailed("ALTER TABLE failed".into()),
            MigrationError::InvariantViolated("orphan edge".into()),
            MigrationError::CorruptSource("page 42 checksum bad".into()),
            MigrationError::BackfillFatal("disk full at row 9999".into()),
            MigrationError::Paused,
            MigrationError::FailuresPresent { count: 7 },
            MigrationError::GateReached {
                phase: "phase4".into(),
            },
            MigrationError::LockHeld { pid: 1234 },
            MigrationError::LockStale { pid: 5678 },
            MigrationError::CheckpointDigestMismatch {
                phase: "phase3".into(),
            },
            MigrationError::BatchStuck {
                last_id: "abc-123".into(),
            },
            MigrationError::RollbackOnCompletedMigration,
            MigrationError::DryRunWouldFail("constraint X would fail".into()),
        ]
    }

    #[test]
    fn display_is_non_empty_for_every_variant() {
        for e in all_variants() {
            let s = format!("{}", e);
            assert!(!s.is_empty(), "Display empty for variant {:?}", e);
        }
    }

    #[test]
    fn exit_code_and_error_tag_pairs_match_catalog() {
        // (variant, expected_exit, expected_tag_str)
        let cases: Vec<(MigrationError, ExitCode, &'static str)> = vec![
            (
                MigrationError::UnsupportedVersion {
                    found: 1,
                    expected: 2,
                },
                ExitCode::UnsupportedVersion,
                "MIG_UNSUPPORTED_VERSION",
            ),
            (
                MigrationError::BackupFailed {
                    path: PathBuf::from("/tmp/x"),
                    source: io::Error::other("x"),
                },
                ExitCode::BackupFailed,
                "MIG_BACKUP_FAILED",
            ),
            (
                MigrationError::DdlFailed("x".into()),
                ExitCode::InternalError,
                "MIG_INTERNAL_ERROR",
            ),
            (
                MigrationError::InvariantViolated("x".into()),
                ExitCode::InternalError,
                "MIG_INTERNAL_ERROR",
            ),
            (
                MigrationError::CorruptSource("x".into()),
                ExitCode::CorruptSource,
                "MIG_CORRUPT_SOURCE",
            ),
            (
                MigrationError::BackfillFatal("x".into()),
                ExitCode::DiskFull,
                "MIG_DISK_FULL",
            ),
            (MigrationError::Paused, ExitCode::Paused, "MIG_PAUSED"),
            (
                MigrationError::FailuresPresent { count: 1 },
                ExitCode::FailuresPresent,
                "MIG_FAILURES_PRESENT",
            ),
            (
                MigrationError::GateReached {
                    phase: "p".into(),
                },
                ExitCode::GateReached,
                "MIG_GATE_REACHED",
            ),
            (
                MigrationError::LockHeld { pid: 1 },
                ExitCode::LockHeld,
                "MIG_LOCK_HELD",
            ),
            (
                MigrationError::LockStale { pid: 1 },
                ExitCode::LockStale,
                "MIG_LOCK_STALE",
            ),
            (
                MigrationError::CheckpointDigestMismatch {
                    phase: "p".into(),
                },
                ExitCode::CheckpointDigestMismatch,
                "MIG_CHECKPOINT_DIGEST_MISMATCH",
            ),
            (
                MigrationError::BatchStuck {
                    last_id: "z".into(),
                },
                ExitCode::BatchStuck,
                "MIG_BATCH_STUCK",
            ),
            (
                MigrationError::RollbackOnCompletedMigration,
                ExitCode::RollbackOnCompletedMigration,
                "MIG_ROLLBACK_COMPLETED_MIGRATION",
            ),
            (
                MigrationError::DryRunWouldFail("x".into()),
                ExitCode::DryRunWouldFail,
                "MIG_DRY_RUN_WOULD_FAIL",
            ),
        ];

        assert_eq!(cases.len(), 15, "all 15 error variants must be covered");
        for (err, expected_exit, expected_tag) in cases {
            assert_eq!(
                err.exit_code(),
                expected_exit,
                "exit_code mismatch for {:?}",
                err
            );
            assert_eq!(
                err.error_tag().as_str(),
                expected_tag,
                "error_tag mismatch for {:?}",
                err
            );
        }
    }

    #[test]
    fn exit_code_numeric_values_match_catalog() {
        assert_eq!(ExitCode::Success as u8, 0);
        assert_eq!(ExitCode::InternalError as u8, 1);
        assert_eq!(ExitCode::Paused as u8, 2);
        assert_eq!(ExitCode::FailuresPresent as u8, 3);
        assert_eq!(ExitCode::UnsupportedVersion as u8, 4);
        assert_eq!(ExitCode::BackupFailed as u8, 5);
        assert_eq!(ExitCode::GateReached as u8, 6);
        assert_eq!(ExitCode::LockHeld as u8, 7);
        assert_eq!(ExitCode::LockStale as u8, 8);
        assert_eq!(ExitCode::CheckpointDigestMismatch as u8, 9);
        assert_eq!(ExitCode::DiskFull as u8, 10);
        assert_eq!(ExitCode::CorruptSource as u8, 11);
        assert_eq!(ExitCode::BatchStuck as u8, 12);
        assert_eq!(ExitCode::RollbackOnCompletedMigration as u8, 13);
        assert_eq!(ExitCode::DryRunWouldFail as u8, 14);
    }

    #[test]
    fn error_tag_display_roundtrip() {
        // Sample of tags — Display must match as_str() verbatim.
        assert_eq!(format!("{}", ErrorTag::InternalError), "MIG_INTERNAL_ERROR");
        assert_eq!(format!("{}", ErrorTag::LockHeld), "MIG_LOCK_HELD");
        assert_eq!(
            format!("{}", ErrorTag::RollbackCompletedMigration),
            "MIG_ROLLBACK_COMPLETED_MIGRATION"
        );
    }

    #[test]
    fn is_resumable_non_resumable_variants() {
        // §10.4 "No" or "n/a"
        assert!(!MigrationError::UnsupportedVersion {
            found: 1,
            expected: 2
        }
        .is_resumable());
        assert!(!MigrationError::LockHeld { pid: 1 }.is_resumable());
        assert!(!MigrationError::RollbackOnCompletedMigration.is_resumable());
        assert!(!MigrationError::DryRunWouldFail("x".into()).is_resumable());
    }

    #[test]
    fn is_resumable_resumable_variants() {
        // §10.4 "Yes" / "Partial" / "Maybe"
        assert!(MigrationError::Paused.is_resumable());
        assert!(MigrationError::FailuresPresent { count: 1 }.is_resumable());
        assert!(MigrationError::GateReached {
            phase: "p".into()
        }
        .is_resumable());
        assert!(MigrationError::LockStale { pid: 1 }.is_resumable());
        assert!(MigrationError::CorruptSource("x".into()).is_resumable());
        assert!(MigrationError::BackfillFatal("x".into()).is_resumable());
        assert!(MigrationError::BatchStuck {
            last_id: "z".into()
        }
        .is_resumable());
    }

    #[test]
    fn backup_failed_chains_io_error_as_source() {
        let err = MigrationError::BackupFailed {
            path: PathBuf::from("/tmp/x.bak"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        };
        // thiserror #[source] should expose the io::Error.
        let src = std::error::Error::source(&err);
        assert!(src.is_some(), "BackupFailed must expose io::Error as source");
    }
}
