//! Orchestrator — wires `dispatch()` → plan execute → `Vec<ScoredResult>`.
//!
//! Owned by `task:retr-impl-orchestrator-plan-execution` (the next
//! task after `…-classifier-dispatch`). The classifier-dispatch task
//! produced [`crate::retrieval::dispatch::DispatchedQuery`]; this module
//! consumes that and produces:
//!
//! - `Vec<ScoredResult>` — pre-fusion candidates, one row per memory or
//!   topic. Each row carries the per-signal `SubScores` populated only
//!   for signals the source plan emitted (the rest stay `None`,
//!   matching the §6.2a docstring).
//! - [`RetrievalOutcome`] — the typed plan-result mapping (§6.4) so
//!   callers can distinguish `Ok` from `DowngradedNoEntity` /
//!   `DowngradedFromEpisodic` / etc. without inspecting plan internals.
//!
//! ## Why a separate module
//!
//! The fusion module (`retrieval::fusion`) takes `Vec<ScoredResult>`
//! and produces `Vec<ScoredResult>`. The plan modules
//! (`retrieval::plans::*`) produce typed plan-specific candidate
//! structs. The orchestrator is the **adapter layer** that translates
//! plan-typed candidates → `ScoredResult` rows, using a `RecordLoader`
//! to hydrate `MemoryRecord`s on demand.
//!
//! Keeping it in its own module keeps `api.rs` focused on the public
//! `GraphQuery` / `GraphQueryResponse` surface and lets the adapter
//! grow without bloating either side.
//!
//! ## Module layout
//!
//! 1. [`RecordLoader`] trait + production impl + test impl.
//! 2. Adapter functions — one per plan, named `*_to_scored`.
//! 3. [`HybridDispatchExecutor`] — implements
//!    [`HybridSubPlanExecutor`](crate::retrieval::plans::hybrid::HybridSubPlanExecutor)
//!    by delegating to sibling plans.
//! 4. [`execute_plan`] — the central `match dispatched.plan_kind`
//!    arm. Produces `(Vec<ScoredResult>, RetrievalOutcome)`.
//!
//! All entries are `pub(crate)` — the orchestrator surface is internal
//! to `engramai`. Public callers go through
//! [`Memory::graph_query`](crate::memory::Memory::graph_query).

use std::collections::HashMap;

use uuid::Uuid;

use crate::retrieval::api::{ScoredResult, SubScores};
use crate::store_api::MemoryId;
use crate::types::MemoryRecord;

// ---------------------------------------------------------------------------
// 0. PlanCollaborators — borrowed bundle of real-data adapter trait objects
// ---------------------------------------------------------------------------

/// Borrowed bundle of the five plan-collaborator trait objects that
/// [`execute_plan`] (and [`HybridDispatchExecutor`] for Hybrid fan-out)
/// need to read real data from the storage / graph backends.
///
/// Each plan in `crate::retrieval::plans::*` declares a small
/// collaborator trait (e.g. [`EntityResolver`] for Factual,
/// [`EpisodicMemoryStore`] for Episodic). Plans are *generic over* this
/// trait so they can be unit-tested against `Null*` stubs without
/// wiring real storage. The orchestrator's job is to bridge that gap:
/// it holds one real implementation of each trait and passes the right
/// borrow into each plan invocation.
///
/// # Why a struct, not five separate parameters?
///
/// `execute_plan` already has five parameters; adding five more would
/// push the call sites past readability. A single struct also makes it
/// obvious to readers that "if you build a `Memory`, you build *all*
/// five collaborators together" — they share lifetimes and are
/// consumed together.
///
/// # Lifetime
///
/// All members are `&dyn` references so the struct itself is cheap to
/// construct per query. The single lifetime `'a` ties all five together
/// — typically `&Memory` (for the storage / embedding-backed adapters)
/// and the `&dyn GraphRead` lifted by `with_graph_read` (for the
/// graph-backed adapters).
///
/// # Tests / null path
///
/// Tests that exercise the empty-result downgrade paths construct a
/// `PlanCollaborators` whose five fields all point at the existing
/// `Null*` types — `NullEntityResolver`, `NullEpisodicStore`,
/// `NullSeedRecaller`, `NullTopicSearcher`, `NullAffectiveSeedRecaller`.
/// This is behaviorally identical to the pre-ISS-049 hardcoded path
/// and is the migration target for any test that doesn't need real
/// data.
pub(crate) struct PlanCollaborators<'a> {
    /// Resolves a free-form query to candidate entity anchors for the
    /// Factual plan (and the Hybrid Factual sub-plan).
    pub entity_resolver: &'a dyn crate::retrieval::plans::factual::EntityResolver,

    /// Time-window memory lookup for the Episodic plan (and the Hybrid
    /// Episodic sub-plan).
    pub episodic_store: &'a dyn crate::retrieval::plans::episodic::EpisodicMemoryStore,

    /// Hybrid-search seed lookup for the Associative plan.
    pub seed_recaller: &'a dyn crate::retrieval::plans::associative::SeedRecaller,

    /// L5 topic search for the Abstract plan (and the Hybrid Abstract
    /// sub-plan).
    pub topic_searcher: &'a dyn crate::retrieval::plans::abstract_l5::TopicSearcher,

    /// Affect-tagged seed recall for the Affective plan (and the
    /// Hybrid Affective sub-plan).
    pub affective_recaller: &'a dyn crate::retrieval::plans::affective::AffectiveSeedRecaller,

    /// Optional embedder for query→memory cosine in plans that don't
    /// otherwise emit a `vector_score`. ISS-172: Factual plan
    /// previously ranked candidates by anchor-fraction + BM25 only,
    /// drowning the gold passage when a single anchor produced 100+
    /// tied `graph_score == 1.0` candidates. Wiring the embedder
    /// through PlanCollaborators lets `factual_to_scored` (and any
    /// future plan that needs it) compute
    /// `cosine(query_embedding, memory_embedding)` once embeddings
    /// are batch-loaded for MMR (ISS-139 Strategy A). `None` ⇒ plan
    /// degrades to graph + BM25 (legacy pre-ISS-172 behaviour).
    pub embedding_provider: Option<&'a crate::embeddings::EmbeddingProvider>,
}

// ---------------------------------------------------------------------------
// 1. RecordLoader — hydrates `MemoryId` → `MemoryRecord`
// ---------------------------------------------------------------------------

/// Hydrates a `MemoryId` into the full `MemoryRecord` needed by
/// `ScoredResult::Memory`.
///
/// Plans surface `MemoryId` (or richer plan-specific rows that *embed*
/// a memory id), but the response envelope wants the live record.
/// Loading is plan-agnostic so we factor it behind a trait — production
/// wires `MemoryStorageLoader`, tests use [`HashMapLoader`] for
/// determinism.
///
/// # Why not just `&Storage`?
///
/// 1. Tests need to assert "loader was called with X ids" without
///    spinning up SQLite.
/// 2. A future tier-aware loader (`load_with_tier(id, MemoryTier)`)
///    will land here when `task:retr-impl-budget-cutoff` adds tier
///    gating; the trait gives us a stable seam to extend.
/// 3. Hybrid sub-plan execution needs the same loader without owning a
///    `&Storage` reference — easier to thread `&dyn RecordLoader`.
///
/// # Missing memories
///
/// `load` returns `None` for ids that no longer exist (forgotten /
/// deleted). The caller adapter **drops** these silently rather than
/// surfacing an error — design §6.2 GUARD-9 ("a missing memory is not
/// a retrieval failure").
pub(crate) trait RecordLoader {
    /// Look up a single memory by id. Returns `None` if missing.
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord>;

    /// Batch lookup. Default impl calls `load` per id; production impls
    /// override with a single SQL `WHERE id IN (...)` query.
    ///
    /// Output preserves input order. Missing ids produce a `None` slot
    /// — callers that want a dense `Vec<MemoryRecord>` filter with
    /// `.flatten()` (idiomatic in Rust 2021).
    fn load_many(&self, ids: &[MemoryId]) -> Vec<Option<MemoryRecord>> {
        ids.iter().map(|id| self.load(id)).collect()
    }

    /// Batch-fetch embeddings for the given memory ids (ISS-139).
    ///
    /// Returns a map containing only the ids that have an embedding row
    /// for the loader's configured model. Missing ids are silently
    /// omitted (caller treats absence as "no diversity signal for this
    /// candidate").
    ///
    /// Default impl returns an empty map — for test loaders that don't
    /// model embeddings, this means MMR sees `embedding: None` on every
    /// candidate and degenerates to pure relevance. The production
    /// `StorageLoader` overrides this with a single `WHERE id IN (...)`
    /// SQL round-trip.
    ///
    /// Intentionally **not** taking a model parameter: the model id is
    /// per-`graph_query` and is captured at loader construction time
    /// (along with the `&Storage` lifetime). Keeping it off the method
    /// keeps test loaders' implementations a no-op.
    fn load_embeddings(&self, _ids: &[&str]) -> std::collections::HashMap<String, Vec<f32>> {
        std::collections::HashMap::new()
    }

    /// BM25 scores for the top-N FTS hits of `query` (ISS-147).
    ///
    /// Returns `id → normalised_score` in `[0, 1]` (already passed
    /// through `signals::bm25_score` with `BM25_DEFAULT_SATURATION` —
    /// adapters consume the map directly without re-normalising).
    ///
    /// Only ids that **matched** the FTS query appear in the map.
    /// Adapter callers look up their candidate ids and fall back to
    /// `Some(0.0)` for misses (NOT `None`) — design §5.1 / ISS-147
    /// AC-3: a `None` triggers missing-signal renormalisation, which
    /// would *upweight* embedding-only candidates and defeat the
    /// hybrid lexical+semantic intent.
    ///
    /// Default impl returns an empty map. Test loaders that don't
    /// model FTS get `Some(0.0)` for every candidate (BM25 channel
    /// becomes uniform zero and fusion behaves identically to the
    /// pre-ISS-147 embed-only path — tests stay deterministic).
    fn fts_scores(&self, _query: &str, _limit: usize) -> std::collections::HashMap<String, f64> {
        std::collections::HashMap::new()
    }
}

/// Production loader — wraps `&Storage` for batched SQL lookups.
///
/// Held as a thin adapter so the lifetime of `&Storage` is bound to
/// the lifetime of the loader, not stashed inside `Memory`. The
/// orchestrator constructs one of these per `graph_query` call.
///
/// `model` is the embedding model id captured at construction so
/// `load_embeddings` can query the right `*_embeddings` rows
/// (ISS-139). Empty string is a valid "embedding provider disabled"
/// sentinel — `load_embeddings` will then find no matching rows and
/// return an empty map.
pub(crate) struct StorageLoader<'a> {
    storage: &'a crate::storage::Storage,
    model: String,
}

impl<'a> StorageLoader<'a> {
    pub(crate) fn new(storage: &'a crate::storage::Storage, model: impl Into<String>) -> Self {
        Self {
            storage,
            model: model.into(),
        }
    }
}

impl RecordLoader for StorageLoader<'_> {
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord> {
        // `Storage::get_by_ids` returns `Vec<MemoryRecord>` filtered to
        // non-deleted, non-superseded rows. For a single id we accept
        // an empty vec (forgotten) or a single row.
        let id_str: &str = id.as_str();
        match self.storage.get_by_ids(&[id_str]) {
            Ok(mut rows) => rows.pop(),
            Err(_) => None,
        }
    }

    fn load_many(&self, ids: &[MemoryId]) -> Vec<Option<MemoryRecord>> {
        if ids.is_empty() {
            return Vec::new();
        }
        // Single SQL round-trip; result order is *not* guaranteed by
        // SQLite for `IN (...)`, so we re-index by id.
        let id_strs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let fetched = match self.storage.get_by_ids(&id_strs) {
            Ok(rows) => rows,
            Err(_) => return vec![None; ids.len()],
        };
        let mut by_id: HashMap<String, MemoryRecord> =
            fetched.into_iter().map(|r| (r.id.clone(), r)).collect();
        ids.iter().map(|id| by_id.remove(id)).collect()
    }

    fn load_embeddings(&self, ids: &[&str]) -> std::collections::HashMap<String, Vec<f32>> {
        if ids.is_empty() || self.model.is_empty() {
            return std::collections::HashMap::new();
        }
        // Single SQL round-trip via the dedicated batch API (ISS-139).
        // SQL errors are silently swallowed → empty map → MMR sees
        // `None` on every candidate and degenerates to relevance-only.
        // This matches the GUARD-9 "missing data is not a retrieval
        // failure" pattern used by `load_many`.
        self.storage
            .get_embeddings_for_ids(ids, &self.model)
            .unwrap_or_default()
    }

    fn fts_scores(&self, query: &str, limit: usize) -> std::collections::HashMap<String, f64> {
        if query.trim().is_empty() || limit == 0 {
            return std::collections::HashMap::new();
        }
        // ISS-147 production read of the BM25 channel. SQL errors are
        // silently swallowed → empty map → adapters fall back to
        // Some(0.0) per fts_scores doc and the trait's GUARD-9
        // pattern. We normalise here so adapters don't need to know
        // about saturation tuning (BM25_DEFAULT_SATURATION = 20.0
        // per design §5.1's "raw values rarely exceed 20" obs).
        let raw = match self.storage.search_fts_with_scores(query, limit) {
            Ok(v) => v,
            Err(_) => return std::collections::HashMap::new(),
        };
        raw.into_iter()
            .map(|(rec, raw_bm25)| {
                let normed = crate::retrieval::fusion::signals::bm25_score(
                    raw_bm25,
                    crate::retrieval::fusion::signals::BM25_DEFAULT_SATURATION,
                );
                (rec.id, normed)
            })
            .collect()
    }
}

/// In-memory loader for tests — preloaded id→record map.
#[cfg(test)]
pub(crate) struct HashMapLoader {
    pub records: HashMap<MemoryId, MemoryRecord>,
}

#[cfg(test)]
impl HashMapLoader {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    pub fn with(mut self, record: MemoryRecord) -> Self {
        self.records.insert(record.id.clone(), record);
        self
    }
}

#[cfg(test)]
impl RecordLoader for HashMapLoader {
    fn load(&self, id: &MemoryId) -> Option<MemoryRecord> {
        self.records.get(id).cloned()
    }
}

// ---------------------------------------------------------------------------
// 2. Per-plan adapters: typed plan result → Vec<ScoredResult>
// ---------------------------------------------------------------------------
//
// Each adapter populates `SubScores` for the signals the source plan
// emits. Signals not emitted stay `None` — fusion treats `None` as
// "no information" (not zero), per §5.1.
//
// Score field on `ScoredResult::Memory` is set to a plan-local default
// (typically `0.0` or the plan's primary signal); fusion overwrites it
// in `fuse_and_rank` using the per-intent weights.

/// Factual plan adapter: 1-hop traversal rows → ScoredResult.
///
/// **Signals emitted**: `graph_score` (number of anchors that
/// surfaced the memory, normalized by `max_anchors`) and
/// `bm25_score` (ISS-147 — lexical channel, `Some(0.0)` for non-FTS
/// hits per §5.1 missing-signal contract). Recency / actr / vector
/// are `None` — Factual is a graph+text plan in v0.3 onwards.
/// ISS-192 fix 3 — edge-seed privilege scoring (pure, no I/O).
///
/// Maps a candidate's anchor-breadth (`seen_via.len() / total_anchors`,
/// already in `[0,1]`) into a graph_score that privileges candidates
/// admitted by the edge-provenance seed — i.e. those carrying a typed
/// graph edge that *asserts* the queried relationship — over pure
/// co-mention breadth.
///
/// When `bonus == 0.0` the result is exactly `breadth` (inert: the
/// edge-seeded flag is ignored, byte-identical to pre-fix behaviour).
///
/// When `bonus > 0.0` the `[0,1]` range is split into two bands:
/// pure co-mentions map into `[0, 1-bonus]` and edge-seeded candidates
/// into `[bonus, 1]`. This guarantees ANY edge-seeded candidate
/// outscores ANY pure co-mention regardless of breadth, while still
/// preserving relative ordering *within* each band. Removes/clamps
/// nothing destructive — multi-hop bridges (co-mention based) keep their
/// breadth ordering, they only lose the artificial advantage over an
/// asserting edge.
fn edge_seeded_graph_score(breadth: f64, edge_seeded: bool, bonus: f64) -> f64 {
    if bonus <= 0.0 {
        return breadth;
    }
    let scaled = breadth * (1.0 - bonus);
    if edge_seeded {
        scaled + bonus
    } else {
        scaled
    }
}

pub(crate) fn factual_to_scored(
    result: &crate::retrieval::plans::factual::FactualPlanResult,
    loader: &dyn RecordLoader,
    bm25_by_id: &HashMap<String, f64>,
    query_embedding: Option<&[f32]>,
) -> Vec<ScoredResult> {
    if result.memories.is_empty() {
        return Vec::new();
    }

    // Normalize graph_score: `seen_via.len() / total_anchors`. When
    // `total_anchors == 0` (defensive), use 1.0 to avoid div-by-zero —
    // but the plan guarantees ≥ 1 anchor at this point so it's purely
    // belt-and-suspenders.
    let total_anchors = result.anchors.len().max(1) as f64;

    // ISS-192 fix 3 — edge-seed privilege weight. Read once per call from
    // `ENGRAM_FACTUAL_EDGE_SEED_BONUS`. Default 0.0 = inert (graph_score is
    // byte-identical to pre-fix breadth). Clamped to [0,1); a value of 1.0
    // would zero out breadth signal entirely, so we cap just below.
    let edge_seed_bonus = std::env::var("ENGRAM_FACTUAL_EDGE_SEED_BONUS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 0.999);

    let ids: Vec<MemoryId> = result
        .memories
        .iter()
        .map(|m| m.memory_id.clone())
        .collect();
    let records = loader.load_many(&ids);

    // ISS-139 Strategy A: batch-fetch embeddings so the MMR Stage C.5
    // hook has diversity signal even on non-Hybrid plans (factual
    // queries on single-conv corpora are the LoCoMo list-question hot
    // path; without this they regressed to relevance-only MMR no-op).
    let id_strs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    let embeddings_by_id = loader.load_embeddings(&id_strs);

    result
        .memories
        .iter()
        .zip(records.into_iter())
        .filter_map(|(row, rec)| {
            let record = rec?; // drop missing rows silently
                               // ISS-192 fix 3 — edge-seed privilege.
                               //
                               // Pure breadth (`seen_via.len() / total_anchors`) treats a
                               // candidate reached via a typed graph edge that *asserts* the
                               // queried relationship identically to one that merely
                               // co-mentions the anchor in N episodes. On dense corpora a
                               // coincidental high-co-mention memory outranks the asserting
                               // edge's source episode, which is exactly how conv-26-q0's
                               // dated gold episode (seen via one PartOf edge) lost to
                               // undated co-mentions. This is breadth-dilution (Defect B).
                               //
                               // The fix is ADDITIVE and removes no candidates (multi-hop
                               // bridges, which are co-mention based, keep their breadth
                               // score — they only lose the artificial advantage over an
                               // asserting edge). We split the [0,1] graph_score range into
                               // two bands: pure co-mentions map into `[0, 1-w]` and
                               // edge-seeded candidates into `[w, 1]`, so ANY edge-seeded
                               // hit outscores ANY pure co-mention. `w` (the bonus weight)
                               // is read from `ENGRAM_FACTUAL_EDGE_SEED_BONUS`, default 0.0
                               // → byte-identical to pre-fix behaviour (inert until the A/B
                               // sets it).
            let breadth = (row.seen_via.len() as f64) / total_anchors;
            let graph_score = edge_seeded_graph_score(breadth, row.edge_seeded, edge_seed_bonus);
            // ISS-147 AC-3: Some(0.0) for FTS misses (NOT None) —
            // None triggers missing-signal renormalisation which
            // would defeat the lexical channel's contribution.
            let bm25 = bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0);
            let embedding = embeddings_by_id.get(record.id.as_str()).cloned();
            // ISS-172: per-candidate cosine(query, memory_embedding)
            // — the semantic signal Factual was missing. Same
            // Some(0.0) convention as BM25: emit 0 when the memory
            // has no embedding row (rare; happens for legacy rows
            // pre-ISS-139 or when load_embeddings can't find the
            // model). `None` query_embedding (no provider wired)
            // keeps `vector_score = None` so fusion renormalizes
            // exactly as it did pre-ISS-172.
            let vector_score: Option<f64> = query_embedding.map(|qv| {
                let cosine = embedding
                    .as_ref()
                    .map(|mv| {
                        crate::embeddings::EmbeddingProvider::cosine_similarity(qv, mv) as f64
                    })
                    .unwrap_or(0.0);
                cosine.clamp(0.0, 1.0)
            });
            let sub_scores = SubScores {
                graph_score: Some(graph_score.clamp(0.0, 1.0)),
                bm25_score: Some(bm25),
                vector_score,
                ..Default::default()
            };
            Some(ScoredResult::Memory {
                record,
                score: 0.0, // overwritten by fusion::combine
                sub_scores,
                embedding,
            })
        })
        .collect()
}

/// Episodic plan adapter: time-windowed memory ids → ScoredResult.
///
/// **Signals emitted**: `recency_score` (linear ramp inside the
/// window) and `bm25_score` (ISS-147 — `Some(0.0)` for non-FTS
/// hits per §5.1). Vector / graph / actr / affect stay `None`.
pub(crate) fn episodic_to_scored(
    result: &crate::retrieval::plans::episodic::EpisodicPlanResult,
    loader: &dyn RecordLoader,
    bm25_by_id: &HashMap<String, f64>,
) -> Vec<ScoredResult> {
    if result.memories.is_empty() {
        return Vec::new();
    }

    let records = loader.load_many(&result.memories);

    // ISS-139 Strategy A: batch-fetch embeddings so MMR sees diversity
    // signal on episodic results too. Episodic plans rank within a
    // time window, but list-style questions ("what did we do last
    // week?") can still cluster on near-duplicate memories.
    let id_strs: Vec<&str> = result.memories.iter().map(|s| s.as_str()).collect();
    let embeddings_by_id = loader.load_embeddings(&id_strs);

    // Recency is computed against the window.end (anchor of the
    // half-life decay). When window is None (defensive — plan
    // downgraded), every recency_score is 0.0.
    let window_end = result.window.as_ref().map(|w| w.end);

    records
        .into_iter()
        .filter_map(|rec| {
            let record = rec?;
            let recency_score = match window_end {
                Some(end) => {
                    // Linear ramp: memory at window.end → 1.0; at
                    // window.start → 0.0. Outside the window (future
                    // memories under as-of-T) → clamped to 0.0.
                    if let Some(start) = result.window.as_ref().map(|w| w.start) {
                        let span = (end - start).num_seconds().max(1) as f64;
                        let offset = (record.created_at - start).num_seconds() as f64;
                        (offset / span).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            // ISS-147 AC-3: Some(0.0) for FTS misses.
            let bm25 = bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0);
            let sub_scores = SubScores {
                recency_score: Some(recency_score),
                bm25_score: Some(bm25),
                ..Default::default()
            };
            let embedding = embeddings_by_id.get(record.id.as_str()).cloned();
            Some(ScoredResult::Memory {
                record,
                score: 0.0, // overwritten by fusion
                sub_scores,
                embedding,
            })
        })
        .collect()
}

/// Associative plan adapter: seed-expanded candidates → ScoredResult.
///
/// **Signals emitted**: `vector_score` (`seed_score`), `graph_score`
/// (derived from `edge_distance`: distance 0 → 1.0, 1 → 0.5, 2 → 0.25,
/// …), `bm25_score` (lexical lookup over the dispatched query text;
/// `Some(0.0)` for FTS misses per ISS-150 AC). Recency / actr stay
/// `None`.
///
/// **ISS-150**: BM25 was originally excluded here on the (incorrect)
/// assumption that Associative results never went through the
/// `combine()` fusion path. They do — Associative-as-fallback is
/// dispatched under the original intent (Factual / Episodic / …)
/// and `FusionConfig::locked()` gives those intents a non-zero
/// `text` weight, with `text_score = max(vector, bm25)`. Leaving
/// `bm25_score = None` silently collapsed the text channel to
/// `vector` for the ~80% of LoCoMo conv-26 queries that fall back
/// to Associative — see ISS-150 evidence section.
pub(crate) fn associative_to_scored(
    result: &crate::retrieval::plans::associative::AssociativePlanResult,
    loader: &dyn RecordLoader,
    bm25_by_id: &HashMap<String, f64>,
) -> Vec<ScoredResult> {
    if result.candidates.is_empty() {
        return Vec::new();
    }

    let ids: Vec<MemoryId> = result
        .candidates
        .iter()
        .map(|c| c.memory_id.clone())
        .collect();
    let records = loader.load_many(&ids);

    // ISS-139 Strategy A: batch-fetch embeddings so MMR sees diversity
    // on associative walks. Associative is the "find me memories
    // related to X" path — high risk of cluster-collapse on dense
    // topics (the same failure mode that motivated MMR in the first
    // place).
    let id_strs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    let embeddings_by_id = loader.load_embeddings(&id_strs);

    result
        .candidates
        .iter()
        .zip(records.into_iter())
        .filter_map(|(cand, rec)| {
            let record = rec?;
            // Distance → score: 1 / 2^d (0 hops = 1.0, 1 = 0.5, …).
            let graph_score = 1.0 / (1u32 << (cand.edge_distance.min(8) as u32)) as f64;
            // ISS-150: Some(0.0) for FTS misses (NOT None) — None
            // would trigger missing-signal renormalisation which
            // defeats the lexical channel's intent. Mirrors ISS-147
            // AC-3 on factual/episodic/affective adapters.
            let bm25 = bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0);
            let sub_scores = SubScores {
                vector_score: Some(cand.seed_score.clamp(0.0, 1.0)),
                graph_score: Some(graph_score),
                bm25_score: Some(bm25),
                ..Default::default()
            };
            let embedding = embeddings_by_id.get(record.id.as_str()).cloned();
            Some(ScoredResult::Memory {
                record,
                score: 0.0,
                sub_scores,
                embedding,
            })
        })
        .collect()
}

/// Abstract plan adapter: L5 topic candidates → ScoredResult::Topic.
///
/// **No `SubScores` populated** — Topic results carry their own
/// `score` (the topic-search score) and provenance (`source_memories`,
/// `contributing_entities`). Fusion preserves Topic scores as-is per
/// §5.2 ("topics keep their existing score").
pub(crate) fn abstract_to_scored(
    result: &crate::retrieval::plans::abstract_l5::AbstractPlanResult,
) -> Vec<ScoredResult> {
    result
        .candidates
        .iter()
        .map(|cand| ScoredResult::Topic {
            topic: cand.topic.clone(),
            score: cand.topic_score,
            source_memories: cand.source_memories.clone(),
            contributing_entities: cand.contributing_entities.iter().copied().collect(),
        })
        .collect()
}

/// Affective plan adapter: mood-congruent candidates → ScoredResult.
///
/// **Signals emitted**: `vector_score` (`text_score`),
/// `affect_similarity`, `recency_score`, `bm25_score` (ISS-147 —
/// `Some(0.0)` for non-FTS hits per §5.1). Graph / actr stay `None`.
pub(crate) fn affective_to_scored(
    result: &crate::retrieval::plans::affective::AffectivePlanResult,
    loader: &dyn RecordLoader,
    bm25_by_id: &HashMap<String, f64>,
) -> Vec<ScoredResult> {
    if result.candidates.is_empty() {
        return Vec::new();
    }

    let ids: Vec<MemoryId> = result
        .candidates
        .iter()
        .map(|c| c.memory_id.clone())
        .collect();
    let records = loader.load_many(&ids);

    // ISS-139 Strategy A: batch-fetch embeddings for MMR diversity on
    // affective queries. Mood-congruent candidates can cluster tightly
    // around a single emotional theme; MMR pulls in adjacent affect.
    let id_strs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    let embeddings_by_id = loader.load_embeddings(&id_strs);

    result
        .candidates
        .iter()
        .zip(records.into_iter())
        .filter_map(|(cand, rec)| {
            let record = rec?;
            // ISS-147 AC-3: Some(0.0) for FTS misses.
            let bm25 = bm25_by_id.get(record.id.as_str()).copied().unwrap_or(0.0);
            let sub_scores = SubScores {
                vector_score: Some(cand.text_score.clamp(0.0, 1.0)),
                recency_score: Some(cand.recency_score.clamp(0.0, 1.0)),
                affect_similarity: Some(cand.affect_similarity.clamp(0.0, 1.0)),
                bm25_score: Some(bm25),
                ..Default::default()
            };
            let embedding = embeddings_by_id.get(record.id.as_str()).cloned();
            Some(ScoredResult::Memory {
                record,
                score: 0.0,
                sub_scores,
                embedding,
            })
        })
        .collect()
}

/// Hybrid plan adapter: ranked heterogeneous items → ScoredResult.
///
/// Hybrid produces `RankedHybridItem { item: HybridItem, rrf_score }`
/// where `HybridItem` is either a memory id or topic UUID. The
/// orchestrator hydrates each into a `ScoredResult::Memory` or
/// `ScoredResult::Topic` row.
///
/// **Score**: the RRF score from Hybrid is preserved on both variants.
/// Fusion treats Hybrid results as already-fused and does not re-weight
/// them (the fusion module's per-intent weights would double-count RRF
/// signals). This is enforced by the calling site in
/// [`execute_plan`] — Hybrid output bypasses `fuse_and_rank`.
///
/// **SubScores**: empty (default) for Memory variants — Hybrid does
/// not surface the underlying signal scores. Future work
/// (`task:retr-impl-explain-trace`) will plumb these through the
/// trace.
pub(crate) fn hybrid_to_scored(
    result: &crate::retrieval::plans::hybrid::HybridPlanResult,
    topics_by_uuid: &HashMap<Uuid, crate::graph::KnowledgeTopic>,
    loader: &dyn RecordLoader,
) -> Vec<ScoredResult> {
    use crate::retrieval::plans::hybrid::HybridItem;

    // ISS-139 Strategy A: collect Memory ids from the post-fusion
    // candidate list and batch-fetch their embeddings in one SQL call.
    // The cost is bounded — Hybrid's RRF output is already top-K
    // truncated (k_seed × ~2) before reaching this function, so the
    // IN-list stays well below SQLite's 999-variable cap.
    //
    // Missing embeddings (e.g. memory rows ingested before embedding
    // provider was enabled) are silently absent from the returned
    // map — MMR treats those candidates as "no diversity signal"
    // and ranks them by relevance only. This matches GUARD-9
    // ("missing data is not a retrieval failure").
    let memory_ids: Vec<&str> = result
        .items
        .iter()
        .filter_map(|ranked| match &ranked.item {
            HybridItem::Memory(id) => Some(id.as_str()),
            HybridItem::Topic(_) => None,
        })
        .collect();
    let embeddings_by_id = loader.load_embeddings(&memory_ids);

    result
        .items
        .iter()
        .filter_map(|ranked| match &ranked.item {
            HybridItem::Memory(id) => {
                let record = loader.load(id)?;
                let embedding = embeddings_by_id.get(id.as_str()).cloned();
                Some(ScoredResult::Memory {
                    record,
                    score: ranked.rrf_score,
                    sub_scores: SubScores::default(),
                    embedding,
                })
            }
            HybridItem::Topic(uuid) => {
                let topic = topics_by_uuid.get(uuid)?.clone();
                let source_memories = topic.source_memories.clone();
                let contributing_entities = topic.contributing_entities.clone();
                Some(ScoredResult::Topic {
                    topic,
                    score: ranked.rrf_score,
                    source_memories,
                    contributing_entities,
                })
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 3. HybridDispatchExecutor — runs sub-plans on behalf of HybridPlan
// ---------------------------------------------------------------------------
//
// Hybrid asks `executor.run(SubPlanKind)` for a ranked `HybridItem` list per
// signal that fired strongly. The executor must therefore hold every
// dependency that the four single-intent plans need: the graph store, the
// query, the budget, and (for Abstract) a `&str` namespace.
//
// **v0.3 collaborator slots are deferred.** The executor wires `Null*`
// implementations of every plan's recaller / resolver / store. That mirrors
// what `execute_plan` does for direct (non-Hybrid) plan dispatch — when a
// real recaller arrives in a later task, both call sites get upgraded
// together. Until then, Hybrid's `Null*` sub-plans return empty lists, and
// the RRF fusion step produces an empty `items` vec — which is the
// behaviorally-correct outcome for "no recall backend installed yet".

/// Concrete executor for [`HybridPlan`]. Holds the per-query state every
/// sub-plan needs, plus a mutable handle to the topic-provenance map the
/// orchestrator builds up for the Hybrid → `ScoredResult::Topic`
/// hydration step.
///
/// `run` dispatches on `SubPlanKind` and runs the corresponding plan with
/// `Null*` collaborators (deferred — see module note above). For each
/// sub-plan that produces topic candidates (only Abstract today), the
/// executor copies the resolved topics into `topics_by_uuid` so
/// [`hybrid_to_scored`] can find them after fusion.
///
/// Lifetimes are scoped to a single `execute_plan` call; the executor is
/// constructed there and dropped before the function returns.
pub(crate) struct HybridDispatchExecutor<'a> {
    /// Borrowed graph store — same handle used by the parent plan.
    pub graph: &'a dyn crate::graph::store::GraphRead,
    /// Echo of the user query — sub-plans need `min_confidence`,
    /// `as_of`, `limit`, etc.
    pub query: &'a crate::retrieval::api::GraphQuery,
    /// Reproducibility-pinned `now` (§5.4).
    pub now: chrono::DateTime<chrono::Utc>,
    /// Sub-plans take **owned** `BudgetController`. We hand each invocation
    /// a fresh `BudgetController::with_defaults()` clone — Hybrid budget
    /// accounting across sub-plans is a follow-up (`task:retr-impl-hybrid-budget`).
    /// Simpler than threading one `&mut` through four type-different plan
    /// signatures, and behaviorally identical until per-stage telemetry
    /// is wired through Hybrid (today the parent context's budget is the
    /// authoritative one and the sub-plan budgets are discarded).
    pub _factual_budget: &'a mut crate::retrieval::budget::BudgetController,
    /// Topics surfaced by Abstract sub-plan runs, keyed by `topic_id`.
    /// Populated as a side effect of `run(SubPlanKind::Abstract)` so the
    /// post-Hybrid `hybrid_to_scored` adapter can hydrate
    /// `ScoredResult::Topic` rows.
    pub topics_by_uuid: &'a mut HashMap<Uuid, crate::graph::KnowledgeTopic>,
    /// Optional self-state for the Affective sub-plan. `None` causes that
    /// sub-plan to surface `DowngradedNoSelfState`, which Hybrid renders
    /// as an empty `items` list — correct behavior when cognitive state
    /// isn't installed.
    pub self_state: Option<crate::graph::affect::SomaticFingerprint>,
    /// Real-data adapters for each plan-collaborator trait. Threaded in
    /// from `execute_plan` (ISS-049 phase 2) so Hybrid sub-plans read
    /// the same backends as direct dispatch.
    pub collaborators: &'a PlanCollaborators<'a>,
}

impl crate::retrieval::plans::hybrid::HybridSubPlanExecutor for HybridDispatchExecutor<'_> {
    fn run(
        &mut self,
        kind: crate::retrieval::plans::hybrid::SubPlanKind,
    ) -> crate::retrieval::plans::hybrid::SubPlanResult {
        use crate::retrieval::plans::hybrid::{HybridItem, SubPlanKind, SubPlanResult};

        log::info!(
            target: "engramai::retrieval",
            "hybrid_sub_plan ENTER sub_kind={:?}",
            kind,
        );

        let result = match kind {
            SubPlanKind::Factual => {
                let inputs = crate::retrieval::plans::factual::FactualPlanInputs {
                    query: &self.query.text,
                    query_time: self.query.query_time.unwrap_or(self.now),
                    as_of: self.query.as_of,
                    include_superseded: self.query.include_superseded,
                    min_confidence: self.query.min_confidence.map(|f| f as f32),
                    max_anchors: 5,
                    predicate_filter: None,
                    // Hard cap (design §4.1 envelope: 5 anchors × 100 =
                    // 500 candidates max). Operating point is the
                    // overfetch formula `α × requested_k / anchors`
                    // computed inside the plan (ISS-105).
                    memory_limit_per_entity: 100,
                    requested_k: self.query.limit,
                    entity_filter: self.query.entity_filter.as_deref(),
                    temporal_reservation: self.query.temporal_reservation_override,
                    query_time_range: crate::query_classifier::extract_time_range(
                        &self.query.text,
                    ),
                };
                let plan = crate::retrieval::plans::factual::FactualPlan::new();
                let resolver = self.collaborators.entity_resolver;
                let mut budget = crate::retrieval::budget::BudgetController::with_defaults();
                let exec_result = plan.execute(&inputs, resolver, self.graph, &mut budget);
                // ISS-063 diagnostic: surface the plan-local outcome before
                // it gets discarded by `SubPlanResult` (which only carries
                // items, not outcome). Without this log a Factual sub-plan
                // that returned 0 because of `NoEntityFound` is
                // indistinguishable from one that found nothing organically.
                let factual_outcome_slug = match &exec_result {
                    Ok(r) => format!("{:?}", r.outcome),
                    Err(e) => format!("err:{e}"),
                };
                let result = exec_result.ok();
                let items: Vec<HybridItem> = result
                    .map(|r| {
                        r.memories
                            .into_iter()
                            .map(|m| HybridItem::Memory(m.memory_id))
                            .collect()
                    })
                    .unwrap_or_default();
                log::info!(
                    target: "engramai::retrieval",
                    "hybrid_sub_plan_outcome sub_kind=Factual outcome={} items={}",
                    factual_outcome_slug,
                    items.len(),
                );
                SubPlanResult { kind, items }
            }
            SubPlanKind::Episodic => {
                let plan = crate::retrieval::plans::episodic::EpisodicPlan::new(
                    self.collaborators.episodic_store,
                    crate::retrieval::plans::episodic::KnowledgeCutoff::default(),
                );
                let inputs = crate::retrieval::plans::episodic::EpisodicPlanInputs {
                    query: self.query,
                    time_window: self.query.time_window.clone(),
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                };
                let result = plan.execute(inputs, self.now);
                // ISS-063 diagnostic — see Factual arm above.
                log::info!(
                    target: "engramai::retrieval",
                    "hybrid_sub_plan_outcome sub_kind=Episodic outcome={:?} items={}",
                    result.outcome,
                    result.memories.len(),
                );
                let items: Vec<HybridItem> = result
                    .memories
                    .into_iter()
                    .map(HybridItem::Memory)
                    .collect();
                SubPlanResult { kind, items }
            }
            SubPlanKind::Abstract => {
                let plan = crate::retrieval::plans::abstract_l5::AbstractPlan::new(
                    self.collaborators.topic_searcher,
                );
                let inputs = crate::retrieval::plans::abstract_l5::AbstractPlanInputs {
                    query: self.query,
                    // ISS-059: thread per-query namespace from `GraphQuery`
                    // so Hybrid's Abstract sub-plan reads the same namespace
                    // as the real adapters constructed in `Memory::graph_query`.
                    namespace: self.query.namespace.as_deref().unwrap_or("default"),
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                };
                let result = plan.execute(inputs, self.graph);
                // ISS-063 diagnostic — see Factual arm above. This is the
                // log line that would have surfaced ISS-060/ISS-061's
                // root cause on first look (Abstract emitting
                // DowngradedL5Unavailable inside Hybrid is currently
                // invisible from the outside).
                log::info!(
                    target: "engramai::retrieval",
                    "hybrid_sub_plan_outcome sub_kind=Abstract outcome={:?} items={}",
                    result.outcome,
                    result.candidates.len(),
                );
                let items: Vec<HybridItem> = result
                    .candidates
                    .iter()
                    .map(|c| {
                        // Side-effect: stash the topic so the parent
                        // `hybrid_to_scored` can hydrate it post-fusion.
                        self.topics_by_uuid
                            .entry(c.topic.topic_id)
                            .or_insert_with(|| c.topic.clone());
                        HybridItem::Topic(c.topic.topic_id)
                    })
                    .collect();
                SubPlanResult { kind, items }
            }
            SubPlanKind::Affective => {
                let plan = crate::retrieval::plans::affective::AffectivePlan::new(
                    self.collaborators.affective_recaller,
                );
                let inputs = crate::retrieval::plans::affective::AffectivePlanInputs {
                    query: self.query,
                    self_state: self.self_state,
                    budget: crate::retrieval::budget::BudgetController::with_defaults(),
                    // Deterministic roll: no telemetry sampling for Hybrid
                    // sub-plans in v0.3 (the parent Hybrid run is what
                    // surfaces in the trace, not the inner Affective).
                    divergence_roll: 1.0,
                };
                let result = plan.execute(inputs);
                // ISS-063 diagnostic — see Factual arm above.
                log::info!(
                    target: "engramai::retrieval",
                    "hybrid_sub_plan_outcome sub_kind=Affective outcome={:?} items={}",
                    result.outcome,
                    result.candidates.len(),
                );
                let items: Vec<HybridItem> = result
                    .candidates
                    .into_iter()
                    .map(|c| HybridItem::Memory(c.memory_id))
                    .collect();
                SubPlanResult { kind, items }
            }
        };

        log::info!(
            target: "engramai::retrieval",
            "hybrid_sub_plan EXIT  sub_kind={:?} items={}",
            result.kind,
            result.items.len(),
        );

        result
    }
}

// ---------------------------------------------------------------------------
// 4. execute_plan — the central dispatch from `DispatchedQuery` → results
// ---------------------------------------------------------------------------

/// Final stage of the orchestrator pipeline: run the dispatched plan and
/// return pre-fusion candidates plus the typed outcome (§6.4).
///
/// Returns:
/// - `Vec<ScoredResult>` — pre-fusion (or, for Hybrid, *post-RRF*; Hybrid
///   bypasses [`fuse_and_rank`] per §5.2). Caller is responsible for the
///   per-intent fusion pass and top-K cutoff.
/// - [`RetrievalOutcome`] — typed success / downgrade surface from the
///   plan's local `*Outcome` enum, lifted via `to_retrieval_outcome`.
///
/// **Why a free function, not a method on `Memory`?** The orchestrator
/// surface is internal; making it a `Memory` method would invite
/// downstream code to call into plan execution directly, bypassing
/// dispatch / classifier wiring. Keeping it free pinned to `pub(crate)`
/// preserves the single public entry point at
/// [`Memory::graph_query`](crate::memory::Memory::graph_query).
///
/// **Mutex extraction.** [`PlanContext`] holds `Arc<Mutex<BudgetController>>`
/// to keep the door open for Hybrid fan-out. The single-plan path here
/// `lock()`s and `mem::replace`s the inner controller with a fresh default
/// — plans take owned `BudgetController` (Episodic / Associative /
/// Abstract / Affective) or `&mut` (Factual). Post-execution the original
/// (now-mutated) controller is dropped; the dispatch context is consumed
/// once per query so this is safe.
pub(crate) fn execute_plan(
    dispatched: crate::retrieval::dispatch::DispatchedQuery,
    graph: &dyn crate::graph::store::GraphRead,
    loader: &dyn RecordLoader,
    collaborators: &PlanCollaborators<'_>,
    self_state: Option<crate::graph::affect::SomaticFingerprint>,
) -> (
    Vec<crate::retrieval::api::ScoredResult>,
    crate::retrieval::api::RetrievalOutcome,
) {
    use crate::retrieval::dispatch::PlanKind;

    let crate::retrieval::dispatch::DispatchedQuery {
        plan_kind,
        context,
        query,
        signal_scores,
        ..
    } = dispatched;

    let now = query.query_time.unwrap_or_else(chrono::Utc::now);

    // ISS-049-followup diagnostics: per-plan execution log so callers
    // tailing `RUST_LOG=engramai::retrieval=info` can distinguish
    //   (a) plan never dispatched (no log line at all)
    //   (b) plan dispatched, returned 0 candidates (count=0 line)
    //   (c) plan dispatched, returned N>0 candidates (count=N line)
    //   (d) plan downgraded (outcome reflects the downgrade variant)
    // Query text is truncated to 80 chars to keep log lines bounded;
    // the full query is on the caller's side and can be correlated by
    // timestamp + plan_kind if needed.
    let q_log: String = query.text.chars().take(80).collect();
    log::info!(
        target: "engramai::retrieval",
        "execute_plan ENTER plan_kind={} query_limit={} query=\"{}\"",
        plan_kind.as_str(),
        query.limit,
        q_log,
    );

    // ISS-147: fetch BM25 scores for the lexical fusion channel
    // ONCE per query (single SQL round-trip) and pass the resulting
    // id→score map into every Memory-emitting adapter below. Adapters
    // fall back to `Some(0.0)` for ids that didn't match the FTS query
    // (AC-3: must be Some(0.0) not None to preserve weight-mass
    // semantics). For loaders without an FTS backing (test fakes /
    // HashMapLoader) the default trait impl returns an empty map and
    // every candidate gets Some(0.0) → BM25 channel is uniformly zero
    // and fusion behaves identically to the pre-ISS-147 embed-only
    // path. K_seed mirrors the Episodic adapter convention
    // (`limit * 4`, clamped to ≥ 40) — large enough to cover
    // overfetched candidate pools without ballooning into 1000+ rows
    // for huge `limit` values.
    // ISS-152: per-query `bm25_pool_override` lets the bench driver
    // widen the BM25 precompute pool for pool-sizing experiments
    // without recompiling. `None` falls back to the existing default.
    let bm25_pool = query
        .bm25_pool_override
        .unwrap_or_else(|| (query.limit.saturating_mul(4)).max(40));
    let bm25_by_id: HashMap<String, f64> = loader.fts_scores(&query.text, bm25_pool);

    // ISS-172: embed query ONCE per execute_plan call so plans whose
    // scoring stage previously emitted no `vector_score` (Factual is
    // the dominant case) can compute `cosine(query, memory_embedding)`
    // against the embeddings already batch-loaded for MMR (ISS-139
    // Strategy A). Plans that already emit a semantic score from
    // their own seed_recaller (Associative, Affective) are unaffected
    // — they read from PlanCollaborators directly.
    //
    // Cost: one `provider.embed(query.text)` per query, matching what
    // HybridSeedRecaller / HybridAffectiveSeedRecaller already pay on
    // their hot paths. `None` when no embedder is wired (test fakes,
    // HashMapLoader paths) → factual_to_scored falls back to
    // graph + BM25 only (legacy pre-ISS-172 behaviour).
    let query_embedding: Option<Vec<f32>> = collaborators
        .embedding_provider
        .and_then(|p| p.embed(&query.text).ok());

    // Extract the budget controller out of the Arc<Mutex<_>>. Single
    // owner here — Hybrid sub-plans construct their own internally.
    let mut budget = match context.budget.lock() {
        Ok(mut guard) => std::mem::replace(
            &mut *guard,
            crate::retrieval::budget::BudgetController::with_defaults(),
        ),
        Err(_) => {
            // Mutex poisoned — surface as Internal-shaped outcome by
            // returning an empty result set; the caller's `Err(...)`
            // wrapping is at the `Memory::graph_query` layer.
            return (Vec::new(), crate::retrieval::api::RetrievalOutcome::Ok);
        }
    };

    let (scored, outcome) = match plan_kind {
        PlanKind::Factual => {
            let inputs = crate::retrieval::plans::factual::FactualPlanInputs {
                query: &query.text,
                query_time: now,
                as_of: query.as_of,
                include_superseded: query.include_superseded,
                min_confidence: query.min_confidence.map(|f| f as f32),
                max_anchors: 5,
                predicate_filter: None,
                // Hard cap; operating point set by overfetch formula
                // inside the plan (ISS-105).
                memory_limit_per_entity: 100,
                requested_k: query.limit,
                entity_filter: query.entity_filter.as_deref(),
                temporal_reservation: query.temporal_reservation_override,
                query_time_range: crate::query_classifier::extract_time_range(&query.text),
            };
            let plan = crate::retrieval::plans::factual::FactualPlan::new();
            let resolver = collaborators.entity_resolver;
            let (scored, primary_outcome, fallback_reason): (
                Vec<crate::retrieval::api::ScoredResult>,
                crate::retrieval::api::RetrievalOutcome,
                Option<&'static str>,
            ) = match plan.execute(&inputs, resolver, graph, &mut budget) {
                Ok(result) => {
                    let scored =
                        factual_to_scored(&result, loader, &bm25_by_id, query_embedding.as_deref());
                    let outcome = result.outcome.to_retrieval_outcome(scored.is_empty());
                    // ISS-063: pick fallback trigger reason based on the
                    // typed outcome variant. Empty `scored` is necessary
                    // but not sufficient — `Cutoff` should not fall back
                    // (we have a partial result the budget aborted).
                    let reason = if scored.is_empty() {
                        match &outcome {
                            crate::retrieval::api::RetrievalOutcome::NoEntityFound { .. } => {
                                Some("no_entity_resolved")
                            }
                            crate::retrieval::api::RetrievalOutcome::EntityFoundNoEdges {
                                ..
                            } => Some("entity_found_no_edges"),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    (scored, outcome, reason)
                }
                // Storage error → fall back to Associative per design
                // §3.4 ("zero resolved anchors → Associative mid-flight").
                // The error itself is dropped here; ISS-062 / budget-cutoff
                // followup will plumb the detail into `RetrievalOutcome`.
                Err(_) => (
                    Vec::new(),
                    crate::retrieval::api::RetrievalOutcome::EntityFoundNoEdges {
                        entities: vec![],
                    },
                    Some("factual_storage_error"),
                ),
            };
            if let Some(reason) = fallback_reason {
                run_associative_fallback(
                    &query,
                    graph,
                    loader,
                    collaborators,
                    FallbackTrigger::Factual { reason },
                )
            } else {
                (scored, primary_outcome)
            }
        }
        PlanKind::Episodic => {
            let inputs = crate::retrieval::plans::episodic::EpisodicPlanInputs {
                query: &query,
                time_window: query.time_window.clone(),
                budget,
            };
            let plan = crate::retrieval::plans::episodic::EpisodicPlan::new(
                collaborators.episodic_store,
                crate::retrieval::plans::episodic::KnowledgeCutoff::default(),
            );
            let result = plan.execute(inputs, now);
            let scored = episodic_to_scored(&result, loader, &bm25_by_id);
            let outcome = result.outcome.to_retrieval_outcome(scored.is_empty());
            // ISS-063: trigger fallback when Episodic emits its
            // downgrade variant (`DowngradedFromEpisodic`) OR when
            // `NoMemoriesInWindow` produced 0 results. `Ok` with
            // 0 results is a contract violation we also catch.
            let fallback_reason: Option<&'static str> = if scored.is_empty() {
                match &outcome {
                    crate::retrieval::api::RetrievalOutcome::DowngradedFromEpisodic { .. } => {
                        Some("downgraded_from_episodic")
                    }
                    crate::retrieval::api::RetrievalOutcome::NoMemoriesInWindow { .. } => {
                        Some("no_memories_in_window")
                    }
                    crate::retrieval::api::RetrievalOutcome::Ok => Some("episodic_empty"),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(reason) = fallback_reason {
                run_associative_fallback(
                    &query,
                    graph,
                    loader,
                    collaborators,
                    FallbackTrigger::Episodic { reason },
                )
            } else {
                (scored, outcome)
            }
        }
        PlanKind::Associative => {
            // ISS-164 — resolve the always-on entity channel flag.
            // Per-query override wins, else fall back to
            // FusionConfig::locked().entity_channel_enabled (default
            // false → byte-identical to pre-ISS-164 §4.3 pipeline).
            let entity_channel_enabled = query.entity_channel_override.unwrap_or_else(|| {
                crate::retrieval::fusion::FusionConfig::locked().entity_channel_enabled
            });
            let inputs = crate::retrieval::plans::associative::AssociativePlanInputs {
                query: &query,
                budget,
                entity_channel_enabled,
                entity_resolver: Some(collaborators.entity_resolver),
            };
            // ISS-K_SEED-CAP (2026-05-06): K_seed default = 10 silently
            // caps the fused candidate pool at ~10 even when the driver
            // requests larger top-K (RUN-0020 K=15 → 145/152 queries got
            // exactly 10 candidates, jsonl-verified). Surface k_seed via
            // query.limit so retrieval can actually saturate the
            // requested top-K. K_pool default 100 is wide enough to
            // absorb seed counts up to ~33 without breaking the §4.3
            // expansion budget; if query.limit > K_pool we don't bother
            // raising the pool — clamping at the existing pool keeps
            // the §7.3 cost caps intact.
            // ISS-152: `k_seed_override` lets the bench driver widen
            // the Associative seed pool for pool-sizing experiments.
            // `None` falls back to `query.limit` (the existing default).
            let plan = crate::retrieval::plans::associative::AssociativePlan::new(
                collaborators.seed_recaller,
            )
            .with_k_seed(query.k_seed_override.unwrap_or(query.limit));
            let result = plan.execute(inputs, graph);
            // ISS-150: thread bm25_by_id (computed once at execute_plan
            // entry) into the Associative adapter so the dispatched
            // intent's text-weighted fusion sees the lexical channel.
            let scored = associative_to_scored(&result, loader, &bm25_by_id);
            // ISS-063: Associative is the terminal plan. If it returns
            // empty, the entire fallback chain is exhausted —
            // `EmptyResultSet`, NOT `Ok`.
            let outcome = if scored.is_empty() {
                crate::retrieval::api::RetrievalOutcome::EmptyResultSet {
                    reason: "associative_empty".to_string(),
                }
            } else {
                crate::retrieval::api::RetrievalOutcome::Ok
            };
            (scored, outcome)
        }
        PlanKind::Abstract => {
            let inputs = crate::retrieval::plans::abstract_l5::AbstractPlanInputs {
                query: &query,
                // ISS-059: thread per-query namespace from `GraphQuery` so the
                // direct Abstract dispatch reads the same namespace as the
                // real `GraphTopicSearcher` adapter wired in
                // `Memory::graph_query`.
                namespace: query.namespace.as_deref().unwrap_or("default"),
                budget,
            };
            let plan = crate::retrieval::plans::abstract_l5::AbstractPlan::new(
                collaborators.topic_searcher,
            );
            let result = plan.execute(inputs, graph);
            let scored = abstract_to_scored(&result);
            let (primary_outcome, fallback_reason): (
                crate::retrieval::api::RetrievalOutcome,
                Option<&'static str>,
            ) = match result.outcome {
                crate::retrieval::plans::abstract_l5::AbstractOutcome::Ok if !scored.is_empty() => {
                    (crate::retrieval::api::RetrievalOutcome::Ok, None)
                }
                crate::retrieval::plans::abstract_l5::AbstractOutcome::DowngradedL5Unavailable => (
                    crate::retrieval::api::RetrievalOutcome::DowngradedFromAbstract {
                        reason: "L5_unavailable".to_string(),
                    },
                    Some("l5_unavailable"),
                ),
                _ => (
                    crate::retrieval::api::RetrievalOutcome::L5NotReady {
                        missing_topic_domains: vec![],
                    },
                    Some("l5_not_ready"),
                ),
            };
            if let Some(reason) = fallback_reason {
                run_associative_fallback(
                    &query,
                    graph,
                    loader,
                    collaborators,
                    FallbackTrigger::Abstract { reason },
                )
            } else {
                (scored, primary_outcome)
            }
        }
        PlanKind::Affective => {
            let inputs = crate::retrieval::plans::affective::AffectivePlanInputs {
                query: &query,
                self_state,
                budget,
                divergence_roll: 1.0,
            };
            let plan = crate::retrieval::plans::affective::AffectivePlan::new(
                collaborators.affective_recaller,
            );
            let result = plan.execute(inputs);
            let scored = affective_to_scored(&result, loader, &bm25_by_id);
            let (primary_outcome, fallback_reason): (
                crate::retrieval::api::RetrievalOutcome,
                Option<&'static str>,
            ) = match result.outcome {
                crate::retrieval::plans::affective::AffectiveOutcome::Ok if !scored.is_empty() => {
                    (crate::retrieval::api::RetrievalOutcome::Ok, None)
                }
                crate::retrieval::plans::affective::AffectiveOutcome::DowngradedNoSelfState => (
                    crate::retrieval::api::RetrievalOutcome::NoCognitiveState,
                    Some("no_self_state"),
                ),
                _ => (
                    crate::retrieval::api::RetrievalOutcome::Ok,
                    Some("affective_empty"),
                ),
            };
            if let Some(reason) = fallback_reason {
                run_associative_fallback(
                    &query,
                    graph,
                    loader,
                    collaborators,
                    FallbackTrigger::Affective { reason },
                )
            } else {
                (scored, primary_outcome)
            }
        }
        PlanKind::Hybrid => {
            // Hybrid needs the classifier signal scores — without them
            // we cannot pick sub-plans. (`CallerOverride` for Hybrid
            // skips Stage 1; treat as "all signals zero" → no sub-plans
            // selected → empty result.)
            // Caller-override path skips Stage 1, so `signal_scores`
            // is None — treat as all-zero so no sub-plans are selected.
            let signals = signal_scores.unwrap_or_else(|| {
                crate::retrieval::classifier::heuristic::SignalScores::from_primary(
                    0.0, 0.0, 0.0, 0.0,
                )
            });
            let mut topics_by_uuid: HashMap<Uuid, crate::graph::KnowledgeTopic> = HashMap::new();
            let mut executor = HybridDispatchExecutor {
                graph,
                query: &query,
                now,
                _factual_budget: &mut budget,
                topics_by_uuid: &mut topics_by_uuid,
                self_state,
                collaborators,
            };
            let inputs = crate::retrieval::plans::hybrid::HybridPlanInputs {
                signals: &signals,
                tau_high: 0.7,
                top_k: query.limit,
            };
            let hybrid_plan = crate::retrieval::plans::hybrid::HybridPlan::new();
            let result = hybrid_plan.execute(inputs, &mut executor);
            let scored = hybrid_to_scored(&result, &topics_by_uuid, loader);
            // ISS-083: when every Hybrid sub-plan returned empty, try a
            // Factual re-dispatch of the same query before surfacing
            // `EmptyResultSet`. Factual is the always-available best-
            // effort plan; if it also returns empty we fall through to
            // a terminal `EmptyResultSet` with an updated reason. This
            // replaces the ISS-063 placeholder that always emitted
            // `hybrid_all_subplans_empty`.
            if scored.is_empty() {
                run_factual_fallback_for_hybrid(&query, now, graph, loader, collaborators)
            } else {
                (scored, crate::retrieval::api::RetrievalOutcome::Ok)
            }
        }
    };

    log::info!(
        target: "engramai::retrieval",
        "execute_plan EXIT  plan_kind={} candidates={} outcome={}",
        plan_kind.as_str(),
        scored.len(),
        outcome.slug(),
    );

    (scored, outcome)
}

// ---------------------------------------------------------------------------
// 4a. Associative fallback — design §3.4 / §6.4 (ISS-063)
// ---------------------------------------------------------------------------
//
// Every non-Associative plan that produces an empty `scored` re-runs the
// query through the Associative plan ("downgrade to Associative"). This
// implements the contract design §3.4 makes: "Plans may further downgrade
// at execution time (e.g., `Factual` with zero resolved anchors →
// `Associative` mid-flight)".
//
// **Depth = 1.** If the Associative fallback itself returns empty,
// `EmptyResultSet { reason }` is the terminal outcome — no further
// fallback chain (Associative IS the v0.2 known-good baseline).
//
// **Hybrid is intentionally excluded.** Sub-plan fallback inside Hybrid
// is filed as a separate concern (ISS-061) — the Hybrid-empty symptom
// may live in `hybrid_to_scored` ID mapping, not fallback routing, and
// that needs to be diagnosed before we layer fallback on top.

/// ISS-083 — run the Factual plan as a best-effort rescue when every
/// Hybrid sub-plan returned empty. Mirrors `run_associative_fallback`
/// in shape: fresh `BudgetController`, same loader, returns
/// `(scored, outcome)`.
///
/// Outcomes:
/// - Factual returns non-empty → `(scored, DowngradedFromHybrid {
///   reason: "subplans_empty_factual_recovered" })`.
/// - Factual is also empty (or errors) → `(vec![], EmptyResultSet {
///   reason: "hybrid_subplans_empty_factual_also_empty" })` (terminal,
///   no further fallback per ISS-083 spec).
fn run_factual_fallback_for_hybrid(
    query: &crate::retrieval::api::GraphQuery,
    now: chrono::DateTime<chrono::Utc>,
    graph: &dyn crate::graph::store::GraphRead,
    loader: &dyn RecordLoader,
    collaborators: &PlanCollaborators<'_>,
) -> (
    Vec<crate::retrieval::api::ScoredResult>,
    crate::retrieval::api::RetrievalOutcome,
) {
    log::info!(
        target: "engramai::retrieval",
        "fallback ENTER trigger=hybrid_to_factual reason=subplans_empty",
    );

    // Fresh budget — Hybrid sub-plans consumed the original allocation.
    // Same rationale as `run_associative_fallback`: a slightly larger
    // envelope is preferable to surfacing 0 candidates.
    let mut budget = crate::retrieval::budget::BudgetController::with_defaults();

    // Mirror the `PlanKind::Factual` arm's input construction so the
    // rescue dispatch sees identical configuration.
    let inputs = crate::retrieval::plans::factual::FactualPlanInputs {
        query: &query.text,
        query_time: now,
        as_of: query.as_of,
        include_superseded: query.include_superseded,
        min_confidence: query.min_confidence.map(|f| f as f32),
        max_anchors: 5,
        predicate_filter: None,
        // Hard cap; operating point set by overfetch formula inside
        // the plan (ISS-105).
        memory_limit_per_entity: 100,
        requested_k: query.limit,
        entity_filter: query.entity_filter.as_deref(),
        temporal_reservation: query.temporal_reservation_override,
        query_time_range: crate::query_classifier::extract_time_range(&query.text),
    };
    let plan = crate::retrieval::plans::factual::FactualPlan::new();
    let resolver = collaborators.entity_resolver;
    // ISS-147: fetch BM25 scores for the fallback's own query so the
    // re-dispatch's adapter populates `bm25_score` consistently with
    // the main `execute_plan` path.
    // ISS-152: same per-query override hook as the main path.
    let bm25_pool = query
        .bm25_pool_override
        .unwrap_or_else(|| (query.limit.saturating_mul(4)).max(40));
    let bm25_by_id: HashMap<String, f64> = loader.fts_scores(&query.text, bm25_pool);
    // ISS-172: embed the fallback's query so the recovered factual
    // pool gets the same semantic ranking as the main `execute_plan`
    // path. Same Option-degrades-to-None convention.
    let query_embedding: Option<Vec<f32>> = collaborators
        .embedding_provider
        .and_then(|p| p.embed(&query.text).ok());
    let scored = match plan.execute(&inputs, resolver, graph, &mut budget) {
        Ok(result) => factual_to_scored(&result, loader, &bm25_by_id, query_embedding.as_deref()),
        Err(_) => Vec::new(),
    };

    let final_outcome = if scored.is_empty() {
        crate::retrieval::api::RetrievalOutcome::EmptyResultSet {
            reason: "hybrid_subplans_empty_factual_also_empty".to_string(),
        }
    } else {
        crate::retrieval::api::RetrievalOutcome::DowngradedFromHybrid {
            reason: "subplans_empty_factual_recovered".to_string(),
        }
    };

    log::info!(
        target: "engramai::retrieval",
        "fallback EXIT  candidates={} outcome={}",
        scored.len(),
        final_outcome.slug(),
    );

    (scored, final_outcome)
}

/// Identifies which primary plan triggered a fallback to Associative.
/// Carries the reason string so the synthesised `RetrievalOutcome` can
/// surface *why* the primary was empty.
enum FallbackTrigger {
    Factual { reason: &'static str },
    Episodic { reason: &'static str },
    Abstract { reason: &'static str },
    Affective { reason: &'static str },
}

/// Run the Associative plan as a fallback for a primary plan that
/// produced no candidates. Returns `(scored, outcome)`:
///
/// - If Associative finds candidates → `(scored, DowngradedFrom*)`
///   carrying the original trigger reason.
/// - If Associative is also empty → `(vec![], EmptyResultSet { reason })`
///   with `reason` identifying the full path
///   (`"factual_then_associative_empty"`, etc.).
fn run_associative_fallback(
    query: &crate::retrieval::api::GraphQuery,
    graph: &dyn crate::graph::store::GraphRead,
    loader: &dyn RecordLoader,
    collaborators: &PlanCollaborators<'_>,
    trigger: FallbackTrigger,
) -> (
    Vec<crate::retrieval::api::ScoredResult>,
    crate::retrieval::api::RetrievalOutcome,
) {
    log::info!(
        target: "engramai::retrieval",
        "fallback ENTER trigger={} reason={}",
        match &trigger {
            FallbackTrigger::Factual { .. } => "factual",
            FallbackTrigger::Episodic { .. } => "episodic",
            FallbackTrigger::Abstract { .. } => "abstract",
            FallbackTrigger::Affective { .. } => "affective",
        },
        match &trigger {
            FallbackTrigger::Factual { reason }
            | FallbackTrigger::Episodic { reason }
            | FallbackTrigger::Abstract { reason }
            | FallbackTrigger::Affective { reason } => reason,
        },
    );

    // Fresh budget — primary plan consumed its allocation. Per design
    // §7.3 cost caps we accept that fallback may push past the original
    // budget envelope; surfacing 0 candidates would be a worse contract
    // violation than a slightly larger budget.
    let budget = crate::retrieval::budget::BudgetController::with_defaults();
    // ISS-164 — same resolution as the primary Associative branch:
    // per-query override wins, else FusionConfig::locked() default
    // (currently false). Fallback paths must mirror the main path so
    // an A/B sweep produces the same on/off semantics whether the
    // primary plan dispatched Associative directly or downgraded into
    // it.
    let entity_channel_enabled = query
        .entity_channel_override
        .unwrap_or_else(|| crate::retrieval::fusion::FusionConfig::locked().entity_channel_enabled);
    let inputs = crate::retrieval::plans::associative::AssociativePlanInputs {
        query,
        budget,
        entity_channel_enabled,
        entity_resolver: Some(collaborators.entity_resolver),
    };
    // ISS-K_SEED-CAP — fallback path. Same reasoning as the primary
    // PlanKind::Associative branch above: surface query.limit as
    // k_seed so the fused pool can actually saturate top-K.
    // ISS-152: same per-query override hook as the main path.
    let plan =
        crate::retrieval::plans::associative::AssociativePlan::new(collaborators.seed_recaller)
            .with_k_seed(query.k_seed_override.unwrap_or(query.limit));
    let result = plan.execute(inputs, graph);

    // ISS-150: recompute BM25 here (analogous to
    // `run_factual_fallback_for_hybrid` at line ~1343). Threading the
    // outer `bm25_by_id` through 4 fallback call sites is noisier than
    // one extra SQL roundtrip per downgrade. Pool sizing matches the
    // ISS-147 primary-path convention `(K*4).max(40)`.
    // ISS-152: same per-query override hook as the main path.
    let bm25_pool = query
        .bm25_pool_override
        .unwrap_or_else(|| (query.limit * 4).max(40));
    let bm25_by_id: HashMap<String, f64> = loader.fts_scores(&query.text, bm25_pool);
    let scored = associative_to_scored(&result, loader, &bm25_by_id);

    let (outcome_label, empty_label) = match &trigger {
        FallbackTrigger::Factual { reason } => (
            crate::retrieval::api::RetrievalOutcome::DowngradedFromFactual {
                reason: (*reason).to_string(),
            },
            "factual_then_associative_empty",
        ),
        FallbackTrigger::Episodic { reason } => (
            crate::retrieval::api::RetrievalOutcome::DowngradedFromEpisodic {
                reason: (*reason).to_string(),
            },
            "episodic_then_associative_empty",
        ),
        FallbackTrigger::Abstract { reason } => (
            crate::retrieval::api::RetrievalOutcome::DowngradedFromAbstract {
                reason: (*reason).to_string(),
            },
            "abstract_then_associative_empty",
        ),
        FallbackTrigger::Affective { reason: _ } => (
            // Affective downgrade keeps its `NoCognitiveState` tag
            // because §6.4 specifies that as the affective downgrade
            // surface (rather than a generic `DowngradedFromAffective`).
            crate::retrieval::api::RetrievalOutcome::NoCognitiveState,
            "affective_then_associative_empty",
        ),
    };

    let final_outcome = if scored.is_empty() {
        crate::retrieval::api::RetrievalOutcome::EmptyResultSet {
            reason: empty_label.to_string(),
        }
    } else {
        outcome_label
    };

    log::info!(
        target: "engramai::retrieval",
        "fallback EXIT  candidates={} outcome={}",
        scored.len(),
        final_outcome.slug(),
    );

    (scored, final_outcome)
}

// ---------------------------------------------------------------------------
// 5. Tests
// ---------------------------------------------------------------------------
//
// End-to-end coverage of `Memory::graph_query` lives in `api.rs` (it
// exercises the dispatch + execute + fusion stack against a real
// `SqliteGraphStore`). Orchestrator-only tests would need a hand-rolled
// `GraphRead` stub for ~25 trait methods; the `api.rs` test path covers
// the same code with one fewer layer of indirection, so we don't
// duplicate here. When per-plan deeper orchestrator tests are needed
// (e.g. driving Hybrid topic provenance side-effects), the
// `crate::graph::test_helpers::fresh_conn()` + `SqliteGraphStore` setup
// is the established pattern — see `retrieval/api.rs`
// `graph_query_with_empty_graph_returns_typed_outcome`.
//
// One exception: `StorageLoader::load_embeddings` is a thin adapter
// over `Storage::get_embeddings_for_ids` (ISS-139) and has no GraphRead
// dependency. We pin its three end-to-end paths directly here.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use crate::types::{MemoryLayer, MemoryRecord, MemoryType};
    use chrono::Utc;
    use tempfile::TempDir;

    const MODEL: &str = "ollama/nomic-embed-text";

    fn open_storage() -> (TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loader.db");
        let storage = Storage::new(&path).expect("open storage");
        (dir, storage)
    }

    fn seed_embedding(s: &mut Storage, id: &str, emb: &[f32]) {
        let now = Utc::now();
        let mut rec = MemoryRecord {
            id: id.to_string(),
            content: format!("content for {id}"),
            memory_type: MemoryType::Factual,
            layer: MemoryLayer::Working,
            created_at: now,
            occurred_at: None,
            access_times: vec![now],
            working_strength: 1.0,
            core_strength: 0.0,
            importance: 0.5,
            pinned: false,
            consolidation_count: 0,
            last_consolidated: None,
            source: String::new(),
            contradicts: None,
            contradicted_by: None,
            superseded_by: None,
            metadata: None,
        };
        s.add(&mut rec, "default").expect("seed memory");
        s.store_embedding(id, emb, MODEL, emb.len())
            .expect("seed embedding");
    }

    #[test]
    fn storage_loader_load_embeddings_empty_input_short_circuits() {
        let (_dir, storage) = open_storage();
        let loader = StorageLoader::new(&storage, MODEL);

        let out = loader.load_embeddings(&[]);
        assert!(out.is_empty(), "empty ids → empty map, no SQL");
    }

    #[test]
    fn storage_loader_load_embeddings_returns_populated_map_for_live_rows() {
        let (_dir, mut storage) = open_storage();
        seed_embedding(&mut storage, "m1", &[0.1, 0.2]);
        seed_embedding(&mut storage, "m2", &[0.3, 0.4]);

        let loader = StorageLoader::new(&storage, MODEL);
        let out = loader.load_embeddings(&["m1", "m2"]);

        assert_eq!(out.len(), 2);
        assert_eq!(out["m1"], vec![0.1f32, 0.2]);
        assert_eq!(out["m2"], vec![0.3f32, 0.4]);
    }

    #[test]
    fn storage_loader_load_embeddings_empty_model_short_circuits() {
        // The documented sentinel pattern (see line ~191): an empty
        // `model` means "no embeddings available for this namespace";
        // MMR degenerates to relevance-only via missing embeddings.
        let (_dir, mut storage) = open_storage();
        seed_embedding(&mut storage, "m1", &[0.1, 0.2]);

        let loader = StorageLoader::new(&storage, "");
        let out = loader.load_embeddings(&["m1"]);
        assert!(out.is_empty(), "empty model → empty map, never hits SQL");
    }

    #[test]
    fn storage_loader_load_embeddings_returns_only_matching_ids() {
        let (_dir, mut storage) = open_storage();
        seed_embedding(&mut storage, "m1", &[0.1, 0.2]);
        seed_embedding(&mut storage, "m2", &[0.3, 0.4]);

        let loader = StorageLoader::new(&storage, MODEL);
        // Ask for one present + one missing — map contains only present.
        let out = loader.load_embeddings(&["m1", "missing"]);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("m1"));
        assert!(!out.contains_key("missing"));
    }

    // ----- ISS-172 regression: factual_to_scored must emit vector_score
    // when a query embedding is provided. Pre-ISS-172 it was always
    // None, drowning the gold passage among 100+ flat-graph_score
    // candidates. -----

    fn fake_factual_result(
        anchors: usize,
        memories: &[(&str, usize)],
    ) -> crate::retrieval::plans::factual::FactualPlanResult {
        use crate::retrieval::plans::factual as f;
        use std::collections::BTreeSet;
        // Build `anchors` anchors with deterministic UUIDs so seen_via
        // sets line up.
        let anchor_ids: Vec<uuid::Uuid> = (0..anchors)
            .map(|i| uuid::Uuid::from_u128(0x0a00_0000_0000_0000_0000_0000_0000_0000 + i as u128))
            .collect();
        let resolved: Vec<f::ResolvedAnchor> = anchor_ids
            .iter()
            .map(|id| f::ResolvedAnchor {
                entity_id: *id,
                canonical_name: format!("anchor-{id}"),
                match_strength: 1.0,
            })
            .collect();
        let mem_rows: Vec<f::FactualMemoryRow> = memories
            .iter()
            .map(|(mid, n_anchors_seen)| f::FactualMemoryRow {
                memory_id: mid.to_string(),
                seen_via: anchor_ids
                    .iter()
                    .take(*n_anchors_seen)
                    .copied()
                    .collect::<BTreeSet<_>>(),
                edge_seeded: false,
            })
            .collect();
        f::FactualPlanResult {
            anchors: resolved,
            edges: Vec::new(),
            linked_entities: BTreeSet::new(),
            memories: mem_rows,
            outcome: f::FactualOutcome::Ok,
        }
    }

    // ISS-192 fix 3 helper: like `fake_factual_result` but each memory
    // carries an explicit `edge_seeded` flag. (Currently the edge-seed
    // scoring is unit-tested via the pure `edge_seeded_graph_score`
    // helper; this builder is kept for any future integration-level test
    // that needs a seeded FactualPlanResult.)
    #[allow(dead_code)]
    fn fake_factual_result_seeded(
        anchors: usize,
        memories: &[(&str, usize, bool)],
    ) -> crate::retrieval::plans::factual::FactualPlanResult {
        use crate::retrieval::plans::factual as f;
        use std::collections::BTreeSet;
        let anchor_ids: Vec<uuid::Uuid> = (0..anchors)
            .map(|i| uuid::Uuid::from_u128(0x0a00_0000_0000_0000_0000_0000_0000_0000 + i as u128))
            .collect();
        let resolved: Vec<f::ResolvedAnchor> = anchor_ids
            .iter()
            .map(|id| f::ResolvedAnchor {
                entity_id: *id,
                canonical_name: format!("anchor-{id}"),
                match_strength: 1.0,
            })
            .collect();
        let mem_rows: Vec<f::FactualMemoryRow> = memories
            .iter()
            .map(|(mid, n_anchors_seen, edge_seeded)| f::FactualMemoryRow {
                memory_id: mid.to_string(),
                seen_via: anchor_ids
                    .iter()
                    .take(*n_anchors_seen)
                    .copied()
                    .collect::<BTreeSet<_>>(),
                edge_seeded: *edge_seeded,
            })
            .collect();
        f::FactualPlanResult {
            anchors: resolved,
            edges: Vec::new(),
            linked_entities: BTreeSet::new(),
            memories: mem_rows,
            outcome: f::FactualOutcome::Ok,
        }
    }

    #[test]
    fn iss192_edge_seed_bonus_zero_is_byte_identical_breadth() {
        // bonus = 0.0 → edge_seeded flag is ignored, graph_score is pure
        // breadth. An edge-seeded candidate with breadth 1/3 must NOT
        // outrank a pure co-mention with breadth 3/3. Pure-function test:
        // no env, no storage → parallel-safe.
        let seed = edge_seeded_graph_score(1.0 / 3.0, true, 0.0);
        let com = edge_seeded_graph_score(1.0, false, 0.0);
        assert!((seed - 1.0 / 3.0).abs() < 1e-6);
        assert!((com - 1.0).abs() < 1e-6);
        assert!(com > seed, "with bonus=0, breadth wins (inert)");
    }

    #[test]
    fn iss192_edge_seed_bonus_privileges_asserting_edge_over_breadth() {
        // bonus = 0.5 → edge-seeded candidate (breadth 1/3) must outrank a
        // pure co-mention with full breadth 3/3. This is the q0 fix: the
        // asserting-edge source episode beats coincidental co-mention
        // noise. Pure-function test → parallel-safe.
        let seed = edge_seeded_graph_score(1.0 / 3.0, true, 0.5);
        let com = edge_seeded_graph_score(1.0, false, 0.5);
        // seed: (1/3)*(1-0.5) + 0.5 = 0.1667 + 0.5 = 0.6667.
        // comention: (3/3)*(1-0.5) = 0.5.
        assert!((seed - (1.0 / 3.0 * 0.5 + 0.5)).abs() < 1e-6);
        assert!((com - 0.5).abs() < 1e-6);
        assert!(
            seed > com,
            "edge-seeded asserting edge must beat full-breadth co-mention"
        );
    }

    #[test]
    fn iss192_edge_seed_preserves_ordering_within_bands() {
        // Within the edge-seeded band, higher breadth still ranks higher;
        // same for the co-mention band. And every seeded candidate beats
        // every co-mention candidate.
        let bonus = 0.5;
        let seeded_lo = edge_seeded_graph_score(0.0, true, bonus); // 0.5
        let seeded_hi = edge_seeded_graph_score(1.0, true, bonus); // 1.0
        let com_lo = edge_seeded_graph_score(0.0, false, bonus); // 0.0
        let com_hi = edge_seeded_graph_score(1.0, false, bonus); // 0.5
        assert!(seeded_hi > seeded_lo, "breadth ordering within seeded band");
        assert!(com_hi > com_lo, "breadth ordering within co-mention band");
        // Boundary: the strongest co-mention (0.5) ties the weakest seeded
        // (0.5) — acceptable; the privilege guarantees seeded ≥ co-mention,
        // and any seeded with breadth>0 strictly wins.
        assert!(seeded_lo >= com_hi, "weakest seeded ≥ strongest co-mention");
        let seeded_any = edge_seeded_graph_score(0.01, true, bonus);
        assert!(
            seeded_any > com_hi,
            "any positive-breadth seeded strictly wins"
        );
    }

    #[test]
    fn iss172_factual_to_scored_emits_none_vector_score_when_no_query_embedding() {
        // Legacy pre-ISS-172 behaviour: caller passes None → no
        // vector_score is emitted → fusion sees only graph + bm25.
        let (_dir, mut storage) = open_storage();
        seed_embedding(&mut storage, "m1", &[0.1, 0.2]);
        let loader = StorageLoader::new(&storage, MODEL);
        let result = fake_factual_result(1, &[("m1", 1)]);
        let bm25 = HashMap::new();
        let scored = factual_to_scored(&result, &loader, &bm25, None);
        assert_eq!(scored.len(), 1, "one memory in, one scored out");
        match &scored[0] {
            ScoredResult::Memory { sub_scores, .. } => {
                assert_eq!(
                    sub_scores.vector_score, None,
                    "no query embedding → vector_score must stay None"
                );
                assert!(sub_scores.graph_score.is_some());
                assert!(sub_scores.bm25_score.is_some());
            }
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn iss172_factual_to_scored_emits_some_vector_score_with_query_embedding() {
        let (_dir, mut storage) = open_storage();
        // Memory embedding [1.0, 0.0]: identical to the query → cosine 1.0.
        seed_embedding(&mut storage, "m1", &[1.0, 0.0]);
        let loader = StorageLoader::new(&storage, MODEL);
        let result = fake_factual_result(1, &[("m1", 1)]);
        let bm25 = HashMap::new();
        let q = [1.0_f32, 0.0_f32];
        let scored = factual_to_scored(&result, &loader, &bm25, Some(&q));
        assert_eq!(scored.len(), 1);
        match &scored[0] {
            ScoredResult::Memory { sub_scores, .. } => {
                let v = sub_scores
                    .vector_score
                    .expect("vector_score must be Some when query_embedding is Some");
                // Identical vectors → cosine 1.0 (within float epsilon).
                assert!(
                    (v - 1.0).abs() < 1e-5,
                    "cosine(q, m) should be 1.0 for identical unit vectors, got {v}"
                );
            }
            _ => panic!("expected Memory variant"),
        }
    }

    #[test]
    fn iss172_factual_to_scored_breaks_graph_score_tie_via_cosine() {
        // The core LoCoMo failure mode: 2 candidates with identical
        // graph_score (both seen via the single anchor), one
        // semantically relevant, one not. Pre-ISS-172 the order
        // depended entirely on BM25 / id-ordering. Post-fix, the
        // semantically relevant one ranks first.
        let (_dir, mut storage) = open_storage();
        // m1 = relevant (cosine 1.0 with query)
        seed_embedding(&mut storage, "m1", &[1.0, 0.0]);
        // m2 = irrelevant (orthogonal → cosine 0.0)
        seed_embedding(&mut storage, "m2", &[0.0, 1.0]);
        let loader = StorageLoader::new(&storage, MODEL);
        // Both memories seen via the single anchor → graph_score == 1.0
        // for both (the tie that drowns gold on LoCoMo SF queries).
        let result = fake_factual_result(1, &[("m1", 1), ("m2", 1)]);
        let bm25 = HashMap::new();
        let q = [1.0_f32, 0.0_f32];
        let scored = factual_to_scored(&result, &loader, &bm25, Some(&q));
        assert_eq!(scored.len(), 2);
        let by_id: HashMap<&str, f64> = scored
            .iter()
            .filter_map(|s| match s {
                ScoredResult::Memory {
                    record, sub_scores, ..
                } => Some((record.id.as_str(), sub_scores.vector_score.unwrap())),
                _ => None,
            })
            .collect();
        let v_m1 = by_id["m1"];
        let v_m2 = by_id["m2"];
        assert!(
            v_m1 > v_m2,
            "relevant memory must have higher vector_score (m1={v_m1}, m2={v_m2})"
        );
        assert!((v_m1 - 1.0).abs() < 1e-5);
        assert!(v_m2.abs() < 1e-5);
    }
}
