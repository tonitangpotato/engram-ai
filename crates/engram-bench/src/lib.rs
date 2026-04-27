//! # engram-bench
//!
//! Benchmark harness, drivers, scorers, and ship-gate evaluator for Engram v0.3.
//!
//! This crate is a **separate workspace member**. Per `v03-benchmarks` design
//! §9.4 and master GUARD-9, its dependencies MUST NOT leak into the runtime
//! dependency graph of `engramai`. A `cargo build -p engramai` must succeed
//! with this crate absent.
//!
//! ## Status
//!
//! This file is a **placeholder** established by `task:bench-impl-cargo-toml`
//! (the manifest task). The full crate root — public re-exports of the
//! `Driver` trait, `RunReport`, `ReleaseDecision`, and `ReproRecord` — is
//! delivered by `task:bench-impl-lib` per the v03-benchmarks build plan.
//!
//! ## See also
//!
//! - Build plan: `.gid-v03-context/v03-benchmarks-build-plan.md`
//! - Design: `.gid/features/v03-benchmarks/design.md`
//! - Requirements: `.gid/features/v03-benchmarks/requirements.md`
