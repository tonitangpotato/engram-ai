//! Harness — driver runner, reproducibility record, and gate evaluator.
//!
//! Per `v03-benchmarks` design §3 (drivers), §6 (reproducibility record),
//! and §4 (gates). The harness is the orchestrator: it loads a `HarnessConfig`,
//! invokes the requested `BenchDriver`, captures the run as a [`RunReport`],
//! emits a `reproducibility.toml`, and evaluates gates against the result.
//!
//! Module layout (per build plan T2 / design §3, §6, §7):
//!
//! - `repro` — reproducibility-record schema (§6.1) and writer (§6.2).
//!   Owned by `task:bench-impl-repro`.
//! - `gates` — gate definitions and evaluator (§4). Owned by
//!   `task:bench-impl-gates`.
//!
//! ## Status
//!
//! This file is a **structural placeholder** established by
//! `task:bench-impl-lib` so the crate root compiles. The full runner
//! (driver invocation, artifact emission per §7.3) is delivered by
//! `task:bench-impl-harness`.

pub mod gates;
pub mod repro;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Identifier for which benchmark driver produced (or will produce) a run.
///
/// Maps 1:1 to the driver subcommands in `engram-bench` CLI (design §7.1)
/// and to the `[run].driver` field in the reproducibility record (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Driver {
    /// LOCOMO multi-turn conversation memory benchmark (design §3.1).
    Locomo,
    /// LongMemEval long-context retrieval benchmark (design §3.2).
    Longmemeval,
    /// Cost harness — N=500 episode ingest corpus (design §3.3).
    Cost,
    /// Test-preservation harness — replays the v0.2 cargo-test suite
    /// against a migrated database (design §3.4).
    TestPreservation,
    /// Cognitive-feature regression harness — three-feature directional
    /// test (design §3.5).
    CognitiveRegression,
    /// Migration data-integrity harness (design §3.6).
    MigrationIntegrity,
}

/// Configuration handed to a driver at run start.
///
/// **Stub.** Full schema (paths, seeds, output directories, fixture roots,
/// LLM-scorer configuration) is delivered by `task:bench-impl-harness`
/// per design §7.2.
#[derive(Debug, Clone, Default)]
pub struct HarnessConfig {
    /// Output root for `benchmarks/runs/<timestamp>_<driver>_<sha>/`.
    /// Per design §6.2.
    pub output_root: Option<PathBuf>,
}

/// Errors raised by drivers and the harness runner.
///
/// **Stub.** The full error catalog (driver-specific failure modes,
/// fixture-missing, corpus-corruption, scorer timeout) is delivered by
/// `task:bench-impl-harness`.
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    /// Catch-all for unimplemented surfaces during scaffolding.
    #[error("benchmark error: {0}")]
    Other(String),
}

/// Driver trait per design §7.2.
///
/// Each benchmark suite (LOCOMO, LongMemEval, cost harness, …) implements
/// this trait. The harness runner invokes [`BenchDriver::run`] with a
/// [`HarnessConfig`] and expects a [`RunReport`] in return; the report
/// carries gate results plus a path to the emitted reproducibility record.
pub trait BenchDriver {
    /// Identifier of this driver — must match the `Driver` enum variant
    /// that maps to its CLI subcommand (design §7.1).
    fn name(&self) -> Driver;

    /// Execute the benchmark and return the run report.
    ///
    /// On error, drivers are still expected to emit partial artifacts
    /// (per design §7.3 "we got halfway and crashed is useful information").
    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError>;
}

/// One driver run's outcome — gates evaluated, artifacts emitted, summary
/// captured. Per design §7.2.
#[derive(Debug, Clone)]
pub struct RunReport {
    /// Which driver produced this report.
    pub driver: Driver,
    /// Path to the emitted `reproducibility.toml` (design §6.1, §6.2).
    pub record_path: PathBuf,
    /// Per-gate evaluation results.
    pub gates: Vec<gates::GateResult>,
    /// Driver-specific summary (the same JSON written to
    /// `<driver>_summary.json` per design §7.3).
    pub summary_json: serde_json::Value,
}

/// Run the full release-gate suite (all drivers, all gates).
///
/// **Stub.** Implemented by `task:bench-impl-harness` per design §7.2.
pub fn run_release_gate(_config: &HarnessConfig) -> Result<Vec<RunReport>, BenchError> {
    Err(BenchError::Other(
        "run_release_gate not yet implemented (task:bench-impl-harness)".into(),
    ))
}

/// Aggregate per-driver reports into a release decision.
///
/// **Stub.** Implemented by `task:bench-impl-harness` per design §7.2.
pub fn aggregate_release_decision(_reports: &[RunReport]) -> ReleaseDecision {
    ReleaseDecision::Block {
        failed_p0: vec!["aggregate_release_decision not yet implemented".into()],
    }
}

/// Final ship/no-ship decision after evaluating all gates across all drivers.
///
/// Per design §7.2:
///
/// - [`ReleaseDecision::Ship`] — all P0 pass, all P1 pass or justified.
/// - [`ReleaseDecision::Block`] — at least one P0 failed; ship is forbidden.
/// - [`ReleaseDecision::ConditionalShip`] — P0 all pass, but at least one
///   P1 failed and was overridden with a committed rationale.
#[derive(Debug, Clone)]
pub enum ReleaseDecision {
    /// Clean release — all P0 pass, all P1 pass or justified.
    Ship,
    /// At least one P0 gate failed.
    Block {
        /// GOAL IDs of P0 gates that failed.
        failed_p0: Vec<String>,
    },
    /// Some P1 gates failed but were overridden with rationales.
    ConditionalShip {
        /// Per-gate human-signed override records (design §4.4).
        overridden: Vec<Override>,
        /// Captured P1 rationales for the release notes.
        p1_rationales: Vec<Rationale>,
    },
}

/// Operator-signed override for a failed gate (design §4.4).
///
/// **Stub.** Full schema (operator identity, signature, gate ID, rationale
/// SHA) lands with `task:bench-impl-gates`.
#[derive(Debug, Clone)]
pub struct Override {
    /// GOAL ID of the gate being overridden, e.g. `"GOAL-5.6"`.
    pub gate: String,
    /// Operator who signed the override (design §6.1 `[override].operator`).
    pub operator: String,
    /// SHA-256 of the rationale file referenced from the reproducibility
    /// record (design §6.1 `[override].rationale_sha`).
    pub rationale_sha: String,
}

/// Captured rationale for a P1 override (design §4.4 — release notes input).
///
/// **Stub.** Lands with `task:bench-impl-gates`.
#[derive(Debug, Clone)]
pub struct Rationale {
    /// GOAL ID this rationale justifies.
    pub gate: String,
    /// Free-form human text — committed verbatim into release notes.
    pub text: String,
}
