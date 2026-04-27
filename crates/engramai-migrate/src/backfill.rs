//! Backfill orchestrator (Phase 4 of the migration ladder).
//!
//! Implements design §5.1 ("Orchestrator") of
//! `.gid/features/v03-migration/design.md`. Satisfies GOAL-4.1 (idempotent
//! backfill) and GOAL-4.4 (per-record failure visibility, jointly with T10).
//!
//! ## Scope of this task (T8)
//!
//! T8 is **orchestrator core only**. The companion tasks split as:
//!
//! - **T8 (this file):** the streaming cursor over `memories`, the per-record
//!   loop, checkpoint advance, progress emission cadence, failure-policy
//!   gating, resume semantics. The orchestrator is *generic* over a
//!   [`RecordProcessor`] trait — it does not itself know how to invoke the
//!   resolution pipeline or write graph deltas.
//! - **T9 (`task:migration-impl-backfill-perrecord`):** the [`RecordProcessor`]
//!   implementation that calls `ResolutionPipeline::resolve_for_backfill` and
//!   `GraphStore::apply_graph_delta`. Lives in a future module that depends on
//!   v03-resolution + v03-graph-layer (which `engramai-migrate` is *not*
//!   coupled to — see Notes #4 in the build plan).
//! - **T10 (`task:migration-impl-backfill-failure`):** the failure-row writer
//!   into `graph_extraction_failures` and the `--retry-failed` subcommand.
//!   T10 hooks into the [`RecordOutcome::Failed`] return value this
//!   orchestrator already produces.
//!
//! Splitting the work this way keeps `engramai-migrate` as a leaf crate
//! (no dependency on `engramai` core) — the trait abstraction is the seam.
//!
//! ## Atomicity contract
//!
//! Per design §5.1, "the orchestrator never holds more than one `memories`
//! row in memory at a time" and "each record is committed independently so
//! a crash does not lose more than the in-flight record." This module
//! enforces both via the `process_one` callback boundary: the
//! [`RecordProcessor`] is responsible for opening its own transaction that
//! covers (a) graph deltas and (b) the [`CheckpointStore::update_backfill_progress`]
//! call, atomically. The orchestrator only coordinates — it does not own a
//! transaction. This keeps the §5.4 "advance-after-commit" invariant
//! testable in isolation (T16 idempotency tests).
//!
//! ## What the orchestrator does NOT do
//!
//! - **Does not open a write transaction.** That is the processor's job
//!   because only the processor knows what graph rows to write.
//! - **Does not emit `MigrationProgress` itself.** It owns a
//!   [`ProgressEmitter`] cadence guard and asks the caller (via a
//!   `Box<dyn FnMut>` callback) to actually write to stderr/stdout/log.
//!   This keeps I/O concerns out of the orchestrator and makes it
//!   testable without capturing process state.
//! - **Does not handle Phase 4 → Phase 5 advancement.** That is the
//!   phase machine's job (T12). The orchestrator returns a
//!   [`BackfillSummary`] when the cursor exhausts; the phase machine
//!   reads it and calls `CheckpointStore::advance_phase`.

use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::checkpoint::CheckpointStore;
use crate::error::MigrationError;
use crate::progress::{MigrationPhase, MigrationProgress, ProgressEmitter};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for the backfill orchestrator (design §5.1).
///
/// Defaults match GOAL-4.5 (emit every 100 records or 5 seconds) and the
/// "Continue on per-record failure" policy that the CLI uses unless the
/// operator passes `--stop-on-failure` (design §5.3).
#[derive(Debug, Clone)]
pub struct BackfillConfig {
    /// Streaming cursor batch size (rows fetched per `prepared.query` page).
    /// Does NOT bound memory across records — `process_one` is called once
    /// per row. This only sizes the SQLite statement window.
    pub batch_size: usize,
    /// Emit progress every N processed records (GOAL-4.5).
    pub emit_every_records: u64,
    /// Emit progress every D wall-clock since last emission (GOAL-4.5).
    pub emit_every_duration: Duration,
    /// What to do on a per-record extraction failure (design §5.3).
    pub on_record_failure: FailurePolicy,
}

impl Default for BackfillConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            emit_every_records: 100,
            emit_every_duration: Duration::from_secs(5),
            on_record_failure: FailurePolicy::Continue,
        }
    }
}

/// Per-record failure policy (design §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePolicy {
    /// Log + record the failure, advance checkpoint, continue. Default.
    Continue,
    /// Log + record the failure, then stop Phase 4 with `EXIT_FAILURES_PRESENT`.
    Stop,
}

/// One row read from the v0.2 `memories` table (design §5.1 cursor SELECT).
///
/// Field set is the minimum the resolution pipeline needs; intentionally
/// narrower than the full v0.2 schema (we don't load `embedding` for backfill —
/// the pipeline re-derives whatever it needs from `content`).
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub id: i64,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: String,
}

/// Per-record processing result returned from [`RecordProcessor::process_one`].
///
/// `Failed` is a normal return value — only true fatal errors propagate as
/// [`MigrationError::BackfillFatal`] (disk full, corrupt SQLite page, etc.).
/// This matches design §5.2: "Per-record failures are data, not control flow."
///
/// ## Failure-row coupling (T10)
///
/// `Failed` carries `episode_id` and `message` so the failure module
/// (`crate::failure`) can write a complete `graph_extraction_failures`
/// row without needing to re-derive that information. `episode_id` is
/// `Option<Uuid>` because the resolution pipeline may fail BEFORE
/// allocating an episode (e.g., extraction blew up at the entity stage);
/// when `None`, the failure module derives a deterministic surrogate via
/// `derive_failure_episode_id(record_id)` so the schema's `NOT NULL`
/// constraint is honored without inventing an arbitrary UUID v4.
#[derive(Debug, Clone)]
pub enum RecordOutcome {
    Succeeded {
        entity_count: u32,
        edge_count: u32,
    },
    Failed {
        record_id: i64,
        /// Failure category — one of the `CATEGORY_*` constants from
        /// `crate::failure`. Free-form `String` here so this crate stays
        /// decoupled from v03-resolution's enum; validated at the
        /// `record_failure` write site.
        kind: String,
        /// Pipeline stage where the failure occurred — one of the
        /// `STAGE_*` constants from `crate::failure`. Free-form `String`
        /// for the same reason as `kind`.
        stage: String,
        /// Episode id allocated by the pipeline before the failure, if
        /// any. `None` → `crate::failure` derives a deterministic
        /// surrogate from `record_id`.
        episode_id: Option<Uuid>,
        /// Operator-readable detail (free-form). Stored in
        /// `graph_extraction_failures.error_detail` after being prefixed
        /// with `"memory:<record_id>|"` (see `failure::format_error_detail`).
        message: String,
    },
}

/// The seam between this crate (migration) and v03-resolution + v03-graph-layer.
///
/// T9 implements this trait with a struct that holds `Arc<dyn GraphStore>` and
/// `Arc<ResolutionPipeline>`. The orchestrator only sees the trait — it does
/// not pull in either downstream crate. See module-level docs for why this
/// abstraction exists.
pub trait RecordProcessor {
    /// Process exactly one v0.2 `MemoryRecord`:
    ///
    /// 1. Run the resolution pipeline over `record.content`.
    /// 2. Atomically (in a single SQLite transaction):
    ///    a. Apply the resulting `GraphDelta` (entities + edges + mentions).
    ///    b. Update `migration_state` via
    ///       [`CheckpointStore::update_backfill_progress`] with the appropriate
    ///       delta counters.
    /// 3. Return [`RecordOutcome::Succeeded`] or [`RecordOutcome::Failed`].
    ///
    /// Returning `Err` aborts backfill (Phase 4 stops, checkpoint preserved
    /// at last successfully advanced row). Reserved for fatal errors only.
    fn process_one(
        &self,
        conn: &mut Connection,
        record: MemoryRecord,
    ) -> Result<RecordOutcome, MigrationError>;
}

/// Summary returned by [`BackfillOrchestrator::run`] when the cursor exhausts.
///
/// Read by the phase machine (T12) to decide whether to advance Phase 4 → 5.
#[derive(Debug, Clone, Default)]
pub struct BackfillSummary {
    pub records_processed: u64,
    pub records_succeeded: u64,
    pub records_failed: u64,
    /// `true` if backfill stopped before exhausting the cursor due to
    /// `FailurePolicy::Stop` triggering on a failed record. The phase
    /// machine maps this to `MigrationError::FailuresPresent`.
    pub stopped_on_failure: bool,
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// The Phase-4 backfill orchestrator (design §5.1).
///
/// Owns the cursor, the cadence guard, and the failure-policy decision.
/// Does NOT own the database connection or the processor — both are passed
/// in per-`run`. This makes the orchestrator a pure "loop coordinator"
/// suitable for unit testing with a fake [`RecordProcessor`].
pub struct BackfillOrchestrator {
    config: BackfillConfig,
    emitter: ProgressEmitter,
}

impl BackfillOrchestrator {
    pub fn new(config: BackfillConfig) -> Self {
        let emitter = ProgressEmitter::with_config(crate::progress::EmitterConfig {
            emit_every_records: config.emit_every_records,
            emit_every_duration: config.emit_every_duration,
        });
        Self { config, emitter }
    }

    /// Run the backfill loop to completion (cursor exhausted) or to the
    /// first per-record failure under [`FailurePolicy::Stop`].
    ///
    /// # Arguments
    /// - `conn`: an open connection to the migrating database. The cursor
    ///   reads `memories` from this connection; the processor also writes
    ///   graph rows through it.
    /// - `processor`: T9's per-record handler.
    /// - `records_total`: optional row count (from a pre-flight `COUNT(*)`).
    ///   Used only for progress display — backfill works with `None`.
    /// - `started_at`: wall-clock start of Phase 4, for `MigrationProgress`.
    ///   On resume, the *original* phase start (read from `migration_state`).
    /// - `on_progress`: callback invoked once per emission cadence trigger,
    ///   carrying the latest [`MigrationProgress`] snapshot. Best-effort —
    ///   panics inside the callback are NOT caught here; the caller is
    ///   responsible for unwind safety (design §5.5: "if the callback panics
    ///   ... the error is logged once" — that logging happens in the CLI
    ///   layer, not in the orchestrator).
    ///
    /// # Resume
    ///
    /// The orchestrator reads `last_processed_memory_id` from
    /// `migration_state` and continues with `id > last`. If no row exists
    /// (fresh run), it starts from `id > -1` (i.e., the entire table). The
    /// counters loaded from `migration_state` are *not* reset — they
    /// accumulate across runs, satisfying GOAL-4.5's "progress survives
    /// process restart" clause.
    pub fn run<P, F>(
        &mut self,
        conn: &mut Connection,
        processor: &P,
        records_total: Option<u64>,
        on_progress: &mut F,
    ) -> Result<BackfillSummary, MigrationError>
    where
        P: RecordProcessor,
        F: FnMut(&MigrationProgress),
    {
        // 1. Read resume cursor + accumulated counters from migration_state.
        let state = CheckpointStore::load_state(conn)
            .map_err(|e| {
                MigrationError::BackfillFatal(format!("failed to load migration_state: {e}"))
            })?
            .ok_or_else(|| {
                MigrationError::InvariantViolated(
                    "backfill: migration_state row missing — preflight should have inserted it"
                        .into(),
                )
            })?;

        let resume_after = state.last_processed_memory_id;
        let started_at = parse_started_at(&state.started_at)?;
        let mut accum = BackfillSummary {
            records_processed: state.records_processed as u64,
            records_succeeded: state.records_succeeded as u64,
            records_failed: state.records_failed as u64,
            stopped_on_failure: false,
        };

        // 2. Stream the cursor. We page manually (LIMIT/OFFSET-style on the
        //    primary key) rather than holding a long-lived prepared cursor
        //    across processor calls — the processor opens its own
        //    transaction, and SQLite cannot have a read cursor and a write
        //    transaction interleaved on the same connection. Paging by
        //    `id > last_seen` is O(n log n) on the PK index — fast enough.
        let mut last_seen: i64 = resume_after;

        loop {
            let page = fetch_page(conn, last_seen, self.config.batch_size)?;
            if page.is_empty() {
                break; // cursor exhausted — Phase 4 done
            }

            for record in page {
                let record_id = record.id;
                last_seen = record_id;

                // 3. Hand off to T9.
                let outcome = processor.process_one(conn, record)?;

                // 4. Update local counters (DB counters are advanced inside
                //    the processor's transaction).
                accum.records_processed += 1;
                match &outcome {
                    RecordOutcome::Succeeded { .. } => {
                        accum.records_succeeded += 1;
                    }
                    RecordOutcome::Failed { .. } => {
                        accum.records_failed += 1;
                        if self.config.on_record_failure == FailurePolicy::Stop {
                            accum.stopped_on_failure = true;
                            // Final emission so the operator sees the stop point.
                            self.emit_now(
                                accum.records_processed,
                                accum.records_succeeded,
                                accum.records_failed,
                                records_total,
                                started_at,
                                on_progress,
                            );
                            return Ok(accum);
                        }
                    }
                }

                // 5. Cadence-gated emission.
                let now = Instant::now();
                if self.emitter.should_emit(now, accum.records_processed) {
                    self.emit_at(
                        now,
                        accum.records_processed,
                        accum.records_succeeded,
                        accum.records_failed,
                        records_total,
                        started_at,
                        on_progress,
                    );
                }
            }
        }

        // 6. Final emission on exhaustion (so observers see the closing
        //    counters even if the last record didn't trigger cadence).
        self.emit_now(
            accum.records_processed,
            accum.records_succeeded,
            accum.records_failed,
            records_total,
            started_at,
            on_progress,
        );

        Ok(accum)
    }

    fn emit_at<F>(
        &mut self,
        now: Instant,
        processed: u64,
        succeeded: u64,
        failed: u64,
        total: Option<u64>,
        started_at: chrono::DateTime<Utc>,
        on_progress: &mut F,
    ) where
        F: FnMut(&MigrationProgress),
    {
        let mut p = MigrationProgress::new(
            MigrationPhase::Backfill,
            started_at,
            total.unwrap_or(0),
        );
        p.records_processed = processed;
        p.records_succeeded = succeeded;
        p.records_failed = failed;
        on_progress(&p);
        self.emitter.record_emission(now, processed);
    }

    fn emit_now<F>(
        &mut self,
        processed: u64,
        succeeded: u64,
        failed: u64,
        total: Option<u64>,
        started_at: chrono::DateTime<Utc>,
        on_progress: &mut F,
    ) where
        F: FnMut(&MigrationProgress),
    {
        self.emit_at(
            Instant::now(),
            processed,
            succeeded,
            failed,
            total,
            started_at,
            on_progress,
        );
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn fetch_page(
    conn: &Connection,
    after_id: i64,
    limit: usize,
) -> Result<Vec<MemoryRecord>, MigrationError> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, content, metadata, created_at \
             FROM memories \
             WHERE id > ?1 \
             ORDER BY id ASC \
             LIMIT ?2",
        )
        .map_err(|e| {
            MigrationError::BackfillFatal(format!("prepare backfill cursor: {e}"))
        })?;

    let rows = stmt
        .query_map(params![after_id, limit as i64], |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                metadata: row.get::<_, Option<String>>(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| {
            MigrationError::BackfillFatal(format!("execute backfill cursor: {e}"))
        })?;

    let mut out = Vec::with_capacity(limit);
    for r in rows {
        out.push(r.map_err(|e| {
            // SQLite row decode failure = source corruption, not fatal IO.
            // Map per design §10.4: corrupt source row → CorruptSource.
            MigrationError::CorruptSource(format!("decode memories row: {e}"))
        })?);
    }
    Ok(out)
}

fn parse_started_at(s: &str) -> Result<chrono::DateTime<Utc>, MigrationError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            MigrationError::InvariantViolated(format!(
                "migration_state.started_at is not RFC3339: {s} ({e})"
            ))
        })
}

// Silence unused-import warning when the `OptionalExtension` trait happens
// to be unused after refactors — kept here because future T9/T10 hooks may
// need optional-row queries on this connection.
#[allow(dead_code)]
fn _keep_optional_extension_in_scope(conn: &Connection) {
    let _ = conn
        .query_row("SELECT 1", [], |_| Ok(()))
        .optional();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::{CheckpointStore, CHECKPOINT_DDL};
    use std::cell::RefCell;

    /// Test fixture: in-memory DB with `memories` + `migration_state` tables.
    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE memories (
                id INTEGER PRIMARY KEY,
                content TEXT NOT NULL,
                metadata TEXT,
                created_at TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute_batch(CHECKPOINT_DDL).unwrap();
        conn
    }

    fn insert_memory(conn: &Connection, id: i64, content: &str) {
        conn.execute(
            "INSERT INTO memories (id, content, metadata, created_at) \
             VALUES (?1, ?2, NULL, '2024-01-01T00:00:00Z')",
            params![id, content],
        )
        .unwrap();
    }

    fn init_state(conn: &Connection) {
        CheckpointStore::insert_initial_state(
            conn,
            MigrationPhase::Backfill,
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
    }

    /// Test processor: records what it saw, returns Succeeded for content
    /// containing "ok", Failed otherwise. Does NOT write to graph tables —
    /// just bumps the checkpoint counters so resume tests work.
    struct FakeProcessor {
        seen: RefCell<Vec<i64>>,
    }

    impl FakeProcessor {
        fn new() -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl RecordProcessor for FakeProcessor {
        fn process_one(
            &self,
            conn: &mut Connection,
            record: MemoryRecord,
        ) -> Result<RecordOutcome, MigrationError> {
            self.seen.borrow_mut().push(record.id);
            let succeeded = record.content.contains("ok");
            // Simulate the atomic processor txn: advance checkpoint here.
            let tx = conn.transaction().map_err(|e| {
                MigrationError::BackfillFatal(format!("test txn: {e}"))
            })?;
            CheckpointStore::update_backfill_progress(
                &tx,
                record.id,
                1,
                if succeeded { 1 } else { 0 },
                if succeeded { 0 } else { 1 },
                "2024-01-01T00:00:01Z",
            )?;
            tx.commit().map_err(|e| {
                MigrationError::BackfillFatal(format!("test commit: {e}"))
            })?;
            Ok(if succeeded {
                RecordOutcome::Succeeded {
                    entity_count: 1,
                    edge_count: 0,
                }
            } else {
                RecordOutcome::Failed {
                    record_id: record.id,
                    kind: crate::failure::CATEGORY_INTERNAL.into(),
                    stage: crate::failure::STAGE_ENTITY_EXTRACT.into(),
                    episode_id: None,
                    message: "test-induced failure".into(),
                }
            })
        }
    }

    #[test]
    fn run_processes_all_records_in_id_order() {
        let mut conn = fresh_db();
        init_state(&conn);
        for (i, c) in [(1, "ok one"), (2, "ok two"), (3, "ok three")] {
            insert_memory(&conn, i, c);
        }
        let proc = FakeProcessor::new();
        let mut emissions = 0;
        let mut on_progress = |_: &MigrationProgress| emissions += 1;
        let mut orch = BackfillOrchestrator::new(BackfillConfig::default());
        let summary = orch
            .run(&mut conn, &proc, Some(3), &mut on_progress)
            .unwrap();
        assert_eq!(*proc.seen.borrow(), vec![1, 2, 3]);
        assert_eq!(summary.records_processed, 3);
        assert_eq!(summary.records_succeeded, 3);
        assert_eq!(summary.records_failed, 0);
        assert!(!summary.stopped_on_failure);
        assert!(emissions >= 1, "at least final emission expected");
    }

    #[test]
    fn run_continues_past_failures_under_continue_policy() {
        let mut conn = fresh_db();
        init_state(&conn);
        insert_memory(&conn, 1, "ok one");
        insert_memory(&conn, 2, "BAD"); // does not contain "ok"
        insert_memory(&conn, 3, "ok three");
        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig::default());
        let summary = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap();
        assert_eq!(summary.records_processed, 3);
        assert_eq!(summary.records_succeeded, 2);
        assert_eq!(summary.records_failed, 1);
        assert!(!summary.stopped_on_failure);
    }

    #[test]
    fn run_stops_on_first_failure_under_stop_policy() {
        let mut conn = fresh_db();
        init_state(&conn);
        insert_memory(&conn, 1, "ok one");
        insert_memory(&conn, 2, "BAD");
        insert_memory(&conn, 3, "ok three"); // never reached
        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig {
            on_record_failure: FailurePolicy::Stop,
            ..Default::default()
        });
        let summary = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap();
        assert_eq!(summary.records_processed, 2);
        assert_eq!(summary.records_succeeded, 1);
        assert_eq!(summary.records_failed, 1);
        assert!(summary.stopped_on_failure);
        assert_eq!(*proc.seen.borrow(), vec![1, 2]); // record 3 not seen
    }

    #[test]
    fn run_resumes_from_last_processed_memory_id() {
        let mut conn = fresh_db();
        init_state(&conn);
        // Pretend a previous run got through record 2.
        CheckpointStore::update_backfill_progress(
            &conn,
            2,
            2,
            2,
            0,
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        insert_memory(&conn, 1, "ok one"); // already done — skipped
        insert_memory(&conn, 2, "ok two"); // already done — skipped
        insert_memory(&conn, 3, "ok three"); // new
        insert_memory(&conn, 4, "ok four"); // new

        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig::default());
        let summary = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap();
        // Only records 3 and 4 should be seen.
        assert_eq!(*proc.seen.borrow(), vec![3, 4]);
        // Counters accumulate across runs (GOAL-4.5).
        assert_eq!(summary.records_processed, 4);
        assert_eq!(summary.records_succeeded, 4);
        assert_eq!(summary.records_failed, 0);
    }

    #[test]
    fn run_on_empty_db_returns_zero_summary() {
        let mut conn = fresh_db();
        init_state(&conn);
        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig::default());
        let summary = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap();
        assert_eq!(summary.records_processed, 0);
        assert!(proc.seen.borrow().is_empty());
    }

    #[test]
    fn run_with_no_state_row_returns_invariant_violation() {
        let mut conn = fresh_db();
        // Skip init_state — migration_state is empty.
        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig::default());
        let err = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap_err();
        assert!(
            matches!(err, MigrationError::InvariantViolated(_)),
            "expected InvariantViolated, got {err:?}"
        );
    }

    #[test]
    fn cursor_pagination_handles_more_than_one_page() {
        let mut conn = fresh_db();
        init_state(&conn);
        // Insert 7 records with batch_size=3 → 3 pages (3+3+1).
        for i in 1..=7 {
            insert_memory(&conn, i, "ok");
        }
        let proc = FakeProcessor::new();
        let mut on_progress = |_: &MigrationProgress| {};
        let mut orch = BackfillOrchestrator::new(BackfillConfig {
            batch_size: 3,
            ..Default::default()
        });
        let summary = orch
            .run(&mut conn, &proc, None, &mut on_progress)
            .unwrap();
        assert_eq!(summary.records_processed, 7);
        assert_eq!(*proc.seen.borrow(), vec![1, 2, 3, 4, 5, 6, 7]);
    }
}
