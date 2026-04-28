//! Failure surfacing & retry driver for backfill (Phase 4 of the migration ladder).
//!
//! Implements design §5.3 of `.gid/features/v03-migration/design.md`:
//!
//! > Per GOAL-4.4 and GUARD-2, extraction failures during backfill **must
//! > be visible per-record**. […] Every per-record failure writes a row to
//! > `graph_extraction_failures` […]. Failure rows are **retryable**. The
//! > `engramai migrate --retry-failed` subcommand scans
//! > `graph_extraction_failures WHERE resolved_at IS NULL` and re-invokes
//! > the pipeline on just those memory IDs.
//!
//! Satisfies GOAL-4.4 (per-record failure visibility, retryable without
//! redoing successes) and contributes to GUARD-2 (no silent degradation).
//!
//! ## Scope of this task (T10)
//!
//! T10 owns three things, all driven through the same
//! [`RecordProcessor`](crate::backfill::RecordProcessor) seam T8 introduced:
//!
//! 1. **Writing failure rows** ([`record_failure`], [`record_outcome_failure`]).
//!    Called by T9's `RecordProcessor::process_one` after a per-record
//!    failure is observed. The write is idempotent (`INSERT OR IGNORE`
//!    on the failure id) — replays from the same orchestrator run cannot
//!    double-count.
//! 2. **Querying unresolved failures** ([`list_unresolved`],
//!    [`count_unresolved`]). Used by the CLI `--status` view and by
//!    `--retry-failed` to enumerate work.
//! 3. **The retry driver** ([`retry_failed`]). Iterates the unresolved
//!    set, hands each row's `MemoryRecord` to a caller-supplied
//!    `RecordProcessor`, and on success calls [`mark_resolved`] (or
//!    [`bump_retry_count`] on continued failure). The driver is the
//!    `engramai migrate --retry-failed` library entry point — the CLI
//!    (T14) is a thin wrapper that constructs a [`RetryConfig`] and
//!    invokes this function.
//!
//! ## Why this is decoupled from T9
//!
//! T10 operates entirely against the abstract
//! [`RecordProcessor`](crate::backfill::RecordProcessor) trait the
//! orchestrator (T8) already established. It does not call into v03-resolution
//! directly: every "do the actual extraction" call goes through
//! `processor.process_one(...)`, exactly as the orchestrator does. This
//! means T10 can land while T9 is still blocked on the upstream
//! `ResolutionPipeline::resolve_for_backfill` triage, because:
//!
//! - The schema constants ([`STAGE_*`], [`CATEGORY_*`]) are reproduced
//!   from v03-graph-layer §4.1 — closed sets validated in code, not via
//!   `CHECK` constraints (per the same rationale as
//!   `validate_failure_closed_sets` in `engramai/src/graph/store.rs`).
//! - The DDL `graph_extraction_failures` is owned by v03-graph-layer; this
//!   module only writes/reads rows. The `engramai-migrate` crate stays a
//!   leaf crate — no `engramai` dependency.
//! - The `RecordProcessor` trait is the only seam: when T9 lands, the same
//!   processor instance the orchestrator uses can be passed to
//!   [`retry_failed`].
//!
//! ## Schema reproduced here
//!
//! `graph_extraction_failures` columns we read/write:
//!
//! ```sql
//! id              BLOB PRIMARY KEY,
//! episode_id      BLOB NOT NULL,
//! stage           TEXT NOT NULL,                  -- closed set, see STAGE_*
//! error_category  TEXT NOT NULL,                  -- closed set, see CATEGORY_*
//! error_detail    TEXT,
//! occurred_at     REAL NOT NULL,                  -- unix seconds
//! retry_count     INTEGER NOT NULL DEFAULT 0,
//! resolved_at     REAL,                           -- NULL until resolved
//! namespace       TEXT NOT NULL DEFAULT 'default'
//! ```
//!
//! ## Idempotence model
//!
//! - **Writes** use `INSERT OR IGNORE` keyed on `id`. Two strategies for
//!   computing `id`:
//!   - If the processor supplied an `episode_id` (T9, when the pipeline
//!     allocated an episode before failing), `id = uuid_v5(NS_FAILURE,
//!     episode_id || stage)`. Same episode + same stage → same row.
//!   - If no episode is available (extraction blew up before episode
//!     allocation), `id = uuid_v5(NS_FAILURE, "memory:" || memory_id ||
//!     "|" || stage)`. The function [`derive_failure_episode_id`]
//!     deterministically fabricates an `episode_id` from the memory_id
//!     in the same case so the schema's `NOT NULL` constraint is
//!     honored without inventing a "real" episode.
//! - **Retries** that succeed call `UPDATE … SET resolved_at = ? WHERE id
//!   = ? AND resolved_at IS NULL` — monotone (once resolved, never
//!   re-cleared, mirroring the graph-layer's `mark_failure_resolved`
//!   semantics).
//! - **Retries** that fail again call `UPDATE … SET retry_count =
//!   retry_count + 1, occurred_at = ?, error_detail = ? WHERE id = ?`
//!   — preserves the original `id` (so the next retry still hits the
//!   same row) and refreshes the timestamp (so operators can see
//!   "this has been failing for N attempts since X").

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::backfill::{MemoryRecord, RecordOutcome, RecordProcessor};
use crate::checkpoint::CheckpointStore;
use crate::error::MigrationError;

// ===========================================================================
// Closed-set constants (reproduced from v03-graph-layer §4.1)
// ===========================================================================

/// Pipeline stage constants for `graph_extraction_failures.stage`.
///
/// Closed set validated in [`validate_stage`]. Mirrors the constants in
/// `engramai/src/graph/audit.rs` — duplicated here (not depended on) to
/// keep `engramai-migrate` a leaf crate. If v03-graph-layer adds a stage,
/// the matching constant must be added here too; the `validate_stage`
/// helper makes the contract explicit.
pub const STAGE_INGEST: &str = "ingest";
pub const STAGE_ENTITY_EXTRACT: &str = "entity_extract";
pub const STAGE_EDGE_EXTRACT: &str = "edge_extract";
pub const STAGE_RESOLVE: &str = "resolve";
pub const STAGE_DEDUP: &str = "dedup";
pub const STAGE_PERSIST: &str = "persist";
pub const STAGE_KNOWLEDGE_COMPILE: &str = "knowledge_compile";
/// Migration-only stage for topic carry-forward failures (design §6.4).
/// Owned by this feature per design §10.4 ("Migration adds
/// `ExtractionStage::TopicCarryForward` as the only new variant specific
/// to this feature").
pub const STAGE_TOPIC_CARRY_FORWARD: &str = "topic_carry_forward";

/// Error category constants for `graph_extraction_failures.error_category`.
pub const CATEGORY_LLM_TIMEOUT: &str = "llm_timeout";
pub const CATEGORY_LLM_INVALID_OUTPUT: &str = "llm_invalid_output";
pub const CATEGORY_BUDGET_EXHAUSTED: &str = "budget_exhausted";
pub const CATEGORY_DB_ERROR: &str = "db_error";
pub const CATEGORY_INTERNAL: &str = "internal";

// Pipeline call-site categories (ISS-047). Must mirror
// `engramai/src/graph/audit.rs` — kept duplicated to preserve
// engramai-migrate's leaf-crate property.
pub const CATEGORY_EXTRACTOR_ERROR: &str = "extractor_error";
pub const CATEGORY_CANDIDATE_RETRIEVAL_ERROR: &str = "candidate_retrieval_error";
pub const CATEGORY_CANONICAL_FETCH_ERROR: &str = "canonical_fetch_error";
pub const CATEGORY_UNRESOLVED_SUBJECT: &str = "unresolved_subject";
pub const CATEGORY_UNRESOLVED_OBJECT: &str = "unresolved_object";
pub const CATEGORY_FIND_EDGES_ERROR: &str = "find_edges_error";
pub const CATEGORY_APPLY_GRAPH_DELTA_ERROR: &str = "apply_graph_delta_error";
pub const CATEGORY_MISSING_CANONICAL: &str = "missing_canonical";
pub const CATEGORY_UNRESOLVED_DEFER: &str = "unresolved_defer";
pub const CATEGORY_QUEUE_FULL: &str = "queue_full";

/// UUID v5 namespace for migration-derived failure ids.
///
/// Random but stable: regenerating this constant would re-key every failure
/// row and break idempotence across releases. Treat as immutable.
const NS_FAILURE: Uuid = Uuid::from_bytes([
    0x4d, 0x69, 0x67, 0x72, 0x46, 0x61, 0x69, 0x6c, 0x45, 0x70, 0x69, 0x73, 0x6f, 0x64, 0x65, 0x21,
]);

/// Default namespace value used when the migrating database does not yet
/// have a populated `namespace` column on its rows. Mirrors the column
/// `DEFAULT 'default'` in v03-graph-layer's DDL.
pub const DEFAULT_NAMESPACE: &str = "default";

// ===========================================================================
// Public types
// ===========================================================================

/// Validate a `stage` string against the closed set.
///
/// Mirrors `validate_failure_closed_sets` from
/// `engramai/src/graph/store.rs`. Rejecting an unknown stage early keeps
/// arbitrary string drift out of the audit table — closed-set validation
/// is the single line of defense (no DB CHECK clause, by design — adding
/// a stage in v0.4 should not require a schema migration).
pub fn validate_stage(stage: &str) -> Result<(), MigrationError> {
    const STAGES: &[&str] = &[
        STAGE_INGEST,
        STAGE_ENTITY_EXTRACT,
        STAGE_EDGE_EXTRACT,
        STAGE_RESOLVE,
        STAGE_DEDUP,
        STAGE_PERSIST,
        STAGE_KNOWLEDGE_COMPILE,
        STAGE_TOPIC_CARRY_FORWARD,
    ];
    if STAGES.contains(&stage) {
        Ok(())
    } else {
        Err(MigrationError::InvariantViolated(format!(
            "graph_extraction_failures.stage out of closed set: {stage:?}"
        )))
    }
}

/// Validate an `error_category` string against the closed set.
pub fn validate_error_category(category: &str) -> Result<(), MigrationError> {
    const CATEGORIES: &[&str] = &[
        CATEGORY_LLM_TIMEOUT,
        CATEGORY_LLM_INVALID_OUTPUT,
        CATEGORY_BUDGET_EXHAUSTED,
        CATEGORY_DB_ERROR,
        CATEGORY_INTERNAL,
        // Pipeline call-site labels (ISS-047)
        CATEGORY_EXTRACTOR_ERROR,
        CATEGORY_CANDIDATE_RETRIEVAL_ERROR,
        CATEGORY_CANONICAL_FETCH_ERROR,
        CATEGORY_UNRESOLVED_SUBJECT,
        CATEGORY_UNRESOLVED_OBJECT,
        CATEGORY_FIND_EDGES_ERROR,
        CATEGORY_APPLY_GRAPH_DELTA_ERROR,
        CATEGORY_MISSING_CANONICAL,
        CATEGORY_UNRESOLVED_DEFER,
        CATEGORY_QUEUE_FULL,
    ];
    if CATEGORIES.contains(&category) {
        Ok(())
    } else {
        Err(MigrationError::InvariantViolated(format!(
            "graph_extraction_failures.error_category out of closed set: {category:?}"
        )))
    }
}

/// One row of `graph_extraction_failures` plus the migration-side fields
/// needed to drive a retry.
///
/// `memory_id` is recovered from `error_detail` (which T10 always
/// prefixes with `"memory:<id>|"` to preserve the link from the failure
/// row back to the source `memories.id`). The schema does not have a
/// `memory_id` column — see module docs for why we don't widen it.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureRecord {
    pub id: Uuid,
    pub episode_id: Uuid,
    /// Source `memories.id`. Recovered from `error_detail`'s
    /// `"memory:<id>|"` prefix; `None` if the row was not written by
    /// migration (e.g., a live ingest failure copied here by some other
    /// path — retry won't run on it).
    pub memory_id: Option<i64>,
    pub stage: String,
    pub error_category: String,
    pub error_detail: Option<String>,
    pub occurred_at: f64,
    pub retry_count: i64,
    pub resolved_at: Option<f64>,
    pub namespace: String,
}

/// Configuration for the [`retry_failed`] driver.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Hard cap on retries per failure row. Once `retry_count >= max_retries`
    /// the row is skipped (operator must intervene). Default: 5.
    pub max_retries: i64,
    /// Namespace filter — only retry rows whose `namespace` matches this
    /// value. Defaults to `"default"`, matching the DDL default.
    pub namespace: String,
    /// If `Some(n)`, retry at most `n` rows per call. Defaults to `None`
    /// (retry all unresolved). Useful for paginated CLI runs.
    pub limit: Option<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            namespace: DEFAULT_NAMESPACE.to_string(),
            limit: None,
        }
    }
}

/// Outcome of a single [`retry_failed`] invocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetrySummary {
    /// Rows the driver visited (matched the namespace + retry-cap predicate).
    pub considered: u64,
    /// Rows where the retry succeeded — `resolved_at` was set.
    pub resolved: u64,
    /// Rows where the retry failed again — `retry_count` was incremented.
    pub still_failing: u64,
    /// Rows skipped because they exceeded `max_retries`.
    pub skipped_over_cap: u64,
    /// Rows that could not be re-run because the source `memories.id` was
    /// missing or unreadable (e.g., row deleted between failure and retry).
    pub skipped_missing_record: u64,
}

// ===========================================================================
// id derivation
// ===========================================================================

/// Derive a deterministic `episode_id` for a memory record that failed
/// before the resolution pipeline allocated a real episode.
///
/// `graph_extraction_failures.episode_id` is `NOT NULL`, but extraction
/// failures during backfill can fire *before* an episode exists (e.g., the
/// pipeline rejects the record at the `entity_extract` stage and never
/// reaches episode creation). Rather than nulling out the column (forbidden
/// by the schema) or fabricating an arbitrary UUID v4 (would re-key on
/// retry, breaking idempotence), we hash the memory_id under a stable v5
/// namespace. Same `memory_id` → same UUID across runs, across processes,
/// across machines.
pub fn derive_failure_episode_id(memory_id: i64) -> Uuid {
    Uuid::new_v5(
        &NS_FAILURE,
        format!("memory:{memory_id}").as_bytes(),
    )
}

/// Derive a deterministic failure-row `id` for a `(namespace, episode_id,
/// stage)` triple.
///
/// Same `(namespace, episode_id, stage)` → same `id`. Combined with `INSERT
/// OR IGNORE`, this gives idempotence across replays: the orchestrator can
/// re-run after a crash and the same row will not appear twice. Including
/// `namespace` in the key prevents row collisions between tenants when
/// the same `memory_id` exists under multiple namespaces (which can
/// happen during multi-tenant migration runs).
pub fn derive_failure_id(namespace: &str, episode_id: Uuid, stage: &str) -> Uuid {
    Uuid::new_v5(
        &NS_FAILURE,
        format!("{}|{}|{}", namespace, episode_id.as_hyphenated(), stage).as_bytes(),
    )
}

/// Build the `error_detail` string that links the failure row back to its
/// `memories.id`. Used by [`record_outcome_failure`] and parsed back out
/// by [`list_unresolved`] / [`retry_failed`].
///
/// Format: `"memory:<id>|<message>"` where `<message>` is the
/// processor-supplied detail (free-form). The leading `memory:<id>|` token
/// is the only part this module relies on for round-tripping.
pub fn format_error_detail(memory_id: i64, message: &str) -> String {
    format!("memory:{memory_id}|{message}")
}

/// Parse a `memories.id` out of an `error_detail` string written by
/// [`format_error_detail`]. Returns `None` if the row was written by some
/// other code path (e.g., a live ingest failure not associated with a
/// migration record).
pub fn parse_memory_id_from_detail(detail: Option<&str>) -> Option<i64> {
    let detail = detail?;
    let rest = detail.strip_prefix("memory:")?;
    let (id_str, _) = rest.split_once('|')?;
    id_str.parse::<i64>().ok()
}

// ===========================================================================
// Writes
// ===========================================================================

/// Inputs to [`record_failure`]. A struct (not a long argument list) so the
/// call sites read clearly and so adding a field later (e.g., extraction
/// trace pointer) doesn't ripple through every caller.
#[derive(Debug, Clone)]
pub struct FailureWrite<'a> {
    pub memory_id: i64,
    /// If the pipeline allocated an episode before failing, supply it here.
    /// `None` → [`record_failure`] derives a deterministic surrogate via
    /// [`derive_failure_episode_id`].
    pub episode_id: Option<Uuid>,
    pub stage: &'a str,
    pub error_category: &'a str,
    /// Free-form, operator-readable. Wrapped via [`format_error_detail`].
    /// May be empty.
    pub message: &'a str,
    pub occurred_at: DateTime<Utc>,
    pub namespace: &'a str,
}

/// Write one `graph_extraction_failures` row.
///
/// Idempotent: replays compute the same `id` (via [`derive_failure_id`]),
/// and the underlying `INSERT OR IGNORE` discards duplicates. Returns the
/// `id` of the row (the existing one on a duplicate, the new one
/// otherwise).
///
/// Validates `stage` and `error_category` against the closed sets — an
/// out-of-set value is a programming error, surfaced as
/// [`MigrationError::InvariantViolated`].
///
/// `tx` is borrowed (`&Transaction`) so this can be called inside the
/// processor's per-record transaction (matching the orchestrator's
/// "advance-after-commit" invariant — failure surfacing must commit
/// atomically with the checkpoint advance).
pub fn record_failure(
    tx: &Transaction<'_>,
    write: &FailureWrite<'_>,
) -> Result<Uuid, MigrationError> {
    validate_stage(write.stage)?;
    validate_error_category(write.error_category)?;

    let episode_id = write
        .episode_id
        .unwrap_or_else(|| derive_failure_episode_id(write.memory_id));
    let id = derive_failure_id(write.namespace, episode_id, write.stage);
    let detail = format_error_detail(write.memory_id, write.message);
    let occurred = dt_to_unix(write.occurred_at);

    tx.execute(
        "INSERT OR IGNORE INTO graph_extraction_failures (
             id, episode_id, stage, error_category,
             error_detail, occurred_at, retry_count, resolved_at, namespace
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, ?7)",
        params![
            id.as_bytes().to_vec(),
            episode_id.as_bytes().to_vec(),
            write.stage,
            write.error_category,
            detail,
            occurred,
            write.namespace,
        ],
    )
    .map_err(map_sqlite)?;

    Ok(id)
}

/// Convenience adapter: translate a [`RecordOutcome::Failed`] (the value
/// the orchestrator hands us) into a [`record_failure`] call.
///
/// The trait-level `RecordOutcome::Failed` carries `kind` and `stage` as
/// free-form strings (so `engramai-migrate` stays decoupled from the
/// `ResolutionError` enum). This helper does the light translation:
///
/// - `outcome.kind` → `error_category` (must be one of the
///   `CATEGORY_*` constants — validated)
/// - `outcome.stage` → `stage` (must be one of the `STAGE_*` constants —
///   validated)
/// - `outcome.record_id` → `memory_id`
///
/// Returns the failure-row `id` for downstream logging / progress reporting.
pub fn record_outcome_failure(
    tx: &Transaction<'_>,
    outcome: &RecordOutcome,
    occurred_at: DateTime<Utc>,
    namespace: &str,
) -> Result<Option<Uuid>, MigrationError> {
    match outcome {
        RecordOutcome::Succeeded { .. } => Ok(None),
        RecordOutcome::Failed {
            record_id,
            kind,
            stage,
            episode_id,
            message,
        } => {
            let write = FailureWrite {
                memory_id: *record_id,
                episode_id: *episode_id,
                stage,
                error_category: kind,
                message,
                occurred_at,
                namespace,
            };
            record_failure(tx, &write).map(Some)
        }
    }
}

/// Mark a failure row resolved. Monotone: refuses to clear a non-NULL
/// `resolved_at` — once resolved, always resolved (mirrors graph-layer's
/// `mark_failure_resolved` semantics).
///
/// Idempotent: marking an already-resolved row is a silent no-op (returns
/// `false` to indicate "no change").
pub fn mark_resolved(
    tx: &Transaction<'_>,
    failure_id: Uuid,
    at: DateTime<Utc>,
) -> Result<bool, MigrationError> {
    let now = dt_to_unix(at);
    let updated = tx
        .execute(
            "UPDATE graph_extraction_failures
             SET resolved_at = ?1
             WHERE id = ?2 AND resolved_at IS NULL",
            params![now, failure_id.as_bytes().to_vec()],
        )
        .map_err(map_sqlite)?;
    Ok(updated > 0)
}

/// Increment a failure row's `retry_count` and refresh its `occurred_at`
/// and `error_detail` after a retry that failed again. Preserves the
/// original `id` so the next retry still hits the same row.
pub fn bump_retry_count(
    tx: &Transaction<'_>,
    failure_id: Uuid,
    new_occurred_at: DateTime<Utc>,
    new_message: Option<&str>,
    memory_id_for_detail: i64,
) -> Result<(), MigrationError> {
    let detail = new_message.map(|m| format_error_detail(memory_id_for_detail, m));
    let now = dt_to_unix(new_occurred_at);
    tx.execute(
        "UPDATE graph_extraction_failures
         SET retry_count  = retry_count + 1,
             occurred_at  = ?1,
             error_detail = COALESCE(?2, error_detail)
         WHERE id = ?3",
        params![now, detail, failure_id.as_bytes().to_vec()],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

// ===========================================================================
// Reads
// ===========================================================================

/// List unresolved failure rows in `namespace`, ordered by `occurred_at`
/// ascending (oldest first — the common operator question is "what's been
/// stuck the longest").
///
/// `limit` caps the row count; `None` returns all rows. The retry driver
/// uses this with `RetryConfig::limit`.
pub fn list_unresolved(
    conn: &Connection,
    namespace: &str,
    limit: Option<u64>,
) -> Result<Vec<FailureRecord>, MigrationError> {
    let sql = match limit {
        Some(_) => {
            "SELECT id, episode_id, stage, error_category, error_detail,
                    occurred_at, retry_count, resolved_at, namespace
             FROM graph_extraction_failures
             WHERE resolved_at IS NULL AND namespace = ?1
             ORDER BY occurred_at ASC
             LIMIT ?2"
        }
        None => {
            "SELECT id, episode_id, stage, error_category, error_detail,
                    occurred_at, retry_count, resolved_at, namespace
             FROM graph_extraction_failures
             WHERE resolved_at IS NULL AND namespace = ?1
             ORDER BY occurred_at ASC"
        }
    };

    let mut stmt = conn.prepare(sql).map_err(map_sqlite)?;
    let rows = match limit {
        Some(n) => stmt
            .query_map(params![namespace, n as i64], row_to_failure_record)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?,
        None => stmt
            .query_map(params![namespace], row_to_failure_record)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?,
    };
    Ok(rows)
}

/// Count unresolved failure rows in `namespace`.
///
/// Cheap (uses the `idx_extraction_failures_unresolved` partial index).
/// Used by the CLI `--status` view and to populate
/// [`MigrationError::FailuresPresent`].
pub fn count_unresolved(conn: &Connection, namespace: &str) -> Result<u64, MigrationError> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_extraction_failures
             WHERE resolved_at IS NULL AND namespace = ?1",
            params![namespace],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    Ok(n.max(0) as u64)
}

/// Look up one failure row by id. Returns `None` if the row does not
/// exist. Used by [`retry_failed`] to re-read a row after the processor
/// commits its transaction (so the retry decision sees the post-processor
/// state).
pub fn get_failure(
    conn: &Connection,
    failure_id: Uuid,
) -> Result<Option<FailureRecord>, MigrationError> {
    conn.query_row(
        "SELECT id, episode_id, stage, error_category, error_detail,
                occurred_at, retry_count, resolved_at, namespace
         FROM graph_extraction_failures
         WHERE id = ?1",
        params![failure_id.as_bytes().to_vec()],
        row_to_failure_record,
    )
    .optional()
    .map_err(map_sqlite)
}

// ===========================================================================
// Retry driver — `engramai migrate --retry-failed`
// ===========================================================================

/// Re-process unresolved failure rows by handing each one's
/// [`MemoryRecord`] to a caller-supplied [`RecordProcessor`].
///
/// This is the library entry point for the `engramai migrate --retry-failed`
/// CLI subcommand (T14). The CLI's job is to (a) construct a real
/// `RecordProcessor` (T9's struct), (b) call this function, (c) print the
/// returned [`RetrySummary`]. All migration logic lives here, in the
/// library, per the project-wide "thin CLI over rust crate" rule.
///
/// ## Algorithm
///
/// For each unresolved row (oldest first, capped at `cfg.limit`):
///
/// 1. Skip if `retry_count >= cfg.max_retries` → bump
///    `summary.skipped_over_cap`.
/// 2. Recover `memory_id` from `error_detail`. If absent → bump
///    `summary.skipped_missing_record` and continue.
/// 3. Read the `memories` row by id. If absent (deleted between failure
///    and retry) → bump `summary.skipped_missing_record` and continue.
/// 4. Call `processor.process_one(conn, record)`. The processor is
///    responsible for opening its own transaction and atomically
///    applying any new graph deltas. Migration's checkpoint counters are
///    NOT touched here — `--retry-failed` is post-Phase-4 work and the
///    operator is reviewing failures, not advancing the overall
///    progress counter.
/// 5. On `Ok(RecordOutcome::Succeeded)` → open a brief transaction and
///    call [`mark_resolved`]; bump `summary.resolved`.
/// 6. On `Ok(RecordOutcome::Failed)` → open a brief transaction and call
///    [`bump_retry_count`]; bump `summary.still_failing`.
/// 7. On `Err(_)` from the processor → propagate. A processor-level `Err`
///    is fatal (matches §5.2 contract) — the operator must investigate
///    before re-attempting.
///
/// ## Why no outer transaction
///
/// The processor (T9) opens its own per-record transaction inside
/// `process_one`. Wrapping that in an outer tx would either (a) require
/// changing the trait signature to take `&Transaction` (breaks T8) or
/// (b) cause SQLite "cannot start a transaction within a transaction"
/// errors. The pragmatic split: processor commits its graph deltas in
/// its own tx; we then commit the failure-row state change in a separate,
/// short tx. The two-step nature is acceptable for `--retry-failed`
/// because (i) it runs after Phase 4 (no concurrent backfill writes),
/// (ii) `mark_resolved` is monotone-idempotent (a crash between the
/// processor commit and the resolve commit leaves the row "succeeded but
/// still flagged unresolved" — the next `--retry-failed` re-runs the
/// processor, which is idempotent on graph deltas, then retries the
/// resolve).
pub fn retry_failed<P>(
    conn: &mut Connection,
    processor: &P,
    cfg: &RetryConfig,
) -> Result<RetrySummary, MigrationError>
where
    P: RecordProcessor,
{
    let candidates = list_unresolved(conn, &cfg.namespace, cfg.limit)?;
    let mut summary = RetrySummary::default();

    for candidate in candidates {
        summary.considered += 1;

        if candidate.retry_count >= cfg.max_retries {
            summary.skipped_over_cap += 1;
            continue;
        }

        let Some(memory_id) = candidate.memory_id else {
            // Failure row was not written by migration — has no
            // recoverable memories.id pointer. Skip it (operator must
            // resolve manually).
            summary.skipped_missing_record += 1;
            continue;
        };

        let Some(record) = load_memory_record(conn, memory_id)? else {
            // memories row gone (manually deleted between Phase 4 and
            // --retry-failed). Skip; do not auto-resolve the failure
            // row — leaving it visible is the safer audit signal.
            summary.skipped_missing_record += 1;
            continue;
        };

        // Processor opens its own transaction internally (per the T8
        // RecordProcessor contract). We do not wrap it.
        let outcome = processor.process_one(conn, record)?;

        // Failure-row state change in a brief, separate transaction.
        let tx = conn.transaction().map_err(map_sqlite)?;
        match outcome {
            RecordOutcome::Succeeded { .. } => {
                let _ = mark_resolved(&tx, candidate.id, Utc::now())?;
                tx.commit().map_err(map_sqlite)?;
                summary.resolved += 1;
            }
            RecordOutcome::Failed { message, .. } => {
                bump_retry_count(
                    &tx,
                    candidate.id,
                    Utc::now(),
                    Some(&message),
                    memory_id,
                )?;
                tx.commit().map_err(map_sqlite)?;
                summary.still_failing += 1;
            }
        }
    }

    Ok(summary)
}

// ===========================================================================
// Internal helpers
// ===========================================================================

fn map_sqlite(e: rusqlite::Error) -> MigrationError {
    MigrationError::DdlFailed(e.to_string())
}

fn dt_to_unix(t: DateTime<Utc>) -> f64 {
    // Microsecond-precision unix seconds, matching graph-layer's storage.
    let secs = t.timestamp() as f64;
    let frac = t.timestamp_subsec_micros() as f64 / 1_000_000.0;
    secs + frac
}

fn row_to_failure_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<FailureRecord> {
    let id_bytes: Vec<u8> = row.get(0)?;
    let ep_bytes: Vec<u8> = row.get(1)?;
    let stage: String = row.get(2)?;
    let error_category: String = row.get(3)?;
    let error_detail: Option<String> = row.get(4)?;
    let occurred_at: f64 = row.get(5)?;
    let retry_count: i64 = row.get(6)?;
    let resolved_at: Option<f64> = row.get(7)?;
    let namespace: String = row.get(8)?;
    let memory_id = parse_memory_id_from_detail(error_detail.as_deref());

    Ok(FailureRecord {
        id: bytes_to_uuid(&id_bytes).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(e))
        })?,
        episode_id: bytes_to_uuid(&ep_bytes).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Blob, Box::new(e))
        })?,
        memory_id,
        stage,
        error_category,
        error_detail,
        occurred_at,
        retry_count,
        resolved_at,
        namespace,
    })
}

fn bytes_to_uuid(bytes: &[u8]) -> Result<Uuid, std::io::Error> {
    if bytes.len() != 16 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("uuid blob wrong length: {} (expected 16)", bytes.len()),
        ));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Ok(Uuid::from_bytes(arr))
}

fn load_memory_record(conn: &Connection, id: i64) -> Result<Option<MemoryRecord>, MigrationError> {
    conn.query_row(
        "SELECT id, content, metadata, created_at FROM memories WHERE id = ?1",
        params![id],
        |row| {
            Ok(MemoryRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                metadata: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(map_sqlite)
}

// Allow the test module access to the checkpoint store DDL / mem-init helpers.
#[allow(dead_code)]
fn _link_checkpoint_store(_c: &CheckpointStore) {
    // Intentional: keeps the `CheckpointStore` import live for tests that
    // exercise the `failure.rs` module against a fully-initialized DB.
    // No runtime effect.
}


// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rusqlite::Connection;

    /// Reproduce the relevant subset of v03-graph-layer's DDL so this
    /// crate can exercise its own writes without depending on `engramai`.
    /// Mirrors `engramai/src/graph/storage_graph.rs` lines 303-316 verbatim
    /// for `graph_extraction_failures` (one source of truth: the design
    /// schema; the constants below must move in lockstep with that file).
    const TEST_DDL: &str = r#"
        CREATE TABLE IF NOT EXISTS graph_extraction_failures (
            id              BLOB PRIMARY KEY,
            episode_id      BLOB NOT NULL,
            stage           TEXT NOT NULL,
            error_category  TEXT NOT NULL,
            error_detail    TEXT,
            occurred_at     REAL NOT NULL,
            retry_count     INTEGER NOT NULL DEFAULT 0,
            resolved_at     REAL,
            namespace       TEXT NOT NULL DEFAULT 'default'
        );
        CREATE INDEX IF NOT EXISTS idx_extraction_failures_episode
            ON graph_extraction_failures(episode_id);
        CREATE INDEX IF NOT EXISTS idx_extraction_failures_unresolved
            ON graph_extraction_failures(occurred_at) WHERE resolved_at IS NULL;

        CREATE TABLE IF NOT EXISTS memories (
            id          INTEGER PRIMARY KEY,
            content     TEXT NOT NULL,
            metadata    TEXT,
            created_at  TEXT NOT NULL
        );
    "#;

    fn fresh_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(TEST_DDL).unwrap();
        c
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn write_one(
        conn: &mut Connection,
        memory_id: i64,
        stage: &str,
        category: &str,
        message: &str,
    ) -> Uuid {
        let tx = conn.transaction().unwrap();
        let id = record_failure(
            &tx,
            &FailureWrite {
                memory_id,
                episode_id: None,
                stage,
                error_category: category,
                message,
                occurred_at: ts(1_700_000_000),
                namespace: DEFAULT_NAMESPACE,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        id
    }

    // ------------- closed-set validation ---------------------------------

    #[test]
    fn validate_stage_accepts_canonical_set() {
        for s in [
            STAGE_ENTITY_EXTRACT,
            STAGE_EDGE_EXTRACT,
            STAGE_DEDUP,
            STAGE_PERSIST,
            STAGE_KNOWLEDGE_COMPILE,
            STAGE_TOPIC_CARRY_FORWARD,
        ] {
            validate_stage(s).unwrap_or_else(|_| panic!("expected {s} to validate"));
        }
    }

    #[test]
    fn validate_stage_rejects_unknown() {
        let err = validate_stage("garbage").unwrap_err();
        match err {
            MigrationError::InvariantViolated(msg) => assert!(msg.contains("garbage")),
            other => panic!("expected InvariantViolated, got {other:?}"),
        }
    }

    #[test]
    fn validate_category_rejects_unknown() {
        let err = validate_error_category("not_a_category").unwrap_err();
        assert!(matches!(err, MigrationError::InvariantViolated(_)));
    }

    // ------------- id derivation -----------------------------------------

    #[test]
    fn derive_failure_episode_id_is_deterministic() {
        let a = derive_failure_episode_id(42);
        let b = derive_failure_episode_id(42);
        let c = derive_failure_episode_id(43);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn derive_failure_id_combines_episode_and_stage() {
        let ep = derive_failure_episode_id(7);
        let a = derive_failure_id(DEFAULT_NAMESPACE, ep, STAGE_ENTITY_EXTRACT);
        let b = derive_failure_id(DEFAULT_NAMESPACE, ep, STAGE_ENTITY_EXTRACT);
        let c = derive_failure_id(DEFAULT_NAMESPACE, ep, STAGE_EDGE_EXTRACT);
        let d = derive_failure_id("other", ep, STAGE_ENTITY_EXTRACT);
        assert_eq!(a, b, "same (namespace, episode, stage) → same id");
        assert_ne!(a, c, "different stage → different id");
        assert_ne!(a, d, "different namespace → different id");
    }

    // ------------- error_detail round-trip -------------------------------

    #[test]
    fn format_and_parse_error_detail_round_trip() {
        let formatted = format_error_detail(123, "llm timeout after 30s");
        assert!(formatted.starts_with("memory:123|"));
        let id = parse_memory_id_from_detail(Some(&formatted));
        assert_eq!(id, Some(123));
    }

    #[test]
    fn parse_memory_id_returns_none_for_unrelated_detail() {
        assert_eq!(parse_memory_id_from_detail(None), None);
        assert_eq!(parse_memory_id_from_detail(Some("nope")), None);
        assert_eq!(parse_memory_id_from_detail(Some("memory:abc|x")), None);
    }

    // ------------- record_failure: write + idempotence -------------------

    #[test]
    fn record_failure_writes_one_row() {
        let mut conn = fresh_db();
        write_one(
            &mut conn,
            10,
            STAGE_ENTITY_EXTRACT,
            CATEGORY_LLM_TIMEOUT,
            "boom",
        );
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_extraction_failures",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn record_failure_is_idempotent_on_replay() {
        let mut conn = fresh_db();
        let a = write_one(&mut conn, 10, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "1");
        let b = write_one(
            &mut conn,
            10,
            STAGE_ENTITY_EXTRACT,
            CATEGORY_LLM_TIMEOUT,
            "different message but same key",
        );
        assert_eq!(a, b, "same (memory_id, stage) keying → same id");
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_extraction_failures",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "second write was a no-op");
    }

    #[test]
    fn record_failure_separates_different_stages() {
        let mut conn = fresh_db();
        write_one(&mut conn, 10, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "x");
        write_one(&mut conn, 10, STAGE_EDGE_EXTRACT, CATEGORY_LLM_TIMEOUT, "y");
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_extraction_failures",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn record_failure_rejects_invalid_stage() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let err = record_failure(
            &tx,
            &FailureWrite {
                memory_id: 1,
                episode_id: None,
                stage: "fake",
                error_category: CATEGORY_INTERNAL,
                message: "x",
                occurred_at: ts(1),
                namespace: DEFAULT_NAMESPACE,
            },
        )
        .unwrap_err();
        assert!(matches!(err, MigrationError::InvariantViolated(_)));
    }

    // ------------- record_outcome_failure adapter ------------------------

    #[test]
    fn record_outcome_failure_skips_succeeded() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let res = record_outcome_failure(
            &tx,
            &RecordOutcome::Succeeded {
                entity_count: 0,
                edge_count: 0,
            },
            ts(1),
            DEFAULT_NAMESPACE,
        )
        .unwrap();
        assert_eq!(res, None);
        tx.commit().unwrap();
    }

    #[test]
    fn record_outcome_failure_writes_failed() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        let id = record_outcome_failure(
            &tx,
            &RecordOutcome::Failed {
                record_id: 99,
                kind: CATEGORY_LLM_TIMEOUT.into(),
                stage: STAGE_PERSIST.into(),
                episode_id: None,
                message: "timeout".into(),
            },
            ts(1),
            DEFAULT_NAMESPACE,
        )
        .unwrap();
        tx.commit().unwrap();
        assert!(id.is_some());
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_extraction_failures",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    // ------------- mark_resolved / bump_retry_count ----------------------

    #[test]
    fn mark_resolved_sets_timestamp_and_is_monotone() {
        let mut conn = fresh_db();
        let id = write_one(&mut conn, 1, STAGE_PERSIST, CATEGORY_DB_ERROR, "x");

        let tx = conn.transaction().unwrap();
        let changed = mark_resolved(&tx, id, ts(2_000_000_000)).unwrap();
        tx.commit().unwrap();
        assert!(changed);

        // Second call: row already resolved → must not flip back to NULL,
        // returns false (no change).
        let tx = conn.transaction().unwrap();
        let changed2 = mark_resolved(&tx, id, ts(2_000_000_001)).unwrap();
        tx.commit().unwrap();
        assert!(!changed2);

        // Resolved_at preserved (monotone).
        let resolved: Option<f64> = conn
            .query_row(
                "SELECT resolved_at FROM graph_extraction_failures WHERE id = ?1",
                params![id.as_bytes().to_vec()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(resolved, Some(2_000_000_000.0));
    }

    #[test]
    fn bump_retry_count_increments_and_refreshes_detail() {
        let mut conn = fresh_db();
        let id = write_one(&mut conn, 5, STAGE_PERSIST, CATEGORY_DB_ERROR, "first");

        let tx = conn.transaction().unwrap();
        bump_retry_count(&tx, id, ts(2_000_000_000), Some("second"), 5).unwrap();
        tx.commit().unwrap();

        let (rc, detail, occurred): (i64, String, f64) = conn
            .query_row(
                "SELECT retry_count, error_detail, occurred_at
                 FROM graph_extraction_failures WHERE id = ?1",
                params![id.as_bytes().to_vec()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(rc, 1);
        assert!(detail.starts_with("memory:5|"));
        assert!(detail.contains("second"));
        assert_eq!(occurred, 2_000_000_000.0);
    }

    // ------------- list_unresolved / count_unresolved --------------------

    #[test]
    fn list_unresolved_orders_by_occurred_at() {
        let mut conn = fresh_db();
        // Different memory_ids so each gets its own row.
        let tx = conn.transaction().unwrap();
        record_failure(
            &tx,
            &FailureWrite {
                memory_id: 1,
                episode_id: None,
                stage: STAGE_PERSIST,
                error_category: CATEGORY_DB_ERROR,
                message: "newer",
                occurred_at: ts(2000),
                namespace: DEFAULT_NAMESPACE,
            },
        )
        .unwrap();
        record_failure(
            &tx,
            &FailureWrite {
                memory_id: 2,
                episode_id: None,
                stage: STAGE_PERSIST,
                error_category: CATEGORY_DB_ERROR,
                message: "older",
                occurred_at: ts(1000),
                namespace: DEFAULT_NAMESPACE,
            },
        )
        .unwrap();
        tx.commit().unwrap();

        let rows = list_unresolved(&conn, DEFAULT_NAMESPACE, None).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].occurred_at < rows[1].occurred_at);
        assert_eq!(rows[0].memory_id, Some(2)); // older first
        assert_eq!(rows[1].memory_id, Some(1));
    }

    #[test]
    fn count_unresolved_excludes_resolved() {
        let mut conn = fresh_db();
        let id_a = write_one(&mut conn, 1, STAGE_PERSIST, CATEGORY_DB_ERROR, "a");
        let _id_b = write_one(&mut conn, 2, STAGE_PERSIST, CATEGORY_DB_ERROR, "b");
        assert_eq!(count_unresolved(&conn, DEFAULT_NAMESPACE).unwrap(), 2);

        let tx = conn.transaction().unwrap();
        mark_resolved(&tx, id_a, ts(9999)).unwrap();
        tx.commit().unwrap();
        assert_eq!(count_unresolved(&conn, DEFAULT_NAMESPACE).unwrap(), 1);
    }

    #[test]
    fn list_unresolved_namespace_isolation() {
        let mut conn = fresh_db();
        let tx = conn.transaction().unwrap();
        record_failure(
            &tx,
            &FailureWrite {
                memory_id: 1,
                episode_id: None,
                stage: STAGE_PERSIST,
                error_category: CATEGORY_DB_ERROR,
                message: "ns1",
                occurred_at: ts(1),
                namespace: "ns1",
            },
        )
        .unwrap();
        record_failure(
            &tx,
            &FailureWrite {
                memory_id: 1,
                episode_id: None,
                stage: STAGE_PERSIST,
                error_category: CATEGORY_DB_ERROR,
                message: "ns2",
                occurred_at: ts(2),
                namespace: "ns2",
            },
        )
        .unwrap();
        tx.commit().unwrap();

        assert_eq!(list_unresolved(&conn, "ns1", None).unwrap().len(), 1);
        assert_eq!(list_unresolved(&conn, "ns2", None).unwrap().len(), 1);
        assert_eq!(list_unresolved(&conn, DEFAULT_NAMESPACE, None).unwrap().len(), 0);
    }

    // ------------- retry_failed driver -----------------------------------

    /// Test processor that records which memory_ids it saw and lets each
    /// test script the success/failure outcome by id.
    struct ScriptedProcessor {
        succeed_for: std::collections::HashSet<i64>,
        seen: std::cell::RefCell<Vec<i64>>,
    }

    impl ScriptedProcessor {
        fn new(succeed_for: &[i64]) -> Self {
            Self {
                succeed_for: succeed_for.iter().copied().collect(),
                seen: Default::default(),
            }
        }
    }

    impl RecordProcessor for ScriptedProcessor {
        fn process_one(
            &self,
            _conn: &mut Connection,
            record: MemoryRecord,
        ) -> Result<RecordOutcome, MigrationError> {
            self.seen.borrow_mut().push(record.id);
            if self.succeed_for.contains(&record.id) {
                Ok(RecordOutcome::Succeeded {
                    entity_count: 1,
                    edge_count: 0,
                })
            } else {
                Ok(RecordOutcome::Failed {
                    record_id: record.id,
                    kind: CATEGORY_LLM_TIMEOUT.into(),
                    stage: STAGE_ENTITY_EXTRACT.into(),
                    episode_id: None,
                    message: "still failing".into(),
                })
            }
        }
    }

    fn seed_memory(conn: &Connection, id: i64) {
        conn.execute(
            "INSERT INTO memories (id, content, metadata, created_at)
             VALUES (?1, ?2, NULL, ?3)",
            params![id, format!("content-{id}"), "2026-01-01T00:00:00Z"],
        )
        .unwrap();
    }

    #[test]
    fn retry_failed_resolves_succeeded_rows_and_bumps_failed() {
        let mut conn = fresh_db();
        seed_memory(&conn, 1);
        seed_memory(&conn, 2);
        write_one(&mut conn, 1, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "old");
        write_one(&mut conn, 2, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "old");

        let processor = ScriptedProcessor::new(&[1]); // 1 succeeds, 2 fails
        let summary = retry_failed(&mut conn, &processor, &RetryConfig::default()).unwrap();

        assert_eq!(summary.considered, 2);
        assert_eq!(summary.resolved, 1);
        assert_eq!(summary.still_failing, 1);
        assert_eq!(summary.skipped_over_cap, 0);
        assert_eq!(summary.skipped_missing_record, 0);

        // Succeeded row → resolved_at set
        assert_eq!(count_unresolved(&conn, DEFAULT_NAMESPACE).unwrap(), 1);
        // Failed row → retry_count == 1
        let rc: i64 = conn
            .query_row(
                "SELECT retry_count FROM graph_extraction_failures
                 WHERE error_detail LIKE 'memory:2|%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rc, 1);
    }

    #[test]
    fn retry_failed_respects_max_retries_cap() {
        let mut conn = fresh_db();
        seed_memory(&conn, 1);
        let id = write_one(&mut conn, 1, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "x");
        // Hit the cap manually.
        conn.execute(
            "UPDATE graph_extraction_failures SET retry_count = 5 WHERE id = ?1",
            params![id.as_bytes().to_vec()],
        )
        .unwrap();

        let processor = ScriptedProcessor::new(&[1]); // would succeed if attempted
        let cfg = RetryConfig {
            max_retries: 5,
            ..Default::default()
        };
        let summary = retry_failed(&mut conn, &processor, &cfg).unwrap();

        assert_eq!(summary.considered, 1);
        assert_eq!(summary.skipped_over_cap, 1);
        assert_eq!(summary.resolved, 0);
        assert!(processor.seen.borrow().is_empty(), "processor was not called");
    }

    #[test]
    fn retry_failed_skips_when_memory_row_missing() {
        let mut conn = fresh_db();
        // No `memories` row for id=42 — write only the failure row.
        write_one(&mut conn, 42, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "x");

        let processor = ScriptedProcessor::new(&[42]);
        let summary = retry_failed(&mut conn, &processor, &RetryConfig::default()).unwrap();

        assert_eq!(summary.considered, 1);
        assert_eq!(summary.skipped_missing_record, 1);
        assert_eq!(summary.resolved, 0);
        // Failure row remains unresolved (operator-visible).
        assert_eq!(count_unresolved(&conn, DEFAULT_NAMESPACE).unwrap(), 1);
    }

    #[test]
    fn retry_failed_idempotent_on_repeat_when_all_resolved() {
        let mut conn = fresh_db();
        seed_memory(&conn, 1);
        write_one(&mut conn, 1, STAGE_ENTITY_EXTRACT, CATEGORY_LLM_TIMEOUT, "x");

        let processor = ScriptedProcessor::new(&[1]);
        let s1 = retry_failed(&mut conn, &processor, &RetryConfig::default()).unwrap();
        assert_eq!(s1.resolved, 1);

        // Second invocation: nothing unresolved → considered = 0.
        let s2 = retry_failed(&mut conn, &processor, &RetryConfig::default()).unwrap();
        assert_eq!(s2.considered, 0);
        assert_eq!(s2.resolved, 0);
    }

    #[test]
    fn retry_failed_limit_caps_iteration() {
        let mut conn = fresh_db();
        seed_memory(&conn, 1);
        seed_memory(&conn, 2);
        seed_memory(&conn, 3);
        // Distinct timestamps so ORDER BY is well-defined.
        let tx = conn.transaction().unwrap();
        for (mid, t) in [(1i64, 100i64), (2, 200), (3, 300)] {
            record_failure(
                &tx,
                &FailureWrite {
                    memory_id: mid,
                    episode_id: None,
                    stage: STAGE_ENTITY_EXTRACT,
                    error_category: CATEGORY_LLM_TIMEOUT,
                    message: "x",
                    occurred_at: ts(t),
                    namespace: DEFAULT_NAMESPACE,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let processor = ScriptedProcessor::new(&[1, 2, 3]);
        let cfg = RetryConfig {
            limit: Some(2),
            ..Default::default()
        };
        let summary = retry_failed(&mut conn, &processor, &cfg).unwrap();
        assert_eq!(summary.considered, 2);
        // Oldest two (1, 2) processed; 3 left for next call.
        assert_eq!(processor.seen.borrow().clone(), vec![1, 2]);
    }
}
