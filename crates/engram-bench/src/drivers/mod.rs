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
//! per the module-level docs in `cost.rs`). The other drivers remain
//! todo per the v03-benchmarks build plan T3 task table.

pub mod cost;
pub mod locomo;
