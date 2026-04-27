//! Benchmark drivers (design §3).
//!
//! One module per suite — each implements [`crate::harness::BenchDriver`]:
//!
//! - `locomo` — design §3.1 (LOCOMO multi-turn memory)
//! - `longmemeval` — design §3.2 (long-context retrieval)
//! - `cost` — design §3.3 (N=500 ingest cost harness)
//! - `test_preservation` — design §3.4 (v0.2 test replay)
//! - `cognitive_regression` — design §3.5 (three-feature directional)
//! - `migration_integrity` — design §3.6 (migration data integrity)
//!
//! ## Status
//!
//! `locomo` landed via `task:bench-impl-driver-locomo`. `cost` landed
//! via `task:bench-impl-driver-cost` (skeleton + GOAL-2.11 placeholder
//! per the module-level docs in `cost.rs`). `longmemeval` landed via
//! `task:bench-impl-driver-longmemeval` (delta_pp computed against
//! immutable v0.2 baseline per §5.1). `test_preservation` landed via
//! `task:bench-impl-driver-test-preservation` — `cargo test` wrapper
//! around v0.2.2's test sources, with skip-aware migration-tool
//! detection (returns `blocked_by` when v03-migration is absent).
//! The remaining `cognitive_regression` and `migration_integrity`
//! drivers stay todo per the v03-benchmarks build plan T3 task table.

pub mod cost;
pub mod locomo;
pub mod longmemeval;
pub mod test_preservation;
