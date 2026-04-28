//! Real-data adapters for the five plan-collaborator traits.
//!
//! Each adapter is a small struct that holds borrowed handles to the
//! collaborator's data sources (a `&dyn GraphRead`, a `&Storage`, an
//! `Option<&EmbeddingProvider>`) and implements the corresponding plan
//! trait. None of them mutate `Memory` — see ISS-049 risk note #3.
//!
//! ## Module layout
//!
//! - [`graph_entity_resolver`] — Factual ([`EntityResolver`])
//! - [`storage_episodic_store`] — Episodic ([`EpisodicMemoryStore`])
//! - [`hybrid_seed_recaller`] — Associative ([`SeedRecaller`])
//! - [`graph_topic_searcher`] — Abstract ([`TopicSearcher`])
//! - [`hybrid_affective_seed_recaller`] — Affective ([`AffectiveSeedRecaller`])
//!
//! All five are constructed inside `Memory::graph_query` and bundled into
//! a [`PlanCollaborators`](crate::retrieval::orchestrator::PlanCollaborators)
//! that the orchestrator passes through to `execute_plan`.

pub mod graph_entity_resolver;
pub mod graph_topic_searcher;
pub mod hybrid_affective_seed_recaller;
pub mod hybrid_seed_recaller;
pub mod storage_episodic_store;

pub use graph_entity_resolver::GraphEntityResolver;
pub use graph_topic_searcher::GraphTopicSearcher;
pub use hybrid_affective_seed_recaller::HybridAffectiveSeedRecaller;
pub use hybrid_seed_recaller::HybridSeedRecaller;
pub use storage_episodic_store::StorageEpisodicStore;
