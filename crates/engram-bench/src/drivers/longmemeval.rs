//! LongMemEval driver (design §3.2).
//!
//! Implements [`crate::harness::BenchDriver`] for the LongMemEval
//! long-context retrieval benchmark. Per design §3.2:
//!
//! 1. Load the LongMemEval question set from a pinned-SHA fixture
//!    (`fixtures/longmemeval/<sha>/queries.jsonl`).
//! 2. For each question's session: spin up a fresh in-memory engramai
//!    `Memory`, replay every session turn via `ingest_with_stats`, then
//!    pose the question through `Memory::graph_query_locked`.
//! 3. The top-1 result's `MemoryRecord::content` is the predicted
//!    answer. Score against the gold via [`crate::scorers::longmemeval`]
//!    (normalised exact-match, parity-test pending — see scorer module
//!    docs).
//! 4. Aggregate across all queries: scorer produces overall mean +
//!    per-category means.
//! 5. Read the v0.2 baseline from `baselines/v02.toml` (loaded once,
//!    immutable per design §5.1) and compute `delta_pp = (overall -
//!    v02_baseline) * 100.0` (in **percentage points**, as GOAL-5.3
//!    threshold is expressed in pp).
//! 6. Emit two artifacts under the run directory:
//!    `longmemeval_summary.json` and `longmemeval_per_query.jsonl`
//!    (design §3.2 outputs). The summary is the scorer's
//!    [`LongMemEvalSummary`] **plus** the driver-level fields
//!    `v02_baseline` and `delta_pp` per design §3.2.
//! 7. Evaluate gate **GOAL-5.3** (`delta_pp ≥ 15.0`) and return it in
//!    the `RunReport`.
//!
//! ## Stage / cost
//!
//! `Stage2 / Expensive`: depends on stage-1 success and is among the
//! most expensive drivers in the suite (per-session DB reset across
//! many sessions, plus per-question retrieval). Per design §4.3 it
//! runs after stage-1 and within stage-2 sorts last (`Expensive`),
//! alongside LOCOMO. The harness `Stage` enum splits drivers by
//! LLM-scorer use (Stage2 = uses scorer); this is a deliberate
//! departure from the design doc's migration-dependence split (where
//! LongMemEval lives in stage-1) — see `harness::mod` enum docs.
//!
//! ## Determinism (design §3.2 inherits §3.1)
//!
//! - Fusion weights pinned via `Memory::graph_query_locked`
//!   (`FusionConfig::locked()` internally).
//! - Query order = file order (line-by-line iteration of
//!   `queries.jsonl`).
//! - No randomness inside the driver itself; per-session DBs use
//!   SQLite `:memory:`.
//!
//! ## v0.2 baseline contract (design §5.1)
//!
//! The v0.2 LongMemEval baseline is **captured once** on the v0.2.2
//! tag and committed to `baselines/v02.toml`; it is **NOT rerun**
//! during a v0.3 benchmark (design §5.1: "v0.2 baseline is not rerun
//! during a v0.3 benchmark — committed once pre-development to prevent
//! gate-goalposting"). This driver only **reads** the file. If the
//! file is missing, the gate evaluates to `GateStatus::Error`, never
//! to a silent zero-baseline pass (per design §4.4 Level 1: missing →
//! ERROR).
//!
//! ## Wiring status
//!
//! Like LOCOMO, this driver consumes `Memory::graph_query_locked`,
//! which is currently an `Internal`-error stub in engramai
//! (`crates/engramai/src/retrieval/api.rs`). The driver threads a
//! `RetrievalError` through to a structured per-query failure
//! (`predicted = ""`, score `0.0`) rather than aborting the whole
//! run; integration tests are `#[ignore]`d until the real retrieval
//! pipeline lands. Unit tests in this file cover everything that does
//! not depend on the retrieval stub: fixture parsing, summary
//! aggregation, baseline loading, gate evaluation, artifact emission.
//!
//! ## Gate registry binding
//!
//! `task:bench-impl-gates` (2026-04-27) reconciled the registry
//! against the authoritative requirements. GOAL-5.3 is now bound to
//! `longmemeval.delta_pp ≥ 15.0`, which this driver emits directly.
//! The previously-populated `longmemeval.session_score` alias has
//! been removed — it existed only to keep an earlier (stale) GOAL-5.6
//! registry row from spuriously erroring. GOAL-5.6 is now bound to
//! `cognitive.regression_count` (a separate driver). The driver does
//! **not** invent gate definitions locally; it only emits metric
//! values. This mirrors the pattern documented in `cost.rs`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::baselines::{self, V02Baseline};
use crate::harness::gates::GateResult;
use crate::harness::{
    fresh_in_memory_db, verify_fixture_sha, BenchDriver, BenchError, CostTier, Driver,
    HarnessConfig, RunReport, Stage,
};
use crate::scorers::longmemeval::{
    LongMemEvalQuery, LongMemEvalScore, LongMemEvalScorer,
};

// ---------------------------------------------------------------------------
// Fixture schema (design §3.2 / §9.2 — JSONL-on-disk, parallels §3.1).
// ---------------------------------------------------------------------------

/// One LongMemEval question record as it appears in
/// `fixtures/longmemeval/<sha>/queries.jsonl` (one JSON object per
/// line). LongMemEval bundles each question with the session turns
/// required to answer it; the driver replays those turns into a fresh
/// `Memory` per question.
///
/// Field names are kept identical to LongMemEval's upstream JSON so a
/// future `version_pin` upgrade only has to touch this struct if
/// upstream renames anything.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LongMemEvalRecord {
    /// Stable question id from the LongMemEval dataset.
    pub question_id: String,
    /// LongMemEval question-type tag (e.g. `single-session-user`,
    /// `multi-session`, `temporal-reasoning`, `knowledge-update`).
    /// Preserved verbatim through the scorer for per-category
    /// aggregation.
    pub question_type: String,
    /// The natural-language question — fed verbatim to
    /// `GraphQuery::new(text)`.
    pub question: String,
    /// Gold answer string for scoring.
    pub answer: String,
    /// Ordered session turns — replayed in this order via
    /// `ingest_with_stats` BEFORE the question is posed. Each entry is
    /// one natural-language utterance / event / message.
    pub haystack_sessions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Source pin (`fixtures/longmemeval/source.toml`) — design §9.2 mirrors
// §9.1 version-pin discipline.
// ---------------------------------------------------------------------------

/// `fixtures/longmemeval/source.toml` schema. Pins the dataset to a
/// single upstream commit AND records the SHA-256 of every fixture file
/// we depend on. Any run with a missing or unparsable `source.toml` is
/// non-reproducible and rejected before any query is loaded.
#[derive(Debug, Clone, Deserialize)]
pub struct LongMemEvalSourcePin {
    /// Upstream LongMemEval repository commit SHA — recorded verbatim
    /// into the reproducibility record's `[dataset]` block (design
    /// §6.1).
    pub version_pin: String,
    /// Fixture entries: relative path → expected SHA-256 (hex).
    pub fixtures: BTreeMap<String, LongMemEvalFixturePin>,
}

/// One entry under `[fixtures.<name>]` in `source.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct LongMemEvalFixturePin {
    /// Path under `fixtures/longmemeval/<version_pin>/` (e.g.
    /// `queries.jsonl`).
    pub path: String,
    /// Expected SHA-256 of the file at `path`, hex-encoded (lowercase
    /// after normalisation in [`verify_fixture_sha`]).
    pub sha256: String,
}

// ---------------------------------------------------------------------------
// Driver-level summary (extends scorer's [`LongMemEvalSummary`] with
// the driver-owned fields per design §3.2 output contract).
// ---------------------------------------------------------------------------

/// Driver-level summary written to `longmemeval_summary.json`.
///
/// This composes the scorer-owned fields (`overall`, `by_category`,
/// `n_queries`) with the driver-owned fields (`v02_baseline`,
/// `delta_pp`) per design §3.2:
///
/// ```text
/// {
///   "overall": <scorer mean>,
///   "by_category": { ... },
///   "n_queries": <int>,
///   "v02_baseline": <baseline overall, e.g. 0.45>,
///   "delta_pp": <(overall - v02_baseline) * 100.0>
/// }
/// ```
///
/// `v02_baseline` and `delta_pp` are `Option<f64>` so the driver can
/// distinguish "baseline missing" (→ gate ERROR per §4.4 Level 1)
/// from "baseline present and worse than v0.3" (→ gate FAIL).
#[derive(Debug, Clone, Serialize)]
pub struct LongMemEvalDriverSummary {
    /// Overall mean score (scorer-owned).
    pub overall: f64,
    /// Per-category means (scorer-owned).
    pub by_category: BTreeMap<String, f64>,
    /// Total query count (scorer-owned).
    pub n_queries: usize,
    /// v0.2 LongMemEval baseline `overall`, read from
    /// `baselines/v02.toml` per design §5.1. `None` when the file is
    /// missing/unparsable — gate evaluation surfaces this as
    /// `GateStatus::Error`.
    pub v02_baseline: Option<f64>,
    /// Delta in **percentage points** (`(overall - v02_baseline) *
    /// 100.0`). `None` when `v02_baseline` is `None`. GOAL-5.3 gate
    /// reads this directly.
    pub delta_pp: Option<f64>,
}

// ---------------------------------------------------------------------------
// Driver type
// ---------------------------------------------------------------------------

/// LongMemEval driver — implements [`BenchDriver`].
///
/// Stateless beyond the dataset / baseline paths handed in via
/// `HarnessConfig`; constructed once per run by the CLI/harness and
/// dropped after the run completes.
#[derive(Debug, Clone, Default)]
pub struct LongMemEvalDriver;

impl LongMemEvalDriver {
    /// Construct a default driver. Kept as an explicit constructor so
    /// `engram_bench::drivers::longmemeval::LongMemEvalDriver::new()`
    /// reads naturally at the binary entry point.
    pub fn new() -> Self {
        Self
    }
}

impl BenchDriver for LongMemEvalDriver {
    fn name(&self) -> Driver {
        Driver::Longmemeval
    }

    fn stage(&self) -> Stage {
        // Harness `Stage` enum splits by LLM-scorer use; LongMemEval
        // sits in Stage2 alongside LOCOMO. (Design §4.3 puts
        // LongMemEval in stage-1 of the migration-dependence DAG —
        // these are different orderings; harness enum is authoritative
        // for the parallel runner.)
        Stage::Stage2
    }

    fn cost_tier(&self) -> CostTier {
        // Hours-scale on a full run (design §4.3 cost ordering bullet
        // 3: "Expensive last: §3.1 LOCOMO, §3.2 LongMemEval").
        CostTier::Expensive
    }

    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError> {
        run_impl(config)
    }
}

// ---------------------------------------------------------------------------
// Fixture loader (design §3.2 input + §6.1 SHA pin enforcement, mirrors
// LOCOMO's loader 1-for-1).
// ---------------------------------------------------------------------------

/// Read and parse `fixtures/longmemeval/source.toml` under
/// `fixture_root`.
///
/// Per design §9.2 (which inherits §9.1's vendoring policy): the
/// upstream LongMemEval commit SHA is committed in this file, plus a
/// SHA-256 for every fixture file we depend on. Any run with a missing
/// or unparsable `source.toml` is non-reproducible and rejected before
/// any query is loaded.
pub(crate) fn load_source_pin(
    fixture_root: &Path,
) -> Result<LongMemEvalSourcePin, BenchError> {
    let path = fixture_root.join("longmemeval").join("source.toml");
    if !path.exists() {
        return Err(BenchError::FixtureMissing(path));
    }
    let body = fs::read_to_string(&path).map_err(BenchError::IoError)?;
    let pin: LongMemEvalSourcePin = toml::from_str(&body).map_err(|e| {
        BenchError::Other(format!(
            "longmemeval source.toml parse {}: {e}",
            path.display()
        ))
    })?;
    Ok(pin)
}

/// Resolve the absolute path of a fixture entry under
/// `fixtures/longmemeval/<version_pin>/<entry.path>`.
pub(crate) fn fixture_path(
    fixture_root: &Path,
    pin: &LongMemEvalSourcePin,
    entry: &LongMemEvalFixturePin,
) -> PathBuf {
    fixture_root
        .join("longmemeval")
        .join(&pin.version_pin)
        .join(&entry.path)
}

/// Load the LongMemEval queries file (`queries.jsonl`) referenced by
/// `source.toml` and verify its SHA matches the pin.
///
/// Returns the parsed records in file order (deterministic per design
/// §3.2 inheriting §3.1: "Query order = file order").
pub(crate) fn load_queries(
    fixture_root: &Path,
    pin: &LongMemEvalSourcePin,
) -> Result<Vec<LongMemEvalRecord>, BenchError> {
    let entry = pin.fixtures.get("queries").ok_or_else(|| {
        BenchError::Other(
            "longmemeval source.toml missing required fixture entry [fixtures.queries]"
                .into(),
        )
    })?;
    let path = fixture_path(fixture_root, pin, entry);
    verify_fixture_sha(&path, &entry.sha256)?;

    let file = fs::File::open(&path).map_err(BenchError::IoError)?;
    let reader = BufReader::new(file);
    let mut records: Vec<LongMemEvalRecord> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(BenchError::IoError)?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: LongMemEvalRecord = serde_json::from_str(&line).map_err(|e| {
            BenchError::Other(format!(
                "longmemeval queries.jsonl line {}: {e}",
                lineno + 1
            ))
        })?;
        records.push(rec);
    }
    Ok(records)
}

// ---------------------------------------------------------------------------
// v0.2 baseline loader (design §5.1).
// ---------------------------------------------------------------------------

/// Try to load `baselines/v02.toml` from the configured baseline root.
///
/// Missing baseline is **NOT** a hard error here — gate evaluation
/// will see `v02_baseline = None` and surface GOAL-5.3 as
/// `GateStatus::Error` with a structured message (per design §4.4
/// Level 1: "missing → ERROR, never silent pass"; design §5.3
/// reiterates the same for unresolved external numbers).
///
/// We deliberately do NOT propagate parse errors as `BenchError` here
/// either — a malformed `v02.toml` is a configuration defect that
/// should surface as a gate ERROR with the parse error embedded in
/// the message (so the run still produces a complete report card),
/// not as a driver crash. The harness's parallel runner already maps
/// driver crashes to GateStatus::Error per §4.4 Level 1; the
/// distinction is "configuration defect → measurable, reportable
/// ERROR vs runtime panic → driver-crash ERROR". Keeping baseline
/// resolution non-fatal lets the driver still emit
/// `longmemeval_summary.json` with `v02_baseline = null` so the
/// reproducibility record (§6.1) records what was attempted.
fn try_load_v02_baseline(baseline_root: &Path) -> Option<V02Baseline> {
    let path = baseline_root.join("v02.toml");
    if !path.exists() {
        return None;
    }
    baselines::load_v02_baseline(&path).ok()
}

/// Compute `delta_pp = (overall - v02_baseline) * 100.0`, in
/// **percentage points** (the unit GOAL-5.3's threshold is expressed
/// in: `delta_pp ≥ 15.0` per design §4.1 row GOAL-5.3 / requirements
/// line 28). `None` when the baseline is unavailable.
///
/// Centralised here (not inlined in `run_impl`) so unit tests can pin
/// the exact formula and so the rounding semantics — none, we keep
/// full f64 precision — are explicit. Any future "should we round to
/// 2dp on disk" decision belongs in the JSON serializer, not in this
/// math.
pub(crate) fn compute_delta_pp(overall: f64, v02_baseline: Option<f64>) -> Option<f64> {
    v02_baseline.map(|b| (overall - b) * 100.0)
}

// ---------------------------------------------------------------------------
// Async-to-sync bridge (engramai retrieval API is `async fn`, the rest
// of engram-bench is sync; per the precedent in
// `crates/engramai/src/retrieval/api.rs::block_on` and in
// `drivers/locomo.rs::block_on`, we use a tiny noop waker —
// engram-bench's `Cargo.toml` deliberately avoids tokio).
//
// The retrieval stub today returns Ready synchronously after the first
// poll. When the real implementation lands and might suspend on I/O,
// this driver will need a real runtime; the change is local to this
// helper and its sibling in `locomo.rs`.
// ---------------------------------------------------------------------------

fn block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    // Safety: `fut` is owned on the stack and not moved while polled.
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
// Per-query replay: ingest a record's session turns, then pose the
// question against `Memory::graph_query_locked`. Returns one
// [`LongMemEvalQuery`] per record, with `predicted` = top-1 result's
// content (or "" on retrieval error / topic / empty result) and a
// parallel `latency_ms` value for the JSONL.
// ---------------------------------------------------------------------------

/// Per-query timing record — companion to the returned scorer-shaped
/// query. Kept separate from `LongMemEvalQuery` so the scorer's data
/// structure stays a pure scoring contract (no measurement noise).
///
/// Visibility note: `pub(crate)` because `write_per_query_jsonl` is
/// `pub(crate)` for unit tests in this module's `tests` block; the
/// type therefore needs to be at least as visible as the function
/// that consumes it. Identical pattern to `drivers::locomo::QueryTiming`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryTiming {
    pub(crate) latency_ms: f64,
}

/// Replay one record: fresh DB → ingest every session turn → pose the
/// question → return scorer-shaped query record + per-query latency.
///
/// `RetrievalError` on the question is handled per-query (predicted =
/// "", latency captured) so a single failure doesn't tank the whole
/// run; the run-level GateStatus::Error path triggers when the
/// **overall** score can't be computed (zero queries) — see
/// `build_snapshot`.
fn replay_record(
    rec: &LongMemEvalRecord,
) -> Result<(LongMemEvalQuery, QueryTiming), BenchError> {
    use engramai::retrieval::api::{GraphQuery, ScoredResult};

    let mut memory = fresh_in_memory_db()?;

    // Step 2 (design §3.2 inheriting §3.1): replay every session turn
    // in order BEFORE posing the question.
    for (turn_idx, turn) in rec.haystack_sessions.iter().enumerate() {
        memory.ingest_with_stats(turn).map_err(|e| {
            BenchError::Other(format!(
                "longmemeval replay: question `{}` session turn {} ingest failed: {e}",
                rec.question_id, turn_idx
            ))
        })?;
    }

    // Step 3: pose the gold question; capture top-1 content as the
    // predicted answer.
    let started = Instant::now();
    let resp = block_on(
        memory.graph_query_locked(GraphQuery::new(rec.question.clone()).with_limit(1)),
    );
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

    let predicted = match resp {
        Ok(response) => match response.results.first() {
            Some(ScoredResult::Memory { record, .. }) => record.content.clone(),
            // Topic results have no plain answer string for
            // LongMemEval's exact-match scorer; treat as empty to score
            // 0.0 (we deliberately don't synthesise a string from a
            // topic — that would be implicit answer-shaping the scorer
            // can't audit). Same policy as `drivers::locomo`.
            Some(ScoredResult::Topic { .. }) => String::new(),
            None => String::new(),
        },
        // Retrieval error → empty prediction → score 0.0. The error
        // surfaces in the per-query JSONL via empty `predicted` and is
        // *not* swallowed silently in the summary (GUARD-2: every gate
        // that can run, does run; the run-level Error path triggers
        // only when the overall score is uncomputable).
        Err(_) => String::new(),
    };

    let query = LongMemEvalQuery {
        id: rec.question_id.clone(),
        category: rec.question_type.clone(),
        predicted,
        gold: rec.answer.clone(),
    };
    Ok((query, QueryTiming { latency_ms }))
}

// ---------------------------------------------------------------------------
// Artifact emission (design §3.2 outputs).
// ---------------------------------------------------------------------------

/// Per-query JSONL row schema (design §3.2 inherits §3.1 shape).
#[derive(Debug, Clone, Serialize)]
struct PerQueryRow<'a> {
    id: &'a str,
    category: &'a str,
    predicted: &'a str,
    gold: &'a str,
    score: f64,
    latency_ms: f64,
}

/// Build the per-driver run directory under `output_root`. We don't
/// have a reproducibility record built yet at this point (gates feed
/// it), so we use a simple `<timestamp>_longmemeval` directory. The
/// harness runner later renames or symlinks to the canonical
/// `<timestamp>_<driver>_<short-sha>` layout once the record exists
/// (design §6.2). Identical pattern to `drivers::locomo::ensure_run_dir`.
fn ensure_run_dir(output_root: &Path) -> Result<PathBuf, BenchError> {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let dir = output_root.join(format!("{ts}_longmemeval"));
    fs::create_dir_all(&dir).map_err(BenchError::IoError)?;
    Ok(dir)
}

/// Write `longmemeval_summary.json` per design §3.2.
///
/// Output shape (when baseline available):
/// ```json
/// {
///   "overall": 0.62,
///   "by_category": { "multi-session": 0.55, "temporal-reasoning": 0.71 },
///   "n_queries": 200,
///   "v02_baseline": 0.45,
///   "delta_pp": 17.0
/// }
/// ```
/// `v02_baseline` and `delta_pp` serialize to JSON `null` when the
/// baseline file was missing — the gate evaluator then surfaces
/// GOAL-5.3 as ERROR rather than silently scoring against zero.
pub(crate) fn write_summary_json(
    dir: &Path,
    summary: &LongMemEvalDriverSummary,
) -> Result<PathBuf, BenchError> {
    let path = dir.join("longmemeval_summary.json");
    let body = serde_json::to_string_pretty(summary)
        .map_err(|e| BenchError::Other(format!("longmemeval summary serialize: {e}")))?;
    let mut f = fs::File::create(&path).map_err(BenchError::IoError)?;
    f.write_all(body.as_bytes()).map_err(BenchError::IoError)?;
    f.write_all(b"\n").map_err(BenchError::IoError)?;
    Ok(path)
}

/// Write `longmemeval_per_query.jsonl` per design §3.2 (one JSON
/// object per line, `\n`-terminated, no trailing comma noise).
pub(crate) fn write_per_query_jsonl(
    dir: &Path,
    queries: &[LongMemEvalQuery],
    scores: &[LongMemEvalScore],
    timings: &[QueryTiming],
) -> Result<PathBuf, BenchError> {
    assert_eq!(
        queries.len(),
        scores.len(),
        "scorer must return one score per query (LongMemEvalScorer::score contract)"
    );
    assert_eq!(
        queries.len(),
        timings.len(),
        "replay must produce one timing per query (replay_record contract)"
    );

    let path = dir.join("longmemeval_per_query.jsonl");
    let mut f = fs::File::create(&path).map_err(BenchError::IoError)?;

    for ((q, s), t) in queries.iter().zip(scores.iter()).zip(timings.iter()) {
        let row = PerQueryRow {
            id: &q.id,
            category: &q.category,
            predicted: &q.predicted,
            gold: &q.gold,
            score: s.score,
            latency_ms: t.latency_ms,
        };
        let line = serde_json::to_string(&row)
            .map_err(|e| BenchError::Other(format!("longmemeval per-query serialize: {e}")))?;
        f.write_all(line.as_bytes()).map_err(BenchError::IoError)?;
        f.write_all(b"\n").map_err(BenchError::IoError)?;
    }

    Ok(path)
}

// ---------------------------------------------------------------------------
// Gate evaluation (GOAL-5.3 binding via `longmemeval.delta_pp`).
// ---------------------------------------------------------------------------
//
// Gate-registry binding (post-reconcile, 2026-04-27): GOAL-5.3 is
// bound directly to `longmemeval.delta_pp ≥ 15.0`. This driver emits
// that key (plus supporting `longmemeval.overall`). GOAL-5.6 is now
// bound to `cognitive.regression_count` (a separate driver), so the
// previous `longmemeval.session_score` alias has been removed. The
// driver does **not** invent gate definitions locally; it only emits
// metric values. This mirrors the pattern used by `cost.rs`.

use crate::harness::gates::{BaselineResolver, GateDefinition, MetricSnapshot};

/// Build the metric snapshot the gates registry consults. Keys follow
/// the `<driver>.<metric>` convention.
///
/// Populated keys:
///
/// - `longmemeval.overall` — scorer overall mean (always present when
///   `n_queries > 0`).
/// - `longmemeval.delta_pp` — `(overall - v02_baseline) * 100.0`,
///   only when both `n_queries > 0` AND the v0.2 baseline was loaded.
///   Missing in either case → `MetricValue::Missing(reason)` with a
///   structured reason string the gate evaluator surfaces verbatim.
///   Bound to GOAL-5.3 in the registry (≥ 15.0 percentage points).
///
/// Per design §4.4 Level 1 ("missing metric → ERROR, never silent
/// pass") we deliberately do NOT register stub `0.0` values: that
/// would score `delta_pp ≥ 15.0` as a fail (numerically) and hide the
/// "no measurement happened" case behind a normal failure. The error
/// path is the safety mechanism.
fn build_snapshot(summary: &LongMemEvalDriverSummary) -> MetricSnapshot {
    let mut snap = MetricSnapshot::new();

    // longmemeval.overall — only valid if the run actually produced
    // queries. Per design §6.1 invariant: empty fixture /
    // short-circuited driver is ERROR, never silent PASS.
    if summary.n_queries == 0 {
        snap.set_missing(
            "longmemeval.overall",
            "n_queries == 0 (fixture empty or driver short-circuited) — \
             LongMemEval overall could not be computed",
        );
        snap.set_missing(
            "longmemeval.delta_pp",
            "n_queries == 0 — LongMemEval overall is uncomputable, \
             so delta_pp is uncomputable",
        );
    } else {
        snap.set_number("longmemeval.overall", summary.overall);

        match summary.delta_pp {
            Some(d) => snap.set_number("longmemeval.delta_pp", d),
            None => snap.set_missing(
                "longmemeval.delta_pp",
                "v0.2 baseline missing (baselines/v02.toml not found or \
                 unparsable) — GOAL-5.3 delta_pp cannot be computed. \
                 Per design §5.1 the baseline must be captured on the \
                 v0.2.2 tag and committed before v0.3 development; per \
                 §4.4 Level 1 a missing baseline is ERROR, not silent PASS.",
            ),
        }
    }

    snap
}

/// `BaselineResolver` impl for the LongMemEval driver. The v0.2
/// baseline is consumed at the **metric** level (we materialise
/// `delta_pp` ourselves and emit it as `longmemeval.delta_pp`), not
/// at the gate level — so the registry has nothing to resolve via
/// `BaselineResolver::resolve` for `longmemeval.*` gates today. If
/// a future gate is bound directly with `Threshold::FromBaseline`
/// (e.g. "delta over Graphiti's published number"), this impl will
/// need to grow real resolution.
struct LongMemEvalBaselines<'a> {
    /// Held but unused today. Plumbed so a future
    /// `Threshold::FromBaseline` gate can reach it without a
    /// downstream signature change.
    _v02: Option<&'a V02Baseline>,
}

impl<'a> BaselineResolver for LongMemEvalBaselines<'a> {
    fn resolve(&self, gate: &GateDefinition) -> Result<f64, String> {
        Err(format!(
            "longmemeval driver has no baseline-relative gate today; gate {} \
             requires a baseline but the LongMemEval driver materialises \
             `delta_pp` directly as a metric value (computed against \
             baselines/v02.toml at run time per design §5.1). If the \
             gates-registry reconcile adds a baseline-relative gate later, \
             this resolver must be replaced.",
            gate.goal
        ))
    }
}

/// Evaluate all `longmemeval.*` gates from the registry against
/// `summary`. Returns whatever the registry has under the
/// `longmemeval.` prefix — see the gate-registry note in the module
/// docs for why GOAL-5.3 is not yet wired here.
pub(crate) fn evaluate_gates(summary: &LongMemEvalDriverSummary) -> Vec<GateResult> {
    let snap = build_snapshot(summary);
    let resolver = LongMemEvalBaselines { _v02: None };
    crate::harness::gates::evaluate_for_driver("longmemeval.", &snap, &resolver)
}

// ---------------------------------------------------------------------------
// Top-level run orchestration (design §3.2 procedure 1–8).
// ---------------------------------------------------------------------------

fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // 1. Load + verify fixtures.
    let pin = load_source_pin(&config.fixture_root)?;
    let records = load_queries(&config.fixture_root, &pin)?;

    // 2. Replay every record (fresh DB per record, ingest session
    //    turns, pose question), accumulating queries + timings.
    let mut all_queries: Vec<LongMemEvalQuery> = Vec::new();
    let mut all_timings: Vec<QueryTiming> = Vec::new();
    for rec in &records {
        let (q, t) = replay_record(rec)?;
        all_queries.push(q);
        all_timings.push(t);
    }

    // 3. Score everything in one shot — scorer aggregates by category
    //    and overall.
    let scorer = LongMemEvalScorer;
    let (scores, scorer_summary) = scorer.score(&all_queries);

    // 4. Read v0.2 baseline (None = ERROR path — see build_snapshot).
    let v02 = try_load_v02_baseline(&config.baseline_root);
    let v02_overall = v02.as_ref().map(|b| b.longmemeval.overall);
    let delta_pp = compute_delta_pp(scorer_summary.overall, v02_overall);

    // 5. Compose driver-level summary (scorer fields + baseline +
    //    delta).
    let summary = LongMemEvalDriverSummary {
        overall: scorer_summary.overall,
        by_category: scorer_summary.by_category.clone(),
        n_queries: scorer_summary.n_queries,
        v02_baseline: v02_overall,
        delta_pp,
    };

    // 6. Emit artifacts.
    let dir = ensure_run_dir(&config.output_root)?;
    write_summary_json(&dir, &summary)?;
    write_per_query_jsonl(&dir, &all_queries, &scores, &all_timings)?;

    // 7. Evaluate gates.
    let gates = evaluate_gates(&summary);

    // 8. Compose summary_json for the harness runner — same shape we
    //    wrote to disk PLUS the source pin so the reproducibility
    //    record can reconstruct dataset provenance, AND the v0.2
    //    baseline content_sha256 (when available) so the §4.2a
    //    meta-gate can verify the immutable-baseline contract held
    //    across this run.
    let summary_json = serde_json::json!({
        "longmemeval_overall": summary.overall,
        "longmemeval_by_category": summary.by_category,
        "longmemeval_n_queries": summary.n_queries,
        "longmemeval_v02_baseline": summary.v02_baseline,
        "longmemeval_delta_pp": summary.delta_pp,
        "longmemeval_version_pin": pin.version_pin,
        "longmemeval_v02_baseline_sha256":
            v02.as_ref().map(|b| b.content_sha256().to_string()),
    });

    // The reproducibility record proper is written by the harness
    // runner. We hand back the run directory so the runner knows
    // where to land it.
    Ok(RunReport {
        driver: Driver::Longmemeval,
        record_path: dir.join("reproducibility.toml"),
        gates,
        summary_json,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
//
// Coverage philosophy (mirrors `drivers::locomo`):
//
// - **Pure logic** (delta_pp math, snapshot building, summary shape,
//   JSON/JSONL writers) is unit-tested with synthetic
//   `LongMemEvalDriverSummary` / `LongMemEvalQuery` values. No fixture
//   I/O.
// - **Fixture loader** is tested with a tempdir that mirrors the
//   on-disk layout (`fixtures/longmemeval/source.toml` + the JSONL
//   file). SHA mismatch + missing fixture each get their own test.
// - **Baseline loader** is tested via the `try_load_v02_baseline`
//   helper directly: present-and-valid path, missing file path,
//   parse-error path. The "missing → ERROR" gate behaviour is covered
//   in the gate-evaluation tests.
// - **End-to-end run** (`run_impl`) is tested with a tempdir that
//   contains a 2-record mini-fixture. It asserts artifacts land on
//   disk in the documented shape; gate evaluation surfaces `Error`
//   (because `graph_query_locked` is currently a stub returning
//   `RetrievalError::Internal` → empty predictions → 0.0 overall).
//   Marked `#[ignore]` until the retrieval pipeline lands.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::GateStatus;

    // --- Helpers ------------------------------------------------------------

    fn driver_summary(
        overall: f64,
        by_cat: &[(&str, f64)],
        n: usize,
        v02: Option<f64>,
    ) -> LongMemEvalDriverSummary {
        let mut bc: BTreeMap<String, f64> = BTreeMap::new();
        for (k, v) in by_cat {
            bc.insert((*k).to_string(), *v);
        }
        LongMemEvalDriverSummary {
            overall,
            by_category: bc,
            n_queries: n,
            v02_baseline: v02,
            delta_pp: compute_delta_pp(overall, v02),
        }
    }

    /// Write a fake `baselines/v02.toml` whose `[longmemeval]` section
    /// reports `overall = <overall>`. Returns the path.
    fn write_fake_v02_baseline(root: &Path, overall: f64) -> PathBuf {
        let body = format!(
            r#"[longmemeval]
dataset_sha = "longmemeval-test-sha"
v02_tag = "v0.2.2"
overall = {overall}
captured_at = "2026-04-01"
captured_by = "unit-test"

[longmemeval.by_category]
"multi-session" = {overall}
"#
        );
        let path = root.join("v02.toml");
        std::fs::write(&path, body).expect("write fake v02 baseline");
        path
    }

    // --- delta_pp formula --------------------------------------------------

    /// `delta_pp` is `(overall - v02_baseline) * 100.0` in **percentage
    /// points**, computed at full f64 precision. Pinning the formula
    /// here so a future "let's round to 2dp" regression is caught.
    #[test]
    fn delta_pp_is_percentage_point_difference_at_full_precision() {
        // 0.62 vs 0.45 → 17.0 pp (the GOAL-5.3 threshold sits at 15.0).
        // Float arithmetic isn't exact here either; pin via tolerance.
        let d = compute_delta_pp(0.62, Some(0.45)).unwrap();
        assert!((d - 17.0).abs() < 1e-9, "expected ≈17.0 pp, got {d}");
        // Boundary: ≈ +15.0 pp (within IEEE-754 tolerance).
        let d15 = compute_delta_pp(0.60, Some(0.45)).unwrap();
        assert!((d15 - 15.0).abs() < 1e-9, "expected ≈15.0 pp, got {d15}");
        // v0.3 below baseline → negative delta (gate fails).
        assert!(compute_delta_pp(0.40, Some(0.45)).unwrap() < 0.0);
        // No baseline → no delta (gate ERROR path).
        assert_eq!(compute_delta_pp(0.62, None), None);
        // Pinning the unit (pp, not fraction): 1pp == 0.01 of the
        // underlying score range. A 5pp delta is +5.0, NOT +0.05.
        let d5 = compute_delta_pp(0.50, Some(0.45)).unwrap();
        assert!((d5 - 5.0).abs() < 1e-9, "delta should be 5.0 pp not 0.05, got {d5}");
    }

    // --- Gate evaluation ---------------------------------------------------

    /// When the fixture is empty the snapshot marks BOTH `overall` and
    /// `delta_pp` as Missing — the gate evaluator surfaces that as
    /// `Error`, never as `Pass` or `Fail` against a silent zero. Per
    /// design §4.4 Level 1.
    #[test]
    fn gates_error_on_zero_queries() {
        let s = driver_summary(0.0, &[], 0, Some(0.45));
        let gates = evaluate_gates(&s);
        // Whatever `longmemeval.*` gates the registry has, all of them
        // must be ERROR (no pass-through silent zero).
        assert!(!gates.is_empty(), "registry must have at least one longmemeval.* gate");
        for g in &gates {
            assert_eq!(
                g.status,
                GateStatus::Error,
                "gate {} should be Error on n_queries==0, got {:?}",
                g.goal,
                g.status
            );
        }
    }

    /// When v0.2 baseline is missing, `delta_pp` is `None` →
    /// `MetricValue::Missing` → gate Error. Per design §4.4 Level 1
    /// + §5.1 immutable-baseline contract.
    #[test]
    fn gates_error_when_v02_baseline_missing() {
        // Real overall, but no baseline → delta_pp is None.
        let s = driver_summary(0.62, &[("multi-session", 0.55)], 100, None);
        let snap = build_snapshot(&s);

        // Direct snapshot inspection — independent of registry shape.
        // `longmemeval.delta_pp` MUST be Missing with a message that
        // mentions baselines/v02.toml so the operator knows what to fix.
        let value = snap.get("longmemeval.delta_pp").expect("key present");
        match value {
            crate::harness::gates::MetricValue::Missing(reason) => {
                assert!(
                    reason.contains("baselines/v02.toml"),
                    "reason should name baselines/v02.toml, got: {reason}"
                );
                assert!(
                    reason.contains("§5.1") || reason.contains("§4.4"),
                    "reason should reference design contract, got: {reason}"
                );
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    /// Post-reconcile: `longmemeval.overall` is populated as `Number`
    /// when a run produced queries. The previously-populated
    /// `longmemeval.session_score` alias has been removed — GOAL-5.6
    /// is no longer bound to it.
    #[test]
    fn snapshot_populates_overall_and_drops_session_score_alias() {
        let s = driver_summary(0.62, &[("multi-session", 0.55)], 100, Some(0.45));
        let snap = build_snapshot(&s);

        let overall = snap.get("longmemeval.overall").expect("overall present");
        match overall {
            crate::harness::gates::MetricValue::Number(a) => {
                assert_eq!(*a, 0.62);
            }
            other => panic!("expected Number for overall, got {other:?}"),
        }

        // The session_score alias was a workaround for a stale
        // registry binding (now reconciled). It must not be populated.
        assert!(
            snap.get("longmemeval.session_score").is_none(),
            "longmemeval.session_score alias must be absent post-reconcile"
        );
    }

    /// `longmemeval.delta_pp` is populated as a `Number` when both the
    /// overall AND the v0.2 baseline are present.
    #[test]
    fn snapshot_populates_delta_pp_when_baseline_available() {
        let s = driver_summary(0.62, &[("multi-session", 0.55)], 100, Some(0.45));
        let snap = build_snapshot(&s);

        let delta = snap.get("longmemeval.delta_pp").expect("delta_pp present");
        match delta {
            crate::harness::gates::MetricValue::Number(v) => {
                assert!(
                    (v - 17.0).abs() < 1e-9,
                    "delta_pp should be 17.0 pp, got {v}"
                );
            }
            other => panic!("expected Number, got {other:?}"),
        }
    }

    /// `BaselineResolver` for this driver is intentionally a refusal —
    /// the v0.2 baseline is consumed at the metric level, not gate
    /// level, so the registry has no `longmemeval.*` gates that need
    /// resolution today. This test pins that contract: any registry
    /// that *did* request resolution would get a structured Err with
    /// the gate name embedded (so a future reconcile sees exactly
    /// which gate prompted the demand).
    #[test]
    fn baseline_resolver_refuses_with_gate_name_in_message() {
        let resolver = LongMemEvalBaselines { _v02: None };
        let fake_gate = GateDefinition {
            goal: "GOAL-FAKE",
            metric_key: "longmemeval.something_baseline_relative",
            comparator: crate::harness::gates::Comparator::GreaterOrEqual,
            threshold: crate::harness::gates::Threshold::FromBaseline {
                description: "test",
            },
            priority: crate::harness::gates::Priority::P0Hard,
            description: "test gate",
        };
        let err = resolver
            .resolve(&fake_gate)
            .expect_err("longmemeval has no baseline-relative gates today");
        assert!(err.contains("GOAL-FAKE"), "error must name the gate, got: {err}");
        assert!(
            err.contains("delta_pp"),
            "error should reference the metric materialisation, got: {err}"
        );
    }

    // --- Artifact emission -------------------------------------------------

    /// `longmemeval_summary.json` is valid JSON with exactly the
    /// documented shape (overall, by_category, n_queries, v02_baseline,
    /// delta_pp). Round-trips through `serde_json::Value`.
    #[test]
    fn summary_json_has_documented_shape_with_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = driver_summary(
            0.62,
            &[("multi-session", 0.55), ("temporal-reasoning", 0.71)],
            200,
            Some(0.45),
        );
        let path = write_summary_json(dir.path(), &s).expect("write summary");
        assert!(path.exists());

        let body = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["overall"], 0.62);
        assert_eq!(v["n_queries"], 200);
        assert_eq!(v["by_category"]["multi-session"], 0.55);
        assert_eq!(v["by_category"]["temporal-reasoning"], 0.71);
        assert_eq!(v["v02_baseline"], 0.45);
        assert!((v["delta_pp"].as_f64().unwrap() - 17.0).abs() < 1e-9);
    }

    /// `v02_baseline` and `delta_pp` serialize to JSON `null` (not
    /// missing keys) when the baseline is absent — so downstream tools
    /// can pin `null != absent` semantics.
    #[test]
    fn summary_json_emits_null_for_missing_baseline_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = driver_summary(0.62, &[("multi-session", 0.55)], 100, None);
        let path = write_summary_json(dir.path(), &s).expect("write summary");

        let body = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["overall"], 0.62);
        // `null`, not absent — consumers can distinguish "we tried and
        // failed" from "we never tried".
        assert!(v["v02_baseline"].is_null());
        assert!(v["delta_pp"].is_null());
    }

    /// `longmemeval_per_query.jsonl` is one JSON object per line, each
    /// line `\n`-terminated. Asserts the row schema documented in §3.2.
    #[test]
    fn per_query_jsonl_one_object_per_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let queries = vec![
            LongMemEvalQuery {
                id: "q1".into(),
                category: "multi-session".into(),
                predicted: "yes".into(),
                gold: "yes".into(),
            },
            LongMemEvalQuery {
                id: "q2".into(),
                category: "temporal-reasoning".into(),
                predicted: "Paris".into(),
                gold: "London".into(),
            },
        ];
        let scorer = LongMemEvalScorer;
        let (scores, _) = scorer.score(&queries);
        let timings = vec![
            QueryTiming { latency_ms: 12.5 },
            QueryTiming { latency_ms: 7.0 },
        ];

        let path = write_per_query_jsonl(dir.path(), &queries, &scores, &timings)
            .expect("write");
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);

        let row0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(row0["id"], "q1");
        assert_eq!(row0["category"], "multi-session");
        assert_eq!(row0["score"], 1.0);
        assert_eq!(row0["latency_ms"], 12.5);

        let row1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(row1["id"], "q2");
        assert_eq!(row1["score"], 0.0);
        assert_eq!(row1["latency_ms"], 7.0);
    }

    // --- Fixture loader ----------------------------------------------------

    /// Lay out a fake `fixtures/longmemeval/<sha>/` tree with a
    /// `source.toml` whose hash matches `body`.
    fn write_fake_fixture(
        root: &Path,
        version_pin: &str,
        queries_jsonl: &str,
    ) -> (PathBuf, String) {
        use sha2::{Digest, Sha256};

        let queries_path_rel = "queries.jsonl";
        let queries_path = root
            .join("longmemeval")
            .join(version_pin)
            .join(queries_path_rel);
        std::fs::create_dir_all(queries_path.parent().unwrap()).unwrap();
        std::fs::write(&queries_path, queries_jsonl.as_bytes()).unwrap();

        let sha = hex::encode(Sha256::digest(queries_jsonl.as_bytes()));
        let source_toml = format!(
            r#"version_pin = "{version_pin}"

[fixtures.queries]
path = "{queries_path_rel}"
sha256 = "{sha}"
"#
        );
        let source_path = root.join("longmemeval").join("source.toml");
        std::fs::write(&source_path, source_toml).unwrap();
        (queries_path, sha)
    }

    /// `load_source_pin` + `load_queries` returns records in file
    /// order with the right field shape.
    #[test]
    fn loader_parses_jsonl_in_file_order() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = "\
{\"question_id\":\"q0\",\"question_type\":\"multi-session\",\"question\":\"who?\",\"answer\":\"alice\",\"haystack_sessions\":[\"hi\",\"alice spoke\"]}
{\"question_id\":\"q1\",\"question_type\":\"temporal-reasoning\",\"question\":\"when?\",\"answer\":\"2020\",\"haystack_sessions\":[\"in 2020 they met\"]}
";
        write_fake_fixture(dir.path(), "abc123", jsonl);

        let pin = load_source_pin(dir.path()).expect("source.toml parses");
        assert_eq!(pin.version_pin, "abc123");

        let recs = load_queries(dir.path(), &pin).expect("queries parse");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].question_id, "q0");
        assert_eq!(recs[0].question_type, "multi-session");
        assert_eq!(recs[0].haystack_sessions.len(), 2);
        assert_eq!(recs[1].question_id, "q1");
        assert_eq!(recs[1].answer, "2020");
    }

    /// SHA pin mismatch → `ChecksumMismatch`, not silent load.
    #[test]
    fn loader_rejects_sha_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = "{\"question_id\":\"q0\",\"question_type\":\"x\",\"question\":\"?\",\"answer\":\"a\",\"haystack_sessions\":[]}\n";
        write_fake_fixture(dir.path(), "abc123", jsonl);

        // Tamper: rewrite under same path with different bytes.
        let tampered = "{\"question_id\":\"TAMPERED\",\"question_type\":\"x\",\"question\":\"?\",\"answer\":\"a\",\"haystack_sessions\":[]}\n";
        let qpath = dir.path().join("longmemeval").join("abc123").join("queries.jsonl");
        std::fs::write(&qpath, tampered).unwrap();

        let pin = load_source_pin(dir.path()).unwrap();
        let err = load_queries(dir.path(), &pin).expect_err("sha must mismatch");
        assert!(
            matches!(err, BenchError::ChecksumMismatch { .. }),
            "expected ChecksumMismatch, got {err:?}"
        );
    }

    /// Missing `source.toml` → `FixtureMissing`, not `Other`.
    #[test]
    fn loader_reports_missing_source_toml() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_source_pin(dir.path()).expect_err("must fail");
        assert!(
            matches!(err, BenchError::FixtureMissing(_)),
            "expected FixtureMissing, got {err:?}"
        );
    }

    /// `source.toml` without `[fixtures.queries]` is a structural
    /// error: surfaced as `BenchError::Other` with an explicit message
    /// rather than panicking.
    #[test]
    fn loader_rejects_source_toml_missing_queries_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("longmemeval")).unwrap();
        std::fs::write(
            dir.path().join("longmemeval").join("source.toml"),
            "version_pin = \"x\"\n[fixtures]\n",
        )
        .unwrap();

        let pin = load_source_pin(dir.path()).expect("parses");
        let err = load_queries(dir.path(), &pin).expect_err("must fail");
        match err {
            BenchError::Other(msg) => {
                assert!(msg.contains("[fixtures.queries]"), "msg: {msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    // --- Baseline loader ---------------------------------------------------

    /// `try_load_v02_baseline` returns `Some` when the file is present
    /// and well-formed. We DON'T compare the parsed struct field-by-
    /// field here (that's `baselines.rs`'s territory); we just confirm
    /// the loader reaches into the configured baseline_root and
    /// produces a struct whose `overall` matches what we wrote.
    #[test]
    fn baseline_loader_returns_some_on_well_formed_file() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_v02_baseline(dir.path(), 0.45);

        let loaded = try_load_v02_baseline(dir.path()).expect("present file → Some");
        assert!((loaded.longmemeval.overall - 0.45).abs() < 1e-9);
    }

    /// Missing baseline file → `None` (not panic). The downstream gate
    /// then reports ERROR per design §4.4 Level 1 — that path is
    /// exercised in `gates_error_when_v02_baseline_missing`.
    #[test]
    fn baseline_loader_returns_none_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = try_load_v02_baseline(dir.path());
        assert!(loaded.is_none(), "missing file should produce None");
    }

    /// Malformed baseline file → `None` (parse error swallowed; the
    /// gate evaluator surfaces a structured ERROR, not a driver
    /// crash). See `try_load_v02_baseline` doc comment for rationale.
    #[test]
    fn baseline_loader_returns_none_on_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("v02.toml"), "not = valid = toml").unwrap();
        let loaded = try_load_v02_baseline(dir.path());
        assert!(loaded.is_none(), "parse error should produce None");
    }

    // --- Driver trait wiring -----------------------------------------------

    /// `BenchDriver` impl exposes the canonical (Driver, Stage,
    /// CostTier) triple — these are scheduling-relevant per design
    /// §4.3 and must never silently change.
    #[test]
    fn driver_metadata_matches_design_3_2_and_4_3() {
        let d = LongMemEvalDriver::new();
        assert_eq!(d.name(), Driver::Longmemeval);
        // Stage2 per harness convention (LLM-scorer using); see Stage
        // enum docs for why this differs from design §4.3's
        // migration-dependence-based stage-1 placement.
        assert_eq!(d.stage(), Stage::Stage2);
        // Expensive last per design §4.3 within-stage cost ordering
        // bullet 3.
        assert_eq!(d.cost_tier(), CostTier::Expensive);
    }

    // --- End-to-end (gated until retrieval lands) --------------------------

    /// Smoke test: full `run_impl` against a tiny on-disk fixture +
    /// fake v0.2 baseline. Currently `#[ignore]`d because
    /// `Memory::graph_query_locked` is a stub; once
    /// `task:retr-impl-orchestrator` lands and the locked fusion path
    /// returns real results, removing the ignore turns this into a
    /// real driver smoke test.
    ///
    /// What it asserts (when un-ignored):
    /// - Artifacts land on disk under `output_root` with the
    ///   documented filenames.
    /// - `RunReport.gates` contains at least one entry.
    /// - `summary_json` carries the `longmemeval_*` fields including
    ///   the version_pin and the v0.2 baseline content_sha256.
    #[test]
    #[ignore = "depends on Memory::graph_query_locked — currently RetrievalError::Internal stub (task:retr-impl-orchestrator)"]
    fn run_impl_emits_artifacts_and_gates() {
        let fix_dir = tempfile::tempdir().unwrap();
        let out_dir = tempfile::tempdir().unwrap();
        let base_dir = tempfile::tempdir().unwrap();

        let jsonl = "\
{\"question_id\":\"q0\",\"question_type\":\"multi-session\",\"question\":\"who met bob\",\"answer\":\"alice\",\"haystack_sessions\":[\"alice met bob in 2020\"]}
";
        write_fake_fixture(fix_dir.path(), "test-sha", jsonl);
        write_fake_v02_baseline(base_dir.path(), 0.45);

        let cfg = HarnessConfig {
            fixture_root: fix_dir.path().to_path_buf(),
            baseline_root: base_dir.path().to_path_buf(),
            output_root: out_dir.path().to_path_buf(),
            parallel_limit: 1,
            seed: 0,
            override_gate: None,
            rationale_file: None,
        };

        let report = LongMemEvalDriver::new()
            .run(&cfg)
            .expect("run completes (even if every gate Errors, run() itself returns Ok)");
        assert_eq!(report.driver, Driver::Longmemeval);
        assert!(!report.gates.is_empty());

        let v = &report.summary_json;
        assert!(v.get("longmemeval_overall").is_some());
        assert_eq!(v["longmemeval_version_pin"], "test-sha");
        assert!(v["longmemeval_v02_baseline_sha256"].is_string());
    }
}
