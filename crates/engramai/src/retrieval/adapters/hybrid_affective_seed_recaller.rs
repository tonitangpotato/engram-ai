//! `HybridAffectiveSeedRecaller` — Affective plan's
//! [`AffectiveSeedRecaller`] backed by the same FTS+vector hybrid path
//! as [`super::hybrid_seed_recaller::HybridSeedRecaller`], but extended
//! to return [`AffectiveSeedHit`] rows that carry `recency_score` and
//! the memory's `affect_snapshot`.
//!
//! ## Affect snapshots are currently `None`
//!
//! v0.3 storage does not yet persist a per-memory affect fingerprint
//! (the `task:graph-impl-affect-snapshot-write` work is queued behind
//! the resolution pipeline rollout). Until that lands, this adapter
//! emits `affect_snapshot: None` for every hit — the plan treats this
//! as neutral (`affect_similarity = 0`) per its module docs, so the
//! Affective ranking degrades to `0.55·text + 0·affect + 0.15·recency`
//! and the plan downgrades cleanly when no signal survives the
//! threshold filter.
//!
//! Crucially, the trait contract forbids dropping affectless rows
//! ("MUST NOT drop such rows: doing so would silently violate
//! GUARD-6"). We honour that by emitting every text hit regardless of
//! affect availability.
//!
//! ## Recency scoring
//!
//! `recency_score = exp(-age_seconds / HALF_LIFE_SECS)` clamped to
//! `[0, 1]`. We use the memory's `created_at` (returned by
//! `hybrid_search` when `include_records=true`) and the current wall
//! clock — same caveat as `GraphEntityResolver`'s `now`.
//!
//! Half-life is 14 days (`14 * 86_400 ≈ 1.21e6 s`). Tuned to match the
//! Affective plan's typical query horizon: a year-old memory ranks at
//! `e^(-365/14) ≈ 1e-11` (effectively zero), a week-old memory at
//! `e^(-0.5) ≈ 0.61`. The plan's `W_RECENCY = 0.15` keeps recency from
//! dominating text relevance even at the freshest end.
//!
//! ## Why include_records: true here (vs false in HybridSeedRecaller)
//!
//! Recency requires `record.created_at`; the Associative plan only
//! needs `(id, score)` so its hybrid call sets `include_records: false`
//! to skip the row hydration. Hybrid_search's per-row hydration cost is
//! one SQL roundtrip per result — bounded by `top_k * SEED_OVERFETCH`,
//! typically <30 rows.

use chrono::Utc;

use crate::embeddings::EmbeddingProvider;
use crate::hybrid_search::{hybrid_search, HybridSearchOpts};
use crate::retrieval::api::GraphQuery;
use crate::retrieval::plans::affective::{
    AffectiveSeedHit, AffectiveSeedRecaller, AffectiveSeedStatus,
};
use crate::storage::Storage;

/// Over-fetch multiplier — match `HybridSeedRecaller`. The plan's
/// affective threshold (§4.5 step 3) drops some candidates, so 3× gives
/// the per-call top-K headroom.
const AFFECT_OVERFETCH: usize = 3;

/// Hard cap on hits — paranoia-bound against pathological `top_k`.
const MAX_AFFECT_HITS: usize = 200;

/// Recency half-life in seconds. 14 days — see module docs.
const HALF_LIFE_SECS: f64 = 14.0 * 86_400.0;

/// Affective seed recaller backed by `hybrid_search` + per-hit recency.
pub struct HybridAffectiveSeedRecaller<'a> {
    pub storage: &'a Storage,
    pub embedding: Option<&'a EmbeddingProvider>,
    pub namespace: &'a str,
    pub embedding_model: &'a str,
}

impl<'a> HybridAffectiveSeedRecaller<'a> {
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

impl<'a> AffectiveSeedRecaller for HybridAffectiveSeedRecaller<'a> {
    fn recall(
        &self,
        query: &GraphQuery,
        top_k: usize,
    ) -> (Vec<AffectiveSeedHit>, AffectiveSeedStatus) {
        if top_k == 0 || query.text.trim().is_empty() {
            return (Vec::new(), AffectiveSeedStatus::Ok);
        }

        let query_vec: Option<Vec<f32>> = match self.embedding {
            Some(p) => p.embed(&query.text).ok(),
            None => None,
        };

        let fetch = top_k.saturating_mul(AFFECT_OVERFETCH).min(MAX_AFFECT_HITS);
        let opts = HybridSearchOpts {
            vector_weight: 0.7,
            fts_weight: 0.3,
            limit: fetch,
            namespace: Some(self.namespace.to_string()),
            // We need `record.created_at` for recency scoring.
            include_records: true,
        };

        let results = match hybrid_search(
            self.storage,
            query_vec.as_deref(),
            &query.text,
            opts,
            self.embedding_model,
        ) {
            Ok(r) => r,
            Err(_) => return (Vec::new(), AffectiveSeedStatus::Ok),
        };

        let now_secs = Utc::now().timestamp() as f64;

        let hits: Vec<AffectiveSeedHit> = results
            .into_iter()
            .take(top_k)
            .map(|r| {
                let recency_score = match r.record.as_ref() {
                    Some(rec) => {
                        let age = now_secs - rec.created_at.timestamp() as f64;
                        // Clamp `age` to non-negative — clock skew /
                        // future-dated rows shouldn't produce
                        // `recency_score > 1.0`.
                        let age = age.max(0.0);
                        let s = (-age / HALF_LIFE_SECS).exp();
                        // Clamp to [0, 1] for safety; exp() is bounded
                        // but f64 rounding could nudge it.
                        s.clamp(0.0, 1.0)
                    }
                    // No record hydrated → score 0 (unknown age) rather
                    // than dropping the row (GUARD-6: never drop hits).
                    None => 0.0,
                };

                AffectiveSeedHit {
                    memory_id: r.id,
                    text_score: r.score,
                    recency_score,
                    // v0.3 storage does not yet persist per-memory
                    // affect fingerprints — see module docs. The plan
                    // handles `None` as `affect_similarity = 0`.
                    affect_snapshot: None,
                }
            })
            .collect();

        (hits, AffectiveSeedStatus::Ok)
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
        let recaller =
            HybridAffectiveSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("");
        let (hits, status) = recaller.recall(&q, 10);
        assert!(hits.is_empty());
        assert_eq!(status, AffectiveSeedStatus::Ok);
    }

    #[test]
    fn whitespace_query_returns_empty_ok() {
        let storage = fresh_storage();
        let recaller =
            HybridAffectiveSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("\t  \n");
        let (hits, _) = recaller.recall(&q, 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn k_zero_returns_empty_ok() {
        let storage = fresh_storage();
        let recaller =
            HybridAffectiveSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("anything");
        let (hits, status) = recaller.recall(&q, 0);
        assert!(hits.is_empty());
        assert_eq!(status, AffectiveSeedStatus::Ok);
    }

    #[test]
    fn empty_storage_returns_empty_ok_not_error() {
        let storage = fresh_storage();
        let recaller =
            HybridAffectiveSeedRecaller::new(&storage, None, "default", "test-model");
        let q = GraphQuery::new("hello world");
        let (hits, status) = recaller.recall(&q, 10);
        assert!(hits.is_empty());
        // GUARD-6: backend failure / empty result must not surface as
        // anything other than `Ok` with empty hits.
        assert_eq!(status, AffectiveSeedStatus::Ok);
    }
}
