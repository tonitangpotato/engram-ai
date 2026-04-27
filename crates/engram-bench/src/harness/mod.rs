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

/// Configuration handed to drivers and the harness runner at run start.
///
/// Per design §7.2 (CLI/runner inputs) and §6.1 (paths captured into
/// reproducibility record). Drivers receive an immutable reference and
/// must not mutate global state outside `output_root`.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Root containing canonical fixtures (LOCOMO conversations,
    /// LongMemEval corpora, etc.). Drivers resolve relative paths
    /// (e.g. `locomo/conversations.jsonl`) under this directory.
    /// Per design §6.1 `[fixtures]`.
    pub fixture_root: PathBuf,

    /// Root containing baseline TOML files (one per driver, design §5).
    /// The baselines loader (`baselines::load`) reads from here.
    pub baseline_root: PathBuf,

    /// Output root for the run directory
    /// `benchmarks/runs/<timestamp>_<driver>_<sha>/`. Per design §6.2.
    pub output_root: PathBuf,

    /// Maximum drivers running concurrently within a single stage.
    /// Per design §4.3 — stage-1 drivers (cheap) parallelize, stage-2
    /// drivers (expensive scorer-heavy) typically serialize. `0` means
    /// "use number of physical cores"; `1` forces fully serial.
    pub parallel_limit: usize,

    /// PRNG seed captured into the reproducibility record (§6.1
    /// `[run].seed`). Drivers that sample (e.g. cost-harness episode
    /// selection) MUST consume this seed for determinism.
    pub seed: u64,

    /// Optional gate override — populated when an operator has signed
    /// off on a P1 (or, exceptionally, P0) failure with a committed
    /// rationale. Aggregation reads this in `aggregate_release_decision`.
    /// Per design §4.4.
    pub override_gate: Option<Override>,

    /// Path to the rationale file referenced by `override_gate`. Read
    /// once; SHA-256 captured into the reproducibility record's
    /// `[override].rationale_sha`. Per design §4.4 / §6.1.
    pub rationale_file: Option<PathBuf>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            fixture_root: PathBuf::from("benchmarks/fixtures"),
            baseline_root: PathBuf::from("benchmarks/baselines"),
            output_root: PathBuf::from("benchmarks/runs"),
            parallel_limit: 0,
            seed: 0xE03_BE5C_5EED_0000,
            override_gate: None,
            rationale_file: None,
        }
    }
}

/// Errors raised by drivers and the harness runner.
///
/// Per design §4.4 ("Failure semantics") and §7.2. Each variant maps
/// to a distinct `[run].status` outcome in the reproducibility record:
/// any error variant ⇒ `status = "error"` (never silently treated as
/// pass).
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    /// A required fixture file (corpus, conversation set) was not found
    /// under `HarnessConfig::fixture_root`. Per design §6.1
    /// `[fixtures]` — every fixture in the record must exist on disk.
    #[error("fixture missing: {0}")]
    FixtureMissing(PathBuf),

    /// A fixture file's SHA-256 did not match the expected hash recorded
    /// in `[fixtures].<name>.sha256`. Indicates the fixture was modified
    /// after the baseline was captured — the run is non-reproducible.
    /// Per design §6.1 / §6.3.
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// Fixture file whose hash diverged.
        path: PathBuf,
        /// Hex-encoded SHA-256 from the baseline / record.
        expected: String,
        /// Hex-encoded SHA-256 we computed from disk just now.
        actual: String,
    },

    /// The required baseline TOML for this driver was not present under
    /// `HarnessConfig::baseline_root`. Per design §4.4 — missing baseline
    /// is `ERROR`, never `PASS`.
    #[error("baseline missing for driver `{driver}` at {path}")]
    BaselineMissing {
        /// Driver name (e.g. `"locomo"`) whose baseline was sought.
        driver: String,
        /// Resolved path that did not exist.
        path: PathBuf,
    },

    /// A driver's `run` panicked. The harness's parallel runner catches
    /// `std::panic::catch_unwind` payloads and reports them through this
    /// variant rather than propagating the panic.
    #[error("driver `{name}` panicked: {msg}")]
    DriverPanic {
        /// Name of the driver whose thread panicked.
        name: String,
        /// Best-effort string captured from the panic payload.
        msg: String,
    },

    /// A driver could not start because a prerequisite stage produced an
    /// error. Stage-2 drivers carry this when stage-1 fails fatally.
    /// Per design §4.3 (DAG) / §4.4 (failure semantics).
    #[error("blocked: prerequisite stage `{0}` failed")]
    BlockedBy(String),

    /// I/O error reading fixtures, writing artifacts, computing hashes.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Catch-all for failure modes not yet promoted to a typed variant.
    /// Drivers may also return this for suite-specific errors that don't
    /// fit cleanly into the categories above.
    #[error("{0}")]
    Other(String),
}

/// Driver execution stage — controls scheduling order in the harness DAG.
///
/// Per design §4.3 ("Gate evaluation order"). Stage-1 drivers are cheap
/// (in-memory only, deterministic, no LLM scorer) and run in parallel.
/// Stage-2 drivers depend on stage-1 results being green, are typically
/// LLM-scorer-heavy, and run after stage-1 completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Stage 1 — fixture-only, deterministic, no external services.
    /// Examples: cost harness, migration-integrity, test-preservation.
    Stage1,
    /// Stage 2 — depends on stage-1 success; may invoke LLM scorers.
    /// Examples: LOCOMO, LongMemEval, cognitive-regression.
    Stage2,
}

/// Cost tier — orders drivers within a stage so the cheapest run first.
///
/// Per design §4.3. The parallel runner sorts drivers within each stage
/// by `cost_tier()` ascending, so a `Cheap` driver that fails fast
/// surfaces its error before any `Expensive` driver burns scorer budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CostTier {
    /// Sub-second per run; cargo-test-style suites.
    Cheap,
    /// Seconds-to-low-minutes; deterministic in-memory ingest/score.
    Medium,
    /// Minutes; LLM-scorer-heavy runs (LOCOMO, LongMemEval).
    Expensive,
}

/// Driver trait per design §7.2.
///
/// Each benchmark suite (LOCOMO, LongMemEval, cost harness, …) implements
/// this trait. The harness runner invokes [`BenchDriver::run`] with a
/// [`HarnessConfig`] and expects a [`RunReport`] in return; the report
/// carries gate results plus a path to the emitted reproducibility record.
pub trait BenchDriver: Send + Sync {
    /// Identifier of this driver — must match the `Driver` enum variant
    /// that maps to its CLI subcommand (design §7.1).
    fn name(&self) -> Driver;

    /// Execution stage (1 = cheap/parallel, 2 = expensive/post-stage1).
    /// Per design §4.3.
    fn stage(&self) -> Stage;

    /// Cost tier — sort key within a stage so cheap drivers fail fast.
    /// Per design §4.3.
    fn cost_tier(&self) -> CostTier;

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

/// Run the full release-gate suite: invokes every driver in `drivers`
/// via [`run_drivers_parallel`], then materializes any [`BenchError`]
/// into an ERROR-status [`RunReport`] so a downstream caller (CLI,
/// reporting layer, [`aggregate_release_decision`]) can reason about
/// failures without losing the per-driver provenance.
///
/// Per design §4.4 ("Failure semantics"):
/// - A driver error is **never** silently dropped.
/// - A driver error is **never** treated as PASS.
/// - The returned vector has exactly `drivers.len()` entries, in the
///   same order as `drivers`.
///
/// Errors that prevent the harness itself from starting (e.g. an I/O
/// failure preparing the run directory) are returned as `Err`. Once
/// the harness is running, every individual driver outcome — success
/// or failure — is captured inside the `Vec<RunReport>` so the caller
/// can render a complete report.
pub fn run_release_gate(
    drivers: &[Box<dyn BenchDriver>],
    config: &HarnessConfig,
) -> Result<Vec<RunReport>, BenchError> {
    let raw = run_drivers_parallel(drivers, config);

    let mut reports = Vec::with_capacity(raw.len());
    for (driver, outcome) in drivers.iter().zip(raw.into_iter()) {
        match outcome {
            Ok(report) => reports.push(report),
            Err(err) => {
                // Materialize the error as an ERROR-status RunReport
                // with a single synthetic GateResult so reporting and
                // aggregation can attribute the failure to this
                // driver. Per design §4.4: missing/null/error => ERROR,
                // never PASS.
                reports.push(RunReport {
                    driver: driver.name(),
                    record_path: PathBuf::new(),
                    gates: vec![gates::GateResult {
                        goal: format!("driver:{:?}", driver.name()),
                        metric_key: format!("driver:{:?}.error", driver.name()),
                        priority: gates::Priority::P0,
                        comparator: gates::Comparator::Eq,
                        threshold: None,
                        observed: None,
                        status: gates::GateStatus::Error,
                        message: err.to_string(),
                    }],
                    summary_json: serde_json::json!({
                        "error": err.to_string(),
                        "driver": format!("{:?}", driver.name()),
                    }),
                });
            }
        }
    }

    Ok(reports)
}

/// Aggregate per-driver run reports into a single ship/no-ship decision.
///
/// Per design §4.4 ("Failure semantics") and §7.2:
///
/// 1. **ANY ERROR ⇒ Block.** A `GateStatus::Error` (driver panic,
///    fixture mismatch, missing baseline, etc.) means we don't know
///    the truth, and "don't know" is never PASS. This holds regardless
///    of priority and is NOT eligible for override — operators must
///    fix the underlying issue, not paper over it.
///
/// 2. **P0 fail ⇒ Block** (unless an `Override` for that exact GOAL is
///    in `overrides`; signed-rationale path per §4.4. Even then this is
///    `ConditionalShip`, never `Ship`).
///
/// 3. **P1 fail ⇒ ConditionalShip** if every failed P1 has an
///    `Override` with a rationale; otherwise `Block`. P1 failures are
///    quality regressions — they require a documented decision to
///    accept, not a silent pass.
///
/// 4. **P2 fail ⇒ Note.** P2 gates are observability/efficiency hints
///    that get logged into release notes but never block the ship.
///
/// 5. **All-pass ⇒ Ship.**
///
/// The `overrides` slice typically comes from operator-signed
/// rationale files committed alongside the release; pass `&[]` when no
/// overrides apply.
pub fn aggregate_release_decision(
    reports: &[RunReport],
    overrides: &[Override],
) -> ReleaseDecision {
    use gates::{GateStatus, Priority};

    let mut errors: Vec<String> = Vec::new();
    let mut failed_p0: Vec<String> = Vec::new();
    let mut failed_p1: Vec<String> = Vec::new();

    for report in reports {
        for gate in &report.gates {
            match gate.status {
                GateStatus::Pass => {}
                GateStatus::Error => errors.push(gate.goal.clone()),
                GateStatus::Fail => match gate.priority {
                    Priority::P0Hard => failed_p0.push(gate.goal.clone()),
                    Priority::P0Expectation => failed_p1.push(gate.goal.clone()),
                    Priority::P1Advisory => { /* advisory: notes, not blockers */ }
                },
            }
        }
    }

    // Rule 1: ANY error blocks — not eligible for override.
    if !errors.is_empty() {
        return ReleaseDecision::Block { failed_p0: errors };
    }

    // Rule 2: any P0 fail without a matching Override blocks.
    let unoverridden_p0: Vec<String> = failed_p0
        .iter()
        .filter(|goal| !overrides.iter().any(|o| &o.gate == *goal))
        .cloned()
        .collect();
    if !unoverridden_p0.is_empty() {
        return ReleaseDecision::Block {
            failed_p0: unoverridden_p0,
        };
    }

    // Rule 3: P1 fails — require override+rationale for ConditionalShip.
    let unoverridden_p1: Vec<String> = failed_p1
        .iter()
        .filter(|goal| !overrides.iter().any(|o| &o.gate == *goal))
        .cloned()
        .collect();
    if !unoverridden_p1.is_empty() {
        return ReleaseDecision::Block {
            failed_p0: unoverridden_p1,
        };
    }

    // If P0 was overridden OR P1 was overridden → ConditionalShip.
    let p0_overridden_here: Vec<&Override> = overrides
        .iter()
        .filter(|o| failed_p0.iter().any(|g| g == &o.gate))
        .collect();
    let p1_overridden_here: Vec<&Override> = overrides
        .iter()
        .filter(|o| failed_p1.iter().any(|g| g == &o.gate))
        .collect();

    if !p0_overridden_here.is_empty() || !p1_overridden_here.is_empty() {
        let overridden: Vec<Override> = p0_overridden_here
            .iter()
            .chain(p1_overridden_here.iter())
            .map(|o| (*o).clone())
            .collect();
        let p1_rationales: Vec<Rationale> = p1_overridden_here
            .iter()
            .map(|o| Rationale {
                gate: o.gate.clone(),
                text: format!(
                    "Overridden by {} (rationale_sha={})",
                    o.operator, o.rationale_sha
                ),
            })
            .collect();
        return ReleaseDecision::ConditionalShip {
            overridden,
            p1_rationales,
        };
    }

    // Rule 5: nothing failed (or only P2 noted) — ship clean.
    ReleaseDecision::Ship
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

// ---------------------------------------------------------------------------
// Fixture verification + fresh database construction
// (sub-task bench-impl-harness-3 — design §6.1 fixtures, §3 driver setup)
// ---------------------------------------------------------------------------

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Verify that the file at `path` has the expected SHA-256 hash.
///
/// Per design §6.1 — every fixture entry in the reproducibility record
/// carries a `sha256` field, and a mismatch means the fixture was
/// modified after the baseline was captured. Such runs are not
/// reproducible, so the harness rejects them via
/// [`BenchError::ChecksumMismatch`] before any driver work begins.
///
/// `expected_hex` is a 64-character lowercase hexadecimal string. Case
/// is normalized internally, but length is enforced as exactly 64.
pub fn verify_fixture_sha(path: &Path, expected_hex: &str) -> Result<(), BenchError> {
    if !path.exists() {
        return Err(BenchError::FixtureMissing(path.to_path_buf()));
    }
    let bytes = fs::read(path).map_err(BenchError::IoError)?;
    let actual = Sha256::digest(&bytes);
    let actual_hex = hex::encode(actual);
    let expected_lc = expected_hex.to_ascii_lowercase();
    if actual_hex != expected_lc {
        return Err(BenchError::ChecksumMismatch {
            path: path.to_path_buf(),
            expected: expected_lc,
            actual: actual_hex,
        });
    }
    Ok(())
}

/// Build a fresh, empty, in-memory `engramai::Memory` for a single
/// driver run.
///
/// Per design §3 — drivers MUST start from an empty store; persistent
/// state across runs would corrupt reproducibility. Uses SQLite's
/// `:memory:` URI so no file is written; literature defaults are
/// applied via `MemoryConfig::default()` (engramai handles None as
/// "use defaults"). The returned `Memory` is moved to the driver.
pub fn fresh_in_memory_db() -> Result<engramai::Memory, BenchError> {
    engramai::Memory::new(":memory:", None)
        .map_err(|e| BenchError::Other(format!("fresh_in_memory_db failed: {e}")))
}

#[cfg(test)]
mod fixture_db_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn verify_fixture_sha_matches() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"hello bench").expect("write");
        let expected = hex::encode(Sha256::digest(b"hello bench"));
        verify_fixture_sha(tmp.path(), &expected).expect("sha should match");
    }

    #[test]
    fn verify_fixture_sha_mismatch_reports_both_hashes() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        tmp.write_all(b"contents A").expect("write");
        let wrong = hex::encode(Sha256::digest(b"contents B"));
        let err = verify_fixture_sha(tmp.path(), &wrong).expect_err("must fail");
        match err {
            BenchError::ChecksumMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, wrong);
                assert_ne!(actual, expected);
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_fixture_sha_missing_file() {
        let nonexistent = std::path::PathBuf::from("/tmp/__engram_bench_does_not_exist__.bin");
        let err = verify_fixture_sha(&nonexistent, &"0".repeat(64))
            .expect_err("must fail on missing file");
        assert!(matches!(err, BenchError::FixtureMissing(_)));
    }
}

// ---------------------------------------------------------------------------
// Parallel driver runner (sub-task bench-impl-harness-4 — design §4.3)
// ---------------------------------------------------------------------------

use std::panic::{self, AssertUnwindSafe};

/// Execute a heterogeneous set of drivers respecting the stage DAG.
///
/// Per design §4.3:
/// - Drivers are partitioned by [`Stage`]: all stage-1 drivers run
///   before any stage-2 driver starts.
/// - Within a stage, drivers are sorted by [`CostTier`] ascending and
///   then run in parallel up to `cfg.parallel_limit` concurrent
///   drivers (`0` = number of physical cores).
/// - If any stage-1 driver returns an error, every stage-2 driver is
///   short-circuited to [`BenchError::BlockedBy`] without invoking its
///   `run()` (per design §4.4 "Failure semantics" — never silently
///   skip, always materialize as ERROR).
/// - A driver panic is caught with `panic::catch_unwind` and returned
///   as [`BenchError::DriverPanic`]; one bad driver does NOT take down
///   sibling drivers in the same stage.
///
/// Returns one `Result` per input driver, in the same order as `drivers`.
pub fn run_drivers_parallel(
    drivers: &[Box<dyn BenchDriver>],
    cfg: &HarnessConfig,
) -> Vec<Result<RunReport, BenchError>> {
    // Index-preserving partition: we need to return results in the
    // SAME order as `drivers`, regardless of stage/cost-tier ordering
    // used internally for scheduling.
    let mut indices_stage1: Vec<usize> = (0..drivers.len())
        .filter(|&i| drivers[i].stage() == Stage::Stage1)
        .collect();
    let mut indices_stage2: Vec<usize> = (0..drivers.len())
        .filter(|&i| drivers[i].stage() == Stage::Stage2)
        .collect();

    // Sort by cost-tier ascending so cheap drivers fail fast.
    indices_stage1.sort_by_key(|&i| drivers[i].cost_tier());
    indices_stage2.sort_by_key(|&i| drivers[i].cost_tier());

    // Output buffer indexed parallel to `drivers`.
    let mut out: Vec<Option<Result<RunReport, BenchError>>> =
        (0..drivers.len()).map(|_| None).collect();

    let parallel = if cfg.parallel_limit == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    } else {
        cfg.parallel_limit
    };

    // Run stage 1.
    let stage1_results = run_one_stage(drivers, &indices_stage1, cfg, parallel);
    let mut any_stage1_failed = false;
    for (idx, res) in indices_stage1.iter().zip(stage1_results.into_iter()) {
        if res.is_err() {
            any_stage1_failed = true;
        }
        out[*idx] = Some(res);
    }

    // Stage 2: short-circuit if stage 1 failed.
    if any_stage1_failed {
        for &idx in &indices_stage2 {
            out[idx] = Some(Err(BenchError::BlockedBy("stage1".to_string())));
        }
    } else {
        let stage2_results = run_one_stage(drivers, &indices_stage2, cfg, parallel);
        for (idx, res) in indices_stage2.iter().zip(stage2_results.into_iter()) {
            out[*idx] = Some(res);
        }
    }

    out.into_iter()
        .map(|slot| slot.expect("every slot filled by stage 1 or stage 2 logic"))
        .collect()
}

/// Run all drivers at the given indices in parallel chunks of size
/// `parallel`. Catches panics. Returns results in the same order as
/// `indices`.
fn run_one_stage(
    drivers: &[Box<dyn BenchDriver>],
    indices: &[usize],
    cfg: &HarnessConfig,
    parallel: usize,
) -> Vec<Result<RunReport, BenchError>> {
    let mut results: Vec<Option<Result<RunReport, BenchError>>> =
        (0..indices.len()).map(|_| None).collect();

    // Process in chunks of `parallel` so we never spawn more than the
    // configured concurrency limit at once.
    for chunk in indices.chunks(parallel.max(1)) {
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(chunk.len());
            for &driver_idx in chunk {
                let driver = &drivers[driver_idx];
                let driver_name = driver.name();
                let h = scope.spawn(move || -> Result<RunReport, BenchError> {
                    let outcome =
                        panic::catch_unwind(AssertUnwindSafe(|| driver.run(cfg)));
                    match outcome {
                        Ok(res) => res,
                        Err(payload) => {
                            let msg = panic_payload_to_string(&payload);
                            Err(BenchError::DriverPanic {
                                name: format!("{driver_name:?}"),
                                msg,
                            })
                        }
                    }
                });
                handles.push(h);
            }
            for (local_idx, h) in handles.into_iter().enumerate() {
                // .join() panics only if the child panicked AND we
                // didn't catch it with catch_unwind, which we do
                // above; so this is effectively infallible. Map any
                // residual to BenchError just in case.
                let global_local = chunk_local_to_results_index(indices, chunk, local_idx);
                let res = h.join().unwrap_or_else(|_| {
                    Err(BenchError::Other(
                        "thread join failed (uncaught panic)".into(),
                    ))
                });
                results[global_local] = Some(res);
            }
        });
    }

    results
        .into_iter()
        .map(|slot| {
            slot.expect("every chunk filled its slots")
        })
        .collect()
}

/// Map a `(chunk, local_idx)` pair back to its position within
/// `indices`. Used because we process `indices` in chunks but still
/// want a flat result vector aligned with `indices`.
fn chunk_local_to_results_index(
    indices: &[usize],
    chunk: &[usize],
    local_idx: usize,
) -> usize {
    // Find where `chunk` starts inside `indices` by reference
    // arithmetic: `chunk` is a sub-slice of `indices` (from
    // `chunks()`), so its start offset is well-defined.
    let chunk_start = unsafe {
        chunk.as_ptr().offset_from(indices.as_ptr()) as usize
    };
    chunk_start + local_idx
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod parallel_runner_tests {
    use super::*;

    struct OkDriver {
        n: Driver,
        s: Stage,
        c: CostTier,
    }
    impl BenchDriver for OkDriver {
        fn name(&self) -> Driver {
            self.n
        }
        fn stage(&self) -> Stage {
            self.s
        }
        fn cost_tier(&self) -> CostTier {
            self.c
        }
        fn run(&self, _cfg: &HarnessConfig) -> Result<RunReport, BenchError> {
            Ok(RunReport {
                driver: self.n,
                record_path: std::path::PathBuf::from("/dev/null"),
                gates: vec![],
                summary_json: serde_json::json!({}),
            })
        }
    }

    struct PanicDriver {
        n: Driver,
        s: Stage,
        c: CostTier,
    }
    impl BenchDriver for PanicDriver {
        fn name(&self) -> Driver {
            self.n
        }
        fn stage(&self) -> Stage {
            self.s
        }
        fn cost_tier(&self) -> CostTier {
            self.c
        }
        fn run(&self, _cfg: &HarnessConfig) -> Result<RunReport, BenchError> {
            panic!("driver panicked on purpose")
        }
    }

    struct FailDriver {
        n: Driver,
        s: Stage,
        c: CostTier,
    }
    impl BenchDriver for FailDriver {
        fn name(&self) -> Driver {
            self.n
        }
        fn stage(&self) -> Stage {
            self.s
        }
        fn cost_tier(&self) -> CostTier {
            self.c
        }
        fn run(&self, _cfg: &HarnessConfig) -> Result<RunReport, BenchError> {
            Err(BenchError::Other("intentional".into()))
        }
    }

    #[test]
    fn driver_panic_caught_and_returned_as_error() {
        let cfg = HarnessConfig::default();
        let drivers: Vec<Box<dyn BenchDriver>> = vec![
            Box::new(OkDriver {
                n: Driver::Cost,
                s: Stage::Stage1,
                c: CostTier::Cheap,
            }),
            Box::new(PanicDriver {
                n: Driver::TestPreservation,
                s: Stage::Stage1,
                c: CostTier::Cheap,
            }),
        ];
        let results = run_drivers_parallel(&drivers, &cfg);
        assert!(results[0].is_ok(), "ok driver should succeed");
        assert!(matches!(
            results[1],
            Err(BenchError::DriverPanic { .. })
        ));
    }

    #[test]
    fn stage2_blocked_when_stage1_fails() {
        let cfg = HarnessConfig::default();
        let drivers: Vec<Box<dyn BenchDriver>> = vec![
            Box::new(FailDriver {
                n: Driver::Cost,
                s: Stage::Stage1,
                c: CostTier::Cheap,
            }),
            Box::new(OkDriver {
                n: Driver::Locomo,
                s: Stage::Stage2,
                c: CostTier::Expensive,
            }),
        ];
        let results = run_drivers_parallel(&drivers, &cfg);
        assert!(results[0].is_err(), "stage1 fails as constructed");
        assert!(
            matches!(results[1], Err(BenchError::BlockedBy(ref s)) if s == "stage1"),
            "stage2 must be blocked, got {:?}",
            results[1]
        );
    }

    #[test]
    fn stage_ordering_respects_stage1_before_stage2() {
        // Sanity: stage 2 ok driver runs when stage 1 ok.
        let cfg = HarnessConfig::default();
        let drivers: Vec<Box<dyn BenchDriver>> = vec![
            Box::new(OkDriver {
                n: Driver::Cost,
                s: Stage::Stage1,
                c: CostTier::Cheap,
            }),
            Box::new(OkDriver {
                n: Driver::Locomo,
                s: Stage::Stage2,
                c: CostTier::Expensive,
            }),
        ];
        let results = run_drivers_parallel(&drivers, &cfg);
        assert!(results.iter().all(Result::is_ok));
    }
}

#[cfg(test)]
mod release_gate_tests {
    use super::*;

    struct ErrDriver {
        n: Driver,
    }
    impl BenchDriver for ErrDriver {
        fn name(&self) -> Driver {
            self.n
        }
        fn stage(&self) -> Stage {
            Stage::Stage1
        }
        fn cost_tier(&self) -> CostTier {
            CostTier::Cheap
        }
        fn run(&self, _cfg: &HarnessConfig) -> Result<RunReport, BenchError> {
            Err(BenchError::FixtureMissing("/nope".into()))
        }
    }

    struct OkDriver2 {
        n: Driver,
    }
    impl BenchDriver for OkDriver2 {
        fn name(&self) -> Driver {
            self.n
        }
        fn stage(&self) -> Stage {
            Stage::Stage1
        }
        fn cost_tier(&self) -> CostTier {
            CostTier::Cheap
        }
        fn run(&self, _cfg: &HarnessConfig) -> Result<RunReport, BenchError> {
            Ok(RunReport {
                driver: self.n,
                record_path: PathBuf::from("/tmp/ok"),
                gates: vec![],
                summary_json: serde_json::json!({}),
            })
        }
    }

    #[test]
    fn driver_error_materializes_as_error_status_report() {
        let cfg = HarnessConfig::default();
        let drivers: Vec<Box<dyn BenchDriver>> = vec![
            Box::new(OkDriver2 {
                n: Driver::Cost,
            }),
            Box::new(ErrDriver {
                n: Driver::TestPreservation,
            }),
        ];
        let reports = run_release_gate(&drivers, &cfg).expect("orchestrator must not Err");
        assert_eq!(reports.len(), 2);
        // Order preserved (matches drivers slice order).
        assert_eq!(reports[0].driver, Driver::Cost);
        assert_eq!(reports[1].driver, Driver::TestPreservation);
        // Failure became an ERROR-status synthetic gate.
        assert_eq!(reports[1].gates.len(), 1);
        assert_eq!(reports[1].gates[0].status, gates::GateStatus::Error);
        assert!(reports[1].gates[0].message.contains("fixture missing"));
    }
}

#[cfg(test)]
mod aggregation_tests {
    use super::*;
    use gates::{Comparator, GateResult, GateStatus, Priority};

    fn gate(goal: &str, status: GateStatus, priority: Priority) -> GateResult {
        GateResult {
            goal: goal.to_string(),
            metric_key: format!("test.{}", goal),
            priority,
            comparator: Comparator::Ge,
            threshold: Some(0.0),
            observed: Some(0.0),
            status,
            message: String::new(),
        }
    }

    fn report(driver: Driver, gates_in: Vec<GateResult>) -> RunReport {
        RunReport {
            driver,
            record_path: PathBuf::new(),
            gates: gates_in,
            summary_json: serde_json::json!({}),
        }
    }

    #[test]
    fn all_pass_ships_clean() {
        let reports = vec![report(
            Driver::Cost,
            vec![
                gate("GOAL-1", GateStatus::Pass, Priority::P0),
                gate("GOAL-2", GateStatus::Pass, Priority::P1),
            ],
        )];
        match aggregate_release_decision(&reports, &[]) {
            ReleaseDecision::Ship => {}
            other => panic!("expected Ship, got {other:?}"),
        }
    }

    #[test]
    fn p0_fail_no_override_blocks() {
        let reports = vec![report(
            Driver::Cost,
            vec![gate("GOAL-P0-FAIL", GateStatus::Fail, Priority::P0)],
        )];
        match aggregate_release_decision(&reports, &[]) {
            ReleaseDecision::Block { failed_p0 } => {
                assert_eq!(failed_p0, vec!["GOAL-P0-FAIL".to_string()]);
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn p0_fail_with_override_yields_conditional_ship() {
        let reports = vec![report(
            Driver::Cost,
            vec![gate("GOAL-P0-FAIL", GateStatus::Fail, Priority::P0)],
        )];
        let ovs = vec![Override {
            gate: "GOAL-P0-FAIL".to_string(),
            operator: "potato".to_string(),
            rationale_sha: "deadbeef".to_string(),
        }];
        match aggregate_release_decision(&reports, &ovs) {
            ReleaseDecision::ConditionalShip { overridden, .. } => {
                assert_eq!(overridden.len(), 1);
                assert_eq!(overridden[0].gate, "GOAL-P0-FAIL");
            }
            other => panic!("expected ConditionalShip, got {other:?}"),
        }
    }

    #[test]
    fn p1_fail_no_override_blocks() {
        let reports = vec![report(
            Driver::Cost,
            vec![gate("GOAL-P1-FAIL", GateStatus::Fail, Priority::P1)],
        )];
        match aggregate_release_decision(&reports, &[]) {
            ReleaseDecision::Block { failed_p0 } => {
                // P1 failures without override flow into the same
                // "blocked, here are the offenders" channel.
                assert_eq!(failed_p0, vec!["GOAL-P1-FAIL".to_string()]);
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn p1_fail_with_override_yields_conditional_ship() {
        let reports = vec![report(
            Driver::Cost,
            vec![
                gate("GOAL-A", GateStatus::Pass, Priority::P0),
                gate("GOAL-P1-FAIL", GateStatus::Fail, Priority::P1),
            ],
        )];
        let ovs = vec![Override {
            gate: "GOAL-P1-FAIL".to_string(),
            operator: "potato".to_string(),
            rationale_sha: "feedface".to_string(),
        }];
        match aggregate_release_decision(&reports, &ovs) {
            ReleaseDecision::ConditionalShip {
                overridden,
                p1_rationales,
            } => {
                assert_eq!(overridden.len(), 1);
                assert_eq!(p1_rationales.len(), 1);
                assert_eq!(p1_rationales[0].gate, "GOAL-P1-FAIL");
            }
            other => panic!("expected ConditionalShip, got {other:?}"),
        }
    }

    #[test]
    fn p2_fail_does_not_block() {
        let reports = vec![report(
            Driver::Cost,
            vec![gate("GOAL-P2-NOTE", GateStatus::Fail, Priority::P2)],
        )];
        match aggregate_release_decision(&reports, &[]) {
            ReleaseDecision::Ship => {}
            other => panic!("expected Ship (P2 fails are notes), got {other:?}"),
        }
    }

    #[test]
    fn any_error_blocks_regardless_of_priority_or_override() {
        let reports = vec![report(
            Driver::Cost,
            vec![gate("GOAL-ERR", GateStatus::Error, Priority::P2)],
        )];
        // Even with an Override claiming to authorize this, ERROR
        // blocks: we don't know the truth, so we can't ship.
        let ovs = vec![Override {
            gate: "GOAL-ERR".to_string(),
            operator: "potato".to_string(),
            rationale_sha: "0".repeat(64),
        }];
        match aggregate_release_decision(&reports, &ovs) {
            ReleaseDecision::Block { failed_p0 } => {
                assert_eq!(failed_p0, vec!["GOAL-ERR".to_string()]);
            }
            other => panic!("expected Block on ERROR, got {other:?}"),
        }
    }
}
