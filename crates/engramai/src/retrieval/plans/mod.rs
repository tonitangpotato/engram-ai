//! # Retrieval execution plans
//!
//! One module per intent plan (design §4). Plans are the executable
//! counterparts to the [`Intent`](crate::retrieval::classifier::Intent)
//! variants the classifier produces. The cross-cutting [`bitemporal`]
//! helper is shared by Factual / Episodic / Hybrid plans (design §4.6).
//!
//! ## Module roster (filled incrementally per `v03-retrieval-build-plan.md` §5.2)
//!
//! - [`bitemporal`] — bi-temporal projection helper (cross-cutting).
//! - [`factual`] — Factual plan (`task:retr-impl-factual-bitemporal`).
//! - `episodic` (`task:retr-impl-episodic`) — pending.
//! - `associative` (`task:retr-impl-associative`) — pending.
//! - `abstract_l5` (`task:retr-impl-abstract-l5`) — pending.
//! - `affective` (`task:retr-impl-affective`) — pending.
//! - `hybrid` (`task:retr-impl-hybrid`) — pending.

pub mod bitemporal;
pub mod factual;

pub use bitemporal::{project_edges, AsOfMode, ProjectedEdge};
pub use factual::{
    EntityResolver, FactualOutcome, FactualPlan, FactualPlanInputs,
    FactualPlanResult, NullEntityResolver, ResolvedAnchor,
};
