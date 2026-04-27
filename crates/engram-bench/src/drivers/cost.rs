//! Cost driver (design §3.3).
//!
//! Implements [`crate::harness::BenchDriver`] for the **N=500 ingest
//! cost harness**. Per design §3.3:
//!
//! 1. Load the corpus pin (`fixtures/cost/source.toml`) — references the
//!    LOCOMO test split's pinned SHA *and* the anonymized rustclaw
//!    production trace's pinned SHA. The 500 episodes are
//!    deterministically sampled: 250 LOCOMO indices + 250 rustclaw
//!    indices, both committed in `cost_corpus.toml` (so a re-run picks
//!    bit-identical episodes).
//! 2. For each episode, spin up a fresh in-memory engramai `Memory`
//!    and call [`engramai::Memory::ingest_with_stats`]. The returned
//!    [`engramai::resolution::ResolutionStats`] is folded into a
//!    running aggregate.
//! 3. After all 500 episodes, compute summary metrics:
//!    `average_llm_calls = total_calls / 500` plus a per-stage
//!    breakdown (extraction / resolve / persist).
//! 4. Emit two artifacts under the run directory: `cost_summary.json`
//!    and `cost_per_episode.jsonl` (design §3.3 output spec).
//! 5. Evaluate gate **GOAL-5.4** (`average ≤ 3.0`) and return it in
//!    the [`crate::harness::RunReport`].
//!
//! ## Stage / cost
//!
//! `Stage1 / Medium`: pure ingest, no retrieval pipeline involvement,
//! no LLM scorer. Per design §4.3 it sits in stage-1 with the other
//! fixture-only deterministic drivers, and is the dominant
//! medium-cost driver of the suite (≈ 30 minutes wall on a full run).
//!
//! ## Counter source — GOAL-2.11 placeholder
//!
//! Design §3.3 binds this driver's `average_llm_calls` to the
//! per-stage counters owned by **GOAL-2.11** in v03-resolution. As of
//! the latest reading of the v0.3 graph (`gid_tasks status=todo` on
//! `goal:2.11`), that counter is **not yet exposed by engramai**:
//! [`engramai::resolution::ResolutionStats`] currently exposes
//! entity/edge counts, decision counts, stage failures, and stage
//! durations — but no `llm_calls_by_stage`. Worse, the convenience
//! wrapper [`engramai::Memory::ingest_with_stats`] presently returns
//! `ResolutionStats::default()` regardless (the real counters land
//! asynchronously in the trace row; benchmarks must drain).
//!
//! Rather than block this driver on the upstream counter landing, we:
//!
//! - Build the full driver (corpus loader, fresh-DB-per-episode loop,
//!   aggregate, artifact emission, gate plumbing) end-to-end so the
//!   integration surface is in place.
//! - Record what we *can* observe today (entities/edges extracted,
//!   stage-failure counts, stage durations) as proxy fields of the
//!   per-episode JSONL — useful as a sanity check that ingest ran.
//! - Mark the LLM-call metrics with a structured *placeholder*:
//!   `summary.average_llm_calls = None`, `summary.by_stage = {}`, plus
//!   a `placeholder_until_goal_2_11_lands: true` flag in the summary
//!   JSON so a CI gate evaluator can distinguish "not yet wired" from
//!   "ran but no calls observed".
//! - Gate evaluation: when the metric is `MetricValue::Missing(...)`
//!   the registry maps it to `GateStatus::Error` (per design §4.4
//!   "missing → ERROR"), so the release pipeline cannot silently pass
//!   GOAL-5.4 while the counter is still stubbed.
//!
//! When GOAL-2.11 lands, the only change required here is in
//! [`fold_episode_stats`]: read `stats.llm_calls_total` and
//! `stats.llm_calls_by_stage` directly. The rest of the driver
//! (corpus determinism, artifact format, gate keys) is stable.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::harness::gates::{GateResult, MetricSnapshot};
use crate::harness::{
    fresh_in_memory_db, verify_fixture_sha, BenchDriver, BenchError, CostTier, Driver,
    HarnessConfig, RunReport, Stage,
};

// ---------------------------------------------------------------------------
// Corpus pin (`fixtures/cost/source.toml`) — design §3.3 + §6.1.
// ---------------------------------------------------------------------------

/// `fixtures/cost/source.toml` schema. Pins the upstream LOCOMO commit
/// SHA (test split is reused — same dataset as the LOCOMO driver, just
/// a deterministic 250-index sample) and a separate
/// `rustclaw_trace_pin` for the anonymized production trace.
///
/// The committed `corpus_seed` + the two pins together let a re-run
/// reconstruct the exact 500-episode list:
///
/// 1. Load the LOCOMO conversations file (verified by its pinned SHA).
/// 2. Flatten its `episodes: Vec<String>` across all conversations,
///    enumerated in file order — yields a flat episode list.
/// 3. Sample 250 indices using `corpus_sample::deterministic_sample`
///    seeded with `corpus_seed`.
/// 4. Repeat for the rustclaw trace (`rustclaw_episodes.jsonl`).
/// 5. Concatenate (LOCOMO half, then rustclaw half) → 500 episodes.
///
/// Per design §785: "Given a fixed seed and corpus, the cost harness
/// must select the same 500 episodes across runs (assert equality of
/// selection indices)." The committed `selection_indices.toml`
/// alongside `source.toml` is the *expected* output of this sampler;
/// the loader cross-checks it on every run as a determinism trip-wire.
#[derive(Debug, Clone, Deserialize)]
pub struct CostSourcePin {
    /// LOCOMO upstream version pin (commit SHA — same convention as
    /// `fixtures/locomo/source.toml`). The flat episode list is built
    /// from `<version_pin>/conversations.jsonl`.
    pub locomo_version_pin: String,
    /// Rustclaw production-trace version pin: a date-stamped tag for
    /// the anonymized snapshot the cost harness consumes (e.g.
    /// `rustclaw-trace-2026-04-22`). The on-disk path resolves to
    /// `<rustclaw_version_pin>/rustclaw_episodes.jsonl` under
    /// `fixtures/cost/`.
    pub rustclaw_version_pin: String,
    /// Seed for the deterministic 250+250 sampler. Committed; never
    /// regenerated at runtime.
    pub corpus_seed: u64,
    /// Per-fixture SHA-256 pins (same shape as the LOCOMO driver's
    /// fixture map). Required entries:
    ///
    /// - `locomo_conversations`: the LOCOMO source file under
    ///   `<locomo_version_pin>/`
    /// - `rustclaw_episodes`: the anonymized rustclaw episode file
    ///   under `<rustclaw_version_pin>/`
    /// - `selection_indices`: the committed determinism trip-wire
    ///   (the 500 chosen indices, half LOCOMO half rustclaw).
    pub fixtures: BTreeMap<String, CostFixturePin>,
}

/// One row of `[fixtures.<name>]` in the cost-corpus source pin.
#[derive(Debug, Clone, Deserialize)]
pub struct CostFixturePin {
    /// Path relative to `fixtures/cost/`.
    pub path: String,
    /// SHA-256 hex of the file at `path` (lowercase, 64 chars).
    pub sha256: String,
}

/// Committed per-corpus determinism trip-wire (`selection_indices.toml`).
/// Loaded after `source.toml`, cross-checked against the live sampler.
///
/// We deliberately commit this rather than recompute on every run for
/// two reasons:
///
/// - **Audit:** a reviewer can grep the file and see exactly which
///   episodes the cost number is computed over.
/// - **Drift detection:** if anyone ever changes the seed or sampler
///   logic, the live sampler's output diverges from the committed
///   indices and the run aborts with `BenchError::Other("cost corpus
///   selection indices drifted: …")` BEFORE any numbers are reported.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CommittedSelection {
    /// Indices into the flattened LOCOMO episode list — sorted
    /// ascending by the sampler convention. Length must be 250.
    pub locomo_indices: Vec<usize>,
    /// Indices into the rustclaw `rustclaw_episodes.jsonl` line list.
    /// Length must be 250.
    pub rustclaw_indices: Vec<usize>,
}

/// One LOCOMO conversation entry — minimal subset needed to flatten
/// out the episode list for sampling. We re-parse here (instead of
/// reusing `crate::drivers::locomo::LocomoConversation`) so the cost
/// driver doesn't take a dependency on the LOCOMO driver's internal
/// types; the only stable contract we share is the on-disk JSONL
/// schema's `episodes: Vec<String>` field, and that's what we extract.
#[derive(Debug, Clone, Deserialize)]
struct LocomoConversationLite {
    /// Stable conversation id (kept for diagnostic messages only —
    /// the sampler operates on the flattened episode index, not the
    /// `(conv_id, ep_idx)` pair, so re-flattening across LOCOMO
    /// versions changes the indices and is correctly detected by the
    /// `selection_indices` trip-wire).
    #[allow(dead_code)]
    conversation_id: String,
    /// The episodes — pushed onto the flat list in file order.
    episodes: Vec<String>,
}

/// One rustclaw production episode (anonymized per design §9.3 /
/// §9.3.1). The on-disk format is `rustclaw_episodes.jsonl` — one JSON
/// object per line, fields:
#[derive(Debug, Clone, Deserialize)]
struct RustclawEpisode {
    /// Stable episode id assigned by the anonymizer pipeline. Used in
    /// `cost_per_episode.jsonl` so a reviewer can trace a high-cost
    /// outlier back to a specific anonymized trace.
    episode_id: String,
    /// The anonymized natural-language episode body — fed verbatim to
    /// `Memory::ingest_with_stats`.
    content: String,
}

/// Read and parse `fixtures/cost/source.toml` under `fixture_root`.
pub(crate) fn load_source_pin(fixture_root: &Path) -> Result<CostSourcePin, BenchError> {
    let path = fixture_root.join("cost").join("source.toml");
    if !path.exists() {
        return Err(BenchError::FixtureMissing(path));
    }
    let body = fs::read_to_string(&path).map_err(BenchError::IoError)?;
    let pin: CostSourcePin = toml::from_str(&body)
        .map_err(|e| BenchError::Other(format!("cost source.toml parse {}: {e}", path.display())))?;

    // Required fixture entries — fail loud if any are missing rather
    // than silently zero-filling a corpus.
    for required in ["locomo_conversations", "rustclaw_episodes", "selection_indices"] {
        if !pin.fixtures.contains_key(required) {
            return Err(BenchError::Other(format!(
                "cost source.toml missing required fixture entry [fixtures.{required}]"
            )));
        }
    }
    Ok(pin)
}

/// Resolve a fixture entry to its absolute path. LOCOMO entries live
/// under `<fixture_root>/cost/<locomo_version_pin>/`; rustclaw entries
/// under `<fixture_root>/cost/<rustclaw_version_pin>/`; the
/// committed `selection_indices` lives directly under
/// `<fixture_root>/cost/`.
fn fixture_path(fixture_root: &Path, pin: &CostSourcePin, fixture_name: &str) -> PathBuf {
    let entry = pin
        .fixtures
        .get(fixture_name)
        .expect("fixture entry presence is verified by load_source_pin");
    let base = fixture_root.join("cost");
    match fixture_name {
        "locomo_conversations" => base.join(&pin.locomo_version_pin).join(&entry.path),
        "rustclaw_episodes" => base.join(&pin.rustclaw_version_pin).join(&entry.path),
        // selection_indices.toml is corpus-meta, not version-pinned to
        // either upstream — it lives at the corpus root.
        _ => base.join(&entry.path),
    }
}

/// Load + SHA-verify the committed `selection_indices.toml`.
pub(crate) fn load_committed_selection(
    fixture_root: &Path,
    pin: &CostSourcePin,
) -> Result<CommittedSelection, BenchError> {
    let path = fixture_path(fixture_root, pin, "selection_indices");
    let entry = pin
        .fixtures
        .get("selection_indices")
        .expect("verified by load_source_pin");
    verify_fixture_sha(&path, &entry.sha256)?;
    let body = fs::read_to_string(&path).map_err(BenchError::IoError)?;
    let sel: CommittedSelection = toml::from_str(&body).map_err(|e| {
        BenchError::Other(format!(
            "cost selection_indices.toml parse {}: {e}",
            path.display()
        ))
    })?;

    // Length invariants — violation here is corpus tampering, not a
    // sampler bug, so fail loud BEFORE running the sampler.
    if sel.locomo_indices.len() != 250 {
        return Err(BenchError::Other(format!(
            "cost selection_indices.toml: locomo_indices.len() = {}, expected 250",
            sel.locomo_indices.len()
        )));
    }
    if sel.rustclaw_indices.len() != 250 {
        return Err(BenchError::Other(format!(
            "cost selection_indices.toml: rustclaw_indices.len() = {}, expected 250",
            sel.rustclaw_indices.len()
        )));
    }
    Ok(sel)
}

/// Load LOCOMO conversations + flatten into a single `Vec<String>` of
/// episode bodies in file order. SHA-verified.
fn load_locomo_episodes(
    fixture_root: &Path,
    pin: &CostSourcePin,
) -> Result<Vec<String>, BenchError> {
    let path = fixture_path(fixture_root, pin, "locomo_conversations");
    let entry = pin
        .fixtures
        .get("locomo_conversations")
        .expect("verified by load_source_pin");
    verify_fixture_sha(&path, &entry.sha256)?;
    let file = fs::File::open(&path).map_err(BenchError::IoError)?;
    let reader = BufReader::new(file);

    let mut flat: Vec<String> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(BenchError::IoError)?;
        if line.trim().is_empty() {
            continue;
        }
        let conv: LocomoConversationLite = serde_json::from_str(&line).map_err(|e| {
            BenchError::Other(format!(
                "cost locomo conversations.jsonl line {}: {e}",
                lineno + 1
            ))
        })?;
        flat.extend(conv.episodes);
    }
    Ok(flat)
}

/// Load rustclaw anonymized episodes (one per JSONL line). SHA-verified.
fn load_rustclaw_episodes(
    fixture_root: &Path,
    pin: &CostSourcePin,
) -> Result<Vec<RustclawEpisode>, BenchError> {
    let path = fixture_path(fixture_root, pin, "rustclaw_episodes");
    let entry = pin
        .fixtures
        .get("rustclaw_episodes")
        .expect("verified by load_source_pin");
    verify_fixture_sha(&path, &entry.sha256)?;
    let file = fs::File::open(&path).map_err(BenchError::IoError)?;
    let reader = BufReader::new(file);

    let mut episodes: Vec<RustclawEpisode> = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line.map_err(BenchError::IoError)?;
        if line.trim().is_empty() {
            continue;
        }
        let ep: RustclawEpisode = serde_json::from_str(&line).map_err(|e| {
            BenchError::Other(format!(
                "cost rustclaw_episodes.jsonl line {}: {e}",
                lineno + 1
            ))
        })?;
        episodes.push(ep);
    }
    Ok(episodes)
}

/// Deterministic sampler: given a slice length `n`, a target sample
/// size `k`, and a `seed`, return `k` distinct indices in `[0, n)`.
///
/// Uses a small splitmix64-style PRNG keyed by `seed` then a partial
/// Fisher-Yates: O(n) memory, O(k) shuffle steps, fully deterministic
/// across architectures (fixed integer arithmetic, no float ordering).
/// Indices are returned **sorted ascending** so the output ordering
/// only depends on `(n, k, seed)`, not on internal swap order.
///
/// Why not the `rand` crate's `seed_from_u64`? Because `rand`'s
/// `StdRng` algorithm has changed across major versions before; a
/// hand-rolled splitmix is locked at this commit and will not silently
/// drift if we bump deps.
pub(crate) fn deterministic_sample(n: usize, k: usize, seed: u64) -> Vec<usize> {
    assert!(
        k <= n,
        "deterministic_sample: k={k} > n={n} (corpus too small for sample size)"
    );
    let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next_u64 = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    // Partial Fisher-Yates: maintain a Vec<usize> identity, swap k of
    // them with random successors, take the first k.
    let mut pool: Vec<usize> = (0..n).collect();
    for i in 0..k {
        let r = (next_u64() as usize) % (n - i);
        pool.swap(i, i + r);
    }
    let mut chosen: Vec<usize> = pool[..k].to_vec();
    chosen.sort_unstable();
    chosen
}

// ---------------------------------------------------------------------------
// Per-episode + summary types (design §3.3 output spec).
// ---------------------------------------------------------------------------

/// One row of `cost_per_episode.jsonl` (design §3.3 output).
///
/// Field naming locked to the design spec — `episode_id`,
/// `calls_by_stage`, `total_calls`. The `proxy_*` fields are the
/// observable fallback values we record while GOAL-2.11 is pending;
/// they're prefixed so a downstream consumer that grep'd for
/// `total_calls` cannot accidentally read them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostEpisodeRecord {
    /// Stable episode id. For LOCOMO half: `"locomo:<flat_index>"`.
    /// For rustclaw half: the `episode_id` field from the anonymized
    /// JSONL row (already stable across runs by construction).
    pub episode_id: String,
    /// Which corpus half this episode came from. Useful for cost
    /// breakdown ("rustclaw episodes are 1.7× more expensive than
    /// LOCOMO" → action item).
    pub source: CostEpisodeSource,
    /// Per-stage LLM call counts for this episode.
    /// **Currently `None`** until GOAL-2.11 lands the per-stage
    /// counter on `ResolutionStats`. See module docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls_by_stage: Option<BTreeMap<String, u64>>,
    /// Total LLM calls for this episode (= sum of `calls_by_stage`).
    /// **Currently `None`** for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_calls: Option<u64>,
    /// Proxy: number of entity mentions extracted (`ResolutionStats
    /// .entities_extracted`). Recorded as a sanity-check that ingest
    /// produced *something* — a corpus-wide zero here would imply
    /// a broken pipeline well before any LLM-call gate fires.
    pub proxy_entities_extracted: u64,
    /// Proxy: number of edges extracted (`ResolutionStats
    /// .edges_extracted`).
    pub proxy_edges_extracted: u64,
    /// Proxy: number of stage failures recorded for this episode.
    /// Per-episode non-zero values warrant investigation; the cost
    /// summary surfaces the corpus-wide sum.
    pub proxy_stage_failures: u64,
    /// Wall-clock duration of the `ingest_with_stats` call (millis).
    /// Not a gate input — pure observability.
    pub ingest_wall_ms: f64,
}

/// Which half of the 250+250 corpus an episode came from. Serialised
/// in lower-case (`"locomo"` / `"rustclaw"`) for downstream tooling
/// that filters JSONL on this field.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CostEpisodeSource {
    /// Sampled from the LOCOMO test split (deterministic seeded sample
    /// of 250 episodes from the flattened conversation episode list).
    Locomo,
    /// Sampled from the anonymized rustclaw production trace (250
    /// episodes from `rustclaw_episodes.jsonl`).
    Rustclaw,
}

/// `cost_summary.json` shape (design §3.3 output).
///
/// **Field-naming contract:** `n_episodes`, `total_calls`, `average`,
/// `by_stage` are all locked by the design — do not rename. The
/// `placeholder_until_goal_2_11_lands` flag is added by this
/// implementation (not in design) to prevent CI from misinterpreting a
/// stubbed run as a passing run; it's serialized whenever the LLM-call
/// fields are absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CostSummary {
    /// Always 500. Validated invariant: a run that produced fewer
    /// than 500 episode records aborts before this struct is built.
    pub n_episodes: usize,
    /// Sum of `total_calls` across all episodes. **`None`** until
    /// GOAL-2.11 lands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_calls: Option<u64>,
    /// `total_calls / n_episodes`. **`None`** until GOAL-2.11 lands.
    /// This is the metric GOAL-5.4 gates on (≤ 3.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average: Option<f64>,
    /// Per-stage breakdown of average calls. Stage names match the
    /// counter keys GOAL-2.11 will expose (currently empty).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_stage: BTreeMap<String, f64>,
    /// Sum of `proxy_entities_extracted` across all episodes. Pure
    /// observability — not a gate input.
    pub proxy_total_entities_extracted: u64,
    /// Sum of `proxy_edges_extracted` across all episodes.
    pub proxy_total_edges_extracted: u64,
    /// Sum of stage failures across all episodes. Non-zero is a
    /// potential signal of pipeline degradation; surfaced in artifact
    /// for human review even though no gate currently consumes it.
    pub proxy_total_stage_failures: u64,
    /// Aggregate wall-clock of all `ingest_with_stats` calls (ms).
    /// Not the same as run wall-clock (run includes corpus loading +
    /// artifact writing + DB construction overhead).
    pub aggregate_ingest_wall_ms: f64,
    /// **TRUE iff `total_calls`/`average`/`by_stage` are absent
    /// because the upstream counter (GOAL-2.11) has not yet landed.**
    /// CI gate evaluator MUST treat `placeholder = true` as
    /// `GateStatus::Error` for GOAL-5.4 even if the run finished
    /// without panicking — see module docs.
    pub placeholder_until_goal_2_11_lands: bool,
}

/// Fold one episode's `ResolutionStats` into a running aggregate.
/// Today this only feeds the proxy fields; once GOAL-2.11 lands, it
/// will additionally accumulate `stats.llm_calls_total` /
/// `stats.llm_calls_by_stage` into the summary's gated fields.
///
/// Pulled out into a free function (not a method on the running
/// aggregate) so unit tests can drive it with synthetic stats values.
fn fold_episode_stats(
    stats: &engramai::resolution::ResolutionStats,
    ingest_wall: Duration,
    source: CostEpisodeSource,
    episode_id: String,
) -> CostEpisodeRecord {
    CostEpisodeRecord {
        episode_id,
        source,
        // ── GOAL-2.11 placeholder. When the counter lands, replace
        //    these two `None`s with the real per-stage counts and
        //    their sum. NOTHING ELSE in this driver needs to change.
        calls_by_stage: None,
        total_calls: None,
        // ── Proxy observables: present today.
        proxy_entities_extracted: stats.entities_extracted,
        proxy_edges_extracted: stats.edges_extracted,
        proxy_stage_failures: stats.stage_failures,
        ingest_wall_ms: ingest_wall.as_secs_f64() * 1000.0,
    }
}

/// Aggregate a complete (length-500) per-episode list into the corpus
/// summary. Caller asserts length invariants; this function is total
/// and never panics on stat values.
fn aggregate_summary(records: &[CostEpisodeRecord]) -> CostSummary {
    let placeholder = records.iter().all(|r| r.total_calls.is_none());
    let total_calls: Option<u64> = if placeholder {
        None
    } else {
        Some(records.iter().filter_map(|r| r.total_calls).sum())
    };
    let average: Option<f64> = total_calls.map(|t| t as f64 / records.len() as f64);
    let by_stage: BTreeMap<String, f64> = if placeholder {
        BTreeMap::new()
    } else {
        // GOAL-2.11 future hook: when calls_by_stage is populated,
        // compute per-stage averages. The closed-form is:
        //   by_stage[s] = sum_e records[e].calls_by_stage[s] / N
        // — but we deliberately don't write that loop today, so the
        // intent is encoded as a TODO rather than as dead code that
        // could pass tests with synthetic data and then quietly
        // diverge from the GOAL-2.11 contract when it lands.
        BTreeMap::new()
    };

    CostSummary {
        n_episodes: records.len(),
        total_calls,
        average,
        by_stage,
        proxy_total_entities_extracted: records.iter().map(|r| r.proxy_entities_extracted).sum(),
        proxy_total_edges_extracted: records.iter().map(|r| r.proxy_edges_extracted).sum(),
        proxy_total_stage_failures: records.iter().map(|r| r.proxy_stage_failures).sum(),
        aggregate_ingest_wall_ms: records.iter().map(|r| r.ingest_wall_ms).sum(),
        placeholder_until_goal_2_11_lands: placeholder,
    }
}

// ---------------------------------------------------------------------------
// Driver impl + run orchestration.
// ---------------------------------------------------------------------------

/// Cost-harness driver — see module docs.
///
/// Stateless; cheap to construct. Dispatches into `run_impl`.
pub struct CostDriver;

impl BenchDriver for CostDriver {
    fn name(&self) -> Driver {
        Driver::Cost
    }

    fn stage(&self) -> Stage {
        // Design §3.3 / §4.3: cost is fixture-only and deterministic
        // (no LLM scorer in the harness itself), so it lives in
        // stage-1 along with migration-integrity and test-preservation.
        Stage::Stage1
    }

    fn cost_tier(&self) -> CostTier {
        // Design §619: "~30min" wall-clock dominated by resolution
        // latency on N=500 ingests. Medium tier.
        CostTier::Medium
    }

    fn run(&self, config: &HarnessConfig) -> Result<RunReport, BenchError> {
        run_impl(config)
    }
}

/// One row of the assembled run plan: an episode with its source-half
/// label and the id we'll emit in `cost_per_episode.jsonl`. Built once
/// from the corpus pin + sampled indices, then iterated for ingest.
struct PlannedEpisode {
    episode_id: String,
    source: CostEpisodeSource,
    content: String,
}

/// Build the 500-episode plan: 250 LOCOMO + 250 rustclaw, in the
/// canonical concat order (LOCOMO first), each half ordered by the
/// committed selection indices ascending. Cross-checks the live
/// sampler output against the committed indices — divergence aborts
/// the run before any ingest fires.
fn assemble_plan(
    fixture_root: &Path,
    pin: &CostSourcePin,
    committed: &CommittedSelection,
) -> Result<Vec<PlannedEpisode>, BenchError> {
    // Load both corpora (SHA-verified inside the loaders).
    let locomo_flat = load_locomo_episodes(fixture_root, pin)?;
    let rustclaw = load_rustclaw_episodes(fixture_root, pin)?;

    // Sampler trip-wire: re-run the sampler with the committed seed
    // and assert byte-equality against `selection_indices.toml`. If
    // anyone changed `corpus_seed` in source.toml, or accidentally
    // regenerated `selection_indices.toml` from a different source
    // length, this fires before any ingest.
    let live_locomo = deterministic_sample(locomo_flat.len(), 250, pin.corpus_seed);
    let live_rustclaw =
        deterministic_sample(rustclaw.len(), 250, pin.corpus_seed.wrapping_add(1));
    if live_locomo != committed.locomo_indices {
        return Err(BenchError::Other(format!(
            "cost corpus selection indices drifted (LOCOMO half): \
             live sampler produced different indices than \
             selection_indices.toml. \
             First mismatch at i={:?}",
            first_mismatch(&live_locomo, &committed.locomo_indices)
        )));
    }
    if live_rustclaw != committed.rustclaw_indices {
        return Err(BenchError::Other(format!(
            "cost corpus selection indices drifted (rustclaw half): \
             live sampler produced different indices than \
             selection_indices.toml. \
             First mismatch at i={:?}",
            first_mismatch(&live_rustclaw, &committed.rustclaw_indices)
        )));
    }

    // Build the plan in committed-indices order. Bounds-check each
    // index against its source length — out-of-range here would mean
    // the committed indices don't match the SHA-pinned corpus and is
    // a corpus-tampering signal.
    let mut plan: Vec<PlannedEpisode> = Vec::with_capacity(500);
    for &i in &committed.locomo_indices {
        let content = locomo_flat.get(i).cloned().ok_or_else(|| {
            BenchError::Other(format!(
                "cost LOCOMO index {i} out of range (flat list len = {})",
                locomo_flat.len()
            ))
        })?;
        plan.push(PlannedEpisode {
            episode_id: format!("locomo:{i}"),
            source: CostEpisodeSource::Locomo,
            content,
        });
    }
    for &i in &committed.rustclaw_indices {
        let ep = rustclaw.get(i).ok_or_else(|| {
            BenchError::Other(format!(
                "cost rustclaw index {i} out of range (jsonl len = {})",
                rustclaw.len()
            ))
        })?;
        plan.push(PlannedEpisode {
            episode_id: ep.episode_id.clone(),
            source: CostEpisodeSource::Rustclaw,
            content: ep.content.clone(),
        });
    }
    debug_assert_eq!(plan.len(), 500);
    Ok(plan)
}

/// Index of the first position where two slices differ (or the shorter
/// length if one is a prefix of the other). Returns `None` only when
/// both slices are byte-equal — but that path is already excluded by
/// the caller's `if !=` check, so callers can `unwrap_or(0)` safely
/// for diagnostic display.
fn first_mismatch<T: PartialEq>(a: &[T], b: &[T]) -> Option<usize> {
    a.iter().zip(b.iter()).position(|(x, y)| x != y).or({
        if a.len() != b.len() {
            Some(a.len().min(b.len()))
        } else {
            None
        }
    })
}

/// Top-level orchestration: load → plan → ingest-loop → aggregate →
/// emit → gate.
fn run_impl(config: &HarnessConfig) -> Result<RunReport, BenchError> {
    // 1. Load corpus pin + committed selection.
    let pin = load_source_pin(&config.fixture_root)?;
    let committed = load_committed_selection(&config.fixture_root, &pin)?;

    // 2. Build the 500-episode plan (sampler trip-wire fires here).
    let plan = assemble_plan(&config.fixture_root, &pin, &committed)?;

    // 3. Ingest loop: fresh in-memory `Memory` per episode, fold the
    //    returned `ResolutionStats` into a per-episode record.
    //
    //    Per design §3.3 step 1 ("a fresh engramai `Memory` instance
    //    configured with the counter reset to zero"): we construct a
    //    new Memory inside the loop rather than reusing one and
    //    relying on a hypothetical counter-reset method. Cleaner
    //    invariant: counters start at 0 because the DB is brand new.
    let mut records: Vec<CostEpisodeRecord> = Vec::with_capacity(plan.len());
    for ep in &plan {
        let mut memory = fresh_in_memory_db()?;
        let started = Instant::now();
        let stats = match memory.ingest_with_stats(&ep.content) {
            Ok((_id, stats)) => stats,
            Err(e) => {
                // A failed ingest aborts the run. Rationale: GOAL-5.4
                // is a *corpus-wide* average; a silently-skipped
                // episode would bias the denominator. Better to fail
                // loud and let the operator decide whether to fix the
                // episode (anonymizer bug?) or override.
                return Err(BenchError::Other(format!(
                    "cost ingest failed on episode `{}`: {e}",
                    ep.episode_id
                )));
            }
        };
        let wall = started.elapsed();
        records.push(fold_episode_stats(&stats, wall, ep.source, ep.episode_id.clone()));
    }

    if records.len() != 500 {
        return Err(BenchError::Other(format!(
            "cost driver internal invariant violated: \
             recorded {} episodes, expected 500",
            records.len()
        )));
    }

    // 4. Aggregate + emit artifacts.
    let summary = aggregate_summary(&records);
    let dir = ensure_run_dir(&config.output_root)?;
    write_summary_json(&dir, &summary)?;
    write_per_episode_jsonl(&dir, &records)?;

    // 5. Gate evaluation.
    let gates = evaluate_gates(&summary);

    // 6. Compose summary_json for the harness/repro layer. Mirror the
    //    on-disk shape exactly so the reproducibility record can
    //    reconstruct the gate inputs from the record alone.
    let summary_json = serde_json::to_value(&summary).map_err(|e| {
        BenchError::Other(format!("cost summary_json serialize: {e}"))
    })?;

    Ok(RunReport {
        driver: Driver::Cost,
        record_path: dir.join("reproducibility.toml"),
        gates,
        summary_json,
    })
}

// ---------------------------------------------------------------------------
// Aggregation + artifact emission.
// ---------------------------------------------------------------------------

/// Create the per-run directory under `output_root`. Mirrors the
/// LOCOMO driver's convention (`<ts>_cost`); the harness runner
/// renames/symlinks to the `<ts>_<driver>_<sha>` layout once the
/// reproducibility record exists (design §6.2).
fn ensure_run_dir(output_root: &Path) -> Result<PathBuf, BenchError> {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let dir = output_root.join(format!("{ts}_cost"));
    fs::create_dir_all(&dir).map_err(BenchError::IoError)?;
    Ok(dir)
}

/// Write `cost_summary.json` per design §3.3.
pub(crate) fn write_summary_json(
    dir: &Path,
    summary: &CostSummary,
) -> Result<PathBuf, BenchError> {
    let path = dir.join("cost_summary.json");
    let body = serde_json::to_string_pretty(summary)
        .map_err(|e| BenchError::Other(format!("cost summary serialize: {e}")))?;
    let mut f = fs::File::create(&path).map_err(BenchError::IoError)?;
    f.write_all(body.as_bytes()).map_err(BenchError::IoError)?;
    f.write_all(b"\n").map_err(BenchError::IoError)?;
    Ok(path)
}

/// Write `cost_per_episode.jsonl` per design §3.3 — one
/// [`CostEpisodeRecord`] per line, `\n`-terminated, no trailing
/// whitespace, no JSON-array wrapper.
pub(crate) fn write_per_episode_jsonl(
    dir: &Path,
    records: &[CostEpisodeRecord],
) -> Result<PathBuf, BenchError> {
    let path = dir.join("cost_per_episode.jsonl");
    let mut f = fs::File::create(&path).map_err(BenchError::IoError)?;
    for record in records {
        let body = serde_json::to_string(record)
            .map_err(|e| BenchError::Other(format!("cost per-episode serialize: {e}")))?;
        f.write_all(body.as_bytes()).map_err(BenchError::IoError)?;
        f.write_all(b"\n").map_err(BenchError::IoError)?;
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Gate evaluation (GOAL-5.4 binding).
// ---------------------------------------------------------------------------

/// Build the metric snapshot the gates registry consults. Keys follow
/// the `<driver>.<metric>` convention.
///
/// **GOAL-5.4 wiring (`cost.average_llm_calls`):**
///
/// - When `summary.average` is `Some(v)` → snapshot has
///   `MetricValue::Number(v)`.
/// - When `summary.average` is `None` (placeholder until GOAL-2.11)
///   → snapshot has `MetricValue::Missing("…GOAL-2.11…")`. The gate
///   evaluator maps `Missing` to `GateStatus::Error` per design §4.4
///   ("missing metric → ERROR, never silent pass").
///
/// We deliberately do *not* register a stub `0.0` value: that would
/// score `average ≤ 3.0` as a pass and hide the unimplemented counter
/// from CI. The error path is the safety mechanism.
fn build_snapshot(summary: &CostSummary) -> MetricSnapshot {
    let mut snap = MetricSnapshot::new();
    match summary.average {
        Some(v) => {
            snap.set_number("cost.average_llm_calls", v);
        }
        None => {
            snap.set_missing(
                "cost.average_llm_calls",
                "GOAL-2.11 per-stage LLM-call counter not yet exposed by \
                 engramai::resolution::ResolutionStats — cost driver \
                 produced placeholder summary. \
                 (See crates/engram-bench/src/drivers/cost.rs module docs.)",
            );
        }
    }
    snap
}

/// Evaluate all `cost.*` gates from the registry against `summary`.
///
/// Post-reconcile (2026-04-27): the registry has exactly one
/// `cost.*` gate — GOAL-5.4 (`cost.average_llm_calls ≤ 3.0`). The
/// previous `cost.p95_total_ms` / `cost.p95_index_ms` /
/// `cost.p95_total_p99_ms` rows have been removed; v0.3.0 ships
/// without latency gates per the current requirements.md §5.
pub(crate) fn evaluate_gates(summary: &CostSummary) -> Vec<GateResult> {
    let snap = build_snapshot(summary);
    crate::harness::gates::evaluate_for_driver("cost.", &snap, &NoBaselines)
}

/// `BaselineResolver` impl that always returns `None`. The cost driver
/// has no baseline-relative gate (GOAL-5.4 is an absolute constant:
/// `≤ 3.0`), so a baselineless resolver is correct.
struct NoBaselines;
impl crate::harness::gates::BaselineResolver for NoBaselines {
    fn resolve(
        &self,
        gate: &crate::harness::gates::GateDefinition,
    ) -> Result<f64, String> {
        Err(format!(
            "cost driver has no baseline resolver; gate {} requires a \
             baseline but the cost-counter driver only owns absolute \
             thresholds (e.g. GOAL-5.4 `≤ 3.0`). If the gates registry \
             added a baseline-relative cost gate, the cost driver's \
             `NoBaselines` resolver must be replaced.",
            gate.goal
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::gates::{GateStatus, MetricValue};
    use std::time::Duration;

    // -----------------------------------------------------------------
    // deterministic_sample
    // -----------------------------------------------------------------

    /// Same `(n, k, seed)` triple → byte-identical output across calls.
    /// This is the foundation of the entire corpus-determinism story
    /// (design §785); a regression here invalidates every committed
    /// `selection_indices.toml`.
    #[test]
    fn deterministic_sample_is_deterministic() {
        let a = deterministic_sample(1000, 250, 42);
        let b = deterministic_sample(1000, 250, 42);
        assert_eq!(a, b);
        assert_eq!(a.len(), 250);
        // Output is sorted ascending by contract.
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        // All indices in range.
        assert!(a.iter().all(|&i| i < 1000));
    }

    /// Different seeds → different samples (overwhelmingly likely; a
    /// failing assertion here would mean the splitmix is degenerate).
    #[test]
    fn deterministic_sample_is_seed_sensitive() {
        let a = deterministic_sample(1000, 250, 42);
        let b = deterministic_sample(1000, 250, 43);
        assert_ne!(a, b, "different seeds produced identical samples");
    }

    /// k = n returns the full identity permutation, sorted.
    #[test]
    fn deterministic_sample_full_takes_all() {
        let s = deterministic_sample(20, 20, 7);
        assert_eq!(s, (0..20).collect::<Vec<_>>());
    }

    /// k = 0 returns empty.
    #[test]
    fn deterministic_sample_zero_returns_empty() {
        let s = deterministic_sample(100, 0, 7);
        assert!(s.is_empty());
    }

    #[test]
    #[should_panic(expected = "k=11 > n=10")]
    fn deterministic_sample_panics_on_oversample() {
        let _ = deterministic_sample(10, 11, 7);
    }

    // -----------------------------------------------------------------
    // first_mismatch
    // -----------------------------------------------------------------

    #[test]
    fn first_mismatch_finds_diff_position() {
        assert_eq!(first_mismatch(&[1, 2, 3], &[1, 2, 4]), Some(2));
        assert_eq!(first_mismatch(&[1, 2, 3], &[1, 2, 3]), None);
        // Length-mismatch case: shorter is a prefix of longer → reports
        // shorter.len() so the diagnostic message is unambiguous.
        assert_eq!(first_mismatch(&[1, 2], &[1, 2, 3]), Some(2));
        assert_eq!(first_mismatch(&[1, 2, 3], &[1, 2]), Some(2));
    }

    // -----------------------------------------------------------------
    // fold_episode_stats / aggregate_summary / build_snapshot
    // -----------------------------------------------------------------

    /// Helper: synthesize a `ResolutionStats` with the four proxy
    /// fields the cost driver actually reads. Other fields default;
    /// the driver doesn't observe them today.
    fn synth_stats(entities: u64, edges: u64, failures: u64) -> engramai::resolution::ResolutionStats {
        let mut s = engramai::resolution::ResolutionStats::default();
        s.entities_extracted = entities;
        s.edges_extracted = edges;
        s.stage_failures = failures;
        s
    }

    /// `fold_episode_stats` copies the proxy fields verbatim and
    /// converts the wall-time to ms (f64).
    #[test]
    fn fold_episode_stats_copies_proxy_fields() {
        let stats = synth_stats(7, 11, 1);
        let rec = fold_episode_stats(
            &stats,
            Duration::from_millis(123),
            CostEpisodeSource::Locomo,
            "locomo:42".into(),
        );
        assert_eq!(rec.proxy_entities_extracted, 7);
        assert_eq!(rec.proxy_edges_extracted, 11);
        assert_eq!(rec.proxy_stage_failures, 1);
        assert!((rec.ingest_wall_ms - 123.0).abs() < 0.01);
        // GOAL-2.11 placeholder fields.
        assert!(rec.calls_by_stage.is_none());
        assert!(rec.total_calls.is_none());
        assert_eq!(rec.episode_id, "locomo:42");
        assert_eq!(rec.source, CostEpisodeSource::Locomo);
    }

    /// `aggregate_summary` on all-`None` records produces a
    /// placeholder summary. This is the path CI runs today (and will
    /// run until GOAL-2.11 lands); the gate-evaluator-Error path
    /// depends on it.
    #[test]
    fn aggregate_summary_placeholder_when_all_calls_none() {
        let recs: Vec<CostEpisodeRecord> = (0..500)
            .map(|i| CostEpisodeRecord {
                episode_id: format!("e{i}"),
                source: if i < 250 {
                    CostEpisodeSource::Locomo
                } else {
                    CostEpisodeSource::Rustclaw
                },
                calls_by_stage: None,
                total_calls: None,
                proxy_entities_extracted: 1,
                proxy_edges_extracted: 2,
                proxy_stage_failures: 0,
                ingest_wall_ms: 1.0,
            })
            .collect();
        let sum = aggregate_summary(&recs);
        assert_eq!(sum.n_episodes, 500);
        assert!(sum.placeholder_until_goal_2_11_lands);
        assert!(sum.total_calls.is_none());
        assert!(sum.average.is_none());
        assert!(sum.by_stage.is_empty());
        // Proxy aggregates summed correctly.
        assert_eq!(sum.proxy_total_entities_extracted, 500);
        assert_eq!(sum.proxy_total_edges_extracted, 1000);
        assert_eq!(sum.proxy_total_stage_failures, 0);
        assert!((sum.aggregate_ingest_wall_ms - 500.0).abs() < 1e-9);
    }

    /// Forward-compat: when GOAL-2.11 lands and records carry real
    /// `total_calls`, `aggregate_summary` computes the gate input
    /// (`average = total / n`) and clears the placeholder flag.
    #[test]
    fn aggregate_summary_computes_average_when_calls_present() {
        let recs: Vec<CostEpisodeRecord> = (0..500)
            .map(|i| CostEpisodeRecord {
                episode_id: format!("e{i}"),
                source: CostEpisodeSource::Locomo,
                calls_by_stage: Some({
                    let mut m = BTreeMap::new();
                    m.insert("extraction".into(), 1);
                    m.insert("resolve".into(), 1);
                    m
                }),
                total_calls: Some(2),
                proxy_entities_extracted: 0,
                proxy_edges_extracted: 0,
                proxy_stage_failures: 0,
                ingest_wall_ms: 0.0,
            })
            .collect();
        let sum = aggregate_summary(&recs);
        assert!(!sum.placeholder_until_goal_2_11_lands);
        assert_eq!(sum.total_calls, Some(1000));
        assert!((sum.average.unwrap() - 2.0).abs() < 1e-12);
        // `by_stage` is intentionally still empty in this build —
        // the future hook is documented in `aggregate_summary` as a
        // TODO so this test pins the *current* behaviour. When
        // GOAL-2.11 lands and the per-stage average is wired, this
        // assertion should be updated to assert `extraction = 1.0,
        // resolve = 1.0`.
        assert!(sum.by_stage.is_empty());
    }

    /// `build_snapshot` emits `Missing` for placeholder summaries,
    /// `Number` for real ones. This is the GOAL-5.4 wiring contract.
    #[test]
    fn build_snapshot_missing_on_placeholder_number_on_real() {
        let placeholder = CostSummary {
            n_episodes: 500,
            total_calls: None,
            average: None,
            by_stage: BTreeMap::new(),
            proxy_total_entities_extracted: 0,
            proxy_total_edges_extracted: 0,
            proxy_total_stage_failures: 0,
            aggregate_ingest_wall_ms: 0.0,
            placeholder_until_goal_2_11_lands: true,
        };
        let snap = build_snapshot(&placeholder);
        match snap.get("cost.average_llm_calls") {
            Some(MetricValue::Missing(msg)) => {
                assert!(
                    msg.contains("GOAL-2.11"),
                    "missing-metric explanation must reference GOAL-2.11; got: {msg}"
                );
            }
            other => panic!("expected MetricValue::Missing, got {other:?}"),
        }

        let real = CostSummary {
            average: Some(2.5),
            placeholder_until_goal_2_11_lands: false,
            ..placeholder
        };
        let snap = build_snapshot(&real);
        match snap.get("cost.average_llm_calls") {
            Some(MetricValue::Number(v)) => assert!((v - 2.5).abs() < 1e-12),
            other => panic!("expected MetricValue::Number(2.5), got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Artifact emission
    // -----------------------------------------------------------------

    /// `cost_summary.json` round-trip preserves the placeholder flag
    /// and proxy fields. This is what CI tooling reads to decide
    /// "GOAL-5.4 = ERROR (placeholder)" vs "GOAL-5.4 = PASS/FAIL".
    #[test]
    fn cost_summary_json_round_trip_preserves_placeholder_flag() {
        let s = CostSummary {
            n_episodes: 500,
            total_calls: None,
            average: None,
            by_stage: BTreeMap::new(),
            proxy_total_entities_extracted: 1234,
            proxy_total_edges_extracted: 5678,
            proxy_total_stage_failures: 2,
            aggregate_ingest_wall_ms: 12345.6,
            placeholder_until_goal_2_11_lands: true,
        };
        let json = serde_json::to_string(&s).unwrap();
        // `total_calls`, `average`, `by_stage` must be ABSENT (we
        // skip serializing them when None/empty) to avoid CI tooling
        // reading `null` and falling through to a default.
        assert!(!json.contains("total_calls"));
        assert!(!json.contains("\"average\""));
        assert!(!json.contains("by_stage"));
        // The placeholder flag MUST be visibly true.
        assert!(json.contains("\"placeholder_until_goal_2_11_lands\":true"));
        // Round-trip equality.
        let back: CostSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    /// On disk: `cost_summary.json` is pretty-printed JSON +
    /// trailing newline; `cost_per_episode.jsonl` is line-delimited
    /// JSON, one record per line. Match design §3.3 exactly.
    #[test]
    fn write_artifacts_to_disk_have_correct_layout() {
        let tmp = tempdir_for_test("cost-driver-artifacts");
        let recs = vec![
            CostEpisodeRecord {
                episode_id: "locomo:0".into(),
                source: CostEpisodeSource::Locomo,
                calls_by_stage: None,
                total_calls: None,
                proxy_entities_extracted: 1,
                proxy_edges_extracted: 0,
                proxy_stage_failures: 0,
                ingest_wall_ms: 1.0,
            },
            CostEpisodeRecord {
                episode_id: "rcw:42".into(),
                source: CostEpisodeSource::Rustclaw,
                calls_by_stage: None,
                total_calls: None,
                proxy_entities_extracted: 2,
                proxy_edges_extracted: 1,
                proxy_stage_failures: 0,
                ingest_wall_ms: 2.0,
            },
        ];
        let summary = aggregate_summary(&recs);
        let sp = write_summary_json(&tmp, &summary).unwrap();
        let jp = write_per_episode_jsonl(&tmp, &recs).unwrap();
        assert!(sp.exists() && jp.exists());

        // summary.json is pretty + ends with newline.
        let body = fs::read_to_string(&sp).unwrap();
        assert!(body.ends_with('\n'));
        assert!(body.contains("\"n_episodes\": 2"));

        // per_episode.jsonl: 2 lines, each parses as a record, in order.
        let lines: Vec<_> = fs::read_to_string(&jp)
            .unwrap()
            .lines()
            .map(|s| s.to_owned())
            .collect();
        assert_eq!(lines.len(), 2);
        let r0: CostEpisodeRecord = serde_json::from_str(&lines[0]).unwrap();
        let r1: CostEpisodeRecord = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(r0.episode_id, "locomo:0");
        assert_eq!(r1.episode_id, "rcw:42");
        assert_eq!(r1.source, CostEpisodeSource::Rustclaw);
    }

    // -----------------------------------------------------------------
    // Source-pin loader
    // -----------------------------------------------------------------

    /// `load_source_pin` rejects files missing required `[fixtures.*]`
    /// entries — guards against silent zero-fill of corpus on a
    /// half-written source.toml.
    #[test]
    fn load_source_pin_rejects_missing_required_fixtures() {
        let tmp = tempdir_for_test("cost-source-pin-missing");
        let dir = tmp.join("cost");
        fs::create_dir_all(&dir).unwrap();
        // source.toml that has only one of the three required entries.
        let body = r#"
            locomo_version_pin = "abc"
            rustclaw_version_pin = "xyz"
            corpus_seed = 42
            [fixtures.locomo_conversations]
            path = "conversations.jsonl"
            sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
        "#;
        fs::write(dir.join("source.toml"), body).unwrap();
        let err = load_source_pin(&tmp).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rustclaw_episodes") || msg.contains("selection_indices"),
            "error must name a missing fixture; got: {msg}"
        );
    }

    /// Helper: per-test scratch dir under `cargo`'s `OUT_DIR` analogue
    /// (we use `std::env::temp_dir`). The directory is created fresh
    /// per-test; we don't bother cleaning up — `cargo` users running
    /// the tests in a sandbox have it nuked between runs anyway, and
    /// the harness driver tests in this crate use the same idiom.
    fn tempdir_for_test(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("engram-bench-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -----------------------------------------------------------------
    // Driver wiring (BenchDriver impl identity)
    // -----------------------------------------------------------------

    #[test]
    fn driver_metadata_matches_design() {
        let d = CostDriver;
        assert_eq!(d.name(), Driver::Cost);
        assert_eq!(d.stage(), Stage::Stage1);
        assert_eq!(d.cost_tier(), CostTier::Medium);
    }

    // -----------------------------------------------------------------
    // Gate evaluation contract
    // -----------------------------------------------------------------

    /// Smoke: `evaluate_gates` on a placeholder summary never returns
    /// `Pass` — the cost driver registered `cost.average_llm_calls` as
    /// `Missing(…)` so GOAL-5.4 must evaluate to `Error`, never silent
    /// pass. Post-reconcile (2026-04-27) the registry has exactly one
    /// `cost.*` gate (GOAL-5.4); this test asserts the
    /// driver-prefix-scoped evaluation upholds GUARD-2 ("missing
    /// metric → ERROR, never silent pass").
    #[test]
    fn evaluate_gates_never_returns_silent_pass_on_placeholder() {
        let placeholder_summary = CostSummary {
            n_episodes: 500,
            total_calls: None,
            average: None,
            by_stage: BTreeMap::new(),
            proxy_total_entities_extracted: 0,
            proxy_total_edges_extracted: 0,
            proxy_total_stage_failures: 0,
            aggregate_ingest_wall_ms: 0.0,
            placeholder_until_goal_2_11_lands: true,
        };
        let gates = evaluate_gates(&placeholder_summary);
        for g in &gates {
            // No gate may report Pass on a placeholder run. Allowed:
            // Error (preferred) or Fail. Forbidden: Pass.
            assert_ne!(
                g.status, GateStatus::Pass,
                "gate {} reported PASS on placeholder summary — GUARD-2 violation",
                g.goal
            );
        }
    }

    /// Forward-compat: when `cost.average_llm_calls` IS populated (≤
    /// 3.0), no gate reports Error for that key. Mirrors the
    /// "happy path after GOAL-2.11" contract.
    #[test]
    fn evaluate_gates_emits_number_metric_when_average_present() {
        let real = CostSummary {
            n_episodes: 500,
            total_calls: Some(1000),
            average: Some(2.0),
            by_stage: BTreeMap::new(),
            proxy_total_entities_extracted: 0,
            proxy_total_edges_extracted: 0,
            proxy_total_stage_failures: 0,
            aggregate_ingest_wall_ms: 0.0,
            placeholder_until_goal_2_11_lands: false,
        };
        let snap = build_snapshot(&real);
        match snap.get("cost.average_llm_calls") {
            Some(MetricValue::Number(v)) => assert!((v - 2.0).abs() < 1e-12),
            other => panic!(
                "expected Number(2.0) for cost.average_llm_calls; got {other:?}"
            ),
        }
    }
}
