//! Cognitive-feature regression driver (design §3.5).
//!
//! Implements [`crate::harness::BenchDriver`] for the **three-feature
//! directional regression suite**, gate **GOAL-5.6**
//! ("interoceptive / metacognition / affect each measurably affect
//! retrieval ranking in the expected direction").
//!
//! ## Current implementation status (skip-aware mode)
//!
//! As of 2026-04-27, [`engramai::Memory::graph_query`] /
//! [`engramai::Memory::graph_query_locked`] dispatch through the
//! orchestrator, but they hardcode `self_state: None` (see
//! `engramai/src/retrieval/api.rs` ~line 357). Until the cognitive
//! state readback lands
//! (`task:retr-impl-cognitive-state-readback` on
//! `.gid-v03-context/graph.db`), changing the cognitive state of a
//! `Memory` between two runs cannot influence retrieval ranking, so
//! the directional metric is undefined.
//!
//! This driver runs in **skip-aware mode** until that lands:
//!
//! 1. It still exercises the call site (`probe_orchestrator_status`)
//!    so the harness fails loudly if `graph_query` regresses to a
//!    pure stub.
//! 2. The summary always carries a `blocked_by` reason and the
//!    driver emits `MetricSnapshot::set_missing(
//!    "cognitive.regression_count", ...)`.
//! 3. The gate evaluator maps `Missing` → [`GateStatus::Error`] per
//!    GUARD-2 ("never silent degrade") — never `Pass`.
//!
//! This mirrors the precedent in `test_preservation.rs`, which uses
//! the same skip-aware pattern to surface the migration-tool gap.
//!
//! ## What this driver asserts (when orchestrator is live)
//!
//! For each of three cognitive features (**interoceptive**, **affect**,
//! **metacognition**) we run two `engramai::Memory` instances seeded
//! with **byte-identical** episode data, set the feature's state to two
//! distinct values `S1` and `S2`, run the same query against both, and
//! collect the top-K result lists. We then compute a directional metric:
//!
//! - **Interoceptive / Affect** — Jaccard distance between the two
//!   top-K ID lists must exceed `jaccard_threshold` (default `0.2`,
//!   i.e. ≥ 2 of 10 items must differ).
//! - **Metacognition** — the count of items filtered out under
//!   `S1.confidence` vs `S2.confidence` must differ (i.e. confidence
//!   visibly gates results).
//!
//! A feature whose two runs satisfy its directional metric is recorded
//! as `regressed: false`. A feature whose runs fail the metric is
//! `regressed: true`. The gate metric `cognitive.regression_count` is
//! the count of regressed features (must be `0` for GOAL-5.6 to pass).
//!
//! ## Why directional, not absolute quality
//!
//! Per design §3.5: measuring whether mood-congruent recall is "good"
//! requires a labelled mood-recall dataset that does not exist for our
//! corpus. Measuring whether mood *changes the output ranking* is
//! robust against the ground-truth gap and catches the regression of
//! interest — "a cognitive input got silently disconnected from the
//! ranking pipeline".
//!
//! ## Stage / cost
//!
//! `Stage2 / Medium` — pure in-process execution (two `Memory`
//! instances, deterministic ingest, fixed query set), no LLM, no
//! network. Stage2 because it depends on retrieval (Stage1 component
//! tests must pass first).
//!
//! ## Output (`cognitive_regression_summary.json`)
//!
//! ```json
//! {
//!   "features": [...],          // per-feature reports (empty when blocked)
//!   "regression_count": 0,
//!   "blocked_by": "..."         // present iff orchestrator stub
//! }
//! ```

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::harness::gates::{
    evaluate_for_driver, GateResult, MetricSnapshot, NoBaselines,
};
use crate::harness::{BenchDriver, BenchError, CostTier, Driver, HarnessConfig, RunReport, Stage};

// ---------------------------------------------------------------------------
// Public driver type — wired into `main::resolve_driver`.
// ---------------------------------------------------------------------------

/// Cognitive-feature regression driver — design §3.5.
#[derive(Debug, Default, Clone, Copy)]
pub struct CognitiveRegressionDriver;

impl CognitiveRegressionDriver {
    /// Construct the zero-sized driver handle.
    pub fn new() -> Self {
        Self
    }
}

impl BenchDriver for CognitiveRegressionDriver {
    fn name(&self) -> Driver {
        Driver::CognitiveRegression
    }

    fn stage(&self) -> Stage {
        // Depends on retrieval being functional (Stage1 component tests).
        Stage::Stage2
    }

    fn cost_tier(&self) -> CostTier {
        // Two Memory instances + N queries × 3 features. Seconds.
        CostTier::Medium
    }

    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError> {
        run_impl(config)
    }
}

// ---------------------------------------------------------------------------
// Summary type
// ---------------------------------------------------------------------------

/// Per-feature regression report (one row per cognitive feature).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureReport {
    /// Feature name: `"interoceptive"`, `"affect"`, `"metacognition"`.
    pub name: String,
    /// Metric kind: `"jaccard"` or `"filter_count_diff"`.
    pub metric: String,
    /// Computed metric value (Jaccard distance or count delta).
    pub value: f64,
    /// Threshold the metric must exceed for `regressed: false`.
    pub threshold: f64,
    /// Top-K result IDs from the S1 (state 1) run.
    pub s1_top_k: Vec<String>,
    /// Top-K result IDs from the S2 (state 2) run.
    pub s2_top_k: Vec<String>,
    /// `true` iff metric ≤ threshold (cognitive feature failed to
    /// influence ranking).
    pub regressed: bool,
}

/// Top-level summary written to `cognitive_regression_summary.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CognitiveRegressionSummary {
    /// Per-feature reports. Empty when `blocked_by` is `Some(...)`.
    #[serde(default)]
    pub features: Vec<FeatureReport>,
    /// Count of features with `regressed: true`. The gate metric
    /// `cognitive.regression_count` reads this. Must be `0` to pass.
    pub regression_count: usize,
    /// Upstream blocker — present iff orchestrator stub prevented a
    /// real run. None on a normal run.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocked_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Run pipeline (skip-aware)
// ---------------------------------------------------------------------------

fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // Probe 1: retrieval orchestrator stub. If `Memory::graph_query`
    // is still the stub returning `RetrievalError::Internal`, record
    // that as the lead `blocked_by`.
    //
    // Probe 2 (below): even if the orchestrator landed, the cognitive
    // directional metric requires cognitive state to actually flow
    // into retrieval (`self_state` argument). That plumbing is
    // tracked separately as `task:retr-impl-cognitive-state-readback`
    // and is the current real blocker.
    let blocked_by = probe_orchestrator_status();

    // Even if `graph_query` is no longer a stub, the cognitive
    // directional metric (S1 vs S2 ranking divergence per feature)
    // requires cognitive state — interoceptive / affect / metacognition
    // — to actually flow into retrieval. As of 2026-04-27,
    // `Memory::graph_query` hardcodes `self_state: None` (see
    // engramai/src/retrieval/api.rs around line 357), so changing
    // S1 → S2 cannot influence ranking. The structural prerequisite
    // is `task:retr-impl-cognitive-state-readback` (filed on
    // .gid-v03-context/graph.db, blocks feature:v03-benchmarks).
    //
    // This second blocker is checked AFTER the orchestrator probe so
    // operators see the right lead error: when the orchestrator was
    // a stub, that was the lead message; once it landed, the missing
    // self-state plumbing surfaces.
    let blocked_by = blocked_by.or_else(|| {
        Some(
            "Memory::graph_query hardcodes `self_state: None` — \
             cognitive state (interoceptive / affect / metacognition) does \
             not yet flow into retrieval, so S1 vs S2 ranking divergence \
             cannot be measured. \
             blocked_by: task:retr-impl-cognitive-state-readback \
             (read the live cognitive state into the orchestrator's \
             self_state argument and add per-query overrides on \
             GraphQuery)"
                .to_string(),
        )
    });

    let summary = CognitiveRegressionSummary {
        features: Vec::new(),
        regression_count: 0,
        blocked_by,
    };

    finalize_run(config, summary)
}

/// Probe whether `Memory::graph_query` is still a stub. Returns
/// `Some(blocked_by_message)` when blocked, `None` when the
/// orchestrator is live.
///
/// Implementation: spin up an ephemeral in-memory Memory, call
/// `graph_query` with a trivial query, and inspect the error variant.
/// `RetrievalError::Internal` with the documented stub message → blocked.
/// Any other `Ok(_)` or non-Internal `Err(_)` → orchestrator is doing
/// *something*, so we let the real pipeline take over.
fn probe_orchestrator_status() -> Option<String> {
    use engramai::retrieval::{GraphQuery, RetrievalError};
    use engramai::Memory;

    // Use a tempdir-like in-memory path so we don't pollute the
    // workspace. SQLite supports `:memory:` for ephemeral DBs.
    let mem = match Memory::new(":memory:", None) {
        Ok(m) => m,
        Err(e) => {
            // If we can't even build a Memory, that's a real failure
            // — but a non-orchestrator one. Surface as blocked with a
            // clear message.
            return Some(format!(
                "cognitive_regression probe failed at Memory::new: {e}; \
                 cannot determine orchestrator status. \
                 blocked_by: task:retr-impl-orchestrator-classifier-dispatch"
            ));
        }
    };

    let q = GraphQuery::new("orchestrator-probe").with_limit(1);
    let res = block_on(mem.graph_query(q));

    match res {
        Err(RetrievalError::Internal(msg))
            if msg.contains("not yet implemented") =>
        {
            Some(format!(
                "Memory::graph_query is a stub: {msg}. \
                 blocked_by: task:retr-impl-orchestrator-classifier-dispatch \
                 (and follow-ups: plan-execution, fusion-assembly, locked-mode, e2e)"
            ))
        }
        Err(RetrievalError::Internal(msg)) => {
            // Internal but *not* the documented stub message — the
            // orchestrator may have started landing. Treat as blocked
            // with a different reason so the operator notices.
            Some(format!(
                "Memory::graph_query returned unexpected Internal error: {msg}. \
                 Re-check task:retr-impl-orchestrator-* status."
            ))
        }
        Err(other) => {
            // Real (non-Internal) retrieval error means the
            // orchestrator IS dispatching but something downstream
            // failed. Let the real pipeline take over and surface
            // that error properly.
            //
            // For skip-aware mode this means: NOT blocked, fall
            // through to the (currently unimplemented) real pipeline
            // body, which returns BenchError::Other.
            log_probe_note(&format!(
                "cognitive_regression probe: graph_query returned non-stub \
                 error variant ({other:?}); orchestrator appears live. \
                 Falling through to real pipeline."
            ));
            None
        }
        Ok(_) => {
            // Orchestrator returned a real response. Real pipeline
            // takes over.
            log_probe_note(
                "cognitive_regression probe: graph_query returned Ok; \
                 orchestrator is live. Falling through to real pipeline.",
            );
            None
        }
    }
}

/// Stderr breadcrumb so an operator running the harness sees *why*
/// the driver decided to skip vs. run. Cheaper than wiring tracing
/// into engram-bench just for this one log line.
fn log_probe_note(msg: &str) {
    eprintln!("[cognitive_regression] {msg}");
}

/// Async-to-sync bridge — engramai retrieval API is `async fn`,
/// engram-bench is sync. Same noop-waker pattern as
/// `drivers/locomo.rs::block_on` (engram-bench deliberately avoids
/// tokio per `Cargo.toml`).
fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let waker = Waker::from(Arc::new(NoopWaker));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

// ---------------------------------------------------------------------------
// Artifact emission + RunReport assembly
// ---------------------------------------------------------------------------

fn finalize_run(
    config: &HarnessConfig,
    summary: CognitiveRegressionSummary,
) -> Result<RunReport, BenchError> {
    let out_dir = ensure_run_dir(config)?;
    let summary_path = out_dir.join("cognitive_regression_summary.json");

    let body = serde_json::to_string_pretty(&summary)
        .map_err(|e| BenchError::Other(format!("summary serialization failed: {e}")))?;
    fs::write(&summary_path, body).map_err(BenchError::IoError)?;

    let record_path = out_dir.join("reproducibility.toml");
    write_reproducibility_stub(&record_path, &summary)?;

    let gates = evaluate_gates(&summary);
    let summary_json = serde_json::to_value(&summary)
        .map_err(|e| BenchError::Other(format!("summary->json failed: {e}")))?;

    Ok(RunReport {
        driver: Driver::CognitiveRegression,
        record_path,
        gates,
        summary_json,
    })
}

/// Resolve the per-run output directory. Matches the
/// `<output_root>/<timestamp>_<driver>/` shape used by other drivers.
/// We don't need the SHA suffix here (no fixture) — keep it simple.
fn ensure_run_dir(config: &HarnessConfig) -> Result<PathBuf, BenchError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dir = config
        .output_root
        .join(format!("{ts}_cognitive_regression"));
    fs::create_dir_all(&dir).map_err(BenchError::IoError)?;
    Ok(dir)
}

/// Minimal reproducibility record. The richer schema (per design §6.1)
/// will be filled in by `task:retr-test-orchestrator-e2e` when this
/// driver actually runs queries — until then the record's job is to
/// document the skip reason.
fn write_reproducibility_stub(
    path: &PathBuf,
    summary: &CognitiveRegressionSummary,
) -> Result<(), BenchError> {
    let mut s = String::new();
    s.push_str("# cognitive_regression reproducibility record\n");
    s.push_str("[run]\n");
    s.push_str("driver = \"cognitive-regression\"\n");
    if let Some(reason) = &summary.blocked_by {
        s.push_str("status = \"error\"\n");
        s.push_str(&format!("blocked_by = {}\n", toml_string_literal(reason)));
    } else {
        s.push_str(&format!(
            "status = \"{}\"\n",
            if summary.regression_count == 0 { "pass" } else { "fail" }
        ));
        s.push_str(&format!(
            "regression_count = {}\n",
            summary.regression_count
        ));
    }
    fs::write(path, s).map_err(BenchError::IoError)?;
    Ok(())
}

/// TOML doesn't allow raw newlines/quotes in basic strings. Use the
/// triple-quoted multi-line form for safety; trim any stray
/// triple-quote sequences from the input to avoid breaking the
/// delimiter.
fn toml_string_literal(s: &str) -> String {
    let escaped = s.replace("\"\"\"", "\"\"\\\"");
    format!("\"\"\"{escaped}\"\"\"")
}

// ---------------------------------------------------------------------------
// Gate evaluation
// ---------------------------------------------------------------------------

/// Build the metric snapshot for `evaluate_for_driver("cognitive.", ...)`.
///
/// - `blocked_by = Some(_)` → `set_missing("cognitive.regression_count", reason)`
///   → gate evaluates to `GateStatus::Error` (GUARD-2: never silent pass).
/// - `blocked_by = None` → `set_number("cognitive.regression_count", count as f64)`
///   → gate compares against `Constant(0.0)` with `Equal`.
fn build_snapshot(summary: &CognitiveRegressionSummary) -> MetricSnapshot {
    let mut snap = MetricSnapshot::new();
    if let Some(reason) = &summary.blocked_by {
        snap.set_missing(
            "cognitive.regression_count",
            format!("cognitive_regression blocked: {reason}"),
        );
    } else {
        snap.set_number(
            "cognitive.regression_count",
            summary.regression_count as f64,
        );
    }
    snap
}

/// Evaluate gates whose `metric_key` starts with `cognitive.` against
/// this driver's summary. Single-gate driver today (only GOAL-5.6).
pub(crate) fn evaluate_gates(summary: &CognitiveRegressionSummary) -> Vec<GateResult> {
    let snap = build_snapshot(summary);
    evaluate_for_driver("cognitive.", &snap, &NoBaselines)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::GateStatus;
    use tempfile::tempdir;

    /// The driver's `Driver`/`Stage`/`CostTier` triple must match what
    /// the harness scheduler expects (Stage2, Medium).
    #[test]
    fn driver_metadata() {
        let d = CognitiveRegressionDriver::new();
        assert_eq!(d.name(), Driver::CognitiveRegression);
        assert_eq!(d.stage(), Stage::Stage2);
        assert_eq!(d.cost_tier(), CostTier::Medium);
    }

    /// When `blocked_by` is set, the snapshot must surface
    /// `cognitive.regression_count` as Missing — never as `0` (which
    /// would silently pass the gate, violating GUARD-2).
    #[test]
    fn blocked_summary_emits_missing_metric() {
        let summary = CognitiveRegressionSummary {
            features: Vec::new(),
            regression_count: 0,
            blocked_by: Some("orchestrator stub".into()),
        };
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "cognitive.regression_count")
            .expect("GOAL-5.6 gate must fire even when blocked");
        assert_eq!(
            g.status,
            GateStatus::Error,
            "blocked run must Error, not Pass — GUARD-2 (never silent degrade)"
        );
    }

    /// When `blocked_by` is None and `regression_count == 0`, gate passes.
    #[test]
    fn unblocked_zero_regression_passes() {
        let summary = CognitiveRegressionSummary {
            features: Vec::new(),
            regression_count: 0,
            blocked_by: None,
        };
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "cognitive.regression_count")
            .expect("GOAL-5.6 gate must be present");
        assert_eq!(g.status, GateStatus::Pass);
    }

    /// When `blocked_by` is None and `regression_count > 0`, gate fails.
    #[test]
    fn unblocked_nonzero_regression_fails() {
        let summary = CognitiveRegressionSummary {
            features: Vec::new(),
            regression_count: 2,
            blocked_by: None,
        };
        let gates = evaluate_gates(&summary);
        let g = gates
            .iter()
            .find(|g| g.metric_key == "cognitive.regression_count")
            .expect("GOAL-5.6 gate must be present");
        assert_eq!(g.status, GateStatus::Fail);
    }

    /// `run_impl` against the live workspace must produce a blocked
    /// summary, write the JSON artifact, and return a `RunReport`
    /// whose gate is `Error`.
    ///
    /// This is the canonical skip-aware integration test. It accepts
    /// EITHER blocker — the orchestrator stub message OR the
    /// cognitive-state-readback message — because both are valid
    /// upstream gaps and which one fires depends on which lands first.
    /// The invariant that matters: gate must be Error, never silent
    /// Pass (GUARD-2).
    #[test]
    fn run_impl_against_stub_orchestrator_produces_blocked_report() {
        let tmp = tempdir().unwrap();
        let mut cfg = HarnessConfig::default();
        cfg.output_root = tmp.path().to_path_buf();

        let report = run_impl(&cfg).expect("driver must not error in skip-aware mode");

        assert_eq!(report.driver, Driver::CognitiveRegression);
        assert!(
            report.record_path.exists(),
            "reproducibility record must be written"
        );

        // Summary JSON must record one of the two recognized blockers.
        let blocked_by = report
            .summary_json
            .get("blocked_by")
            .and_then(|v| v.as_str())
            .expect("summary.blocked_by must be set when prereqs missing");
        assert!(
            blocked_by.contains("task:retr-impl-orchestrator")
                || blocked_by.contains("task:retr-impl-cognitive-state-readback"),
            "blocked_by must name an upstream task; got: {blocked_by}"
        );

        // Gate must be Error (not Pass, not Fail).
        let g = report
            .gates
            .iter()
            .find(|g| g.metric_key == "cognitive.regression_count")
            .expect("GOAL-5.6 gate must be present in report");
        assert_eq!(
            g.status,
            GateStatus::Error,
            "blocked prereqs must surface as Error, not silent Pass"
        );
    }
}
