//! # Abstract / L5 Knowledge Topic plan (`task:retr-impl-abstract-l5`)
//!
//! Thematic / summary retrieval against the **L5 Knowledge Topic** table
//! (`knowledge_topics`, v03-graph-layer §4.1). Implements design **§4.4**
//! (`.gid/features/v03-retrieval/design.md`).
//!
//! ## Pipeline (mirrors design §4.4 numbering)
//!
//! 1. **Topic candidate recall.** Vector + BM25 search over
//!    `knowledge_topics` via the injected [`TopicSearcher`] trait. The
//!    plan does **not** know the index implementation — production
//!    wires the real engine, tests stub.
//! 2. **Traceability expansion.** For each candidate topic, load the
//!    full [`KnowledgeTopic`] row via [`GraphRead::get_topic`].
//!    `source_memories` and `contributing_entities` are stored as JSON
//!    arrays on the row itself (graph-layer §4.1 schema) — no joins,
//!    just a row read.
//! 3. **Rerank.** Optional cross-encoder rerank is `task:retr-impl-
//!    reranker-contract` territory. This plan emits unscored rows
//!    (just like Factual / Episodic / Associative) and lets fusion
//!    apply per-plan weights.
//! 4. **Result shape.** [`AbstractCandidate`] — fusion converts these
//!    to [`crate::retrieval::api::ScoredResult::Topic`] (design §6.2).
//!
//! ## `cluster_weights` (GOAL-3.7)
//!
//! `KnowledgeTopic.cluster_weights` (set by the Knowledge Compiler at
//! synthesis time, v03-resolution §5bis.3) is **respected, not
//! recomputed**. The plan forwards it unchanged on each candidate row;
//! callers (benchmarks, fusion) read it directly off the topic. This
//! plan never modifies the field — write-side ownership lives in
//! v03-resolution per the §4.4 "the retrieval feature consumes L5; it
//! does not synthesize it" boundary.
//!
//! ## Downgrade lattice (§4.4 closing paragraph + GUARD-2)
//!
//! L5 may be unavailable for legitimate reasons: a fresh database, the
//! compiler hasn't run yet, or the candidate set's top-K topic scores
//! are all below `config.l5_min_topic_score`. In any of these cases the
//! plan surfaces [`AbstractOutcome::DowngradedL5Unavailable`] with
//! `downgrade_reason = "L5_unavailable"` — the orchestrator (`task:retr-
//! impl-graph-query-api`) is responsible for routing to Associative.
//! Returning *nothing* would violate GUARD-2 ("never silently degrade").
//!
//! On-demand synthesis from the read path is **not** attempted; it
//! would break the latency budget (§7). Operators run
//! `compile_knowledge` explicitly or wait for the scheduled run.
//!
//! ## What this module does NOT do
//!
//! - **No scoring or ranking.** Sub-scores and final ordering are
//!   applied by the fusion module (`task:retr-impl-fusion`, design
//!   §5). The plan is a pure data collector.
//! - **No L5 synthesis.** The compiler (v03-resolution §5bis) writes
//!   `knowledge_topics`; this plan only reads.
//! - **No Associative fallback execution.** When L5 is empty we
//!   surface [`AbstractOutcome::DowngradedL5Unavailable`] and let the
//!   orchestrator route per design §3.4.
//! - **No reranker invocation.** The optional cross-encoder pass
//!   (§5.3) is wired in fusion / response assembly, not here.
//!
//! ## Design refs / requirements
//!
//! - Design §4.4 (Abstract / L5 plan), §5bis (Knowledge Compiler producer)
//! - GOAL-3.6 — abstract → L5 topics with traces
//! - GOAL-3.7 — affect-weighted clustering input respected (cluster_weights)
//! - GUARD-2  — never silently degrade (encoded via the
//!   `DowngradedL5Unavailable` outcome surfaced to the orchestrator)

use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::graph::store::GraphRead;
use crate::graph::topic::KnowledgeTopic;
use crate::retrieval::api::{EntityId, GraphQuery};
use crate::retrieval::budget::{BudgetController, Stage};
use crate::store_api::MemoryId;

// ---------------------------------------------------------------------------
// 1. Constants & defaults
// ---------------------------------------------------------------------------

/// Default top-K cap on topic candidates pulled from the searcher
/// before traceability expansion. Mirrors `K_seed = 10` from the
/// Associative plan — small enough that a few row reads stay cheap,
/// large enough that the cluster_weights diff workflows in
/// `v03-benchmarks` see a meaningful spread.
pub const DEFAULT_K_TOPICS: usize = 10;

/// Default minimum score threshold below which a topic candidate is
/// considered "no useful L5 hit" (design §4.4, last paragraph). When
/// every candidate is below this threshold the plan downgrades — see
/// [`AbstractOutcome::DowngradedL5Unavailable`].
///
/// `0.0` is the conservative default: only treat L5 as unavailable
/// when the searcher itself returned nothing. Production deployments
/// raise this threshold via `with_min_topic_score` once a calibrated
/// floor exists.
pub const DEFAULT_L5_MIN_TOPIC_SCORE: f64 = 0.0;

// ---------------------------------------------------------------------------
// 2. TopicSearcher trait + NullTopicSearcher
// ---------------------------------------------------------------------------

/// Output of [`TopicSearcher::search`]. The plan does not interpret
/// `score`; it forwards as-is to fusion (where the abstract weights
/// in §5.2 combine `topic_match` with reranker output if present).
///
/// `score` is the *combined* vector + BM25 ranking score from the
/// searcher backend, in `[0.0, 1.0]`. Backends that emit only one
/// signal MUST normalize before returning so the threshold semantics
/// in [`AbstractPlan`] stay backend-agnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicHit {
    pub topic_id: Uuid,
    pub score: f64,
}

/// Status returned by a topic searcher. `Cutoff` lets the searcher
/// surface a knowledge-cutoff gate without the plan having to know
/// the storage clock — symmetric with [`super::associative::
/// SeedRecallStatus`] (GUARD-2 stays clean).
#[derive(Debug, Clone, PartialEq)]
pub enum TopicSearchStatus {
    /// Hits returned (possibly empty). Plan handles `Empty` and
    /// `DowngradedL5Unavailable` itself.
    Ok,
    /// Backend declared the query strictly outside the knowledge
    /// cutoff window.
    Cutoff,
}

/// Vector + BM25 search over `knowledge_topics` (design §4.4 step 1).
/// Implementations MUST honour `namespace` as a hard filter and MUST
/// return at most `top_k` hits ordered by descending `score`.
///
/// The trait is object-safe so plan instances can hold `Box<dyn
/// TopicSearcher>` if needed.
pub trait TopicSearcher {
    /// Run the topic recall step.
    ///
    /// `namespace` is an explicit parameter (not derived from the
    /// query) because [`GraphQuery`] does not yet expose namespace —
    /// the orchestrator passes it from request context. Backends MUST
    /// scope to `namespace` and MUST NOT cross namespace boundaries.
    fn search(
        &self,
        query: &GraphQuery,
        namespace: &str,
        top_k: usize,
    ) -> (Vec<TopicHit>, TopicSearchStatus);
}

/// Empty-result searcher. Useful as a default for unit tests that
/// want the plan's "L5 substrate is missing" behaviour without
/// constructing a stub.
#[derive(Debug, Clone, Default)]
pub struct NullTopicSearcher;

impl TopicSearcher for NullTopicSearcher {
    fn search(
        &self,
        _q: &GraphQuery,
        _ns: &str,
        _k: usize,
    ) -> (Vec<TopicHit>, TopicSearchStatus) {
        (Vec::new(), TopicSearchStatus::Ok)
    }
}

// ---------------------------------------------------------------------------
// 3. AbstractCandidate (output row)
// ---------------------------------------------------------------------------

/// Pre-fusion candidate row produced by the Abstract plan.
///
/// Fusion (§5.2) consumes `topic_score` as the primary signal and
/// converts the row to [`crate::retrieval::api::ScoredResult::Topic`].
/// `source_memories` / `contributing_entities` are forwarded to that
/// variant unchanged (design §6.2 — the topic carries its own
/// provenance, no extra joins).
///
/// `topic` is the **full** [`KnowledgeTopic`] row — including
/// `cluster_weights` (GOAL-3.7) and the supersession pointer fields.
/// The plan never strips fields; fusion / response assembly decide
/// what to expose to the caller.
#[derive(Debug, Clone)]
pub struct AbstractCandidate {
    pub topic: KnowledgeTopic,
    /// Score from the [`TopicSearcher`] backend. The fusion module
    /// re-normalizes / weights — see §5.1 (`topic_match`).
    pub topic_score: f64,
    /// Snapshot of `topic.source_memories` for explicit fusion access.
    /// Duplicated for ergonomics and so callers can move the topic
    /// out of the candidate without losing provenance.
    pub source_memories: Vec<MemoryId>,
    /// Snapshot of `topic.contributing_entities`. Same rationale as
    /// `source_memories`.
    pub contributing_entities: Vec<EntityId>,
}

// ---------------------------------------------------------------------------
// 4. Inputs / outputs / outcome
// ---------------------------------------------------------------------------

/// Inputs assembled by the dispatcher before invoking
/// [`AbstractPlan::execute`]. Mirrors
/// [`super::associative::AssociativePlanInputs`] so the orchestrator
/// keeps a uniform call-site shape across plans.
pub struct AbstractPlanInputs<'a> {
    /// Original query — surfaces `limit` and the eventual reranker
    /// opt-in. `min_confidence` does not apply to topic recall (no
    /// edges involved).
    pub query: &'a GraphQuery,

    /// Namespace scoping for [`TopicSearcher::search`] and
    /// [`GraphRead::list_topics`]. The orchestrator threads the
    /// request namespace here (see `task:retr-impl-graph-query-api`).
    pub namespace: &'a str,

    /// Per-stage cost controller. Plan never panics on exhaustion —
    /// it short-circuits with whatever it has so far (design §7.3,
    /// "cutoff returns partial, never error").
    pub budget: BudgetController,
}

/// Typed outcome (§6.4). The plan never returns scored rows; fusion
/// owns scoring (§5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum AbstractOutcome {
    /// Pipeline produced ≥ 1 candidate.
    Ok,
    /// Searcher returned hits but every candidate failed the
    /// traceability load (deleted topic row, etc.). Distinct from
    /// `DowngradedL5Unavailable` because the substrate *exists*; the
    /// caller may surface a "transient" outcome instead of routing
    /// to Associative.
    Empty,
    /// L5 substrate is unavailable for the query: searcher returned
    /// nothing OR every candidate scored below
    /// [`AbstractPlan::min_topic_score`]. The orchestrator routes to
    /// Associative per design §3.4 / §4.4.
    DowngradedL5Unavailable,
    /// Searcher signalled a knowledge-cutoff gate.
    Cutoff,
}

/// Plan output. `candidates` is **unscored** — see
/// [`AbstractCandidate::topic_score`].
#[derive(Debug, Clone)]
pub struct AbstractPlanResult {
    pub candidates: Vec<AbstractCandidate>,
    pub outcome: AbstractOutcome,
    pub elapsed: Duration,
}

// ---------------------------------------------------------------------------
// 5. AbstractPlan struct + execute()
// ---------------------------------------------------------------------------

/// Abstract / L5 plan. Generic over the [`TopicSearcher`] backend —
/// production wires the real vector + BM25 index, tests stub. The
/// `GraphRead` dependency is dyn-borrowed at `execute` time so a
/// single plan instance can be reused across queries.
#[derive(Debug, Clone)]
pub struct AbstractPlan<S = NullTopicSearcher>
where
    S: TopicSearcher,
{
    searcher: S,
    /// Top-K cap on topic candidates before traceability expansion.
    pub k_topics: usize,
    /// Score floor below which a candidate is treated as "no L5 hit"
    /// (design §4.4, last paragraph).
    pub min_topic_score: f64,
}

impl Default for AbstractPlan<NullTopicSearcher> {
    fn default() -> Self {
        Self {
            searcher: NullTopicSearcher,
            k_topics: DEFAULT_K_TOPICS,
            min_topic_score: DEFAULT_L5_MIN_TOPIC_SCORE,
        }
    }
}

impl<S> AbstractPlan<S>
where
    S: TopicSearcher,
{
    pub fn new(searcher: S) -> Self {
        Self {
            searcher,
            k_topics: DEFAULT_K_TOPICS,
            min_topic_score: DEFAULT_L5_MIN_TOPIC_SCORE,
        }
    }

    /// Override the top-K topic candidate cap. Values < 1 are clamped
    /// to 1 — a zero-K run is meaningless and would always downgrade.
    pub fn with_k_topics(mut self, k: usize) -> Self {
        self.k_topics = k.max(1);
        self
    }

    /// Override the L5 substrate-availability score floor. Values are
    /// clamped to `[0.0, 1.0]`.
    pub fn with_min_topic_score(mut self, s: f64) -> Self {
        self.min_topic_score = s.clamp(0.0, 1.0);
        self
    }

    /// Execute the full §4.4 pipeline against `graph`.
    ///
    /// The plan never mutates the store; all stages are read-only.
    /// `graph` is borrowed `dyn` so callers can pass any `GraphRead`
    /// impl (production = `SqliteGraphStore`; tests = in-memory stub).
    pub fn execute(
        &self,
        mut inputs: AbstractPlanInputs<'_>,
        graph: &dyn GraphRead,
    ) -> AbstractPlanResult {
        let started = Instant::now();

        // -------- Step 1 — topic candidate recall ---------------------
        // Re-uses `Stage::SeedRecall` because the cost shape mirrors
        // associative seed recall (vector + BM25 over a single table).
        // A dedicated `Stage::TopicRecall` could be added later
        // without breaking compatibility — `Stage` is `#[non_exhaustive]`.
        inputs.budget.begin_stage(Stage::SeedRecall);
        let (hits, status) = self.searcher.search(
            inputs.query,
            inputs.namespace,
            self.k_topics,
        );
        inputs.budget.end_stage();

        if matches!(status, TopicSearchStatus::Cutoff) {
            return AbstractPlanResult {
                candidates: Vec::new(),
                outcome: AbstractOutcome::Cutoff,
                elapsed: started.elapsed(),
            };
        }

        // L5 substrate-empty branch: searcher returned nothing.
        if hits.is_empty() {
            return AbstractPlanResult {
                candidates: Vec::new(),
                outcome: AbstractOutcome::DowngradedL5Unavailable,
                elapsed: started.elapsed(),
            };
        }

        // L5 substrate-empty branch: every hit below the floor. This
        // is the "compiler hasn't run for this domain yet" signal in
        // production (raw similarity scores are very low when the
        // index has no relevant content).
        let any_above_floor = hits.iter().any(|h| h.score >= self.min_topic_score);
        if !any_above_floor {
            return AbstractPlanResult {
                candidates: Vec::new(),
                outcome: AbstractOutcome::DowngradedL5Unavailable,
                elapsed: started.elapsed(),
            };
        }

        // -------- Step 2 — traceability expansion ---------------------
        // Each topic row already carries source_memories +
        // contributing_entities + cluster_weights — no joins needed
        // (design §4.4 step 2, graph-layer §4.1 schema). We use
        // `MemoryLookup` here because the cost shape matches the
        // factual / episodic memory hydration steps (point-lookup by
        // PK).
        inputs.budget.begin_stage(Stage::MemoryLookup);
        let mut candidates: Vec<AbstractCandidate> = Vec::with_capacity(hits.len());
        for hit in &hits {
            // Skip hits below the floor — they did not earn a row read.
            if hit.score < self.min_topic_score {
                continue;
            }
            // Row read failure or topic missing → drop the candidate
            // silently. Abstract is best-effort by design (§4.4 has
            // no per-step error path back to the caller); a missing
            // topic is a transient race with the compiler / supersede
            // path, not a plan failure.
            let Ok(Some(topic)) = graph.get_topic(hit.topic_id) else {
                continue;
            };
            // Namespace re-check: a searcher backend MAY return rows
            // it filtered correctly server-side, but a buggy stub or
            // an index drift could leak cross-namespace rows. Drop
            // them defensively — namespace isolation is a security
            // surface, not a soft hint.
            if topic.namespace != inputs.namespace {
                continue;
            }
            let source_memories = topic.source_memories.clone();
            let contributing_entities = topic.contributing_entities.clone();
            candidates.push(AbstractCandidate {
                topic,
                topic_score: hit.score,
                source_memories,
                contributing_entities,
            });
        }
        inputs.budget.end_stage();

        // -------- Step 3 — finalize (sort, surface) -------------------
        // Stable, deterministic ordering before fusion (fusion may
        // re-sort by combined score). Sort key:
        //   1. descending topic_score (higher relevance = better)
        //   2. ascending topic_id     (deterministic tiebreak)
        inputs.budget.begin_stage(Stage::Scoring);
        candidates.sort_by(|a, b| {
            b.topic_score
                .partial_cmp(&a.topic_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.topic.topic_id.cmp(&b.topic.topic_id))
        });
        inputs.budget.end_stage();

        let outcome = if candidates.is_empty() {
            AbstractOutcome::Empty
        } else {
            AbstractOutcome::Ok
        };

        AbstractPlanResult {
            candidates,
            outcome,
            elapsed: started.elapsed(),
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        edge::{ConfidenceSource, ResolutionMethod},
        schema::{CanonicalPredicate, Predicate},
        store::{
            CandidateMatch, CandidateQuery, EntityMentions, PipelineRunRow,
            ProposedPredicateStats,
        },
        Edge, EdgeEnd, Entity, EntityKind, GraphError,
    };
    use chrono::{DateTime, Utc};
    use serde_json::json;
    use std::collections::HashMap;

    // ----- Fixture ----------------------------------------------------------

    /// In-memory `GraphRead` stub. Only `get_topic` is functional; the
    /// other read methods panic — calling them in the abstract plan
    /// would be a regression we want to be loud.
    #[derive(Default)]
    struct FakeGraph {
        topics: HashMap<Uuid, KnowledgeTopic>,
    }

    impl FakeGraph {
        fn add_topic(&mut self, t: KnowledgeTopic) {
            self.topics.insert(t.topic_id, t);
        }
    }

    fn fresh_topic(
        ns: &str,
        title: &str,
        sources: Vec<&str>,
        contributing: Vec<Uuid>,
        cluster_weights: Option<serde_json::Value>,
    ) -> KnowledgeTopic {
        let mut t = KnowledgeTopic::new(
            Uuid::new_v4(),
            title.to_string(),
            format!("{title} summary"),
            ns.to_string(),
            1.0,
        );
        t.source_memories = sources.into_iter().map(|s| s.to_string()).collect();
        t.contributing_entities = contributing;
        t.cluster_weights = cluster_weights;
        t
    }

    impl GraphRead for FakeGraph {
        fn get_topic(
            &self,
            id: Uuid,
        ) -> Result<Option<KnowledgeTopic>, GraphError> {
            Ok(self.topics.get(&id).cloned())
        }

        // ---- Unused by AbstractPlan — panic on accidental reach -------
        fn get_entity(&self, _id: Uuid) -> Result<Option<Entity>, GraphError> {
            unimplemented!("not used by AbstractPlan")
        }
        fn list_entities_by_kind(
            &self,
            _k: &EntityKind,
            _l: usize,
        ) -> Result<Vec<Entity>, GraphError> {
            unimplemented!()
        }
        fn search_candidates(
            &self,
            _q: &CandidateQuery,
        ) -> Result<Vec<CandidateMatch>, GraphError> {
            unimplemented!()
        }
        fn resolve_alias(
            &self,
            _n: &str,
        ) -> Result<Option<Uuid>, GraphError> {
            unimplemented!()
        }
        fn get_edge(&self, _id: Uuid) -> Result<Option<Edge>, GraphError> {
            unimplemented!()
        }
        fn find_edges(
            &self,
            _s: Uuid,
            _p: &Predicate,
            _o: Option<&EdgeEnd>,
            _v: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn edges_of(
            &self,
            _s: Uuid,
            _p: Option<&Predicate>,
            _i: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn edges_as_of(
            &self,
            _s: Uuid,
            _at: DateTime<Utc>,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn traverse(
            &self,
            _s: Uuid,
            _md: usize,
            _mr: usize,
            _pf: &[Predicate],
        ) -> Result<Vec<(Uuid, Edge)>, GraphError> {
            unimplemented!()
        }
        fn entities_in_episode(
            &self,
            _e: Uuid,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn edges_in_episode(
            &self,
            _e: Uuid,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn mentions_of_entity(
            &self,
            _e: Uuid,
        ) -> Result<EntityMentions, GraphError> {
            unimplemented!()
        }
        fn entities_linked_to_memory(
            &self,
            _m: &str,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn memories_mentioning_entity(
            &self,
            _e: Uuid,
            _l: usize,
        ) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
        fn edges_sourced_from_memory(
            &self,
            _m: &str,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn list_topics(
            &self,
            _ns: &str,
            _is: bool,
            _l: usize,
        ) -> Result<Vec<KnowledgeTopic>, GraphError> {
            unimplemented!("AbstractPlan reads topics by id, not list_topics")
        }
        fn latest_pipeline_run_for_memory(
            &self,
            _m: &str,
        ) -> Result<Option<PipelineRunRow>, GraphError> {
            unimplemented!()
        }
        fn list_proposed_predicates(
            &self,
            _mu: u64,
        ) -> Result<Vec<ProposedPredicateStats>, GraphError> {
            unimplemented!()
        }
        fn list_failed_episodes(
            &self,
            _u: bool,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn list_namespaces(&self) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
    }

    /// Deterministic topic searcher — returns a fixed list regardless
    /// of `query`.
    struct StubSearcher {
        hits: Vec<TopicHit>,
        status: TopicSearchStatus,
    }

    impl TopicSearcher for StubSearcher {
        fn search(
            &self,
            _q: &GraphQuery,
            _ns: &str,
            _k: usize,
        ) -> (Vec<TopicHit>, TopicSearchStatus) {
            (self.hits.clone(), self.status.clone())
        }
    }

    // Suppress unused-import warning when only one fixture variant is
    // exercised — Edge / EdgeEnd / etc. are imported to satisfy the
    // GraphRead trait stubs above and aren't constructed in tests.
    #[allow(dead_code)]
    fn _silence(
        _e: Edge,
        _ee: EdgeEnd,
        _cs: ConfidenceSource,
        _rm: ResolutionMethod,
        _cp: CanonicalPredicate,
    ) {
    }

    fn budget() -> BudgetController {
        BudgetController::with_defaults()
    }
    fn query() -> GraphQuery {
        GraphQuery::new("anything")
    }

    // ----- Unit tests -------------------------------------------------------

    #[test]
    fn empty_searcher_downgrades_to_l5_unavailable() {
        let plan = AbstractPlan::default();
        let graph = FakeGraph::default();
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::DowngradedL5Unavailable);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn cutoff_propagates_from_searcher_backend() {
        let searcher = StubSearcher {
            hits: vec![],
            status: TopicSearchStatus::Cutoff,
        };
        let plan = AbstractPlan::new(searcher);
        let graph = FakeGraph::default();
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::Cutoff);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn happy_path_returns_topics_with_provenance_and_cluster_weights() {
        let mut graph = FakeGraph::default();
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        let cw = json!({"cluster_a": 0.6, "cluster_b": 0.4});
        let topic_a = fresh_topic(
            "default",
            "Topic A",
            vec!["m1", "m2"],
            vec![e1, e2],
            Some(cw.clone()),
        );
        let topic_b = fresh_topic(
            "default",
            "Topic B",
            vec!["m3"],
            vec![e1],
            None,
        );
        let id_a = topic_a.topic_id;
        let id_b = topic_b.topic_id;
        graph.add_topic(topic_a);
        graph.add_topic(topic_b);

        let searcher = StubSearcher {
            hits: vec![
                TopicHit { topic_id: id_a, score: 0.9 },
                TopicHit { topic_id: id_b, score: 0.7 },
            ],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::Ok);
        assert_eq!(res.candidates.len(), 2);

        // Order: descending topic_score → A (0.9) before B (0.7).
        assert_eq!(res.candidates[0].topic.topic_id, id_a);
        assert!((res.candidates[0].topic_score - 0.9).abs() < 1e-9);
        assert_eq!(res.candidates[0].source_memories, vec!["m1", "m2"]);
        assert_eq!(res.candidates[0].contributing_entities, vec![e1, e2]);
        // GOAL-3.7: cluster_weights respected (forwarded unchanged).
        assert_eq!(res.candidates[0].topic.cluster_weights, Some(cw));

        assert_eq!(res.candidates[1].topic.topic_id, id_b);
        assert!(res.candidates[1].topic.cluster_weights.is_none());
    }

    #[test]
    fn min_topic_score_floor_downgrades_when_all_below() {
        let mut graph = FakeGraph::default();
        let topic = fresh_topic("default", "Weak", vec!["m1"], vec![], None);
        let id = topic.topic_id;
        graph.add_topic(topic);

        let searcher = StubSearcher {
            hits: vec![TopicHit { topic_id: id, score: 0.1 }],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher).with_min_topic_score(0.5);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        // All candidates below floor → downgrade.
        assert_eq!(res.outcome, AbstractOutcome::DowngradedL5Unavailable);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn min_topic_score_floor_keeps_above_drops_below() {
        let mut graph = FakeGraph::default();
        let high = fresh_topic("default", "High", vec!["m1"], vec![], None);
        let low = fresh_topic("default", "Low", vec!["m2"], vec![], None);
        let id_high = high.topic_id;
        let id_low = low.topic_id;
        graph.add_topic(high);
        graph.add_topic(low);

        let searcher = StubSearcher {
            hits: vec![
                TopicHit { topic_id: id_high, score: 0.8 },
                TopicHit { topic_id: id_low, score: 0.2 },
            ],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher).with_min_topic_score(0.5);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::Ok);
        assert_eq!(res.candidates.len(), 1);
        assert_eq!(res.candidates[0].topic.topic_id, id_high);
    }

    #[test]
    fn missing_topic_row_drops_candidate_silently() {
        let mut graph = FakeGraph::default();
        let topic = fresh_topic("default", "Real", vec!["m1"], vec![], None);
        let real_id = topic.topic_id;
        let phantom_id = Uuid::new_v4();
        graph.add_topic(topic);

        let searcher = StubSearcher {
            hits: vec![
                TopicHit { topic_id: phantom_id, score: 0.95 }, // missing
                TopicHit { topic_id: real_id, score: 0.6 },
            ],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        // Only the real one survives; outcome remains Ok because at
        // least one candidate hydrated.
        assert_eq!(res.outcome, AbstractOutcome::Ok);
        assert_eq!(res.candidates.len(), 1);
        assert_eq!(res.candidates[0].topic.topic_id, real_id);
    }

    #[test]
    fn all_topics_missing_emits_empty_not_downgrade() {
        // Searcher returned hits, but every row is gone (transient
        // race with supersede). Distinct from `DowngradedL5Unavailable`
        // because the substrate exists.
        let graph = FakeGraph::default();
        let searcher = StubSearcher {
            hits: vec![TopicHit {
                topic_id: Uuid::new_v4(),
                score: 0.9,
            }],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::Empty);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn cross_namespace_topics_are_dropped_defensively() {
        // Buggy searcher returns a row from a different namespace.
        // Plan must not leak it into the result — namespace isolation
        // is a security surface.
        let mut graph = FakeGraph::default();
        let other_ns = fresh_topic("other", "Leak", vec!["m1"], vec![], None);
        let leak_id = other_ns.topic_id;
        graph.add_topic(other_ns);

        let searcher = StubSearcher {
            hits: vec![TopicHit { topic_id: leak_id, score: 0.99 }],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        // Empty (substrate exists but candidate dropped on
        // namespace recheck).
        assert_eq!(res.outcome, AbstractOutcome::Empty);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn deterministic_tiebreak_on_equal_scores() {
        let mut graph = FakeGraph::default();
        // Construct two topics with deterministic UUIDs so we can
        // assert ordering.
        let mut a = fresh_topic("default", "A", vec![], vec![], None);
        let mut b = fresh_topic("default", "B", vec![], vec![], None);
        a.topic_id = Uuid::from_u128(1);
        b.topic_id = Uuid::from_u128(2);
        graph.add_topic(a.clone());
        graph.add_topic(b.clone());

        let searcher = StubSearcher {
            hits: vec![
                TopicHit { topic_id: b.topic_id, score: 0.5 },
                TopicHit { topic_id: a.topic_id, score: 0.5 },
            ],
            status: TopicSearchStatus::Ok,
        };
        let plan = AbstractPlan::new(searcher);
        let res = plan.execute(
            AbstractPlanInputs {
                query: &query(),
                namespace: "default",
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AbstractOutcome::Ok);
        // Equal score → ascending topic_id → A (1) before B (2).
        assert_eq!(res.candidates[0].topic.topic_id, a.topic_id);
        assert_eq!(res.candidates[1].topic.topic_id, b.topic_id);
    }

    #[test]
    fn k_topics_clamp_to_at_least_one() {
        let plan = AbstractPlan::default().with_k_topics(0);
        assert_eq!(plan.k_topics, 1);
    }

    #[test]
    fn min_topic_score_clamps_to_unit_interval() {
        let p1 = AbstractPlan::default().with_min_topic_score(-0.5);
        assert!((p1.min_topic_score - 0.0).abs() < 1e-9);
        let p2 = AbstractPlan::default().with_min_topic_score(1.5);
        assert!((p2.min_topic_score - 1.0).abs() < 1e-9);
    }
}
