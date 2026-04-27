//! `engram-bench` binary entry point — design §7.1.
//!
//! Single binary with subcommands; shared infrastructure (fixture
//! loading, gate evaluation, reproducibility record emission) lives
//! in the library crate. This file is **dispatch only** — every
//! subcommand defers to library code so the same logic is exercised
//! by both unit tests and the binary.
//!
//! ## Exit codes (design §7.1)
//!
//! - `0` — all gates the driver evaluated passed.
//! - `1` — at least one gate failed (Block decision).
//! - `2` — driver error (fixture missing, crash, unrecoverable).
//! - `3` — override used (signals to CI: do not auto-release).
//!
//! ## Subcommands
//!
//! Per design §7.1, six driver subcommands plus `release-gate`,
//! `explain`, and `clean-fixtures`. Drivers not yet implemented
//! return exit code 2 with a clear error message — `engram-bench
//! locomo` should never silently no-op.
//!
//! ## Determinism
//!
//! Subcommands forward their flags into [`engram_bench::HarnessConfig`]
//! verbatim. The seed flag is the *only* source of randomness; without
//! `--seed`, the default `42` is captured into the reproducibility
//! record so re-runs are bit-identical.

use clap::{Parser, Subcommand, ValueEnum};
use engram_bench::harness::gates::GateStatus;
use engram_bench::reporting::{
    diff_runs, render_diff, render_drilldown, render_summary_table, SummaryHeader,
};
use engram_bench::{
    aggregate_release_decision, BenchDriver, Driver, HarnessConfig, ReleaseDecision, RunReport,
};
use is_terminal::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

// ---------------------------------------------------------------------------
// Exit code constants (design §7.1).
// ---------------------------------------------------------------------------

/// All gates passed.
const EXIT_OK: u8 = 0;
/// At least one gate failed.
const EXIT_GATE_FAIL: u8 = 1;
/// Driver error (fixture missing, panic, unrecoverable).
const EXIT_DRIVER_ERROR: u8 = 2;
/// Override used — CI must not auto-release.
const EXIT_OVERRIDE: u8 = 3;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "engram-bench",
    version,
    about = "Engram v0.3 benchmark harness — design §7.1",
    long_about = "Single binary with subcommands. Each driver implements \
                  BenchDriver and emits a reproducibility.toml + per_query.jsonl \
                  + summary.json. See design §7.3 for output contract."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Override fixture root (default: benchmarks/fixtures/).
    #[arg(long, value_name = "PATH", global = true)]
    fixtures_dir: Option<PathBuf>,

    /// Override baseline root (default: benchmarks/baselines/).
    #[arg(long, value_name = "PATH", global = true)]
    baselines_dir: Option<PathBuf>,

    /// Run record destination (default: benchmarks/runs/).
    #[arg(long, value_name = "PATH", global = true)]
    output_dir: Option<PathBuf>,

    /// Stdout format.
    #[arg(long, value_enum, default_value_t = Format::Summary, global = true)]
    format: Format,

    /// PRNG seed captured into reproducibility record (§6.1 [run].seed).
    #[arg(long, default_value_t = 42, global = true)]
    seed: u64,

    /// Manual P0/P1 override — requires --rationale (§4.4).
    #[arg(long, value_name = "GOAL", global = true)]
    override_gate: Option<String>,

    /// Rationale file (required when --override-gate is set).
    #[arg(long, value_name = "PATH", global = true)]
    rationale: Option<PathBuf>,

    /// Verbose logging.
    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run LOCOMO suite (GOAL-5.1, 5.2).
    Locomo,
    /// Run LongMemEval suite (GOAL-5.3).
    Longmemeval,
    /// Run cost harness (N=500) (GOAL-5.4).
    Cost {
        /// Override N=500 (debug only).
        #[arg(long, default_value_t = 500)]
        n: usize,
    },
    /// Run v0.2 test-preservation replay (GOAL-5.5).
    TestPreservation,
    /// Run cognitive-feature regression suite (GOAL-5.6).
    CognitiveRegression,
    /// Run migration data-integrity suite (GOAL-5.7).
    MigrationIntegrity,
    /// Run ALL drivers in §4.3 order and emit a combined release decision.
    ReleaseGate,
    /// Drill-down on one gate from a previous run.
    Explain {
        /// GOAL identifier, e.g. `GOAL-5.1`.
        goal: String,
        /// Path to the run directory or reproducibility.toml.
        #[arg(long, value_name = "PATH")]
        run: PathBuf,
    },
    /// Print regression diff between two run directories.
    Diff {
        /// Prior run directory or repro.toml.
        #[arg(long, value_name = "PATH")]
        prior: PathBuf,
        /// Current run directory or repro.toml.
        #[arg(long, value_name = "PATH")]
        current: PathBuf,
    },
    /// Print summary of a previously-emitted run record.
    Show {
        /// Path to the run directory or reproducibility.toml.
        #[arg(value_name = "PATH")]
        run: PathBuf,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Summary,
    Json,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let result = run(cli);
    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("engram-bench: error: {}", err);
            // Print the full error chain for debuggability — driver errors
            // typically wrap an inner anyhow::Error or io::Error.
            let mut source = err.source();
            while let Some(s) = source {
                eprintln!("  caused by: {}", s);
                source = s.source();
            }
            ExitCode::from(EXIT_DRIVER_ERROR)
        }
    }
}

fn init_logging(verbose: bool) {
    use std::io::Write;
    let level = if verbose { "debug" } else { "info" };
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format(|buf, record| {
            writeln!(buf, "[{}] {}", record.level(), record.args())
        })
        .try_init();
}

fn build_config(cli: &Cli) -> HarnessConfig {
    let mut cfg = HarnessConfig::default();
    if let Some(p) = &cli.fixtures_dir {
        cfg.fixture_root = p.clone();
    }
    if let Some(p) = &cli.baselines_dir {
        cfg.baseline_root = p.clone();
    }
    if let Some(p) = &cli.output_dir {
        cfg.output_root = p.clone();
    }
    cfg.seed = cli.seed;
    cfg.rationale_file = cli.rationale.clone();
    // Note: cli.override_gate → HarnessConfig.override_gate is plumbed by
    // run_release_gate when needed. The Override struct requires the
    // GOAL + operator + rationale_sha tuple which is not available
    // until the rationale file has been read; that lives in the
    // release-gate path, not here.
    cfg
}

fn run(cli: Cli) -> Result<u8, Box<dyn std::error::Error>> {
    let cfg = build_config(&cli);
    let use_color = std::io::stdout().is_terminal();

    match &cli.command {
        Command::Locomo => run_single(Driver::Locomo, &cfg, &cli, use_color),
        Command::Longmemeval => run_single(Driver::Longmemeval, &cfg, &cli, use_color),
        Command::Cost { n: _ } => run_single(Driver::Cost, &cfg, &cli, use_color),
        Command::TestPreservation => run_single(Driver::TestPreservation, &cfg, &cli, use_color),
        Command::CognitiveRegression => run_single(Driver::CognitiveRegression, &cfg, &cli, use_color),
        Command::MigrationIntegrity => run_single(Driver::MigrationIntegrity, &cfg, &cli, use_color),
        Command::ReleaseGate => run_release(&cfg, &cli, use_color),
        Command::Explain { goal, run } => cmd_explain(goal, run, use_color),
        Command::Diff { prior, current } => cmd_diff(prior, current, use_color),
        Command::Show { run } => cmd_show(run, use_color),
    }
}

// ---------------------------------------------------------------------------
// Driver dispatch
// ---------------------------------------------------------------------------

/// Resolve a [`Driver`] enum to a concrete [`BenchDriver`] trait object.
///
/// Returns `None` for drivers whose implementation is not yet present;
/// the CLI converts that into an `EXIT_DRIVER_ERROR` rather than a
/// silent no-op (per design §4.4 "missing measurement is never PASS").
fn resolve_driver(driver: Driver) -> Option<Box<dyn BenchDriver>> {
    match driver {
        Driver::Locomo => Some(Box::new(engram_bench::drivers::locomo::LocomoDriver)),
        Driver::Longmemeval => Some(Box::new(
            engram_bench::drivers::longmemeval::LongMemEvalDriver,
        )),
        Driver::Cost => Some(Box::new(engram_bench::drivers::cost::CostDriver)),
        Driver::TestPreservation => Some(Box::new(
            engram_bench::drivers::test_preservation::TestPreservationDriver,
        )),
        // The remaining two drivers are skeletons — surface clearly.
        Driver::CognitiveRegression
        | Driver::MigrationIntegrity => None,
    }
}

fn run_single(
    driver: Driver,
    cfg: &HarnessConfig,
    cli: &Cli,
    use_color: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let bench: Box<dyn BenchDriver> = match resolve_driver(driver) {
        Some(b) => b,
        None => {
            eprintln!(
                "engram-bench: driver {:?} is not yet implemented — see \
                 task:bench-impl-driver-{} (v03-benchmarks build plan).",
                driver,
                driver_slug(driver)
            );
            return Ok(EXIT_DRIVER_ERROR);
        }
    };

    let report = bench.run(cfg)?;

    emit(&[report.clone()], cli.format, use_color)?;
    Ok(decide_exit_single(&report))
}

fn run_release(
    cfg: &HarnessConfig,
    cli: &Cli,
    use_color: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    // Order matches design §4.3: stage-1 cheap drivers first, then stage-2.
    let drivers: Vec<Box<dyn BenchDriver>> = [
        Driver::TestPreservation,
        Driver::CognitiveRegression,
        Driver::MigrationIntegrity,
        Driver::Cost,
        Driver::Locomo,
        Driver::Longmemeval,
    ]
    .into_iter()
    .filter_map(resolve_driver)
    .collect();

    let reports = engram_bench::run_release_gate(&drivers, cfg)?;
    let decision = aggregate_release_decision(&reports, &[]);
    emit(&reports, cli.format, use_color)?;

    Ok(match &decision {
        ReleaseDecision::Ship => EXIT_OK,
        ReleaseDecision::ConditionalShip { .. } => EXIT_OVERRIDE,
        ReleaseDecision::Block { .. } => EXIT_GATE_FAIL,
    })
}

fn decide_exit_single(report: &RunReport) -> u8 {
    let mut had_error = false;
    let mut had_fail = false;
    for g in &report.gates {
        match g.status {
            GateStatus::Pass => {}
            GateStatus::Error => had_error = true,
            GateStatus::Fail => had_fail = true,
        }
    }
    if had_error {
        EXIT_DRIVER_ERROR
    } else if had_fail {
        EXIT_GATE_FAIL
    } else {
        EXIT_OK
    }
}

// ---------------------------------------------------------------------------
// Output emission
// ---------------------------------------------------------------------------

fn emit(reports: &[RunReport], format: Format, use_color: bool) -> Result<(), Box<dyn std::error::Error>> {
    match format {
        Format::Summary => {
            let header = synthetic_header(reports);
            // For single-driver runs, build a verdict the same way release-gate
            // does so the summary line is always populated.
            let decision = aggregate_release_decision(reports, &[]);
            let table = render_summary_table(&header, reports, &decision, use_color);
            print!("{}", table);
        }
        Format::Json => {
            // Emit the structured form — gates + summary_json per driver.
            let payload: Vec<_> = reports
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "driver": format!("{:?}", r.driver),
                        "record_path": r.record_path,
                        "gates": r.gates,
                        "summary": r.summary_json,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn synthetic_header(reports: &[RunReport]) -> SummaryHeader {
    // The runner doesn't yet plumb actual build identity from cargo env
    // into reports; we surface the artifact path of the first report so
    // operators can navigate to it.
    let artifact = reports
        .first()
        .and_then(|r| r.record_path.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no artifact path)".to_string());
    SummaryHeader {
        build_label: format!("engram @ {}", git_short_sha()),
        toolchain_label: rustc_version_hint(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: chrono::Utc::now().to_rfc3339(),
        duration: "—".to_string(),
        artifact_path: artifact,
    }
}

fn git_short_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn rustc_version_hint() -> String {
    // Static hint — embedding the actual rustc version requires a
    // build.rs. Acceptable for now; the reproducibility record (§6.1)
    // is the canonical source for build identity.
    format!(
        "rustc {}-{}",
        std::env::consts::ARCH,
        std::env::consts::OS,
    )
}

// ---------------------------------------------------------------------------
// `explain` / `diff` / `show`
// ---------------------------------------------------------------------------

/// Convert a `(goal, GateRow)` from a reproducibility record into the
/// in-memory [`GateResult`] surface that reporting uses. Comparator is
/// not pinned in the record (per §6.1 schema), so we re-derive it from
/// the canonical registry — an unknown goal degrades to `Equal` rather
/// than crashing, since `render_drilldown` will refuse the unknown goal
/// anyway.
fn row_to_result(
    goal: &str,
    row: &engram_bench::harness::repro::GateRow,
) -> engram_bench::harness::gates::GateResult {
    use engram_bench::harness::gates::{registry, Comparator, GateResult};
    let comparator = registry()
        .iter()
        .find(|d| d.goal == goal)
        .map(|d| d.comparator)
        .unwrap_or(Comparator::Equal);
    GateResult {
        goal: goal.to_string(),
        metric_key: registry()
            .iter()
            .find(|d| d.goal == goal)
            .map(|d| d.metric_key.to_string())
            .unwrap_or_else(|| format!("unknown:{}", goal)),
        priority: row.priority,
        comparator,
        threshold: Some(row.threshold),
        observed: Some(row.metric),
        status: row.status,
        message: format!("{} {} {}", row.metric, comparator_str(comparator), row.threshold),
    }
}

fn comparator_str(c: engram_bench::harness::gates::Comparator) -> &'static str {
    use engram_bench::harness::gates::Comparator;
    match c {
        Comparator::GreaterOrEqual => "≥",
        Comparator::LessOrEqual => "≤",
        Comparator::Equal => "=",
        Comparator::Compound => "∘",
    }
}

fn driver_str_to_enum(s: &str) -> Driver {
    match s {
        "locomo" => Driver::Locomo,
        "longmemeval" => Driver::Longmemeval,
        "cost" => Driver::Cost,
        "test-preservation" => Driver::TestPreservation,
        "cognitive-regression" => Driver::CognitiveRegression,
        "migration-integrity" => Driver::MigrationIntegrity,
        _ => Driver::Locomo, // best-effort default; report-rendering only.
    }
}

fn cmd_explain(
    goal: &str,
    run_path: &PathBuf,
    use_color: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let path = resolve_repro_path(run_path)?;
    let record = engram_bench::harness::repro::ReproRecord::read_toml(&path)?;
    let row = record
        .gates
        .get(goal)
        .ok_or_else(|| format!("gate {} not found in run record", goal))?;
    let result = row_to_result(goal, row);

    let out = render_drilldown(goal, &result, None, &[], use_color)
        .ok_or_else(|| format!("gate {} is not in the canonical registry", goal))?;
    print!("{}", out);
    Ok(EXIT_OK)
}

fn cmd_diff(
    prior: &PathBuf,
    current: &PathBuf,
    use_color: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let prior_rec =
        engram_bench::harness::repro::ReproRecord::read_toml(&resolve_repro_path(prior)?)?;
    let current_rec =
        engram_bench::harness::repro::ReproRecord::read_toml(&resolve_repro_path(current)?)?;
    let prior_results: Vec<_> = prior_rec
        .gates
        .iter()
        .map(|(g, r)| row_to_result(g, r))
        .collect();
    let current_results: Vec<_> = current_rec
        .gates
        .iter()
        .map(|(g, r)| row_to_result(g, r))
        .collect();
    let diff = diff_runs(&prior_results, &current_results);
    print!("{}", render_diff(&diff, use_color));
    let any_regression = diff.iter().any(|r| {
        matches!(r.current_status, GateStatus::Fail | GateStatus::Error)
            && r.prior_status == GateStatus::Pass
    });
    Ok(if any_regression { EXIT_GATE_FAIL } else { EXIT_OK })
}

fn cmd_show(run_path: &PathBuf, use_color: bool) -> Result<u8, Box<dyn std::error::Error>> {
    let path = resolve_repro_path(run_path)?;
    let record = engram_bench::harness::repro::ReproRecord::read_toml(&path)?;
    let gates: Vec<_> = record
        .gates
        .iter()
        .map(|(g, r)| row_to_result(g, r))
        .collect();
    let report = RunReport {
        driver: driver_str_to_enum(&record.run.driver),
        record_path: path,
        gates,
        summary_json: serde_json::json!({}),
    };
    let header = synthetic_header(std::slice::from_ref(&report));
    let decision = aggregate_release_decision(std::slice::from_ref(&report), &[]);
    print!("{}", render_summary_table(&header, &[report], &decision, use_color));
    Ok(EXIT_OK)
}

fn resolve_repro_path(p: &PathBuf) -> std::io::Result<PathBuf> {
    if p.is_dir() {
        Ok(p.join("reproducibility.toml"))
    } else {
        Ok(p.clone())
    }
}

fn driver_slug(driver: Driver) -> &'static str {
    match driver {
        Driver::Locomo => "locomo",
        Driver::Longmemeval => "longmemeval",
        Driver::Cost => "cost",
        Driver::TestPreservation => "test-preservation",
        Driver::CognitiveRegression => "cognitive-regression",
        Driver::MigrationIntegrity => "migration-integrity",
    }
}
