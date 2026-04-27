//! LOCOMO driver (design §3.1).
//!
//! Implements [`crate::harness::BenchDriver`] for the LOCOMO multi-turn
//! memory benchmark. Per design §3.1:
//!
//! 1. Load LOCOMO conversations from a pinned-SHA fixture
//!    (`fixtures/locomo/<sha>/conversations.jsonl`).
//! 2. For each conversation: spin up a fresh in-memory engramai
//!    `Memory`, replay every episode via `ingest_with_stats`, then run
//!    every gold question through `Memory::graph_query_locked`.
//! 3. The top-1 result's `MemoryRecord::content` is the predicted
//!    answer. Score against the gold via [`crate::scorers::locomo`]
//!    (normalised exact match — bit-parity-tested in sub-task 2 of the
//!    scorer task).
//! 4. Aggregate across all conversations: overall mean + per-category
//!    means. `temporal` is reported separately for GOAL-5.2.
//! 5. Emit two artifacts under the run directory:
//!    `locomo_summary.json` and `locomo_per_query.jsonl` (design §3.1).
//! 6. Evaluate gates GOAL-5.1 (overall ≥ 68.5%) and GOAL-5.2 (temporal
//!    ≥ Graphiti baseline) and return them in the `RunReport`.
//!
//! ## Stage / cost
//!
//! `Stage2 / Expensive`: depends on stage-1 success and is the most
//! expensive driver in the suite (LLM scorer hits + per-conversation
//! DB reset). Per design §4.3 it runs after stage-1 and within stage-2
//! sorts last (`Expensive`).
//!
//! ## Determinism (design §3.1)
//!
//! - Fusion weights pinned via `Memory::graph_query_locked` (which
//!   internally uses `FusionConfig::locked()`).
//! - Query order = file order (we iterate the JSONL line-by-line).
//! - No randomness inside the driver itself; per-conversation DBs use
//!   SQLite `:memory:`.
//!
//! ## Wiring status
//!
//! `Memory::graph_query_locked` is currently an `Internal`-error stub
//! in engramai (`crates/engramai/src/retrieval/api.rs`). The driver
//! threads a `RetrievalError` through to a structured per-query
//! failure (`predicted = ""`, score `0.0`) rather than aborting the
//! whole run; integration tests are `#[ignore]`d until the real
//! retrieval pipeline lands. Unit tests in this file cover everything
//! that does not depend on the retrieval stub: fixture parsing,
//! summary aggregation, gate evaluation, artifact emission.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::baselines::{self, ExternalBaselines};
use crate::harness::gates::GateResult;
use crate::harness::{
    fresh_in_memory_db, verify_fixture_sha, BenchDriver, BenchError, CostTier, Driver,
    HarnessConfig, RunReport, Stage,
};
use crate::scorers::locomo::{LocomoQuery, LocomoScore, LocomoScorer, LocomoSummary};

// ---------------------------------------------------------------------------
// Fixture schema (design §3.1 / §9.1 — JSONL-on-disk).
// ---------------------------------------------------------------------------

/// One LOCOMO conversation as it appears in
/// `fixtures/locomo/<sha>/conversations.jsonl` (one JSON object per
/// line). Field names match LOCOMO's upstream dataset; we do not
/// rename them so a future `version_pin = "<sha>"` upgrade only has to
/// touch this struct if upstream renames anything.
#[derive(Debug, Clone, Deserialize)]
pub struct LocomoConversation {
    /// Stable conversation id from the LOCOMO dataset.
    pub conversation_id: String,
    /// Ordered episodes — replayed in this order via `ingest_with_stats`.
    /// Each entry is one natural-language utterance / event.
    pub episodes: Vec<String>,
    /// Gold question/answer set evaluated AFTER all episodes are
    /// ingested (design §3.1 step 3).
    pub questions: Vec<LocomoGoldQuery>,
}

/// One gold-labelled query attached to a [`LocomoConversation`].
#[derive(Debug, Clone, Deserialize)]
pub struct LocomoGoldQuery {
    /// Globally-unique query id (also stable across LOCOMO releases).
    pub id: String,
    /// LOCOMO category tag (e.g. `temporal`, `single-hop`, `multi-hop`).
    pub category: String,
    /// Natural-language question text — fed verbatim to
    /// `GraphQuery::new(text)`.
    pub question: String,
    /// Gold answer string for scoring.
    pub gold: String,
}

// ---------------------------------------------------------------------------
// Source pin (`fixtures/locomo/source.toml`) — design §9.1 version pin.
// ---------------------------------------------------------------------------

/// `fixtures/locomo/source.toml` schema. Pins the dataset to a single
/// upstream commit AND records the SHA-256 of every fixture file so
/// the run is non-reproducible if either the pin or the file content
/// drifts (design §6.1 / §9.1).
#[derive(Debug, Clone, Deserialize)]
pub struct LocomoSourcePin {
    /// Upstream LOCOMO repository commit SHA — recorded verbatim into
    /// the reproducibility record's `[dataset]` block.
    pub version_pin: String,
    /// Fixture entries: relative path → expected SHA-256 (hex).
    pub fixtures: BTreeMap<String, LocomoFixturePin>,
}

/// One entry under `[fixtures.<name>]` in `source.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct LocomoFixturePin {
    /// Path under `fixtures/locomo/<version_pin>/` (e.g.
    /// `conversations.jsonl`).
    pub path: String,
    /// Expected SHA-256 of the file at `path`, hex-encoded (lowercase
    /// after normalisation in [`verify_fixture_sha`]).
    pub sha256: String,
}

// ---------------------------------------------------------------------------
// Driver type
// ---------------------------------------------------------------------------

/// LOCOMO driver — implements [`BenchDriver`].
///
/// Stateless beyond the dataset / baseline paths; constructed once per
/// run by the CLI/harness and dropped after the run completes.
#[derive(Debug, Clone, Default)]
pub struct LocomoDriver;

impl LocomoDriver {
    /// Construct a default driver. Kept as an explicit constructor (not
    /// just `Default::default()`) so `engram_bench::drivers::locomo::LocomoDriver::new()`
    /// reads naturally at the binary entry point.
    pub fn new() -> Self {
        Self
    }
}

// ---------------------------------------------------------------------------
// BenchDriver impl — wired in stages below.
// ---------------------------------------------------------------------------

impl BenchDriver for LocomoDriver {
    fn name(&self) -> Driver {
        Driver::Locomo
    }

    fn stage(&self) -> Stage {
        // Design §3.1 + §4.3: LOCOMO is the canonical Stage 2 driver
        // (depends on stage-1 success, runs the most expensive scorer).
        Stage::Stage2
    }

    fn cost_tier(&self) -> CostTier {
        // Hours-scale on a full run — the most expensive in the suite.
        CostTier::Expensive
    }

    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError> {
        run_impl(config)
    }
}

// ---------------------------------------------------------------------------
// Fixture loader (design §3.1 input + §6.1 SHA pin enforcement).
// ---------------------------------------------------------------------------

/// Read and parse `fixtures/locomo/source.toml` under `fixture_root`.
///
/// Per design §9.1: the upstream LOCOMO commit SHA is committed in
/// this file, plus a SHA-256 for every fixture file we depend on. Any
/// run with a missing or unparsable `source.toml` is non-reproducible
/// and rejected before any conversation is loaded.
pub(crate) fn load_source_pin(fixture_root: &Path) -> Result<LocomoSourcePin, BenchError> {
    let path = fixture_root.join("locomo").join("source.toml");
    if !path.exists() {
        return Err(BenchError::FixtureMissing(path));
    }
    let body = fs::read_to_string(&path).map_err(BenchError::IoError)?;
    let pin: LocomoSourcePin = toml::from_str(&body).map_err(|e| {
        BenchError::Other(format!(
            "locomo source.toml parse {}: {e}",
            path.display()
        ))
    })?;
    Ok(pin)
}

/// Resolve the absolute path of a fixture entry under
/// `fixtures/locomo/<version_pin>/<entry.path>`.
pub(crate) fn fixture_path(
    fixture_root: &Path,
    pin: &LocomoSourcePin,
    entry: &LocomoFixturePin,
) -> PathBuf {
    fixture_root
        .join("locomo")
        .join(&pin.version_pin)
        .join(&entry.path)
}

/// Load the LOCOMO conversations file (`conversations.jsonl`) referenced
/// by `source.toml` and verify its SHA matches the pin.
///
/// Returns the parsed conversations in file order (deterministic per
/// design §3.1 "Query order: file order").
pub(crate) fn load_conversations(
    fixture_root: &Path,
    pin: &LocomoSourcePin,
) -> Result<Vec<LocomoConversation>, BenchError> {
    let entry = pin.fixtures.get("conversations").ok_or_else(|| {
        BenchError::Other(
            "locomo source.toml missing required fixture entry [fixtures.conversations]"
                .into(),
        )
    })?;
    let path = fixture_path(fixture_root, pin, entry);
    verify_fixture_sha(&path, &entry.sha256)?;

    let file = fs::File::open(&path).map_err(BenchError::IoError)?;
    let reader = BufReader::new(file);
    let mut conversations: Vec<LocomoConversation> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(BenchError::IoError)?;
        if line.trim().is_empty() {
            continue;
        }
        let conv: LocomoConversation = serde_json::from_str(&line).map_err(|e| {
            BenchError::Other(format!(
                "locomo conversations.jsonl line {}: {e}",
                lineno + 1
            ))
        })?;
        conversations.push(conv);
    }
    Ok(conversations)
}


// ---------------------------------------------------------------------------
// Async-to-sync bridge (engramai retrieval API is `async fn`, the rest of
// engram-bench is sync; per the precedent in
// `crates/engramai/src/retrieval/api.rs::block_on`, we use a tiny noop
// waker — engram-bench's `Cargo.toml` deliberately avoids tokio).
//
// The retrieval stub today returns Ready synchronously after the first
// poll. When the real implementation lands and might suspend on I/O,
// this driver will need a real runtime; the change is local to this
// helper.
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
// Per-query replay: ingest a conversation, then run every gold question
// against `Memory::graph_query_locked`. Returns one [`LocomoQuery`] per
// gold question, with `predicted` = top-1 result's content (or "" on
// retrieval error) and a parallel `latency_ms` vector for the JSONL.
// ---------------------------------------------------------------------------

/// Per-query timing record — companion to the returned `LocomoQuery`
/// vector. Kept separate from `LocomoQuery` so the scorer's data
/// structure stays a pure scoring contract (no measurement noise).
///
/// Visibility note: `pub(crate)` (not private) because
/// `write_per_query_jsonl` is `pub(crate)` for unit tests in this
/// module's `tests` block; the type therefore needs to be at least as
/// visible as the function that consumes it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryTiming {
    pub(crate) latency_ms: f64,
}

/// Replay one conversation: fresh DB → ingest every episode → per-query
/// retrieve top-1 → return scorer-shaped query records + per-query
/// latency. `RetrievalError` on any individual query is handled
/// per-query (predicted = "", latency captured) so a single failure
/// doesn't tank the entire conversation.
fn replay_conversation(
    conv: &LocomoConversation,
) -> Result<(Vec<LocomoQuery>, Vec<QueryTiming>), BenchError> {
    use engramai::retrieval::api::{GraphQuery, ScoredResult};

    let mut memory = fresh_in_memory_db()?;

    // Step 2 (design §3.1): replay every episode in order.
    for (ep_idx, episode) in conv.episodes.iter().enumerate() {
        memory.ingest_with_stats(episode).map_err(|e| {
            BenchError::Other(format!(
                "locomo replay: conversation `{}` episode {} ingest failed: {e}",
                conv.conversation_id, ep_idx
            ))
        })?;
    }

    // Step 3: query each gold question; capture top-1 content as the
    // predicted answer.
    let mut queries: Vec<LocomoQuery> = Vec::with_capacity(conv.questions.len());
    let mut timings: Vec<QueryTiming> = Vec::with_capacity(conv.questions.len());

    for q in &conv.questions {
        let started = Instant::now();
        let resp = block_on(
            memory.graph_query_locked(GraphQuery::new(q.question.clone()).with_limit(1)),
        );
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

        let predicted = match resp {
            Ok(response) => match response.results.first() {
                Some(ScoredResult::Memory { record, .. }) => record.content.clone(),
                // Topic results have no plain answer string for LOCOMO's
                // exact-match scorer; treat as empty to score 0.0
                // (we deliberately don't synthesise a string from a topic
                // — that would be implicit answer-shaping the scorer
                // can't audit).
                Some(ScoredResult::Topic { .. }) => String::new(),
                None => String::new(),
            },
            // Retrieval error → empty prediction → score 0.0. The error
            // surfaces in the per-query JSONL via empty `predicted` and
            // is *not* swallowed silently in the summary: the run-level
            // GateStatus::Error path triggers when the *overall* score
            // can't be computed (zero queries).
            Err(_) => String::new(),
        };

        queries.push(LocomoQuery {
            id: q.id.clone(),
            category: q.category.clone(),
            predicted,
            gold: q.gold.clone(),
        });
        timings.push(QueryTiming { latency_ms });
    }

    Ok((queries, timings))
}

// ---------------------------------------------------------------------------
// Artifact emission (design §3.1 outputs).
// ---------------------------------------------------------------------------

/// Per-query JSONL row schema (design §3.1
/// `locomo_per_query.jsonl`).
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
/// it), so we use a simple `<timestamp>_locomo` directory. The harness
/// runner later renames or symlinks to the canonical
/// `<timestamp>_<driver>_<short-sha>` layout once the record exists
/// (design §6.2).
fn ensure_run_dir(output_root: &Path) -> Result<PathBuf, BenchError> {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let dir = output_root.join(format!("{ts}_locomo"));
    fs::create_dir_all(&dir).map_err(BenchError::IoError)?;
    Ok(dir)
}

/// Write `locomo_summary.json` per design §3.1.
pub(crate) fn write_summary_json(
    dir: &Path,
    summary: &LocomoSummary,
) -> Result<PathBuf, BenchError> {
    let path = dir.join("locomo_summary.json");
    let body = serde_json::to_string_pretty(summary)
        .map_err(|e| BenchError::Other(format!("locomo summary serialize: {e}")))?;
    let mut f = fs::File::create(&path).map_err(BenchError::IoError)?;
    f.write_all(body.as_bytes()).map_err(BenchError::IoError)?;
    f.write_all(b"\n").map_err(BenchError::IoError)?;
    Ok(path)
}

/// Write `locomo_per_query.jsonl` per design §3.1 (one JSON object per
/// line, `\n`-terminated, no trailing comma noise).
pub(crate) fn write_per_query_jsonl(
    dir: &Path,
    queries: &[LocomoQuery],
    scores: &[LocomoScore],
    timings: &[QueryTiming],
) -> Result<PathBuf, BenchError> {
    assert_eq!(
        queries.len(),
        scores.len(),
        "scorer must return one score per query (LocomoScorer::score contract)"
    );
    assert_eq!(
        queries.len(),
        timings.len(),
        "replay must produce one timing per query (replay_conversation contract)"
    );

    let path = dir.join("locomo_per_query.jsonl");
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
            .map_err(|e| BenchError::Other(format!("locomo per-query serialize: {e}")))?;
        f.write_all(line.as_bytes()).map_err(BenchError::IoError)?;
        f.write_all(b"\n").map_err(BenchError::IoError)?;
    }

    Ok(path)
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Gate evaluation (design §4.1, §4.2 — table rows for GOAL-5.1, GOAL-5.2).
// ---------------------------------------------------------------------------
//
// Thresholds and comparators live in `harness::gates::REGISTRY` — this driver
// only translates its `LocomoSummary` into a `MetricSnapshot` and delegates
// evaluation to the registry-driven engine. Do NOT inline new thresholds
// here; add them to the registry (design §4.4 attestation).

use crate::harness::gates::{BaselineResolver, GateDefinition, MetricSnapshot};

/// Adapter from `ExternalBaselines` → `BaselineResolver` for the gates
/// that need an external number (currently GOAL-5.2 → Graphiti temporal).
struct LocomoBaselines<'a> {
    external: Option<&'a ExternalBaselines>,
}

impl<'a> BaselineResolver for LocomoBaselines<'a> {
    fn resolve(&self, gate: &GateDefinition) -> Result<f64, String> {
        match gate.goal {
            "GOAL-5.2" => self
                .external
                .and_then(|e| e.locomo.graphiti.temporal)
                .ok_or_else(|| {
                    "Graphiti temporal baseline unresolved in baselines/external.toml — \
                     design §5.3 placeholder must be filled before release"
                        .to_string()
                }),
            other => Err(format!("LOCOMO has no baseline for {}", other)),
        }
    }
}

/// Build a `MetricSnapshot` from the LOCOMO summary, marking missing
/// metrics explicitly so `evaluate_for_driver` returns `GateStatus::Error`
/// rather than coercing to PASS.
fn build_snapshot(summary: &LocomoSummary) -> MetricSnapshot {
    let mut snap = MetricSnapshot::new();

    // GOAL-5.1: locomo.overall — only valid if the run actually produced
    // queries. Per design §6.1: empty fixture / short-circuited driver
    // is ERROR, never silent PASS.
    if summary.n_queries == 0 {
        snap.set_missing(
            "locomo.overall",
            "n_queries == 0 (fixture empty or driver short-circuited) — \
             LOCOMO overall could not be computed",
        );
    } else {
        snap.set_number("locomo.overall", summary.overall);
    }

    // GOAL-5.2: locomo.temporal — pulled from the by_category map.
    match summary.by_category.get("temporal").copied() {
        Some(t) => snap.set_number("locomo.temporal", t),
        None => snap.set_missing(
            "locomo.temporal",
            "LOCOMO summary missing `by_category.temporal` — required for GOAL-5.2",
        ),
    }

    snap
}

/// Evaluate the LOCOMO-scoped subset of the release-gate registry against
/// `summary` and the loaded external baselines.
///
/// The set of gates (currently GOAL-5.1 and GOAL-5.2) is determined by
/// the registry; this function returns whatever has the `locomo.` prefix.
pub(crate) fn evaluate_gates(
    summary: &LocomoSummary,
    external: Option<&ExternalBaselines>,
) -> Vec<GateResult> {
    let snap = build_snapshot(summary);
    let resolver = LocomoBaselines { external };
    crate::harness::gates::evaluate_for_driver("locomo.", &snap, &resolver)
}

/// Try to load `baselines/external.toml` from the configured baseline
/// root. Missing baseline is NOT a hard error here — gate evaluation
/// will see `external = None` and surface GOAL-5.2 as `GateStatus::Error`
/// with a structured message (per design §4.4 "missing → ERROR").
fn try_load_external_baselines(baseline_root: &Path) -> Option<ExternalBaselines> {
    let path = baseline_root.join("external.toml");
    if !path.exists() {
        return None;
    }
    baselines::load_external_baselines(&path).ok()
}

// ---------------------------------------------------------------------------
// Top-level run orchestration.
// ---------------------------------------------------------------------------

fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // 1. Load + verify fixtures.
    let pin = load_source_pin(&config.fixture_root)?;
    let conversations = load_conversations(&config.fixture_root, &pin)?;

    // 2. Replay every conversation, accumulating queries + timings.
    let mut all_queries: Vec<LocomoQuery> = Vec::new();
    let mut all_timings: Vec<QueryTiming> = Vec::new();
    for conv in &conversations {
        let (qs, ts) = replay_conversation(conv)?;
        all_queries.extend(qs);
        all_timings.extend(ts);
    }

    // 3. Score everything in one shot — scorer aggregates by category
    //    and overall (design §3.1 step 5).
    let scorer = LocomoScorer;
    let (scores, summary) = scorer.score(&all_queries);

    // 4. Emit artifacts.
    let dir = ensure_run_dir(&config.output_root)?;
    write_summary_json(&dir, &summary)?;
    write_per_query_jsonl(&dir, &all_queries, &scores, &all_timings)?;

    // 5. Evaluate gates against external baselines.
    let external = try_load_external_baselines(&config.baseline_root);
    let gates = evaluate_gates(&summary, external.as_ref());

    // 6. Compose summary_json for the harness — same shape we wrote to
    //    disk PLUS the source pin so the reproducibility record can
    //    reconstruct dataset provenance.
    let summary_json = serde_json::json!({
        "locomo_overall": summary.overall,
        "locomo_by_category": summary.by_category,
        "locomo_n_queries": summary.n_queries,
        "locomo_version_pin": pin.version_pin,
    });

    // The reproducibility record proper is written by the harness
    // runner (task:bench-impl-repro sub-task 2). We hand back the
    // run directory so the runner knows where to land it.
    Ok(RunReport {
        driver: Driver::Locomo,
        record_path: dir.join("reproducibility.toml"),
        gates,
        summary_json,
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
//
// Coverage philosophy:
//
// - **Pure logic** (gate eval, summary math, JSONL/JSON shape) is
//   unit-tested directly with synthetic `LocomoSummary` / `LocomoQuery`
//   values. No fixture I/O.
// - **Fixture loader** is tested with a tempdir that mirrors the
//   on-disk layout (`fixtures/locomo/source.toml` + the JSONL file).
//   SHA mismatch + missing fixture each get their own test.
// - **End-to-end run** (`run_impl`) is tested with a tempdir that
//   contains a 2-conversation / 4-query mini-fixture. It asserts
//   artifacts land on disk in the documented shape and that gate
//   evaluation surfaces `Error` (because `graph_query_locked` is
//   currently a stub returning `RetrievalError::Internal` → empty
//   predictions → 0.0 overall → gate `Fail`/`Error`).
//   Marked `#[ignore]` until the retrieval pipeline lands so CI
//   doesn't have to ship a fixture for a not-yet-functional driver.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::{Comparator, GateStatus, Priority};
    use std::collections::BTreeMap;

    // --- Helpers ------------------------------------------------------------

    fn summary(overall: f64, by_cat: &[(&str, f64)], n: usize) -> LocomoSummary {
        let mut bc: BTreeMap<String, f64> = BTreeMap::new();
        for (k, v) in by_cat {
            bc.insert((*k).to_string(), *v);
        }
        LocomoSummary {
            overall,
            by_category: bc,
            n_queries: n,
        }
    }

    fn external_with_temporal(t: Option<f64>) -> ExternalBaselines {
        // Build via TOML round-trip — `ExternalBaselines` and its
        // sub-tables are only `Deserialize`/`Serialize`, with private
        // fields that Rust would not let a unit test set directly. If
        // `t` is None we emit no `temporal` key so the field
        // deserialises as None (it's `Option<f64>`).
        let temporal_block = match t {
            Some(v) => format!("temporal = {v}\n"),
            None => String::new(),
        };
        let toml_body = format!(
            r#"
[locomo.mem0]
overall = 0.65
source = "test"
url = "https://example.invalid/"

[locomo.graphiti]
{temporal_block}source = "test"
url = "https://example.invalid/"
"#
        );
        toml::from_str(&toml_body).expect("hand-crafted external.toml must parse")
    }

    // --- Gate evaluation ---------------------------------------------------

    /// GOAL-5.1 passes when overall ≥ 0.685; both gates report Pass when
    /// temporal also clears the Graphiti baseline.
    #[test]
    fn gates_pass_when_above_thresholds() {
        let s = summary(0.70, &[("temporal", 0.66), ("single-hop", 0.80)], 100);
        let ext = external_with_temporal(Some(0.65));
        let gates = evaluate_gates(&s, Some(&ext));

        assert_eq!(gates.len(), 2);
        let g51 = gates.iter().find(|g| g.goal == "GOAL-5.1").unwrap();
        let g52 = gates.iter().find(|g| g.goal == "GOAL-5.2").unwrap();
        assert_eq!(g51.status, GateStatus::Pass);
        assert_eq!(g52.status, GateStatus::Pass);
        assert_eq!(g51.priority, Priority::P0);
        assert_eq!(g52.priority, Priority::P0);
    }

    /// GOAL-5.1 fails when overall under threshold; the gate carries a
    /// human-readable message naming both numbers.
    #[test]
    fn gate_5_1_fails_below_threshold_with_message() {
        let s = summary(0.50, &[("temporal", 0.70)], 100);
        let ext = external_with_temporal(Some(0.65));
        let gates = evaluate_gates(&s, Some(&ext));

        let g51 = gates.iter().find(|g| g.goal == "GOAL-5.1").unwrap();
        assert_eq!(g51.status, GateStatus::Fail);
        let msg = &g51.message;
        assert!(msg.contains("0.5"), "message should reference observed metric, got: {msg}");
        assert!(
            msg.contains("0.685"),
            "message should reference threshold 0.685, got: {msg}"
        );
    }

    /// Empty fixture (`n_queries == 0`) must surface as `Error`, never
    /// silent zero-pass. Per design §6.1 invariant.
    #[test]
    fn gate_5_1_errors_on_zero_queries() {
        let s = summary(0.0, &[], 0);
        let ext = external_with_temporal(Some(0.65));
        let gates = evaluate_gates(&s, Some(&ext));

        let g51 = gates.iter().find(|g| g.goal == "GOAL-5.1").unwrap();
        assert_eq!(g51.status, GateStatus::Error);
        assert!(g51.message.contains("n_queries"));
    }

    /// GOAL-5.2 errors (not fails) when the Graphiti temporal baseline
    /// is unresolved (None), per design §5.3 placeholder semantics.
    #[test]
    fn gate_5_2_errors_when_graphiti_baseline_unresolved() {
        let s = summary(0.70, &[("temporal", 0.66)], 100);
        let ext = external_with_temporal(None);
        let gates = evaluate_gates(&s, Some(&ext));

        let g52 = gates.iter().find(|g| g.goal == "GOAL-5.2").unwrap();
        assert_eq!(g52.status, GateStatus::Error);
        let msg = &g52.message;
        assert!(
            msg.contains("Graphiti") && msg.contains("baseline"),
            "msg should explain the placeholder, got: {msg}"
        );
    }

    /// GOAL-5.2 errors when the summary lacks the `temporal` bucket
    /// entirely (e.g. dataset filtered out all temporal queries). The
    /// driver must not silently report 0.0.
    #[test]
    fn gate_5_2_errors_when_summary_missing_temporal_category() {
        let s = summary(0.70, &[("single-hop", 0.66)], 100); // no temporal
        let ext = external_with_temporal(Some(0.65));
        let gates = evaluate_gates(&s, Some(&ext));

        let g52 = gates.iter().find(|g| g.goal == "GOAL-5.2").unwrap();
        assert_eq!(g52.status, GateStatus::Error);
        assert!(g52.message.contains("by_category.temporal"));
    }

    /// Both gates report `observed` and `threshold` correctly even on
    /// failure — the reproducibility record consumes these directly.
    #[test]
    fn gate_results_carry_metric_and_threshold_on_fail() {
        let s = summary(0.40, &[("temporal", 0.30)], 100);
        let ext = external_with_temporal(Some(0.55));
        let gates = evaluate_gates(&s, Some(&ext));

        let g51 = gates.iter().find(|g| g.goal == "GOAL-5.1").unwrap();
        assert_eq!(g51.observed, Some(0.40));
        assert_eq!(g51.threshold, Some(0.685));
        assert_eq!(g51.comparator, Comparator::Ge);

        let g52 = gates.iter().find(|g| g.goal == "GOAL-5.2").unwrap();
        assert_eq!(g52.observed, Some(0.30));
        assert_eq!(g52.threshold, Some(0.55));
    }

    // --- Artifact emission --------------------------------------------------

    /// `locomo_summary.json` is valid JSON with exactly the documented
    /// shape (overall, by_category, n_queries). Round-trips through
    /// `serde_json::Value`.
    #[test]
    fn summary_json_has_documented_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = summary(0.71, &[("temporal", 0.60), ("single-hop", 0.80)], 50);
        let path = write_summary_json(dir.path(), &s).expect("write summary");
        assert!(path.exists());

        let body = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(v["overall"], 0.71);
        assert_eq!(v["n_queries"], 50);
        assert_eq!(v["by_category"]["temporal"], 0.60);
        assert_eq!(v["by_category"]["single-hop"], 0.80);
    }

    /// `locomo_per_query.jsonl` is one JSON object per line, each line
    /// `\n`-terminated. Asserts the row schema documented in §3.1.
    #[test]
    fn per_query_jsonl_one_object_per_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let queries = vec![
            LocomoQuery {
                id: "q1".into(),
                category: "temporal".into(),
                predicted: "yes".into(),
                gold: "yes".into(),
            },
            LocomoQuery {
                id: "q2".into(),
                category: "single-hop".into(),
                predicted: "Paris".into(),
                gold: "London".into(),
            },
        ];
        let scorer = LocomoScorer;
        let (scores, _) = scorer.score(&queries);
        let timings = vec![
            QueryTiming { latency_ms: 12.5 },
            QueryTiming { latency_ms: 7.0 },
        ];

        let path =
            write_per_query_jsonl(dir.path(), &queries, &scores, &timings).expect("write");
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);

        let row0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(row0["id"], "q1");
        assert_eq!(row0["category"], "temporal");
        assert_eq!(row0["score"], 1.0);
        assert_eq!(row0["latency_ms"], 12.5);

        let row1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(row1["id"], "q2");
        assert_eq!(row1["score"], 0.0);
        assert_eq!(row1["latency_ms"], 7.0);
    }

    // --- Fixture loader -----------------------------------------------------

    /// Helper: lay out a fake `fixtures/locomo/<sha>/` tree with a
    /// `source.toml` whose hash matches `body`.
    fn write_fake_fixture(
        root: &Path,
        version_pin: &str,
        conv_jsonl: &str,
    ) -> (PathBuf, String) {
        use sha2::{Digest, Sha256};

        let conv_path_rel = "conversations.jsonl";
        let conv_path = root
            .join("locomo")
            .join(version_pin)
            .join(conv_path_rel);
        std::fs::create_dir_all(conv_path.parent().unwrap()).unwrap();
        std::fs::write(&conv_path, conv_jsonl.as_bytes()).unwrap();

        let sha = hex::encode(Sha256::digest(conv_jsonl.as_bytes()));
        let source_toml = format!(
            r#"version_pin = "{version_pin}"

[fixtures.conversations]
path = "{conv_path_rel}"
sha256 = "{sha}"
"#
        );
        let source_path = root.join("locomo").join("source.toml");
        std::fs::write(&source_path, source_toml).unwrap();
        (conv_path, sha)
    }

    /// `load_source_pin` + `load_conversations` returns conversations in
    /// file order with the right field shape.
    #[test]
    fn loader_parses_jsonl_in_file_order() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = "\
{\"conversation_id\":\"c0\",\"episodes\":[\"e0a\",\"e0b\"],\"questions\":[{\"id\":\"q0\",\"category\":\"temporal\",\"question\":\"when\",\"gold\":\"never\"}]}
{\"conversation_id\":\"c1\",\"episodes\":[\"e1\"],\"questions\":[{\"id\":\"q1\",\"category\":\"single-hop\",\"question\":\"who\",\"gold\":\"alice\"}]}
";
        write_fake_fixture(dir.path(), "abc123", jsonl);

        let pin = load_source_pin(dir.path()).expect("source.toml parses");
        assert_eq!(pin.version_pin, "abc123");

        let convs = load_conversations(dir.path(), &pin).expect("conversations parse");
        assert_eq!(convs.len(), 2);
        assert_eq!(convs[0].conversation_id, "c0");
        assert_eq!(convs[0].episodes, vec!["e0a", "e0b"]);
        assert_eq!(convs[0].questions[0].category, "temporal");
        assert_eq!(convs[1].conversation_id, "c1");
    }

    /// SHA pin mismatch → ChecksumMismatch, not a silent load.
    #[test]
    fn loader_rejects_sha_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = "{\"conversation_id\":\"c0\",\"episodes\":[],\"questions\":[]}\n";
        write_fake_fixture(dir.path(), "abc123", jsonl);

        // Tamper: rewrite the file under the same path with different bytes.
        let tampered = "{\"conversation_id\":\"TAMPERED\",\"episodes\":[],\"questions\":[]}\n";
        let conv_path = dir.path().join("locomo").join("abc123").join("conversations.jsonl");
        std::fs::write(&conv_path, tampered).unwrap();

        let pin = load_source_pin(dir.path()).unwrap();
        let err = load_conversations(dir.path(), &pin).expect_err("sha must mismatch");
        assert!(
            matches!(err, BenchError::ChecksumMismatch { .. }),
            "expected ChecksumMismatch, got {err:?}"
        );
    }

    /// Missing `source.toml` → FixtureMissing, not Other.
    #[test]
    fn loader_reports_missing_source_toml() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_source_pin(dir.path()).expect_err("must fail");
        assert!(
            matches!(err, BenchError::FixtureMissing(_)),
            "expected FixtureMissing, got {err:?}"
        );
    }

    /// `source.toml` without a `[fixtures.conversations]` entry is a
    /// structural error: we surface it as `BenchError::Other` with an
    /// explicit message rather than panicking.
    #[test]
    fn loader_rejects_source_toml_missing_conversations_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("locomo")).unwrap();
        // Valid TOML, valid `version_pin`, valid (empty) `[fixtures]`
        // table — but no `conversations` entry inside it. This is the
        // shape we want to reject at `load_conversations` time, not at
        // `load_source_pin` time (parsing must succeed; the structural
        // gap surfaces only when we try to use it).
        std::fs::write(
            dir.path().join("locomo").join("source.toml"),
            "version_pin = \"x\"\n[fixtures]\n",
        )
        .unwrap();

        let pin = load_source_pin(dir.path()).expect("parses");
        let err = load_conversations(dir.path(), &pin).expect_err("must fail");
        match err {
            BenchError::Other(msg) => {
                assert!(msg.contains("[fixtures.conversations]"), "msg: {msg}");
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    // --- Driver trait wiring -----------------------------------------------

    /// `BenchDriver` impl exposes the canonical (Driver, Stage, CostTier)
    /// triple — these are scheduling-relevant per design §4.3 and must
    /// never silently change.
    #[test]
    fn driver_metadata_matches_design_3_1_and_4_3() {
        let d = LocomoDriver::new();
        assert_eq!(d.name(), Driver::Locomo);
        assert_eq!(d.stage(), Stage::Stage2);
        assert_eq!(d.cost_tier(), CostTier::Expensive);
    }

    // --- End-to-end (gated until retrieval lands) --------------------------

    /// Smoke test: full `run_impl` against a tiny on-disk fixture.
    /// Currently `#[ignore]`d because `Memory::graph_query_locked` is a
    /// stub; once `task:retr-impl-orchestrator` lands and the locked
    /// fusion path returns real results, removing the ignore turns this
    /// into a real driver smoke test.
    ///
    /// What it asserts (when un-ignored):
    /// - Artifacts land on disk under `output_root` with the documented
    ///   filenames.
    /// - `RunReport.gates` contains exactly two entries (GOAL-5.1, GOAL-5.2).
    /// - `summary_json` carries `locomo_overall`, `locomo_by_category`,
    ///   `locomo_n_queries`, and `locomo_version_pin`.
    #[test]
    #[ignore = "depends on Memory::graph_query_locked — currently RetrievalError::Internal stub (task:retr-impl-orchestrator)"]
    fn run_impl_emits_artifacts_and_gates() {
        let fix_dir = tempfile::tempdir().unwrap();
        let out_dir = tempfile::tempdir().unwrap();
        let base_dir = tempfile::tempdir().unwrap();

        let jsonl = "\
{\"conversation_id\":\"c0\",\"episodes\":[\"alice met bob in 2020\"],\"questions\":[{\"id\":\"q0\",\"category\":\"temporal\",\"question\":\"when did they meet\",\"gold\":\"2020\"}]}
";
        write_fake_fixture(fix_dir.path(), "test-sha", jsonl);

        // External baseline — Graphiti temporal pinned for gate eval.
        std::fs::write(
            base_dir.path().join("external.toml"),
            r#"
[locomo.mem0]
overall = 0.6
source = "test"
url = "https://example.invalid/"

[locomo.graphiti]
temporal = 0.5
source = "test"
url = "https://example.invalid/"
"#,
        )
        .unwrap();

        let cfg = HarnessConfig {
            fixture_root: fix_dir.path().to_path_buf(),
            baseline_root: base_dir.path().to_path_buf(),
            output_root: out_dir.path().to_path_buf(),
            parallel_limit: 1,
            seed: 0,
            override_gate: None,
            rationale_file: None,
        };

        let report = LocomoDriver::new()
            .run(&cfg)
            .expect("run completes (even if every gate Errors, run() itself returns Ok)");
        assert_eq!(report.driver, Driver::Locomo);
        assert_eq!(report.gates.len(), 2);
        assert!(report.gates.iter().any(|g| g.goal == "GOAL-5.1"));
        assert!(report.gates.iter().any(|g| g.goal == "GOAL-5.2"));

        let v = &report.summary_json;
        assert!(v.get("locomo_overall").is_some());
        assert_eq!(v["locomo_version_pin"], "test-sha");
    }
}
