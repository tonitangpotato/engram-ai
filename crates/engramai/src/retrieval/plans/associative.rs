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
use crate::retrieval::plans::factual::EntityResolver;
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

// Blanket impl: `&T: SeedRecaller` whenever `T: SeedRecaller`. Required by
// the orchestrator's `PlanCollaborators` (ISS-049 phase 2), which holds
// `&dyn SeedRecaller` and feeds it as the type parameter on
// `AssociativePlan<&dyn _>`.
impl<T> SeedRecaller for &T
where
    T: SeedRecaller + ?Sized,
{
    fn recall(
        &self,
        query: &GraphQuery,
        k_seed: usize,
    ) -> (Vec<SeedHit>, SeedRecallStatus) {
        (**self).recall(query, k_seed)
    }
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

    /// ISS-164 — always-on entity channel toggle.
    ///
    /// When `true` AND `entity_resolver` is `Some`, the plan calls
    /// `EntityResolver::resolve(query.text)` during Step 2 (extract
    /// seed entities) and unions the resolved anchor entities into
    /// `seed_entities` before the 1-hop edge expansion. When
    /// `false`, the plan executes the §4.3 pipeline byte-identically
    /// (no resolver call, no `seed_entities` injection).
    ///
    /// Defaults to `false` via the orchestrator — flipped on by
    /// callers that opt in via
    /// `GraphQuery::entity_channel_override` or
    /// `FusionConfig::entity_channel_enabled`.
    pub entity_channel_enabled: bool,

    /// ISS-164 — entity resolver borrowed from `PlanCollaborators`.
    ///
    /// `Some` when the orchestrator wires the real
    /// `GraphEntityResolver`; `None` when the plan is invoked from
    /// unit tests that do not exercise the entity channel. The plan
    /// only calls `.resolve` when both this is `Some` AND
    /// `entity_channel_enabled` is `true` — either gate being off
    /// preserves byte-identity with the pre-ISS-164 pipeline.
    pub entity_resolver: Option<&'a dyn EntityResolver>,
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

        // -------- Step 2b — ISS-164 always-on entity channel -----------
        // When opted in (Q.entity_channel_override or
        // FusionConfig.entity_channel_enabled), resolve the raw query
        // text to entity anchors. The resolved anchors flow into the
        // pipeline through TWO channels (mirrors Factual §4.1):
        //
        //   1. **Direct memory recovery** at edge distance 1 — same
        //      semantics as Factual's
        //      `memories_mentioning_entity(anchor.entity_id, ...)`
        //      step. This is the high-value path: the gold-fact
        //      memory typically mentions the anchor entity directly
        //      (e.g. "Caroline" → caroline_fact_mem), not via an
        //      intermediate edge hop.
        //   2. **Edge-hop bridging** — by unioning the anchor into
        //      `seed_entities`, Step 3 also fans out through any
        //      1-hop edges from the anchor, recovering memories
        //      mentioning related entities. Cheap extra signal.
        //
        // Byte-identity contract: when `entity_channel_enabled =
        // false` OR `entity_resolver = None` OR resolver returns 0
        // anchors, neither channel fires and the plan behaves
        // byte-identically to the pre-ISS-164 §4.3 pipeline. Tests
        // pin all three preservation cases.
        //
        // `seed_score` for injected anchors uses
        // `ResolvedAnchor.match_strength` (f32 → f64): 1.0 exact /
        // 0.8 alias / 0.5 fuzzy per `factual.rs` convention. Merge
        // policy matches the seeds-derived loop above — keep the
        // **max** when an entity surfaces from both channels (§4.3
        // step 5 "max not sum").
        //
        // `injected_anchors` is the subset of `seed_entities` that
        // came from the resolver (not from seed memories). Step 3b
        // uses it to recover the anchor's mentioned memories
        // directly, in addition to whatever Step 3 expansion finds.
        let mut injected_anchors: HashMap<EntityId, f64> = HashMap::new();
        if inputs.entity_channel_enabled {
            if let Some(resolver) = inputs.entity_resolver {
                // Resolver returns an empty Vec on error / no match;
                // the trait does not surface failures (factual.rs:114).
                // Either way the loop below is a no-op when empty,
                // preserving byte-identity with the pre-ISS-164 path.
                let mut anchors = resolver.resolve(&inputs.query.text);
                // ISS-164 self-review (2026-05-26): mirror Factual's
                // `min_confidence` filter (factual.rs:408). Without
                // this, fuzzy `match_strength = 0.5` anchors would
                // injection-flood the channel — every weak anchor
                // burns one slot of `PER_ENTITY_MEMORY_CAP=16` in
                // Step 3b' regardless of relevance. The cast is
                // `f32 ← f64` to match `ResolvedAnchor.match_strength`
                // and is identical to how Factual reads the same field
                // (orchestrator.rs:771 / :1035 / :1410). `None` ⇒ no
                // filter, identical pre-fix behavior; a fast-path
                // skip on `None` avoids touching the Vec.
                if let Some(floor) = inputs.query.min_confidence {
                    let floor = floor as f32;
                    anchors.retain(|a| a.match_strength >= floor);
                }
                for anchor in anchors {
                    let score = anchor.match_strength as f64;
                    seed_entities
                        .entry(anchor.entity_id)
                        .and_modify(|s| {
                            if score > *s {
                                *s = score;
                            }
                        })
                        .or_insert(score);
                    // Track separately so Step 3b can call
                    // `memories_mentioning_entity` directly on the
                    // anchor. We keep the max if the same anchor was
                    // resolved twice with different strengths.
                    injected_anchors
                        .entry(anchor.entity_id)
                        .and_modify(|s| {
                            if score > *s {
                                *s = score;
                            }
                        })
                        .or_insert(score);
                }
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

        // -------- Step 3b' — ISS-164 direct anchor memory recovery -----
        // The injected anchors from Step 2b also surface their OWN
        // mentioned memories at distance 1 — mirrors Factual's
        // `memories_mentioning_entity(anchor.entity_id, ...)` path
        // (factual.rs §4.1). Without this, anchors that have no
        // outgoing 1-hop edges would never contribute candidates,
        // defeating the channel's purpose for proper-noun queries
        // where the gold-fact memory mentions the anchor directly
        // (the documented LoCoMo conv-26 pattern).
        //
        // No-op when `injected_anchors` is empty — i.e., when the
        // channel was off or the resolver returned zero anchors,
        // which preserves byte-identity with the pre-ISS-164
        // pipeline (tests pin both cases).
        //
        // `via` is set to the anchor itself (`Some(*entity)`) to
        // record the provenance — the candidate was reached because
        // the resolver picked this entity from the query text. This
        // mirrors the §4.3 step-3 convention of recording the bridge
        // entity that surfaced the memory.
        for (entity, seed_score) in &injected_anchors {
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
                    Some(*entity),
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
        fn edges_into(
            &self,
            _object: Uuid,
            _predicate: Option<&Predicate>,
            _include_invalidated: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            // Associative plan tests don't exercise incoming-edge traversal.
            Ok(Vec::new())
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
    #[derive(Clone)]
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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
                entity_channel_enabled: false,
                entity_resolver: None,
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

    // ---------------------------------------------------------------
    // ISS-164 — always-on entity channel
    // ---------------------------------------------------------------

    /// Stub [`EntityResolver`] for ISS-164 unit tests. Returns whatever
    /// anchor list it was constructed with, regardless of the query
    /// text — pure-function contract is preserved (deterministic on
    /// the bound `(state, query)`).
    struct StubResolver {
        anchors: Vec<crate::retrieval::plans::factual::ResolvedAnchor>,
    }

    impl crate::retrieval::plans::factual::EntityResolver for StubResolver {
        fn resolve(
            &self,
            _query: &str,
        ) -> Vec<crate::retrieval::plans::factual::ResolvedAnchor> {
            self.anchors.clone()
        }
    }

    /// ISS-164 byte-identity #1 — channel off, resolver wired with
    /// anchors.
    ///
    /// When `entity_channel_enabled = false`, the resolver is never
    /// called and the plan's output is bit-for-bit identical to the
    /// pre-ISS-164 §4.3 pipeline. This is the production default and
    /// the locked-config baseline.
    #[test]
    fn iss164_channel_off_preserves_byte_identity() {
        let e1 = Uuid::new_v4();
        let e_anchor = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e1]);
        graph.add_edge(e1, Uuid::new_v4(), 0.9);
        graph.entity_mems(e_anchor, &["anchor_only_mem"]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let resolver = StubResolver {
            anchors: vec![
                crate::retrieval::plans::factual::ResolvedAnchor {
                    entity_id: e_anchor,
                    canonical_name: "Caroline".into(),
                    match_strength: 1.0,
                },
            ],
        };

        let plan_off = AssociativePlan::new(seeds.clone());
        let res_off = plan_off.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: false,
                entity_resolver: Some(&resolver),
            },
            &graph,
        );

        let plan_baseline = AssociativePlan::new(seeds);
        let res_baseline = plan_baseline.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: false,
                entity_resolver: None,
            },
            &graph,
        );

        // The only differences expected between the two `Result`s are
        // `elapsed` (wall-clock). Candidate set + outcome must match
        // byte-for-byte. Compare the candidate vectors as sets of
        // `(memory_id, edge_distance)` since insertion-order is
        // HashMap-defined.
        let key = |r: &AssociativePlanResult| -> Vec<(MemoryId, u8)> {
            let mut v: Vec<_> = r
                .candidates
                .iter()
                .map(|c| (c.memory_id.clone(), c.edge_distance))
                .collect();
            v.sort();
            v
        };
        assert_eq!(res_off.outcome, res_baseline.outcome);
        assert_eq!(key(&res_off), key(&res_baseline));
        // Critically: `anchor_only_mem` must NOT appear — the
        // resolver was provided but the channel was off.
        assert!(
            !res_off
                .candidates
                .iter()
                .any(|c| c.memory_id.as_str() == "anchor_only_mem"),
            "channel-off plan must not surface anchor-only memories"
        );
    }

    /// ISS-164 byte-identity #2 — channel on, resolver returns zero
    /// anchors.
    ///
    /// This is the LoCoMo "Caroline mentioned no proper noun in the
    /// query" case. Channel is enabled but the resolver finds no
    /// match — the plan must behave identically to the channel-off
    /// path. Confirms the `if !anchors.is_empty()` branch protects
    /// the §5.4 envelope when the resolver is a no-op.
    #[test]
    fn iss164_channel_on_zero_anchors_preserves_byte_identity() {
        let e1 = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e1]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let empty_resolver = StubResolver { anchors: vec![] };

        let plan_on = AssociativePlan::new(seeds.clone());
        let res_on = plan_on.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: true,
                entity_resolver: Some(&empty_resolver),
            },
            &graph,
        );

        let plan_off = AssociativePlan::new(seeds);
        let res_off = plan_off.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: false,
                entity_resolver: None,
            },
            &graph,
        );

        let key = |r: &AssociativePlanResult| -> Vec<(MemoryId, u8)> {
            let mut v: Vec<_> = r
                .candidates
                .iter()
                .map(|c| (c.memory_id.clone(), c.edge_distance))
                .collect();
            v.sort();
            v
        };
        assert_eq!(res_on.outcome, res_off.outcome);
        assert_eq!(key(&res_on), key(&res_off));
    }

    /// ISS-164 effect test — channel on, resolver returns one anchor
    /// not surfaced by the seed memories. The anchor's mentioned
    /// memories must appear in the candidate pool at edge distance 1,
    /// proving the entity channel actually feeds the §4.3 expansion.
    ///
    /// This is the LoCoMo "Caroline who?" case — the query mentions
    /// a person whose mentions are NOT in any seed memory, so the
    /// pre-ISS-164 plan would never expand to them.
    #[test]
    fn iss164_channel_on_anchors_expand_seed_entities() {
        let e_seed = Uuid::new_v4();
        let e_anchor = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[e_seed]);
        // The seed entity has NO edges and NO mentioned memories
        // beyond the seed itself.
        graph.entity_mems(e_seed, &["seed1"]);
        // The anchor entity points at a memory the seeds can NEVER
        // reach via the pre-ISS-164 pipeline.
        graph.entity_mems(e_anchor, &["caroline_fact_mem"]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let resolver = StubResolver {
            anchors: vec![
                crate::retrieval::plans::factual::ResolvedAnchor {
                    entity_id: e_anchor,
                    canonical_name: "Caroline".into(),
                    match_strength: 1.0,
                },
            ],
        };

        // Baseline: channel off — the anchor's memories must NOT
        // appear (the pre-ISS-164 contract).
        let plan_off = AssociativePlan::new(seeds.clone());
        let res_off = plan_off.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: false,
                entity_resolver: Some(&resolver),
            },
            &graph,
        );
        assert!(
            !res_off
                .candidates
                .iter()
                .any(|c| c.memory_id.as_str() == "caroline_fact_mem"),
            "baseline (channel off) must not reach anchor-only memories"
        );

        // Channel on — the anchor's memories MUST appear at edge
        // distance 1 (the §4.3 step 3 distance for entity-discovered
        // memories not at distance 0).
        let plan_on = AssociativePlan::new(seeds);
        let res_on = plan_on.execute(
            AssociativePlanInputs {
                query: &query(),
                budget: budget(),
                entity_channel_enabled: true,
                entity_resolver: Some(&resolver),
            },
            &graph,
        );
        let caroline = res_on
            .candidates
            .iter()
            .find(|c| c.memory_id.as_str() == "caroline_fact_mem")
            .expect("channel-on plan must surface anchor-discovered memories");
        // §4.3 step 3 — entities discovered via the channel feed into
        // `memories_mentioning_entity` at distance 1 (not 0; seeds
        // own distance 0).
        assert_eq!(caroline.edge_distance, 1);
    }

    /// ISS-164 self-review (2026-05-26) — `min_confidence` floor must
    /// drop weak anchors before they reach Step 2b injection.
    ///
    /// Without this filter, fuzzy `match_strength = 0.5` anchors flood
    /// the channel — each weak anchor burns one `PER_ENTITY_MEMORY_CAP`
    /// slot in Step 3b' regardless of relevance. Mirrors Factual's
    /// `factual.rs:408` filter, sourced from `GraphQuery.min_confidence`
    /// (same field; `f64 → f32` cast matches `orchestrator.rs:771`).
    ///
    /// Setup: one strong anchor (1.0) + one weak (0.4). Floor = 0.5.
    /// Expected: only the strong anchor's memory surfaces; the weak
    /// anchor's memory must NOT appear, proving the filter ran before
    /// `memories_mentioning_entity` was called on the weak anchor.
    #[test]
    fn iss164_min_confidence_drops_weak_anchors() {
        let e_strong = Uuid::new_v4();
        let e_weak = Uuid::new_v4();
        let mut graph = FakeGraph::default();
        graph.link_mem("seed1", &[Uuid::new_v4()]);
        graph.entity_mems(e_strong, &["strong_mem"]);
        graph.entity_mems(e_weak, &["weak_mem"]);

        let seeds = StubSeeds {
            hits: vec![SeedHit {
                memory_id: "seed1".into(),
                score: 0.8,
            }],
            status: SeedRecallStatus::Ok,
        };
        let resolver = StubResolver {
            anchors: vec![
                crate::retrieval::plans::factual::ResolvedAnchor {
                    entity_id: e_strong,
                    canonical_name: "Strong".into(),
                    match_strength: 1.0,
                },
                crate::retrieval::plans::factual::ResolvedAnchor {
                    entity_id: e_weak,
                    canonical_name: "Weak".into(),
                    match_strength: 0.4,
                },
            ],
        };

        let mut q = query();
        q.min_confidence = Some(0.5);

        let plan = AssociativePlan::new(seeds);
        let res = plan.execute(
            AssociativePlanInputs {
                query: &q,
                budget: budget(),
                entity_channel_enabled: true,
                entity_resolver: Some(&resolver),
            },
            &graph,
        );

        assert!(
            res.candidates
                .iter()
                .any(|c| c.memory_id.as_str() == "strong_mem"),
            "strong anchor (match_strength=1.0 ≥ floor 0.5) must survive"
        );
        assert!(
            !res.candidates
                .iter()
                .any(|c| c.memory_id.as_str() == "weak_mem"),
            "weak anchor (match_strength=0.4 < floor 0.5) must be dropped \
             before memories_mentioning_entity is called"
        );
    }
}
