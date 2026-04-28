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
//! `cognitive_regression` and `migration_integrity` landed in
//! skip-aware form (2026-04-27): the call sites are wired but the
//! real pipelines are gated on `task:retr-impl-orchestrator-*` (the
//! `Memory::graph_query` orchestrator stub). Both drivers detect the
//! stub and surface `GateStatus::Error` per GUARD-2 — never silent
//! pass. When the orchestrator lands, the same drivers activate by
//! probe-fall-through (no further code change to mod.rs).

pub mod cognitive_regression;
pub mod cost;
pub mod locomo;
pub mod longmemeval;
pub mod migration_integrity;
pub mod test_preservation;
