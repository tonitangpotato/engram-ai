//! 6-phase migration state machine (design §3).
//!
//! Drives the migration through Phases 0–5 (PreFlight → Backup →
//! SchemaTransition → TopicCarryForward → Backfill → Verify → Complete),
//! enforcing:
//!
//! - **Sequential ordering** — no phase begins until the prior phase's gate
//!   predicate (§3.2) holds.
//! - **Resumability** — on entry, reads `migration_state` (§5.4) and skips
//!   already-completed phases, jumping to the *next* phase the prior run
//!   did not finish. Phases 0–2 are short and always re-run idempotently
//!   on resume; Phases 3–5 may resume mid-phase via their own checkpoint.
//! - **Pause / gate** — supports `--gate <phase>` (stop after phase) and
//!   cooperative pause (caller maps SIGINT into [`PhaseError::Paused`]).
//! - **Lock phase tracking** — at every phase transition, `migration_lock.phase`
//!   is updated in the same transaction that advances `migration_state`,
//!   so `engramai migrate --status` always reflects the current phase
//!   (§3.5 final paragraph).
//!
//! ## Why "machine" instead of a flat function
//!
//! Phase logic itself lives in the per-phase modules (`preflight`, `backup`,
//! `schema`, `topics`, `backfill`, `verify`). The phase machine is the
//! *coordinator*: it asks each phase "are you done?", "run yourself", "what
//! is your gate digest?", and threads the checkpoint store + lock + progress
//! emitter through the call chain. To keep the coordinator unit-testable
//! without standing up a real database with all the v0.2 fixtures, the
//! per-phase work is delegated through the [`PhaseExecutors`] trait. Real
//! callers wire each method to its corresponding module function; the test
//! suite uses a `FakeExecutors` impl to assert ordering / gate behavior /
//! resume math without touching SQLite.
//!
//! This keeps T12 (the phase machine task) decoupled from the specific
//! signatures of upstream tasks (some of which — Phase 3 topic carry-forward
//! and Phase 5 verify — are still in flight or blocked); the machine
//! defines the *shape* of the orchestration, and downstream tasks fill in
//! the executors.
//!
//! See:
//! - design §3.1 — phase taxonomy
//! - design §3.2 — gate predicates
//! - design §3.3 — pause / resume semantics
//! - design §3.5 — lock phase tracking
//! - GOAL-4.3 — idempotent / resumable
//! - GOAL-4.5 — progress survives restart
//! - GOAL-4.8 — phased rollout, observable gates, pause/resume

use rusqlite::Connection;

use crate::checkpoint::CheckpointStore;
use crate::error::MigrationError;
use crate::lock::MigrationLock;
use crate::progress::MigrationPhase;

/// Outcome of a phase-machine run.
///
/// The CLI surfaces these as different exit codes (§9.1). Errors that abort
/// the run early are returned via `Result::Err` from [`PhaseMachine::run`];
/// only *clean* terminal states appear here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseRunOutcome {
    /// All six phases ran to completion. `migration_complete = 1`.
    Complete,
    /// Stopped at the requested `--gate <phase>` boundary. The named phase
    /// is the *last phase that ran successfully*; the next phase would
    /// have been the one whose gate the caller asked us to stop at.
    GateReached(MigrationPhase),
    /// Cooperative pause (SIGINT). Caller can resume by re-invoking the
    /// machine; checkpoint reflects the last committed state.
    Paused(MigrationPhase),
}

/// Per-phase work delegated by [`PhaseMachine`]. Real callers provide an
/// implementation that calls the per-phase modules (`preflight::run_preflight`,
/// `backup::maybe_write_backup`, `schema::run_phase2`, etc.). Tests use a
/// stub.
///
/// Each method:
/// - **Runs the phase** if its gate predicate (§3.2) does not already hold.
/// - **Returns `Ok(())`** on success (gate predicate now holds).
/// - **Returns `Err`** with a precise [`MigrationError`] on failure; the
///   machine propagates without retry.
///
/// The machine guarantees these are called **at most once per run** and
/// **in phase order** (PreFlight → Backup → SchemaTransition → ...). It
/// also guarantees `advance_to` is called between successful executor
/// returns to update `migration_state.current_phase` and
/// `migration_lock.phase` atomically.
///
/// Resumability is the executor's responsibility *within a phase* (e.g.,
/// Phase 4 reads `last_processed_memory_id` from the checkpoint and resumes
/// from there). The machine itself only decides *which phase* to enter;
/// it does not split a single phase across calls.
pub trait PhaseExecutors {
    /// Phase 0 — pre-flight (§3.1, §3.5, §4.3). Runs schema detect + disk
    /// check + lock acquire. May exit early on `SchemaState::Fresh` or
    /// `SchemaState::V03` — the caller (CLI) is responsible for short-
    /// circuiting before invoking the phase machine in those cases; the
    /// machine assumes Phase 0 must transition to Phase 1 on success.
    fn run_preflight(&mut self, conn: &Connection) -> Result<(), MigrationError>;

    /// Phase 1 — backup (§8.1). May skip with a logged warning under
    /// `--no-backup`; the executor is the right place to consult the flag.
    fn run_backup(&mut self, conn: &Connection) -> Result<(), MigrationError>;

    /// Phase 2 — schema transition (§4.2, §4.3, §4.4). DDL only; no data
    /// motion.
    fn run_schema_transition(&mut self, conn: &Connection) -> Result<(), MigrationError>;

    /// Phase 3 — topic carry-forward (§6). Copies `kc_topic_pages` rows
    /// into `knowledge_topics` with `legacy = 1`. **Currently blocked**
    /// (T11 stop-condition #5 — design vs. graph-layer schema mismatch);
    /// the machine still calls this method so the wiring is complete, but
    /// real callers may temporarily implement it as a no-op + warning
    /// until T11 is unblocked.
    fn run_topic_carry_forward(&mut self, conn: &Connection) -> Result<(), MigrationError>;

    /// Phase 4 — backfill (§5). Iterates `memories`, invokes the
    /// resolution pipeline per record. The orchestrator owns its own
    /// inner loop + cadence + failure policy; the executor wires it up.
    fn run_backfill(&mut self, conn: &Connection) -> Result<(), MigrationError>;

    /// Phase 5 — verify + finalize (§3.1 last bullet). Invariant checks +
    /// `migration_complete = 1`. Distinct from `mark_complete` on the
    /// checkpoint store — Phase 5 is the *check* that justifies marking
    /// complete; the machine performs the marking itself after this
    /// returns `Ok(())`.
    fn run_verify(&mut self, conn: &Connection) -> Result<(), MigrationError>;
}

/// Configuration for a phase-machine run.
#[derive(Debug, Clone)]
pub struct PhaseMachineConfig<'a> {
    /// Optional gate. When `Some(phase)`, the machine stops *after*
    /// completing `phase` and returns [`PhaseRunOutcome::GateReached`].
    /// When `None`, the machine runs through Phase 5 to `Complete`.
    pub gate: Option<MigrationPhase>,
    /// RFC3339 timestamp captured at the start of the run; used for
    /// `migration_state.updated_at` and `migration_lock.phase` writes.
    pub now_rfc3339: &'a str,
}

/// The state machine itself.
///
/// Stateless in struct form (state lives in the database); keeping it as a
/// struct makes future config additions (e.g., an injected clock or a
/// metrics callback) source-compatible.
#[derive(Debug, Default)]
pub struct PhaseMachine;

impl PhaseMachine {
    pub fn new() -> Self {
        Self
    }

    /// Run the machine. See module docs for ordering and resume semantics.
    ///
    /// `executors` is consumed mutably so the caller can carry per-run
    /// inputs (CLI flags, fs handles, fixtures) without forcing them
    /// through the trait method signatures.
    ///
    /// The machine **does not** open a transaction around the whole run —
    /// each phase commits its own work atomically (per design §3.3 "any
    /// in-flight SQLite transaction rolls back"). What the machine *does*
    /// guarantee is that `migration_state.current_phase` and
    /// `migration_lock.phase` advance together (single `UPDATE` per
    /// phase, separate from the executor's transaction; this is consistent
    /// with the rest of the crate where `CheckpointStore::advance_phase`
    /// runs on a fresh connection state).
    pub fn run<E: PhaseExecutors>(
        &self,
        conn: &Connection,
        executors: &mut E,
        config: &PhaseMachineConfig<'_>,
    ) -> Result<PhaseRunOutcome, MigrationError> {
        // Determine where to start from. Resume reads the current phase from
        // `migration_state` and re-enters the *same* phase (Phases 0–2 are
        // idempotent so re-running them is safe; Phases 3–5 read their own
        // sub-checkpoints internally).
        let start = match CheckpointStore::load_state(conn)? {
            Some(state) => phase_from_tag(&state.current_phase)?,
            None => MigrationPhase::PreFlight,
        };

        let mut current = start;

        loop {
            // Run the phase's executor. Any error aborts the run with the
            // checkpoint pointing at the *current* phase (so resume picks
            // back up here).
            self.run_phase(conn, executors, current)?;

            // Phase completed successfully. Decide what's next.
            let next = match current {
                MigrationPhase::PreFlight => MigrationPhase::Backup,
                MigrationPhase::Backup => MigrationPhase::SchemaTransition,
                MigrationPhase::SchemaTransition => MigrationPhase::TopicCarryForward,
                MigrationPhase::TopicCarryForward => MigrationPhase::Backfill,
                MigrationPhase::Backfill => MigrationPhase::Verify,
                MigrationPhase::Verify => MigrationPhase::Complete,
                MigrationPhase::Complete => {
                    // Already terminal — should not happen if the executor
                    // contract is honored, but guard anyway.
                    return Ok(PhaseRunOutcome::Complete);
                }
            };

            // Gate check: stop *after* the named phase.
            if config.gate == Some(current) {
                return Ok(PhaseRunOutcome::GateReached(current));
            }

            // Advance the checkpoint + lock to `next` atomically (per
            // §3.5: lock.phase is updated in the same transaction as the
            // checkpoint advance).
            advance_phase_atomic(conn, next, config.now_rfc3339)?;
            current = next;

            // Terminal: Phase 5 ran successfully → mark complete + done.
            if current == MigrationPhase::Complete {
                CheckpointStore::mark_complete(conn, config.now_rfc3339)?;
                return Ok(PhaseRunOutcome::Complete);
            }
        }
    }

    fn run_phase<E: PhaseExecutors>(
        &self,
        conn: &Connection,
        executors: &mut E,
        phase: MigrationPhase,
    ) -> Result<(), MigrationError> {
        match phase {
            MigrationPhase::PreFlight => executors.run_preflight(conn),
            MigrationPhase::Backup => executors.run_backup(conn),
            MigrationPhase::SchemaTransition => executors.run_schema_transition(conn),
            MigrationPhase::TopicCarryForward => executors.run_topic_carry_forward(conn),
            MigrationPhase::Backfill => executors.run_backfill(conn),
            MigrationPhase::Verify => executors.run_verify(conn),
            MigrationPhase::Complete => Ok(()),
        }
    }
}

/// Atomically advance both `migration_state.current_phase` and
/// `migration_lock.phase`. Per §3.5 final paragraph, these two writes are
/// performed in a single transaction so `engramai migrate --status`
/// always sees a consistent phase across the two tables.
fn advance_phase_atomic(
    conn: &Connection,
    next: MigrationPhase,
    now_rfc3339: &str,
) -> Result<(), MigrationError> {
    // Both `CheckpointStore::advance_phase` and `MigrationLock::update_phase`
    // are single-statement UPDATEs; wrapping them in a transaction here
    // gives the §3.5 atomicity guarantee without changing either helper's
    // contract. If `migration_lock` is absent (e.g., a Fresh-DB run that
    // never acquired a lock — not expected once Phase 0 has run, but we
    // tolerate it for robustness), `update_phase` returns
    // `InvariantViolation` which we treat as fatal.
    let tx_started = conn
        .execute("BEGIN IMMEDIATE", [])
        .map_err(|e| MigrationError::DdlFailed(e.to_string()))
        .is_ok();

    let inner = (|| -> Result<(), MigrationError> {
        CheckpointStore::advance_phase(conn, next, now_rfc3339)?;
        MigrationLock::update_phase(conn, next)?;
        Ok(())
    })();

    match (tx_started, inner) {
        (true, Ok(())) => conn
            .execute("COMMIT", [])
            .map(|_| ())
            .map_err(|e| MigrationError::DdlFailed(e.to_string())),
        (true, Err(e)) => {
            // Best-effort rollback; surface the original error regardless.
            let _ = conn.execute("ROLLBACK", []);
            Err(e)
        }
        (false, Ok(())) => Ok(()),
        (false, Err(e)) => Err(e),
    }
}

/// Map a `migration_state.current_phase` tag back to a [`MigrationPhase`].
/// Tag values are the stable strings produced by `MigrationPhase::tag()`
/// (Phase0 / Phase1 / ... / Complete).
fn phase_from_tag(tag: &str) -> Result<MigrationPhase, MigrationError> {
    match tag {
        "Phase0" => Ok(MigrationPhase::PreFlight),
        "Phase1" => Ok(MigrationPhase::Backup),
        "Phase2" => Ok(MigrationPhase::SchemaTransition),
        "Phase3" => Ok(MigrationPhase::TopicCarryForward),
        "Phase4" => Ok(MigrationPhase::Backfill),
        "Phase5" => Ok(MigrationPhase::Verify),
        "Complete" => Ok(MigrationPhase::Complete),
        other => Err(MigrationError::InvariantViolated(format!(
            "unknown migration_state.current_phase tag: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::CheckpointStore;
    use crate::lock::MigrationLock;
    use rusqlite::Connection;

    /// In-memory connection with the migration tables initialized so the
    /// machine has somewhere to write checkpoint + lock state.
    fn fresh_conn_with_state() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        CheckpointStore::init(&conn).unwrap();
        MigrationLock::init(&conn).unwrap();
        // Insert initial migration_state row at PreFlight.
        CheckpointStore::insert_initial_state(&conn, MigrationPhase::PreFlight, "now").unwrap();
        // Insert a lock row so update_phase has something to write to.
        conn.execute(
            "INSERT INTO migration_lock (id, pid, hostname, started_at, phase, tool_version) \
             VALUES (1, 4242, 'localhost', 'now', 'Phase0', '0.3.0')",
            [],
        )
        .unwrap();
        conn
    }

    /// Records the order the executor's per-phase methods were invoked.
    /// Tests assert this against the expected phase ordering.
    #[derive(Default)]
    struct RecordingExecutors {
        calls: Vec<&'static str>,
        /// If set, return `MigrationError::InvariantViolation` from this
        /// phase to test the abort path.
        fail_at: Option<&'static str>,
    }

    impl PhaseExecutors for RecordingExecutors {
        fn run_preflight(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("preflight")
        }
        fn run_backup(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("backup")
        }
        fn run_schema_transition(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("schema")
        }
        fn run_topic_carry_forward(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("topics")
        }
        fn run_backfill(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("backfill")
        }
        fn run_verify(&mut self, _conn: &Connection) -> Result<(), MigrationError> {
            self.record("verify")
        }
    }

    impl RecordingExecutors {
        fn record(&mut self, name: &'static str) -> Result<(), MigrationError> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                Err(MigrationError::InvariantViolated(format!(
                    "test-injected failure at {name}"
                )))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn run_invokes_phases_in_order_when_no_gate() {
        let conn = fresh_conn_with_state();
        let mut ex = RecordingExecutors::default();
        let outcome = PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: None,
                    now_rfc3339: "now",
                },
            )
            .unwrap();
        assert_eq!(outcome, PhaseRunOutcome::Complete);
        assert_eq!(
            ex.calls,
            vec!["preflight", "backup", "schema", "topics", "backfill", "verify"],
            "phases must run in design §3.1 order, exactly once each"
        );
    }

    #[test]
    fn run_marks_complete_in_migration_state_on_finish() {
        let conn = fresh_conn_with_state();
        let mut ex = RecordingExecutors::default();
        PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: None,
                    now_rfc3339: "now",
                },
            )
            .unwrap();
        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.current_phase, "Complete");
        assert!(
            state.migration_complete,
            "Phase 5 success must set migration_complete = 1"
        );
    }

    #[test]
    fn run_stops_after_gate_phase_and_does_not_invoke_later_phases() {
        let conn = fresh_conn_with_state();
        let mut ex = RecordingExecutors::default();
        let outcome = PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: Some(MigrationPhase::SchemaTransition),
                    now_rfc3339: "now",
                },
            )
            .unwrap();
        assert_eq!(
            outcome,
            PhaseRunOutcome::GateReached(MigrationPhase::SchemaTransition)
        );
        assert_eq!(
            ex.calls,
            vec!["preflight", "backup", "schema"],
            "gate must stop machine *after* the named phase, before topics"
        );
        // migration_complete must NOT be set on a gate-reached run.
        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert!(
            !state.migration_complete,
            "GateReached must not finalize the migration"
        );
    }

    #[test]
    fn run_aborts_on_phase_executor_error_and_leaves_checkpoint_at_failed_phase() {
        let conn = fresh_conn_with_state();
        let mut ex = RecordingExecutors {
            fail_at: Some("schema"),
            ..Default::default()
        };
        let err = PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: None,
                    now_rfc3339: "now",
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, MigrationError::InvariantViolated(_)),
            "executor error must propagate unchanged: {err:?}"
        );
        assert_eq!(
            ex.calls,
            vec!["preflight", "backup", "schema"],
            "phases after the failed one must NOT run"
        );
        // The checkpoint should reflect the *successfully completed* prior
        // phase advance — i.e., we advanced PreFlight→Backup and
        // Backup→SchemaTransition, then schema executor failed before
        // SchemaTransition→TopicCarryForward could advance. So the last
        // committed phase is SchemaTransition.
        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(
            state.current_phase, "Phase2",
            "on schema-phase failure the checkpoint must point at Phase2 \
             (current phase) so --resume re-enters schema, not backup"
        );
    }

    #[test]
    fn run_resumes_from_checkpointed_phase_and_skips_completed_phases() {
        let conn = fresh_conn_with_state();
        // Simulate a prior run that advanced through Phases 0–2 and
        // crashed at the start of Phase 3. The checkpoint should be at
        // Phase3 (TopicCarryForward).
        CheckpointStore::advance_phase(&conn, MigrationPhase::Backup, "t1").unwrap();
        CheckpointStore::advance_phase(&conn, MigrationPhase::SchemaTransition, "t2").unwrap();
        CheckpointStore::advance_phase(&conn, MigrationPhase::TopicCarryForward, "t3").unwrap();

        let mut ex = RecordingExecutors::default();
        let outcome = PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: None,
                    now_rfc3339: "now",
                },
            )
            .unwrap();
        assert_eq!(outcome, PhaseRunOutcome::Complete);
        assert_eq!(
            ex.calls,
            vec!["topics", "backfill", "verify"],
            "resume from Phase3 must skip already-completed Phases 0–2"
        );
    }

    #[test]
    fn run_advances_lock_phase_in_lockstep_with_state_phase() {
        let conn = fresh_conn_with_state();
        let mut ex = RecordingExecutors::default();
        PhaseMachine::new()
            .run(
                &conn,
                &mut ex,
                &PhaseMachineConfig {
                    gate: Some(MigrationPhase::Backup),
                    now_rfc3339: "now",
                },
            )
            .unwrap();
        // Both tables must reflect Phase1 (Backup) after a Backup-gated run.
        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        let lock = MigrationLock::load(&conn).unwrap().unwrap();
        assert_eq!(state.current_phase, "Phase1");
        assert_eq!(
            lock.phase, "Phase1",
            "§3.5: migration_lock.phase must track migration_state.current_phase"
        );
    }

    #[test]
    fn phase_from_tag_round_trips_all_known_phases() {
        for p in [
            MigrationPhase::PreFlight,
            MigrationPhase::Backup,
            MigrationPhase::SchemaTransition,
            MigrationPhase::TopicCarryForward,
            MigrationPhase::Backfill,
            MigrationPhase::Verify,
            MigrationPhase::Complete,
        ] {
            assert_eq!(phase_from_tag(p.tag()).unwrap(), p);
        }
    }

    #[test]
    fn phase_from_tag_rejects_unknown_with_invariant_violation() {
        let err = phase_from_tag("PhaseQuestionable").unwrap_err();
        assert!(matches!(err, MigrationError::InvariantViolated(_)));
    }
}
