//! Gate definitions and evaluator (design §4).
//!
//! Each release gate is a (GOAL, metric, threshold, comparator) tuple.
//! Evaluation order and dependency DAG are specified in design §4.3.
//!
//! ## Status
//!
//! **Stub.** Gate registry, comparator semantics, and the evaluator
//! (`evaluate_gates(driver_summary, baselines) -> Vec<GateResult>`) are
//! delivered by `task:bench-impl-gates`. This stub defines only the public
//! types referenced by `harness::RunReport` and lib.rs re-exports.

use serde::{Deserialize, Serialize};

/// Comparator kinds used by gate definitions (design §4.1, §4.2).
///
/// Each gate compares an observed metric against a threshold using one
/// of these comparators:
///
/// - `Ge` — metric ≥ threshold (most "higher is better" gates).
/// - `Le` — metric ≤ threshold (latency, cost-per-token, error rate).
/// - `Eq` — metric == threshold (test-count parity per GOAL-5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Comparator {
    /// Greater-than-or-equal — metric must reach the threshold.
    Ge,
    /// Less-than-or-equal — metric must stay under the threshold.
    Le,
    /// Equality — metric must match exactly (e.g. test-count freeze).
    Eq,
}

/// Gate priority — controls release-decision aggregation (design §4.1, §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Priority {
    /// P0 — ship-blocking; failure ⇒ `ReleaseDecision::Block`.
    P0,
    /// P1 — quality gate; failure ⇒ `ConditionalShip` if overridden, else `Block`.
    P1,
    /// P2 — observability; failure logs but does not block.
    P2,
}

/// Outcome of evaluating one gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateStatus {
    /// Metric satisfied the comparator vs the threshold.
    Pass,
    /// Metric did NOT satisfy the comparator.
    Fail,
    /// Gate could not be evaluated (missing metric, driver crash, fixture
    /// missing). Per design §6.1: missing gate-relevant fields cause
    /// `[run].status = "error"`.
    Error,
}

/// Result of evaluating one gate against one driver run (design §7.2).
#[derive(Debug, Clone)]
pub struct GateResult {
    /// GOAL identifier this gate enforces, e.g. `"GOAL-5.1"`.
    pub goal: String,
    /// Observed metric value.
    pub metric: f64,
    /// Threshold from the gate definition.
    pub threshold: f64,
    /// How `metric` was compared to `threshold`.
    pub comparator: Comparator,
    /// Pass / Fail / Error outcome.
    pub status: GateStatus,
    /// Priority class (drives release-decision aggregation).
    pub priority: Priority,
    /// Human-readable explanation, present on `Fail` or `Error`.
    pub message: Option<String>,
}
