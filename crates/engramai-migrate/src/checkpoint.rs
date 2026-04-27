//! Checkpoint persistence and phase-digest integrity (§5.4).
//!
//! This module owns the two SQLite tables that make migration crash-safe and
//! resumable:
//!
//! 1. `migration_state` — singleton row (`CHECK (id = 1)`) tracking the current
//!    phase and monotone counters (`records_processed`, `records_succeeded`,
//!    `records_failed`). Updated *inside* the same transaction as per-record
//!    graph writes (§5.4 "advance-after-commit" invariant).
//!
//! 2. `migration_phase_digest` — one row per completed phase, recording a
//!    SHA-256 digest of stable canonicalized content (§5.4 phase digests).
//!    On resume, every completed phase's digest is recomputed; mismatch =>
//!    `MIG_CHECKPOINT_DIGEST_MISMATCH` (§10.4).
//!
//! Concrete digest *content* (what bytes are hashed for Phase 2/3/4) lives in
//! the phase implementations — this module supplies the hashing primitives,
//! storage, and verification flow only. That separation keeps the checkpoint
//! store free of phase-specific schema knowledge.

use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};

use crate::error::MigrationError;
use crate::progress::MigrationPhase;

// ---------------------------------------------------------------------------
// DDL
// ---------------------------------------------------------------------------

/// `CREATE TABLE` statements for the checkpoint subsystem (§5.4).
///
/// Both statements are idempotent (`IF NOT EXISTS`) so re-running on an
/// already-initialized DB is a no-op.
pub const CHECKPOINT_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS migration_state (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    current_phase            TEXT    NOT NULL,
    last_processed_memory_id INTEGER,
    records_processed        INTEGER NOT NULL DEFAULT 0,
    records_succeeded        INTEGER NOT NULL DEFAULT 0,
    records_failed           INTEGER NOT NULL DEFAULT 0,
    started_at               TEXT    NOT NULL,
    updated_at               TEXT    NOT NULL,
    migration_complete       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS migration_phase_digest (
    phase        TEXT    PRIMARY KEY,
    completed_at TEXT    NOT NULL,
    row_counts   TEXT    NOT NULL,
    content_hash TEXT    NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// Row structs
// ---------------------------------------------------------------------------

/// One row of `migration_state` (singleton; `id = 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStateRow {
    /// Always `1` — table is constrained to a single row.
    pub id: i64,
    /// Current phase tag (matches `MigrationPhase::tag()`, e.g. `"Phase4"`).
    pub current_phase: String,
    /// `-1` sentinel = no records processed yet (per design §5.4).
    pub last_processed_memory_id: i64,
    pub records_processed: i64,
    pub records_succeeded: i64,
    pub records_failed: i64,
    /// RFC 3339 timestamp from the migration's first `init`.
    pub started_at: String,
    /// RFC 3339 timestamp of the most recent state mutation.
    pub updated_at: String,
    /// 0 = in progress, 1 = post-Phase 5 gate passed.
    pub migration_complete: bool,
}

/// One row of `migration_phase_digest` (PK = `phase`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseDigestRow {
    pub phase: String,
    pub completed_at: String,
    /// JSON object, e.g. `{"graph_entities": 47213, "graph_edges": 102914}`.
    pub row_counts: String,
    /// Hex SHA-256 of the phase's canonical content. May itself be a JSON
    /// object for multi-table phases (Phase 4: `{"entities":"…","edges":"…"}`).
    pub content_hash: String,
}

// ---------------------------------------------------------------------------
// Sentinel
// ---------------------------------------------------------------------------

/// Sentinel value for `last_processed_memory_id` when no records have been
/// processed yet. Design §5.4 specifies `-1` (not NULL) so the comparison
/// `id > last_processed_memory_id` works without a NULL guard.
pub const NO_RECORDS_PROCESSED: i64 = -1;

// ---------------------------------------------------------------------------
// CheckpointStore
// ---------------------------------------------------------------------------

/// Read/write façade over the checkpoint tables.
///
/// `CheckpointStore` is intentionally a thin wrapper around `&Connection` —
/// it does not own the connection. Callers (the orchestrator, individual
/// phases) pass in their own `&Connection` or `&Transaction`, which lets
/// state writes share the same transaction as the per-record graph writes
/// (§5.4 "advance-after-commit" invariant).
pub struct CheckpointStore;

impl CheckpointStore {
    /// Create the DDL on a fresh DB (idempotent).
    pub fn init(conn: &Connection) -> Result<(), MigrationError> {
        conn.execute_batch(CHECKPOINT_DDL).map_err(map_sqlite)?;
        Ok(())
    }

    // ----- migration_state CRUD -------------------------------------------

    /// Insert the singleton state row. Errors if a row already exists.
    pub fn insert_initial_state(
        conn: &Connection,
        phase: MigrationPhase,
        now_rfc3339: &str,
    ) -> Result<MigrationStateRow, MigrationError> {
        let phase_tag = phase.tag();
        conn.execute(
            "INSERT INTO migration_state \
             (id, current_phase, last_processed_memory_id, records_processed, \
              records_succeeded, records_failed, started_at, updated_at, \
              migration_complete) \
             VALUES (1, ?1, ?2, 0, 0, 0, ?3, ?3, 0)",
            params![phase_tag, NO_RECORDS_PROCESSED, now_rfc3339],
        )
        .map_err(map_sqlite)?;

        Self::load_state(conn).map(|opt| opt.expect("row just inserted"))
    }

    /// Fetch the singleton row, if it exists.
    pub fn load_state(
        conn: &Connection,
    ) -> Result<Option<MigrationStateRow>, MigrationError> {
        conn.query_row(
            "SELECT id, current_phase, last_processed_memory_id, \
                    records_processed, records_succeeded, records_failed, \
                    started_at, updated_at, migration_complete \
             FROM migration_state WHERE id = 1",
            [],
            row_to_state,
        )
        .optional()
        .map_err(map_sqlite)
    }

    /// Move to a new phase (e.g., Phase2 → Phase3). Updates `current_phase`
    /// and `updated_at`. Counters are *not* reset (design §5.4: counters
    /// accumulate across phases and across resumes).
    pub fn advance_phase(
        conn: &Connection,
        new_phase: MigrationPhase,
        now_rfc3339: &str,
    ) -> Result<(), MigrationError> {
        let n = conn
            .execute(
                "UPDATE migration_state \
                 SET current_phase = ?1, updated_at = ?2 \
                 WHERE id = 1",
                params![new_phase.tag(), now_rfc3339],
            )
            .map_err(map_sqlite)?;
        debug_assert_eq!(n, 1, "migration_state singleton must exist");
        Ok(())
    }

    /// Update Phase 4 backfill counters and cursor. Designed to be called
    /// inside the same transaction as the per-record graph write so the
    /// "advance-after-commit" invariant holds.
    ///
    /// Counters are written as deltas to the existing row (monotone +=).
    pub fn update_backfill_progress(
        conn: &Connection,
        last_processed_memory_id: i64,
        delta_processed: i64,
        delta_succeeded: i64,
        delta_failed: i64,
        now_rfc3339: &str,
    ) -> Result<(), MigrationError> {
        let n = conn
            .execute(
                "UPDATE migration_state \
                 SET last_processed_memory_id = ?1, \
                     records_processed = records_processed + ?2, \
                     records_succeeded = records_succeeded + ?3, \
                     records_failed    = records_failed    + ?4, \
                     updated_at        = ?5 \
                 WHERE id = 1",
                params![
                    last_processed_memory_id,
                    delta_processed,
                    delta_succeeded,
                    delta_failed,
                    now_rfc3339,
                ],
            )
            .map_err(map_sqlite)?;
        debug_assert_eq!(n, 1, "migration_state singleton must exist");
        Ok(())
    }

    /// Mark migration as complete (post-Phase 5 gate).
    pub fn mark_complete(
        conn: &Connection,
        now_rfc3339: &str,
    ) -> Result<(), MigrationError> {
        let n = conn
            .execute(
                "UPDATE migration_state \
                 SET migration_complete = 1, current_phase = ?1, updated_at = ?2 \
                 WHERE id = 1",
                params![MigrationPhase::Complete.tag(), now_rfc3339],
            )
            .map_err(map_sqlite)?;
        debug_assert_eq!(n, 1, "migration_state singleton must exist");
        Ok(())
    }

    // ----- migration_phase_digest CRUD ------------------------------------

    /// Insert (or replace) a phase-digest row. The same phase running twice
    /// (e.g., re-verify after a fix) is allowed; the latest write wins.
    pub fn put_phase_digest(
        conn: &Connection,
        row: &PhaseDigestRow,
    ) -> Result<(), MigrationError> {
        conn.execute(
            "INSERT INTO migration_phase_digest \
             (phase, completed_at, row_counts, content_hash) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(phase) DO UPDATE SET \
                completed_at = excluded.completed_at, \
                row_counts   = excluded.row_counts, \
                content_hash = excluded.content_hash",
            params![row.phase, row.completed_at, row.row_counts, row.content_hash],
        )
        .map_err(map_sqlite)?;
        Ok(())
    }

    /// Read a single phase-digest row, if present.
    pub fn load_phase_digest(
        conn: &Connection,
        phase: MigrationPhase,
    ) -> Result<Option<PhaseDigestRow>, MigrationError> {
        conn.query_row(
            "SELECT phase, completed_at, row_counts, content_hash \
             FROM migration_phase_digest WHERE phase = ?1",
            params![phase.tag()],
            row_to_digest,
        )
        .optional()
        .map_err(map_sqlite)
    }

    /// All recorded phase digests, ordered by phase tag for stability.
    pub fn list_phase_digests(
        conn: &Connection,
    ) -> Result<Vec<PhaseDigestRow>, MigrationError> {
        let mut stmt = conn
            .prepare(
                "SELECT phase, completed_at, row_counts, content_hash \
                 FROM migration_phase_digest ORDER BY phase ASC",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], row_to_digest)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    /// Verify a freshly-recomputed digest against the stored value.
    ///
    /// Returns `Ok(())` on match. On mismatch, returns
    /// `MigrationError::CheckpointDigestMismatch { phase }` — the caller is
    /// responsible for honoring `--ignore-digest-mismatch` (the design's
    /// documented escape hatch) before treating this as fatal.
    ///
    /// On "no stored digest", returns `Ok(())` — verification is a no-op for
    /// phases that have not yet run. Callers that require the digest to
    /// exist should check via [`load_phase_digest`] first.
    pub fn verify_phase_digest(
        conn: &Connection,
        phase: MigrationPhase,
        recomputed_hex: &str,
    ) -> Result<(), MigrationError> {
        match Self::load_phase_digest(conn, phase)? {
            None => Ok(()),
            Some(row) if row.content_hash == recomputed_hex => Ok(()),
            Some(_) => Err(MigrationError::CheckpointDigestMismatch {
                phase: phase.tag().to_string(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Digest primitive
// ---------------------------------------------------------------------------

/// Compute hex SHA-256 of an arbitrary byte slice. Centralized so every phase
/// uses the same canonicalization / encoding.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn row_to_state(row: &Row<'_>) -> rusqlite::Result<MigrationStateRow> {
    Ok(MigrationStateRow {
        id: row.get(0)?,
        current_phase: row.get(1)?,
        last_processed_memory_id: row.get::<_, Option<i64>>(2)?.unwrap_or(NO_RECORDS_PROCESSED),
        records_processed: row.get(3)?,
        records_succeeded: row.get(4)?,
        records_failed: row.get(5)?,
        started_at: row.get(6)?,
        updated_at: row.get(7)?,
        migration_complete: row.get::<_, i64>(8)? != 0,
    })
}

fn row_to_digest(row: &Row<'_>) -> rusqlite::Result<PhaseDigestRow> {
    Ok(PhaseDigestRow {
        phase: row.get(0)?,
        completed_at: row.get(1)?,
        row_counts: row.get(2)?,
        content_hash: row.get(3)?,
    })
}

fn map_sqlite(e: rusqlite::Error) -> MigrationError {
    // The migration crate doesn't have a dedicated "checkpoint write fail"
    // variant in the public catalog (§10.4); rusqlite errors at the
    // checkpoint layer are unrecoverable schema/I-O problems and surface
    // through the closest existing variant, `DdlFailed`. The string carries
    // the full rusqlite message so operators can diagnose.
    MigrationError::DdlFailed(format!("checkpoint store: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        CheckpointStore::init(&c).unwrap();
        c
    }

    const T1: &str = "2026-04-27T10:00:00Z";
    const T2: &str = "2026-04-27T10:05:00Z";
    const T3: &str = "2026-04-27T10:10:00Z";

    // ---- DDL --------------------------------------------------------------

    #[test]
    fn init_is_idempotent() {
        let c = Connection::open_in_memory().unwrap();
        CheckpointStore::init(&c).unwrap();
        CheckpointStore::init(&c).unwrap(); // second call must not error
    }

    #[test]
    fn init_creates_both_tables() {
        let c = fresh();
        let mut stmt = c
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(names.contains(&"migration_state".to_string()));
        assert!(names.contains(&"migration_phase_digest".to_string()));
    }

    // ---- migration_state singleton ---------------------------------------

    #[test]
    fn insert_initial_creates_row_with_zero_counters() {
        let c = fresh();
        let row = CheckpointStore::insert_initial_state(&c, MigrationPhase::PreFlight, T1).unwrap();
        assert_eq!(row.id, 1);
        assert_eq!(row.current_phase, "Phase0");
        assert_eq!(row.last_processed_memory_id, NO_RECORDS_PROCESSED);
        assert_eq!(row.records_processed, 0);
        assert_eq!(row.records_succeeded, 0);
        assert_eq!(row.records_failed, 0);
        assert_eq!(row.started_at, T1);
        assert_eq!(row.updated_at, T1);
        assert!(!row.migration_complete);
    }

    #[test]
    fn second_insert_violates_singleton_check() {
        let c = fresh();
        CheckpointStore::insert_initial_state(&c, MigrationPhase::PreFlight, T1).unwrap();
        // Second INSERT collides with PRIMARY KEY (id = 1).
        let err = CheckpointStore::insert_initial_state(&c, MigrationPhase::PreFlight, T2);
        assert!(err.is_err(), "second insert must fail (singleton)");
    }

    #[test]
    fn load_state_returns_none_when_empty() {
        let c = fresh();
        assert!(CheckpointStore::load_state(&c).unwrap().is_none());
    }

    #[test]
    fn advance_phase_updates_phase_and_timestamp_only() {
        let c = fresh();
        let r0 = CheckpointStore::insert_initial_state(&c, MigrationPhase::Backup, T1).unwrap();
        // Mutate counters then advance — the advance must not reset them.
        CheckpointStore::update_backfill_progress(&c, 100, 5, 4, 1, T2).unwrap();
        CheckpointStore::advance_phase(&c, MigrationPhase::Backfill, T3).unwrap();
        let r1 = CheckpointStore::load_state(&c).unwrap().unwrap();
        assert_eq!(r1.current_phase, "Phase4");
        assert_eq!(r1.updated_at, T3);
        // Counters preserved across the phase advance:
        assert_eq!(r1.records_processed, 5);
        assert_eq!(r1.records_succeeded, 4);
        assert_eq!(r1.records_failed, 1);
        assert_eq!(r1.last_processed_memory_id, 100);
        // started_at never changes after init.
        assert_eq!(r1.started_at, r0.started_at);
    }

    #[test]
    fn update_backfill_progress_accumulates_monotonically() {
        let c = fresh();
        CheckpointStore::insert_initial_state(&c, MigrationPhase::Backfill, T1).unwrap();

        CheckpointStore::update_backfill_progress(&c, 10, 10, 9, 1, T2).unwrap();
        CheckpointStore::update_backfill_progress(&c, 25, 15, 14, 1, T3).unwrap();

        let r = CheckpointStore::load_state(&c).unwrap().unwrap();
        assert_eq!(r.records_processed, 25, "10 + 15");
        assert_eq!(r.records_succeeded, 23, "9 + 14");
        assert_eq!(r.records_failed, 2, "1 + 1");
        assert_eq!(r.last_processed_memory_id, 25);
        assert_eq!(
            r.records_processed,
            r.records_succeeded + r.records_failed,
            "design §5.4 invariant: processed = succeeded + failed"
        );
    }

    #[test]
    fn mark_complete_sets_flag_and_phase() {
        let c = fresh();
        CheckpointStore::insert_initial_state(&c, MigrationPhase::Verify, T1).unwrap();
        CheckpointStore::mark_complete(&c, T2).unwrap();
        let r = CheckpointStore::load_state(&c).unwrap().unwrap();
        assert!(r.migration_complete);
        assert_eq!(r.current_phase, "Complete");
        assert_eq!(r.updated_at, T2);
    }

    // ---- migration_phase_digest -----------------------------------------

    fn mk_digest(phase: &str, hash: &str) -> PhaseDigestRow {
        PhaseDigestRow {
            phase: phase.to_string(),
            completed_at: T1.to_string(),
            row_counts: r#"{"graph_entities":47213}"#.to_string(),
            content_hash: hash.to_string(),
        }
    }

    #[test]
    fn put_and_load_phase_digest_roundtrip() {
        let c = fresh();
        let row = mk_digest("Phase2", "deadbeef");
        CheckpointStore::put_phase_digest(&c, &row).unwrap();
        let loaded = CheckpointStore::load_phase_digest(&c, MigrationPhase::SchemaTransition)
            .unwrap()
            .unwrap();
        assert_eq!(loaded, row);
    }

    #[test]
    fn put_phase_digest_upserts_on_conflict() {
        let c = fresh();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase3", "aaaa")).unwrap();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase3", "bbbb")).unwrap();
        let r = CheckpointStore::load_phase_digest(&c, MigrationPhase::TopicCarryForward)
            .unwrap()
            .unwrap();
        assert_eq!(r.content_hash, "bbbb", "later put_phase_digest must win");
        // Still only one row for Phase3:
        let all = CheckpointStore::list_phase_digests(&c).unwrap();
        assert_eq!(all.iter().filter(|r| r.phase == "Phase3").count(), 1);
    }

    #[test]
    fn load_phase_digest_returns_none_when_absent() {
        let c = fresh();
        let r = CheckpointStore::load_phase_digest(&c, MigrationPhase::Backfill).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn list_phase_digests_orders_by_phase() {
        let c = fresh();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase4", "44")).unwrap();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase2", "22")).unwrap();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase3", "33")).unwrap();
        let rows = CheckpointStore::list_phase_digests(&c).unwrap();
        let phases: Vec<_> = rows.iter().map(|r| r.phase.as_str()).collect();
        assert_eq!(phases, vec!["Phase2", "Phase3", "Phase4"]);
    }

    // ---- digest verification --------------------------------------------

    #[test]
    fn verify_phase_digest_matches_returns_ok() {
        let c = fresh();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase2", "abc123")).unwrap();
        CheckpointStore::verify_phase_digest(&c, MigrationPhase::SchemaTransition, "abc123")
            .unwrap();
    }

    #[test]
    fn verify_phase_digest_mismatch_returns_error_with_tag() {
        let c = fresh();
        CheckpointStore::put_phase_digest(&c, &mk_digest("Phase2", "stored")).unwrap();
        let err = CheckpointStore::verify_phase_digest(
            &c,
            MigrationPhase::SchemaTransition,
            "different",
        )
        .unwrap_err();
        match err {
            MigrationError::CheckpointDigestMismatch { phase } => {
                assert_eq!(phase, "Phase2");
            }
            other => panic!("expected CheckpointDigestMismatch, got {:?}", other),
        }
    }

    #[test]
    fn verify_phase_digest_no_stored_row_is_noop() {
        // When no digest has been stored yet for a phase, verification must
        // be a no-op (the caller is checking *if* a stored value matches —
        // absence is not by itself corruption). Callers that require
        // presence must use load_phase_digest first.
        let c = fresh();
        CheckpointStore::verify_phase_digest(&c, MigrationPhase::Backfill, "anything").unwrap();
    }

    // ---- digest primitive -----------------------------------------------

    #[test]
    fn sha256_hex_is_stable_and_correct() {
        // Known SHA-256 of empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Known SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_hex_distinguishes_inputs() {
        assert_ne!(sha256_hex(b"phase2"), sha256_hex(b"phase3"));
    }

    // ---- transactional advance-after-commit (§5.4 invariant) ------------

    #[test]
    fn checkpoint_update_inside_transaction_rolls_back_on_abort() {
        // The "advance-after-commit" invariant: a record is only marked
        // processed if its graph delta successfully persisted. We model
        // that here by performing the checkpoint update inside an explicit
        // transaction and rolling it back — the counters must NOT advance.
        let mut c = fresh();
        CheckpointStore::insert_initial_state(&c, MigrationPhase::Backfill, T1).unwrap();

        let tx = c.transaction().unwrap();
        CheckpointStore::update_backfill_progress(&tx, 99, 50, 49, 1, T2).unwrap();
        // Sanity inside the tx:
        let r_in_tx = CheckpointStore::load_state(&tx).unwrap().unwrap();
        assert_eq!(r_in_tx.records_processed, 50);
        // Abort.
        tx.rollback().unwrap();

        let r_after = CheckpointStore::load_state(&c).unwrap().unwrap();
        assert_eq!(r_after.records_processed, 0, "rollback discarded the bump");
        assert_eq!(r_after.last_processed_memory_id, NO_RECORDS_PROCESSED);
    }

    #[test]
    fn checkpoint_update_inside_transaction_persists_on_commit() {
        let mut c = fresh();
        CheckpointStore::insert_initial_state(&c, MigrationPhase::Backfill, T1).unwrap();

        let tx = c.transaction().unwrap();
        CheckpointStore::update_backfill_progress(&tx, 99, 50, 49, 1, T2).unwrap();
        tx.commit().unwrap();

        let r = CheckpointStore::load_state(&c).unwrap().unwrap();
        assert_eq!(r.records_processed, 50);
        assert_eq!(r.last_processed_memory_id, 99);
    }
}
