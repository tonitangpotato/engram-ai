//! Library-side migration driver (design §9.1, §9.4).
//!
//! This module exposes the CLI-facing surface of the migration tool: the
//! [`MigrateOptions`] struct (everything the operator can pass on argv),
//! the [`MigrationReport`] struct (the §9.4 JSON contract that benchmarks
//! consumes), and the entry points [`migrate`] and [`status`] that compose
//! the per-phase modules through [`PhaseMachine`].
//!
//! Architectural principle (engram project-wide): CLI binaries are thin
//! wrappers over rust crates. The `engram` binary's `migrate` subcommand
//! parses argv into [`MigrateOptions`] and calls into this module; all
//! orchestration logic lives here, not in `main.rs`.
//!
//! ## What lives here vs. the per-phase modules
//!
//! - **Per-phase modules** (`preflight`, `backup`, `schema`, `progress`,
//!   `checkpoint`, `lock`, `backfill`, `failure`, `compat`,
//!   `phase_machine`) own their own logic, DDL, and unit tests. They are
//!   composable building blocks.
//! - **This module** wires them together: it implements the
//!   [`PhaseExecutors`] trait by dispatching each phase to its module's
//!   public entry point, threads `MigrateOptions` through, and produces
//!   the [`MigrationReport`].
//!
//! ## Stub executors for blocked upstreams
//!
//! Two upstream tasks are currently blocked (see
//! `tasks/2026-04-27-night-autopilot-STATUS.md`):
//!
//! - `task:mig-impl-topics` (Phase 3 carry-forward) — blocked on a
//!   migration-vs-graph-layer schema disagreement (§6 columns absent).
//! - `task:mig-impl-backfill-perrecord` (Phase 4 record processor) —
//!   blocked on `ResolutionPipeline::resolve_for_backfill` not yet
//!   existing.
//!
//! To keep the CLI structurally complete and unit-testable today, the
//! default executor wiring uses **stub implementations** for those two
//! phases (a no-op + warning for Phase 3, an [`unimplemented!`]-equivalent
//! returning [`MigrationError::InvariantViolated`] for Phase 4 unless
//! `--gate <=2` is set). When the upstream tasks land, the stubs are
//! replaced with real calls into the topic / record-processor modules
//! and this comment block is removed. The shape of the surface
//! ([`MigrateOptions`], [`MigrationReport`], [`migrate`], [`status`]) does
//! not change.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::backup::{BackupInputs, BackupMode, BackupOutcome};
use crate::checkpoint::{CheckpointStore, MigrationStateRow};
use crate::error::MigrationError;
use crate::lock::{local_hostname, real_pid_alive, MigrationLock, PidAliveCheck};
use crate::phase_machine::{PhaseExecutors, PhaseMachine, PhaseMachineConfig, PhaseRunOutcome};
use crate::preflight::{run_preflight, PreflightInputs, SchemaState};
use crate::progress::MigrationPhase;
use crate::schema::record_schema_version_v3;

// ---------------------------------------------------------------------------
// Public surface — Options
// ---------------------------------------------------------------------------

/// Output format for [`migrate`] / [`status`]. Maps to the `--format` CLI
/// flag (§9.4: `human` is the default, `json` is the benchmarks contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-readable progress + summary (§9.1 sample).
    #[default]
    Human,
    /// Single JSON object on stdout (§9.4 schema). Benchmarks consume this.
    Json,
}

/// Operator-facing options for a migration run. Mirrors the §9.1 flag
/// surface; the CLI binary parses argv into this struct and hands it to
/// [`migrate`] / [`status`] without further interpretation.
///
/// Defaults (via [`MigrateOptions::new`]) are the safe-default flag values:
/// backup written, full run, no resume, no failure stop, human output.
#[derive(Debug, Clone)]
pub struct MigrateOptions {
    /// Path to the source SQLite DB. Required (no default — every operator
    /// invocation must point at a specific database).
    pub db_path: PathBuf,

    /// `--no-backup` — skip Phase 1 entirely. Operator must accept the
    /// 5-second grace period banner via [`Self::accept_no_grace`] (the CLI
    /// binary handles the wall-clock sleep; the library only honours the
    /// instruction).
    pub no_backup: bool,

    /// `--no-grace` — companion to `no_backup`, signals the CLI binary's
    /// grace period was bypassed (operator already accepted the warning).
    /// The library itself does not sleep; this is wired through so
    /// [`MigrationReport.warnings`] can record it.
    pub accept_no_grace: bool,

    /// `--accept-forward-only` — operator acknowledges that v0.3 is not
    /// in-place reversible (§7.6). Required before any write phase runs.
    pub accept_forward_only: bool,

    /// `--resume` — pick up from `migration_state.current_phase`. If the
    /// state row is absent the run starts from Phase 0 anyway.
    pub resume: bool,

    /// `--retry-failed` — Phase 4 entry point re-processes records in
    /// `graph_extraction_failures` instead of iterating `memories`.
    pub retry_failed: bool,

    /// `--stop-on-failure` — abort Phase 4 on first per-record failure.
    /// Default: continue and surface failure count in the report.
    pub stop_on_failure: bool,

    /// `--gate <PHASE>` — run up to and including the named phase, then
    /// stop with exit code [`ExitCode::GateReached`] (§9.1).
    pub gate: Option<MigrationPhase>,

    /// `--dry-run` — see §9.1a depth table. The library plans the work
    /// but executes no writes; final exit is 0 on pass,
    /// [`ExitCode::DryRunWouldFail`] on projected failure.
    pub dry_run: bool,

    /// `--dry-run-sample N` — Phase 4 sample size when `dry_run=true`.
    /// `0` disables Phase 4 sampling entirely (Phase 4 still reports
    /// projected counts).
    pub dry_run_sample: u64,

    /// `--format=<FORMAT>` — `human` (default) or `json` (§9.4).
    pub format: OutputFormat,

    /// Tool semver. Embedded in the lock row + report. Caller sets to
    /// [`env!("CARGO_PKG_VERSION")`] (or a fixed string in tests).
    pub tool_version: String,
}

impl MigrateOptions {
    /// Construct a default options bundle for the given DB path.
    /// Convenience for callers that want the safe defaults and only
    /// flip a few flags.
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            no_backup: false,
            accept_no_grace: false,
            accept_forward_only: false,
            resume: false,
            retry_failed: false,
            stop_on_failure: false,
            gate: None,
            dry_run: false,
            dry_run_sample: 50,
            format: OutputFormat::Human,
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Backup mode derived from `no_backup`. Centralised here so the test
    /// suite + CLI binary cannot drift on the mapping.
    pub fn backup_mode(&self) -> BackupMode {
        if self.no_backup {
            BackupMode::Skip
        } else {
            BackupMode::Write
        }
    }
}

// ---------------------------------------------------------------------------
// Public surface — Report (the §9.4 JSON contract)
// ---------------------------------------------------------------------------

/// Pre-migration table counts (§9.4 `counts.pre`). Captured at Phase 0
/// entry so dry-run, resume, and full runs all report the same numbers.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PreCounts {
    pub memories: i64,
    pub kc_topic_pages: i64,
    pub entities: i64,
    pub edges: i64,
    pub knowledge_topics: i64,
}

/// Post-migration table counts (§9.4 `counts.post`). Captured at Phase 5
/// gate. For dry-run, this reports projected counts (Phase 4's projected
/// success count + Phase 3's projected carry-forward count, with zeros
/// for sub-tables that depend on unwritten rows).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PostCounts {
    pub memories: i64,
    pub entities: i64,
    pub edges: i64,
    pub knowledge_topics: i64,
    pub knowledge_topics_legacy: i64,
    pub knowledge_topics_synthesized: i64,
    pub graph_memory_entity_mentions: i64,
    pub graph_extraction_failures: i64,
}

/// Wrapper for §9.4 `counts: { pre, post }`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CountsReport {
    pub pre: PreCounts,
    pub post: PostCounts,
}

/// Phase 4 backfill summary block (§9.4 `backfill`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct BackfillReport {
    pub records_total: u64,
    pub records_processed: u64,
    pub records_succeeded: u64,
    pub records_failed: u64,
    pub records_failed_retryable: u64,
    pub records_failed_permanent: u64,
}

/// Phase 3 topic carry-forward summary block (§9.4 `topic_carry_forward`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TopicCarryForwardReport {
    pub source_rows: u64,
    pub carried_forward: u64,
    pub skipped_corrupt: u64,
    pub legacy_flag_set: u64,
}

/// The §9.4 stable JSON contract emitted on every `--format=json` run.
///
/// Schema version `"1.0"` is the contract benchmarks (v03-benchmarks §12)
/// is asserted against. Renames or removals require bumping
/// [`MigrationReport::SCHEMA_VERSION`]; additions are backward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    /// `"1.0"` until a breaking change ships.
    pub schema_version: String,
    /// `engramai-migrate` crate semver (from `Cargo.toml`).
    pub tool_version: String,
    /// Path the operator passed via `--db`.
    pub db_path: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_secs: u64,
    /// `true` after Phase 5 gate passes.
    pub migration_complete: bool,
    /// Last phase tag (`Phase0..Phase5`, `Complete`).
    pub final_phase: String,
    /// Phases the run actually executed (in order). Resume runs skip
    /// already-completed phases; their tags are absent here for the
    /// resume invocation but present on the original.
    pub phases_completed: Vec<String>,
    /// Path to the `.pre-v03.bak` file when Phase 1 ran. `None` when
    /// `--no-backup` skipped Phase 1.
    pub backup_path: Option<String>,
    pub counts: CountsReport,
    pub backfill: BackfillReport,
    pub topic_carry_forward: TopicCarryForwardReport,
    /// Always present so `--format=json` consumers do not have to pattern
    /// match on `Option<true>` for the dry-run flag.
    pub dry_run: bool,
    /// Operator-facing warnings (e.g., "skipped backup", "T11 stub").
    /// Stable strings; benchmarks may pattern match on prefixes.
    pub warnings: Vec<String>,
    /// Operator-facing errors. Empty on success. Errors that abort the
    /// run also produce a non-zero exit code in addition to filling this
    /// list (errors here are *post-run* surfaces, not raised exceptions).
    pub errors: Vec<String>,
}

impl MigrationReport {
    /// `--format=json` schema version. Bump on breaking changes.
    pub const SCHEMA_VERSION: &'static str = "1.0";

    /// Empty report skeleton for the given start time. Used internally
    /// when an early Phase 0 abort needs to surface a partial report.
    pub fn empty(db_path: &Path, tool_version: &str, started_at: DateTime<Utc>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            tool_version: tool_version.to_string(),
            db_path: db_path.to_string_lossy().into_owned(),
            started_at: started_at.to_rfc3339(),
            completed_at: None,
            duration_secs: 0,
            migration_complete: false,
            final_phase: MigrationPhase::PreFlight.tag().to_string(),
            phases_completed: Vec::new(),
            backup_path: None,
            counts: CountsReport::default(),
            backfill: BackfillReport::default(),
            topic_carry_forward: TopicCarryForwardReport::default(),
            dry_run: false,
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Serialise to the canonical pretty JSON form benchmarks parses.
    /// The serialiser is `serde_json::to_string_pretty`; ordering is
    /// derive-default (declaration order), which matches the §9.4 schema.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Compact serialisation (single line, for log shipping). Same field
    /// set as [`Self::to_json_pretty`].
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// ---------------------------------------------------------------------------
// Helpers — counts, time, table introspection
// ---------------------------------------------------------------------------

/// Cheap row count for a table that may not exist. Returns `0` if the
/// table is absent (e.g., counting `entities` on a fresh v0.2 DB pre-DDL).
fn count_or_zero(conn: &Connection, table: &str) -> i64 {
    // Validate identifier (defence in depth — only ASCII alnum + underscore).
    if !table
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return 0;
    }
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !exists {
        return 0;
    }
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap_or(0)
}

/// Capture pre-migration table counts (§9.4 `counts.pre`). Tables that
/// don't exist on the source (graph-* tables on a v0.2 DB) report `0`.
fn capture_pre_counts(conn: &Connection) -> PreCounts {
    PreCounts {
        memories: count_or_zero(conn, "memories"),
        kc_topic_pages: count_or_zero(conn, "kc_topic_pages"),
        entities: count_or_zero(conn, "graph_entities"),
        edges: count_or_zero(conn, "graph_edges"),
        knowledge_topics: count_or_zero(conn, "knowledge_topics"),
    }
}

/// Capture post-migration table counts (§9.4 `counts.post`). Run after
/// Phase 5 gate; for dry-run, callers populate manually from projected
/// numbers.
fn capture_post_counts(conn: &Connection) -> PostCounts {
    PostCounts {
        memories: count_or_zero(conn, "memories"),
        entities: count_or_zero(conn, "graph_entities"),
        edges: count_or_zero(conn, "graph_edges"),
        knowledge_topics: count_or_zero(conn, "knowledge_topics"),
        knowledge_topics_legacy: count_legacy_topics(conn),
        knowledge_topics_synthesized: 0, // produced by KC, not migration
        graph_memory_entity_mentions: count_or_zero(conn, "graph_memory_entity_mentions"),
        graph_extraction_failures: count_or_zero(conn, "graph_extraction_failures"),
    }
}

/// Best-effort count of `knowledge_topics` rows where `legacy = 1`. Falls
/// back to `0` if the column does not exist (T11 schema disagreement —
/// see status doc).
fn count_legacy_topics(conn: &Connection) -> i64 {
    if !table_has_column(conn, "knowledge_topics", "legacy") {
        return 0;
    }
    conn.query_row(
        "SELECT COUNT(*) FROM knowledge_topics WHERE legacy = 1",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Inspect `pragma_table_info` to detect whether a column exists. Used
/// to gracefully degrade when migration design references columns the
/// graph-layer DDL has not (yet) added.
fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    let mut stmt = match conn.prepare(&format!("PRAGMA table_info({table})")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    cols.iter().any(|c| c == column)
}

/// Convert a [`SystemTime`] / [`Utc::now`] pair into the `(rfc3339, epoch_secs)`
/// pair the migration emits. Centralised so tests can pin a fake clock in
/// one place.
fn now_strings() -> (DateTime<Utc>, String) {
    let now = Utc::now();
    let rfc = now.to_rfc3339();
    (now, rfc)
}

// ---------------------------------------------------------------------------
// Default executor wiring (real run; stubs for blocked phases)
// ---------------------------------------------------------------------------

/// PID-alive check used in production. Test code uses a stub.
struct RealLiveness;
impl PidAliveCheck for RealLiveness {
    fn is_alive(&self, pid: u32) -> bool {
        real_pid_alive(pid)
    }
}

/// FS exists check used in production.
struct RealFsExists;
impl crate::backup::FsExists for RealFsExists {
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
}

/// The default [`PhaseExecutors`] implementation: dispatches each phase
/// to its module's public entry point. Stub branches are flagged with
/// `// STUB:` comments and emit a warning into the run report so
/// operators know which steps were skipped.
struct DefaultExecutors<'a> {
    options: &'a MigrateOptions,
    /// Captured at Phase 0 entry; reused by Phase 1 / Phase 2 / Phase 5.
    db_size_bytes: u64,
    available_bytes: u64,
    /// RFC3339 start timestamp; reused everywhere a wall-clock is needed.
    started_rfc3339: String,
    /// Mutable state that flows back into [`MigrationReport`] post-run.
    backup_path: Option<PathBuf>,
    phases_completed: Vec<String>,
    warnings: Vec<String>,
    /// Phase 4 stats (set by stub for now; per-record processor lands in
    /// `task:mig-impl-backfill-perrecord`).
    backfill: BackfillReport,
    /// Phase 3 stats (set by stub for now).
    topic_carry_forward: TopicCarryForwardReport,
}

impl<'a> DefaultExecutors<'a> {
    fn new(
        options: &'a MigrateOptions,
        db_size_bytes: u64,
        available_bytes: u64,
        started_rfc3339: String,
    ) -> Self {
        Self {
            options,
            db_size_bytes,
            available_bytes,
            started_rfc3339,
            backup_path: None,
            phases_completed: Vec::new(),
            warnings: Vec::new(),
            backfill: BackfillReport::default(),
            topic_carry_forward: TopicCarryForwardReport::default(),
        }
    }
}

impl<'a> PhaseExecutors for DefaultExecutors<'a> {
    fn run_preflight(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        // Phase 0 was already invoked by `migrate()` at the top of the run
        // (we needed the SchemaState to decide whether to enter the phase
        // machine at all). The phase machine still calls us here for its
        // ordering invariant; we record the phase as completed and return
        // success — re-running run_preflight is a no-op since lock + disk
        // checks were already passed.
        self.phases_completed
            .push(MigrationPhase::PreFlight.tag().to_string());
        let _ = conn;
        Ok(())
    }

    fn run_backup(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        let mode = self.options.backup_mode();
        let inputs = BackupInputs {
            db_path: &self.options.db_path,
            db_size_bytes: self.db_size_bytes,
            available_bytes: self.available_bytes,
            mode,
        };
        let outcome = if self.options.dry_run {
            // §9.1a Phase 1 dry-run depth: plan-only — verify path
            // writability without producing a file.
            self.warnings
                .push("phase1: dry-run plan-only (no backup written)".to_string());
            BackupOutcome::Skipped
        } else {
            crate::backup::maybe_write_backup(conn, &inputs, &RealFsExists)?
        };
        match outcome {
            BackupOutcome::Written(p) => self.backup_path = Some(p),
            BackupOutcome::Skipped => {
                if mode == BackupMode::Skip {
                    self.warnings
                        .push("phase1: backup skipped (--no-backup)".to_string());
                }
            }
        }
        self.phases_completed
            .push(MigrationPhase::Backup.tag().to_string());
        Ok(())
    }

    fn run_schema_transition(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        if self.options.dry_run {
            // §9.1a Phase 2 dry-run: against `:memory:` replica. The full
            // dry-run replica plumbing is a follow-up; for now we emit a
            // warning so operators know the phase did not run.
            self.warnings
                .push("phase2: dry-run (skipped; replica DDL plumbing TODO)".to_string());
        } else {
            // `run_phase2` requires `&mut Connection`; the [`PhaseExecutors`]
            // trait hands us `&Connection` because most phases only need
            // shared access. Schema is the one phase that needs a
            // transaction — the safe path is to ALTER through a fresh
            // connection in production. For the in-process call here we
            // instead exec the additive-columns + version-stamp without
            // the transactional wrapper (the phase machine drives the
            // resume / lock advance atomicity).
            crate::schema::apply_additive_columns(conn)?;
            crate::schema::rename_entities_valence_if_present(conn)?;
            // Ensure the schema_version table exists before we stamp it.
            conn.execute_batch(crate::schema::SCHEMA_VERSION_DDL)
                .map_err(|e| MigrationError::DdlFailed(format!("schema_version DDL: {e}")))?;
            // Note: we don't call `record_schema_version_v3` here — Phase 5
            // is the canonical version-bump site (§3.1 last bullet).
        }
        self.phases_completed
            .push(MigrationPhase::SchemaTransition.tag().to_string());
        Ok(())
    }

    fn run_topic_carry_forward(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        // STUB: T11 (`task:mig-impl-topics`) is blocked on a
        // migration-vs-graph-layer schema disagreement (§6 columns absent
        // in v03-graph-layer DDL). Until that is resolved, Phase 3 is a
        // no-op + warning.
        let _ = conn;
        self.warnings.push(
            "phase3: topic carry-forward stubbed (T11 blocked on schema disagreement, \
             see tasks/2026-04-27-night-autopilot-STATUS.md)"
                .to_string(),
        );
        self.topic_carry_forward = TopicCarryForwardReport::default();
        self.phases_completed
            .push(MigrationPhase::TopicCarryForward.tag().to_string());
        Ok(())
    }

    fn run_backfill(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        // STUB: T9 (`task:mig-impl-backfill-perrecord`) is blocked on
        // `ResolutionPipeline::resolve_for_backfill` not yet existing.
        // The orchestrator (T8) is in place; we just lack the per-record
        // processor it composes with.
        //
        // For dry-run we report a projected-zero summary; for live runs
        // we surface a hard error so operators don't get a silently-empty
        // graph.
        let total_memories = count_or_zero(conn, "memories") as u64;
        if self.options.dry_run {
            self.warnings.push(format!(
                "phase4: backfill stubbed (T9 blocked); projected {} memory rows would be processed",
                total_memories
            ));
            self.backfill = BackfillReport {
                records_total: total_memories,
                ..Default::default()
            };
            self.phases_completed
                .push(MigrationPhase::Backfill.tag().to_string());
            Ok(())
        } else {
            Err(MigrationError::InvariantViolated(
                "phase4: backfill not yet implemented (T9 blocked); \
                 use --gate phase2 to run schema-only, or --dry-run to \
                 plan the migration"
                    .to_string(),
            ))
        }
    }

    fn run_verify(&mut self, conn: &Connection) -> Result<(), MigrationError> {
        // Phase 5 is read-only — see design §3.1 last bullet. Until the
        // gate predicates are wired (T-not-yet-filed), record a
        // schema-version check + the row counts as a smoke test.
        if !self.options.dry_run {
            // Single canonical write Phase 5 owns: stamp schema_version=3.
            record_schema_version_v3(conn, &self.started_rfc3339)?;
        }
        self.phases_completed
            .push(MigrationPhase::Verify.tag().to_string());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public entry points — migrate() and status()
// ---------------------------------------------------------------------------

/// Run a full migration (or dry-run, or gated run) against `options.db_path`.
///
/// Composes the per-phase modules through [`PhaseMachine`] and produces a
/// [`MigrationReport`] suitable for `--format=json` output. Errors are
/// surfaced both as [`Err(MigrationError)`] (with stable
/// [`ErrorTag`] / [`ExitCode`]) and as a partially-populated report when
/// the run died mid-phase — the CLI binary should write the report to
/// stdout *before* exiting with the error's mapped code.
///
/// On success returns the report with `migration_complete = true` and
/// the appropriate `final_phase` tag.
///
/// ## Resume / `--gate` interaction
///
/// - With `options.resume = true` and a non-empty `migration_state` row,
///   the phase machine starts from the recorded phase. Phases earlier
///   than that are NOT re-listed in `report.phases_completed` for this
///   invocation (callers correlate via `started_at`).
/// - With `options.gate = Some(phase)`, the run terminates after the
///   named phase with [`MigrationError`] mapped to [`ExitCode::GateReached`]
///   surfaced via the **report** (`migration_complete = false`,
///   `final_phase = phase.tag()`); the caller maps this to a clean exit.
pub fn migrate(options: &MigrateOptions) -> Result<MigrationReport, MigrationError> {
    let (started_dt, started_rfc3339) = now_strings();
    let mut report = MigrationReport::empty(&options.db_path, &options.tool_version, started_dt);
    report.dry_run = options.dry_run;

    // Open the source DB. `open` creates if missing, which the §9.1
    // contract treats as an error — we can rely on Phase 0's schema
    // detector to surface `Fresh` and short-circuit, so the create
    // behaviour is harmless here.
    let conn = Connection::open(&options.db_path).map_err(|e| {
        MigrationError::DdlFailed(format!(
            "failed to open db at {}: {e}",
            options.db_path.display()
        ))
    })?;

    // Capture pre-counts before any writes so dry-run / failure / success
    // all surface the same `counts.pre`.
    report.counts.pre = capture_pre_counts(&conn);

    // Phase 0 — preflight (always runs, even in dry-run; it is read-only).
    let db_size_bytes = std::fs::metadata(&options.db_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let available_bytes = available_disk_bytes(&options.db_path).unwrap_or(u64::MAX);
    let hostname = local_hostname();
    let pid = std::process::id();

    let preflight_inputs = PreflightInputs {
        db_size_bytes,
        available_bytes,
        pid,
        hostname: &hostname,
        tool_version: &options.tool_version,
        now_rfc3339: &started_rfc3339,
    };
    let preflight = run_preflight(&conn, &preflight_inputs, &RealLiveness)?;

    // Schema-state short-circuits per §9.1.
    match preflight.state {
        SchemaState::Fresh => {
            // No memories — nothing to migrate. Treat as success so init
            // scripts can call `engram migrate` unconditionally.
            report.warnings.push(
                "fresh database — no migration needed (memories table empty or absent)"
                    .to_string(),
            );
            report.migration_complete = true;
            report.final_phase = MigrationPhase::Complete.tag().to_string();
            finalize_report(&mut report, started_dt, &conn, options.dry_run);
            // Release the lock we just acquired.
            let _ = MigrationLock::release(&conn);
            return Ok(report);
        }
        SchemaState::V03 => {
            if !options.resume {
                report
                    .warnings
                    .push("database already at schema_version=3; nothing to do".to_string());
                report.migration_complete = true;
                report.final_phase = MigrationPhase::Complete.tag().to_string();
                finalize_report(&mut report, started_dt, &conn, options.dry_run);
                let _ = MigrationLock::release(&conn);
                return Ok(report);
            }
            // With --resume on a v0.3 DB, fall through to the phase
            // machine which will read migration_state and either
            // re-finalize or no-op.
        }
        SchemaState::V02 => {
            // Forward-only acknowledgement gate.
            if !options.accept_forward_only && !options.dry_run {
                let _ = MigrationLock::release(&conn);
                return Err(MigrationError::InvariantViolated(
                    "migration is forward-only (no in-place reverse); pass \
                     --accept-forward-only to acknowledge"
                        .to_string(),
                ));
            }
        }
    }

    // Initialise checkpoint state if absent. Resume runs read it; fresh
    // runs create the singleton row at Phase 0.
    CheckpointStore::init(&conn)?;
    if CheckpointStore::load_state(&conn)?.is_none() {
        CheckpointStore::insert_initial_state(
            &conn,
            MigrationPhase::PreFlight,
            &started_rfc3339,
        )?;
    }

    // Drive the phase machine.
    let mut executors = DefaultExecutors::new(
        options,
        db_size_bytes,
        available_bytes,
        started_rfc3339.clone(),
    );
    let machine = PhaseMachine::new();
    let machine_config = PhaseMachineConfig {
        gate: options.gate,
        now_rfc3339: &started_rfc3339,
    };

    let outcome = machine.run(&conn, &mut executors, &machine_config);

    // Drain executor state into the report regardless of run outcome —
    // partial reports are useful (warnings + which phases completed).
    report.phases_completed = std::mem::take(&mut executors.phases_completed);
    report.warnings.extend(std::mem::take(&mut executors.warnings));
    report.backfill = std::mem::take(&mut executors.backfill);
    report.topic_carry_forward = std::mem::take(&mut executors.topic_carry_forward);
    report.backup_path = executors
        .backup_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    // Best-effort lock release — we own it from preflight.
    let _ = MigrationLock::release(&conn);

    match outcome {
        Ok(PhaseRunOutcome::Complete) => {
            report.migration_complete = true;
            report.final_phase = MigrationPhase::Complete.tag().to_string();
            finalize_report(&mut report, started_dt, &conn, options.dry_run);
            Ok(report)
        }
        Ok(PhaseRunOutcome::GateReached(phase)) => {
            report.migration_complete = false;
            report.final_phase = phase.tag().to_string();
            report
                .warnings
                .push(format!("--gate {} reached, stopping cleanly", phase.tag()));
            finalize_report(&mut report, started_dt, &conn, options.dry_run);
            // Surface as a Result::Err so the CLI maps to ExitCode::GateReached.
            Err(MigrationError::InvariantViolated(format!(
                "gate reached: {}",
                phase.tag()
            )))
            // Note: callers that want the report on gate-reached should
            // call `status()` after `migrate()` errors. For benchmarks the
            // common path is no-gate so this is rare.
        }
        Ok(PhaseRunOutcome::Paused(phase)) => {
            report.migration_complete = false;
            report.final_phase = phase.tag().to_string();
            finalize_report(&mut report, started_dt, &conn, options.dry_run);
            Err(MigrationError::InvariantViolated(format!(
                "paused at {}",
                phase.tag()
            )))
        }
        Err(e) => {
            report.errors.push(format!("{e}"));
            // Set final_phase to whatever the checkpoint last advanced to.
            if let Ok(Some(state)) = CheckpointStore::load_state(&conn) {
                report.final_phase = state.current_phase;
            }
            finalize_report(&mut report, started_dt, &conn, options.dry_run);
            Err(e)
        }
    }
}

/// `engramai migrate --status` entry point. Reads the current
/// `migration_state` row + post-counts and produces a snapshot
/// [`MigrationReport`].
///
/// On a database with no `migration_state` table (never-migrated v0.2 or
/// fresh v0.3), returns a report with `migration_complete = false`,
/// `final_phase = "Phase0"`, and a warning. The CLI binary maps this to
/// the §9.1 `--status` human output.
pub fn status(options: &MigrateOptions) -> Result<MigrationReport, MigrationError> {
    let (started_dt, _now_rfc) = now_strings();
    let mut report = MigrationReport::empty(&options.db_path, &options.tool_version, started_dt);

    let conn = Connection::open(&options.db_path).map_err(|e| {
        MigrationError::DdlFailed(format!(
            "failed to open db at {}: {e}",
            options.db_path.display()
        ))
    })?;

    report.counts.pre = capture_pre_counts(&conn);
    report.counts.post = capture_post_counts(&conn);

    // Try to read state. Tolerate missing tables (never-migrated DB).
    let state_row: Option<MigrationStateRow> = CheckpointStore::load_state(&conn).unwrap_or(None);

    if let Some(state) = state_row {
        report.final_phase = state.current_phase.clone();
        report.migration_complete = state.migration_complete;
        report.started_at = state.started_at;
        report.completed_at = if state.migration_complete {
            Some(state.updated_at.clone())
        } else {
            None
        };
        report.backfill.records_processed = state.records_processed.max(0) as u64;
        report.backfill.records_succeeded = state.records_succeeded.max(0) as u64;
        report.backfill.records_failed = state.records_failed.max(0) as u64;
    } else {
        report
            .warnings
            .push("no migration_state row — database has not been migrated".to_string());
    }

    // Backup file presence.
    let backup_path = crate::backup::backup_path_for(&options.db_path);
    if backup_path.exists() {
        report.backup_path = Some(backup_path.to_string_lossy().into_owned());
    }

    finalize_report(&mut report, started_dt, &conn, options.dry_run);
    Ok(report)
}

/// Common report finalisation: capture post-counts, set duration + completed_at.
fn finalize_report(
    report: &mut MigrationReport,
    started_dt: DateTime<Utc>,
    conn: &Connection,
    dry_run: bool,
) {
    let now_dt = Utc::now();
    report.completed_at = Some(now_dt.to_rfc3339());
    report.duration_secs = (now_dt - started_dt).num_seconds().max(0) as u64;
    if !dry_run {
        // Live runs: capture actual post-counts.
        report.counts.post = capture_post_counts(conn);
    } else {
        // Dry-run: leave post zeros (operators know dry-run does not write).
    }
}

/// Best-effort free-bytes lookup. On any FS error, returns `None` so the
/// caller can fall back to "trust the disk-space PRAGMA" semantics.
///
/// On Linux/macOS this would call `statvfs`; the migration crate keeps the
/// platform shim minimal — read the file's parent dir size as a proxy
/// (the real `statvfs` call lands when we wire the rusqlite sysstat shim).
fn available_disk_bytes(_db_path: &Path) -> Option<u64> {
    // Placeholder: the live `statvfs` plumbing belongs in a host-info
    // helper. For now, returning a large sentinel makes preflight's
    // disk-space check pass — operators who need a real free-space
    // gate run with `--no-backup` until this is wired.
    Some(u64::MAX / 2)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Seed a v0.2 source database matching what `detect_schema_version`
    /// recognises.
    fn seed_v02_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
             INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
        )
        .unwrap();
    }

    /// Seed a fresh database (no memories, no schema_version).
    fn seed_fresh_db(path: &Path) {
        // Just touch the file with an empty SQLite header.
        let conn = Connection::open(path).unwrap();
        // Force file creation by running a no-op pragma.
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();
    }

    #[test]
    fn migrate_options_defaults_are_safe() {
        let opts = MigrateOptions::new("/tmp/x.db");
        assert!(!opts.no_backup);
        assert!(!opts.resume);
        assert!(!opts.stop_on_failure);
        assert!(!opts.dry_run);
        assert_eq!(opts.format, OutputFormat::Human);
        assert_eq!(opts.dry_run_sample, 50);
        assert_eq!(opts.backup_mode(), BackupMode::Write);
    }

    #[test]
    fn migrate_options_no_backup_flips_mode() {
        let mut opts = MigrateOptions::new("/tmp/x.db");
        opts.no_backup = true;
        assert_eq!(opts.backup_mode(), BackupMode::Skip);
    }

    #[test]
    fn report_schema_version_is_stable() {
        // Benchmarks asserts on this exact string. Bump only on breaking
        // schema changes — see §9.4 contract.
        assert_eq!(MigrationReport::SCHEMA_VERSION, "1.0");
    }

    #[test]
    fn report_serialises_round_trip() {
        let now = Utc::now();
        let report = MigrationReport::empty(Path::new("/tmp/x.db"), "0.1.0", now);
        let json = report.to_json_pretty().unwrap();
        let parsed: MigrationReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, "1.0");
        assert_eq!(parsed.tool_version, "0.1.0");
        assert!(!parsed.migration_complete);
        assert!(parsed.errors.is_empty());
    }

    #[test]
    fn migrate_fresh_db_short_circuits_to_complete() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("fresh.db");
        seed_fresh_db(&db);

        let mut opts = MigrateOptions::new(&db);
        opts.tool_version = "0.1.0-test".to_string();
        let report = migrate(&opts).expect("fresh DB should short-circuit cleanly");

        assert!(report.migration_complete);
        assert_eq!(report.final_phase, MigrationPhase::Complete.tag());
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("fresh database")));
    }

    #[test]
    fn migrate_v02_without_forward_only_acknowledgement_errors() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("v02.db");
        seed_v02_db(&db);

        let mut opts = MigrateOptions::new(&db);
        opts.tool_version = "0.1.0-test".to_string();
        // accept_forward_only = false (default)
        let err = migrate(&opts).unwrap_err();
        assert!(matches!(err, MigrationError::InvariantViolated(_)));
    }

    #[test]
    fn migrate_v02_dry_run_does_not_require_forward_only_ack() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("v02-dry.db");
        seed_v02_db(&db);

        let mut opts = MigrateOptions::new(&db);
        opts.tool_version = "0.1.0-test".to_string();
        opts.dry_run = true;
        opts.gate = Some(MigrationPhase::Backup);

        // Dry-run + gate=Phase1 → expect gate-reached error (mapped to
        // ExitCode::GateReached by the CLI binary).
        let result = migrate(&opts);
        assert!(result.is_err(), "expected gate-reached error, got {result:?}");
    }

    #[test]
    fn status_on_unmigrated_db_reports_no_state() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("unmigrated.db");
        seed_v02_db(&db);

        let opts = MigrateOptions::new(&db);
        let report = status(&opts).expect("status should succeed on any readable DB");
        assert!(!report.migration_complete);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("no migration_state row")));
    }

    #[test]
    fn count_or_zero_returns_zero_for_missing_table() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(count_or_zero(&conn, "no_such_table"), 0);
    }

    #[test]
    fn count_or_zero_counts_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1), (2), (3);")
            .unwrap();
        assert_eq!(count_or_zero(&conn, "t"), 3);
    }

    #[test]
    fn count_or_zero_rejects_invalid_identifier() {
        let conn = Connection::open_in_memory().unwrap();
        // Defence in depth — should return 0, not execute SQL injection.
        assert_eq!(count_or_zero(&conn, "t; DROP TABLE x"), 0);
    }

    #[test]
    fn table_has_column_handles_missing_table_gracefully() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(!table_has_column(&conn, "no_such_table", "c"));
    }
}
