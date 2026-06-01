//! # Factual plan (`task:retr-impl-factual-bitemporal`)
//!
//! Entity-anchored, graph-backed retrieval plan. Implements the steps in
//! design **§4.1** (`.gid/features/v03-retrieval/design.md`) and applies
//! the bi-temporal projection from **§4.6** so factual results respect
//! GOAL-3.4 (`as-of-T`) and GOAL-3.5 (superseded edges queryable via
//! opt-in flag) without violating GUARD-3 (supersession never erases).
//!
//! ## Pipeline (design §4.1)
//!
//! 1. **Entity resolution.** Tokenize the query, resolve each token via
//!    [`EntityResolver`] → `Vec<ResolvedAnchor>`.
//! 2. **Anchor validation.** Empty anchors → return
//!    [`FactualOutcome::DowngradedNoEntity`] (caller switches to
//!    Associative; this plan does not run sub-plans itself — that's the
//!    orchestrator's job in `task:retr-impl-graph-query-api`).
//! 3. **1-hop edge traversal.** For each anchor fetch live (or as-of)
//!    edges via [`GraphRead::edges_of`] / [`GraphRead::edges_as_of`].
//!    Apply [`project_edges`] for the `as-of-T` and "include superseded"
//!    cases (design §4.6).
//! 4. **Memory candidate lookup.** Union `{anchors} ∪
//!    {linked_entities}` and look up source memories via
//!    [`GraphRead::memories_mentioning_entity`].
//! 5. **Return** [`FactualPlanResult`] — *unscored*. Fusion / scoring is
//!    `task:retr-impl-fusion`'s responsibility (design §5).
//!
//! ## What this module does NOT do
//!
//! - **No scoring or ranking.** Sub-scores (vector / BM25 / graph /
//!   recency) and final ordering are applied by the fusion module
//!   (`task:retr-impl-fusion`, design §5). This plan is a pure data
//!   collector.
//! - **No Associative fallback execution.** When anchors are empty we
//!   surface [`FactualOutcome::DowngradedNoEntity`] and let the
//!   orchestrator route to `plans::associative` (per design §3.4 the
//!   downgrade lattice goes via [`RetrievalOutcome`], not by changing
//!   intent inside a plan).
//! - **No clock sampling.** `query_time` is injected from the caller
//!   (design §5.4 reproducibility pin); this plan never reads the system
//!   clock.
//! - **No budget enforcement.** [`BudgetController`] is consulted via
//!   `should_cutoff()` between stages so the plan returns partial
//!   results on cutoff (design §7.3, "cutoff returns partial, never
//!   error"); the controller itself owns the timing.
//!
//! ## Design refs / requirements
//!
//! - Design §4.1 (Factual plan), §4.6 (bi-temporal projection)
//! - GOAL-3.3 — factual graph-grounded with provenance + bi-temporal
//! - GOAL-3.4 — as-of-T projection
//! - GOAL-3.5 — superseded edges queryable via opt-in
//! - GUARD-3  — supersession never erases (verified by the
//!   [`AsOfMode::IncludeSuperseded`] branch + a property test in
//!   `task:retr-test-determinism-routing-accuracy`)

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use log::{debug, trace};
use uuid::Uuid;

/// Log target for Factual-plan diagnostics. Enable with
/// `RUST_LOG=engramai::retrieval::factual=trace` (or `=debug` for the
/// per-stage summaries without the per-edge firehose). Kept off the
/// default path: a silent plan is the normal case, and this target only
/// emits when explicitly requested. Added after ISS-189, where the plan
/// dropped the answer episode and no in-plan signal existed to see *why*
/// (anchor set, traversal direction, seed contents, recency truncation).
const LOG_TARGET: &str = "engramai::retrieval::factual";

use crate::graph::edge::{Edge, EdgeEnd};
use crate::graph::error::GraphError;
use crate::graph::schema::Predicate;
use crate::graph::store::GraphRead;
use crate::retrieval::budget::{BudgetController, Stage};
use crate::retrieval::plans::bitemporal::{project_edges, AsOfMode, ProjectedEdge};
use crate::store_api::MemoryId;

// ---------------------------------------------------------------------------
// EntityResolver — the query-token → anchor surface
// ---------------------------------------------------------------------------

/// A single resolved anchor — design §4.1 step 1 output.
///
/// Carries identity + a confidence-like `match_strength` so callers /
/// downstream filters can drop weak fuzzy matches if they want a tighter
/// factual semantic. Values in `[0.0, 1.0]`. The match-strength scale
/// mirrors the classifier-heuristic score for entity signals (§3.2):
/// `Exact = 1.0`, `Alias ≈ 0.8`, `Fuzzy ≈ 0.5`.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAnchor {
    /// Canonical entity ID in the v0.3 graph (matches `graph_entities.id`).
    pub entity_id: Uuid,
    /// Canonical name surfaced to traces / debugging — *never* used as a
    /// key (the graph is keyed by `entity_id`).
    pub canonical_name: String,
    /// Confidence in this resolution, `[0.0, 1.0]`. Sub-`min_confidence`
    /// anchors are dropped before traversal.
    pub match_strength: f32,
}

/// Plugin interface for converting a query string into entity anchors.
///
/// **Pure-function contract.** Implementations must be deterministic over
/// `(store-snapshot, query)` — no clock sampling, no random seeds, no
/// implicit caches that depend on call order. This is what makes Factual
/// reproducible (design §5.4).
///
/// **No `Send + Sync` bound.** The orchestrator holds `&dyn EntityResolver`
/// inside a single synchronous closure (`Memory::with_graph_read`) and
/// never moves it across threads — the Phase-1 doc-comment that said
/// "held inside `Arc<dyn EntityResolver>`" was superseded by the
/// `&dyn` wiring in `PlanCollaborators` (ISS-049 phase 2). Real
/// adapters wrap rusqlite handles which are `!Sync`; requiring
/// `Send + Sync` would force every adapter to add a Mutex layer that
/// is never observably exercised.
///
/// **Output ordering.** Implementations SHOULD return anchors sorted by
/// `match_strength` descending — the plan applies a stable secondary sort
/// on `entity_id` so duplicates are handled deterministically.
pub trait EntityResolver {
    /// Resolve `query` to candidate anchors. Empty result is allowed
    /// (and triggers the §4.1 step 2 downgrade).
    fn resolve(&self, query: &str) -> Vec<ResolvedAnchor>;
}

/// No-op resolver. Useful for tests where the entity store is irrelevant
/// (we exercise the downgrade path) or as a typed placeholder before the
/// graph-backed resolver from `task:retr-impl-classifier-heuristic` is
/// wired in.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEntityResolver;

impl EntityResolver for NullEntityResolver {
    fn resolve(&self, _query: &str) -> Vec<ResolvedAnchor> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Inputs / outputs
// ---------------------------------------------------------------------------

/// Inputs for one execution of the Factual plan.
///
/// All fields are owned by the caller; the plan consumes a borrow so the
/// orchestrator (`Memory::graph_query` body in
/// `task:retr-impl-graph-query-api`) can construct one of these per
/// query without re-allocating the query string.
#[derive(Debug, Clone)]
pub struct FactualPlanInputs<'a> {
    /// User query string. Drives [`EntityResolver::resolve`] only — the
    /// plan does not re-tokenize for traversal.
    pub query: &'a str,

    /// Reproducibility pin (design §5.4). Carried into [`AsOfMode`] so
    /// "live now" judgements never sample the system clock.
    pub query_time: DateTime<Utc>,

    /// `as-of-T` projection (design §4.6 + GOAL-3.4). `None` ⇒ default
    /// "live at `query_time`".
    pub as_of: Option<DateTime<Utc>>,

    /// GOAL-3.5 opt-in: include superseded edges in the response. When
    /// set, the projection is [`AsOfMode::IncludeSuperseded`] and the
    /// returned [`FactualPlanResult::edges`] carry superseded annotations.
    pub include_superseded: bool,

    /// Drop anchors with `match_strength < min_confidence`. `None` ⇒ no
    /// floor (every resolved anchor is kept). Mirrors
    /// [`crate::retrieval::api::GraphQuery::min_confidence`].
    pub min_confidence: Option<f32>,

    /// Cap on anchors traversed (design §4.1 latency budget — "1 hop ×
    /// `max_anchors` (default 5)"). Anchors beyond this cap are dropped
    /// after the confidence filter. A value of `0` is allowed — it
    /// produces an empty traversal which the plan then surfaces as
    /// [`FactualOutcome::DowngradedNoEntity`].
    pub max_anchors: usize,

    /// Optional predicate restriction passed through to
    /// [`GraphRead::edges_of`]. `None` ⇒ all predicates.
    pub predicate_filter: Option<Predicate>,

    /// Per-anchor **hard cap** on memories retrieved by
    /// [`GraphRead::memories_mentioning_entity`]. The plan caps the total
    /// candidate set at `max_anchors * memory_limit_per_entity` (in
    /// practice the graph is sparse and this rarely binds; the cap exists
    /// to keep traversal bounded under degenerate hub-entity cases).
    ///
    /// Note: this is the *ceiling*, not the operating point. The plan
    /// computes a per-query `effective_limit` from `requested_k` and the
    /// number of resolved anchors (ISS-105 — overfetch principle from
    /// design §4.5 / §7.3 generalized to Factual). `memory_limit_per_entity`
    /// only binds when the overfetch formula would exceed it.
    pub memory_limit_per_entity: usize,

    /// User-requested result count from
    /// [`crate::retrieval::api::GraphQuery::limit`]. Used to size the
    /// per-anchor recall budget so Factual contributes
    /// `α × requested_k` candidates to fuse_rrf, where α = 3 is the
    /// overfetch ratio inherited from Affective (design §4.5 step 2 +
    /// §7.3, ISS-105). The plan computes
    /// `effective_limit = min(α × requested_k / anchors.len(),
    /// memory_limit_per_entity)` after entity resolution.
    pub requested_k: usize,

    /// Optional fixed entity allowlist from
    /// [`crate::retrieval::api::GraphQuery::entity_filter`]. When set,
    /// only anchors whose `entity_id` is in this set are kept.
    pub entity_filter: Option<&'a [Uuid]>,
}

/// Default caps used by [`FactualPlanInputs`] in tests / placeholder
/// orchestrator wiring.
///
/// Keep in sync with design §4.1 ("max anchors default 5", traversal
/// "conservative cap at 500 edges visited"). The per-entity memory cap
/// is set so 5 anchors × 100 = 500 candidates max — matches the §4.1
/// budget envelope.
impl FactualPlanInputs<'_> {
    /// Sensible default for the v0.3 budget envelope (design §4.1).
    /// Used by tests and placeholder wiring; production callers go
    /// through `RetrievalConfig` (out of scope for this task).
    pub const DEFAULT_MAX_ANCHORS: usize = 5;
    /// Default per-entity memory cap (design §4.1 latency envelope).
    pub const DEFAULT_MEMORY_LIMIT_PER_ENTITY: usize = 100;
    /// Overfetch ratio α from design §4.5 / §7.3 (ISS-105). Each
    /// fuse-contributing sub-plan should produce α × requested_k
    /// candidates so RRF has a real selection pool, not just a
    /// merge-with-dedup output. α = 3 is the value chosen by
    /// Affective and generalized to Factual / Associative here.
    pub const OVERFETCH_RATIO: usize = 3;
}

/// Per-edge candidate row surfaced by the Factual plan.
///
/// We carry the raw [`Edge`] *and* the projection annotation so the
/// fusion module can compute provenance scores (graph distance, recency)
/// without re-running the bi-temporal projection. Tests can also assert
/// edge identity directly on these rows.
#[derive(Debug, Clone)]
pub struct FactualEdgeRow {
    /// The anchor that produced this edge (subject side of the 1-hop
    /// expansion). Tracked so per-anchor budgets and trace
    /// (`task:retr-impl-explain-trace`) can attribute edges to anchors.
    pub anchor_id: Uuid,
    /// The other end of the edge (linked entity, if any). `None` when
    /// the edge points to a literal (`EdgeEnd::Literal`).
    pub linked_entity: Option<Uuid>,
    /// Bi-temporal projection of the raw edge. Use
    /// [`ProjectedEdge::is_live`] to distinguish live vs superseded rows
    /// in the [`AsOfMode::IncludeSuperseded`] case.
    pub projected: ProjectedEdge,
}

/// Per-memory candidate row surfaced by the Factual plan.
///
/// `seen_via` records every anchor whose 1-hop traversal surfaced this
/// memory. The orchestrator can use the cardinality as a "graph_score"
/// signal in fusion (more anchors agreeing → stronger graph evidence).
/// Sorted by `BTreeSet` so iteration order is deterministic.
///
/// `edge_seeded` is true when this memory was admitted to the candidate
/// pool by the ISS-189 D1 edge-provenance seed — i.e. it is the source
/// episode of a *typed graph edge* that asserts the queried relationship,
/// not merely a coincidental co-mention surfaced by the recency scan.
/// ISS-192 fix 3 uses this to give edge-seeded candidates a numerator
/// privilege in `graph_score`: an asserting edge is categorically stronger
/// evidence than breadth of co-mention. A memory can be both edge-seeded
/// AND co-mentioned; the flag is true if it was edge-seeded at all.
#[derive(Debug, Clone)]
pub struct FactualMemoryRow {
    pub memory_id: MemoryId,
    pub seen_via: BTreeSet<Uuid>,
    pub edge_seeded: bool,
}

/// Outcome surface for the Factual plan — design §6.4 mapping.
///
/// `Ok` and `Empty` map onto [`crate::retrieval::api::RetrievalOutcome`]
/// 1:1 today; the richer `DowngradedFromFactual { reason }` variant lands
/// when `task:retr-impl-typed-outcomes` (T12) ships. Until then we keep
/// the local enum so this module compiles without depending on T12's
/// surface, and we provide a `to_retrieval_outcome` adaptor below for
/// the orchestrator wiring.
#[derive(Debug, Clone, PartialEq)]
pub enum FactualOutcome {
    /// Plan ran end-to-end with non-empty results (post-filter).
    Ok,
    /// Plan ran end-to-end but the candidate set is empty after
    /// projection / filtering (e.g. as-of-T pre-existence).
    Empty,
    /// §4.1 step 2 — anchor-resolution returned no usable entity. The
    /// orchestrator should switch to Associative (§4.3). Carries a
    /// human-readable reason so traces can distinguish "no token
    /// matched" from "all matches below `min_confidence`".
    DowngradedNoEntity { reason: &'static str },
    /// Anchors resolved but every 1-hop edge was filtered out (e.g.
    /// `as-of-T` precedes any edge's `valid_from`). Distinct from
    /// `Empty` for trace fidelity — `Empty` means memories were absent,
    /// `DowngradedNoEdges` means the graph itself had no live structure
    /// at the projection instant.
    DowngradedNoEdges,
    /// Per-stage budget cutoff fired between stages (§7.3). The plan
    /// returns whatever it has accumulated — never an error.
    Cutoff,
}

impl FactualOutcome {
    /// Lift to the public [`crate::retrieval::api::RetrievalOutcome`]
    /// (T12 — `task:retr-impl-typed-outcomes`).
    ///
    /// Mapping (design §6.4):
    /// - `Ok` (non-empty results) → `RetrievalOutcome::Ok`
    /// - `Ok` (empty results, post-projection) → `EntityFoundNoEdges`
    ///   with the surviving anchors empty (the projection cleared
    ///   them; caller treats it as "no edges to traverse")
    /// - `Empty` → `EntityFoundNoEdges { entities: vec![] }` — the
    ///   plan resolved anchors but the candidate set was projected
    ///   empty; no anchor list is preserved at this layer (the
    ///   adaptor takes only the local outcome, not the rich anchors)
    /// - `DowngradedNoEntity` → `NoEntityFound` (no token resolved)
    /// - `DowngradedNoEdges` → `EntityFoundNoEdges`
    /// - `Cutoff` → `Ok` (partial results, never `Err`) when results
    ///   are present; `EntityFoundNoEdges` when empty (the budget
    ///   fired before edges could be assembled)
    ///
    /// `results_empty` lets the adaptor distinguish "we ran cleanly
    /// but the answer set is empty" from "we got rows" — both stay
    /// inside `Ok(_)` per GUARD-6 semantics.
    pub fn to_retrieval_outcome(
        &self,
        results_empty: bool,
    ) -> crate::retrieval::api::RetrievalOutcome {
        use crate::retrieval::api::RetrievalOutcome;
        match self {
            FactualOutcome::Ok if !results_empty => RetrievalOutcome::Ok,
            FactualOutcome::Ok | FactualOutcome::Empty | FactualOutcome::DowngradedNoEdges => {
                RetrievalOutcome::EntityFoundNoEdges { entities: vec![] }
            }
            FactualOutcome::DowngradedNoEntity { .. } => RetrievalOutcome::NoEntityFound {
                query_tokens: vec![],
            },
            FactualOutcome::Cutoff if !results_empty => RetrievalOutcome::Ok,
            FactualOutcome::Cutoff => RetrievalOutcome::EntityFoundNoEdges { entities: vec![] },
        }
    }
}

/// Result envelope returned by [`FactualPlan::execute`].
///
/// Holds *unscored* candidate rows. Fusion / scoring is
/// `task:retr-impl-fusion`. Order in `memories` is stable (sorted by
/// `MemoryId`) so test assertions don't depend on hash-map traversal.
#[derive(Debug, Clone)]
pub struct FactualPlanResult {
    /// Anchors that survived `min_confidence` + `max_anchors` filters
    /// (in match-strength-descending order, ties broken by `entity_id`
    /// ascending).
    pub anchors: Vec<ResolvedAnchor>,
    /// Projected 1-hop edges — what fusion will use for graph signals.
    pub edges: Vec<FactualEdgeRow>,
    /// Linked entities discovered through 1-hop traversal (excludes
    /// the anchors themselves so the count reflects "neighborhood
    /// breadth" cleanly).
    pub linked_entities: BTreeSet<Uuid>,
    /// Candidate memories, sorted by `memory_id` ascending.
    pub memories: Vec<FactualMemoryRow>,
    /// Plan-level outcome (see [`FactualOutcome`]).
    pub outcome: FactualOutcome,
}

impl FactualPlanResult {
    /// Empty result with a downgrade outcome — used when anchors are
    /// missing / all filtered out. Helper keeps the no-results paths
    /// uniform.
    fn empty_with(outcome: FactualOutcome) -> Self {
        Self {
            anchors: Vec::new(),
            edges: Vec::new(),
            linked_entities: BTreeSet::new(),
            memories: Vec::new(),
            outcome,
        }
    }
}

// ---------------------------------------------------------------------------
// FactualPlan — the executor
// ---------------------------------------------------------------------------

/// Stateless executor for the Factual plan.
///
/// Construction is trivial — the executor borrows everything it needs at
/// `execute()` time. Held as a unit struct so the orchestrator can grow
/// configuration fields here later (e.g. `RetrievalConfig`) without
/// breaking the public function name.
#[derive(Debug, Clone, Copy, Default)]
pub struct FactualPlan;

impl FactualPlan {
    /// Construct a [`FactualPlan`]. Currently zero-cost; reserved for
    /// future per-instance configuration.
    pub fn new() -> Self {
        Self
    }

    /// Run the plan against `graph` for `inputs`, accumulating costs in
    /// `budget`. Returns a [`FactualPlanResult`] — *never* an `Err` for
    /// a budget cutoff (see §7.3). Storage errors propagate via
    /// [`GraphError`].
    pub fn execute<G: GraphRead + ?Sized>(
        &self,
        inputs: &FactualPlanInputs<'_>,
        resolver: &dyn EntityResolver,
        graph: &G,
        budget: &mut BudgetController,
    ) -> Result<FactualPlanResult, GraphError> {
        // ----- Stage 1: entity resolution (design §4.1 step 1) -----
        budget.begin_stage(Stage::EntityResolution);
        let mut anchors = resolver.resolve(inputs.query);
        budget.end_stage();

        // Confidence floor (§4.1 step 1 secondary filter).
        if let Some(floor) = inputs.min_confidence {
            anchors.retain(|a| a.match_strength >= floor);
        }
        // Optional explicit allowlist (`GraphQuery.entity_filter`).
        if let Some(allow) = inputs.entity_filter {
            let allow_set: HashSet<Uuid> = allow.iter().copied().collect();
            anchors.retain(|a| allow_set.contains(&a.entity_id));
        }
        // Stable sort: descending match_strength, ties broken ascending
        // entity_id (so determinism doesn't depend on resolver ordering).
        anchors.sort_by(|a, b| {
            b.match_strength
                .partial_cmp(&a.match_strength)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
        // Deduplicate by entity_id, keeping the strongest match.
        let mut seen = HashSet::new();
        anchors.retain(|a| seen.insert(a.entity_id));
        // Apply max_anchors cap.
        if anchors.len() > inputs.max_anchors {
            anchors.truncate(inputs.max_anchors);
        }

        // §4.1 step 2 — empty anchors ⇒ downgrade.
        if anchors.is_empty() {
            debug!(
                target: LOG_TARGET,
                "stage1 resolve: query={:?} → 0 anchors (DowngradedNoEntity)",
                inputs.query
            );
            return Ok(FactualPlanResult::empty_with(
                FactualOutcome::DowngradedNoEntity {
                    reason: "no_resolved_anchor",
                },
            ));
        }

        // ISS-189 logging — anchor set is the first thing to verify when a
        // factual recall misses: a wrong/incomplete anchor set silently
        // bounds every downstream stage. `debug!` for the summary count,
        // `trace!` for the per-anchor identity.
        debug!(
            target: LOG_TARGET,
            "stage1 resolve: query={:?} → {} anchor(s)",
            inputs.query,
            anchors.len()
        );
        for a in &anchors {
            trace!(
                target: LOG_TARGET,
                "  anchor entity_id={} name={:?} match_strength={:.3}",
                a.entity_id, a.canonical_name, a.match_strength
            );
        }

        // Translate (as_of, include_superseded, query_time) → AsOfMode.
        let mode = AsOfMode::from_query(inputs.as_of, inputs.include_superseded, inputs.query_time);

        // Early cutoff check (cheap — we haven't touched the DB yet
        // beyond resolution). If the outer budget is already blown,
        // surface partial results immediately (§7.3).
        if budget.outer_should_cutoff() {
            return Ok(FactualPlanResult {
                anchors,
                edges: Vec::new(),
                linked_entities: BTreeSet::new(),
                memories: Vec::new(),
                outcome: FactualOutcome::Cutoff,
            });
        }

        // ----- Stage 2: 1-hop edge traversal (design §4.1 step 3) -----
        budget.begin_stage(Stage::EdgeTraversal);
        let edges = traverse_anchors(graph, &anchors, &mode, inputs.predicate_filter.as_ref())?;
        budget.end_stage();

        // Collect linked entities (anchors are excluded so the set
        // reflects neighborhood breadth, not membership).
        let anchor_ids: HashSet<Uuid> = anchors.iter().map(|a| a.entity_id).collect();
        let mut linked_entities: BTreeSet<Uuid> = BTreeSet::new();
        for row in &edges {
            if let Some(eid) = row.linked_entity {
                if !anchor_ids.contains(&eid) {
                    linked_entities.insert(eid);
                }
            }
        }

        // ISS-189 logging — traversal is the stage that misled the first
        // root-cause attempt. `traverse_anchors` only follows OUTGOING
        // edges (`subject = anchor`); an answer reachable only via an
        // INCOMING edge (anchor on the object side) never appears here.
        // Logging the per-edge `memory_id` makes that visible: if the
        // expected source episode is absent from this list, the recall
        // gap is in traversal direction, not in fusion/ranking.
        debug!(
            target: LOG_TARGET,
            "stage2 traverse: {} anchor(s) → {} outgoing edge(s), {} linked entit(ies)",
            anchors.len(),
            edges.len(),
            linked_entities.len()
        );
        for row in &edges {
            trace!(
                target: LOG_TARGET,
                "  edge anchor={} pred={:?} linked={:?} memory_id={:?} live={}",
                row.anchor_id,
                &row.projected.edge.predicate,
                row.linked_entity,
                row.projected.edge.memory_id,
                row.projected.is_live
            );
        }

        if edges.is_empty() {
            // Anchors resolved but 1-hop projection is empty — distinct
            // from `DowngradedNoEntity`. Memory lookup might still
            // produce hits via the anchors themselves, so we don't
            // short-circuit here; downgrade is surfaced after lookup
            // if memories also come up empty.
        }

        if budget.outer_should_cutoff() {
            return Ok(FactualPlanResult {
                anchors,
                edges,
                linked_entities,
                memories: Vec::new(),
                outcome: FactualOutcome::Cutoff,
            });
        }

        // ----- Stage 3: memory candidate lookup (design §4.1 step 4) -----
        budget.begin_stage(Stage::MemoryLookup);
        // Search set = anchors ∪ linked_entities. We iterate anchors
        // first so seen_via is biased toward anchor coverage (the
        // graph_score signal in fusion uses this).
        let mut memories: BTreeMap<MemoryId, BTreeSet<Uuid>> = BTreeMap::new();

        // ISS-189 D1 — seed candidates from edge provenance.
        //
        // Stage 2 already traversed the typed graph and found the edges
        // that *assert* the queried relationship. Each such edge carries
        // the `memory_id` of the source episode that established it. That
        // episode is, by construction, the most directly relevant answer
        // candidate for a factual query — yet the legacy Stage 3 threw it
        // away and re-derived candidates from a recency-truncated flat
        // `memories_mentioning_entity` scan (ORDER BY recorded_at DESC
        // LIMIT). On dense anchors (e.g. an entity mentioned in hundreds
        // of episodes) the answer episode ranks far down the recency list
        // and gets cut before fusion ever sees it.
        //
        // Seeding from `edges[].memory_id` makes the plan honor its own
        // successful graph traversal: the source episode is admitted to
        // the candidate pool unconditionally, attributed to the anchor
        // that produced the edge. The recency-scan loops below still run
        // to add neighborhood breadth, but they can no longer silently
        // discard the answer.
        for edge in &edges {
            if let Some(mid) = &edge.projected.edge.memory_id {
                memories
                    .entry(mid.clone())
                    .or_default()
                    .insert(edge.anchor_id);
            }
        }

        // ISS-192 fix 3 — remember WHICH memories the edge-provenance seed
        // admitted. These carry a typed graph edge that asserts the queried
        // relationship, so `factual_to_scored` can give them a graph_score
        // numerator privilege over coincidental co-mentions (which the
        // recency scan below adds with equal per-anchor weight). Captured
        // here, before the recency scan mutates `memories`, so it reflects
        // only edge provenance.
        let edge_seeded_ids: BTreeSet<MemoryId> = memories.keys().cloned().collect();

        // ISS-189 logging — how many candidates the edge-provenance seed
        // contributed before the recency scan runs. If this is 0 on a
        // query whose answer lives on a traversed edge, the seed source
        // (Stage 2 edges) is empty for the wrong-direction reason above.
        let seeded_count = memories.len();
        debug!(
            target: LOG_TARGET,
            "stage3 seed: edge-provenance contributed {} candidate(s)",
            seeded_count
        );

        // Per-anchor recall budget (ISS-105):
        //
        //   effective_limit = min(α × requested_k / max(1, anchors.len()),
        //                         memory_limit_per_entity)
        //
        // Rationale: Factual must contribute ≈ α × requested_k candidates
        // to fuse_rrf so RRF has a real selection pool (design §4.5
        // overfetch principle). The per-anchor share is the total
        // budget divided by actual anchor count, then floored at 1
        // and ceilinged at the structural cap. Falls back to the
        // legacy `memory_limit_per_entity` when `requested_k == 0`
        // (defensive — `GraphQuery::limit` is required by the API).
        let effective_limit = if inputs.requested_k == 0 {
            inputs.memory_limit_per_entity.max(1)
        } else {
            let total_budget =
                FactualPlanInputs::OVERFETCH_RATIO.saturating_mul(inputs.requested_k);
            let per_anchor = total_budget / anchors.len().max(1);
            per_anchor.clamp(1, inputs.memory_limit_per_entity.max(1))
        };
        let limit = effective_limit;

        debug!(
            target: LOG_TARGET,
            "stage3 recency-scan: effective_limit={} (requested_k={}, anchors={}, cap={})",
            limit,
            inputs.requested_k,
            anchors.len(),
            inputs.memory_limit_per_entity
        );

        for anchor in &anchors {
            let hits = graph.memories_mentioning_entity(anchor.entity_id, limit)?;
            // A full-limit return means the recency-ordered scan was
            // truncated: any source episode older than the `limit`-th most
            // recent mention of this anchor was dropped here. ISS-189: this
            // is exactly how the answer episode got cut (ranked 519/524 by
            // recency on a dense anchor).
            if hits.len() >= limit {
                trace!(
                    target: LOG_TARGET,
                    "  anchor={} recency-scan TRUNCATED at limit={} (older mentions dropped)",
                    anchor.entity_id, limit
                );
            } else {
                trace!(
                    target: LOG_TARGET,
                    "  anchor={} recency-scan returned {} (not truncated)",
                    anchor.entity_id, hits.len()
                );
            }
            for mid in hits {
                memories.entry(mid).or_default().insert(anchor.entity_id);
            }
            if budget.outer_should_cutoff() {
                budget.end_stage();
                return Ok(FactualPlanResult {
                    anchors,
                    edges,
                    linked_entities,
                    memories: memories_into_rows(memories, &edge_seeded_ids),
                    outcome: FactualOutcome::Cutoff,
                });
            }
        }
        for linked in &linked_entities {
            let hits = graph.memories_mentioning_entity(*linked, limit)?;
            for mid in hits {
                memories.entry(mid).or_default().insert(*linked);
            }
            if budget.outer_should_cutoff() {
                budget.end_stage();
                return Ok(FactualPlanResult {
                    anchors,
                    edges,
                    linked_entities,
                    memories: memories_into_rows(memories, &edge_seeded_ids),
                    outcome: FactualOutcome::Cutoff,
                });
            }
        }
        budget.end_stage();

        let memory_rows = memories_into_rows(memories, &edge_seeded_ids);

        // Decide outcome:
        // - edges empty AND memories empty → DowngradedNoEdges (graph
        //   was structurally silent at the projection instant).
        // - memories empty (but edges non-empty) → Empty (graph had
        //   neighborhood, but nobody mentioned the anchors / linked
        //   entities in source memories).
        // - else → Ok.
        let outcome = if edges.is_empty() && memory_rows.is_empty() {
            FactualOutcome::DowngradedNoEdges
        } else if memory_rows.is_empty() {
            FactualOutcome::Empty
        } else {
            FactualOutcome::Ok
        };

        debug!(
            target: LOG_TARGET,
            "factual done: outcome={:?} candidates={} (seeded={})",
            outcome,
            memory_rows.len(),
            seeded_count
        );

        Ok(FactualPlanResult {
            anchors,
            edges,
            linked_entities,
            memories: memory_rows,
            outcome,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk 1-hop edges from each anchor and apply the bi-temporal
/// projection. Encapsulates the storage-layer / projection split so
/// `FactualPlan::execute` stays linear.
fn traverse_anchors<G: GraphRead + ?Sized>(
    graph: &G,
    anchors: &[ResolvedAnchor],
    mode: &AsOfMode,
    predicate: Option<&Predicate>,
) -> Result<Vec<FactualEdgeRow>, GraphError> {
    let mut out: Vec<FactualEdgeRow> = Vec::new();

    for anchor in anchors {
        // Two store call paths:
        // - AsOfMode::At(t)  → `edges_as_of(anchor, t)` returns the
        //   point-in-time slice without us having to filter manually.
        //   We then run `project_edges` only as a no-op pass-through
        //   (the storage layer already applied the filter), but doing
        //   so keeps the trace shape uniform.
        // - everything else → `edges_of(anchor, predicate, include_invalidated)`
        //   then `project_edges`.
        let raw_edges: Vec<Edge> = match mode {
            AsOfMode::At(t) => {
                // edges_as_of doesn't take a predicate filter — apply it
                // post-hoc to keep the storage signature minimal.
                let mut e = graph.edges_as_of(anchor.entity_id, *t)?;
                if let Some(p) = predicate {
                    e.retain(|edge| &edge.predicate == p);
                }
                e
            }
            _ => graph.edges_of(anchor.entity_id, predicate, mode.wants_history())?,
        };

        let projected = project_edges(raw_edges, *mode);
        for pe in projected {
            let linked_entity = match &pe.edge.object {
                EdgeEnd::Entity { id } => Some(*id),
                EdgeEnd::Literal { .. } => None,
            };
            out.push(FactualEdgeRow {
                anchor_id: anchor.entity_id,
                linked_entity,
                projected: pe,
            });
        }

        // ISS-189 — also traverse INCOMING edges (this anchor on the
        // object side). Asymmetric predicates such as `PartOf` store no
        // inverse, so the episode that establishes the relationship is
        // recorded on the edge pointing *at* the anchor
        // (`dog --part_of--> Audrey`). An outgoing-only walk from the
        // anchor never sees that edge's `memory_id`, so the answer
        // episode never enters the candidate pool. For an incoming edge
        // the neighbor is the *subject*, not the object.
        let incoming: Vec<Edge> = match mode {
            AsOfMode::At(t) => {
                // No object-side as-of primitive; reuse the live-or-history
                // incoming query and apply the predicate + temporal filter
                // through `project_edges` below.
                let mut e = graph.edges_into(anchor.entity_id, predicate, true)?;
                e.retain(|edge| {
                    let valid = edge.valid_from.map(|vf| vf <= *t).unwrap_or(true)
                        && edge.valid_to.map(|vt| vt > *t).unwrap_or(true)
                        && edge.invalidated_at.map(|ia| ia > *t).unwrap_or(true);
                    valid
                });
                e
            }
            _ => graph.edges_into(anchor.entity_id, predicate, mode.wants_history())?,
        };

        let projected_in = project_edges(incoming, *mode);
        for pe in projected_in {
            // Neighbor = subject for an incoming edge.
            let linked_entity = Some(pe.edge.subject_id);
            out.push(FactualEdgeRow {
                anchor_id: anchor.entity_id,
                linked_entity,
                projected: pe,
            });
        }
    }
    Ok(out)
}

/// Convert the accumulator map into a sorted [`Vec<FactualMemoryRow>`].
/// Sort key: `memory_id` ascending — tests rely on this.
fn memories_into_rows(
    map: BTreeMap<MemoryId, BTreeSet<Uuid>>,
    edge_seeded_ids: &BTreeSet<MemoryId>,
) -> Vec<FactualMemoryRow> {
    map.into_iter()
        .map(|(memory_id, seen_via)| {
            let edge_seeded = edge_seeded_ids.contains(&memory_id);
            FactualMemoryRow {
                memory_id,
                seen_via,
                edge_seeded,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::{Edge, EdgeEnd};
    use crate::graph::error::GraphError;
    use crate::graph::schema::Predicate;
    use crate::graph::store::{
        CandidateMatch, CandidateQuery, EntityMentions, GraphRead, PipelineRunRow,
        ProposedPredicateStats,
    };
    use crate::graph::{Entity, EntityKind, KnowledgeTopic};
    use crate::retrieval::budget::{BudgetController, CostCaps, StageBudget};
    use chrono::TimeZone;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use uuid::Uuid;

    // ---- helpers --------------------------------------------------------

    fn ts(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.timestamp_opt(secs, 0).single().unwrap()
    }

    fn budget() -> BudgetController {
        BudgetController::new(None, StageBudget::default(), CostCaps::default())
    }

    /// Resolver returning a fixed list of anchors regardless of query.
    struct FixedResolver(Vec<ResolvedAnchor>);
    impl EntityResolver for FixedResolver {
        fn resolve(&self, _q: &str) -> Vec<ResolvedAnchor> {
            self.0.clone()
        }
    }

    /// Minimal in-memory `GraphRead` stub. Implements only the methods the
    /// Factual plan calls; everything else returns an error so tests can't
    /// accidentally rely on broader graph behavior.
    #[derive(Default)]
    struct StubGraph {
        edges_of_map: HashMap<Uuid, Vec<Edge>>,
        edges_into_map: HashMap<Uuid, Vec<Edge>>,
        edges_as_of_map: HashMap<Uuid, Vec<Edge>>,
        memories_of: HashMap<Uuid, Vec<String>>,
        memories_calls: RefCell<usize>,
        memories_limits_seen: RefCell<Vec<usize>>,
    }

    impl StubGraph {
        fn add_edge_for(&mut self, anchor: Uuid, edge: Edge) {
            self.edges_of_map.entry(anchor).or_default().push(edge);
        }
        /// Register `edge` as an INCOMING edge to `object` (ISS-189): the
        /// edge's entity-object is `object`, its subject is some neighbor.
        fn add_incoming_edge_for(&mut self, object: Uuid, edge: Edge) {
            self.edges_into_map.entry(object).or_default().push(edge);
        }
        fn add_memories(&mut self, entity: Uuid, mids: Vec<&str>) {
            self.memories_of
                .insert(entity, mids.into_iter().map(String::from).collect());
        }
        fn set_edges_as_of(&mut self, anchor: Uuid, edges: Vec<Edge>) {
            self.edges_as_of_map.insert(anchor, edges);
        }
    }

    impl GraphRead for StubGraph {
        fn get_entity(&self, _: Uuid) -> Result<Option<Entity>, GraphError> {
            unimplemented!("not used by factual plan tests")
        }
        fn list_entities_by_kind(
            &self,
            _: &EntityKind,
            _: usize,
        ) -> Result<Vec<Entity>, GraphError> {
            unimplemented!()
        }
        fn search_candidates(&self, _: &CandidateQuery) -> Result<Vec<CandidateMatch>, GraphError> {
            unimplemented!()
        }
        fn resolve_alias(&self, _: &str) -> Result<Option<Uuid>, GraphError> {
            unimplemented!()
        }
        fn get_edge(&self, _: Uuid) -> Result<Option<Edge>, GraphError> {
            unimplemented!()
        }
        fn find_edges(
            &self,
            _: Uuid,
            _: &Predicate,
            _: Option<&EdgeEnd>,
            _: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn edges_of(
            &self,
            subject: Uuid,
            predicate: Option<&Predicate>,
            include_invalidated: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            let edges = self.edges_of_map.get(&subject).cloned().unwrap_or_default();
            let mut filtered: Vec<Edge> = edges
                .into_iter()
                .filter(|e| include_invalidated || e.invalidated_at.is_none())
                .collect();
            if let Some(p) = predicate {
                filtered.retain(|e| &e.predicate == p);
            }
            Ok(filtered)
        }
        fn edges_into(
            &self,
            object: Uuid,
            predicate: Option<&Predicate>,
            include_invalidated: bool,
        ) -> Result<Vec<Edge>, GraphError> {
            let edges = self
                .edges_into_map
                .get(&object)
                .cloned()
                .unwrap_or_default();
            let mut filtered: Vec<Edge> = edges
                .into_iter()
                .filter(|e| include_invalidated || e.invalidated_at.is_none())
                .collect();
            if let Some(p) = predicate {
                filtered.retain(|e| &e.predicate == p);
            }
            Ok(filtered)
        }
        fn edges_as_of(
            &self,
            subject: Uuid,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> Result<Vec<Edge>, GraphError> {
            Ok(self
                .edges_as_of_map
                .get(&subject)
                .cloned()
                .unwrap_or_default())
        }
        fn traverse(
            &self,
            _: Uuid,
            _: usize,
            _: usize,
            _: &[Predicate],
        ) -> Result<Vec<(Uuid, Edge)>, GraphError> {
            unimplemented!()
        }
        fn entities_in_episode(&self, _: Uuid) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn edges_in_episode(&self, _: Uuid) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn mentions_of_entity(&self, _: Uuid) -> Result<EntityMentions, GraphError> {
            unimplemented!()
        }
        fn entities_linked_to_memory(&self, _: &str) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn memories_mentioning_entity(
            &self,
            entity: Uuid,
            limit: usize,
        ) -> Result<Vec<String>, GraphError> {
            *self.memories_calls.borrow_mut() += 1;
            self.memories_limits_seen.borrow_mut().push(limit);
            let mut out = self.memories_of.get(&entity).cloned().unwrap_or_default();
            if out.len() > limit {
                out.truncate(limit);
            }
            Ok(out)
        }
        fn edges_sourced_from_memory(&self, _: &str) -> Result<Vec<Edge>, GraphError> {
            unimplemented!()
        }
        fn get_topic(&self, _: Uuid) -> Result<Option<KnowledgeTopic>, GraphError> {
            unimplemented!()
        }
        fn list_topics(
            &self,
            _: &str,
            _: bool,
            _: usize,
        ) -> Result<Vec<KnowledgeTopic>, GraphError> {
            unimplemented!()
        }
        fn latest_pipeline_run_for_memory(
            &self,
            _: &str,
        ) -> Result<Option<PipelineRunRow>, GraphError> {
            unimplemented!()
        }
        fn list_proposed_predicates(
            &self,
            _: u64,
        ) -> Result<Vec<ProposedPredicateStats>, GraphError> {
            unimplemented!()
        }
        fn list_failed_episodes(&self, _: bool) -> Result<Vec<Uuid>, GraphError> {
            unimplemented!()
        }
        fn list_namespaces(&self) -> Result<Vec<String>, GraphError> {
            unimplemented!()
        }
    }

    fn make_inputs<'a>(query: &'a str) -> FactualPlanInputs<'a> {
        FactualPlanInputs {
            query,
            query_time: ts(1_000),
            as_of: None,
            include_superseded: false,
            min_confidence: None,
            max_anchors: FactualPlanInputs::DEFAULT_MAX_ANCHORS,
            predicate_filter: None,
            memory_limit_per_entity: FactualPlanInputs::DEFAULT_MEMORY_LIMIT_PER_ENTITY,
            // Tests pre-date ISS-105 overfetch wiring; 0 selects the
            // legacy "use memory_limit_per_entity directly" branch so
            // existing assertions keep their semantics.
            requested_k: 0,
            entity_filter: None,
        }
    }

    // ---- tests ----------------------------------------------------------

    #[test]
    fn null_resolver_downgrades_no_entity() {
        let plan = FactualPlan::new();
        let graph = StubGraph::default();
        let resolver = NullEntityResolver;
        let mut b = budget();
        let result = plan
            .execute(&make_inputs("anything"), &resolver, &graph, &mut b)
            .unwrap();
        assert!(matches!(
            result.outcome,
            FactualOutcome::DowngradedNoEntity { .. }
        ));
        assert!(result.anchors.is_empty());
        assert!(result.edges.is_empty());
        assert!(result.memories.is_empty());
        // Storage was never queried (early downgrade).
        assert_eq!(*graph.memories_calls.borrow(), 0);
    }

    #[test]
    fn min_confidence_drops_weak_anchors() {
        let strong = ResolvedAnchor {
            entity_id: Uuid::from_u128(1),
            canonical_name: "Alice".into(),
            match_strength: 1.0,
        };
        let weak = ResolvedAnchor {
            entity_id: Uuid::from_u128(2),
            canonical_name: "Bob".into(),
            match_strength: 0.4,
        };
        let plan = FactualPlan::new();
        let graph = StubGraph::default();
        let resolver = FixedResolver(vec![strong.clone(), weak]);
        let mut inputs = make_inputs("alice");
        inputs.min_confidence = Some(0.5);
        let mut b = budget();
        let result = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
        // Weak anchor dropped; strong anchor kept; no edges/memories
        // because the stub graph is empty → DowngradedNoEdges.
        assert_eq!(result.anchors.len(), 1);
        assert_eq!(result.anchors[0].entity_id, strong.entity_id);
        assert_eq!(result.outcome, FactualOutcome::DowngradedNoEdges);
    }

    #[test]
    fn max_anchors_caps_traversal() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();
        let mut anchors = Vec::new();
        for i in 0..10u128 {
            let id = Uuid::from_u128(i + 1);
            anchors.push(ResolvedAnchor {
                entity_id: id,
                canonical_name: format!("e{}", i),
                match_strength: 1.0 - (i as f32 * 0.01),
            });
            graph.add_memories(id, vec!["m1"]);
        }
        let resolver = FixedResolver(anchors);
        let mut inputs = make_inputs("q");
        inputs.max_anchors = 3;
        let mut b = budget();
        let result = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
        assert_eq!(result.anchors.len(), 3);
        // Strongest match_strengths should win (sorted descending).
        let strengths: Vec<f32> = result.anchors.iter().map(|a| a.match_strength).collect();
        assert!(strengths.windows(2).all(|w| w[0] >= w[1]));
        // Memory lookup ran for exactly 3 anchors (no linked entities).
        assert_eq!(*graph.memories_calls.borrow(), 3);
    }

    #[test]
    fn traversal_collects_linked_entities_and_memories() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let alice = Uuid::from_u128(101);
        let bob = Uuid::from_u128(202);
        let carol = Uuid::from_u128(303);

        // alice --knows--> bob, alice --knows--> carol
        let edge_to_bob = Edge::new(
            alice,
            Predicate::proposed("knows"),
            EdgeEnd::Entity { id: bob },
            Some(ts(100)),
            ts(150),
        );
        let edge_to_carol = Edge::new(
            alice,
            Predicate::proposed("knows"),
            EdgeEnd::Entity { id: carol },
            Some(ts(200)),
            ts(250),
        );
        graph.add_edge_for(alice, edge_to_bob);
        graph.add_edge_for(alice, edge_to_carol);
        graph.add_memories(alice, vec!["m_alice"]);
        graph.add_memories(bob, vec!["m_bob_1", "m_bob_2"]);
        graph.add_memories(carol, vec!["m_carol", "m_alice"]); // m_alice shared

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: alice,
            canonical_name: "Alice".into(),
            match_strength: 1.0,
        }]);

        let mut b = budget();
        let result = plan
            .execute(&make_inputs("alice"), &resolver, &graph, &mut b)
            .unwrap();

        assert_eq!(result.outcome, FactualOutcome::Ok);
        assert_eq!(result.anchors.len(), 1);
        assert_eq!(result.edges.len(), 2);
        assert!(result.linked_entities.contains(&bob));
        assert!(result.linked_entities.contains(&carol));
        assert!(!result.linked_entities.contains(&alice));

        // Memories sorted by id ascending.
        let mids: Vec<&str> = result
            .memories
            .iter()
            .map(|r| r.memory_id.as_str())
            .collect();
        let mut sorted = mids.clone();
        sorted.sort();
        assert_eq!(mids, sorted);
        // m_alice seen via alice (anchor) AND via carol (linked entity).
        let m_alice = result
            .memories
            .iter()
            .find(|r| r.memory_id == "m_alice")
            .unwrap();
        assert!(m_alice.seen_via.contains(&alice));
        assert!(m_alice.seen_via.contains(&carol));
    }

    #[test]
    fn include_superseded_returns_history_via_projection() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();
        let alice = Uuid::from_u128(1);
        // One live edge, one superseded edge.
        let live = Edge::new(
            alice,
            Predicate::proposed("p"),
            EdgeEnd::Entity {
                id: Uuid::from_u128(2),
            },
            Some(ts(100)),
            ts(150),
        );
        let mut superseded = Edge::new(
            alice,
            Predicate::proposed("p"),
            EdgeEnd::Entity {
                id: Uuid::from_u128(3),
            },
            Some(ts(50)),
            ts(80),
        );
        superseded.invalidated_at = Some(ts(140));
        graph.add_edge_for(alice, live.clone());
        graph.add_edge_for(alice, superseded.clone());
        graph.add_memories(alice, vec!["m"]);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: alice,
            canonical_name: "a".into(),
            match_strength: 1.0,
        }]);

        // Default mode (Now): superseded edge filtered.
        let mut b = budget();
        let r1 = plan
            .execute(&make_inputs("a"), &resolver, &graph, &mut b)
            .unwrap();
        assert_eq!(r1.edges.len(), 1);
        assert!(r1.edges.iter().all(|e| e.projected.is_live));

        // include_superseded: history view with annotation.
        let mut inputs = make_inputs("a");
        inputs.include_superseded = true;
        let mut b2 = budget();
        let r2 = plan.execute(&inputs, &resolver, &graph, &mut b2).unwrap();
        assert_eq!(r2.edges.len(), 2);
        let any_dead = r2.edges.iter().any(|e| !e.projected.is_live);
        assert!(
            any_dead,
            "superseded edge must be present and annotated dead"
        );
        // GUARD-3: superseded row carries its `superseded_at` for audit.
        let dead = r2
            .edges
            .iter()
            .find(|e| !e.projected.is_live)
            .expect("superseded row");
        assert!(dead.projected.superseded_at.is_some());
    }

    #[test]
    fn as_of_t_uses_edges_as_of_path() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();
        let alice = Uuid::from_u128(1);
        let only_at_500 = Edge::new(
            alice,
            Predicate::proposed("at_t"),
            EdgeEnd::Entity {
                id: Uuid::from_u128(2),
            },
            Some(ts(400)),
            ts(450),
        );
        graph.set_edges_as_of(alice, vec![only_at_500.clone()]);
        graph.add_memories(alice, vec!["m"]);
        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: alice,
            canonical_name: "a".into(),
            match_strength: 1.0,
        }]);

        let mut inputs = make_inputs("a");
        inputs.as_of = Some(ts(500));
        let mut b = budget();
        let r = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].projected.edge.id, only_at_500.id);
        assert!(r.edges[0].projected.is_live);
    }

    #[test]
    fn entity_filter_restricts_anchor_set() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();
        let a = Uuid::from_u128(1);
        let b_id = Uuid::from_u128(2);
        graph.add_memories(a, vec!["ma"]);
        graph.add_memories(b_id, vec!["mb"]);
        let resolver = FixedResolver(vec![
            ResolvedAnchor {
                entity_id: a,
                canonical_name: "A".into(),
                match_strength: 1.0,
            },
            ResolvedAnchor {
                entity_id: b_id,
                canonical_name: "B".into(),
                match_strength: 0.9,
            },
        ]);
        let allow = vec![a];
        let mut inputs = make_inputs("q");
        inputs.entity_filter = Some(&allow);
        let mut bg = budget();
        let r = plan.execute(&inputs, &resolver, &graph, &mut bg).unwrap();
        assert_eq!(r.anchors.len(), 1);
        assert_eq!(r.anchors[0].entity_id, a);
        // Only the allowed entity's memories were fetched.
        let mids: Vec<&str> = r.memories.iter().map(|m| m.memory_id.as_str()).collect();
        assert_eq!(mids, vec!["ma"]);
    }

    #[test]
    fn determinism_anchor_sort_stable() {
        // Two anchors with identical match_strength → tie broken by
        // entity_id ascending. Verify the order doesn't drift across runs.
        let plan = FactualPlan::new();
        let graph = StubGraph::default();
        let id_lo = Uuid::from_u128(1);
        let id_hi = Uuid::from_u128(2);
        let resolver = FixedResolver(vec![
            ResolvedAnchor {
                entity_id: id_hi,
                canonical_name: "hi".into(),
                match_strength: 0.7,
            },
            ResolvedAnchor {
                entity_id: id_lo,
                canonical_name: "lo".into(),
                match_strength: 0.7,
            },
        ]);
        let mut bg = budget();
        let r = plan
            .execute(&make_inputs("q"), &resolver, &graph, &mut bg)
            .unwrap();
        assert_eq!(r.anchors[0].entity_id, id_lo);
        assert_eq!(r.anchors[1].entity_id, id_hi);
    }

    #[test]
    fn factual_outcome_to_retrieval_outcome_lift() {
        use crate::retrieval::api::RetrievalOutcome;
        // Ok with results → Ok.
        assert!(matches!(
            FactualOutcome::Ok.to_retrieval_outcome(false),
            RetrievalOutcome::Ok
        ));
        // Ok but empty results → EntityFoundNoEdges (anchors resolved
        // but candidates projected away).
        assert!(matches!(
            FactualOutcome::Ok.to_retrieval_outcome(true),
            RetrievalOutcome::EntityFoundNoEdges { .. }
        ));
        // No-entity-resolved → NoEntityFound (T12).
        assert!(matches!(
            FactualOutcome::DowngradedNoEntity { reason: "x" }.to_retrieval_outcome(false),
            RetrievalOutcome::NoEntityFound { .. }
        ));
        // No-edges → EntityFoundNoEdges.
        assert!(matches!(
            FactualOutcome::DowngradedNoEdges.to_retrieval_outcome(false),
            RetrievalOutcome::EntityFoundNoEdges { .. }
        ));
        // Cutoff with results → Ok (partial result is still success).
        assert!(matches!(
            FactualOutcome::Cutoff.to_retrieval_outcome(false),
            RetrievalOutcome::Ok
        ));
        // Cutoff with no results → EntityFoundNoEdges.
        assert!(matches!(
            FactualOutcome::Cutoff.to_retrieval_outcome(true),
            RetrievalOutcome::EntityFoundNoEdges { .. }
        ));
    }

    /// ISS-105 regression: when `requested_k > 0`, the per-anchor
    /// recall budget must be `min(α × requested_k / anchors.len(),
    /// memory_limit_per_entity)`, not `memory_limit_per_entity`
    /// directly. Locks in the overfetch principle (design §4.5 / §7.3
    /// generalized to Factual).
    #[test]
    fn iss105_overfetch_per_anchor_limit_scales_with_requested_k() {
        let plan = FactualPlan::new();

        // Case 1: 1 anchor, requested_k = 10 → effective_limit = 30
        // (3 × 10 / 1, well under cap of 100).
        {
            let mut graph = StubGraph::default();
            let alice = Uuid::from_u128(1);
            graph.add_memories(alice, vec!["m1"]);
            let resolver = FixedResolver(vec![ResolvedAnchor {
                entity_id: alice,
                canonical_name: "Alice".into(),
                match_strength: 1.0,
            }]);
            let mut inputs = make_inputs("alice");
            inputs.requested_k = 10;
            inputs.memory_limit_per_entity = 100;
            let mut b = budget();
            let _ = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
            assert_eq!(
                graph.memories_limits_seen.borrow().as_slice(),
                &[30],
                "1 anchor × K=10, α=3 → per-anchor limit = 30"
            );
        }

        // Case 2: 1 anchor, requested_k = 50 → effective_limit = 100
        // (3 × 50 = 150 clamped to memory_limit_per_entity = 100).
        {
            let mut graph = StubGraph::default();
            let alice = Uuid::from_u128(1);
            graph.add_memories(alice, vec!["m1"]);
            let resolver = FixedResolver(vec![ResolvedAnchor {
                entity_id: alice,
                canonical_name: "Alice".into(),
                match_strength: 1.0,
            }]);
            let mut inputs = make_inputs("alice");
            inputs.requested_k = 50;
            inputs.memory_limit_per_entity = 100;
            let mut b = budget();
            let _ = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
            assert_eq!(
                graph.memories_limits_seen.borrow().as_slice(),
                &[100],
                "1 anchor × K=50 → 150 clamped to cap=100"
            );
        }

        // Case 3: requested_k = 0 falls back to memory_limit_per_entity
        // (legacy test path stays valid).
        {
            let mut graph = StubGraph::default();
            let alice = Uuid::from_u128(1);
            graph.add_memories(alice, vec!["m1"]);
            let resolver = FixedResolver(vec![ResolvedAnchor {
                entity_id: alice,
                canonical_name: "Alice".into(),
                match_strength: 1.0,
            }]);
            let mut inputs = make_inputs("alice");
            inputs.requested_k = 0;
            inputs.memory_limit_per_entity = 50;
            let mut b = budget();
            let _ = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
            assert_eq!(
                graph.memories_limits_seen.borrow().as_slice(),
                &[50],
                "requested_k=0 → use memory_limit_per_entity directly"
            );
        }
    }

    /// ISS-105 regression: with multiple anchors, the overfetch budget
    /// is divided across them so the user-facing K stays invariant to
    /// anchor count (5 anchors × K=50 → 30 per anchor, total ≈ 150 = α·K).
    #[test]
    fn iss105_overfetch_budget_split_across_anchors() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();
        let ids: Vec<Uuid> = (1..=5).map(|i| Uuid::from_u128(i)).collect();
        for id in &ids {
            graph.add_memories(*id, vec!["m"]);
        }
        let resolver = FixedResolver(
            ids.iter()
                .enumerate()
                .map(|(i, id)| ResolvedAnchor {
                    entity_id: *id,
                    canonical_name: format!("E{i}"),
                    match_strength: 1.0,
                })
                .collect(),
        );
        let mut inputs = make_inputs("e0 e1 e2 e3 e4");
        inputs.requested_k = 50;
        inputs.memory_limit_per_entity = 100;
        inputs.max_anchors = 5;
        let mut b = budget();
        let _ = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();
        // 5 anchors, no linked entities → 5 calls, each with limit
        // = 3 × 50 / 5 = 30.
        let limits = graph.memories_limits_seen.borrow();
        assert_eq!(limits.len(), 5, "one call per anchor");
        for l in limits.iter() {
            assert_eq!(*l, 30, "α·K / anchors = 3×50/5 = 30");
        }
    }

    /// ISS-189 AC-1 — edge provenance seeds the candidate pool.
    ///
    /// Stage 2 traverses an edge carrying `memory_id = "answer_ep"`. The
    /// Factual plan must admit `answer_ep` to its candidate set directly
    /// from the edge, attributed to the anchor that produced the edge —
    /// regardless of whether the recency scan would have surfaced it.
    #[test]
    fn iss189_ac1_edge_memory_id_seeds_candidate() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let audrey = Uuid::from_u128(1);
        let pepper = Uuid::from_u128(2);

        // pepper --part_of--> audrey, sourced from the answer episode.
        let mut edge = Edge::new(
            audrey,
            Predicate::proposed("part_of"),
            EdgeEnd::Entity { id: pepper },
            Some(ts(100)),
            ts(150),
        );
        edge.memory_id = Some("answer_ep".into());
        graph.add_edge_for(audrey, edge);
        // Recency scan returns nothing for the anchor — proves the
        // candidate came from the edge, not the flat scan.
        graph.add_memories(audrey, vec![]);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: audrey,
            canonical_name: "Audrey".into(),
            match_strength: 1.0,
        }]);

        let mut b = budget();
        let result = plan
            .execute(&make_inputs("audrey dogs"), &resolver, &graph, &mut b)
            .unwrap();

        assert_eq!(result.outcome, FactualOutcome::Ok);
        let answer = result
            .memories
            .iter()
            .find(|r| r.memory_id == "answer_ep")
            .expect("edge memory_id must seed the candidate pool");
        assert!(
            answer.seen_via.contains(&audrey),
            "seeded candidate attributed to the anchor that produced the edge"
        );
    }

    /// ISS-189 AC-2 — the answer episode survives a dense anchor.
    ///
    /// Reproduction of conv-44 q29: Audrey is mentioned in many episodes;
    /// the answer episode ranks below the recency-truncation limit, so the
    /// flat `memories_mentioning_entity` scan cuts it. The edge provenance
    /// seeding (D1) must rescue it before fusion.
    #[test]
    fn iss189_ac2_answer_survives_recency_truncation() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let audrey = Uuid::from_u128(1);
        let pepper = Uuid::from_u128(2);

        let mut edge = Edge::new(
            audrey,
            Predicate::proposed("part_of"),
            EdgeEnd::Entity { id: pepper },
            Some(ts(100)),
            ts(150),
        );
        edge.memory_id = Some("answer_ep".into());
        graph.add_edge_for(audrey, edge);

        // Audrey mentioned in many recent episodes; the answer ranks
        // last. With the legacy per-anchor limit the StubGraph truncates
        // the scan and drops `answer_ep`.
        let mut mentions: Vec<&str> = (0..50)
            .map(|i| Box::leak(format!("recent_{i:02}").into_boxed_str()) as &str)
            .collect();
        mentions.push("answer_ep");
        graph.add_memories(audrey, mentions);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: audrey,
            canonical_name: "Audrey".into(),
            match_strength: 1.0,
        }]);

        let mut inputs = make_inputs("audrey dogs");
        // Tight K so the recency scan is truncated well above the answer.
        inputs.requested_k = 3; // α·K = 9 < 51 mentions → answer cut
        inputs.memory_limit_per_entity = 100;

        let mut b = budget();
        let result = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();

        assert!(
            result.memories.iter().any(|r| r.memory_id == "answer_ep"),
            "answer episode must survive recency truncation via edge seeding"
        );
    }

    /// ISS-189 AC-3 — seeding is additive, not a replacement.
    ///
    /// The edge-provenance seed must coexist with the recency-scan
    /// candidates: both the seeded answer and the flat-scan neighborhood
    /// appear, and a candidate surfaced by both paths records both
    /// attributions in `seen_via`.
    #[test]
    fn iss189_ac3_seeding_is_additive() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let audrey = Uuid::from_u128(1);
        let pepper = Uuid::from_u128(2);

        let mut edge = Edge::new(
            audrey,
            Predicate::proposed("part_of"),
            EdgeEnd::Entity { id: pepper },
            Some(ts(100)),
            ts(150),
        );
        edge.memory_id = Some("answer_ep".into());
        graph.add_edge_for(audrey, edge);

        // Flat scan surfaces a neighborhood episode AND the answer_ep
        // (so we can assert dual attribution).
        graph.add_memories(audrey, vec!["neighbor_ep", "answer_ep"]);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: audrey,
            canonical_name: "Audrey".into(),
            match_strength: 1.0,
        }]);

        let mut b = budget();
        let result = plan
            .execute(&make_inputs("audrey dogs"), &resolver, &graph, &mut b)
            .unwrap();

        // Both candidates present.
        assert!(result.memories.iter().any(|r| r.memory_id == "neighbor_ep"));
        let answer = result
            .memories
            .iter()
            .find(|r| r.memory_id == "answer_ep")
            .expect("answer present via both edge seed and flat scan");
        // Surfaced by both paths → single audrey attribution (same
        // entity), but it must be present.
        assert!(answer.seen_via.contains(&audrey));
    }

    /// ISS-189 AC-4 — INCOMING-edge provenance seeds the candidate pool.
    ///
    /// This is the real conv-44-q0 topology: the answer episode is
    /// recorded on edges pointing *at* the anchor
    /// (`pepper --part_of--> Audrey`), with `Audrey` as the entity
    /// object. Audrey has NO outgoing edge carrying the answer. An
    /// outgoing-only traversal (pre-fix) never sees `answer_ep`; the
    /// incoming-edge traversal must pull it in.
    #[test]
    fn iss189_ac4_incoming_edge_memory_id_seeds_candidate() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let audrey = Uuid::from_u128(1);
        let pepper = Uuid::from_u128(2);

        // pepper --part_of--> audrey, sourced from the answer episode.
        // Audrey is the OBJECT here, so this is an incoming edge to Audrey.
        let mut edge = Edge::new(
            pepper,
            Predicate::proposed("part_of"),
            EdgeEnd::Entity { id: audrey },
            Some(ts(100)),
            ts(150),
        );
        edge.memory_id = Some("answer_ep".into());
        graph.add_incoming_edge_for(audrey, edge);

        // Audrey has NO outgoing edges and the recency scan returns
        // nothing — proves the candidate came ONLY from the incoming
        // edge.
        graph.add_memories(audrey, vec![]);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: audrey,
            canonical_name: "Audrey".into(),
            match_strength: 1.0,
        }]);

        let mut b = budget();
        let result = plan
            .execute(
                &make_inputs("which year audrey adopt dogs"),
                &resolver,
                &graph,
                &mut b,
            )
            .unwrap();

        assert_eq!(result.outcome, FactualOutcome::Ok);
        let answer = result
            .memories
            .iter()
            .find(|r| r.memory_id == "answer_ep")
            .expect("incoming-edge memory_id must seed the candidate pool");
        assert!(
            answer.seen_via.contains(&audrey),
            "incoming-edge seed attributed to the anchor on the object side"
        );
        // The subject (pepper) is recorded as a linked entity.
        assert!(
            result.linked_entities.contains(&pepper),
            "incoming-edge subject must appear as a linked entity"
        );
    }

    /// ISS-189 AC-5 — incoming-edge predicate filter is honored.
    ///
    /// When a `predicate_filter` is set, incoming-edge traversal must
    /// apply it just like the outgoing path: only edges whose predicate
    /// matches the filter contribute candidates.
    #[test]
    fn iss189_ac5_incoming_edge_respects_predicate_filter() {
        let plan = FactualPlan::new();
        let mut graph = StubGraph::default();

        let audrey = Uuid::from_u128(1);
        let pepper = Uuid::from_u128(2);
        let house = Uuid::from_u128(3);

        // Matching predicate: pepper --part_of--> audrey (answer).
        let mut want = Edge::new(
            pepper,
            Predicate::proposed("part_of"),
            EdgeEnd::Entity { id: audrey },
            Some(ts(100)),
            ts(150),
        );
        want.memory_id = Some("answer_ep".into());
        graph.add_incoming_edge_for(audrey, want);

        // Non-matching predicate: house --located_at--> audrey (noise).
        let mut noise = Edge::new(
            house,
            Predicate::proposed("located_at"),
            EdgeEnd::Entity { id: audrey },
            Some(ts(100)),
            ts(150),
        );
        noise.memory_id = Some("noise_ep".into());
        graph.add_incoming_edge_for(audrey, noise);

        graph.add_memories(audrey, vec![]);

        let resolver = FixedResolver(vec![ResolvedAnchor {
            entity_id: audrey,
            canonical_name: "Audrey".into(),
            match_strength: 1.0,
        }]);

        let mut inputs = make_inputs("audrey dogs");
        inputs.predicate_filter = Some(Predicate::proposed("part_of"));

        let mut b = budget();
        let result = plan.execute(&inputs, &resolver, &graph, &mut b).unwrap();

        assert!(
            result.memories.iter().any(|r| r.memory_id == "answer_ep"),
            "matching-predicate incoming edge must seed the candidate"
        );
        assert!(
            !result.memories.iter().any(|r| r.memory_id == "noise_ep"),
            "non-matching-predicate incoming edge must be filtered out"
        );
    }
}
