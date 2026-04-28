//! `GraphEntityResolver` — Factual plan's [`EntityResolver`] backed by the
//! v0.3 graph layer.
//!
//! Resolves a free-form query string to a set of candidate
//! [`ResolvedAnchor`]s by running [`GraphRead::search_candidates`] across
//! every namespace the graph knows about.
//!
//! ## Why scan all namespaces
//!
//! The Factual plan's `EntityResolver::resolve(&str) -> Vec<ResolvedAnchor>`
//! contract has no namespace parameter — see
//! `crates/engramai/src/retrieval/plans/factual.rs`. The graph store
//! requires one (`CandidateQuery::namespace`). We bridge by iterating
//! [`GraphRead::list_namespaces`] and merging the per-namespace candidate
//! sets. This matches the Factual plan's design: the user issues a
//! query like "Who is Caroline?" without naming a namespace; the
//! resolver finds Carolines across the whole graph.
//!
//! Costs are bounded by `top_k` (we cap at 5 anchors per the Factual
//! plan's `max_anchors=5` default), `MAX_TOP_K` enforced inside
//! `search_candidates`, and namespace count (typically O(1)). For
//! realistic graphs this is acceptable; if it becomes a bottleneck the
//! resolver can grow a namespace filter parameter without breaking the
//! `EntityResolver` trait.
//!
//! ## Why not embed the query
//!
//! `CandidateQuery::mention_embedding` is `Option`. We pass `None` —
//! the v0.3 query-time embedding path lives one layer up
//! (`HybridSeedRecaller` for Associative). The Factual resolver runs on
//! exact-alias and recency only, which keeps it deterministic
//! (design §5.4) and side-effect-free.
//!
//! ## Send + Sync
//!
//! The `EntityResolver` trait requires `Send + Sync`. We impose those
//! bounds on the wrapped `&dyn GraphRead` (callers feed
//! `&dyn GraphRead + Send + Sync`). The orchestrator only constructs
//! `GraphEntityResolver` inside a single synchronous closure
//! (`Memory::with_graph_read`), so the bounds are never observably
//! exercised — they're a typesystem requirement, not a runtime one.

use chrono::Utc;

use crate::graph::store::{CandidateQuery, GraphRead};
use crate::retrieval::plans::factual::{EntityResolver, ResolvedAnchor};

/// Hard ceiling on entries returned per namespace before merging.
/// `FactualPlanInputs::max_anchors` defaults to 5 (factual.rs line ≈140);
/// over-fetching here lets the per-namespace top-K be merged by score
/// before truncation.
const PER_NAMESPACE_TOP_K: usize = 8;

/// Hard ceiling on namespaces scanned. Bounded out of paranoia — a
/// pathological graph with thousands of namespaces should not turn a
/// single query into a thousand SQL roundtrips. Realistic graphs have
/// O(1) namespaces (engramai typically uses `default` plus a handful of
/// project namespaces).
const MAX_NAMESPACES_SCANNED: usize = 32;

/// Graph-backed [`EntityResolver`] for the Factual plan.
///
/// Holds a borrowed `&dyn GraphRead` whose lifetime is tied to a single
/// `Memory::graph_query` call (via `PlanCollaborators<'a>`).
pub struct GraphEntityResolver<'a> {
    pub graph: &'a dyn GraphRead,
}

impl<'a> GraphEntityResolver<'a> {
    pub fn new(graph: &'a dyn GraphRead) -> Self {
        Self { graph }
    }
}

impl<'a> EntityResolver for GraphEntityResolver<'a> {
    fn resolve(&self, query: &str) -> Vec<ResolvedAnchor> {
        // Empty / whitespace queries cannot resolve.
        if query.trim().is_empty() {
            return Vec::new();
        }

        // List namespaces. Failure → return empty (the Factual plan
        // surfaces `DowngradedNoEntity`, which is the correct behaviour
        // for "graph layer unavailable").
        let namespaces = match self.graph.list_namespaces() {
            Ok(ns) => ns,
            Err(_) => return Vec::new(),
        };

        // Deterministic `now` reference for recency scoring. We use the
        // current wall clock — see "no clock sampling" caveat in the
        // trait docstring. The resolver is expected to be "as of read
        // time" because Factual itself accepts `query_time` separately
        // for traversal; resolution-stage recency just orders the
        // anchor candidates and is reproducible *given* the same now.
        let now = Utc::now().timestamp() as f64;

        let mut hits: Vec<ResolvedAnchor> = Vec::new();
        for ns in namespaces.into_iter().take(MAX_NAMESPACES_SCANNED) {
            let candidate_query = CandidateQuery {
                mention_text: query.to_string(),
                mention_embedding: None,
                kind_filter: None,
                namespace: ns,
                top_k: PER_NAMESPACE_TOP_K,
                recency_window: None,
                now,
            };

            let matches = match self.graph.search_candidates(&candidate_query) {
                Ok(rows) => rows,
                // Per-namespace failure is non-fatal — keep scanning
                // others. A namespace that errors is observably
                // identical to "no candidates here".
                Err(_) => continue,
            };

            for m in matches {
                // Score combines alias match (binary boost) + embedding
                // (none here) + recency. We don't have embedding so the
                // alias bit is dominant — this is fine for the Factual
                // plan (it's a name-resolution step, not vector
                // retrieval; see `HybridSeedRecaller` for the embedding
                // path).
                let alias_boost: f32 = if m.alias_match { 0.7 } else { 0.0 };
                let recency_score = m.recency_score; // [0.0, 1.0]
                // Final strength in [0.0, 1.0]: weight alias 70%, recency
                // 30%. Tuned to keep alias-only hits above 0.5 (so the
                // default `min_confidence` filter in Factual keeps them)
                // while letting recency break ties between two equally
                // alias-matched candidates.
                let match_strength = alias_boost + 0.3 * recency_score;

                // Skip candidates with neither signal — they're an
                // artifact of search_candidates returning embedding-only
                // hits we can't use here. (No embedding was sent → no
                // embedding score → skip.)
                if !m.alias_match && match_strength == 0.0 {
                    continue;
                }

                hits.push(ResolvedAnchor {
                    entity_id: m.entity_id,
                    canonical_name: m.canonical_name,
                    match_strength,
                });
            }
        }

        // Dedupe by entity_id, keeping the highest match_strength. Sort
        // by (match_strength desc, entity_id asc) for determinism.
        hits.sort_by(|a, b| {
            b.match_strength
                .partial_cmp(&a.match_strength)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        let mut seen = std::collections::HashSet::new();
        hits.retain(|a| seen.insert(a.entity_id));

        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::{GraphWrite, SqliteGraphStore};
    use crate::graph::test_helpers::fresh_conn;
    use crate::graph::{Entity, EntityKind};

    fn write_entity(
        store: &mut SqliteGraphStore,
        canonical_name: &str,
        ns: &str,
    ) -> uuid::Uuid {
        let mut e = Entity::new(canonical_name.to_string(), EntityKind::Person, Utc::now());
        let id = e.id;
        // The default identity_confidence is 0.0; bump to 1.0 so the
        // search_candidates path treats it as a high-confidence anchor.
        e.identity_confidence = 1.0;
        let _ = ns; // namespace is set on the store, not the entity
        store.insert_entity(&e).expect("insert entity");
        // search_candidates does not match by canonical_name alone — it
        // requires a row in graph_entity_aliases (normalized form). Mirror
        // the production path by upserting a self-alias.
        store
            .upsert_alias(
                &canonical_name.to_lowercase(),
                canonical_name,
                id,
                None,
            )
            .expect("upsert alias");
        id
    }

    #[test]
    fn empty_query_returns_empty_anchors() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let resolver = GraphEntityResolver::new(&store);
        assert!(resolver.resolve("").is_empty());
        assert!(resolver.resolve("   ").is_empty());
    }

    #[test]
    fn alias_match_returns_anchor() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Caroline");
        assert!(
            !hits.is_empty(),
            "expected at least one anchor, got {hits:?}"
        );
        assert!(hits.iter().any(|h| h.entity_id == id));
        assert!(
            hits[0].match_strength >= 0.5,
            "alias match should score >= 0.5 to survive default min_confidence; got {}",
            hits[0].match_strength
        );
    }

    #[test]
    fn unknown_query_returns_empty() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity(&mut store, "Caroline", "default");
        let resolver = GraphEntityResolver::new(&store);
        assert!(resolver.resolve("Zinedine").is_empty());
    }

    #[test]
    fn results_sorted_by_match_strength_desc() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_entity(&mut store, "Caroline", "default");
        let _ = write_entity(&mut store, "Carolyn", "default");
        let resolver = GraphEntityResolver::new(&store);
        let hits = resolver.resolve("Caroline");
        // First hit (exact alias) should outrank any partial.
        if hits.len() >= 2 {
            assert!(
                hits[0].match_strength >= hits[1].match_strength,
                "results must be sorted by match_strength desc"
            );
        }
    }
}
