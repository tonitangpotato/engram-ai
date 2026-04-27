//! Benchmark scorers (design §3.1, §3.2 scoring methodology).
//!
//! - `locomo` — answer-equivalence judge for LOCOMO (§3.1, §11.1).
//!   Owned by `task:bench-impl-scorer-locomo`.
//! - `longmemeval` — answer-correctness judge for LongMemEval (§3.2).
//!   Owned by `task:bench-impl-scorer-longmemeval`.

pub mod locomo;
pub mod longmemeval;
