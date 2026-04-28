//! T9 — Per-record [`RecordProcessor`] implementation that wires the v0.2
//! migration cursor (T8) into the v0.3 resolution pipeline.
//!
//! ## Position in the migration call graph
//!
//! ```text
//! BackfillOrchestrator (T8)               this module (T9)
//! ─────────────────────────────           ─────────────────────────────
//!  fetch_page → MemoryRecord ──── process_one ───► resolve_for_backfill
//!                                                     │ (v03-resolution §6.5)
//!                                                     ▼
//!                                                  GraphDelta
//!                                                     │
//!                                                     ▼
//!                                              apply_graph_delta
//!                                              (v03-graph-layer §5)
//!                                                     │
//!                                                     ▼
//!                                              update_backfill_progress
//!                                              (checkpoint, T7)
//! ```
//!
//! ## Design references
//!
//! - v03-migration §5.2 ("Per-record pipeline invocation") — the contract.
//! - v03-migration §5.3 ("Failure surfacing & retry") — failure model.
//! - v03-resolution §6.5 ("Migration backfill entry point") — the upstream API.
//! - v03-graph-layer §5bis (`apply_graph_delta`) — the persist API.
//!
//! ## Atomicity model — observation, not deviation
//!
//! v03-migration §5.2 reads "single SQLite transaction wraps apply +
//! checkpoint advance." `GraphStore::apply_graph_delta` (v03-graph-layer
//! §5bis) however opens **its own** transaction internally and commits
//! before returning. Wrapping it in an outer transaction would require
//! refactoring `apply_graph_delta` to accept a borrowed `&Transaction` —
//! out of T9 scope.
//!
//! The pragmatic compromise:
//!
//! 1. Call `apply_graph_delta(&delta)`. It opens, writes, and commits
//!    its own tx. After this returns, the graph state is durable AND
//!    idempotent (the `graph_applied_deltas` row exists, keyed by
//!    `(memory_id, delta_hash)`).
//! 2. Open a small follow-up tx and call
//!    [`CheckpointStore::update_backfill_progress`]. This advances the
//!    checkpoint counters and `last_processed_memory_id`.
//!
//! Crash window: between (1) commit and (2) commit. On resume:
//! - The cursor reads `last_processed_memory_id` (still the *previous*
//!   record), so the same record is re-fetched.
//! - `apply_graph_delta` finds the matching `graph_applied_deltas` row
//!   and returns `ApplyReport::already_applied_marker()` — no new writes,
//!   no double-counting at the graph layer.
//! - The checkpoint counters then advance correctly on this retry.
//!
//! Net behavior: at-least-once apply, exactly-once durable effect, monotone
//! checkpoint counters. The "advance-after-commit" property in §5.2 is
//! preserved — the checkpoint never advances past a record whose graph
//! writes did not commit. The only weaker property vs. the design's
//! literal wording is "single tx", which the idempotence row redeems.
//!
//! This is an architectural observation worth filing as a follow-up
//! (refactor `apply_graph_delta` to take an external `&Transaction`),
//! but T9 does not need it to satisfy GOAL-4.1 / GOAL-4.4.
//!
//! ## Pipeline-store vs migration-conn
//!
//! [`ResolutionPipeline`](engramai::resolution::ResolutionPipeline) holds
//! its own `Arc<Mutex<dyn GraphStore>>` for in-pipeline reads
//! (`search_candidates`, `find_edges`). For backfill we expect that store
//! to point at the **same SQLite database** the orchestrator is migrating,
//! so candidate lookups see entities that prior records in the same run
//! have already inserted. Wiring the two is the caller's responsibility
//! (the CLI binding in T11/T12) — T9 does not enforce a particular
//! topology, only that:
//!
//! - the pipeline's store is consistent with the migration conn (e.g. both
//!   wrap connections to the same DB file, or share one connection
//!   threaded through `Arc<Mutex>`),
//! - the migration conn passed to [`PipelineRecordProcessor::process_one`]
//!   is the one that owns the per-record write transaction.
//!
//! ## v0.2 → engramai MemoryRecord conversion
//!
//! [`backfill::MemoryRecord`](crate::backfill::MemoryRecord) is the narrow
//! v0.2 row read by T8. The pipeline expects the richer
//! [`engramai::types::MemoryRecord`]. Conversion rules ([`to_engramai_record`]):
//!
//! - `id` → `format!("{}", row.id)` (v0.2 source schemas use either INTEGER
//!   or TEXT primary keys; the cast yields a stable string in both cases).
//! - `memory_type` → parsed from `metadata` JSON `"memory_type"` field if
//!   present, else `MemoryType::Factual` (the v0.2 default).
//! - `layer` → `MemoryLayer::Archive`. Backfilled rows are historic by
//!   definition; the resolution pipeline does not branch on layer for
//!   `resolve_for_backfill`, so the choice is informational.
//! - `created_at` → parsed RFC3339; if unparseable, treated as a corrupt
//!   source row and surfaced as `RecordOutcome::Failed{kind=internal,
//!   stage=entity_extract}` rather than a fatal error (per §5.3 "per-record
//!   failures are data, not control flow").
//! - All cognitive fields (`working_strength`, `core_strength`,
//!   `importance`, etc.) → defaults from
//!   [`engramai::types::MemoryType::default_importance`]. The pipeline
//!   does not consult these for resolution.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use engramai::graph::store::GraphWrite;
use engramai::graph::{ApplyReport, GraphDelta, GraphStore, StageFailureRow};
use engramai::resolution::pipeline::{PipelineError, ResolutionPipeline};
use engramai::types::{MemoryLayer, MemoryRecord as EngramaiMemoryRecord, MemoryType};

use crate::backfill::{MemoryRecord, RecordOutcome, RecordProcessor};
use crate::checkpoint::CheckpointStore;
use crate::error::MigrationError;
use crate::failure::{
    CATEGORY_INTERNAL, STAGE_DEDUP, STAGE_EDGE_EXTRACT, STAGE_ENTITY_EXTRACT, STAGE_PERSIST,
};

// ---------------------------------------------------------------------------
// BackfillResolver — testability seam
// ---------------------------------------------------------------------------

/// Narrow seam over [`ResolutionPipeline::resolve_for_backfill`] so the
/// processor's high-level flow (conversion, failure routing, checkpoint
/// advance) can be unit-tested without spinning up a real pipeline.
///
/// The blanket impl below covers `Arc<ResolutionPipeline<S>>` for production
/// callers. Tests substitute a hand-rolled `impl BackfillResolver` that
/// returns scripted `GraphDelta` / `PipelineError` values.
pub trait BackfillResolver: Send + Sync {
    fn resolve_for_backfill(
        &self,
        memory: &EngramaiMemoryRecord,
    ) -> Result<GraphDelta, PipelineError>;
}

impl<S> BackfillResolver for Arc<ResolutionPipeline<S>>
where
    S: GraphStore + Send + ?Sized + 'static,
{
    fn resolve_for_backfill(
        &self,
        memory: &EngramaiMemoryRecord,
    ) -> Result<GraphDelta, PipelineError> {
        ResolutionPipeline::resolve_for_backfill(self.as_ref(), memory)
    }
}

// ---------------------------------------------------------------------------
// PipelineRecordProcessor
// ---------------------------------------------------------------------------

/// Wires T8's [`RecordProcessor`] seam to the v03-resolution pipeline.
///
/// Cheap to clone (all fields are `Arc` or shared handles); intended to be
/// constructed once per migration run by the CLI layer (T11/T12) and
/// passed by reference to [`crate::BackfillOrchestrator::run`].
pub struct PipelineRecordProcessor {
    resolver: Arc<dyn BackfillResolver>,
    /// Namespace tag passed to `record_failure` rows. Independent of the
    /// pipeline's own namespace because failure-row writes go through
    /// the migration conn, not the pipeline store.
    namespace: String,
}

impl PipelineRecordProcessor {
    /// Construct a processor wrapping any [`BackfillResolver`]. Production
    /// callers pass `Arc<ResolutionPipeline<S>>` (which has a blanket
    /// `BackfillResolver` impl); tests pass a scripted fake.
    pub fn new(resolver: Arc<dyn BackfillResolver>) -> Self {
        Self {
            resolver,
            namespace: String::new(),
        }
    }

    /// Override the namespace tag written to `graph_extraction_failures`.
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self
    }
}

impl RecordProcessor for PipelineRecordProcessor {
    fn process_one(
        &self,
        conn: &mut Connection,
        record: MemoryRecord,
    ) -> Result<RecordOutcome, MigrationError> {
        let record_id = record.id;
        let now = Utc::now();

        // ---- Convert v0.2 row → engramai MemoryRecord ----
        // A parse failure here is per-record data corruption, not fatal.
        // Surface as RecordOutcome::Failed with stage=entity_extract
        // (the earliest pipeline stage that would have observed it).
        let engramai_rec = match to_engramai_record(&record) {
            Ok(r) => r,
            Err(parse_err) => {
                // Advance checkpoint as failed; emit failure row.
                advance_checkpoint_as_failed(
                    conn,
                    record_id,
                    STAGE_ENTITY_EXTRACT,
                    CATEGORY_INTERNAL,
                    &parse_err,
                    now,
                    &self.namespace,
                )?;
                return Ok(RecordOutcome::Failed {
                    record_id,
                    kind: CATEGORY_INTERNAL.to_string(),
                    stage: STAGE_ENTITY_EXTRACT.to_string(),
                    episode_id: None,
                    message: parse_err,
                });
            }
        };

        // ---- Run the resolution pipeline ----
        // resolve_for_backfill never invokes apply_graph_delta itself
        // (per v03-resolution §6.5) — we own the persist step below.
        let delta = match self.resolver.resolve_for_backfill(&engramai_rec) {
            Ok(d) => d,
            Err(PipelineError::ExtractionFailure(msg)) => {
                // Per design §5.3: surface as per-record failure.
                // The pipeline does not currently produce this variant,
                // but the contract permits it (forward-compat).
                advance_checkpoint_as_failed(
                    conn,
                    record_id,
                    STAGE_DEDUP,
                    CATEGORY_INTERNAL,
                    &msg,
                    now,
                    &self.namespace,
                )?;
                return Ok(RecordOutcome::Failed {
                    record_id,
                    kind: CATEGORY_INTERNAL.to_string(),
                    stage: STAGE_DEDUP.to_string(),
                    episode_id: None,
                    message: msg,
                });
            }
            Err(PipelineError::Fatal(msg)) => {
                // Storage-level error → abort backfill, preserve checkpoint.
                return Err(MigrationError::BackfillFatal(format!(
                    "resolve_for_backfill record_id={record_id}: {msg}"
                )));
            }
        };

        // ---- Persist delta atomically (own internal tx) ----
        let report = apply_delta_through_migration_conn(conn, &delta)?;

        // ---- Inspect stage_failures: they may exist even with Ok(delta) ----
        let had_stage_failures = !delta.stage_failures.is_empty();

        // ---- Advance checkpoint counters ----
        let (delta_succeeded, delta_failed) = if had_stage_failures { (0, 1) } else { (1, 0) };
        CheckpointStore::update_backfill_progress(
            conn,
            record_id,
            1,
            delta_succeeded,
            delta_failed,
            &now.to_rfc3339(),
        )?;

        // ---- Build outcome ----
        if had_stage_failures {
            let first = &delta.stage_failures[0];
            Ok(RecordOutcome::Failed {
                record_id,
                kind: classify_stage_failure_kind(first).to_string(),
                stage: classify_stage_failure_stage(first).to_string(),
                episode_id: None,
                message: format!(
                    "{} stage_failures persisted to graph_extraction_failures",
                    delta.stage_failures.len()
                ),
            })
        } else {
            let _ = report; // ApplyReport currently only used for forward telemetry.
            Ok(RecordOutcome::Succeeded {
                entity_count: delta.entities.len() as u32,
                edge_count: delta.edges.len() as u32,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a v0.2 cursor row into the engramai pipeline's [`MemoryRecord`].
///
/// Returns `Err(human_readable_message)` on per-record data parse failure
/// (not fatal — caller surfaces it as a `RecordOutcome::Failed`).
fn to_engramai_record(rec: &MemoryRecord) -> Result<EngramaiMemoryRecord, String> {
    // Parse created_at. v0.2 rows store RFC3339 strings.
    let created_at = DateTime::parse_from_rfc3339(&rec.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("created_at not RFC3339 ({e}): {:?}", rec.created_at))?;

    // Optional metadata extraction.
    let (memory_type, importance, metadata_json) = match rec.metadata.as_deref() {
        None | Some("") => (MemoryType::Factual, 0.5_f64, None),
        Some(raw) => parse_metadata(raw),
    };

    Ok(EngramaiMemoryRecord {
        id: format!("{}", rec.id),
        content: rec.content.clone(),
        memory_type,
        layer: MemoryLayer::Archive,
        created_at,
        access_times: vec![created_at],
        working_strength: 0.5,
        core_strength: 0.5,
        importance,
        pinned: false,
        consolidation_count: 0,
        last_consolidated: None,
        source: "v02-backfill".to_string(),
        contradicts: None,
        contradicted_by: None,
        superseded_by: None,
        metadata: metadata_json,
    })
}

/// Pull `memory_type` and `importance` out of a metadata JSON blob,
/// tolerating malformed input. Falls back to (`Factual`, 0.5, raw).
fn parse_metadata(raw: &str) -> (MemoryType, f64, Option<serde_json::Value>) {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        // Unparseable metadata is not fatal — record still ingests with
        // defaults; the malformed string is dropped rather than halting
        // the migration. Operators see records_succeeded but no metadata.
        Err(_) => return (MemoryType::Factual, 0.5, None),
    };

    let memory_type = value
        .get("memory_type")
        .and_then(|v| v.as_str())
        .and_then(|s| match s.to_ascii_lowercase().as_str() {
            "factual" => Some(MemoryType::Factual),
            "episodic" => Some(MemoryType::Episodic),
            "relational" => Some(MemoryType::Relational),
            "emotional" => Some(MemoryType::Emotional),
            "procedural" => Some(MemoryType::Procedural),
            "opinion" => Some(MemoryType::Opinion),
            "causal" => Some(MemoryType::Causal),
            _ => None,
        })
        .unwrap_or(MemoryType::Factual);

    let importance = value
        .get("importance")
        .and_then(|v| v.as_f64())
        .filter(|f| f.is_finite() && (0.0..=1.0).contains(f))
        .unwrap_or(0.5);

    (memory_type, importance, Some(value))
}

/// Apply `delta` through the migration's connection. Wraps the call so the
/// processor body stays focused on the high-level flow and the borrow
/// dance is contained.
///
/// This temporarily constructs a `SqliteGraphStore` over the migration
/// `conn` for the duration of the call. Any pipeline-internal
/// `GraphStore` (used during `resolve_for_backfill` for candidate reads)
/// must point at the *same* database — see module-level docs.
fn apply_delta_through_migration_conn(
    conn: &mut Connection,
    delta: &GraphDelta,
) -> Result<ApplyReport, MigrationError> {
    // The `GraphWrite` trait import above brings `apply_graph_delta` into
    // scope (the method lives on `GraphWrite`, not on the struct itself).
    let mut store = engramai::graph::SqliteGraphStore::new(conn);
    store
        .apply_graph_delta(delta)
        .map_err(|e| MigrationError::BackfillFatal(format!("apply_graph_delta: {e}")))
}

/// Helper used when the pipeline never produced a delta (parse failure or
/// `PipelineError::ExtractionFailure`): write a single failure row and
/// advance the checkpoint with `delta_failed=1`. Both happen in their
/// own small transactions (see module-level "Atomicity model").
fn advance_checkpoint_as_failed(
    conn: &mut Connection,
    record_id: i64,
    stage: &str,
    kind: &str,
    message: &str,
    now: DateTime<Utc>,
    namespace: &str,
) -> Result<(), MigrationError> {
    // ---- Failure row ----
    {
        let tx = conn
            .transaction()
            .map_err(|e| MigrationError::BackfillFatal(format!("open failure tx: {e}")))?;
        let write = crate::failure::FailureWrite {
            memory_id: record_id,
            episode_id: None,
            stage,
            error_category: kind,
            message,
            occurred_at: now,
            namespace,
        };
        crate::failure::record_failure(&tx, &write)?;
        tx.commit()
            .map_err(|e| MigrationError::BackfillFatal(format!("commit failure tx: {e}")))?;
    }

    // ---- Checkpoint advance ----
    CheckpointStore::update_backfill_progress(conn, record_id, 1, 0, 1, &now.to_rfc3339())?;
    Ok(())
}

/// Map a [`StageFailureRow`] stage tag into the migration's `STAGE_*`
/// constants. The graph-layer `stage` is a free-form string set by
/// v03-resolution; in practice one of "entity_extract", "edge_extract",
/// "dedup", "persist". Pass through unchanged when it matches; otherwise
/// default to `dedup` so the row survives `validate_stage`.
fn classify_stage_failure_stage(f: &StageFailureRow) -> &'static str {
    match f.stage.as_str() {
        "entity_extract" => STAGE_ENTITY_EXTRACT,
        "edge_extract" => STAGE_EDGE_EXTRACT,
        "dedup" => STAGE_DEDUP,
        "persist" => STAGE_PERSIST,
        _ => STAGE_DEDUP,
    }
}

/// Map a [`StageFailureRow`] category into the migration's `CATEGORY_*`
/// constants. The graph-layer carries `error_category` as a free-form
/// string; we accept the well-known v03-resolution categories verbatim
/// and default to `internal` for unknown values.
fn classify_stage_failure_kind(f: &StageFailureRow) -> &'static str {
    match f.error_category.as_str() {
        "llm_timeout" => crate::failure::CATEGORY_LLM_TIMEOUT,
        "llm_invalid_output" => crate::failure::CATEGORY_LLM_INVALID_OUTPUT,
        "budget_exhausted" => crate::failure::CATEGORY_BUDGET_EXHAUSTED,
        "db_error" => crate::failure::CATEGORY_DB_ERROR,
        _ => CATEGORY_INTERNAL,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backfill::MemoryRecord as RawRecord;
    use crate::checkpoint::{CheckpointStore, CHECKPOINT_DDL};
    use crate::failure::list_unresolved;
    use chrono::TimeZone;
    use engramai::graph::storage_graph::init_graph_tables;
    use engramai::graph::GraphDelta;
    use std::sync::Mutex as StdMutex;
    use uuid::Uuid;

    // ---- Test fixtures ----------------------------------------------------

    /// Scripted resolver: replays a queue of pre-built results.
    /// Each call to `resolve_for_backfill` consumes one element.
    struct ScriptedResolver {
        queue: StdMutex<std::collections::VecDeque<Result<GraphDelta, PipelineError>>>,
    }

    impl ScriptedResolver {
        fn new(items: Vec<Result<GraphDelta, PipelineError>>) -> Arc<Self> {
            Arc::new(Self {
                queue: StdMutex::new(items.into_iter().collect()),
            })
        }
    }

    impl BackfillResolver for ScriptedResolver {
        fn resolve_for_backfill(
            &self,
            _memory: &EngramaiMemoryRecord,
        ) -> Result<GraphDelta, PipelineError> {
            self.queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("ScriptedResolver: more calls than scripted results")
        }
    }

    /// Build a connection with the schema T9 needs:
    /// - `memories` table (so graph_entities ALTER targets exist)
    /// - graph layer tables (graph_entities, graph_edges, ...)
    /// - migration_state singleton
    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();

        // Minimal v0.2 memories table — matches T8's fetch_page schema.
        conn.execute_batch(
            "CREATE TABLE memories (
                id INTEGER PRIMARY KEY,
                content TEXT NOT NULL,
                metadata TEXT,
                created_at TEXT NOT NULL,
                entity_ids TEXT NOT NULL DEFAULT '[]',
                edge_ids TEXT NOT NULL DEFAULT '[]'
            );",
        )
        .unwrap();

        init_graph_tables(&conn).expect("graph schema init");
        conn.execute_batch(CHECKPOINT_DDL).unwrap();

        // Insert the singleton migration_state row that
        // update_backfill_progress expects.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO migration_state \
             (id, current_phase, last_processed_memory_id, started_at, updated_at) \
             VALUES (1, 'Phase4', -1, ?1, ?1)",
            rusqlite::params![now],
        )
        .unwrap();
        conn
    }

    fn fixture_record(id: i64) -> RawRecord {
        RawRecord {
            id,
            content: format!("memory {id} content"),
            metadata: None,
            created_at: Utc.with_ymd_and_hms(2026, 4, 26, 12, 0, 0).unwrap().to_rfc3339(),
        }
    }

    // ---- Tests ------------------------------------------------------------

    #[test]
    fn to_engramai_record_happy_path() {
        let raw = RawRecord {
            id: 42,
            content: "hello".into(),
            metadata: Some(r#"{"memory_type":"episodic","importance":0.7}"#.into()),
            created_at: "2026-04-26T12:00:00Z".into(),
        };
        let r = to_engramai_record(&raw).unwrap();
        assert_eq!(r.id, "42");
        assert_eq!(r.content, "hello");
        assert!(matches!(r.memory_type, MemoryType::Episodic));
        assert!((r.importance - 0.7).abs() < 1e-9);
        assert!(matches!(r.layer, MemoryLayer::Archive));
    }

    #[test]
    fn to_engramai_record_defaults_on_no_metadata() {
        let raw = fixture_record(1);
        let r = to_engramai_record(&raw).unwrap();
        assert!(matches!(r.memory_type, MemoryType::Factual));
        assert!((r.importance - 0.5).abs() < 1e-9);
    }

    #[test]
    fn to_engramai_record_tolerates_malformed_metadata() {
        // Malformed JSON → defaults applied, no error.
        let raw = RawRecord {
            id: 7,
            content: "x".into(),
            metadata: Some("{not valid json".into()),
            created_at: "2026-04-26T12:00:00Z".into(),
        };
        let r = to_engramai_record(&raw).unwrap();
        assert!(matches!(r.memory_type, MemoryType::Factual));
        assert!(r.metadata.is_none());
    }

    #[test]
    fn to_engramai_record_fails_on_bad_timestamp() {
        let raw = RawRecord {
            id: 9,
            content: "x".into(),
            metadata: None,
            created_at: "not-a-timestamp".into(),
        };
        let err = to_engramai_record(&raw).unwrap_err();
        assert!(err.contains("not RFC3339"), "msg: {err}");
    }

    #[test]
    fn process_one_succeeded_with_empty_delta() {
        // Pipeline returns Ok(empty delta). Apply succeeds (no work),
        // checkpoint advances with succeeded=1, outcome is Succeeded.
        let mut conn = fresh_db();
        let memory_uuid = Uuid::nil();
        let delta = GraphDelta::new(memory_uuid);
        let resolver = ScriptedResolver::new(vec![Ok(delta)]);
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>);

        let outcome = proc.process_one(&mut conn, fixture_record(101)).unwrap();
        match outcome {
            RecordOutcome::Succeeded { entity_count, edge_count } => {
                assert_eq!(entity_count, 0);
                assert_eq!(edge_count, 0);
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_processed, 1);
        assert_eq!(state.records_succeeded, 1);
        assert_eq!(state.records_failed, 0);
        assert_eq!(state.last_processed_memory_id, 101);
    }

    #[test]
    fn process_one_records_failed_when_stage_failures_present() {
        // Pipeline returns Ok(delta) but with stage_failures populated.
        // Per design §5.3, this counts as a per-record failure at the
        // migration layer even though graph state still committed.
        let mut conn = fresh_db();
        let mut delta = GraphDelta::new(Uuid::nil());
        delta.stage_failures.push(StageFailureRow {
            episode_id: Uuid::nil(),
            stage: "entity_extract".into(),
            error_category: "llm_timeout".into(),
            error_detail: "model didn't respond".into(),
            occurred_at: 1714158000.0,
        });
        let resolver = ScriptedResolver::new(vec![Ok(delta)]);
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>)
            .with_namespace("default");

        let outcome = proc.process_one(&mut conn, fixture_record(202)).unwrap();
        match outcome {
            RecordOutcome::Failed { record_id, kind, stage, .. } => {
                assert_eq!(record_id, 202);
                assert_eq!(kind, crate::failure::CATEGORY_LLM_TIMEOUT);
                assert_eq!(stage, STAGE_ENTITY_EXTRACT);
            }
            other => panic!("expected Failed, got {other:?}"),
        }

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_processed, 1);
        assert_eq!(state.records_succeeded, 0);
        assert_eq!(state.records_failed, 1);
        assert_eq!(state.last_processed_memory_id, 202);

        // graph_applied_deltas should have the row (delta did commit).
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_applied_deltas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 1, "delta should still be persisted via apply_graph_delta");

        // graph_extraction_failures should hold the stage_failure row.
        let failed_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_extraction_failures", [], |r| r.get(0))
            .unwrap();
        assert!(failed_rows >= 1, "stage_failure must be persisted");
    }

    #[test]
    fn process_one_parse_failure_surfaces_as_failed_outcome() {
        // Bad created_at → conversion fails → Failed outcome, failure row,
        // checkpoint advanced with delta_failed=1, NOT a fatal error.
        let mut conn = fresh_db();
        let resolver = ScriptedResolver::new(vec![]); // never called
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>)
            .with_namespace("default");

        let bad = RawRecord {
            id: 303,
            content: "x".into(),
            metadata: None,
            created_at: "garbage-timestamp".into(),
        };
        let outcome = proc.process_one(&mut conn, bad).unwrap();
        assert!(matches!(outcome, RecordOutcome::Failed { record_id: 303, .. }));

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_failed, 1);
        assert_eq!(state.last_processed_memory_id, 303);

        // Failure row should be present in graph_extraction_failures.
        let unresolved = list_unresolved(&conn, "default", Some(100)).unwrap();
        assert_eq!(unresolved.len(), 1, "expected one unresolved failure row");
    }

    #[test]
    fn process_one_fatal_pipeline_error_aborts_run() {
        // PipelineError::Fatal → MigrationError::BackfillFatal.
        // Checkpoint must NOT advance (caller preserves it).
        let mut conn = fresh_db();
        let resolver = ScriptedResolver::new(vec![Err(PipelineError::Fatal(
            "disk full".into(),
        ))]);
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>);

        let err = proc.process_one(&mut conn, fixture_record(404)).unwrap_err();
        match err {
            MigrationError::BackfillFatal(msg) => {
                assert!(msg.contains("disk full"), "msg: {msg}");
                assert!(msg.contains("404"), "fatal msg should reference record id: {msg}");
            }
            other => panic!("expected BackfillFatal, got {other:?}"),
        }

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_processed, 0, "checkpoint must not advance on fatal");
        assert_eq!(state.last_processed_memory_id, -1);
    }

    #[test]
    fn process_one_extraction_failure_advances_checkpoint() {
        // PipelineError::ExtractionFailure → Failed outcome (not fatal),
        // failure row written, checkpoint advanced with delta_failed=1.
        let mut conn = fresh_db();
        let resolver = ScriptedResolver::new(vec![Err(PipelineError::ExtractionFailure(
            "schema mismatch".into(),
        ))]);
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>)
            .with_namespace("default");

        let outcome = proc.process_one(&mut conn, fixture_record(505)).unwrap();
        match outcome {
            RecordOutcome::Failed { record_id, kind, stage, .. } => {
                assert_eq!(record_id, 505);
                assert_eq!(kind, CATEGORY_INTERNAL);
                assert_eq!(stage, STAGE_DEDUP);
            }
            other => panic!("expected Failed, got {other:?}"),
        }

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_failed, 1);
        assert_eq!(state.records_processed, 1);
    }

    #[test]
    fn process_one_idempotent_on_replay() {
        // Process the same record twice. apply_graph_delta keys on
        // (memory_id, delta_hash) → second call returns
        // ApplyReport::already_applied. Checkpoint counters still
        // advance both times (at-least-once apply, exactly-once durable
        // graph effect — see module docs "Atomicity model").
        let mut conn = fresh_db();
        let memory_uuid = Uuid::from_u128(606);
        let delta1 = GraphDelta::new(memory_uuid);
        let delta2 = GraphDelta::new(memory_uuid); // identical → same hash
        let resolver = ScriptedResolver::new(vec![Ok(delta1), Ok(delta2)]);
        let proc = PipelineRecordProcessor::new(resolver as Arc<dyn BackfillResolver>);

        let _o1 = proc.process_one(&mut conn, fixture_record(606)).unwrap();
        let _o2 = proc.process_one(&mut conn, fixture_record(606)).unwrap();

        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_applied_deltas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 1, "duplicate apply should not insert a second row");

        let state = CheckpointStore::load_state(&conn).unwrap().unwrap();
        assert_eq!(state.records_processed, 2, "checkpoint counts both attempts");
    }
}
