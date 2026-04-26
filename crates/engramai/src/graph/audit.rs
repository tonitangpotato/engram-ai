//! v0.3 graph audit types — pipeline runs, extraction failures, resolution traces.
//!
//! See `.gid/features/v03-graph-layer/design.md` §4.2.
//!
//! These types are append-only audit rows. `PipelineRun` exposes a small state
//! machine (`Running -> {Succeeded, Failed, Cancelled}`) enforced by
//! `finish_*` helpers; once terminal, a run is never reopened.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::GraphError;

// ---------------------------------------------------------------------------
// Closed-set string constants — single source of truth for stage / category
// / decision labels persisted as TEXT columns.
// ---------------------------------------------------------------------------

// Stage labels — closed set, single source of truth.
pub const STAGE_ENTITY_EXTRACT: &str = "entity_extract";
pub const STAGE_EDGE_EXTRACT: &str = "edge_extract";
pub const STAGE_DEDUP: &str = "dedup";
pub const STAGE_PERSIST: &str = "persist";

// Failure categories — closed set.
pub const CATEGORY_LLM_TIMEOUT: &str = "llm_timeout";
pub const CATEGORY_LLM_INVALID_OUTPUT: &str = "llm_invalid_output";
pub const CATEGORY_BUDGET_EXHAUSTED: &str = "budget_exhausted";
pub const CATEGORY_DB_ERROR: &str = "db_error";
pub const CATEGORY_INTERNAL: &str = "internal";

// Decision labels for ResolutionTrace.
pub const DECISION_NEW: &str = "new";
pub const DECISION_MATCHED_EXISTING: &str = "matched_existing";
pub const DECISION_SUPERSEDED: &str = "superseded";
pub const DECISION_MERGED: &str = "merged";
pub const DECISION_REJECTED: &str = "rejected";

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Pipeline run kinds (§4.2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineKind {
    Resolution,
    Reextract,
    KnowledgeCompile,
}

/// Pipeline run lifecycle status (§4.2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

// ---------------------------------------------------------------------------
// PipelineRun
// ---------------------------------------------------------------------------

/// PipelineRun — orchestrator-level audit row for a synthesis or resolution run (§4.2).
/// Append-only: status transitions are written via dedicated finish_* helpers and never
/// reopened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: Uuid,
    pub kind: PipelineKind,
    pub status: RunStatus,
    pub started_at: f64,
    pub finished_at: Option<f64>,
    pub stats: serde_json::Value,         // free-form per-pipeline stats blob
    pub input_summary: serde_json::Value, // captured at begin
}

impl PipelineRun {
    /// Construct a new run in Running state. `now_secs` is the unix epoch seconds f64.
    pub fn start(kind: PipelineKind, input_summary: serde_json::Value, now_secs: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            status: RunStatus::Running,
            started_at: now_secs,
            finished_at: None,
            stats: serde_json::Value::Null,
            input_summary,
        }
    }

    /// Transition Running -> Succeeded. Returns Err(InvariantViolation) if not in Running state.
    /// `stats` is the final per-run stats blob (e.g., entities_resolved, edges_inserted).
    pub fn finish_succeeded(
        &mut self,
        stats: serde_json::Value,
        now_secs: f64,
    ) -> Result<(), GraphError> {
        self.transition(RunStatus::Succeeded, stats, now_secs)
    }

    pub fn finish_failed(
        &mut self,
        stats: serde_json::Value,
        now_secs: f64,
    ) -> Result<(), GraphError> {
        self.transition(RunStatus::Failed, stats, now_secs)
    }

    pub fn finish_cancelled(
        &mut self,
        stats: serde_json::Value,
        now_secs: f64,
    ) -> Result<(), GraphError> {
        self.transition(RunStatus::Cancelled, stats, now_secs)
    }

    fn transition(
        &mut self,
        to: RunStatus,
        stats: serde_json::Value,
        now_secs: f64,
    ) -> Result<(), GraphError> {
        if self.status != RunStatus::Running {
            return Err(GraphError::Invariant(
                "PipelineRun status transition from non-Running",
            ));
        }
        self.status = to;
        self.stats = stats;
        self.finished_at = Some(now_secs);
        Ok(())
    }

    /// True if the run is still active (Running). Convenience for callers.
    pub fn is_active(&self) -> bool {
        matches!(self.status, RunStatus::Running)
    }
}

// ---------------------------------------------------------------------------
// ExtractionFailure
// ---------------------------------------------------------------------------

/// ExtractionFailure — one row per stage failure (§4.1, §4.2).
///
/// `stage` and `error_category` are persisted as TEXT in SQLite. The design doc
/// types them as `&'static str`, but `serde::Deserialize` for `&'static str` is
/// not derivable from owned input (DB rows / JSON), so we store them as `String`
/// and constrain values via the closed-set `STAGE_*` / `CATEGORY_*` constants —
/// callers should assign with `STAGE_ENTITY_EXTRACT.to_string()` etc. Same
/// pattern as `delta::StageFailureRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionFailure {
    pub id: Uuid,
    pub episode_id: Uuid,
    pub stage: String,          // see STAGE_* consts
    pub error_category: String, // see CATEGORY_* consts
    pub error_detail: Option<String>,
    pub occurred_at: f64,
    pub resolved_at: Option<f64>, // None = unresolved
}

// ---------------------------------------------------------------------------
// ResolutionTrace
// ---------------------------------------------------------------------------

/// Resolution trace — per-decision audit (§4.2).
///
/// See `ExtractionFailure` for the `String` vs `&'static str` rationale —
/// values are constrained via `STAGE_*` and `DECISION_*` constants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionTrace {
    pub trace_id: Uuid,
    pub run_id: Uuid,
    pub edge_id: Option<Uuid>,
    pub entity_id: Option<Uuid>,
    pub stage: String, // 'entity_extract' | 'edge_extract' | 'dedup' | 'persist'
    pub decision: String, // 'new' | 'matched_existing' | 'superseded' | 'merged' | 'rejected'
    pub reason: Option<String>,
    pub candidates: Option<serde_json::Value>,
    pub recorded_at: f64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_run_starts_in_running() {
        let r = PipelineRun::start(
            PipelineKind::Resolution,
            serde_json::json!({"episodes": 1}),
            100.0,
        );
        assert_eq!(r.status, RunStatus::Running);
        assert_eq!(r.started_at, 100.0);
        assert_eq!(r.finished_at, None);
        assert!(r.is_active());
    }

    #[test]
    fn pipeline_run_finish_succeeded_transitions() {
        let mut r =
            PipelineRun::start(PipelineKind::Resolution, serde_json::Value::Null, 1.0);
        r.finish_succeeded(serde_json::json!({"ok": true}), 2.0)
            .unwrap();
        assert_eq!(r.status, RunStatus::Succeeded);
        assert_eq!(r.finished_at, Some(2.0));
        assert!(!r.is_active());
    }

    #[test]
    fn pipeline_run_finish_failed_transitions() {
        let mut r =
            PipelineRun::start(PipelineKind::Reextract, serde_json::Value::Null, 1.0);
        r.finish_failed(serde_json::json!({"err": "timeout"}), 5.0)
            .unwrap();
        assert_eq!(r.status, RunStatus::Failed);
    }

    #[test]
    fn pipeline_run_finish_cancelled_transitions() {
        let mut r = PipelineRun::start(
            PipelineKind::KnowledgeCompile,
            serde_json::Value::Null,
            1.0,
        );
        r.finish_cancelled(serde_json::Value::Null, 3.0).unwrap();
        assert_eq!(r.status, RunStatus::Cancelled);
    }

    #[test]
    fn pipeline_run_double_finish_rejected() {
        let mut r =
            PipelineRun::start(PipelineKind::Resolution, serde_json::Value::Null, 1.0);
        r.finish_succeeded(serde_json::Value::Null, 2.0).unwrap();
        let err = r.finish_failed(serde_json::Value::Null, 3.0);
        assert!(err.is_err());
    }

    #[test]
    fn pipeline_run_transition_from_failed_rejected() {
        let mut r =
            PipelineRun::start(PipelineKind::Resolution, serde_json::Value::Null, 1.0);
        r.finish_failed(serde_json::Value::Null, 2.0).unwrap();
        assert!(r.finish_succeeded(serde_json::Value::Null, 3.0).is_err());
    }

    #[test]
    fn pipeline_run_serde_roundtrip() {
        let mut r = PipelineRun::start(
            PipelineKind::Resolution,
            serde_json::json!({"k":"v"}),
            100.0,
        );
        r.finish_succeeded(serde_json::json!({"count": 42}), 200.0)
            .unwrap();
        let s = serde_json::to_string(&r).unwrap();
        let r2: PipelineRun = serde_json::from_str(&s).unwrap();
        assert_eq!(r2.status, RunStatus::Succeeded);
        assert_eq!(r2.stats, serde_json::json!({"count": 42}));
    }

    #[test]
    fn extraction_failure_constructs() {
        let f = ExtractionFailure {
            id: Uuid::new_v4(),
            episode_id: Uuid::new_v4(),
            stage: STAGE_ENTITY_EXTRACT.to_string(),
            error_category: CATEGORY_LLM_TIMEOUT.to_string(),
            error_detail: Some("timeout after 30s".into()),
            occurred_at: 100.0,
            resolved_at: None,
        };
        assert_eq!(f.resolved_at, None);
        assert_eq!(f.stage, "entity_extract");
    }

    #[test]
    fn resolution_trace_serde_roundtrip() {
        let t = ResolutionTrace {
            trace_id: Uuid::new_v4(),
            run_id: Uuid::new_v4(),
            edge_id: None,
            entity_id: Some(Uuid::new_v4()),
            stage: STAGE_DEDUP.to_string(),
            decision: DECISION_MATCHED_EXISTING.to_string(),
            reason: Some("alias hit".into()),
            candidates: Some(serde_json::json!([{"id": "x", "score": 0.9}])),
            recorded_at: 1.0,
        };
        let s = serde_json::to_string(&t).unwrap();
        let t2: ResolutionTrace = serde_json::from_str(&s).unwrap();
        assert_eq!(t2.decision, "matched_existing");
    }

    #[test]
    fn pipeline_kind_serde_snake_case() {
        let k = PipelineKind::KnowledgeCompile;
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, "\"knowledge_compile\"");
    }
}
