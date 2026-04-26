//! §6.3 introspection — `ExtractionStatus` and supporting types.
//!
//! Step C of §3.1 ingestion: defines the public surface returned by
//! `Memory::extraction_status(memory_id)`. The status is computed from
//! the latest row in `graph_pipeline_runs` for a given memory id (Step B
//! gave us `GraphStore::latest_pipeline_run_for_memory`).
//!
//! # Design references
//!
//! - `.gid/features/v03-resolution/design.md` §6.3 (Introspection — public
//!   API contract for `ExtractionStatus`).
//! - `.gid/features/v03-resolution/reviews/design-r1.md` §FINDING-… —
//!   `Completed` carries `had_semantic_content: bool` so operators can
//!   filter empty-but-successful runs without rescanning the graph.
//! - `audit::RunStatus` — the persisted lifecycle enum that this module
//!   maps *out* to the public `ExtractionStatus`. The mapping is the
//!   single source of truth for status semantics; whenever a new
//!   `RunStatus` variant lands (e.g. `Pending` for queue-full tracking
//!   in Step C-bis / Step D), update [`ExtractionStatus::from_run_row`]
//!   accordingly.
//!
//! # Scope of this step (Step C)
//!
//! - `ExtractionStatus` enum with all five design-spec variants.
//! - `FailureKind` and `ResolutionTraceSummary` are real types kept
//!   intentionally minimal — they will be enriched by Step D (worker
//!   pool wiring) and §7 trace persistence work. The shapes here are
//!   the public-API skeleton; field additions are non-breaking when
//!   they happen behind `#[non_exhaustive]`.
//! - The mapping helper [`ExtractionStatus::from_run_row`] converts
//!   one `PipelineRunRow` into the corresponding status. Absence of a
//!   row maps to `NotStarted` *at the call site* (the helper takes an
//!   `Option<PipelineRunRow>` and handles the `None` arm explicitly).
//!
//! # Out of scope (deferred)
//!
//! - `Pending { queue_full: true }` requires the worker pool to write a
//!   `pending`-status row at enqueue time (Step C-bis or Step D). Until
//!   then, an enqueued-but-not-yet-running memory shows up as
//!   `NotStarted`. The mapping helper is the only place this needs to
//!   change when the variant lands.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::audit::RunStatus;
use crate::graph::store::PipelineRunRow;
use crate::resolution::context::PipelineStage;

/// Aggregate per-run summary embedded in `ExtractionStatus::Completed`.
///
/// This is the **caller-facing** projection of `ResolutionTrace`
/// (§7.1). The full per-decision trace lives in
/// `graph_resolution_traces`; what's surfaced here is the minimum a
/// downstream consumer needs to introspect a successful run without a
/// follow-up DB hop:
///
/// - `run_id` — primary key into `graph_pipeline_runs` /
///   `graph_resolution_traces` for callers that want the full trace.
///
/// Future fields (per design §7.1 / §7.2) — entity/edge counts,
/// `DecisionMix` aggregates, stage durations — will be added behind
/// `#[non_exhaustive]` so adding them is a non-breaking change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResolutionTraceSummary {
    pub run_id: Uuid,
}

impl ResolutionTraceSummary {
    /// Construct a minimal summary referencing a completed run. Used by
    /// [`ExtractionStatus::from_run_row`] when mapping a `Succeeded`
    /// row; richer summaries (counts, durations) land alongside §7
    /// trace persistence.
    pub fn from_run_id(run_id: Uuid) -> Self {
        Self { run_id }
    }
}

/// Structured failure category for `ExtractionStatus::Failed`.
///
/// The persisted `error_detail` column on `graph_pipeline_runs` is free-form
/// text (the resolution layer writes a JSON-encoded `StageFailure` blob into
/// it on `Failed` rows). For the public API, we surface a closed-set enum so
/// downstream tooling can match exhaustively without parsing JSON. Mapping
/// from the persisted `error_kind` string lives in [`FailureKind::from_str`].
///
/// Today there are only two variants — the full closed set lands when Step D
/// wires the worker pool's per-stage error categorization. New variants are
/// additive thanks to `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// LLM call failed (timeout, rate limit, transient 5xx, malformed
    /// response). The worker pool's error categorization will split
    /// this into finer variants in Step D.
    LlmError,
    /// Catch-all for failures we haven't classified yet. New variants
    /// will be carved out of this as the worker pool's failure
    /// taxonomy stabilizes.
    Other,
}

impl FailureKind {
    /// Best-effort parse of a persisted `error_kind` string into the
    /// public enum. Unknown kinds map to `Other` so old rows always
    /// decode (forward compatibility).
    ///
    /// The closed-set string constants in
    /// [`crate::graph::audit`] (`CATEGORY_LLM_TIMEOUT`,
    /// `CATEGORY_LLM_INVALID_OUTPUT`, ...) are the canonical names;
    /// they all currently fold into `LlmError` until we split them.
    pub fn from_kind_str(s: &str) -> Self {
        use crate::graph::audit::{
            CATEGORY_LLM_INVALID_OUTPUT, CATEGORY_LLM_TIMEOUT,
        };
        match s {
            CATEGORY_LLM_TIMEOUT | CATEGORY_LLM_INVALID_OUTPUT => Self::LlmError,
            _ => Self::Other,
        }
    }
}

/// Public ingestion / extraction status for a memory id (§6.3).
///
/// Returned by `Memory::extraction_status(memory_id)`. Computed from the
/// latest row in `graph_pipeline_runs` for the memory. The mapping rules
/// are the single source of truth; see [`Self::from_run_row`].
///
/// Variants follow design §6.3 verbatim. `Pending { queue_full: true }`
/// requires Step C-bis (a `RunStatus::Pending` row written at enqueue);
/// until then, an enqueued-but-not-yet-picked-up memory reads as
/// `NotStarted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExtractionStatus {
    /// No `graph_pipeline_runs` row exists for this memory. Either the
    /// memory predates v0.3 or — once Step C-bis lands — its enqueue
    /// row hasn't been written yet. Distinct from `Pending` only after
    /// the worker pool wiring; today, both legacy memories and freshly
    /// enqueued ones surface here.
    NotStarted,

    /// The memory is queued for processing. `queue_full=true` means the
    /// in-memory `BoundedJobQueue` rejected the enqueue and the memory
    /// is recoverable via `reextract --pending`.
    ///
    /// **Step C status**: this variant exists in the public API but is
    /// not yet emitted — `from_run_row` only produces it when a
    /// `RunStatus::Pending` lands (Step C-bis / Step D).
    Pending {
        since: DateTime<Utc>,
        queue_full: bool,
    },

    /// A worker is currently processing this memory. Mapped from
    /// `RunStatus::Running`.
    Running {
        started_at: DateTime<Utc>,
        run_id: Uuid,
    },

    /// The most recent run finished successfully. `had_semantic_content`
    /// is `false` when extraction ran cleanly but produced 0 mentions /
    /// 0 edges (§3.3.2 "no semantic content"); operators can filter
    /// these without rescanning the graph.
    ///
    /// **Step C status**: `had_semantic_content` defaults to `true`
    /// (the conservative interpretation: the run finished, so by
    /// convention it had content). The accurate value lands once §3.5
    /// persist writes the per-run `output_summary` JSON and Step D's
    /// trace flush populates `ResolutionTraceSummary` properly.
    Completed {
        completed_at: DateTime<Utc>,
        run_id: Uuid,
        trace: ResolutionTraceSummary,
        had_semantic_content: bool,
    },

    /// The most recent run terminated in an error state. `stage` and
    /// `kind` are the structured surface; `message` is the raw
    /// human-readable detail from `graph_pipeline_runs.error_detail`.
    Failed {
        failed_at: DateTime<Utc>,
        stage: PipelineStage,
        kind: FailureKind,
        message: String,
    },
}

impl ExtractionStatus {
    /// Map `Option<PipelineRunRow>` (the result of
    /// `GraphStore::latest_pipeline_run_for_memory`) to the public
    /// status enum.
    ///
    /// `None` → [`Self::NotStarted`].
    ///
    /// `Some(row)` is dispatched on `row.status`:
    ///
    /// | RunStatus  | ExtractionStatus                        |
    /// |------------|-----------------------------------------|
    /// | Running    | `Running { started_at, run_id }`        |
    /// | Succeeded  | `Completed { ... had_semantic_content }`|
    /// | Failed     | `Failed { failed_at, stage, kind, ... }`|
    /// | Cancelled  | `Failed { kind: Other, ... }` (cancelled is treated as a non-success terminal — operators reextract to retry) |
    ///
    /// `started_at` / `finished_at` come from the row directly. For
    /// `Failed` rows we attempt to recover `stage` from the JSON
    /// `error_detail` blob the resolution layer writes (best effort —
    /// unknown shapes fall back to `PipelineStage::Ingest` as the
    /// least-specific stage).
    pub fn from_run_row(row: Option<PipelineRunRow>) -> Self {
        let Some(row) = row else {
            return Self::NotStarted;
        };

        match row.status {
            RunStatus::Running => Self::Running {
                started_at: row.started_at,
                run_id: row.run_id,
            },
            RunStatus::Succeeded => Self::Completed {
                // `finished_at` should always be Some on a Succeeded row
                // (the audit state machine writes it on transition).
                // Fall back to `started_at` rather than panic — Succeeded
                // without finished_at is a corrupt row, not a logic bug
                // worth crashing the read path over.
                completed_at: row.finished_at.unwrap_or(row.started_at),
                run_id: row.run_id,
                trace: ResolutionTraceSummary::from_run_id(row.run_id),
                // Step C: optimistic default — see variant docstring.
                // Step D will read this from output_summary JSON.
                had_semantic_content: true,
            },
            RunStatus::Failed | RunStatus::Cancelled => {
                let (stage, kind, message) = decode_failure_detail(row.error_detail.as_deref());
                Self::Failed {
                    failed_at: row.finished_at.unwrap_or(row.started_at),
                    stage,
                    kind,
                    message,
                }
            }
        }
    }
}

/// Best-effort decode of the `error_detail` column for a `Failed` /
/// `Cancelled` row. The resolution layer stores a JSON blob shaped like
/// `{"stage": "...", "error_kind": "...", "message": "..."}`; older rows
/// (or rows written by other callers of `finish_pipeline_run`) may store
/// plain text.
///
/// Falls back gracefully:
/// - Missing `stage` → `PipelineStage::Ingest` (least-specific).
/// - Missing `error_kind` → `FailureKind::Other`.
/// - Non-JSON or empty → message = the raw string (or `""` if `None`).
fn decode_failure_detail(raw: Option<&str>) -> (PipelineStage, FailureKind, String) {
    let Some(text) = raw else {
        return (PipelineStage::Ingest, FailureKind::Other, String::new());
    };

    // Attempt structured decode first.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        let stage = value
            .get("stage")
            .and_then(|v| v.as_str())
            .and_then(parse_stage_str)
            .unwrap_or(PipelineStage::Ingest);
        let kind = value
            .get("error_kind")
            .and_then(|v| v.as_str())
            .map(FailureKind::from_kind_str)
            .unwrap_or(FailureKind::Other);
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return (stage, kind, message);
    }

    // Plain-text fallback.
    (PipelineStage::Ingest, FailureKind::Other, text.to_string())
}

/// Inverse of `PipelineStage::as_str` for the closed set defined in
/// `resolution::context`. Unknown labels return `None` so the caller
/// can decide on a fallback (we use `Ingest` — see
/// [`decode_failure_detail`]).
fn parse_stage_str(s: &str) -> Option<PipelineStage> {
    match s {
        "ingest" => Some(PipelineStage::Ingest),
        "entity_extract" => Some(PipelineStage::EntityExtract),
        "edge_extract" => Some(PipelineStage::EdgeExtract),
        "resolve" => Some(PipelineStage::Resolve),
        "persist" => Some(PipelineStage::Persist),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::audit::PipelineKind;

    #[test]
    fn none_row_maps_to_not_started() {
        assert!(matches!(
            ExtractionStatus::from_run_row(None),
            ExtractionStatus::NotStarted
        ));
    }

    #[test]
    fn running_row_maps_to_running() {
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: None,
        };
        let want_run_id = r.run_id;
        let want_started = r.started_at;
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Running { run_id, started_at } => {
                assert_eq!(run_id, want_run_id);
                assert_eq!(started_at, want_started);
            }
            other => panic!("expected Running, got {other:?}"),
        }
    }

    #[test]
    fn succeeded_row_maps_to_completed() {
        let started = Utc::now();
        let finished = Utc::now();
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Succeeded,
            started_at: started,
            finished_at: Some(finished),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: None,
        };
        let want_run_id = r.run_id;
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Completed {
                completed_at,
                run_id,
                trace,
                had_semantic_content,
            } => {
                assert_eq!(completed_at, finished);
                assert_eq!(run_id, want_run_id);
                assert_eq!(trace.run_id, want_run_id);
                // Step C default — see variant docstring.
                assert!(had_semantic_content);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn succeeded_row_without_finished_at_falls_back_to_started_at() {
        // Defensive: corrupt rows shouldn't crash the read path.
        let started = Utc::now();
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Succeeded,
            started_at: started,
            finished_at: None,
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: None,
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Completed { completed_at, .. } => assert_eq!(completed_at, started),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn failed_row_with_json_error_detail_decodes_stage_and_kind() {
        let detail = r#"{"stage": "edge_extract", "error_kind": "llm_timeout", "message": "Anthropic 504"}"#;
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: Some(detail.to_string()),
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Failed {
                stage,
                kind,
                message,
                ..
            } => {
                assert_eq!(stage, PipelineStage::EdgeExtract);
                assert_eq!(kind, FailureKind::LlmError);
                assert_eq!(message, "Anthropic 504");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn failed_row_with_plain_text_error_detail_decodes_to_other() {
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: Some("worker panicked".to_string()),
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Failed {
                stage,
                kind,
                message,
                ..
            } => {
                assert_eq!(stage, PipelineStage::Ingest);
                assert_eq!(kind, FailureKind::Other);
                assert_eq!(message, "worker panicked");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn failed_row_with_no_error_detail_yields_empty_message() {
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: None,
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Failed { message, kind, .. } => {
                assert!(message.is_empty());
                assert_eq!(kind, FailureKind::Other);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn cancelled_row_maps_to_failed_other() {
        // Cancelled runs surface to the operator as a recoverable
        // failure (they can reextract). The mapping is documented in
        // `from_run_row`'s rustdoc.
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Cancelled,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: Some("operator stop".to_string()),
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Failed { kind, message, .. } => {
                assert_eq!(kind, FailureKind::Other);
                assert_eq!(message, "operator stop");
            }
            other => panic!("expected Failed for Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn unknown_stage_string_falls_back_to_ingest() {
        let detail = r#"{"stage": "wat", "error_kind": "internal", "message": ""}"#;
        let r = PipelineRunRow {
            run_id: Uuid::new_v4(),
            kind: PipelineKind::Resolution,
            status: RunStatus::Failed,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            memory_id: Some("m-1".to_string()),
            episode_id: Some(Uuid::new_v4()),
            error_detail: Some(detail.to_string()),
        };
        match ExtractionStatus::from_run_row(Some(r)) {
            ExtractionStatus::Failed { stage, .. } => assert_eq!(stage, PipelineStage::Ingest),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn failure_kind_from_kind_str_known_categories_fold_to_llm_error() {
        use crate::graph::audit::{CATEGORY_LLM_INVALID_OUTPUT, CATEGORY_LLM_TIMEOUT};
        assert_eq!(FailureKind::from_kind_str(CATEGORY_LLM_TIMEOUT), FailureKind::LlmError);
        assert_eq!(FailureKind::from_kind_str(CATEGORY_LLM_INVALID_OUTPUT), FailureKind::LlmError);
        assert_eq!(FailureKind::from_kind_str("unknown_category"), FailureKind::Other);
    }
}
