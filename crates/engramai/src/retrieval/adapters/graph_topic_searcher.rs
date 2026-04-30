//! `GraphTopicSearcher` — Abstract plan's [`TopicSearcher`] backed by
//! the v0.3 `knowledge_topics` rows held by [`GraphRead::list_topics`],
//! ranked by cosine similarity between the query embedding and each
//! topic's stored embedding plus a lightweight title/summary text-match
//! signal.
//!
//! ## Why list_topics + in-memory rank
//!
//! The Abstract plan's `TopicSearcher::search(query, namespace, top_k)`
//! contract does not require a SQL-side ANN index — see the trait
//! docstring's "Performance" caveat. v0.3 namespaces typically hold
//! O(10²) topics (the LoCoMo-26 corpus has ~30 after synthesis), so
//! pulling the namespace's topic list and scoring in-memory is
//! cheap (<1 ms for 100 topics × 768-dim embeddings) and keeps the
//! ranking logic transparent for benchmark debugging.
//!
//! When namespaces grow past ~10⁴ topics this adapter will need a
//! `Storage::search_topics_ann(query_vec, namespace, top_k)` SQL
//! variant — flagged in design §4.4 as the future ANN integration
//! point. The trait surface does not change.
//!
//! ## Scoring
//!
//! `score = 0.7 · cosine + 0.3 · token_overlap`, normalized to `[0, 1]`.
//!
//! - **cosine**: `(EmbeddingProvider::cosine_similarity + 1.0) / 2.0`
//!   (maps `[-1, 1] → [0, 1]`). Skipped when either the query has no
//!   embedding or the topic row has none — in that case the overlap
//!   term carries the full weight.
//! - **token_overlap**: lowercased Jaccard over `query.text` ∩
//!   (`title` ⊕ `summary`). Cheap, language-agnostic, deterministic.
//!   Used as a tie-breaker when embeddings are absent and as a
//!   secondary signal when both are present.
//!
//! Weights are aligned with `HybridSeedRecaller` (0.7 vector / 0.3 text)
//! for consistency. The Abstract plan's downstream fusion (§5.2) re-applies
//! its own weights on top, so absolute scaling here is less important
//! than relative ordering.
//!
//! ## Status
//!
//! Always emits `Ok` — the v0.3 knowledge-cutoff machinery lives in the
//! plan's bitemporal filter, not in topic search. Errors from
//! `list_topics` collapse to `(empty, Ok)` (matches `NullTopicSearcher`).

use uuid::Uuid;

use crate::embeddings::EmbeddingProvider;
use crate::graph::store::GraphRead;
use crate::retrieval::api::GraphQuery;
use crate::retrieval::plans::abstract_l5::{TopicHit, TopicSearchStatus, TopicSearcher};

/// Hard cap on topics scanned per query. 1 024 keeps scoring under ~5 ms
/// for 768-dim embeddings; realistic graphs are 1-2 orders of magnitude
/// below this.
const MAX_TOPICS_SCANNED: usize = 1024;

/// When `true`, include superseded topics in the candidate set. v0.3
/// design §4.4 step 1 specifies live-only by default; superseded
/// inclusion is a future feature flag (`task:retr-history-view`).
const INCLUDE_SUPERSEDED: bool = false;

/// Graph-backed [`TopicSearcher`] over `knowledge_topics`.
pub struct GraphTopicSearcher<'a> {
    pub graph: &'a dyn GraphRead,
    pub embedding: Option<&'a EmbeddingProvider>,
}

impl<'a> GraphTopicSearcher<'a> {
    pub fn new(
        graph: &'a dyn GraphRead,
        embedding: Option<&'a EmbeddingProvider>,
    ) -> Self {
        Self { graph, embedding }
    }
}

impl<'a> TopicSearcher for GraphTopicSearcher<'a> {
    fn search(
        &self,
        query: &GraphQuery,
        namespace: &str,
        top_k: usize,
    ) -> (Vec<TopicHit>, TopicSearchStatus) {
        if top_k == 0 || query.text.trim().is_empty() {
            return (Vec::new(), TopicSearchStatus::Ok);
        }

        // Pull namespace's live topics. Failure → empty Ok (plan handles
        // L5 unavailable as `DowngradedL5Unavailable`).
        let topics = match self
            .graph
            .list_topics(namespace, INCLUDE_SUPERSEDED, MAX_TOPICS_SCANNED)
        {
            Ok(t) => t,
            Err(_) => return (Vec::new(), TopicSearchStatus::Ok),
        };

        if topics.is_empty() {
            return (Vec::new(), TopicSearchStatus::Ok);
        }

        // Embed query once if a provider is plugged in. Failure → fall
        // back to text-overlap-only scoring (no error surface — same
        // graceful degradation as HybridSeedRecaller).
        let query_vec: Option<Vec<f32>> = match self.embedding {
            Some(p) => p.embed(&query.text).ok(),
            None => None,
        };

        let q_tokens = tokenize(&query.text);

        let mut scored: Vec<(Uuid, f64)> = Vec::with_capacity(topics.len());
        for topic in topics {
            let cosine: Option<f64> = match (&query_vec, &topic.embedding) {
                (Some(qv), Some(tv)) if qv.len() == tv.len() => {
                    let sim = EmbeddingProvider::cosine_similarity(qv, tv);
                    // [-1, 1] → [0, 1].
                    Some(((sim + 1.0) / 2.0) as f64)
                }
                _ => None,
            };

            // Lowercased Jaccard over query tokens vs (title + summary).
            let mut t_text = topic.title.clone();
            t_text.push(' ');
            t_text.push_str(&topic.summary);
            let t_tokens = tokenize(&t_text);
            let overlap = jaccard(&q_tokens, &t_tokens);

            // Combined score:
            //   - both present: 0.7·cosine + 0.3·overlap
            //   - only cosine:  cosine
            //   - only overlap: overlap
            //   - neither:      0.0 (skip)
            let score = match cosine {
                Some(c) => 0.7 * c + 0.3 * overlap,
                None => overlap,
            };

            // Drop zero-score topics — surfacing them would inflate the
            // candidate count without providing signal. The plan's
            // threshold filter (§4.4 step 2) would drop them anyway.
            if score > 0.0 {
                scored.push((topic.topic_id, score));
            }
        }

        // Sort by (score desc, topic_id asc) for determinism.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        let hits: Vec<TopicHit> = scored
            .into_iter()
            .take(top_k)
            .map(|(topic_id, score)| TopicHit { topic_id, score })
            .collect();

        (hits, TopicSearchStatus::Ok)
    }
}

/// Tokenize on whitespace + ASCII punctuation, lowercased. CJK
/// characters survive intact (each rune is its own "word") which is
/// adequate for the bilingual queries we see in practice. A future
/// language-aware tokenizer (`task:retr-cjk-tokenizer`) can replace
/// this without touching the trait surface.
fn tokenize(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Jaccard index `|A ∩ B| / |A ∪ B|` for two token sets.
/// Returns `0.0` when both sides are empty (defined for our use case as
/// "no signal").
fn jaccard(
    a: &std::collections::HashSet<String>,
    b: &std::collections::HashSet<String>,
) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::store::{GraphWrite, SqliteGraphStore};
    use crate::graph::test_helpers::fresh_conn;
    use crate::graph::topic::KnowledgeTopic;
    use crate::graph::{Entity, EntityKind};
    use chrono::Utc;

    fn write_topic(
        store: &mut SqliteGraphStore,
        title: &str,
        summary: &str,
        ns: &str,
    ) -> Uuid {
        let topic_id = Uuid::new_v4();
        // graph_topics.id has a FK on graph_entities(id). Mirror a Topic
        // entity row before upserting (production path: synthesis.rs §230).
        let mut e = Entity::new_random_id(title.to_string(), EntityKind::Topic, Utc::now());
        e.id = topic_id;
        store.insert_entity(&e).expect("insert mirror entity");

        let topic = KnowledgeTopic {
            topic_id,
            title: title.to_string(),
            summary: summary.to_string(),
            embedding: None,
            source_memories: Vec::new(),
            contributing_entities: Vec::new(),
            cluster_weights: None,
            synthesis_run_id: None,
            synthesized_at: Utc::now().timestamp() as f64,
            superseded_by: None,
            superseded_at: None,
            namespace: ns.to_string(),
        };
        store.upsert_topic(&topic).expect("upsert");
        topic.topic_id
    }

    #[test]
    fn empty_query_returns_empty_ok() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let s = GraphTopicSearcher::new(&store, None);
        let q = GraphQuery::new("");
        let (hits, status) = s.search(&q, "default", 10);
        assert!(hits.is_empty());
        assert_eq!(status, TopicSearchStatus::Ok);
    }

    #[test]
    fn k_zero_returns_empty_ok() {
        let mut conn = fresh_conn();
        let store = SqliteGraphStore::new(&mut conn);
        let s = GraphTopicSearcher::new(&store, None);
        let q = GraphQuery::new("anything");
        let (hits, status) = s.search(&q, "default", 0);
        assert!(hits.is_empty());
        assert_eq!(status, TopicSearchStatus::Ok);
    }

    #[test]
    fn token_overlap_finds_matching_topic() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let id = write_topic(
            &mut store,
            "Caroline's coffee preferences",
            "Caroline likes oat milk lattes",
            "default",
        );
        let _other = write_topic(
            &mut store,
            "Quantum cryptography lecture notes",
            "Notes on lattice-based schemes",
            "default",
        );
        let s = GraphTopicSearcher::new(&store, None);
        let q = GraphQuery::new("What does Caroline drink?");
        let (hits, _) = s.search(&q, "default", 5);
        assert!(!hits.is_empty(), "expected at least one match");
        assert_eq!(
            hits[0].topic_id, id,
            "Caroline-titled topic should rank first"
        );
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn unknown_namespace_returns_empty_ok() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _ = write_topic(&mut store, "Hello", "world", "default");
        let s = GraphTopicSearcher::new(&store, None);
        let q = GraphQuery::new("hello");
        let (hits, status) = s.search(&q, "no-such-namespace", 10);
        assert!(hits.is_empty());
        assert_eq!(status, TopicSearchStatus::Ok);
    }

    #[test]
    fn results_sorted_by_score_desc() {
        let mut conn = fresh_conn();
        let mut store = SqliteGraphStore::new(&mut conn);
        let _strong = write_topic(
            &mut store,
            "rust async runtime tokio",
            "tokio is the de-facto async runtime",
            "default",
        );
        let _weak = write_topic(
            &mut store,
            "JavaScript event loop",
            "single-threaded async via callbacks",
            "default",
        );
        let s = GraphTopicSearcher::new(&store, None);
        let q = GraphQuery::new("tokio async runtime");
        let (hits, _) = s.search(&q, "default", 10);
        if hits.len() >= 2 {
            assert!(
                hits[0].score >= hits[1].score,
                "topics should be sorted by score desc"
            );
        }
    }
}
