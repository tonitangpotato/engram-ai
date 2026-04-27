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
//! **Structural placeholder.** Each sub-module is its own task per the
//! v03-benchmarks build plan (T3 task table). This `mod.rs` exists so the
//! crate compiles before the per-driver tasks land.
