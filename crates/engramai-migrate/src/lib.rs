//! # engramai-migrate
//!
//! Migration tooling for upgrading engramai databases from v0.2.x → v0.3.
//!
//! This crate is the implementation of the migration design specified in
//! `.gid/features/v03-migration/design.md`. Currently only the error model
//! (§10 of the design) is implemented; the CLI binary, schema DDL, backup,
//! backfill, and verification phases will land in subsequent tasks.
//!
//! See the design document, especially §10.4 ("Error catalog"), for the
//! stable user-facing contract that `MigrationError`, `ExitCode`, and
//! `ErrorTag` encode.

pub mod checkpoint;
pub mod error;
pub mod lock;
pub mod progress;

pub use checkpoint::{
    sha256_hex, CheckpointStore, MigrationStateRow, PhaseDigestRow, CHECKPOINT_DDL,
    NO_RECORDS_PROCESSED,
};
pub use error::{ErrorTag, ExitCode, MigrationError};
pub use lock::{
    local_hostname, real_pid_alive, AcquiredLock, LockHolder, MigrationLock, PidAliveCheck,
    LOCK_DDL,
};
pub use progress::{
    EmitterConfig, MigrationLogRow, MigrationPhase, MigrationProgress, ProgressCallback,
    ProgressEmitter, ProgressEvent,
};
