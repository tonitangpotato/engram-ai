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

pub mod backfill;
pub mod backup;
pub mod checkpoint;
pub mod compat;
pub mod error;
pub mod failure;
pub mod lock;
pub mod phase_machine;
pub mod preflight;
pub mod progress;
pub mod schema;

pub use backfill::{
    BackfillConfig, BackfillOrchestrator, BackfillSummary, FailurePolicy, MemoryRecord,
    RecordOutcome, RecordProcessor,
};

pub use backup::{
    backup_path_for, maybe_write_backup, required_free_bytes as backup_required_free_bytes,
    schema_version_digest, BackupInputs, BackupMode, BackupOutcome, FsExists, RealFs,
    BACKUP_SUFFIX,
};
pub use checkpoint::{
    sha256_hex, CheckpointStore, MigrationStateRow, PhaseDigestRow, CHECKPOINT_DDL,
    NO_RECORDS_PROCESSED,
};
pub use compat::{
    assert_v02_compat, contract_for, MethodContract, V02CompatSurface, BEHAVIORAL_CONTRACT,
    V02_FROZEN_METHODS,
};
pub use schema::{
    apply_additive_columns, record_schema_version_v3, rename_entities_valence_if_present,
    run_phase2, SCHEMA_VERSION_DDL,
};
pub use error::{ErrorTag, ExitCode, MigrationError};
pub use failure::{
    bump_retry_count, count_unresolved, derive_failure_episode_id, derive_failure_id,
    format_error_detail, get_failure, list_unresolved, mark_resolved, parse_memory_id_from_detail,
    record_failure, record_outcome_failure, retry_failed, validate_error_category, validate_stage,
    FailureRecord, FailureWrite, RetryConfig, RetrySummary, CATEGORY_BUDGET_EXHAUSTED,
    CATEGORY_DB_ERROR, CATEGORY_INTERNAL, CATEGORY_LLM_INVALID_OUTPUT, CATEGORY_LLM_TIMEOUT,
    DEFAULT_NAMESPACE, STAGE_DEDUP, STAGE_EDGE_EXTRACT, STAGE_ENTITY_EXTRACT,
    STAGE_KNOWLEDGE_COMPILE, STAGE_PERSIST, STAGE_TOPIC_CARRY_FORWARD,
};
pub use lock::{
    local_hostname, real_pid_alive, AcquiredLock, LockHolder, MigrationLock, PidAliveCheck,
    LOCK_DDL,
};
pub use phase_machine::{PhaseExecutors, PhaseMachine, PhaseMachineConfig, PhaseRunOutcome};
pub use preflight::{
    check_disk_space, detect_schema_version, required_free_bytes, run_preflight,
    PreflightInputs, PreflightOutcome, SchemaState, DISK_SPACE_MULTIPLIER_DEN,
    DISK_SPACE_MULTIPLIER_NUM, TARGET_SCHEMA_VERSION,
};
pub use progress::{
    EmitterConfig, MigrationLogRow, MigrationPhase, MigrationProgress, ProgressCallback,
    ProgressEmitter, ProgressEvent,
};
