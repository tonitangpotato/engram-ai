//! `HybridSeedRecaller` — Associative plan's [`SeedRecaller`] backed by
//! the engramai `hybrid_search` path (FTS + vector cosine), wrapped to
//! produce the `(Vec<SeedHit>, SeedRecallStatus)` shape the plan
//! expects.
//!
//! Owns three borrows:
//!
//! - `&Storage` — for FTS (`search_fts_ns`) and vector embeddings.
//! - `Option<&EmbeddingProvider>` — when present, the query string is
//!   embedded once and the cosine signal is added to the FTS rank.
//! - `&str namespace` — Storage's `hybrid_search` is namespace-scoped.
//!   The orchestrator passes `"default"` until `GraphQuery::namespace`
//!   plumbs through (matches the rest of v0.3 — see the `""`
//!   namespace literal in orchestrator.rs).
//! - `&str embedding_model` — the model identifier `hybrid_search`
//!   uses to look up stored vectors. Defaults to the provider's
//!   declared model when one is plugged in; falls back to a neutral
//!   string when no provider is configured (in which case the vector
//!   path is unused — only FTS contributes).
//!
//! ## Status semantics
//!
//! The trait gives us `Ok` / `Cutoff`. We never emit `Cutoff`: the
//! v0.3 knowledge-cutoff machinery lives in the plan's bitemporal
//! filter, not in the seed-recall layer. Storage failures collapse to
//! `(empty, Ok)` — the plan then surfaces `DowngradedNoSeeds`, which is
//! the design-intended outcome for "vector backend unavailable".
//!
//! ## Determinism
//!
//! `hybrid_search` orders by the final hybrid score; ties are broken by
//! Storage's row order. This is good enough for the LoCoMo benchmark
//! (which scores recall@10) but means two structurally-identical
//! queries can permute tied rows. That's a design property of FTS+vector
//! fusion, not an adapter bug — flagged here so future readers don't
//! chase a phantom non-determinism.
//!
//! ## No re-embedding on hot path?
//!
//! When `Some(provider)` is present we call `provider.embed(query)` per
//! `recall` invocation. The cost (~10–100 ms for a remote provider, <5
//! ms for local) is acceptable because the Associative plan is the
//! primary recall path: the budget already allocates a vector lookup
//! per query. Caching across calls is a future optimization
//! (`task:retr-cache-query-embeddings`).

use crate::embeddings::EmbeddingProvider;
use crate::hybrid_search::{hybrid_search, HybridSearchOpts};
use crate::retrieval::api::GraphQuery;
use crate::retrieval::plans::associative::{SeedHit, SeedRecallStatus, SeedRecaller};
use crate::storage::Storage;

/// Default top-K multiplier — seed recall typically requests 10 hits;
/// over-fetching to 30 lets the plan's downstream filter (entity-edge
/// expansion, dedup) work with headroom.
const SEED_OVERFETCH: usize = 3;

/// Hard cap on rows returned from the hybrid layer. 200 is well above
/// any realistic `k_seed` and prevents pathological calls (e.g.
/// `k_seed=usize::MAX`) from materializing unbounded vectors.
const MAX_SEEDS: usize = 200;

/// Hybrid (FTS + vector) seed recaller for the Associative plan.
///
/// All four fields are borrowed; the struct is constructed inside
/// `Memory::graph_query`'s `with_graph_read` closure and dropped before
/// the closure returns.
pub struct HybridSeedRecaller<'a> {
    pub storage: &'a Storage,
    pub embedding: Option<&'a EmbeddingProvider>,
    pub namespace: &'a str,
    pub embedding_model: &'a str,
}

impl<'a> HybridSeedRecaller<'a> {
    pub fn new(
        storage: &'a Storage,
        embedding: Option<&'a EmbeddingProvider>,
        namespace: &'a str,
        embedding_model: &'a str,
    ) -> Self {
        Self {
            storage,
            embedding,
            namespace,
            embedding_model,
        }
    }
}

impl<'a> SeedRecaller for HybridSeedRecaller<'a> {
    fn recall(
        &self,
        query: &GraphQuery,
        k_seed: usize,
    ) -> (Vec<SeedHit>, SeedRecallStatus) {
        if k_seed == 0 || query.text.trim().is_empty() {
            return (Vec::new(), SeedRecallStatus::Ok);
        }

        // Embed the query once if we have a provider. Failure → fall
        // back to FTS-only by passing `None` for the vector. Don't
        // surface the embedding error: the plan would treat it as a
        // hard failure when behaviorally we want graceful degradation.
        let query_vec: Option<Vec<f32>> = match self.embedding {
            Some(provider) => provider.embed(&query.text).ok(),
            None => None,
        };

        let fetch = k_seed.saturating_mul(SEED_OVERFETCH).min(MAX_SEEDS);
        let opts = HybridSearchOpts {
            vector_weight: 0.7,
            fts_weight: 0.3,
            limit: fetch,
            namespace: Some(self.namespace.to_string()),
            include_records: false, // Plan only needs (id, score).
        };

        let results = match hybrid_search(
            self.storage,
            query_vec.as_deref(),
            &query.text,
            opts,
            self.embedding_model,
        ) {
            Ok(r) => r,
            // Storage / FTS failure → empty `Ok`. Plan downgrades to
            // `DowngradedNoSeeds`; this matches `NullSeedRecaller`.
            Err(_) => return (Vec::new(), SeedRecallStatus::Ok),
        };

        let hits: Vec<SeedHit> = results
            .into_iter()
            .take(k_seed)
            .map(|r| SeedHit {
                memory_id: r.id,
                score: r.score,
            })
            .collect();

        (hits, SeedRecallStatus::Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_storage() -> Storage {
        Storage::new(":memory:").expect("in-memory storage")
    }

    #[test]
    fn empty_query_returns_empty_ok() {
        let storage = fresh_storage();
        let recaller = HybridSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("");
        let (hits, status) = recaller.recall(&q, 10);
        assert!(hits.is_empty());
        assert_eq!(status, SeedRecallStatus::Ok);
    }

    #[test]
    fn whitespace_query_returns_empty_ok() {
        let storage = fresh_storage();
        let recaller = HybridSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("   \t\n  ");
        let (hits, _) = recaller.recall(&q, 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn k_zero_returns_empty_ok() {
        let storage = fresh_storage();
        let recaller = HybridSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("anything");
        let (hits, status) = recaller.recall(&q, 0);
        assert!(hits.is_empty());
        assert_eq!(status, SeedRecallStatus::Ok);
    }

    #[test]
    fn empty_storage_returns_empty_ok_not_error() {
        let storage = fresh_storage();
        let recaller = HybridSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("hello world");
        let (hits, status) = recaller.recall(&q, 10);
        // No memories → empty hits, but status MUST be Ok (not a failure
        // signal). The plan downgrades to `DowngradedNoSeeds` which is
        // the correct behaviour, not `Cutoff`.
        assert!(hits.is_empty());
        assert_eq!(status, SeedRecallStatus::Ok);
    }
}
