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
//! - [`episodic`] — Episodic plan (`task:retr-impl-episodic`).
//! - [`associative`] — Associative plan (`task:retr-impl-associative`).
//! - [`abstract_l5`] — Abstract / L5 plan (`task:retr-impl-abstract-l5`).
//! - `affective` (`task:retr-impl-affective`) — pending.
//! - `hybrid` (`task:retr-impl-hybrid`) — pending.

pub mod bitemporal;
pub mod episodic;
pub mod factual;
pub mod associative;
pub mod abstract_l5;

pub use bitemporal::{project_edges, AsOfMode, ProjectedEdge};
pub use episodic::{
    EpisodicMemoryStore, EpisodicOutcome, EpisodicPlan, EpisodicPlanInputs,
    EpisodicPlanResult, KnowledgeCutoff, NullEpisodicStore, ResolvedWindow,
};
pub use factual::{
    EntityResolver, FactualOutcome, FactualPlan, FactualPlanInputs,
    FactualPlanResult, NullEntityResolver, ResolvedAnchor,
};
pub use associative::{
    AssociativeCandidate, AssociativeOutcome, AssociativePlan,
    AssociativePlanInputs, AssociativePlanResult, NullSeedRecaller,
    SeedHit, SeedRecallStatus, SeedRecaller,
    DEFAULT_K_POOL, DEFAULT_K_SEED,
};
pub use abstract_l5::{
    AbstractCandidate, AbstractOutcome, AbstractPlan, AbstractPlanInputs,
    AbstractPlanResult, NullTopicSearcher, TopicHit, TopicSearchStatus,
    TopicSearcher, DEFAULT_K_TOPICS, DEFAULT_L5_MIN_TOPIC_SCORE,
};
