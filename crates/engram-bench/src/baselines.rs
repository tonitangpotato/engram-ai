//! Baseline numbers consumed by gates (design §5).
//!
//! Loads `benchmarks/baselines/*.toml`:
//!
//! - `v02.toml` — pre-v0.3 LongMemEval baseline (§5.1).
//! - `v02_test_count.toml` — frozen v0.2 test count (§5.2).
//! - `external.toml` — published mem0 / Graphiti numbers (§5.3).
//!
//! ## Status
//!
//! **Structural placeholder.** Loader and typed accessor functions
//! (`load_v02_baseline()`, `load_external_baselines()`, etc.) are owned
//! by `task:bench-impl-baselines`.
