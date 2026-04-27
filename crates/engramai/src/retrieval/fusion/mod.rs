//! # Fusion & ranking (v0.3)
//!
//! Per design §5, candidates from a plan flow through:
//!
//! 1. Per-signal scoring (`signals.rs` — owned by `task:retr-impl-fusion`)
//! 2. Per-plan weighted combination (`combiner.rs` — same task)
//! 3. **Optional reranker** ([`reranker`] — this module,
//!    `task:retr-impl-reranker-contract`)
//!
//! Steps 1 and 2 are not yet present at the time this module was authored;
//! the reranker contract stands on its own and integrates with the fusion
//! output (`Vec<ScoredResult>`) regardless of how that output is produced.

pub mod combiner;
pub mod reranker;
pub mod signals;

pub use combiner::{
    combine, fuse_and_rank, reciprocal_rank_fusion, FusionConfig, FusionWeights,
    SignalWeightMatrix, RRF_DEFAULT_K,
};
pub use reranker::{
    assert_reranker_contract, ContractCheck, NullReranker, Reranker,
};
pub use signals::{
    actr_score, bm25_score, graph_score, recency_score, vector_score,
    BM25_DEFAULT_SATURATION, GRAPH_DEFAULT_DECAY, RECENCY_DEFAULT_HALF_LIFE,
};
