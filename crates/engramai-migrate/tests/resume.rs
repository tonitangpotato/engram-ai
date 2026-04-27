//! T17 — Resume / interrupt + disk-full tests (design §11.3, §11.6).
//!
//! Implements the §11.3 / §11.6 test matrix from
//! `.gid/features/v03-migration/design.md`:
//!
//! - `test_phase_gate_stops_cleanly` (§11.3 last bullet) — `--gate Phase2`
//!   surfaces a gate-reached error and leaves Phase 3 unrun.
//! - `test_resume_after_gate_continues_from_checkpoint` — pre-Phase-4
//!   substitute for `test_resume_after_sigint_phase4`. Uses `--gate Phase1`
//!   to simulate a controlled "interrupt" after Backup, then re-invokes
//!   `migrate()` with `resume = true` + a later gate, asserting the phase
//!   machine resumes at the checkpointed phase rather than rerunning
//!   PreFlight from scratch.
//! - `test_resume_phase_advance_is_monotone` — pre-Phase-4 substitute for
//!   `test_resume_counters_monotone`. Asserts `migration_state.current_phase`
//!   advances monotonically across a (gate, resume, gate, resume, complete)
//!   sequence (i.e., the checkpoint never goes backward).
//! - `test_resume_phase4_state_machine` — invariant test on `MigrationPhase`
//!   transitions, kept here (not in `phase_machine.rs` unit tests) so the
//!   integration suite documents the §3.1 ordering at the library boundary.
//!
//! ## Phase 4 / disk-full coverage — ignored, gated on T9
//!
//! The full §11.3 + §11.6 suite (`test_resume_after_sigint_phase4`,
//! `test_random_kill_resume_matches_clean`, `test_resume_counters_monotone`,
//! `test_resume_disk_full_surfaces_mig_disk_full`) cannot be implemented
//! today: Phase 4 (per-record backfill) is stubbed in
//! [`cli.rs::run_backfill`] because `task:mig-impl-backfill-perrecord`
//! (T9) is blocked on `ResolutionPipeline::resolve_for_backfill` (see
//! `tasks/2026-04-27-night-autopilot-STATUS.md`). The test bodies live
//! below behind `#[ignore]` so they're in place when T9 lands.
//!
//! All tests use `tempfile::tempdir` so they don't touch the working
//! directory.
//!
//! GOAL coverage:
//! - GOAL-4.3 (idempotent migration; partial migrations resumable; no
//!   duplicate rows on re-run).
//! - GOAL-4.8 (phased rollout with observable gates; pause/resume safe).

use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;

use engramai_migrate::{
    migrate, CheckpointStore, MigrateOptions, MigrationPhase, MigrationStateRow,
};

// ---------------------------------------------------------------------------
// Fixtures & helpers
// ---------------------------------------------------------------------------

/// Seed a v0.2-shaped database — same minimum schema as `idempotency.rs`
/// uses, so the two integration files agree on the `SchemaState::V02`
/// detection path.
fn seed_v02_db(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        // We seed BOTH `memories` AND `hebbian_links` because Phase 2
        // (`apply_additive_columns`) ALTERs both. The sibling
        // idempotency.rs fixture omits `hebbian_links` and gets away
        // with it because its assertions only check `is_err()`; here
        // we assert on the *kind* of error (gate-reached vs DDL
        // failure), so the fixture must let Phase 2 succeed.
        // Mirrors the seed shape used by `schema::tests::*`.
        "CREATE TABLE memories (id TEXT PRIMARY KEY, content TEXT, created_at TEXT);\
         INSERT INTO memories VALUES ('m1', 'hello', '2026-01-01T00:00:00Z');\
         INSERT INTO memories VALUES ('m2', 'world', '2026-01-02T00:00:00Z');\
         CREATE TABLE hebbian_links (a TEXT, b TEXT);\
         CREATE TABLE schema_version (version INTEGER PRIMARY KEY, updated_at TEXT NOT NULL);\
         INSERT INTO schema_version VALUES (2, '2026-01-01T00:00:00Z');",
    )
    .unwrap();
}

/// Build a `MigrateOptions` populated with the v0.2-acknowledgement
/// flags every test in this file needs (forward-only ack + skip-backup
/// to avoid sidecar writes muddying the file layout).
fn options_for(db: &Path) -> MigrateOptions {
    let mut opts = MigrateOptions::new(db);
    opts.tool_version = "0.1.0-resume-test".to_string();
    opts.accept_forward_only = true;
    opts.no_backup = true;
    opts.accept_no_grace = true;
    opts
}

/// Read the `migration_state` singleton row (panics on absence — every
/// test in this file runs at least Phase 0 first, which initialises it).
fn load_state(db: &Path) -> MigrationStateRow {
    let conn = Connection::open(db).unwrap();
    CheckpointStore::load_state(&conn)
        .unwrap()
        .expect("migration_state row must exist after Phase 0")
}

// ---------------------------------------------------------------------------
// test_phase_gate_stops_cleanly  (design §11.3, last bullet)
// ---------------------------------------------------------------------------

/// §11.3: `--gate Phase2` causes `migrate()` to terminate **after**
/// Phase 2 (SchemaTransition) without running Phase 3 (TopicCarryForward).
///
/// Per design §3.1 last bullet, Phase 2 is *not* the canonical
/// schema_version=3 bump site (Phase 5 is). So after a Phase2-gated
/// run we expect:
///
///   - `migrate()` returns `Err(MigrationError::InvariantViolated(...))`
///     mapped to `ExitCode::GateReached` by the binary;
///   - `migration_state.current_phase` advanced to `Phase2`;
///   - `migration_state.migration_complete == false`;
///   - `schema_version` row still says 2 (Phase 5 didn't run).
#[test]
fn test_phase_gate_stops_cleanly() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("gate2.db");
    seed_v02_db(&db);

    let mut opts = options_for(&db);
    opts.gate = Some(MigrationPhase::SchemaTransition);

    let result = migrate(&opts);
    assert!(
        result.is_err(),
        "gate-reached on Phase2 must surface as Err; got {:?}",
        result
    );

    // The CLI binary inspects the error message to map to ExitCode::GateReached.
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("gate reached"),
        "error message must include 'gate reached' (binary maps it to ExitCode::GateReached): {err_msg}"
    );

    let state = load_state(&db);
    assert_eq!(
        state.current_phase, "Phase2",
        "after gate at Phase2, checkpoint must point at Phase2"
    );
    assert!(
        !state.migration_complete,
        "GateReached must NOT mark migration_complete = true (only Phase 5 success does)"
    );

    // Phase 5 didn't run, so schema_version is still 2.
    let conn = Connection::open(&db).unwrap();
    let v: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        v, 2,
        "Phase2 gate must NOT bump schema_version (Phase 5 is the canonical bump site)"
    );
}

// ---------------------------------------------------------------------------
// test_resume_after_gate_continues_from_checkpoint  (§11.3 substitute)
// ---------------------------------------------------------------------------

/// §11.3 (pre-Phase-4 substitute): a controlled "interrupt" via
/// `--gate Phase1` leaves the checkpoint at Phase1 (Backup). The
/// follow-up `migrate()` invocation with `resume = true` and a later
/// gate must resume at the checkpointed phase (Phase1) and advance past
/// it — *without* re-running Phase 0 from scratch.
///
/// We assert this through the **checkpoint state**, not through
/// process-level signals: the design's resume contract is "reading
/// `migration_state.current_phase` and entering that phase next"
/// (cli.rs `migrate()` + `phase_machine::run`). The Phase4 SIGINT
/// flavour will assert the same invariant from the subprocess side
/// once T9 unblocks Phase 4.
///
/// **What this proves**: the resume path is wired end-to-end through
/// the public API — the second `migrate()` call doesn't re-execute
/// Phase 0/1 (which would corrupt counters once Phase 4 ships).
#[test]
fn test_resume_after_gate_continues_from_checkpoint() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("resume-gate.db");
    seed_v02_db(&db);

    // Run 1: gate at Phase1 → exits with GateReached after Backup.
    let mut opts1 = options_for(&db);
    opts1.gate = Some(MigrationPhase::Backup);
    let r1 = migrate(&opts1);
    assert!(r1.is_err(), "Phase1 gate must surface Err; got {:?}", r1);

    // Checkpoint: Phase1, not Phase0 (advance happens *after* phase
    // execution, gated *before* the next advance).
    let state1 = load_state(&db);
    assert_eq!(state1.current_phase, "Phase1");
    assert!(!state1.migration_complete);

    // Run 2: same DB, resume = true, gate at Phase2 → must resume at
    // Phase1 (re-run is safe; Phase1/Backup is idempotent because we
    // disabled backup with no_backup), advance to Phase2, exit cleanly.
    let mut opts2 = options_for(&db);
    opts2.resume = true;
    opts2.gate = Some(MigrationPhase::SchemaTransition);
    let r2 = migrate(&opts2);
    assert!(
        r2.is_err(),
        "second run gated at Phase2 must surface gate-reached Err; got {:?}",
        r2
    );

    // Checkpoint advanced monotonically Phase1 → Phase2; not reset.
    let state2 = load_state(&db);
    assert_eq!(
        state2.current_phase, "Phase2",
        "resume must advance the phase, not reset it to Phase0"
    );
    assert!(!state2.migration_complete);

    // started_at must NOT change across resume — it's the original-run
    // anchor per design §5.4 ("started_at only changes on a fresh
    // (non-resume) run").
    assert_eq!(
        state1.started_at, state2.started_at,
        "started_at must be invariant across --resume invocations \
         (design §5.4: anchors the original run's identity)"
    );
}

// ---------------------------------------------------------------------------
// test_resume_phase_advance_is_monotone  (§11.3 substitute)
// ---------------------------------------------------------------------------

/// §11.3 (pre-Phase-4 substitute for `test_resume_counters_monotone`):
/// across a sequence of (gate, resume, gate, resume) runs, the phase
/// pointer in `migration_state.current_phase` is monotone non-decreasing.
///
/// Once T9 lands and Phase 4 is live, this same invariant will be
/// extended in `test_resume_counters_monotone` to cover
/// `records_processed` / `records_succeeded` / `records_failed`. The
/// counter version requires Phase 4 actually doing work; the phase
/// version is testable today and catches the most common resume bug
/// shape (an off-by-one or "restart from PreFlight" regression that
/// would corrupt counters when they exist).
#[test]
fn test_resume_phase_advance_is_monotone() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("monotone.db");
    seed_v02_db(&db);

    let mut phases_seen: Vec<MigrationPhase> = Vec::new();

    // Step 1: gate at Phase0 (PreFlight) — exit immediately after
    // preflight runs.
    let mut o = options_for(&db);
    o.gate = Some(MigrationPhase::PreFlight);
    let _ = migrate(&o);
    let s = load_state(&db);
    phases_seen.push(parse_phase(&s.current_phase));

    // Step 2: resume + gate at Phase1.
    let mut o = options_for(&db);
    o.resume = true;
    o.gate = Some(MigrationPhase::Backup);
    let _ = migrate(&o);
    let s = load_state(&db);
    phases_seen.push(parse_phase(&s.current_phase));

    // Step 3: resume + gate at Phase2.
    let mut o = options_for(&db);
    o.resume = true;
    o.gate = Some(MigrationPhase::SchemaTransition);
    let _ = migrate(&o);
    let s = load_state(&db);
    phases_seen.push(parse_phase(&s.current_phase));

    // Phases must be monotone non-decreasing.
    let as_u8: Vec<u8> = phases_seen.iter().map(|p| phase_ordinal(*p)).collect();
    for w in as_u8.windows(2) {
        assert!(
            w[0] <= w[1],
            "phase pointer regressed across resume: {phases_seen:?} (ordinals {as_u8:?})"
        );
    }

    // And it must actually have *advanced* (otherwise the test is
    // vacuously true — a resume that never moves forward is a bug).
    assert!(
        as_u8.last() > as_u8.first(),
        "resume sequence never advanced the phase: {phases_seen:?}"
    );
    assert_eq!(
        phases_seen.last(),
        Some(&MigrationPhase::SchemaTransition),
        "final state should be Phase2 (last gate)"
    );
}

// ---------------------------------------------------------------------------
// test_resume_phase4_state_machine — invariant on §3.1 ordering
// ---------------------------------------------------------------------------

/// Invariant test at the library boundary: the `MigrationPhase` enum's
/// `tag()` ↔ `Display` mapping matches the §3.1 phase order. This is
/// already covered by unit tests in `phase_machine.rs`, but recording
/// it here pins the contract at the **public** surface (the integration
/// suite is what external consumers / downstream crates would depend on).
///
/// If a future refactor renames `Phase4` to `BackfillPhase` or similar,
/// every external test (incl. T17) breaks loudly here, not silently
/// inside `phase_machine.rs`.
#[test]
fn test_resume_phase_tags_match_design_order() {
    // Design §3.1 phase order:
    //   Phase 0 = PreFlight  (tag "Phase0")
    //   Phase 1 = Backup     (tag "Phase1")
    //   Phase 2 = SchemaTransition (tag "Phase2")
    //   Phase 3 = TopicCarryForward (tag "Phase3")
    //   Phase 4 = Backfill   (tag "Phase4")
    //   Phase 5 = Verify     (tag "Phase5")
    //   Terminal = Complete  (tag "Complete")
    let pairs = [
        (MigrationPhase::PreFlight, "Phase0"),
        (MigrationPhase::Backup, "Phase1"),
        (MigrationPhase::SchemaTransition, "Phase2"),
        (MigrationPhase::TopicCarryForward, "Phase3"),
        (MigrationPhase::Backfill, "Phase4"),
        (MigrationPhase::Verify, "Phase5"),
        (MigrationPhase::Complete, "Complete"),
    ];
    for (phase, expected_tag) in pairs {
        assert_eq!(
            phase.tag(),
            expected_tag,
            "phase {:?} must serialize as '{}' for resume to read \
             migration_state.current_phase correctly",
            phase,
            expected_tag
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 4 / disk-full tests — gated on T9
// ---------------------------------------------------------------------------

/// §11.3 first bullet — full SIGINT-driven Phase 4 resume test.
///
/// **Currently `#[ignore]` — gated on `task:mig-impl-backfill-perrecord`
/// (T9).** Phase 4 is stubbed in `cli.rs::run_backfill` (returns
/// `InvariantViolated` on live runs) because
/// `ResolutionPipeline::resolve_for_backfill` doesn't exist yet
/// (see `tasks/2026-04-27-night-autopilot-STATUS.md`).
///
/// When T9 lands, replace the `#[ignore]` body with the design §11.3
/// flow:
///
/// 1. Spawn the binary as a subprocess (`engramai migrate --batch-size=10`).
/// 2. After ~3 progress emissions, send SIGINT.
/// 3. Assert exit code 2 and `migration_state` reflects partial progress.
/// 4. Run `engramai migrate --resume`.
/// 5. Diff final DB state against an uninterrupted-baseline run (byte-
///    identical on `graph_*` tables modulo `occurred_at` timestamps,
///    which we normalize before comparison).
///
/// The Phase-machine-level resume contract is already covered by
/// `test_resume_after_gate_continues_from_checkpoint` (this file) +
/// `phase_machine::tests::run_resumes_from_checkpointed_phase_and_skips_completed_phases`
/// (unit-level). What this test adds when it ships is the **subprocess
/// and signal-handling** layer (SIGINT handler in the binary, NDJSON
/// progress emission cadence, exit-code mapping to `ExitCode::Paused`).
#[test]
#[ignore = "T9 (mig-impl-backfill-perrecord) blocked — Phase 4 is a stub; \
            see cli.rs::run_backfill"]
fn test_resume_after_sigint_phase4() {
    // TODO(T9): implement per design §11.3 first bullet.
    unimplemented!("blocked on T9");
}

/// §11.3 second bullet — randomized fuzz: kill the migration subprocess
/// at N random points during Phase 4, `--resume` after each, verify
/// final state matches the uninterrupted baseline.
///
/// **Currently `#[ignore]` — gated on T9.** See
/// `test_resume_after_sigint_phase4` for the rationale.
///
/// When T9 lands: 20 seeds in CI per the design contract. The hard-kill
/// (vs. SIGINT) flavour stresses the "transaction not yet committed"
/// resume path — distinct from the cooperative-pause path that
/// `test_resume_after_sigint_phase4` covers.
#[test]
#[ignore = "T9 (mig-impl-backfill-perrecord) blocked — Phase 4 is a stub"]
fn test_random_kill_resume_matches_clean() {
    // TODO(T9): implement per design §11.3 second bullet.
    unimplemented!("blocked on T9");
}

/// §11.3 third bullet — `records_processed` / `records_succeeded`
/// monotone non-decreasing across (run, interrupt, resume, interrupt,
/// resume, complete).
///
/// **Currently `#[ignore]` — gated on T9.** The phase-pointer
/// monotonicity invariant is covered today by
/// `test_resume_phase_advance_is_monotone` (this file). When T9 lands,
/// extend that test or replace this stub with the counter version.
#[test]
#[ignore = "T9 (mig-impl-backfill-perrecord) blocked — Phase 4 counters \
            do not advance until backfill is implemented"]
fn test_resume_counters_monotone() {
    // TODO(T9): implement per design §11.3 third bullet.
    // Pseudocode:
    //   1. Run with batch-size=5; SIGINT after ~10 records.
    //   2. snapshot1 = (records_processed, records_succeeded, records_failed)
    //   3. --resume; SIGINT after another ~10 records.
    //   4. snapshot2 = (...); assert each field >= snapshot1's.
    //   5. --resume to completion.
    //   6. snapshot3 = (...); assert each field >= snapshot2's.
    unimplemented!("blocked on T9");
}

/// §11.6 — disk-full mid-resume surfaces `MIG_DISK_FULL` (exit code 10),
/// preserves the checkpoint, and resumes successfully after disk grows.
///
/// **Currently `#[ignore]` — gated on T9 AND requires Linux tmpfs.**
/// Per design §11.6, on macOS this test must be `#[cfg_attr(target_os
/// = "macos", ignore)]` and the Linux CI is the authoritative
/// coverage. The double-gate (T9 + Linux) is why we keep it inert here
/// rather than half-implementing it on macOS.
///
/// When T9 lands and the test runs on Linux:
///
/// 1. Mount a 32 MiB tmpfs into a temp dir.
/// 2. Place the v0.2 fixture there + size batch so WAL projects
///    to exceed available space mid-Phase-4.
/// 3. Assert exit code 10 (`MIG_DISK_FULL`).
/// 4. Assert checkpoint reflects last successfully committed record
///    (not corrupted).
/// 5. Grow tmpfs; `--resume`; assert completion + byte-identical
///    final state vs an uninterrupted run (modulo timestamps).
#[test]
#[ignore = "T9 (mig-impl-backfill-perrecord) blocked AND Linux-only \
            (macOS lacks tmpfs); see design §11.6"]
fn test_resume_disk_full_surfaces_mig_disk_full() {
    // TODO(T9): implement per design §11.6.
    unimplemented!("blocked on T9 + needs Linux tmpfs");
}

// ---------------------------------------------------------------------------
// Local helpers — phase ↔ ordinal mapping for the monotonicity test
// ---------------------------------------------------------------------------

/// Parse a `migration_state.current_phase` tag into the public enum.
/// Panics on unknown — the integration suite is allowed to assume the
/// library never writes garbage here (covered by
/// `phase_machine::tests::phase_from_tag_*`).
fn parse_phase(tag: &str) -> MigrationPhase {
    match tag {
        "Phase0" => MigrationPhase::PreFlight,
        "Phase1" => MigrationPhase::Backup,
        "Phase2" => MigrationPhase::SchemaTransition,
        "Phase3" => MigrationPhase::TopicCarryForward,
        "Phase4" => MigrationPhase::Backfill,
        "Phase5" => MigrationPhase::Verify,
        "Complete" => MigrationPhase::Complete,
        other => panic!("unknown phase tag in migration_state: {other:?}"),
    }
}

/// Map a phase to its §3.1 ordinal so we can compare with `<=`. Defined
/// here (not on the enum) to keep the public API surface minimal.
fn phase_ordinal(p: MigrationPhase) -> u8 {
    match p {
        MigrationPhase::PreFlight => 0,
        MigrationPhase::Backup => 1,
        MigrationPhase::SchemaTransition => 2,
        MigrationPhase::TopicCarryForward => 3,
        MigrationPhase::Backfill => 4,
        MigrationPhase::Verify => 5,
        MigrationPhase::Complete => 6,
    }
}
