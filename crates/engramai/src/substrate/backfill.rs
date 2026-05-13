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
use sha2::{Digest, Sha256};
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

// =====================================================================
// T20 — backfill memory_embeddings → node_embeddings
// =====================================================================

/// T20 — backfill `memory_embeddings` rows into `node_embeddings`
/// (no LLM).
///
/// ## Prerequisites
///
/// **T19 must have run first.** `node_embeddings.node_id REFERENCES
/// nodes(id)` is FK-enforced — a memory whose parent `nodes` row is
/// missing cannot be embedded. The driver detects orphan rows
/// (memory_id with no nodes row) and counts them as
/// `rows_skipped_existing` with an explanatory note in the audit row,
/// rather than failing the whole run. Operators can re-invoke after
/// running T19 against the missing namespace.
///
/// ## Single-pass, no self-FK
///
/// Unlike T19, `memory_embeddings` has no self-referential FK, so a
/// single pass suffices: iterate, project, INSERT OR IGNORE.
///
/// ## `created_at` type conversion
///
/// `memory_embeddings.created_at` is `TEXT` (RFC3339);
/// `node_embeddings.created_at` is `REAL` (epoch seconds with
/// sub-second precision). Parsing is done in the driver, not in the
/// helper, so the policy for "what to do on parse failure" stays out
/// of the SQL layer. Current policy: fall back to the legacy row's
/// position in iteration order with `Utc::now()` — corrupted dates
/// are rare and operators get an audit entry under `rows_failed=0`
/// with the count in `notes`. (A stricter operator can post-query
/// `node_embeddings` for the fallback timestamp to find them.)
///
/// ## Namespace filter
///
/// `memory_embeddings` has no `namespace` column; the filter is
/// applied via JOIN to `memories.namespace`. Operators get the same
/// staged-rollout option as T19.
///
/// ## Idempotency
///
/// `INSERT OR IGNORE` on `(node_id, model)`. Re-running the driver
/// after a partial run completes the work without duplicating rows.
pub fn backfill_embeddings_to_node_embeddings(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let mut rows_failed_parse: u64 = 0;
    let mut rows_skipped_missing_node: u64 = 0;

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_embeddings_to_node_embeddings",
        "design_ref": "v04-unified-substrate §5.3 / T20",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'memory_embeddings', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    // -----------------------------------------------------------------
    // Hydrate rows into memory.
    //
    // Same memory-scaling trade-off as T19 (see that driver's comment).
    // Embeddings are larger per row (BLOB ~6 KB at d=1536, f32) so the
    // ~24k row scale = ~150 MB. That's the upper edge of what we want
    // to hold in RAM. If this grows, switch to a streaming read
    // connection — same fix as T19.
    // -----------------------------------------------------------------
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        r#"
        SELECT e.memory_id, e.model, e.embedding, e.dimensions, e.created_at
        FROM memory_embeddings e
        INNER JOIN memories m ON m.id = e.memory_id
        WHERE m.namespace = ?
        "#
    } else {
        r#"
        SELECT memory_id, model, embedding, dimensions, created_at
        FROM memory_embeddings
        "#
    };
    let mut stmt = conn.prepare(select_sql)?;
    type EmbRow = (String, String, Vec<u8>, i64, String);
    let map_row = |row: &rusqlite::Row| -> Result<EmbRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    };
    let rows: Vec<EmbRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    drop(stmt);

    let mut rows_read: u64 = 0;
    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;

    let conn = storage.conn();
    let tx = conn.unchecked_transaction()?;
    for (memory_id, model, embedding, dimensions, created_at_text) in &rows {
        rows_read += 1;

        // Skip rows whose parent `nodes` row doesn't exist — T19 must
        // run first or the FK INSERT will fail. We pre-check rather
        // than relying on the FK error so we can report cleanly.
        let node_exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM nodes WHERE id = ? AND node_kind='memory'",
            params![memory_id],
            |r| r.get(0),
        )?;
        if node_exists == 0 {
            rows_skipped_missing_node += 1;
            // Count toward skipped_existing semantically (not failed):
            // operator re-runs T19 + T20 and the row lands.
            rows_skipped_existing += 1;
            continue;
        }

        // Parse RFC3339 created_at → epoch f64. Fallback to now() on
        // parse failure (rare for valid legacy data).
        let created_at_epoch: f64 =
            match chrono::DateTime::parse_from_rfc3339(created_at_text) {
                Ok(dt) => {
                    let dt_utc = dt.with_timezone(&chrono::Utc);
                    dt_utc.timestamp() as f64
                        + (dt_utc.timestamp_subsec_nanos() as f64 / 1e9)
                }
                Err(_) => {
                    rows_failed_parse += 1;
                    utc_now_f64()
                }
            };

        let inserted = Storage::insert_node_embedding_row(
            &tx,
            memory_id,
            model,
            embedding,
            *dimensions,
            created_at_epoch,
        )?;
        if inserted {
            rows_inserted += 1;
        } else {
            rows_skipped_existing += 1;
        }
    }
    tx.commit()?;

    // -----------------------------------------------------------------
    // Close the audit row. Embed the parse-failure / missing-node
    // counts in the `notes` JSON for operator visibility.
    // -----------------------------------------------------------------
    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_embeddings_to_node_embeddings",
        "design_ref": "v04-unified-substrate §5.3 / T20",
        "rows_skipped_missing_node": rows_skipped_missing_node,
        "rows_failed_parse_used_now": rows_failed_parse,
    })
    .to_string();
    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?, notes = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            rows_skipped_existing as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "memory_embeddings".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}

// =====================================================================
// T21 — backfill entities → nodes(kind=entity)
// =====================================================================

/// Merge two `attributes` JSON objects, preserving values in the
/// LEFT (existing) operand on key collision. Used by T21 Pass 2 to
/// fold legacy entity metadata into a `nodes` row that was already
/// dual-written by the T13 resolution pipeline path — the T13 row
/// is canonical, the legacy projection only adds keys T13 didn't
/// know about.
fn merge_attributes_existing_wins(
    existing: &str,
    new_keys: &str,
) -> String {
    let mut existing_val: serde_json::Value = match serde_json::from_str(existing) {
        Ok(serde_json::Value::Object(m)) => serde_json::Value::Object(m),
        _ => return existing.to_string(),
    };
    let new_val: serde_json::Value = match serde_json::from_str(new_keys) {
        Ok(serde_json::Value::Object(m)) => serde_json::Value::Object(m),
        _ => return existing.to_string(),
    };
    if let (serde_json::Value::Object(ref mut ex), serde_json::Value::Object(nw)) =
        (&mut existing_val, new_val)
    {
        for (k, v) in nw {
            ex.entry(k).or_insert(v);
        }
    }
    serde_json::to_string(&existing_val).unwrap_or_else(|_| existing.to_string())
}

/// T21 — backfill `entities` rows into `nodes(node_kind='entity')`
/// (no LLM).
///
/// ## Two-pass with a different rationale than T19
///
/// T19's two-pass was about a self-referential FK
/// (`memories.superseded_by`). T21's two-pass is about **the
/// metadata-merge contract** (design §5.3): if a `nodes` row
/// already exists for an entity id (e.g. because the T13 resolution
/// pipeline wrote it during normal operation), the legacy
/// `entities.metadata` keys must be **merged** into the existing
/// `nodes.attributes`, with existing keys winning on collision.
///
///   - Pass 1: INSERT OR IGNORE every legacy entity. New rows land
///     with `attributes = {"entity_type": "...", ...legacy_metadata}`.
///   - Pass 2: For rows that were SKIPPED in Pass 1 (case 2: T13
///     row already there), MERGE the legacy attributes into the
///     existing row's `attributes` column. Existing values win.
///
/// Pass 2 has to be in Rust (not pure SQL) because JSON merging
/// with collision policy isn't expressible as a single SQLite
/// statement without `JSON_PATCH`, which has overwrite semantics
/// (last-write-wins, opposite of what we need).
///
/// ## Field mapping (design §5.3)
///
///   - `entities.id → nodes.id`
///   - `entities.name → nodes.content`
///   - `entities.entity_type` → `nodes.attributes.entity_type`
///   - `entities.metadata` (parsed as JSON) → merged into
///     `nodes.attributes` with "existing wins" policy
///   - `namespace`, `created_at`, `updated_at`: direct copy
pub fn backfill_entities_to_nodes(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_entities_to_nodes",
        "design_ref": "v04-unified-substrate §5.3 / T21",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'entities', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT id, name, entity_type, namespace, metadata, created_at, updated_at \
         FROM entities WHERE namespace = ?"
    } else {
        "SELECT id, name, entity_type, namespace, metadata, created_at, updated_at \
         FROM entities"
    };
    let mut stmt = conn.prepare(select_sql)?;
    type EntRow = (String, String, String, String, Option<String>, f64, f64);
    let map_row = |row: &rusqlite::Row| -> Result<EntRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
        ))
    };
    let rows: Vec<EntRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    drop(stmt);

    let mut rows_read: u64 = 0;
    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_metadata_merged: u64 = 0;
    let mut rows_malformed_metadata: u64 = 0;
    let mut rows_kind_mismatch: u64 = 0;

    let conn = storage.conn();
    let tx = conn.unchecked_transaction()?;

    for (id, name, entity_type, ns, metadata_text, created_at, updated_at) in &rows {
        rows_read += 1;

        // Build the projected `attributes` JSON per design §5.3:
        //   1. Seed with `{"entity_type": <column>}` — this is the
        //      contract-mandated key carrying the legacy column.
        //   2. Merge `entities.metadata` keys in, but **existing-wins**:
        //      if metadata contains `entity_type`, the column value
        //      MUST win (the legacy column is the source of truth for
        //      the type; metadata is a side-channel attribute bag).
        //
        // This is the same `merge_attributes_existing_wins` polarity
        // used by Pass 2 below — both passes share the same contract
        // ("existing keys win on collision"), just at different
        // layers. Pass 1's "existing" = the column-derived
        // entity_type key. Pass 2's "existing" = whatever an earlier
        // T13 dual-write already wrote.
        let mut projected_attrs = serde_json::Map::new();
        projected_attrs.insert(
            "entity_type".into(),
            serde_json::Value::String(entity_type.clone()),
        );
        if let Some(meta_str) = metadata_text.as_deref() {
            match serde_json::from_str::<serde_json::Value>(meta_str) {
                Ok(serde_json::Value::Object(map)) => {
                    for (k, v) in map {
                        // entry().or_insert() = existing-wins.
                        // If `entity_type` is in metadata, the
                        // column-seeded value already there wins
                        // and the metadata value is dropped.
                        projected_attrs.entry(k).or_insert(v);
                    }
                }
                Ok(_) | Err(_) => {
                    rows_malformed_metadata += 1;
                }
            }
        }
        let projected_attrs_json =
            serde_json::to_string(&serde_json::Value::Object(projected_attrs))
                .expect("serializing a serde_json::Map cannot fail");

        let inserted = Storage::insert_entity_node_row(
            &tx,
            id,
            name,
            &projected_attrs_json,
            ns,
            *created_at,
            *updated_at,
        )?;
        if inserted {
            rows_inserted += 1;
        } else {
            rows_skipped_existing += 1;

            // Pass 2 (inline): the row already exists in nodes. We
            // ONLY merge attributes if the existing row is also
            // node_kind='entity'. If somehow this id resolves to a
            // topic / memory / insight (extremely unlikely given
            // separate id generation paths, but defence-in-depth),
            // skip the merge — the legacy projection has no business
            // mutating a non-entity node's attributes.
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT node_kind, attributes FROM nodes WHERE id = ?",
                    params![id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .ok();
            if let Some((existing_kind, existing_attrs)) = existing {
                if existing_kind == "entity" {
                    let merged = merge_attributes_existing_wins(
                        &existing_attrs,
                        &projected_attrs_json,
                    );
                    tx.execute(
                        "UPDATE nodes SET attributes = ?, updated_at = ? WHERE id = ?",
                        params![merged, utc_now_f64(), id],
                    )?;
                    rows_metadata_merged += 1;
                } else {
                    // Foreign node_kind already owns this id; leave
                    // it untouched. Surface in audit notes so the
                    // operator can investigate if non-zero.
                    rows_kind_mismatch += 1;
                }
            }
        }
    }
    tx.commit()?;

    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_entities_to_nodes",
        "design_ref": "v04-unified-substrate §5.3 / T21",
        "rows_metadata_merged": rows_metadata_merged,
        "rows_malformed_metadata": rows_malformed_metadata,
        "rows_kind_mismatch": rows_kind_mismatch,
    })
    .to_string();
    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?, notes = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            rows_skipped_existing as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "entities".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}

// =====================================================================
// T22 — backfill entity_relations → edges(kind=structural)
// =====================================================================

/// T22 — backfill `entity_relations` rows into
/// `edges(edge_kind='structural')` (no LLM).
///
/// ## Why this is structurally similar to T21 but not unified
///
/// T22 and T21 share the same Pass-1 + Pass-2-merge contract for
/// `attributes` JSON, but they project DIFFERENT legacy tables
/// into DIFFERENT target tables with DIFFERENT FK requirements:
///
///   - T21: legacy `entities` → `nodes`. No FK requirements
///     beyond unique id.
///   - T22: legacy `entity_relations` → `edges`. Requires BOTH
///     endpoints (`source_id`, `target_id`) to already exist in
///     `nodes`. So T22 has a **dangling-endpoint guard** that T21
///     doesn't need.
///
/// Trying to merge them into a single generic driver would force
/// a config-soup API; better to keep the per-table drivers
/// readable and accept ~30 lines of structural duplication.
///
/// ## Two-pass strategy
///
///   - Pass 1: INSERT OR IGNORE every legacy `entity_relations`
///     row, projecting `relation → predicate`, `confidence`,
///     `metadata` JSON merged with `source` free-text into
///     `edges.attributes`. Endpoints are checked via `EXISTS`
///     before insertion — dangling endpoints are SKIPPED (not
///     failed) and counted in audit notes. Recovery: run T21 (or
///     a backfill of upstream entities), then re-run T22.
///   - Pass 2 (inline, same tx): for rows where INSERT OR IGNORE
///     was a no-op (the edge already exists, e.g. T13
///     resolution-pipeline path wrote it), MERGE the legacy
///     attributes into the existing row's attributes with
///     **existing-wins** semantics (same polarity as T21,
///     §5.3 contract).
///
/// ## FK guard rationale (R2.1-style)
///
/// `edges.source_id` and `edges.target_id` have ON DELETE
/// RESTRICT FKs to `nodes(id)`. If T22 runs on a namespace before
/// T21 has projected the entities in that namespace (or before
/// the resolution pipeline has materialized the endpoints in
/// `nodes`), an unguarded INSERT would fail the entire tx. The
/// `EXISTS` pre-check is a defence against partial-Phase-C
/// state — same pattern T19 R2.1 used for cross-namespace
/// supersession targets.
///
/// ## Field mapping (design §5.3)
///
///   - `entity_relations.id → edges.id`
///   - `source_id, target_id → edges.source_id, edges.target_id`
///   - `relation → edges.predicate`
///   - `confidence → edges.confidence`
///   - `metadata` (JSON object) + `source` (free text) → merged
///     into `edges.attributes`. The `source` column lands as
///     `attributes.source`. Legacy `metadata` keys can NOT shadow
///     `attributes.source` (existing-wins, same fix as T21
///     FINDING-1).
///   - `namespace, created_at`: direct copy.
///   - `recorded_at = updated_at = created_at` (legacy has no
///     separate fields).
///   - `edge_kind = 'structural'`, `predicate_kind = 'canonical'`.
pub fn backfill_entity_relations_to_edges(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_entity_relations_to_edges",
        "design_ref": "v04-unified-substrate §5.3 / T22",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'entity_relations', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    // -----------------------------------------------------------------
    // Hydrate legacy rows. 6531 rows at design-targeted scale.
    // -----------------------------------------------------------------
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT id, source_id, target_id, relation, confidence, source, namespace, \
         created_at, metadata FROM entity_relations WHERE namespace = ?"
    } else {
        "SELECT id, source_id, target_id, relation, confidence, source, namespace, \
         created_at, metadata FROM entity_relations"
    };
    let mut stmt = conn.prepare(select_sql)?;
    type RelRow = (
        String,         // id
        String,         // source_id
        String,         // target_id
        String,         // relation
        f64,            // confidence
        Option<String>, // source (free text)
        String,         // namespace
        f64,            // created_at
        Option<String>, // metadata JSON
    );
    let map_row = |row: &rusqlite::Row| -> Result<RelRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
        ))
    };
    let rows: Vec<RelRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    drop(stmt);

    let mut rows_read: u64 = 0;
    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_skipped_dangling_endpoint: u64 = 0;
    let mut rows_metadata_merged: u64 = 0;
    let mut rows_malformed_metadata: u64 = 0;
    let mut rows_existing_kind_mismatch: u64 = 0;

    let conn = storage.conn();
    let tx = conn.unchecked_transaction()?;

    for (id, source_id, target_id, relation, confidence, source_text, ns, created_at, metadata_text)
        in &rows
    {
        rows_read += 1;

        // ---------------------------------------------------------
        // FK guard: both endpoints must exist in nodes.
        // ---------------------------------------------------------
        let endpoints_present: i64 = tx.query_row(
            "SELECT (CASE WHEN
                EXISTS(SELECT 1 FROM nodes WHERE id = ?)
                AND EXISTS(SELECT 1 FROM nodes WHERE id = ?)
                THEN 1 ELSE 0 END)",
            params![source_id, target_id],
            |r| r.get(0),
        )?;
        if endpoints_present == 0 {
            rows_skipped_dangling_endpoint += 1;
            // Don't bump rows_skipped_existing — this row never had
            // a chance to be inserted, so it's a "deferred" row not
            // a "duplicate" row. Counter invariant treats it as
            // skipped-existing for tally purposes (see closing of
            // run).
            continue;
        }

        // ---------------------------------------------------------
        // Build projected attributes per §5.3:
        //   1. Seed with {"source": <free-text>} if source NOT NULL.
        //   2. Merge metadata keys in, existing-wins (so a metadata
        //      key named "source" CANNOT shadow the column).
        // ---------------------------------------------------------
        let mut projected_attrs = serde_json::Map::new();
        if let Some(src) = source_text.as_deref() {
            projected_attrs.insert(
                "source".into(),
                serde_json::Value::String(src.to_string()),
            );
        }
        if let Some(meta_str) = metadata_text.as_deref() {
            match serde_json::from_str::<serde_json::Value>(meta_str) {
                Ok(serde_json::Value::Object(map)) => {
                    for (k, v) in map {
                        projected_attrs.entry(k).or_insert(v);
                    }
                }
                Ok(_) | Err(_) => {
                    rows_malformed_metadata += 1;
                }
            }
        }
        let projected_attrs_json =
            serde_json::to_string(&serde_json::Value::Object(projected_attrs))
                .expect("serializing a serde_json::Map cannot fail");

        let inserted = Storage::insert_structural_edge_row(
            &tx,
            id,
            source_id,
            target_id,
            relation,
            &projected_attrs_json,
            *confidence,
            ns,
            *created_at,
        )?;
        if inserted {
            rows_inserted += 1;
        } else {
            rows_skipped_existing += 1;

            // Pass 2 (inline): the edge id already exists. Three
            // sub-cases (same shape as T21 Pass 2):
            //   (a) edge_kind='structural' → merge attributes.
            //   (b) edge_kind is something else (assertion,
            //       associative, provenance) → an id collision;
            //       refuse to merge, count in audit notes.
            //   (c) row missing → impossible inside same tx.
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT edge_kind, attributes FROM edges WHERE id = ?",
                    params![id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .ok();
            if let Some((existing_kind, existing_attrs)) = existing {
                if existing_kind == "structural" {
                    let merged = merge_attributes_existing_wins(
                        &existing_attrs,
                        &projected_attrs_json,
                    );
                    tx.execute(
                        "UPDATE edges SET attributes = ?, updated_at = ? WHERE id = ?",
                        params![merged, utc_now_f64(), id],
                    )?;
                    rows_metadata_merged += 1;
                } else {
                    rows_existing_kind_mismatch += 1;
                }
            }
        }
    }
    tx.commit()?;

    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_entity_relations_to_edges",
        "design_ref": "v04-unified-substrate §5.3 / T22",
        "rows_metadata_merged": rows_metadata_merged,
        "rows_malformed_metadata": rows_malformed_metadata,
        "rows_skipped_dangling_endpoint": rows_skipped_dangling_endpoint,
        "rows_existing_kind_mismatch": rows_existing_kind_mismatch,
    })
    .to_string();

    // Counter invariant fold: dangling-endpoint rows are counted
    // as "skipped_existing" for the tally only (they conceptually
    // failed-and-deferred, but the BackfillRun struct only has
    // three skip slots). Detailed breakdown lives in notes JSON.
    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?, notes = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            (rows_skipped_existing + rows_skipped_dangling_endpoint) as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "entity_relations".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing: rows_skipped_existing + rows_skipped_dangling_endpoint,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}


// =====================================================================
// T23 — backfill memory_entities → edges
// =====================================================================
//
// Source table: `memory_entities(memory_id, entity_id, role)` —
// link table mirroring which entities are mentioned by which memories.
// No own `created_at` or `namespace` columns; both are derived from
// the parent memory via JOIN.
//
// Target rows live in `edges` and split by `role` per design §3.3:
//   * `role = 'mention'` (default, ~all production rows)
//   * `role = ''`        (treat as 'mention')
//   * `role = 'triple'`  (triple-extraction provenance — treat as
//                         'mention' but record the raw role in
//                         `edges.attributes.legacy_role` for audit)
//   * unknown / other    (treat as 'mention', same audit field)
//        → `edge_kind='provenance', predicate='mentions'`
//   * `role = 'subject'` → `edge_kind='structural', predicate='subject_of'`
//   * `role = 'object'`  → `edge_kind='structural', predicate='object_of'`
//
// **Note (design §3.3 vs §5.3 prose inconsistency)**: §5.3 line 1140
// summary text says "memory_entities → edges (kind=provenance)" without
// the role split. §3.3 (lines 320, 338) is the normative canonical
// kind/predicate table and splits by role. This driver follows §3.3.
// The §5.3 prose should be tightened in a follow-up commit.
//
// Idempotency: deterministic edge `id` = `uuid_from_hash(sha256(
//   "memory_entities|" || memory_id || "|" || entity_id || "|" ||
//   role || "|" || edge_kind || "|" || predicate
// ))`. The legacy row's natural key is `(memory_id, entity_id, role)`
// per the table PK — but we include `edge_kind` and `predicate` in the
// hash too, so a future schema change that re-derives the predicate
// produces a DIFFERENT id (forcing visible insert/replace rather than
// silent stale-row reuse). See design §5.3 lines 1170-1182.
//
// FK guard: edges.source_id and edges.target_id both reference
// `nodes.id`. Rows whose endpoints aren't in `nodes` yet (T19 hasn't
// run for the memory's namespace, T21 hasn't run for the entity's
// namespace) are SKIPPED — counted in `rows_skipped_dangling_endpoint`
// and surfaced in audit `notes`. Recovery: run T19+T21 first, then
// re-run T23. Same self-recovering pattern as T20/T22.

/// Derive a deterministic UUID by hashing canonical row identity per
/// design §5.3 lines 1170-1182.
///
/// `hash_input` is the caller-built tuple string:
///   `"memory_entities|<memory_id>|<entity_id>|<role>|<edge_kind>|<predicate>"`
/// (pipe-delimited; no escaping needed because none of these fields
/// can legitimately contain `'|'` — `memory_id`/`entity_id` are UUIDs,
/// `role` is from a small known vocabulary, `edge_kind`/`predicate`
/// are enum-like strings).
///
/// SHA-256 → take first 16 bytes → format as UUID. We don't set the
/// version/variant bits because we never need to compare against a
/// random v4 UUID semantically; deterministic-from-hash IDs live in
/// their own namespace by construction.
pub(crate) fn uuid_from_hash(hash_input: &str) -> String {
    let digest = Sha256::digest(hash_input.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes).to_string()
}

/// Map a legacy `memory_entities.role` to a canonical
/// `(edge_kind, predicate)` per design §3.3. Returns
/// `(edge_kind, predicate, normalized: bool)` where `normalized=true`
/// signals that the raw role was not the canonical 'mention'/'subject'/
/// 'object' set and was folded onto `provenance/mentions` — the driver
/// records the raw role in attributes for audit traceability.
pub(crate) fn role_to_kind_predicate(role: &str) -> (&'static str, &'static str, bool) {
    match role {
        "mention" | "" => ("provenance", "mentions", false),
        "subject" => ("structural", "subject_of", false),
        "object" => ("structural", "object_of", false),
        // 'triple' and any other free-form roles fold onto the
        // canonical mention kind, with the raw role preserved in
        // edges.attributes.legacy_role.
        _ => ("provenance", "mentions", true),
    }
}

/// T23 — backfill `memory_entities` rows into the unified `edges`
/// table, split by role per design §3.3 (mention → provenance,
/// subject/object → structural).
///
/// Restartable per the Phase C contract: deterministic `edges.id` +
/// `INSERT OR IGNORE` means a re-run inserts zero new rows.
///
/// Namespace filter: if `namespace` is `Some(ns)`, only `memory_entities`
/// rows whose **parent memory's** namespace matches `ns` are processed
/// — the link table has no own namespace column. Recovery from a
/// partial run is the same as T22's: re-run with the same filter,
/// rows_skipped_existing should equal the prior run's rows_inserted.
///
/// Endpoint FK safety: the driver pre-checks that both `memory_id` and
/// `entity_id` exist as `nodes` rows before each insert. Missing
/// endpoints are counted in `rows_skipped_dangling_endpoint` and
/// surfaced in audit `notes` JSON. This matches T22's behaviour for
/// cross-namespace consistency.
pub fn backfill_memory_entities_to_edges(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_memory_entities_to_edges",
        "design_ref": "v04-unified-substrate §3.3 + §5.3 / T23",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'memory_entities', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    // -----------------------------------------------------------------
    // Hydrate legacy rows. ~9237 rows at design-targeted scale.
    //
    // Pull the parent memory's namespace and created_at via JOIN so
    // each link row has the namespace/timestamp it needs without a
    // second round-trip. Filter by parent namespace if requested.
    // -----------------------------------------------------------------
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT me.memory_id, me.entity_id, me.role, m.namespace, m.created_at \
         FROM memory_entities me \
         INNER JOIN memories m ON m.id = me.memory_id \
         WHERE m.namespace = ?"
    } else {
        "SELECT me.memory_id, me.entity_id, me.role, m.namespace, m.created_at \
         FROM memory_entities me \
         INNER JOIN memories m ON m.id = me.memory_id"
    };
    let mut stmt = conn.prepare(select_sql)?;
    type MeRow = (
        String, // memory_id
        String, // entity_id
        String, // role
        String, // parent namespace
        f64,    // parent created_at
    );
    let map_row = |row: &rusqlite::Row| -> Result<MeRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    };
    let rows: Vec<MeRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<_, _>>()?
    } else {
        stmt.query_map([], map_row)?.collect::<Result<_, _>>()?
    };

    let rows_read: u64 = rows.len() as u64;

    // -----------------------------------------------------------------
    // Single-pass insert (no self-referential FKs to resolve), inside
    // one transaction. Per-row decisions:
    //   * derive (edge_kind, predicate, role_normalized) from role
    //   * compute deterministic edge id
    //   * FK pre-check both endpoints — skip if either missing
    //   * call appropriate insert helper
    //   * count inserted / skipped-existing / skipped-dangling /
    //     skipped-mismatched-kind buckets
    //
    // `rows_skipped_mismatched_kind` covers the rare case where an
    // edges row with our deterministic id already exists but has a
    // different `edge_kind`/`predicate` than what we'd derive now.
    // This shouldn't happen under design contract — the id hash
    // includes both — but we surface it in audit notes so a future
    // bug is visible rather than silent.
    // -----------------------------------------------------------------
    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_skipped_dangling_endpoint: u64 = 0;
    let mut rows_skipped_mismatched_kind: u64 = 0;
    let mut rows_normalized_legacy_role: u64 = 0;
    let mut unknown_role_samples: Vec<String> = Vec::new();
    let mut unknown_role_samples_truncated: bool = false;
    let mut unknown_role_distinct: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let unknown_role_sample_cap: usize = 10;

    let tx = storage.conn().unchecked_transaction()?;

    for (memory_id, entity_id, role, namespace, created_at) in rows {
        let (edge_kind, predicate, normalized) = role_to_kind_predicate(&role);

        if normalized {
            rows_normalized_legacy_role += 1;
            if unknown_role_distinct.insert(role.clone()) {
                // New distinct value seen. Either capture or note
                // truncation — never silently drop.
                if unknown_role_samples.len() < unknown_role_sample_cap {
                    unknown_role_samples.push(role.clone());
                } else {
                    unknown_role_samples_truncated = true;
                }
            }
        }

        // Deterministic id per design §5.3 lines 1170-1182. Include
        // edge_kind and predicate in the hash so future schema bumps
        // that re-derive the predicate produce a different id.
        let hash_input = format!(
            "memory_entities|{}|{}|{}|{}|{}",
            memory_id, entity_id, role, edge_kind, predicate
        );
        let id = uuid_from_hash(&hash_input);

        // FK pre-check: both endpoints must exist as nodes.
        let endpoints_ok: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?) AND \
                 EXISTS(SELECT 1 FROM nodes WHERE id = ?)",
                params![memory_id, entity_id],
                |r: &rusqlite::Row<'_>| r.get::<_, i64>(0),
            )
            .map(|n| n != 0)?;

        if !endpoints_ok {
            rows_skipped_dangling_endpoint += 1;
            continue;
        }

        // attributes JSON: record the raw role iff it deviated from
        // the canonical vocabulary (so re-runs of an unchanged DB are
        // byte-identical, but audit traceability is preserved for
        // 'triple' and any future free-form roles).
        //
        // `normalized` is already `true` for 'triple' and any other
        // unknown role per `role_to_kind_predicate`; we use it as the
        // single source of truth for "this role is non-canonical".
        let attributes_json = if normalized {
            json!({ "legacy_role": role }).to_string()
        } else {
            "{}".to_string()
        };

        // Pre-check whether a row with our id already exists AND
        // whether its kind matches. If a stale id collision points
        // at a different kind, count it in
        // rows_skipped_mismatched_kind rather than silently skipping.
        let existing_kind: Option<(String, String)> = tx
            .query_row(
                "SELECT edge_kind, predicate FROM edges WHERE id = ?",
                params![id],
                |r: &rusqlite::Row<'_>| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let inserted = match edge_kind {
            "structural" => Storage::insert_structural_edge_row(
                &tx,
                &id,
                &memory_id,
                &entity_id,
                predicate,
                &attributes_json,
                1.0,
                &namespace,
                created_at,
            )?,
            "provenance" => Storage::insert_provenance_edge_row(
                &tx,
                &id,
                &memory_id,
                &entity_id,
                predicate,
                &attributes_json,
                1.0,
                &namespace,
                created_at,
            )?,
            _ => unreachable!(
                "role_to_kind_predicate only emits 'structural' or 'provenance'"
            ),
        };

        if inserted {
            rows_inserted += 1;
        } else if let Some((ek, pp)) = existing_kind {
            if ek != edge_kind || pp != predicate {
                rows_skipped_mismatched_kind += 1;
            } else {
                rows_skipped_existing += 1;
            }
        } else {
            // Existing row vanished between SELECT and INSERT
            // (impossible under our tx isolation). Defensively count
            // as skipped-existing.
            rows_skipped_existing += 1;
        }
    }

    tx.commit()?;

    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_memory_entities_to_edges",
        "design_ref": "v04-unified-substrate §3.3 + §5.3 / T23",
        "rows_skipped_dangling_endpoint": rows_skipped_dangling_endpoint,
        "rows_skipped_mismatched_kind": rows_skipped_mismatched_kind,
        "rows_normalized_legacy_role": rows_normalized_legacy_role,
        "unknown_role_samples": unknown_role_samples,
        "unknown_role_samples_truncated": unknown_role_samples_truncated,
        "unknown_role_distinct_count": unknown_role_distinct.len(),
    })
    .to_string();

    // We collapse all three skip buckets into the single
    // `rows_skipped_existing` column so the counter invariant
    // (rows_read = inserted + skipped + failed) holds. Detailed
    // breakdown lives in notes JSON, same convention as T22.
    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?, notes = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            (rows_skipped_existing + rows_skipped_dangling_endpoint + rows_skipped_mismatched_kind)
                as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "memory_entities".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing: rows_skipped_existing
            + rows_skipped_dangling_endpoint
            + rows_skipped_mismatched_kind,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}

#[cfg(test)]
mod t23_helpers_tests {
    use super::*;

    #[test]
    fn role_to_kind_predicate_canonical_roles() {
        assert_eq!(role_to_kind_predicate("mention"), ("provenance", "mentions", false));
        assert_eq!(role_to_kind_predicate(""), ("provenance", "mentions", false));
        assert_eq!(role_to_kind_predicate("subject"), ("structural", "subject_of", false));
        assert_eq!(role_to_kind_predicate("object"), ("structural", "object_of", false));
    }

    #[test]
    fn role_to_kind_predicate_unknown_normalized() {
        let (k, p, n) = role_to_kind_predicate("triple");
        assert_eq!((k, p), ("provenance", "mentions"));
        assert!(n, "triple must signal normalization for audit");

        let (k, p, n) = role_to_kind_predicate("custom_role_x");
        assert_eq!((k, p), ("provenance", "mentions"));
        assert!(n);
    }

    #[test]
    fn uuid_from_hash_is_deterministic() {
        let a = uuid_from_hash("memory_entities|m1|e1|mention|provenance|mentions");
        let b = uuid_from_hash("memory_entities|m1|e1|mention|provenance|mentions");
        assert_eq!(a, b);
        // ...and different inputs yield different IDs:
        let c = uuid_from_hash("memory_entities|m1|e2|mention|provenance|mentions");
        assert_ne!(a, c);
        // Parses as a valid UUID:
        assert!(Uuid::parse_str(&a).is_ok());
    }
}

// =====================================================================
// T24 — backfill `hebbian_links` → `edges(kind=associative)`
// =====================================================================
//
// Phase C, heaviest table (~43,710 rows production). This is the only
// backfill driver that performs **SQL-side merge** before insert,
// because legacy `hebbian_links` allows both `(A, B)` and `(B, A)` as
// separate primary keys while design §4.3 requires that associative
// edges be canonicalized to `(min, max)` — one unified row per
// (canonical pair, signal_source).
//
// Production data analysis (2026-05-13, /Users/potato/rustclaw/engram-memory.db):
//
//   * 43,346 total rows
//   * 43,227 distinct canonical pairs
//   * 119 pairs have rows in BOTH directions (collision class)
//   * All 119 collisions are SAME signal_source — single-row-per-pair
//     post-merge
//   * All `signal_source = 'corecall'`, no NULL/empty
//   * `direction = 'bidirectional'` for all rows
//   * ~606 rows have non-zero `temporal_forward`
//   * ~666 rows have non-zero `temporal_backward`
//   * `signal_detail` empty everywhere in current production
//
// Merge policy per design §4.3:
//   * `weight = SUM(strength)` per (canonical_pair, namespace, signal_source)
//   * `coactivation_count = SUM(coactivation_count)` per same group
//   * `temporal_forward = SUM(temporal_forward)`
//   * `temporal_backward = SUM(temporal_backward)`
//   * `created_at = MIN(created_at)` (earliest observation wins)
//   * `direction` packed as sorted-distinct JSON array if heterogeneous,
//     scalar string if homogeneous (current production: always
//     `"bidirectional"`)
//   * `signal_detail` packed as sorted-distinct JSON array if
//     heterogeneous, scalar string if homogeneous (current production:
//     always empty)
//
// Deterministic id per amended design §5.3 hash template:
//
//   hash_input = "hebbian_links|<min_id>|<max_id>|<namespace>|associative|co_activated"
//
// — canonicalization of the endpoint pair happens INSIDE the hash so
// that `(A,B)` and `(B,A)` legacy rows both map to the same id and
// merge via `INSERT OR IGNORE`. Signal_source is NOT in the hash —
// design §4.3 keeps signal_source as a separate row-identity
// dimension via the partial unique index `idx_edges_assoc_unique`.
// Today production only has `signal_source='corecall'` so a hash that
// included signal_source would behave identically; the choice to
// EXCLUDE signal_source matches the design's "smallest UNIQUE tuple"
// rule (canonical pair + namespace IS the smallest unique tuple at
// the legacy-table level — signal_source is a future extension).
//
// CAVEAT: if production data ever acquires multi-signal-source rows
// for the same pair, the hash will collide. Design §4.3 declares
// signal_source the row-identity dimension and the partial unique
// index covers it via `json_extract(attributes, '$.signal_source')`,
// so the SECOND row (different signal_source, same id) would be
// rejected by `INSERT OR IGNORE` on the primary id BEFORE the unique
// index check — silently dropped. This is wrong. To future-proof,
// the hash MUST include signal_source. We do so below. See test
// `t24_hash_includes_signal_source_for_future_proofing`.
pub(crate) fn backfill_hebbian_links_to_edges_hash_input(
    canonical_lo: &str,
    canonical_hi: &str,
    namespace: &str,
    signal_source: &str,
) -> String {
    // Matches design §5.3 amended template for hebbian_links plus
    // signal_source as the additional discriminator from §4.3 row
    // identity. Six pipe-delimited tokens, fixed for this table.
    format!(
        "hebbian_links|{}|{}|{}|{}|associative|co_activated",
        canonical_lo, canonical_hi, namespace, signal_source
    )
}

/// Driver: backfill `hebbian_links` → `edges(kind=associative,
/// predicate=co_activated)` with SQL-side direction merge.
///
/// Restartable per the Phase C contract: deterministic `edges.id` +
/// `INSERT OR IGNORE` means a re-run inserts zero new rows. Safe to
/// invoke after T19 (memories→nodes) so endpoint FKs resolve. Skips
/// rows whose endpoints are missing from `nodes` and counts them in
/// `rows_skipped_dangling_endpoint` for the same self-recovering
/// pattern as T22/T23.
///
/// Namespace filter: if `namespace` is `Some(ns)`, only `hebbian_links`
/// rows with `namespace = ns` are processed. The legacy table has its
/// own namespace column — no JOIN required (unlike T23 which had to
/// derive namespace from the parent memory).
///
/// Counter buckets (collapsed into `rows_skipped_existing` for the
/// `BackfillRun` invariant, with detailed breakdown in notes JSON):
///   * `rows_skipped_existing` — row with the same deterministic id
///     already exists in `edges` (idempotent rerun)
///   * `rows_skipped_dangling_endpoint` — either endpoint not in
///     `nodes` yet
///   * `rows_skipped_mismatched_kind` — deterministic id collision
///     against a pre-existing edge of a different kind (contract
///     violation; never fires under correct setup but surfaced for
///     debugging)
///
/// **Audit field**: `notes.merged_collision_pairs` counts how many
/// canonical pairs had both `(A,B)` AND `(B,A)` legacy rows. This is
/// the most diagnostic Phase C statistic — if it's 0 on the production
/// DB we know all merges were trivial; if it's >0 the merge policy
/// actually fired.
pub fn backfill_hebbian_links_to_edges(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_hebbian_links_to_edges",
        "design_ref": "v04-unified-substrate §3.3 + §4.3 + §5.3 / T24",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'hebbian_links', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    // -----------------------------------------------------------------
    // SQL-side merge: group by (canonical_a, canonical_b, namespace,
    // signal_source). For each group, sum strength/coactivation_count/
    // temporal_*, min created_at, GROUP_CONCAT distinct direction +
    // signal_detail values (sorted) so we can decide scalar-vs-array
    // attribute layout downstream.
    //
    // We compute `rows_read = COUNT(*)` BEFORE the GROUP BY so the
    // invariant `rows_read = inserted + skipped` reflects how many
    // legacy rows we processed, not how many groups we produced.
    // -----------------------------------------------------------------

    // Rows-read count: total legacy rows the driver looked at.
    let rows_read: u64 = {
        let conn = storage.conn();
        let sql = if namespace.is_some() {
            "SELECT COUNT(*) FROM hebbian_links WHERE namespace = ?"
        } else {
            "SELECT COUNT(*) FROM hebbian_links"
        };
        let count: i64 = if let Some(ns) = namespace {
            conn.query_row(sql, params![ns], |r| r.get(0))?
        } else {
            conn.query_row(sql, [], |r| r.get(0))?
        };
        count as u64
    };

    // Distinct collision count for audit. A "collision pair" is a
    // canonical pair (lo, hi) that has legacy rows in both
    // directions. This stat is the smoking gun for whether the merge
    // policy actually fired.
    let merged_collision_pairs: u64 = {
        let conn = storage.conn();
        let sql = if namespace.is_some() {
            "SELECT COUNT(*) FROM (\
               SELECT 1 FROM hebbian_links h1 \
               WHERE h1.source_id < h1.target_id AND h1.namespace = ?1 \
                 AND EXISTS (\
                   SELECT 1 FROM hebbian_links h2 \
                   WHERE h2.source_id = h1.target_id \
                     AND h2.target_id = h1.source_id \
                     AND h2.namespace = ?1 \
                 ) \
             )"
        } else {
            "SELECT COUNT(*) FROM (\
               SELECT 1 FROM hebbian_links h1 \
               WHERE h1.source_id < h1.target_id \
                 AND EXISTS (\
                   SELECT 1 FROM hebbian_links h2 \
                   WHERE h2.source_id = h1.target_id \
                     AND h2.target_id = h1.source_id \
                 ) \
             )"
        };
        let n: i64 = if let Some(ns) = namespace {
            conn.query_row(sql, params![ns], |r| r.get(0))?
        } else {
            conn.query_row(sql, [], |r| r.get(0))?
        };
        n as u64
    };

    // Merged groups. One row per (canonical_pair, namespace,
    // signal_source). `legacy_count` (COUNT(*)) is folded into the
    // GROUP BY result itself so the merge loop doesn't have to
    // re-query per group — saves ~43k round-trips at production scale
    // (T24-r1 FINDING-2).
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT \
           CASE WHEN source_id < target_id THEN source_id ELSE target_id END AS canon_lo, \
           CASE WHEN source_id < target_id THEN target_id ELSE source_id END AS canon_hi, \
           namespace, \
           COALESCE(signal_source, 'corecall') AS signal_source, \
           SUM(strength) AS weight_sum, \
           SUM(coactivation_count) AS coact_sum, \
           SUM(temporal_forward) AS tfwd_sum, \
           SUM(temporal_backward) AS tbwd_sum, \
           MIN(created_at) AS min_created, \
           GROUP_CONCAT(DISTINCT direction) AS directions_csv, \
           GROUP_CONCAT(DISTINCT COALESCE(signal_detail, '')) AS details_csv, \
           COUNT(*) AS legacy_count \
         FROM hebbian_links \
         WHERE namespace = ? \
         GROUP BY canon_lo, canon_hi, namespace, signal_source"
    } else {
        "SELECT \
           CASE WHEN source_id < target_id THEN source_id ELSE target_id END AS canon_lo, \
           CASE WHEN source_id < target_id THEN target_id ELSE source_id END AS canon_hi, \
           namespace, \
           COALESCE(signal_source, 'corecall') AS signal_source, \
           SUM(strength) AS weight_sum, \
           SUM(coactivation_count) AS coact_sum, \
           SUM(temporal_forward) AS tfwd_sum, \
           SUM(temporal_backward) AS tbwd_sum, \
           MIN(created_at) AS min_created, \
           GROUP_CONCAT(DISTINCT direction) AS directions_csv, \
           GROUP_CONCAT(DISTINCT COALESCE(signal_detail, '')) AS details_csv, \
           COUNT(*) AS legacy_count \
         FROM hebbian_links \
         GROUP BY canon_lo, canon_hi, namespace, signal_source"
    };
    let mut stmt = conn.prepare(select_sql)?;
    type HebRow = (
        String, // canonical lo
        String, // canonical hi
        String, // namespace
        String, // signal_source
        f64,    // weight_sum
        i64,    // coact_sum
        i64,    // tfwd_sum
        i64,    // tbwd_sum
        f64,    // min_created_at
        String, // directions_csv
        String, // details_csv
        u64,    // legacy_count (rows collapsed in this group)
    );
    let map_row = |row: &rusqlite::Row| -> Result<HebRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            row.get::<_, Option<String>>(10)?.unwrap_or_default(),
            row.get::<_, i64>(11)? as u64,
        ))
    };
    let groups: Vec<HebRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<_, _>>()?
    } else {
        stmt.query_map([], map_row)?.collect::<Result<_, _>>()?
    };

    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_skipped_dangling_endpoint: u64 = 0;
    let mut rows_skipped_mismatched_kind: u64 = 0;
    let mut groups_processed: u64 = 0;
    // For each merged group, we may have collapsed N legacy rows into
    // 1 unified edge. To make `rows_read = inserted + skipped` hold,
    // a successful insert counts as `inserted += N_collapsed`, NOT 1.
    // Same for skipped. We track this via a per-group row count.

    let tx = storage.conn().unchecked_transaction()?;

    for (lo, hi, ns_val, sig_src, weight_sum, coact_sum, tfwd_sum, tbwd_sum, min_created, dirs_csv, details_csv, legacy_count)
        in groups
    {
        groups_processed += 1;

        // legacy_count comes pre-computed from the outer GROUP BY's
        // COUNT(*) aggregate (T24-r1 FINDING-2 fix). Previously this
        // block re-queried `SELECT COUNT(*) FROM hebbian_links WHERE
        // ...` per group, costing ~43k extra round-trips at
        // production scale. Folding into the outer SELECT is one
        // extra column for zero scan-cost overhead.

        // Deterministic id encoding canonical pair + namespace +
        // signal_source. See module-level comment for why
        // signal_source is in the hash even though §5.3's amended
        // template doesn't list it (future-proofing for multi-
        // signal-source production data).
        let hash_input =
            backfill_hebbian_links_to_edges_hash_input(&lo, &hi, &ns_val, &sig_src);
        let id = uuid_from_hash(&hash_input);

        // FK pre-check.
        let endpoints_ok: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ?) AND \
                 EXISTS(SELECT 1 FROM nodes WHERE id = ?)",
                params![lo, hi],
                |r: &rusqlite::Row<'_>| r.get::<_, i64>(0),
            )
            .map(|n| n != 0)?;

        if !endpoints_ok {
            rows_skipped_dangling_endpoint += legacy_count;
            continue;
        }

        // Decide direction representation: scalar if homogeneous,
        // sorted array if heterogeneous. GROUP_CONCAT preserves
        // insertion order; sort for deterministic output.
        let directions: Vec<String> = {
            let mut v: Vec<String> = dirs_csv
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let direction_value: serde_json::Value = match directions.len() {
            0 => json!("bidirectional"), // fallback if all NULL
            1 => json!(directions[0]),
            _ => json!(directions),
        };

        // Same logic for signal_detail. Production today has it all
        // empty so direction_value is the only non-trivial one.
        let details: Vec<String> = {
            let mut v: Vec<String> = details_csv
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            v.sort();
            v.dedup();
            v
        };
        let signal_detail_value: serde_json::Value = match details.len() {
            0 => json!(""),
            1 => json!(details[0]),
            _ => json!(details),
        };

        let attributes_json = json!({
            "signal_source": sig_src,
            "signal_detail": signal_detail_value,
            "coactivation_count": coact_sum,
            "temporal_forward": tfwd_sum,
            "temporal_backward": tbwd_sum,
            "direction": direction_value,
        })
        .to_string();

        // Defense-in-depth: detect deterministic id collisions
        // against a foreign kind. Should never fire because the
        // hash already encodes edge_kind+predicate effectively
        // (table name + canonical pair is unique to this driver),
        // but make the impossible visible.
        let existing_kind: Option<(String, String)> = tx
            .query_row(
                "SELECT edge_kind, predicate FROM edges WHERE id = ?",
                params![id],
                |r: &rusqlite::Row<'_>| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        let inserted = Storage::insert_associative_edge_row(
            &tx,
            &id,
            &lo,
            &hi,
            &attributes_json,
            weight_sum,
            &ns_val,
            min_created,
        )?;

        if inserted {
            rows_inserted += legacy_count;
        } else if let Some((ek, pp)) = existing_kind {
            if ek != "associative" || pp != "co_activated" {
                rows_skipped_mismatched_kind += legacy_count;
            } else {
                rows_skipped_existing += legacy_count;
            }
        } else {
            rows_skipped_existing += legacy_count;
        }
    }

    tx.commit()?;

    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_hebbian_links_to_edges",
        "design_ref": "v04-unified-substrate §3.3 + §4.3 + §5.3 / T24",
        "groups_processed": groups_processed,
        "merged_collision_pairs": merged_collision_pairs,
        "rows_skipped_dangling_endpoint": rows_skipped_dangling_endpoint,
        "rows_skipped_mismatched_kind": rows_skipped_mismatched_kind,
    })
    .to_string();

    let conn = storage.conn();
    conn.execute(
        r#"
        UPDATE backfill_runs
        SET rows_read = ?, rows_inserted = ?, rows_skipped_existing = ?,
            rows_failed = 0, finished_at = ?, notes = ?
        WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            (rows_skipped_existing + rows_skipped_dangling_endpoint + rows_skipped_mismatched_kind)
                as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "hebbian_links".into(),
        rows_read,
        rows_inserted,
        rows_skipped_existing: rows_skipped_existing
            + rows_skipped_dangling_endpoint
            + rows_skipped_mismatched_kind,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}

#[cfg(test)]
mod t24_helpers_tests {
    use super::*;

    #[test]
    fn t24_hash_canonicalization_collapses_directions() {
        // The crux of T24: (A,B) and (B,A) legacy rows MUST hash to
        // the same UUID so that INSERT OR IGNORE merges them.
        let a = backfill_hebbian_links_to_edges_hash_input("mem-A", "mem-B", "default", "corecall");
        // Caller is responsible for canonicalization, but if they
        // accidentally pass (lo=mem-B, hi=mem-A) the contract
        // breaks. Verify the deterministic id is order-sensitive
        // (so the driver MUST canonicalize before calling) — that's
        // the safer behaviour because it surfaces caller bugs.
        let b = backfill_hebbian_links_to_edges_hash_input("mem-B", "mem-A", "default", "corecall");
        assert_ne!(a, b, "hash is order-sensitive; caller MUST canonicalize");

        // After canonicalization at the call site, both directions
        // produce the same hash:
        let canon = |s: &str, t: &str| -> (String, String) {
            if s < t { (s.into(), t.into()) } else { (t.into(), s.into()) }
        };
        let (lo1, hi1) = canon("mem-A", "mem-B");
        let (lo2, hi2) = canon("mem-B", "mem-A");
        let h1 = backfill_hebbian_links_to_edges_hash_input(&lo1, &hi1, "default", "corecall");
        let h2 = backfill_hebbian_links_to_edges_hash_input(&lo2, &hi2, "default", "corecall");
        assert_eq!(h1, h2, "canonicalized directions hash identically");
    }

    #[test]
    fn t24_hash_includes_signal_source_for_future_proofing() {
        // Current production has signal_source='corecall' everywhere
        // so this dimension is degenerate today. But §4.3 declares
        // signal_source the row-identity dimension; future multi-
        // signal rows MUST get distinct edges. The hash must
        // therefore encode signal_source.
        let h1 = backfill_hebbian_links_to_edges_hash_input("a", "b", "default", "corecall");
        let h2 = backfill_hebbian_links_to_edges_hash_input("a", "b", "default", "multi");
        assert_ne!(h1, h2, "different signal_source MUST produce different id");
    }

    #[test]
    fn t24_hash_includes_namespace() {
        let h1 = backfill_hebbian_links_to_edges_hash_input("a", "b", "ns-a", "corecall");
        let h2 = backfill_hebbian_links_to_edges_hash_input("a", "b", "ns-b", "corecall");
        assert_ne!(h1, h2, "namespace is part of identity");
    }
}

// =============================================================
// T25 — synthesis_provenance → edges (provenance, derived_from)
// =============================================================

/// T25 — Phase C backfill: project `synthesis_provenance` rows into
/// the unified `edges` table as
/// `edge_kind='provenance', predicate='derived_from'`.
///
/// Field mapping (per v04-unified-substrate design.md §3.3 + §4.5 + §5.3):
///   * `id`              — **legacy.id passed through verbatim** (NOT
///                         a hash). Provenance is append-only and has
///                         no partial unique index per §3.2; Phase B's
///                         T16 dual-write uses legacy.id directly so a
///                         re-emission via Phase B AFTER backfill
///                         collides with the backfilled edge on PK
///                         (idempotent), rather than landing as a
///                         second edge under a hashed id.
///   * `source_id`       — `legacy.insight_id` (insight is "derived from"
///                         the source, so edge points insight → source).
///   * `target_id`       — `legacy.source_id`.
///   * `edge_kind`       — `'provenance'` (closed taxonomy §3.3).
///   * `predicate`       — `'derived_from'` (§3.3 row 9).
///   * `confidence`      — `legacy.confidence` passed through. **This is
///                         the first Phase C driver to pass a legacy
///                         confidence column through to the helper
///                         (others used 1.0 because their legacy tables
///                         had no confidence column).** Establishes the
///                         FINDING-3 policy: legacy-column-wins when
///                         present, default-to-1.0 otherwise.
///   * `namespace`       — derived from `insight_id`'s memory namespace
///                         (JOIN). synthesis_provenance has no own NS
///                         column; same pattern as T23 memory_entities.
///   * `attributes` JSON — embeds `gate_decision`, parsed `gate_scores`,
///                         `cluster_id`, `source_original_importance`,
///                         `synthesis_timestamp` (verbatim string).
///   * `created_at`,
///   * `recorded_at`,
///   * `updated_at`      — all parsed from `synthesis_timestamp` (RFC3339
///                         string in legacy) per Phase B T16 convention.
///
/// FK guard: skip rows whose `insight_id` or `source_id` has no
/// projected node in unified `nodes` (i.e. T19 hasn't run yet, or
/// the parent memory was hard-deleted). Recorded in audit notes as
/// `rows_skipped_dangling_endpoint`. Re-run after T19 picks them up.
///
/// Re-run semantics: per-row `INSERT OR IGNORE`. On second run, any
/// edge whose id is already in `edges` increments `rows_skipped_existing`.
/// Mismatched-kind defense-in-depth: if a row with the same id exists
/// under a different `edge_kind` / `predicate`, increment
/// `rows_skipped_mismatched_kind`. Under correct contract this cannot
/// happen (legacy.id is a UUID minted by the synthesis writer; no
/// other driver uses raw UUIDs as edge ids).
pub fn backfill_synthesis_provenance_to_edges(
    storage: &mut Storage,
    namespace: Option<&str>,
) -> Result<BackfillRun, rusqlite::Error> {
    let run_id = Uuid::new_v4().to_string();
    let started_at = utc_now_f64();

    let notes_open = json!({
        "namespace_filter": namespace,
        "driver": "backfill_synthesis_provenance_to_edges",
        "design_ref": "v04-unified-substrate §5.3 / T25",
    })
    .to_string();

    storage.conn().execute(
        r#"
        INSERT INTO backfill_runs (
            run_id, legacy_table, rows_read, rows_inserted,
            rows_skipped_existing, rows_failed,
            started_at, finished_at, notes
        ) VALUES (?, 'synthesis_provenance', 0, 0, 0, 0, ?, NULL, ?)
        "#,
        params![run_id, started_at, notes_open],
    )?;

    // Count is namespace-scoped via the JOIN on memories(insight_id).
    let rows_read: i64 = if let Some(ns) = namespace {
        storage.conn().query_row(
            "SELECT COUNT(*) FROM synthesis_provenance sp \
             JOIN memories mi ON mi.id = sp.insight_id \
             WHERE mi.namespace = ?",
            params![ns],
            |r| r.get(0),
        )?
    } else {
        storage
            .conn()
            .query_row("SELECT COUNT(*) FROM synthesis_provenance", [], |r| r.get(0))?
    };
    let rows_read = rows_read as u64;

    // Hydrate. JOIN to memories to fetch insight namespace; LEFT
    // JOIN twice to `nodes` to fold the FK guard into the SELECT
    // (T25-r1 FINDING-2 — saves ~903 per-row round-trips); LEFT JOIN
    // to `edges` to fold the existing-edge probe used for the
    // mismatched-kind defense-in-depth (T25-r1 FINDING-4).
    let conn = storage.conn();
    let select_sql = if namespace.is_some() {
        "SELECT sp.id, sp.insight_id, sp.source_id, sp.cluster_id, \
                sp.synthesis_timestamp, sp.gate_decision, sp.gate_scores, \
                sp.confidence, sp.source_original_importance, \
                mi.namespace, \
                (ni.id IS NOT NULL AND ns.id IS NOT NULL) AS endpoints_ok, \
                ee.edge_kind AS existing_edge_kind, \
                ee.predicate AS existing_edge_predicate \
         FROM synthesis_provenance sp \
         JOIN memories mi ON mi.id = sp.insight_id \
         LEFT JOIN nodes ni ON ni.id = sp.insight_id \
         LEFT JOIN nodes ns ON ns.id = sp.source_id \
         LEFT JOIN edges ee ON ee.id = sp.id \
         WHERE mi.namespace = ?"
    } else {
        "SELECT sp.id, sp.insight_id, sp.source_id, sp.cluster_id, \
                sp.synthesis_timestamp, sp.gate_decision, sp.gate_scores, \
                sp.confidence, sp.source_original_importance, \
                mi.namespace, \
                (ni.id IS NOT NULL AND ns.id IS NOT NULL) AS endpoints_ok, \
                ee.edge_kind AS existing_edge_kind, \
                ee.predicate AS existing_edge_predicate \
         FROM synthesis_provenance sp \
         JOIN memories mi ON mi.id = sp.insight_id \
         LEFT JOIN nodes ni ON ni.id = sp.insight_id \
         LEFT JOIN nodes ns ON ns.id = sp.source_id \
         LEFT JOIN edges ee ON ee.id = sp.id"
    };
    let mut stmt = conn.prepare(select_sql)?;
    type SpRow = (
        String,         // id
        String,         // insight_id
        String,         // source_id
        String,         // cluster_id
        String,         // synthesis_timestamp (RFC3339)
        String,         // gate_decision
        Option<String>, // gate_scores (JSON string or NULL)
        f64,            // confidence
        Option<f64>,    // source_original_importance
        String,         // namespace (from insight memory)
        bool,           // endpoints_ok (both nodes projected)
        Option<String>, // existing_edge_kind (None when no row)
        Option<String>, // existing_edge_predicate
    );
    let map_row = |row: &rusqlite::Row| -> Result<SpRow, rusqlite::Error> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
        ))
    };
    let rows: Vec<SpRow> = if let Some(ns) = namespace {
        stmt.query_map(params![ns], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    drop(stmt);

    let tx = storage.conn().unchecked_transaction()?;

    let mut rows_inserted: u64 = 0;
    let mut rows_skipped_existing: u64 = 0;
    let mut rows_skipped_dangling_endpoint: u64 = 0;
    let mut rows_skipped_mismatched_kind: u64 = 0;

    for (
        id,
        insight_id,
        source_id,
        cluster_id,
        synthesis_timestamp,
        gate_decision,
        gate_scores,
        confidence,
        source_original_importance,
        ns_val,
        endpoints_ok,
        existing_edge_kind,
        existing_edge_predicate,
    ) in &rows
    {
        // FK guard: both endpoints must have projected nodes.
        // Folded into the outer SELECT via LEFT JOIN nodes (T25-r1
        // FINDING-2 — eliminated ~903 per-row round-trips).
        if !*endpoints_ok {
            rows_skipped_dangling_endpoint += 1;
            continue;
        }

        // Parse synthesis_timestamp (RFC3339) → unix epoch.
        // synthesis_timestamp is stored as RFC3339 in legacy. Fall
        // back to current time if parsing fails (never observed in
        // production but defensive — a malformed legacy row would
        // otherwise crash the whole run).
        let ts_unix: f64 = match chrono::DateTime::parse_from_rfc3339(synthesis_timestamp) {
            Ok(dt) => {
                let dt_utc = dt.with_timezone(&chrono::Utc);
                dt_utc.timestamp() as f64
                    + (dt_utc.timestamp_subsec_nanos() as f64 / 1e9)
            }
            Err(_) => utc_now_f64(),
        };

        // Build attributes JSON. gate_scores in legacy is a TEXT
        // column holding pre-encoded JSON; we parse and re-embed so
        // it lands as a nested object, not a quoted string.
        let mut attrs = serde_json::Map::new();
        attrs.insert(
            "gate_decision".to_string(),
            serde_json::Value::String(gate_decision.clone()),
        );
        attrs.insert(
            "cluster_id".to_string(),
            serde_json::Value::String(cluster_id.clone()),
        );
        attrs.insert(
            "synthesis_timestamp".to_string(),
            serde_json::Value::String(synthesis_timestamp.clone()),
        );
        if let Some(score_json) = gate_scores.as_deref() {
            if !score_json.is_empty() {
                match serde_json::from_str::<serde_json::Value>(score_json) {
                    Ok(v) => {
                        attrs.insert("gate_scores".to_string(), v);
                    }
                    Err(_) => {
                        // Malformed legacy gate_scores — preserve as
                        // a string so the operator can see it.
                        attrs.insert(
                            "gate_scores".to_string(),
                            serde_json::Value::String(score_json.to_string()),
                        );
                    }
                }
            }
        }
        if let Some(orig) = source_original_importance {
            attrs.insert(
                "source_original_importance".to_string(),
                serde_json::Number::from_f64(*orig)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        let attrs_json = serde_json::to_string(&serde_json::Value::Object(attrs))
            .expect("serializing serde_json::Map cannot fail");

        // Defense-in-depth: check id collision under a different
        // edge_kind/predicate. Folded into the outer SELECT via
        // LEFT JOIN edges (T25-r1 FINDING-4 — eliminated ~903
        // per-row round-trips). Under correct contract this never
        // fires — legacy.id is a UUID minted by the synthesis writer
        // and no other driver uses raw UUIDs as edge ids.
        let existing_kind: Option<(String, String)> =
            match (existing_edge_kind, existing_edge_predicate) {
                (Some(k), Some(p)) => Some((k.clone(), p.clone())),
                _ => None,
            };

        let inserted = Storage::insert_provenance_edge_row(
            &tx,
            id,
            insight_id,
            source_id,
            "derived_from",
            &attrs_json,
            *confidence,
            ns_val,
            ts_unix,
        )?;

        if inserted {
            rows_inserted += 1;
        } else if let Some((ek, pp)) = existing_kind {
            if ek != "provenance" || pp != "derived_from" {
                rows_skipped_mismatched_kind += 1;
            } else {
                rows_skipped_existing += 1;
            }
        } else {
            rows_skipped_existing += 1;
        }
    }

    tx.commit()?;

    let finished_at = utc_now_f64();
    let notes_closed = json!({
        "namespace_filter": namespace,
        "driver": "backfill_synthesis_provenance_to_edges",
        "design_ref": "v04-unified-substrate §5.3 / T25",
        "rows_skipped_dangling_endpoint": rows_skipped_dangling_endpoint,
        "rows_skipped_mismatched_kind": rows_skipped_mismatched_kind,
        "confidence_policy": "legacy.confidence pass-through (FINDING-3 reference impl)",
    })
    .to_string();

    storage.conn().execute(
        r#"
        UPDATE backfill_runs
           SET rows_read = ?,
               rows_inserted = ?,
               rows_skipped_existing = ?,
               rows_failed = 0,
               finished_at = ?,
               notes = ?
         WHERE run_id = ?
        "#,
        params![
            rows_read as i64,
            rows_inserted as i64,
            (rows_skipped_existing + rows_skipped_dangling_endpoint + rows_skipped_mismatched_kind)
                as i64,
            finished_at,
            notes_closed,
            run_id,
        ],
    )?;

    let run = BackfillRun {
        run_id,
        legacy_table: "synthesis_provenance".to_string(),
        rows_read,
        rows_inserted,
        rows_skipped_existing: rows_skipped_existing
            + rows_skipped_dangling_endpoint
            + rows_skipped_mismatched_kind,
        rows_failed: 0,
    };
    run.assert_counter_invariant();
    Ok(run)
}
