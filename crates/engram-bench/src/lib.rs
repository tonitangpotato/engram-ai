//! # engram-bench
//!
//! Benchmark harness, drivers, scorers, and ship-gate evaluator for
//! Engram v0.3.
//!
//! This crate is a **separate workspace member** per `v03-benchmarks`
//! design §9.4 and master GUARD-9: its dependencies MUST NOT leak into
//! the runtime dependency graph of `engramai`. Building the system under
//! measurement (`cargo build -p engramai`) must succeed with this entire
//! crate absent.
//!
//! ## Architecture (design §3, §4, §6, §7)
//!
//! ```text
//!                 ┌──────────────────┐
//!     CLI ──────► │  harness         │ ──── reproducibility.toml (§6)
//!  (engram-bench) │  - runner        │ ──── <driver>_summary.json
//!                 │  - gates (§4)    │ ──── <driver>_per_query.jsonl
//!                 │  - repro (§6)    │
//!                 └────────┬─────────┘
//!                          │
//!                          ▼
//!     ┌────────────────────────────────────────────────────────┐
//!     │ drivers (one per suite, all implement BenchDriver)      │
//!     │   §3.1 locomo  §3.2 longmemeval  §3.3 cost              │
//!     │   §3.4 test_preservation  §3.5 cognitive_regression     │
//!     │   §3.6 migration_integrity                              │
//!     └─────────┬────────────────────┬────────────────────┬─────┘
//!               │                    │                    │
//!               ▼                    ▼                    ▼
//!         scorers (§3.1, §3.2)   anonymizer (§9.3.1)   baselines (§5)
//! ```
//!
//! ## Public surface (design §7.2)
//!
//! - [`BenchDriver`] — trait implemented by each suite driver.
//! - [`RunReport`] — a single driver run's outcome (gates + artifacts).
//! - [`ReleaseDecision`] — final ship/no-ship aggregation.
//! - [`ReproRecord`] — reproducibility-record schema (design §6.1).
//! - Gate primitives — [`GateResult`], [`GateStatus`], [`Comparator`],
//!   [`Priority`].
//! - [`Driver`] — driver identifier enum.
//! - [`HarnessConfig`], [`BenchError`] — runner inputs / errors.
//!
//! Top-level functions:
//!
//! - [`run_release_gate`] — execute the full release-qualification suite.
//! - [`aggregate_release_decision`] — collapse a vector of run reports
//!   into a single ship/no-ship decision.
//!
//! ## Module map
//!
//! - [`harness`] — runner + gate evaluator + reproducibility writer.
//! - [`drivers`] — one sub-module per benchmark suite.
//! - [`scorers`] — answer-equivalence judges for LOCOMO and LongMemEval.
//! - [`anonymizer`] — rustclaw-trace anonymization (design §9.3.1).
//! - [`baselines`] — typed loader for `benchmarks/baselines/*.toml`.
//! - [`reporting`] — stdout summary, drill-down, regression diff (§10).
//!
//! ## Status
//!
//! Crate root + module skeletons, established by `task:bench-impl-lib`.
//! Each module is filled in by its own implementation task per the
//! v03-benchmarks build plan (T3 task table).
//!
//! ## See also
//!
//! - Build plan: `.gid-v03-context/v03-benchmarks-build-plan.md`
//! - Design: `.gid/features/v03-benchmarks/design.md`
//! - Requirements: `.gid/features/v03-benchmarks/requirements.md`

#![warn(missing_docs)]

// ---------------------------------------------------------------------------
// Module declarations (design §3, §6, §7; build plan T2)
// ---------------------------------------------------------------------------

pub mod anonymizer;
pub mod baselines;
pub mod drivers;
pub mod harness;
pub mod reporting;
pub mod scorers;

// ---------------------------------------------------------------------------
// Public re-exports — the API surface defined in design §7.2.
//
// Re-exporting here lets CI integration code write
// `use engram_bench::{BenchDriver, RunReport, …}` instead of reaching into
// `engram_bench::harness::*`. The canonical home of each type is its module;
// these re-exports are convenience only.
// ---------------------------------------------------------------------------

pub use harness::{
    aggregate_release_decision, run_release_gate, BenchDriver, BenchError, Driver, HarnessConfig,
    Override, Rationale, ReleaseDecision, RunReport,
};
pub use harness::gates::{Comparator, GateResult, GateStatus, Priority};
pub use harness::repro::ReproRecord;
