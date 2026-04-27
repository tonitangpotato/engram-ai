//! # v0.3 Retrieval — query-time path
//!
//! Read-side counterpart to the v0.3 [`resolution`](crate::resolution)
//! pipeline. Where resolution converts a write into a populated graph slice,
//! retrieval routes a read into one of five **intent plans** (Factual,
//! Episodic, Abstract, Affective, Hybrid) and assembles results across the
//! cognitive layers (L1–L5) with bi-temporal correctness, deterministic
//! ranking, and an opt-in explain trace.
//!
//! See `.gid/features/v03-retrieval/design.md` for the full spec.
//!
//! ## Layout (incremental, per the v03 retrieval build plan)
//!
//! - `classifier::heuristic` — Stage-1 signal scorers (this task,
//!   `task:retr-impl-classifier-heuristic`). Pure-function entity / temporal
//!   / abstract / affective signal extractors feeding the orchestrator.
//! - `classifier` (orchestrator), `plans/*`, `fusion`, `budget`, `metrics`,
//!   `outcomes`, `trace`, `reranker` — landed by sibling tasks. Modules are
//!   added as their tasks complete; this `mod.rs` only declares what
//!   currently exists so the crate keeps compiling at every step.

pub mod api;
pub mod budget;
pub mod classifier;
pub mod fusion;
pub mod outcomes;
pub mod plans;

pub use api::{
    EntityId, GraphQuery, GraphQueryResponse, MemoryTier, PlanTrace, ScoredResult, SubScores,
    TimeWindow,
};
pub use outcomes::{RetrievalError, RetrievalOutcome};
pub use budget::{
    BudgetController, CostCap, CostCaps, CostCounters, Stage, StageBudget,
};
pub use fusion::{NullReranker, Reranker};
