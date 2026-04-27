//! # Associative plan (`task:retr-impl-associative`)
//!
//! Free-form spread-activation retrieval. Implements design **§4.3**
//! (`.gid/features/v03-retrieval/design.md`) — extends the existing
//! `recall_associated` semantics with **edge-hop traversal** from the
//! top-K seed results.
//!
//! ## Pipeline (mirrors design §4.3 numbering)
//!
//! 1. **Seed recall** — `hybrid_recall(query, k=K_seed)`, default
//!    `K_seed = 10`. Produced by an injected
//!    [`SeedRecaller`] trait so unit tests can stub the hybrid path.
//! 2. **Extract seed entities** — for each seed memory, call
//!    [`GraphRead::entities_linked_to_memory`] (graph layer §4.2).
//! 3. **Edge-hop expansion** — fetch 1-hop edges per seed entity via
//!    [`GraphRead::edges_of`], honouring
//!    [`GraphQuery::min_confidence`]. Pool capped at `K_pool`
//!    (default 100). Connected entities feed back through
//!    [`GraphRead::memories_mentioning_entity`] to recover candidate
//!    memories at edge distance 1.
//! 4. **Spread-activation scoring** — emitted as unscored rows for the
//!    fusion module (§5.2). The plan tags each row with the minimum
//!    edge distance reached; fusion combines `seed_score
//!    × edge_distance_decay × actr_activation`. Scoring weights live
//!    in fusion (§5.2 — `0.40·seed + 0.35·edge_distance + 0.25·actr`),
//!    not here.
//! 5. **Deduplication** — when the same memory is reachable via
//!    multiple paths, the plan keeps the **minimum** edge distance
//!    (equivalent to the **max** spread-activation contribution per
//!    §4.3 step 5; min-distance is the canonical pre-fusion form).
//! 6. **Fusion** — handed off to §5.2 (out of scope here).
//!
//! ## Outcome shape (§6.4)
//!
//! - [`AssociativeOutcome::Ok`] — at least one candidate.
//! - [`AssociativeOutcome::Empty`] — pipeline ran but produced no
//!   memories (seeds had no entities, expansion found nothing).
//! - [`AssociativeOutcome::DowngradedNoSeeds`] — seed recall returned
//!   nothing; fallback path is the caller's concern (§3.4).
//! - [`AssociativeOutcome::Cutoff`] — reserved for the future
//!   knowledge-cutoff gate; emitted today only when the seed recaller
//!   itself signals a cutoff.
//!
//! Cross-references: [`super::factual`], [`super::episodic`].

use std::collections::HashMap;
use std::time::Instant;

use crate::graph::store::GraphRead;
use crate::graph::EdgeEnd;
use crate::retrieval::api::{EntityId, GraphQuery};
use crate::retrieval::budget::{BudgetController, Stage};
use crate::store_api::MemoryId;

// ---------------------------------------------------------------------------
// 1. Constants & defaults
// ---------------------------------------------------------------------------

/// Default seed-recall fanout (`K_seed`, design §4.3 step 1).
pub const DEFAULT_K_SEED: usize = 10;

/// Default expansion pool cap (`K_pool`, design §4.3 step 3). Pool size
/// counts unique candidate memories, including seeds themselves.
pub const DEFAULT_K_POOL: usize = 100;

/// Hard cap on memories pulled via `memories_mentioning_entity` per
/// expanded entity. Prevents a single hub entity from saturating the
/// pool.
const PER_ENTITY_MEMORY_CAP: usize = 16;

// ---------------------------------------------------------------------------
// 2. SeedRecaller trait + NullSeedRecaller
// ---------------------------------------------------------------------------

/// Output of [`SeedRecaller::recall`] — one seed memory plus the score
/// that surfaced it. The plan does not reinterpret `score`; it is
/// forwarded as the `seed_score` signal to fusion (§5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SeedHit {
    pub memory_id: MemoryId,
    pub score: f64,
}

/// Status returned by a seed recaller. `Cutoff` lets the seed backend
/// surface the knowledge-cutoff gate without the plan having to know
/// the storage clock — keeps GUARD-2 (§6.4) clean.
#[derive(Debug, Clone, PartialEq)]
pub enum SeedRecallStatus {
    /// Hits returned (possibly empty); plan handles `Empty` /
    /// `DowngradedNoSeeds` itself.
    Ok,
    /// Backend declared the query strictly outside the cutoff window.
    Cutoff,
}

/// Hybrid-recall seed source (design §4.3 step 1). The default v0.3
/// implementation wraps the engramai `hybrid_recall` path, but plans
/// take the trait so unit tests can swap in deterministic stubs.
pub trait SeedRecaller {
    fn recall(
        &self,
        query: &GraphQuery,
        k_seed: usize,
    ) -> (Vec<SeedHit>, SeedRecallStatus);
}

/// Inert default — used in unit tests / when the hybrid path is absent.
/// Always returns an empty seed set with `Ok` status; the plan surfaces
/// [`AssociativeOutcome::DowngradedNoSeeds`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSeedRecaller;

impl SeedRecaller for NullSeedRecaller {
    fn recall(
        &self,
        _query: &GraphQuery,
        _k_seed: usize,
    ) -> (Vec<SeedHit>, SeedRecallStatus) {
        (Vec::new(), SeedRecallStatus::Ok)
    }
}

// ---------------------------------------------------------------------------
// 3. AssociativeCandidate (output row)
// ---------------------------------------------------------------------------

/// Pre-fusion candidate row produced by the Associative plan. The
/// fusion module (§5.2) consumes the tuple
/// `(seed_score, edge_distance, …)` to compute the final score.
///
/// `edge_distance` is the **minimum** hop count from any seed memory:
/// `0` for seeds themselves, `1` for memories reached by a single
/// `entity → 1-hop edge → entity` traversal. The plan dedupes by
/// keeping the smallest `edge_distance` per `memory_id` (design §4.3
/// step 5). Larger distances are pruned, not summed — sum would bias
/// scoring toward hubs.
#[derive(Debug, Clone, PartialEq)]
pub struct AssociativeCandidate {
    pub memory_id: MemoryId,
    /// Best seed score that reached this memory (max over reaching
    /// seeds — the most-confident path wins, mirroring §4.3 step 5's
    /// "max not sum" rule, but applied to seed scores). Forwarded as
    /// the `seed_score` signal in §5.1.
    pub seed_score: f64,
    /// Minimum number of edge hops from any seed (0 = seed itself).
    pub edge_distance: u8,
    /// Entity that bridged the seed and this memory, if any. `None`
    /// for the seed row itself. Surfaced for trace / debugging — not
    /// consumed by fusion in v0.3.
    pub via_entity: Option<EntityId>,
}

// ---------------------------------------------------------------------------
// 4. Inputs / outputs / outcome
// ---------------------------------------------------------------------------

/// Inputs assembled by the dispatcher before invoking
/// [`AssociativePlan::execute`].
///
/// Mirrors [`super::episodic::EpisodicPlanInputs`] in shape; the plan
/// pulls its temporal anchor straight off `query` rather than via a
/// separate resolved window (associative recall is timeless by
/// default — §4.3 has no time-window step).
pub struct AssociativePlanInputs<'a> {
    /// Original query — surfaces `min_confidence`, `limit`, optional
    /// `entity_filter` (used as a post-filter, not a hard gate).
    pub query: &'a GraphQuery,

    /// Per-stage cost controller. Plan never panics on exhaustion — it
    /// short-circuits with whatever it has so far.
    pub budget: BudgetController,
}

/// Typed outcome (§6.4). The plan never returns scored rows; fusion
/// owns scoring (§5.2).
#[derive(Debug, Clone, PartialEq)]
pub enum AssociativeOutcome {
    /// Pipeline produced ≥ 1 candidate (could be just the seeds).
    Ok,
    /// Pipeline ran end-to-end but no candidates survived (no seeds
    /// reached, entity expansion empty, etc.).
    Empty,
    /// Seed recaller returned zero hits — the caller may degrade
    /// further (§3.4); Associative itself cannot continue without
    /// seeds.
    DowngradedNoSeeds,
    /// Seed backend signalled a knowledge-cutoff gate.
    Cutoff,
}

/// Plan output. `candidates` is **unscored** — see
/// [`AssociativeCandidate::seed_score`] / `edge_distance`. Fusion
/// (§5.2) reranks and applies `0.40·seed + 0.35·edge_distance +
/// 0.25·actr`.
#[derive(Debug, Clone)]
pub struct AssociativePlanResult {
    pub candidates: Vec<AssociativeCandidate>,
    pub outcome: AssociativeOutcome,
    pub elapsed: std::time::Duration,
}

// ---------------------------------------------------------------------------
// 5. AssociativePlan struct + execute()
// ---------------------------------------------------------------------------

/// Spread-activation associative plan. Generic over the
/// [`SeedRecaller`] backend so the v0.3 pipeline can wire the real
/// `hybrid_recall` while tests stub it. The `GraphRead` dependency is
/// dyn-borrowed at `execute` time so a single plan instance can be
/// reused across queries hitting different graph stores.
#[derive(Debug, Clone)]
pub struct AssociativePlan<R = NullSeedRecaller>
where
    R: SeedRecaller,
{
    seeds: R,
    /// `K_seed` (design §4.3 step 1).
    pub k_seed: usize,
    /// `K_pool` (design §4.3 step 3).
    pub k_pool: usize,
}

impl Default for AssociativePlan<NullSeedRecaller> {
    fn default() -> Self {
        Self {
            seeds: NullSeedRecaller,
            k_seed: DEFAULT_K_SEED,
            k_pool: DEFAULT_K_POOL,
        }
    }
}

impl<R> AssociativePlan<R>
where
    R: SeedRecaller,
{
    pub fn new(seeds: R) -> Self {
        Self {
            seeds,
            k_seed: DEFAULT_K_SEED,
            k_pool: DEFAULT_K_POOL,
        }
    }

    /// Override `K_seed` (design §4.3 step 1). Values < 1 are clamped
    /// to 1 — a zero-seed run is meaningless and would always
    /// downgrade.
    pub fn with_k_seed(mut self, k: usize) -> Self {
        self.k_seed = k.max(1);
        self
    }

    /// Override `K_pool` (design §4.3 step 3). Values < 1 are clamped
    /// to 1.
    pub fn with_k_pool(mut self, k: usize) -> Self {
        self.k_pool = k.max(1);
        self
    }

    /// Execute the full §4.3 pipeline against `graph`.
    ///
    /// `graph` is borrowed `dyn` so callers can pass any `GraphRead`
    /// impl (production = `SqliteGraphStore`; tests = in-memory stub).
    /// The plan never mutates the store; all stages are read-only.
    pub fn execute(
        &self,
        mut inputs: AssociativePlanInputs<'_>,
        graph: &dyn GraphRead,
    ) -> AssociativePlanResult {
        let started = Instant::now();
        let min_conf = inputs.query.min_confidence.unwrap_or(0.0);

        // -------- Step 1 — seed recall ---------------------------------
        inputs.budget.begin_stage(Stage::SeedRecall);
        let (seeds, seed_status) = self.seeds.recall(inputs.query, self.k_seed);
        inputs.budget.end_stage();

        if matches!(seed_status, SeedRecallStatus::Cutoff) {
            return AssociativePlanResult {
                candidates: Vec::new(),
                outcome: AssociativeOutcome::Cutoff,
                elapsed: started.elapsed(),
            };
        }
        if seeds.is_empty() {
            return AssociativePlanResult {
                candidates: Vec::new(),
                outcome: AssociativeOutcome::DowngradedNoSeeds,
                elapsed: started.elapsed(),
            };
        }

        // Pool: memory_id -> (best seed_score, min edge_distance,
        //                    via_entity for the min-distance reach).
        // Insertion order is preserved by HashMap iteration only
        // unintentionally; we re-sort on output, so it does not
        // matter.
        let mut pool: HashMap<MemoryId, AssociativeCandidate> =
            HashMap::with_capacity(self.k_pool);

        // Seed rows enter at distance 0.
        for hit in &seeds {
            upsert_candidate(
                &mut pool,
                hit.memory_id.clone(),
                hit.score,
                0,
                None,
                self.k_pool,
            );
        }

        // -------- Step 2 — extract seed entities -----------------------
        inputs.budget.begin_stage(Stage::EntityExtract);
        // Map seed entity → best seed_score that surfaced it. When a
        // hub entity comes from multiple seeds we keep the max
        // (mirrors §4.3 step 5's "max not sum" rule applied to seeds).
        let mut seed_entities: HashMap<EntityId, f64> = HashMap::new();
        for hit in &seeds {
            // `entities_linked_to_memory` is a cheap join lookup; on
            // error we skip the seed rather than failing the whole
            // plan (associative is best-effort by design — §4.3).
            let Ok(entity_ids) =
                graph.entities_linked_to_memory(&hit.memory_id)
            else {
                continue;
            };
            for ent in entity_ids {
                seed_entities
                    .entry(ent)
                    .and_modify(|s| {
                        if hit.score > *s {
                            *s = hit.score;
                        }
                    })
                    .or_insert(hit.score);
            }
        }
        inputs.budget.end_stage();

        // -------- Step 3 — 1-hop edge expansion ------------------------
        inputs.budget.begin_stage(Stage::EdgeHop);
        // `expanded_entities` collects the **objects** of 1-hop edges,
        // tagged with the best seed_score reaching them through the
        // bridge entity.
        let mut expanded_entities: HashMap<EntityId, (f64, EntityId)> =
            HashMap::new();
        for (subj_entity, seed_score) in &seed_entities {
            if pool.len() >= self.k_pool {
                break;
            }
            // `include_invalidated = false` mirrors the default
            // bi-temporal projection (design §4.6). Associative does
            // not opt into superseded edges in v0.3.
            let Ok(edges) = graph.edges_of(*subj_entity, None, false) else {
                continue;
            };
            for edge in edges {
                if edge.confidence < min_conf {
                    continue;
                }
                // Only traverse entity↔entity edges; literal-valued
                // attribute edges have no memory provenance to
                // expand into.
                let EdgeEnd::Entity { id: obj } = edge.object else {
                    continue;
                };
                // Skip the trivial self-loop and the back-edge to a
                // seed entity — both add no information.
                if obj == *subj_entity || seed_entities.contains_key(&obj) {
                    continue;
                }
                expanded_entities
                    .entry(obj)
                    .and_modify(|(best, _via)| {
                        if *seed_score > *best {
                            *best = *seed_score;
                            *_via = *subj_entity;
                        }
                    })
                    .or_insert((*seed_score, *subj_entity));
            }
        }
        inputs.budget.end_stage();

        // -------- Step 3b — recover memories at distance 1 -------------
        inputs.budget.begin_stage(Stage::MemoryLookup);
        for (entity, (seed_score, via)) in &expanded_entities {
            if pool.len() >= self.k_pool {
                break;
            }
            let Ok(mems) =
                graph.memories_mentioning_entity(*entity, PER_ENTITY_MEMORY_CAP)
            else {
                continue;
            };
            for mid in mems {
                upsert_candidate(
                    &mut pool,
                    mid,
                    *seed_score,
                    1,
                    Some(*via),
                    self.k_pool,
                );
                if pool.len() >= self.k_pool {
                    break;
                }
            }
        }
        inputs.budget.end_stage();

        // -------- Step 5 — finalize (sort, surface) --------------------
        inputs.budget.begin_stage(Stage::Scoring);
        let mut candidates: Vec<AssociativeCandidate> =
            pool.into_values().collect();
        // Stable, deterministic ordering before fusion (fusion may
        // re-sort by combined score). Sort key:
        //   1. ascending edge_distance (closer = better default)
        //   2. descending seed_score    (higher confidence = better)
        //   3. ascending memory_id      (deterministic tiebreak)
        candidates.sort_by(|a, b| {
            a.edge_distance
                .cmp(&b.edge_distance)
                .then_with(|| {
                    b.seed_score
                        .partial_cmp(&a.seed_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.memory_id.cmp(&b.memory_id))
        });
        inputs.budget.end_stage();

        let outcome = if candidates.is_empty() {
            AssociativeOutcome::Empty
        } else {
            AssociativeOutcome::Ok
        };

        AssociativePlanResult {
            candidates,
            outcome,
            elapsed: started.elapsed(),
        }
    }
}

/// Internal helper — insert-or-merge a candidate into the pool, honouring
/// the dedup rule (`min` edge_distance, `max` seed_score). Capacity is
/// enforced *before* insertion of new memories; existing rows always
/// merge regardless of capacity (otherwise the dedup invariant would
/// silently drop better paths to memories already in the pool).
fn upsert_candidate(
    pool: &mut HashMap<MemoryId, AssociativeCandidate>,
    memory_id: MemoryId,
    seed_score: f64,
    edge_distance: u8,
    via_entity: Option<EntityId>,
    cap: usize,
) {
    use std::collections::hash_map::Entry;
    // Capture length BEFORE taking a mutable borrow via `entry` (E0502).
    let pool_len = pool.len();
    match pool.entry(memory_id.clone()) {
        Entry::Occupied(mut e) => {
            let cur = e.get_mut();
            // Min edge_distance, max seed_score — design §4.3 step 5
            // applied per signal (distance and score are independent).
            if edge_distance < cur.edge_distance {
                cur.edge_distance = edge_distance;
                cur.via_entity = via_entity;
            }
            if seed_score > cur.seed_score {
                cur.seed_score = seed_score;
            }
        }
        Entry::Vacant(slot) => {
            if pool_len >= cap {
                return;
            }
            slot.insert(AssociativeCandidate {
                memory_id,
                seed_score,
                edge_distance,
                via_entity,
            });
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
        Edge, EdgeEnd, Entity, EntityKind, GraphError, KnowledgeTopic,
    };
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap as Map;
    use uuid::Uuid;

    // ----- Fixture ----------------------------------------------------------

    /// In-memory `GraphRead` stub. Only the three methods used by the
    /// Associative plan return data; the rest panic via `unimplemented!`
    /// — calling them in tests would mean the plan is reaching beyond
    /// its §4.3 contract and we want the regression to be loud.
    #[derive(Default)]
    struct FakeGraph {
        // memory_id -> entities linked
        mem_to_entities: Map<String, Vec<Uuid>>,
        // subject_entity -> list of edges (object + confidence)
        edges: Map<Uuid, Vec<(Uuid, f64)>>,
        // entity -> memories mentioning it
        entity_to_mems: Map<Uuid, Vec<String>>,
    }

    impl FakeGraph {
        fn link_mem(&mut self, mem: &str, ents: &[Uuid]) {
            self.mem_to_entities
                .insert(mem.to_string(), ents.to_vec());
        }
        fn add_edge(&mut self, subj: Uuid, obj: Uuid, conf: f64) {
            self.edges.entry(subj).or_default().push((obj, conf));
        }
        fn entity_mems(&mut self, ent: Uuid, mems: &[&str]) {
            self.entity_to_mems
                .insert(ent, mems.iter().map(|s| s.to_string()).collect());
        }
    }

    fn fake_edge(subj: Uuid, obj: Uuid, conf: f64) -> Edge {
        Edge {
            id: Uuid::new_v4(),
            subject_id: subj,
            predicate: Predicate::Canonical(CanonicalPredicate::RelatedTo),
            object: EdgeEnd::Entity { id: obj },
            summary: String::new(),
            valid_from: None,
            valid_to: None,
            recorded_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            invalidated_at: None,
            invalidated_by: None,
            supersedes: None,
            episode_id: None,
            memory_id: None,
            resolution_method: ResolutionMethod::Automatic,
            activation: 0.0,
            confidence: conf,
            confidence_source: ConfidenceSource::Recovered,
            agent_affect: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    impl GraphRead for FakeGraph {
        fn entities_linked_to_memory(
            &self,
            memory_id: &str,
        ) -> Result<Vec<Uuid>, GraphError> {
            Ok(self
                .mem_to_entities
                .get(memory_id)
                .cloned()
                .unwrap_or_default())
        }
        fn edges_of(
            &self,
            subject: Uuid,
            _predicate: Option<&Predicate>,
            _include_invalidated: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            Ok(self
                .edges
                .get(&subject)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(obj, c)| fake_edge(subject, obj, c))
                .collect())
        }
        fn memories_mentioning_entity(
            &self,
            entity: Uuid,
            limit: usize,
        ) -> Result<Vec<String>, GraphError> {
            Ok(self
                .entity_to_mems
                .get(&entity)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(limit)
                .collect())
        }

        // ---- Unused by AssociativePlan — panic on accidental reach ----
        fn get_entity(&self, _id: Uuid) -> Result<Option<Entity>, GraphError> {
            unimplemented!("not used by AssociativePlan")
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
        fn edges_as_of(
            &self,
            _s: Uuid,
            _at: chrono::DateTime<Utc>,
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
        fn edges_sourced_from_memory(
            &self,
            _m: &str,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn get_topic(
            &self,
            _id: Uuid,
        ) -> Result<Option<KnowledgeTopic>, GraphError> {
            unimplemented!()
        }
        fn list_topics(
            &self,
            _ns: &str,
            _is: bool,
            _l: usize,
        ) -> Result<Vec<KnowledgeTopic>, GraphError> {
            unimplemented!()
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
            _unresolved_only: bool,
        ) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn list_namespaces(&self) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
    }

    /// Deterministic seed recaller — returns a fixed list regardless of
    /// `query`.
    struct StubSeeds {
        hits: Vec<SeedHit>,
        status: SeedRecallStatus,
    }
    impl SeedRecaller for StubSeeds {
        fn recall(
            &self,
            _q: &GraphQuery,
            _k: usize,
        ) -> (Vec<SeedHit>, SeedRecallStatus) {
            (self.hits.clone(), self.status.clone())
        }
    }

    fn budget() -> BudgetController {
        BudgetController::with_defaults()
    }
    fn query() -> GraphQuery {
        GraphQuery::new("anything")
    }

    // ----- Unit tests -------------------------------------------------------

    #[test]
    fn empty_seeds_downgrades() {
        let plan = AssociativePlan::default();
        let graph = FakeGraph::default();
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AssociativeOutcome::DowngradedNoSeeds);
        assert!(res.candidates.is_empty());
    }

    #[test]
    fn cutoff_propagates_from_seed_backend() {
        let seeds = StubSeeds {
            hits: vec![],
            status: SeedRecallStatus::Cutoff,
        };
        let plan = AssociativePlan::new(seeds);
        let graph = FakeGraph::default();
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AssociativeOutcome::Cutoff);
    }

    #[test]
    fn seeds_only_no_entities_emits_seeds_at_distance_0() {
        // Seeds reach the pool even when their entity provenance is empty
        // — Associative is best-effort (§4.3).
        let seeds = StubSeeds {
            hits: vec![
                SeedHit {
                    memory_id: "m1".into(),
                    score: 0.9,
                },
                SeedHit {
                    memory_id: "m2".into(),
                    score: 0.5,
                },
            ],
            status: SeedRecallStatus::Ok,
        };
        let plan = AssociativePlan::new(seeds);
        let graph = FakeGraph::default();
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AssociativeOutcome::Ok);
        assert_eq!(res.candidates.len(), 2);
        assert!(res.candidates.iter().all(|c| c.edge_distance == 0));
    }

    #[test]
    fn one_hop_expansion_adds_distance_one_memories() {
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e1]);
        graph.add_edge(e1, e2, 0.9);
        graph.entity_mems(e2, &["expanded1", "expanded2"]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let plan = AssociativePlan::new(seeds);
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            &graph,
        );
        assert_eq!(res.outcome, AssociativeOutcome::Ok);
        // 1 seed + 2 expanded
        assert_eq!(res.candidates.len(), 3);
        let by_id: Map<String, &AssociativeCandidate> =
            res.candidates.iter().map(|c| (c.memory_id.clone(), c)).collect();
        assert_eq!(by_id["seed1"].edge_distance, 0);
        assert_eq!(by_id["expanded1"].edge_distance, 1);
        assert_eq!(by_id["expanded2"].edge_distance, 1);
        assert_eq!(by_id["expanded1"].via_entity, Some(e1));
    }

    #[test]
    fn min_confidence_filters_low_edges() {
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e1]);
        graph.add_edge(e1, e2, 0.3); // below threshold
        graph.entity_mems(e2, &["expanded1"]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let plan = AssociativePlan::new(seeds);
        let mut q = query();
        q.min_confidence = Some(0.5);
        let res = plan.execute(
            AssociativePlanInputs {
                query: &q,
                budget: budget(),
            },
            &graph,
        );
        // Edge filtered out, only the seed survives.
        assert_eq!(res.candidates.len(), 1);
        assert_eq!(res.candidates[0].memory_id, "seed1");
    }

    #[test]
    fn dedup_keeps_min_distance_and_max_seed_score() {
        // Memory `dup` is reachable both as a seed (distance 0,
        // score 0.4) AND via 1-hop expansion from a higher-score seed
        // (distance 1, score 0.95 reachable via e1 → e2 → dup).
        // Dedup rule: min distance (0 wins) AND max seed_score
        // (0.95 wins) — independently per signal (§4.3 step 5).
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("hub", &[e1]);
        graph.add_edge(e1, e2, 0.9);
        graph.entity_mems(e2, &["dup"]);

        let seeds = StubSeeds {
            hits: vec![
                SeedHit {
                    memory_id: "hub".into(),
                    score: 0.95,
                },
                SeedHit {
                    memory_id: "dup".into(),
                    score: 0.4,
                },
            ],
            status: SeedRecallStatus::Ok,
        };
        let plan = AssociativePlan::new(seeds);
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            &graph,
        );
        let dup = res
            .candidates
            .iter()
            .find(|c| c.memory_id == "dup")
            .unwrap();
        assert_eq!(dup.edge_distance, 0); // min wins (seed beats hop)
        assert!((dup.seed_score - 0.95).abs() < 1e-9); // max wins (hop's 0.95 > seed's 0.4)
    }

    #[test]
    fn k_pool_caps_total_candidates() {
        let e1 = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e1]);
        // hub entity links to many memories
        let mems: Vec<&str> = ["a", "b", "c", "d", "e"].into();
        graph.entity_mems(e1, &mems);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        // K_pool = 3 → seed + at most 2 expanded
        let plan = AssociativePlan::new(seeds).with_k_pool(3);
        let res = plan.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
            },
            // Need at least one outgoing edge to trigger expansion.
            // Without an edge, expanded_entities is empty and only the
            // seed lands in the pool.
            &{
                let mut g = graph;
                let e2 = Uuid::new_v4();
                g.add_edge(e1, e2, 0.9);
                g.entity_mems(e2, &mems);
                g
            },
        );
        assert!(res.candidates.len() <= 3);
    }
}
