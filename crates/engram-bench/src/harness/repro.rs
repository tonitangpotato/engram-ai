//! Reproducibility record (design §6).
//!
//! Schema and writer for `reproducibility.toml` — the committed artifact
//! that lets a release-qualification run be re-executed bit-identically
//! months later (design §6.3 replay workflow).
//!
//! ## Status
//!
//! **Stub.** Schema fields and the writer are delivered by
//! `task:bench-impl-repro`. This file exists so `harness::mod` can name
//! `repro::ReproRecord` in re-exports today.

use serde::{Deserialize, Serialize};

/// Top-level reproducibility-record schema (design §6.1).
///
/// Mirrors the on-disk TOML structure: `[run]`, `[build]`, `[dataset]`,
/// `[fusion]`, `[models]`, `[result]`, `[gates]`, optional `[override]`.
///
/// **Stub.** Full sub-table types and the TOML round-trip implementation
/// land with `task:bench-impl-repro`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReproRecord {
    /// Placeholder marker — replaced by typed sub-tables in
    /// `task:bench-impl-repro`. Keeps the type non-empty so serde derives
    /// don't trip the `dead_code` lint while the stub is in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
}
