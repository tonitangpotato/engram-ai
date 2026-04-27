//! Baseline numbers consumed by gates (design §5).
//!
//! Three baseline files live in `benchmarks/baselines/` and **must be
//! committed before v0.3 implementation begins** — otherwise the deltas
//! in GOAL-5.3 / GOAL-5.5 are not measurable and the corresponding gates
//! are not enforceable (design §5 preamble).
//!
//! - `v02.toml` — pre-v0.3 LongMemEval baseline (§5.1, GOAL-5.3).
//! - `v02_test_count.toml` — frozen v0.2 test count (§5.2, GOAL-5.5).
//! - `external.toml` — published mem0 / Graphiti numbers (§5.3,
//!   GOAL-5.1, GOAL-5.2).
//!
//! ## Failure semantics (design §5.3 + §4.4 Level 1)
//!
//! "If any value is `null` or missing at release-qualification time,
//! the corresponding gate reports `ERROR` (not `PASS`), blocking
//! release per §4.4 Level 1." Concretely:
//!
//! - **File missing on disk** → [`BaselineError::FileMissing`].
//! - **TOML parse failure** → [`BaselineError::Parse`].
//! - **Required field absent** (e.g. `external.locomo.graphiti.temporal`
//!   placeholder still null) → [`BaselineError::MissingValue`].
//!
//! Each error carries the offending path / field so the gate result can
//! cite it verbatim. Callers must propagate these as `GateStatus::Error`
//! — never substitute a default and never mark the gate `Pass`.
//!
//! ## Immutability invariant (design §5.1)
//!
//! "Once committed, `baselines/v02.toml` is immutable." This is a
//! repository-level invariant enforced by code review + git history;
//! the loader provides [`V02Baseline::content_sha256`] so callers /
//! reproducibility records (§6.1) can pin the exact bytes that were
//! read at run time, surfacing any in-place edits as a SHA mismatch.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Schema: v0.2 LongMemEval baseline (design §5.1)
// ---------------------------------------------------------------------------

/// Top-level structure of `benchmarks/baselines/v02.toml` (design §5.1).
///
/// Captured one-time on the `v0.2.2` git tag; consumed by the LongMemEval
/// gate (GOAL-5.3 — "v0.3 ≥ v0.2 + 15pp").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V02Baseline {
    /// LongMemEval section.
    pub longmemeval: V02LongmemevalSection,
    /// SHA-256 of the raw TOML file bytes — populated by
    /// [`load_v02_baseline`]; never read from the file itself.
    #[serde(skip)]
    content_sha256: String,
}

/// `[longmemeval]` table inside `v02.toml` (design §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V02LongmemevalSection {
    /// SHA of the LongMemEval dataset used during capture — must match
    /// the SHA that v0.3 will use (design §5.1 step 3).
    pub dataset_sha: String,
    /// Git tag the baseline was captured on (always `"v0.2.2"`).
    pub v02_tag: String,
    /// Overall LongMemEval score (0.0–1.0).
    pub overall: f64,
    /// Per-category sub-scores (factual, episodic, …) — keyed by
    /// category name.
    pub by_category: std::collections::BTreeMap<String, f64>,
    /// Capture date (ISO-8601 `YYYY-MM-DD`).
    pub captured_at: String,
    /// Operator who captured the baseline.
    pub captured_by: String,
}

impl V02Baseline {
    /// SHA-256 of the raw TOML bytes that produced this struct.
    ///
    /// Embedded in the reproducibility record (design §6.1) so that any
    /// in-place edit to the immutable baseline file is detectable as a
    /// hash mismatch on later runs.
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

// ---------------------------------------------------------------------------
// Schema: v0.2 test-count freeze (design §5.2)
// ---------------------------------------------------------------------------

/// Top-level structure of `benchmarks/baselines/v02_test_count.toml`
/// (design §5.2). Consumed by the test-preservation gate (GOAL-5.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V02TestCount {
    /// `[tests]` table.
    pub tests: V02TestCountSection,
    /// SHA-256 of the raw TOML bytes — populated by
    /// [`load_v02_test_count`].
    #[serde(skip)]
    content_sha256: String,
}

/// `[tests]` table inside `v02_test_count.toml` (design §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V02TestCountSection {
    /// Git tag the count was captured on.
    pub v02_tag: String,
    /// Total number of test functions on `v0.2.2` (sorted, unique
    /// `cargo test --no-run` output).
    pub total: u64,
    /// SHA-256 of the sorted test-name list — lets the harness detect
    /// drift without committing the full list (design §5.2 final note).
    pub list_sha: String,
    /// Capture date (ISO-8601).
    pub captured_at: String,
}

impl V02TestCount {
    /// SHA-256 of the raw TOML bytes that produced this struct.
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

// ---------------------------------------------------------------------------
// Schema: external baselines (mem0 + Graphiti) — design §5.3
// ---------------------------------------------------------------------------

/// Top-level structure of `benchmarks/baselines/external.toml`
/// (design §5.3). Consumed by LOCOMO gates (GOAL-5.1, GOAL-5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalBaselines {
    /// `[locomo]` parent — holds nested `mem0` and `graphiti` subtables.
    pub locomo: LocomoExternalSection,
    /// SHA-256 of the raw TOML bytes — populated by
    /// [`load_external_baselines`].
    #[serde(skip)]
    content_sha256: String,
}

/// `[locomo]` table — composite of mem0 + Graphiti subtables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocomoExternalSection {
    /// `[locomo.mem0]` — overall LOCOMO score from the mem0 paper
    /// (GOAL-5.1 baseline).
    pub mem0: Mem0Baseline,
    /// `[locomo.graphiti]` — temporal LOCOMO score from the Graphiti
    /// paper (GOAL-5.2 baseline).
    pub graphiti: GraphitiBaseline,
}

/// `[locomo.mem0]` — published mem0 LOCOMO numbers (design §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mem0Baseline {
    /// Overall LOCOMO score from mem0 (0.0–1.0).
    pub overall: f64,
    /// Provenance string (paper citation / table reference).
    pub source: String,
    /// Stable URL for the source.
    pub url: String,
}

/// `[locomo.graphiti]` — published Graphiti temporal-LOCOMO numbers.
///
/// `temporal` is [`Option<f64>`] because design §5.3 explicitly allows
/// a `null` placeholder ("resolved before v0.3 ships; placeholder
/// flagged by harness if missing"). Unresolved placeholder → loader
/// returns the raw struct unchanged; the gate evaluator translates
/// `None` to `GateStatus::Error` via [`ExternalBaselines::ensure_complete`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphitiBaseline {
    /// Temporal-LOCOMO sub-score from Graphiti (0.0–1.0). May be
    /// `None` until the number is committed pre-release.
    pub temporal: Option<f64>,
    /// Provenance string.
    pub source: String,
    /// Stable URL.
    pub url: String,
}

impl ExternalBaselines {
    /// SHA-256 of the raw TOML bytes that produced this struct.
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    /// Verify all *required* fields are populated (design §5.3 +
    /// §4.4 Level 1).
    ///
    /// Returns [`BaselineError::MissingValue`] if any placeholder
    /// (e.g. `locomo.graphiti.temporal = null`) is still unresolved
    /// at release-qualification time. Callers (gate evaluator) MUST
    /// surface this as `GateStatus::Error`, blocking release.
    pub fn ensure_complete(&self) -> Result<(), BaselineError> {
        if self.locomo.graphiti.temporal.is_none() {
            return Err(BaselineError::MissingValue {
                file: "external.toml".to_string(),
                field: "locomo.graphiti.temporal".to_string(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by the baseline loaders.
///
/// Each variant carries the offending path / field so the gate result
/// can cite it verbatim per design §4.4 Level 2 ("threshold violations
/// are visible — the summary table prints metric, threshold, and
/// status on one line per gate").
#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    /// Baseline file does not exist on disk. Per design §5 preamble
    /// this is a precondition violation — the file MUST be committed
    /// before v0.3 implementation begins.
    #[error("baseline file missing: {0:?} (design §5 — must be committed before v0.3)")]
    FileMissing(PathBuf),

    /// I/O error reading a baseline file (permissions, transient
    /// filesystem issue, …).
    #[error("baseline I/O error reading {path:?}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// TOML parse failure (malformed file, schema mismatch).
    #[error("baseline parse error in {path:?}: {source}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying TOML deserialization error.
        #[source]
        source: toml::de::Error,
    },

    /// Required field missing or `null` (e.g. unresolved placeholder).
    /// Per design §5.3 + §4.4 Level 1: "if any value is `null` or
    /// missing at release-qualification time, the corresponding gate
    /// reports `ERROR` (not `PASS`), blocking release."
    #[error("baseline {file} is missing required field {field}")]
    MissingValue {
        /// Filename (e.g. `"external.toml"`).
        file: String,
        /// Dotted field path (e.g. `"locomo.graphiti.temporal"`).
        field: String,
    },
}

// ---------------------------------------------------------------------------
// Loaders
// ---------------------------------------------------------------------------

/// Read the entire contents of `path` as a UTF-8 string, mapping
/// I/O errors to typed [`BaselineError`] variants.
fn read_baseline_file(path: &Path) -> Result<String, BaselineError> {
    if !path.exists() {
        return Err(BaselineError::FileMissing(path.to_path_buf()));
    }
    fs::read_to_string(path).map_err(|source| BaselineError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Compute the SHA-256 of a string and return it as lower-case hex.
///
/// Used for the `content_sha256` field on every baseline struct so the
/// reproducibility record (design §6.1) can pin the exact bytes read.
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Load `benchmarks/baselines/v02.toml` (design §5.1).
///
/// Returns the parsed [`V02Baseline`] with `content_sha256` populated.
/// File missing → [`BaselineError::FileMissing`]; the gate evaluator
/// must surface this as `GateStatus::Error`.
pub fn load_v02_baseline(path: &Path) -> Result<V02Baseline, BaselineError> {
    let content = read_baseline_file(path)?;
    let mut parsed: V02Baseline =
        toml::from_str(&content).map_err(|source| BaselineError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    parsed.content_sha256 = sha256_hex(&content);
    Ok(parsed)
}

/// Load `benchmarks/baselines/v02_test_count.toml` (design §5.2).
pub fn load_v02_test_count(path: &Path) -> Result<V02TestCount, BaselineError> {
    let content = read_baseline_file(path)?;
    let mut parsed: V02TestCount =
        toml::from_str(&content).map_err(|source| BaselineError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    parsed.content_sha256 = sha256_hex(&content);
    Ok(parsed)
}

/// Load `benchmarks/baselines/external.toml` (design §5.3).
///
/// Note: this only parses the file. Callers that need to enforce
/// "no unresolved placeholders" (i.e., the release-qualification path)
/// must additionally call [`ExternalBaselines::ensure_complete`].
/// This split lets diagnostic / inspection tooling read placeholder-
/// containing files without erroring.
pub fn load_external_baselines(path: &Path) -> Result<ExternalBaselines, BaselineError> {
    let content = read_baseline_file(path)?;
    let mut parsed: ExternalBaselines =
        toml::from_str(&content).map_err(|source| BaselineError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    parsed.content_sha256 = sha256_hex(&content);
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    #[test]
    fn v02_baseline_roundtrip() {
        let toml_src = r#"
[longmemeval]
dataset_sha = "abc123"
v02_tag = "v0.2.2"
overall = 0.732
captured_at = "2026-04-15"
captured_by = "potato"

[longmemeval.by_category]
factual = 0.81
episodic = 0.65
"#;
        let f = write_temp(toml_src);
        let b = load_v02_baseline(f.path()).expect("load");
        assert_eq!(b.longmemeval.dataset_sha, "abc123");
        assert_eq!(b.longmemeval.v02_tag, "v0.2.2");
        assert!((b.longmemeval.overall - 0.732).abs() < 1e-9);
        assert_eq!(b.longmemeval.by_category.get("factual"), Some(&0.81));
        assert_eq!(b.longmemeval.by_category.get("episodic"), Some(&0.65));
        assert_eq!(b.longmemeval.captured_by, "potato");
        // SHA-256 of the file contents should be 64 hex chars.
        assert_eq!(b.content_sha256().len(), 64);
        assert!(b.content_sha256().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn v02_test_count_roundtrip() {
        let toml_src = r#"
[tests]
v02_tag = "v0.2.2"
total = 280
list_sha = "deadbeef"
captured_at = "2026-04-15"
"#;
        let f = write_temp(toml_src);
        let t = load_v02_test_count(f.path()).expect("load");
        assert_eq!(t.tests.total, 280);
        assert_eq!(t.tests.list_sha, "deadbeef");
        assert_eq!(t.content_sha256().len(), 64);
    }

    #[test]
    fn external_baselines_with_temporal_present_passes_complete_check() {
        let toml_src = r#"
[locomo.mem0]
overall = 0.685
source = "mem0 paper"
url = "https://example.com/mem0"

[locomo.graphiti]
temporal = 0.71
source = "Graphiti paper"
url = "https://example.com/graphiti"
"#;
        let f = write_temp(toml_src);
        let e = load_external_baselines(f.path()).expect("load");
        assert!((e.locomo.mem0.overall - 0.685).abs() < 1e-9);
        assert_eq!(e.locomo.graphiti.temporal, Some(0.71));
        e.ensure_complete().expect("complete");
    }

    #[test]
    fn external_baselines_with_temporal_null_loads_but_fails_complete_check() {
        // Per design §5.3: placeholder is allowed in the file. The harness
        // surfaces it as ERROR via `ensure_complete`, NOT at load time,
        // so diagnostic tooling can still read placeholder files.
        let toml_src = r#"
[locomo.mem0]
overall = 0.685
source = "mem0"
url = "https://example.com/mem0"

[locomo.graphiti]
source = "Graphiti paper"
url = "https://example.com/graphiti"
"#;
        let f = write_temp(toml_src);
        let e = load_external_baselines(f.path()).expect("load placeholder file ok");
        assert!(e.locomo.graphiti.temporal.is_none());
        match e.ensure_complete() {
            Err(BaselineError::MissingValue { file, field }) => {
                assert_eq!(file, "external.toml");
                assert_eq!(field, "locomo.graphiti.temporal");
            }
            other => panic!("expected MissingValue error, got {:?}", other),
        }
    }

    #[test]
    fn missing_file_returns_file_missing_error() {
        let bogus = PathBuf::from("/nonexistent/does/not/exist/v02.toml");
        match load_v02_baseline(&bogus) {
            Err(BaselineError::FileMissing(p)) => assert_eq!(p, bogus),
            other => panic!("expected FileMissing, got {:?}", other),
        }
    }

    #[test]
    fn malformed_toml_returns_parse_error() {
        let f = write_temp("this is = not valid toml { [[[");
        match load_v02_baseline(f.path()) {
            Err(BaselineError::Parse { path, .. }) => assert_eq!(path, f.path()),
            other => panic!("expected Parse error, got {:?}", other),
        }
    }

    #[test]
    fn content_sha256_is_deterministic() {
        let toml_src = r#"
[tests]
v02_tag = "v0.2.2"
total = 280
list_sha = "abc"
captured_at = "2026-04-15"
"#;
        let f1 = write_temp(toml_src);
        let f2 = write_temp(toml_src);
        let a = load_v02_test_count(f1.path()).expect("a");
        let b = load_v02_test_count(f2.path()).expect("b");
        assert_eq!(
            a.content_sha256(),
            b.content_sha256(),
            "identical content must produce identical SHA"
        );
    }

    #[test]
    fn content_sha256_changes_when_content_changes() {
        let toml1 = r#"
[tests]
v02_tag = "v0.2.2"
total = 280
list_sha = "abc"
captured_at = "2026-04-15"
"#;
        let toml2 = r#"
[tests]
v02_tag = "v0.2.2"
total = 281
list_sha = "abc"
captured_at = "2026-04-15"
"#;
        let f1 = write_temp(toml1);
        let f2 = write_temp(toml2);
        let a = load_v02_test_count(f1.path()).expect("a");
        let b = load_v02_test_count(f2.path()).expect("b");
        assert_ne!(
            a.content_sha256(),
            b.content_sha256(),
            "different content must produce different SHA — immutability detection"
        );
    }
}
