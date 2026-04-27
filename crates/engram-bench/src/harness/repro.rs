//! Reproducibility record (design §6).
//!
//! Schema and writer for `reproducibility.toml` — the committed artifact
//! that lets a release-qualification run be re-executed bit-identically
//! months later (design §6.3 replay workflow).
//!
//! ## Status
//!
//! **Sub-task 1 of `task:bench-impl-repro` (schema only).** Defines the
//! type hierarchy mirroring the on-disk TOML structure from design §6.1.
//! Writer/reader round-trip helpers and the meta-gate validator land in
//! sub-tasks 2 and 3 respectively.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::BenchError;
use super::gates::{GateStatus, Priority};

/// Top-level reproducibility-record schema (design §6.1).
///
/// Mirrors the on-disk TOML structure:
/// `[run]`, `[build]`, `[dataset]`, `[fusion]`, `[models]`, `[result]`,
/// `[gates]`, optional `[override]`.
///
/// Per design §6.1 invariant: every section except `[override]` is always
/// present. Missing required values are represented as the type's default
/// (e.g. empty string, `0`) and cause `[run].status = "error"` if the
/// missing field is gate-relevant — that semantic check lives in
/// sub-task 3 (`validate_record`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReproRecord {
    /// `[run]` — driver identity + execution timestamps + final status.
    pub run: RunSection,
    /// `[build]` — engramai commit + toolchain provenance.
    pub build: BuildSection,
    /// `[dataset]` — fixture SHAs + selection seeds (per-driver fields).
    pub dataset: DatasetSection,
    /// `[fusion]` — frozen fusion weights captured from `FusionConfig::locked()`.
    pub fusion: FusionSection,
    /// `[models]` — embedding / rerank / LLM model identifiers.
    pub models: ModelsSection,
    /// `[result]` — driver-specific summary block. Free-form per driver
    /// (e.g. `locomo_overall`, `cost_per_qualified_token`) — typed at the
    /// driver layer; stored here as a TOML table to preserve forward
    /// compatibility across drivers.
    pub result: ResultSection,
    /// `[gates]` — evaluated gate outcomes for this run, keyed by GOAL id
    /// (e.g. `"GOAL-5.1"`). Per §6.1: every gate evaluated for the run's
    /// driver appears here exactly once.
    pub gates: BTreeMap<String, GateRow>,
    /// `[override]` — present only when `--override-gate` was used.
    /// Absent for clean runs (skipped via `skip_serializing_if`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_section: Option<OverrideSection>,
}

/// `[run]` — driver identity, lifecycle timestamps, and the run's final
/// `pass | fail | error` verdict (design §6.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunSection {
    /// Driver name: `locomo` | `longmemeval` | `cost` | `test-preservation`
    /// | `cognitive-regression` | `migration-integrity`.
    pub driver: String,
    /// RFC 3339 timestamp captured at driver invocation start.
    pub started_at: String,
    /// RFC 3339 timestamp captured at driver invocation end.
    pub finished_at: String,
    /// Final status — see design §6.1 invariant: `error` is mandatory when
    /// any gate-relevant field is missing/null.
    pub status: RunStatus,
}

/// Final run verdict per design §6.1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// All gates evaluated and passed.
    Pass,
    /// At least one gate evaluated and failed (no override).
    Fail,
    /// At least one gate-relevant field is missing or invalid (per §6.1
    /// invariant: missing → error, never silent pass).
    #[default]
    Error,
}

/// `[build]` — toolchain + engramai version provenance (design §6.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BuildSection {
    /// Git commit SHA of engramai built into the run binary.
    pub engram_commit_sha: String,
    /// engramai semver string (e.g. `0.3.0-rc.1`).
    pub engram_version: String,
    /// Cargo build profile (`release` for ship-gate runs).
    pub cargo_profile: String,
    /// `rustc --version` output.
    pub rustc_version: String,
    /// Host target triple (e.g. `aarch64-apple-darwin`).
    pub host_triple: String,
}

/// `[dataset]` — fixture SHAs and seeded-selection commitments (design §6.1).
///
/// All four fields are populated per-driver. Drivers that don't use a
/// particular fixture (e.g. `cost` driver doesn't load LOCOMO) leave that
/// field empty — `validate_record` (sub-task 3) enforces driver-appropriate
/// presence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatasetSection {
    /// SHA-256 of the LOCOMO fixture file, hex-encoded.
    #[serde(default)]
    pub locomo_sha: String,
    /// SHA-256 of the LongMemEval fixture file, hex-encoded.
    #[serde(default)]
    pub longmemeval_sha: String,
    /// Seed for the cost-corpus sampler (design §3.3 — deterministic).
    #[serde(default)]
    pub cost_corpus_seed: u64,
    /// SHA-256 over the committed selection-indices file emitted by the
    /// cost-corpus sampler.
    #[serde(default)]
    pub cost_corpus_selection_sha: String,
}

/// `[fusion]` — frozen fusion weights captured from `FusionConfig::locked()`
/// (design §6.1). The `frozen` flag is the cross-check against the locked
/// config: any run with `frozen = false` MUST be rejected at gate time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FusionSection {
    /// Per-channel fusion weights (factual, episodic, …) keyed by channel
    /// name. `BTreeMap` for stable serialization order.
    pub weights: BTreeMap<String, f64>,
    /// `true` ⇔ weights were captured from `FusionConfig::locked()`.
    pub frozen: bool,
}

/// `[models]` — embedding / rerank / LLM model identifiers + LLM
/// determinism setting (design §6.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelsSection {
    /// Embedding model identifier (e.g. `bge-small-en`).
    pub embedding_model: String,
    /// Reranker model identifier (e.g. `ms-marco-MiniLM`).
    pub rerank_model: String,
    /// LLM identifier used by drivers that invoke generation.
    pub llm_model: String,
    /// LLM sampling temperature. Convention per design §6.1: `0.0` for
    /// deterministic release-gate runs.
    pub llm_temperature: f64,
}

/// `[result]` — driver-specific summary block (design §6.1).
///
/// Free-form: each driver writes its own summary fields (e.g. LOCOMO
/// writes `locomo_overall` + `locomo_by_category`; cost driver writes
/// `cost_per_qualified_token`). Stored as a TOML table to preserve forward
/// compatibility — drivers may add new summary fields without bumping the
/// reproducibility-record schema.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResultSection(pub toml::Table);

/// One row in the `[gates]` table — recorded outcome of a single gate
/// (design §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateRow {
    /// Observed metric value at evaluation time.
    pub metric: f64,
    /// Threshold the gate enforced.
    pub threshold: f64,
    /// Pass / Fail / Error per design §4.4 Level 1.
    pub status: GateStatus,
    /// Priority class — recorded so replay can re-aggregate
    /// `ReleaseDecision` without rebuilding the gate table from scratch.
    pub priority: Priority,
}

/// `[override]` — present only when an operator used `--override-gate` to
/// permit a `Fail` to ship under explicit accountability (design §4.4
/// manual override, §6.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OverrideSection {
    /// GOAL id of the gate being overridden (e.g. `"GOAL-5.2"`).
    pub gate: String,
    /// Path to the operator-authored rationale Markdown file.
    pub rationale_file: String,
    /// SHA-256 of the rationale file content, hex-encoded — locks the
    /// rationale text to this run.
    pub rationale_sha: String,
    /// Operator identity (matches `.gid/releases/overrides.log` row).
    pub operator: String,
}

// ---------------------------------------------------------------------------
// Sub-task 2: writer / reader + on-disk layout helpers (design §6.2).
// ---------------------------------------------------------------------------

/// File name of the reproducibility record inside a run directory
/// (design §6.2 layout: `benchmarks/runs/.../reproducibility.toml`).
pub const REPRO_FILE_NAME: &str = "reproducibility.toml";

impl ReproRecord {
    /// Serialize this record as TOML and write it atomically (parent dir
    /// created on demand) to `path`. Per design §6.2: the file is named
    /// `reproducibility.toml` and lives inside a per-run directory.
    ///
    /// Failure modes:
    /// - `BenchError::Other` if serialization fails (should not happen for
    ///   well-formed records — every field type implements `Serialize`).
    /// - `BenchError::IoError` if directory creation or file write fails.
    pub fn write_toml(&self, path: &Path) -> Result<(), BenchError> {
        let body = toml::to_string_pretty(self)
            .map_err(|e| BenchError::Other(format!("toml serialize: {e}")))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        fs::write(path, body)?;
        Ok(())
    }

    /// Read and deserialize a reproducibility record from a TOML file on
    /// disk. The dual of [`Self::write_toml`].
    ///
    /// Failure modes:
    /// - `BenchError::IoError` if the file is missing or unreadable.
    /// - `BenchError::Other` wrapping the TOML parse diagnostic if the
    ///   file exists but cannot be deserialized into [`ReproRecord`]
    ///   (e.g. missing required field, type mismatch).
    pub fn read_toml(path: &Path) -> Result<Self, BenchError> {
        let body = fs::read_to_string(path)?;
        let record: ReproRecord = toml::from_str(&body)
            .map_err(|e| BenchError::Other(format!("toml parse {}: {e}", path.display())))?;
        Ok(record)
    }
}

/// Build the canonical on-disk run-directory path for a record per design
/// §6.2: `<runs_root>/<timestamp>_<driver>_<short-sha>/`.
///
/// `started_at` is taken verbatim from `RunSection::started_at` and
/// path-sanitised: `:` becomes `-` so the timestamp is a legal directory
/// name on every supported host (the design example shows
/// `2026-MM-DDTHH-MM-SSZ`, hyphenated).
///
/// `engram_commit_sha` is truncated to its first 12 hex characters as the
/// `<short-sha>` component (the example writes `<short-sha>`; 12 is the
/// project-wide convention used in `git log --abbrev=12`).
pub fn run_dir_path(runs_root: &Path, record: &ReproRecord) -> PathBuf {
    let ts = record.run.started_at.replace(':', "-");
    let driver = if record.run.driver.is_empty() {
        "unknown"
    } else {
        record.run.driver.as_str()
    };
    let sha = &record.build.engram_commit_sha;
    let short_sha = if sha.len() >= 12 { &sha[..12] } else { sha.as_str() };
    runs_root.join(format!("{ts}_{driver}_{short_sha}"))
}

/// Convenience: full path to the `reproducibility.toml` inside the run
/// directory built by [`run_dir_path`].
pub fn repro_file_path(runs_root: &Path, record: &ReproRecord) -> PathBuf {
    run_dir_path(runs_root, record).join(REPRO_FILE_NAME)
}

// ---------------------------------------------------------------------------
// Sub-task 3: meta-gate validator + replay-precondition extractor
// (design §4.2a, §6.3).
// ---------------------------------------------------------------------------

/// Sentinel strings that are NEVER acceptable in a release-qualification
/// record. Per design §4.2a check 4: "no field contains a sentinel value
/// (e.g., `\"TODO\"`, empty string where a value was required)."
const SENTINEL_STRINGS: &[&str] = &["TODO", "FIXME", "XXX", "PLACEHOLDER"];

/// Validate a [`ReproRecord`] against the §4.2a meta-gate checks.
///
/// Per design §4.2a, this is the single function that turns the
/// "reproducibility-by-contract" promise into something enforceable. It
/// runs four explicit checks:
///
/// 1. **Schema conformance.** All required scalars are non-empty
///    (`run.driver`, `run.started_at`, `run.finished_at`,
///    `build.engram_commit_sha`, `build.engram_version`).
/// 2. **Gate coverage.** Every id in `expected_gate_ids` is present in
///    `record.gates`. Missing ⇒ violation.
/// 3. **Override iff `--override-gate` used.** `record.override_section`
///    is `Some` ⇔ at least one `GateRow` has `status = Fail` (the only
///    case where overriding makes sense). All-pass-with-override and
///    fail-without-override both fail this check.
/// 4. **No sentinel values.** No string field equals one of
///    [`SENTINEL_STRINGS`]; no required string field is empty. Per
///    §4.2a check 4 — silent placeholders are forbidden.
///
/// On the first failure encountered, returns `BenchError::Other` with a
/// human-readable diagnostic naming the offending field. The §10.1
/// summary renderer surfaces this as `[FAIL-P1] GOAL-5.8 reproducibility
/// record: <message>` per design §4.2a "Surface" paragraph.
///
/// `expected_gate_ids` is supplied by the caller (typically via
/// `harness::gates::standard_gates()` once sub-task `bench-impl-gates-1`
/// lands) to keep this validator from depending on the still-evolving
/// gate inventory.
pub fn validate_record(
    record: &ReproRecord,
    expected_gate_ids: &[&str],
) -> Result<(), BenchError> {
    // Check 1: schema conformance — required scalars non-empty.
    let required_scalars: &[(&str, &str)] = &[
        ("run.driver", record.run.driver.as_str()),
        ("run.started_at", record.run.started_at.as_str()),
        ("run.finished_at", record.run.finished_at.as_str()),
        ("build.engram_commit_sha", record.build.engram_commit_sha.as_str()),
        ("build.engram_version", record.build.engram_version.as_str()),
    ];
    for (field, value) in required_scalars {
        if value.is_empty() {
            return Err(BenchError::Other(format!(
                "reproducibility record incomplete: required field `{field}` is empty"
            )));
        }
        for sentinel in SENTINEL_STRINGS {
            if value.eq_ignore_ascii_case(sentinel) {
                return Err(BenchError::Other(format!(
                    "reproducibility record contains sentinel value `{sentinel}` in `{field}`"
                )));
            }
        }
    }

    // Check 2: gate coverage — every expected gate id is present.
    for gate_id in expected_gate_ids {
        if !record.gates.contains_key(*gate_id) {
            return Err(BenchError::Other(format!(
                "reproducibility record missing gate `{gate_id}` in [gates] table"
            )));
        }
    }

    // Check 3: override iff at least one gate failed.
    let any_failed = record
        .gates
        .values()
        .any(|row| matches!(row.status, GateStatus::Fail));
    match (any_failed, record.override_section.as_ref()) {
        (true, None) => {
            return Err(BenchError::Other(
                "reproducibility record has Fail gate without [override] section".into(),
            ));
        }
        (false, Some(_)) => {
            return Err(BenchError::Other(
                "reproducibility record has [override] section but no Fail gate".into(),
            ));
        }
        _ => {}
    }

    // Check 4 (additional sentinel scan + override field coverage).
    if let Some(ov) = &record.override_section {
        let override_fields: &[(&str, &str)] = &[
            ("override.gate", ov.gate.as_str()),
            ("override.rationale_file", ov.rationale_file.as_str()),
            ("override.rationale_sha", ov.rationale_sha.as_str()),
            ("override.operator", ov.operator.as_str()),
        ];
        for (field, value) in override_fields {
            if value.is_empty() {
                return Err(BenchError::Other(format!(
                    "reproducibility record incomplete: required field `{field}` is empty"
                )));
            }
            for sentinel in SENTINEL_STRINGS {
                if value.eq_ignore_ascii_case(sentinel) {
                    return Err(BenchError::Other(format!(
                        "reproducibility record contains sentinel value `{sentinel}` in `{field}`"
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Distilled "everything needed to re-run this benchmark" set, extracted
/// from a [`ReproRecord`] per design §6.3 step 1.
///
/// The replay workflow consumes this struct as the contract: every field
/// here must be reproduced in the replay environment, otherwise the
/// replay fails its preconditions and never proceeds to scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayPlan {
    /// `build.engram_commit_sha` — must be checked out in the replay
    /// workspace before running (§6.3 step 2).
    pub engram_commit_sha: String,
    /// `build.rustc_version` — must match (or accept documented drift)
    /// in the replay toolchain (§6.3 step 3).
    pub rustc_version: String,
    /// Per-fixture SHA-256s extracted from `[dataset]`. Keys are the
    /// `DatasetSection` field names (`locomo_sha`, `longmemeval_sha`,
    /// `cost_corpus_selection_sha`); empty values are dropped so the map
    /// only contains fixtures the recorded run actually exercised.
    pub dataset_shas: BTreeMap<String, String>,
    /// `fusion.weights` — copied verbatim. Replay must materialize the
    /// same `FusionConfig::locked()` snapshot.
    pub fusion_weights: BTreeMap<String, f64>,
    /// `models.embedding_model` / `rerank_model` / `llm_model`
    /// triplet — same identifiers must be served at replay time.
    pub embedding_model: String,
    /// See `embedding_model`.
    pub rerank_model: String,
    /// See `embedding_model`.
    pub llm_model: String,
}

/// Extract the replay contract from a record per design §6.3 step 1.
///
/// Returns `BenchError::Other` if any required field is missing — the
/// replay workflow refuses to proceed under partial information rather
/// than silently filling defaults (per GUARD-2 + §4.4 Level 1 semantics).
pub fn replay_preconditions(record: &ReproRecord) -> Result<ReplayPlan, BenchError> {
    if record.build.engram_commit_sha.is_empty() {
        return Err(BenchError::Other(
            "replay precondition: build.engram_commit_sha is empty".into(),
        ));
    }
    if record.build.rustc_version.is_empty() {
        return Err(BenchError::Other(
            "replay precondition: build.rustc_version is empty".into(),
        ));
    }
    if record.models.embedding_model.is_empty()
        || record.models.rerank_model.is_empty()
        || record.models.llm_model.is_empty()
    {
        return Err(BenchError::Other(
            "replay precondition: models.{embedding,rerank,llm}_model must all be set".into(),
        ));
    }

    let mut dataset_shas = BTreeMap::new();
    let candidates: &[(&str, &str)] = &[
        ("locomo_sha", record.dataset.locomo_sha.as_str()),
        ("longmemeval_sha", record.dataset.longmemeval_sha.as_str()),
        (
            "cost_corpus_selection_sha",
            record.dataset.cost_corpus_selection_sha.as_str(),
        ),
    ];
    for (field, value) in candidates {
        if !value.is_empty() {
            dataset_shas.insert((*field).to_string(), (*value).to_string());
        }
    }

    Ok(ReplayPlan {
        engram_commit_sha: record.build.engram_commit_sha.clone(),
        rustc_version: record.build.rustc_version.clone(),
        dataset_shas,
        fusion_weights: record.fusion.weights.clone(),
        embedding_model: record.models.embedding_model.clone(),
        rerank_model: record.models.rerank_model.clone(),
        llm_model: record.models.llm_model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a fully populated `ReproRecord` (all 8 sections including
    /// `[override]`) through TOML and assert the deserialized value equals
    /// the original. Anchors design §6.1's "every field always present"
    /// invariant against accidental field-rename or default-skip changes.
    #[test]
    fn full_record_roundtrip() {
        let mut weights = BTreeMap::new();
        weights.insert("factual".into(), 0.4);
        weights.insert("episodic".into(), 0.6);

        let mut gates = BTreeMap::new();
        gates.insert(
            "GOAL-5.1".into(),
            GateRow {
                metric: 0.712,
                threshold: 0.685,
                status: GateStatus::Pass,
                priority: Priority::P0,
            },
        );

        let mut result_table = toml::Table::new();
        result_table.insert("locomo_overall".into(), toml::Value::Float(0.712));

        let original = ReproRecord {
            run: RunSection {
                driver: "locomo".into(),
                started_at: "2026-04-27T10:00:00Z".into(),
                finished_at: "2026-04-27T10:42:00Z".into(),
                status: RunStatus::Pass,
            },
            build: BuildSection {
                engram_commit_sha: "abcdef0123456789".into(),
                engram_version: "0.3.0-rc.1".into(),
                cargo_profile: "release".into(),
                rustc_version: "1.83.0".into(),
                host_triple: "aarch64-apple-darwin".into(),
            },
            dataset: DatasetSection {
                locomo_sha: "deadbeef".into(),
                longmemeval_sha: String::new(),
                cost_corpus_seed: 0,
                cost_corpus_selection_sha: String::new(),
            },
            fusion: FusionSection {
                weights,
                frozen: true,
            },
            models: ModelsSection {
                embedding_model: "bge-small-en".into(),
                rerank_model: "ms-marco-MiniLM".into(),
                llm_model: "claude-haiku-3.5".into(),
                llm_temperature: 0.0,
            },
            result: ResultSection(result_table),
            gates,
            override_section: Some(OverrideSection {
                gate: "GOAL-5.2".into(),
                rationale_file: "overrides/2026-04-27-locomo-temporal.md".into(),
                rationale_sha: "cafebabe".into(),
                operator: "potato".into(),
            }),
        };

        let serialized = toml::to_string(&original).expect("serialize");
        let restored: ReproRecord = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(restored, original);
    }

    /// Round-trip a clean (no-override) record: `[override]` must be absent
    /// from the serialized TOML so committed records stay minimal, and
    /// must round-trip back to `None`.
    #[test]
    fn clean_record_omits_override() {
        let original = ReproRecord {
            run: RunSection {
                driver: "cost".into(),
                started_at: "2026-04-27T11:00:00Z".into(),
                finished_at: "2026-04-27T11:05:00Z".into(),
                status: RunStatus::Pass,
            },
            ..Default::default()
        };

        let serialized = toml::to_string(&original).expect("serialize");
        assert!(
            !serialized.contains("[override]"),
            "clean record must not emit [override] section, got:\n{serialized}"
        );

        let restored: ReproRecord = toml::from_str(&serialized).expect("deserialize");
        assert!(restored.override_section.is_none());
        assert_eq!(restored.run.driver, "cost");
    }

    /// Sub-task 2: write a record to disk and read it back. Path layout
    /// (`<runs_root>/<timestamp>_<driver>_<short-sha>/reproducibility.toml`)
    /// is built by `repro_file_path`; the writer creates parent dirs.
    #[test]
    fn write_then_read_roundtrip_on_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runs_root = tmp.path().join("benchmarks").join("runs");

        let record = ReproRecord {
            run: RunSection {
                driver: "longmemeval".into(),
                started_at: "2026-04-27T11:30:00Z".into(),
                finished_at: "2026-04-27T11:55:00Z".into(),
                status: RunStatus::Pass,
            },
            build: BuildSection {
                engram_commit_sha: "0123456789abcdef0123".into(),
                engram_version: "0.3.0-rc.1".into(),
                cargo_profile: "release".into(),
                rustc_version: "1.83.0".into(),
                host_triple: "aarch64-apple-darwin".into(),
            },
            ..Default::default()
        };

        let target = repro_file_path(&runs_root, &record);
        // Layout assertions: per-run directory is unique and embeds
        // (sanitised timestamp, driver, 12-char short sha).
        let dir_name = target
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .expect("run dir name")
            .to_string();
        assert_eq!(
            dir_name,
            "2026-04-27T11-30-00Z_longmemeval_0123456789ab"
        );
        assert_eq!(target.file_name().and_then(|s| s.to_str()), Some(REPRO_FILE_NAME));

        record.write_toml(&target).expect("write");
        assert!(target.exists(), "writer must create the file");

        let restored = ReproRecord::read_toml(&target).expect("read");
        assert_eq!(restored, record);
    }

    /// Sub-task 2: the reader must reject a TOML file that omits a
    /// required field (e.g. `[run].driver`). Returning a typed
    /// `BenchError` is the contract that lets the meta-gate (sub-task 3)
    /// surface schema violations as `RunStatus::Error` rather than
    /// silently filling defaults.
    #[test]
    fn read_rejects_missing_required_field() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("reproducibility.toml");
        // `[run]` table is present but lacks the mandatory `driver` field.
        // `started_at` / `finished_at` / `status` are also absent — any
        // single missing required field is enough to trip the parser.
        fs::write(&path, "[build]\nengram_version = \"0.3.0\"\n").expect("seed");

        let err = ReproRecord::read_toml(&path).expect_err("must fail");
        match err {
            BenchError::Other(msg) => {
                assert!(
                    msg.contains("toml parse"),
                    "error must identify the parse stage, got: {msg}"
                );
            }
            other => panic!("expected BenchError::Other, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Sub-task 3 tests: validate_record (§4.2a) + replay_preconditions (§6.3).
    // -----------------------------------------------------------------------

    /// Build a "happy-path" record with one passing P0 gate. Used as the
    /// shared fixture for the validator tests below.
    fn happy_record() -> ReproRecord {
        let mut gates = BTreeMap::new();
        gates.insert(
            "GOAL-5.1".into(),
            GateRow {
                metric: 0.712,
                threshold: 0.685,
                status: GateStatus::Pass,
                priority: Priority::P0,
            },
        );
        ReproRecord {
            run: RunSection {
                driver: "locomo".into(),
                started_at: "2026-04-27T12:00:00Z".into(),
                finished_at: "2026-04-27T12:30:00Z".into(),
                status: RunStatus::Pass,
            },
            build: BuildSection {
                engram_commit_sha: "deadbeef0123".into(),
                engram_version: "0.3.0-rc.1".into(),
                cargo_profile: "release".into(),
                rustc_version: "1.83.0".into(),
                host_triple: "aarch64-apple-darwin".into(),
            },
            models: ModelsSection {
                embedding_model: "bge-small-en".into(),
                rerank_model: "ms-marco-MiniLM".into(),
                llm_model: "claude-haiku-3.5".into(),
                llm_temperature: 0.0,
            },
            gates,
            ..Default::default()
        }
    }

    /// Test 1: a clean, complete record with all expected gates passes.
    #[test]
    fn validate_passes_on_complete_record() {
        let record = happy_record();
        validate_record(&record, &["GOAL-5.1"]).expect("complete record must validate");
    }

    /// Test 2: a missing required scalar (`build.engram_commit_sha`) fails
    /// schema conformance. The error must name the specific field so the
    /// §10.1 summary can surface the first malformed field per §4.2a
    /// "Surface" requirement.
    #[test]
    fn validate_rejects_missing_required_field() {
        let mut record = happy_record();
        record.build.engram_commit_sha.clear();
        let err = validate_record(&record, &["GOAL-5.1"]).expect_err("must fail");
        let BenchError::Other(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("build.engram_commit_sha"), "got: {msg}");
    }

    /// Test 3: override section is present without any `Fail` gate — the
    /// "iff" check must reject this (override is meaningless without a
    /// gate to override).
    #[test]
    fn validate_rejects_override_without_fail_gate() {
        let mut record = happy_record();
        record.override_section = Some(OverrideSection {
            gate: "GOAL-5.2".into(),
            rationale_file: "rationale.md".into(),
            rationale_sha: "abc123".into(),
            operator: "potato".into(),
        });
        // All gates pass → override is contradictory.
        let err = validate_record(&record, &["GOAL-5.1"]).expect_err("must fail");
        let BenchError::Other(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("[override] section but no Fail gate"), "got: {msg}");
    }

    /// Test 4: a sentinel string ("TODO") leaked into a required field.
    /// Per §4.2a check 4 this is a hard failure regardless of any other
    /// validity.
    #[test]
    fn validate_rejects_sentinel_value() {
        let mut record = happy_record();
        record.build.engram_version = "TODO".into();
        let err = validate_record(&record, &["GOAL-5.1"]).expect_err("must fail");
        let BenchError::Other(msg) = err else { panic!("wrong variant") };
        assert!(msg.contains("sentinel value `TODO`"), "got: {msg}");
    }

    /// Test 5: `replay_preconditions` extracts every required field from a
    /// happy-path record into a `ReplayPlan` and drops empty fixture-SHA
    /// fields (the cost driver doesn't load LOCOMO, so its record has an
    /// empty `locomo_sha` that must NOT enter the plan).
    #[test]
    fn replay_preconditions_extracts_plan() {
        let mut record = happy_record();
        record.dataset.locomo_sha = "loco-sha-here".into();
        record.dataset.longmemeval_sha.clear(); // intentionally empty
        record.fusion.weights.insert("factual".into(), 0.4);
        record.fusion.weights.insert("episodic".into(), 0.6);

        let plan = replay_preconditions(&record).expect("happy record must extract");
        assert_eq!(plan.engram_commit_sha, "deadbeef0123");
        assert_eq!(plan.rustc_version, "1.83.0");
        assert_eq!(plan.dataset_shas.get("locomo_sha"), Some(&"loco-sha-here".to_string()));
        assert!(
            !plan.dataset_shas.contains_key("longmemeval_sha"),
            "empty fixture SHA must be dropped, plan was: {:?}",
            plan.dataset_shas
        );
        assert_eq!(plan.fusion_weights.get("factual"), Some(&0.4));
        assert_eq!(plan.embedding_model, "bge-small-en");
    }
}
