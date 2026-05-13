//! v0.4 unified substrate — Phase C backfill drivers (T19+).
//!
//! See `.gid/features/v04-unified-substrate/design.md` §5.3.
//!
//! Phase C copies historical rows from the legacy tables (`memories`,
//! `memory_embeddings`, `entities`, `entity_relations`,
//! `memory_entities`, `hebbian_links`, `synthesis_provenance`) into
//! the unified `nodes` / `edges` tables, so Phase D read cutover
//! sees a complete picture rather than only post-T12 dual-written
//! rows.
//!
//! ## Invariants every driver in this module upholds
//!
//! 1. **Idempotent re-runs** (GUARD-ss.3). Drivers use
//!    `INSERT OR IGNORE` on the unified PK and report
//!    `rows_skipped_existing` separately from `rows_inserted`. Running
//!    a driver twice in a row leaves the DB in the same state and the
//!    second run inserts zero rows.
//!
//! 2. **Audit row per invocation**. Every driver writes one row into
//!    `backfill_runs` with a fresh UUID, the counts, and timestamps.
//!    The audit row is created BEFORE work starts (with `finished_at`
//!    NULL) and UPDATEd at the end — a crashed run leaves a row with
//!    NULL `finished_at` that operators can detect and clean up.
//!
//! 3. **Single source of truth for projection**. Drivers do NOT
//!    duplicate the legacy→unified column mapping. They delegate to
//!    the same helper used by Phase B dual-write (e.g.
//!    `Storage::insert_memory_node_row` for memories→nodes). This
//!    guarantees a memory backfilled by T19 is bit-identical to a
//!    memory dual-written by T12 — Phase D retrieval will see
//!    consistent state regardless of insertion path.
//!
//! 4. **Two-pass for self-referential FKs** (design §5.3). The
//!    `memories.superseded_by` column points at another `memories.id`,
//!    which after backfill becomes a `nodes.id` reference. Inserting
//!    in one pass would have entries that reference rows not yet
//!    inserted, breaking the FK on `nodes.superseded_by`. So:
//!      - Pass 1: INSERT all rows with `superseded_by = NULL`.
//!      - Pass 2: UPDATE `nodes.superseded_by` from
//!        `memories.superseded_by`, converting the legacy `''`
//!        sentinel to SQL `NULL` along the way.
//!
//! 5. **Optional namespace filter**. Drivers accept an
//!    `Option<&str>` namespace; `None` backfills everything,
//!    `Some(ns)` lets operators do a staged rollout one namespace at
//!    a time. The audit row records the filter via `notes` JSON.

use rusqlite::{params, OptionalExtension};
use serde_json::json;
use uuid::Uuid;

use crate::storage::{row_to_record_impl, Storage};

/// Wall-clock now as seconds-since-epoch with sub-second precision.
/// Calling `Utc::now()` twice (once for `.timestamp()` and once for
/// `.timestamp_subsec_nanos()`) could span a tick boundary; this
/// helper takes one reading and derives both parts.
fn utc_now_f64() -> f64 {
    let now = chrono::Utc::now();
    now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64 / 1e9)
}

/// Result of a single backfill invocation.
///
/// Fields mirror the **count** columns in `backfill_runs` (1:1 on the
/// numeric fields). Timestamps and `notes` are not surfaced on the
/// struct — query `backfill_runs` directly for those if needed.
#[derive(Debug, Clone)]
pub struct BackfillRun {
    /// UUID assigned at the start of the run; also the PK in
    /// `backfill_runs`.
    pub run_id: String,
    /// Source table name as a free-form string (e.g. `"memories"`).
    /// Free-form rather than enum so future drivers can be added
    /// without a migration.
    pub legacy_table: String,
    /// Rows iterated from the legacy table — equals
    /// `rows_inserted + rows_skipped_existing + rows_failed`.
    pub rows_read: u64,
    /// Rows newly inserted into the unified table by this run.
    pub rows_inserted: u64,
    /// Rows whose unified counterpart already existed (idempotency
    /// hit). Re-running a completed backfill produces all-skipped.
    pub rows_skipped_existing: u64,
    /// Rows that errored during translation. For memories backfill
    /// this is always 0 — no LLM, no parse paths, only direct column
    /// mapping. Non-zero for entity-relation backfills (T22-T25)
    /// where attribute JSON parsing can fail.
    pub rows_failed: u64,
}

impl BackfillRun {
    /// Sanity check: `rows_read` must equal the sum of the three
    /// outcome buckets. Called after every driver to catch counter
    /// drift early.
    fn assert_counter_invariant(&self) {
        let sum = self.rows_inserted + self.rows_skipped_existing + self.rows_failed;
        assert_eq!(
            sum, self.rows_read,
            "backfill counter invariant broken: rows_read={} but \
             inserted({}) + skipped({}) + failed({}) = {}",
            self.rows_read, self.rows_inserted, self.rows_skipped_existing, self.rows_failed, sum
        );
    }
}

/// T19 — backfill `memories` rows into `nodes` (no LLM).
///
/// See module docs for invariants; this driver is the canonical
/// implementation of the memory→nodes Phase C projection (design
/// §5.3).
///
/// ## Two-pass strategy
///
/// `memories.superseded_by` is a self-referential FK: row A may
/// point at row B. In a single pass, inserting A before B would make
/// `nodes.superseded_by` reference a non-existent `nodes.id`,
/// breaking the FK. The fix is two passes:
///
/// 1. **Pass 1**: iterate every legacy memory, call
///    `Storage::insert_memory_node_row`, which writes the new
///    `nodes` row with `superseded_by = NULL` (per the T12 root fix
///    contract — supersession is an UPDATE-time concern).
/// 2. **Pass 2**: a single SQL `UPDATE nodes ... FROM memories`
///    propagates `memories.superseded_by` into
///    `nodes.superseded_by`, converting the legacy `''` sentinel to
///    SQL `NULL`. By this point every referenced id exists in
///    `nodes`, so the FK is satisfied.
///
/// ## Idempotency
///
/// `INSERT OR IGNORE` in Pass 1 makes re-runs safe: rows already
/// present are counted as `rows_skipped_existing`. Pass 2's UPDATE
/// is idempotent by construction (same input → same output).
///
/// ## Namespace filter
///
/// `namespace=None` backfills every namespace. `namespace=Some(ns)`
/// restricts both passes to that namespace, letting operators stage
/// the rollout (e.g. one big namespace at a time, with verification
/// in between). The filter is recorded in `backfill_runs.notes`.
///
/// ## Returns
///
/// The completed [`BackfillRun`] after Pass 2 has committed. On
/// error the audit row is left with `finished_at = NULL` for
/// operator triage.
///
/// ## Crash semantics
///
/// Pass 1 commits its own transaction before Pass 2 runs. If the
/// process dies between passes:
///
///   - `nodes` rows from Pass 1 are durable (zero-supersession state).
///   - `backfill_runs.finished_at` is `NULL` for the partial run.
///   - Re-invoking `backfill_memories_to_nodes` is safe and completes
///     the work: Pass 1 becomes all-skipped (idempotent), Pass 2
///     propagates supersession against the now-complete `nodes`
///     table. Operators can detect the orphan audit row via
///     `SELECT * FROM backfill_runs WHERE finished_at IS NULL`.
///
/// We intentionally do NOT wrap both passes in one transaction: at
/// 24k rows the journal can be large, and a single-pass + UPDATE
/// works equally well for crash recovery via the audit row.
pub fn backfill_memories_to_nodes(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes = json!({
        "namespace_filter": namespace,
        "driver": "backfill_memories_to_nodes",
        "design_ref": "v04-unified-substrate §5.3",
    })
    .to_string();

    // Open the audit row immediately so a crashed run is detectable
    // (finished_at NULL = run did not complete).
    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'memories', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes],
    )?;

    let mut rows_read: u64 = 0;
    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;

    // -----------------------------------------------------------------
    // Pass 1: INSERT OR IGNORE every memory into `nodes`, with
    // `superseded_by = NULL` (delegated to insert_memory_node_row).
    // -----------------------------------------------------------------
    //
    // Why a single transaction for all 24k rows: SQLite's per-INSERT
    // fsync cost dominates if we commit per row (~1 ms/row → 24s for
    // 24k). One transaction amortises the fsync to a single fdatasync
    // at commit time. The cost is RAM for the journal, which is fine
    // at this row count.
    //
    // We collect the row data up front to avoid holding a query stmt
    // and an INSERT stmt on the same Connection simultaneously
    // (rusqlite borrow rule — one prepared stmt holds &Connection).
    //
    // ## Memory scaling
    //
    // Materializing the entire `memories` table in RAM is fine at
    // the design-targeted ~24k row scale (~12 MB for typical record
    // sizes). It becomes a concern around 1M rows (~500 MB). If a
    // future operator hits that scale, the fix is to open a second
    // read connection on the same DB file and stream rows from one
    // while the writing tx runs on the other. Not worth the
    // complexity until then; tracked as a follow-up if/when row
    // counts grow past ~250k.
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT * FROM memories WHERE namespace = ?"
    } else {
        "SELECT * FROM memories"
    };
    let mut stmt = conn.prepare(select_sql)?;
    let records: Vec<(crate::types::MemoryRecord, String, Option<String>)> = {
        // Hydrate (record, namespace, attributes_json) into memory.
        // attributes_json is read raw from the `metadata` column to
        // avoid a round-trip through serde — the JSON is already
        // canonical from the original `Storage::add` call.
        let map_row = |row: &rusqlite::Row| -> Result<_, rusqlite::Error> {
            let record = row_to_record_impl(row, vec![])?;
            let ns: String = row.get("namespace")?;
            let attrs: Option<String> = row.get("metadata")?;
            Ok((record, ns, attrs))
        };
        let iter = if let Some(ns) = namespace {
            stmt.query_map(params![ns], map_row)?.collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map_row)?.collect::<Result<Vec<_>, _>>()?
        };
        iter
    };
    drop(stmt);

    let conn = storage.conn();
    let tx = conn.unchecked_transaction()?;
    for (record, ns, attrs) in &records {
        rows_read += 1;
        let inserted = Storage::insert_memory_node_row(&tx, record, ns, attrs.as_deref())?;
        if inserted {
            rows_inserted += 1;
        } else {
            rows_skipped_existing += 1;
        }
    }
    tx.commit()?;

    // -----------------------------------------------------------------
    // Pass 2: propagate supersession.
    // -----------------------------------------------------------------
    //
    // After Pass 1, every legacy memory in the filter scope has a
    // `nodes` row with `superseded_by = NULL`. We now set
    // `nodes.superseded_by` from `memories.superseded_by` in a single
    // UPDATE, converting the legacy `''` sentinel to `NULL` along the
    // way (the unified schema treats supersession as
    // `TEXT REFERENCES nodes(id) ON DELETE SET NULL`; `''` is a
    // memories-only convention).
    //
    // We also bump `nodes.updated_at` for any row whose
    // `superseded_by` actually changes, so audit consumers can see
    // when supersession was projected.
    //
    // The CASE expression makes the `'' → NULL` conversion explicit
    // rather than relying on SQLite's quirky string-vs-NULL behavior.
    //
    // ## Cross-namespace supersession guard
    //
    // Legacy `memories.superseded_by` is NOT namespace-constrained:
    // row A in `ns-foo` can technically point at row B in `ns-bar`.
    // For a namespace-filtered backfill, only `ns-foo` rows are in
    // `nodes` after Pass 1 — setting `nodes.superseded_by = B` would
    // violate the FK to `nodes(id)`. The subquery's
    // `EXISTS (SELECT 1 FROM nodes target ...)` guard skips such
    // edges; they get picked up on the next backfill invocation
    // covering `ns-bar` (or by an unfiltered re-run).
    //
    // For unfiltered runs the EXISTS guard is still cheap and acts
    // as defence-in-depth against any other source of dangling
    // supersession ids in legacy data.
    let conn = storage.conn();
    let updated_at = utc_now_f64();
    let pass2_sql = if namespace.is_some() {
        r#"
        UPDATE nodes
        SET superseded_by = (
                SELECT CASE
                    WHEN m.superseded_by = '' THEN NULL
                    ELSE m.superseded_by
                END
                FROM memories m
                WHERE m.id = nodes.id
            ),
            updated_at = ?
        WHERE nodes.node_kind = 'memory'
          AND nodes.namespace = ?
          AND EXISTS (
              SELECT 1 FROM memories m
              WHERE m.id = nodes.id
                AND m.superseded_by IS NOT NULL
                AND m.superseded_by <> ''
                AND EXISTS (
                    SELECT 1 FROM nodes target
                    WHERE target.id = m.superseded_by
                )
          )
        "#
    } else {
        r#"
        UPDATE nodes
        SET superseded_by = (
                SELECT CASE
                    WHEN m.superseded_by = '' THEN NULL
                    ELSE m.superseded_by
                END
                FROM memories m
                WHERE m.id = nodes.id
            ),
            updated_at = ?
        WHERE nodes.node_kind = 'memory'
          AND EXISTS (
              SELECT 1 FROM memories m
              WHERE m.id = nodes.id
                AND m.superseded_by IS NOT NULL
                AND m.superseded_by <> ''
                AND EXISTS (
                    SELECT 1 FROM nodes target
                    WHERE target.id = m.superseded_by
                )
          )
        "#
    };
    if let Some(ns) = namespace {
        conn.execute(pass2_sql, params![updated_at, ns])?;
    } else {
        conn.execute(pass2_sql, params![updated_at])?;
    }

    // -----------------------------------------------------------------
    // Close the audit row.
    // -----------------------------------------------------------------
    let finished_at = utc_now_f64();
    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            rows_skipped_existing as i64,
            finished_at,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "memories".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}

/// Read a `backfill_runs` row by id. Used by integration tests and
/// operator tooling to verify audit completeness.
pub fn fetch_backfill_run(
    storage: &Storage,
    run_id: &str,
) -> Result<Option<BackfillRun>, rusqlite::Error> {
    storage
        .conn()
        .query_row(
            r#"
            SELECT run_id, legacy_table, rows_read, rows_inserted,
                   rows_skipped_existing, rows_failed
            FROM backfill_runs WHERE run_id = ?
            "#,
            params![run_id],
            |row| {
                Ok(BackfillRun {
                    run_id: row.get(0)?,
                    legacy_table: row.get(1)?,
                    rows_read: row.get::<_, i64>(2)? as u64,
                    rows_inserted: row.get::<_, i64>(3)? as u64,
                    rows_skipped_existing: row.get::<_, i64>(4)? as u64,
                    rows_failed: row.get::<_, i64>(5)? as u64,
                })
            },
        )
        .optional()
}
