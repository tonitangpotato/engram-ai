# Design: Engram v0.3 — Retrieval

> **Feature:** v03-retrieval
> **GOAL namespace:** GOAL-3.X
> **Master design:** `docs/DESIGN-v0.3.md` §5 (Read path / Retrieval)
> **Depends on:** `v03-graph-layer/design.md` (Entity/Edge types, storage, CRUD), `v03-resolution/design.md` (write-path pipeline populating the graph)
> **Status:** Draft (2026-04-24)

---

## 1. Overview & Scope Boundary

This feature owns the **query-time path**: `query string → intent classification → plan selection → plan execution (graph + vector + BM25) → fusion & ranking → results`.

**In scope:**
- Intent classifier (heuristic, no LLM on the hot path)
- Four execution plans: factual, temporal, associative, mixed
- Fusion & reranking across signal sources (vector, BM25, edge-distance, recency, ACT-R activation)
- Structured query API (`GraphQuery`) as additive surface on top of existing `recall*` methods
- `explain()` observability API returning plan + scoring trace
- Per-plan latency budget structure (absolute numbers live in `v03-benchmarks`)

**Out of scope (owned elsewhere):**
- Entity/Edge type definitions, storage schema, CRUD — `v03-graph-layer`
- Populating the graph (extraction pipeline) — `v03-resolution`
- Absolute latency numbers & benchmark harness — `v03-benchmarks`
- v0.2 → v0.3 data migration — `v03-migration`

**Backward compatibility commitment:** `recall`, `recall_recent`, `recall_associated`, `hybrid_recall` signatures and documented semantics are unchanged in v0.3. The new surface (`GraphQuery`, `explain`) is strictly additive.

---

## 2. Requirements Coverage

All 14 `GOAL-3.X` requirements and 2 cross-cutting guards (GUARD-3, GUARD-6) satisfied. Full traceability table in **§11**. Summary:

- **Classification & routing** (GOAL-3.1, 3.2) → §3
- **Factual + bi-temporal** (GOAL-3.3, 3.4, 3.5, GUARD-3) → §4.1, §4.6
- **Abstract / L5** (GOAL-3.6, 3.7, 3.13) → §4.4, §6.3
- **Affective + self-state bias** (GOAL-3.8, 3.14, GUARD-6) → §4.5
- **Tier API** (GOAL-3.9) → §6.5
- **Typed outcomes** (GOAL-3.10) → §6.4
- **Observability / explain** (GOAL-3.11) → §6.3, §8
- **Novel predicates** (GOAL-3.12) → §6.6

---

## 3. Query Classification

### 3.1 Intent categories

A query is classified into **exactly one** of five intents (per GOAL-3.1):

- **`Factual`** — "who is X", "what does Y do", "is Y still married to Z". Anchor entities named/implied; answer is edges around those entities. Grounds in the semantic graph (L4 edges) with per-fact provenance and bi-temporal validity (GOAL-3.3, GOAL-3.4).
- **`Episodic`** — "what happened yesterday", "the meeting last Tuesday". A concrete time window dominates; answers are source episodes (memories), not synthesized facts.
- **`Abstract`** — "what has the user been working on", "summarize our work on Y". Thematic / high-level; answered from **L5 Knowledge Topics** with source-memory traces (GOAL-3.6).
- **`Affective`** — "what made me anxious this week", "things I felt good about". Dominant signal is emotional tone; answered by affect-weighted recall (GOAL-3.7, GOAL-3.8).
- **`Hybrid`** — any query where ≥ 2 of the above signals are strong simultaneously (e.g., "what did we decide about the graph layer last Tuesday" → Factual + Episodic).

Exactly one intent is assigned per query. `Hybrid` is **not** a low-confidence fallback — it fires only when the classifier is **high-confidence about multiple signals**.

**Mapping to DESIGN-v0.3 terms:** `Factual` ↔ graph/semantic path; `Episodic` ↔ temporal recall path; `Abstract` ↔ L5 Knowledge Topic path; `Affective` ↔ affect-weighted recall path; `Hybrid` ↔ cross-layer fusion path. See §5.2 of master DESIGN.

### 3.2 Classifier design (heuristic first, LLM fallback per GOAL-3.2)

The classifier runs on **every** call through `GraphQuery`. Two-stage per GOAL-3.2:

**Stage 1 — heuristic (always runs, sub-millisecond, no LLM):**

Signals (scored independently, then thresholded):

1. **Entity signal** — does any query token resolve to a known canonical entity via alias/fuzzy lookup? `score = max(exact ? 1.0 : 0.0, alias * 0.8, fuzzy * 0.5)`.
2. **Temporal signal** — time expression detected via regex/keyword set: `yesterday | today | last (week|month|...) | before | after | \d{4}-\d{2}-\d{2} | \d+ (days?|weeks?) ago | since | until | as of`. Binary: `1.0` on match else `0.0` for v0.3.
3. **Abstract signal** — query matches patterns indicating thematic/summary intent: `what (has|have) | summarize | overview | themes? | patterns? | trends? | working on`. Score `0.0` / `1.0` binary.
4. **Affective signal** — query contains emotion vocabulary: `felt | feeling | anxious | happy | sad | excited | stressed | proud | ashamed | angry | ...` (seed list, extensible via config). Binary.
5. **Associative signal** — derived: `1.0 - max(entity, temporal, abstract, affective)`. Captures "no strong primary signal".

**Stage 1 routing rule:**

```
strong_signals = {s for s in {entity, temporal, abstract, affective} if s.score ≥ threshold_s}

if |strong_signals| == 0:                              → Intent::Factual with `downgrade_hint = Associative`
                                                          (plan builder materializes the Associative plan §4.3)
elif |strong_signals| == 1 and confidence ≥ τ_high:    → single-intent plan (Factual/Episodic/Abstract/Affective)
elif |strong_signals| ≥ 2 and each ≥ τ_high:           → Hybrid (§4.7)
else:                                                   → Stage 2 (LLM fallback)
```

**Note on `Associative`:** per §3.1, there are exactly **5** intents (`Factual, Episodic, Abstract, Affective, Hybrid`). `Associative` is a **plan-kind only** (§4.3), not an `Intent` variant — it is reached when the classifier emits `Intent::Factual` with no strong signals, and the plan builder routes to the Associative plan. Downgrades from other plans to Associative are recorded via `RetrievalOutcome::Downgraded*` (§6.4), not by changing the classified intent.

Default thresholds: `threshold_s = 0.7` for entity, `1.0` for the binary signals; `τ_high = 0.7`. Configurable via `RetrievalConfig`.

**Stage 2 — LLM fallback (fires only when heuristic is uncertain):**

Triggered iff Stage 1 returns the "ambiguous" branch (mixed-strength signals, none crossing `τ_high`). Budget-capped (see §7 — default 200ms; on timeout, plan defaults to `Associative` with `classifier_method = "heuristic_timeout"` in trace).

LLM fallback prompt takes the query + signal scores + small in-context intent examples, returns one of the 5 intents. The LLM's output is **not trusted blindly** — if it returns an intent inconsistent with hard signals (e.g., says `Episodic` when temporal signal was 0.0), the classifier logs a mismatch and defaults back to heuristic's best guess.

**`classifier_method` is observable** (GOAL-3.2): every `PlanTrace.classifier` includes `method: Heuristic | LlmFallback | HeuristicTimeout`. This is the primary knob for diagnosing classification errors in production.

### 3.3 Caller-specified intent (override)

`GraphQuery` accepts `intent: Option<Intent>`. When `Some(_)`, both classifier stages are bypassed. Trace records `method: CallerOverride`.

### 3.4 Fallback & ambiguity

Classifier is **total** — always returns a plan. If even LLM fallback times out or errors, the default is `Associative` (degrades to existing `recall_associated` — a known-good v0.2 behavior). Plans may further **downgrade at execution time** (e.g., `Factual` with zero resolved anchors → `Associative` mid-flight); downgrades recorded in trace.

---

## 4. Routing & Execution Plans

Each plan is a pure function over `(classified_intent, query, store)` producing `(Vec<ScoredResult>, PlanTrace)`. Plans may internally call the existing `hybrid_recall` and `recall_associated` methods — they compose, not replace.

### 4.1 Factual plan

Goal: return memories **anchored** to specific entities mentioned in the query, ranked by graph proximity plus textual relevance.

**Steps:**

1. **Entity resolution.** Tokenize query; resolve each token against entity store (exact canonical_id → alias → fuzzy match over entity names). Produce `anchors: Vec<EntityId>`.
2. **Downgrade check.** If `anchors.is_empty()` → downgrade to Associative (§4.3), record downgrade reason in trace.
3. **Edge traversal.** For each anchor, fetch 1-hop edges (configurable max hops, default 1 for v0.3). Collect `linked_entities: Set<EntityId>` and `edges_traversed: Vec<EdgeId>`.
4. **Memory lookup.** Query memories linked to `{anchors} ∪ {linked_entities}` via the `graph_memory_entity_mentions` table (defined in `v03-graph-layer/design.md` §4.1).
5. **Rerank.** Pass candidate memories through `hybrid_recall`-style scoring (vector + BM25) to rank by textual relevance to the full query.
6. **Fusion.** Combine per-memory scores: `final = w_graph * graph_score + w_text * text_score + w_recency * recency` (weights in §5).

**Latency budget:** dominated by step 4 (memory lookup). Edge traversal is bounded to 1 hop × `max_anchors` (default 5) × avg fanout — conservative cap at 500 edges visited.

### 4.2 Episodic plan

Goal: return source memories within a time window (GOAL-3.4 "as-of-T" queries are handled here too). Formerly called "Temporal" in drafts.

**Steps:**

1. **Parse time window.** Convert the temporal expression to `(start: Option<DateTime>, end: Option<DateTime>)`. Absolute dates parse directly; relative ("last week") resolves against `query_time` (injected, not `now()` — for reproducibility, see §5.4). **If no time expression can be parsed with confidence ≥ 0.5** (classifier mis-route, or expression too fuzzy), the Episodic plan downgrades to Associative, emitting `RetrievalOutcome::DowngradedFromEpisodic { reason: "no_time_expression" }` (see §6.4). The graph-filter step (§4.2.3) is dropped; fusion proceeds with Associative weights.
2. **Time-bounded recall.** Call existing `recall_recent`-path (extended to accept explicit `[start, end]`) for memories in window.
3. **Optional graph filter.** If any entity signal (§3.2) had score ≥ `τ_graph_filter` (default `0.3`, see §7.3) even though not dominant, intersect results with memories linked to those entities.
4. **Bi-temporal edge projection (if query is `as-of-T`, GOAL-3.4).** If the query carries an `as_of: DateTime` (parsed from "as of ..." or supplied via `GraphQuery.time_window.as_of`), **any edges surfaced in results are filtered to their state valid at T**: edges with `valid_from ≤ T` and (`valid_to is null OR valid_to > T`) are kept; edges superseded after T are kept as if not-yet-superseded; edges that became valid after T are excluded. Current-state queries (no `as_of`) behave as today.
5. **Rerank** (BM25 on non-temporal tokens).
6. **Fusion.** `final = w_text * text_score + w_recency * recency + w_graph * graph_score` (graph term only if §4.2.3 applied).

**`query_time` injection** is what makes episodic retrieval **reproducible** (§5.4). Identical `(query, query_time, store_snapshot)` → identical results.

### 4.3 Associative plan

Goal: free-form exploration; spread across the graph. Extends existing `recall_associated` semantics with **edge-hop traversal** from top-K seed results.

**Steps:**

1. **Seed recall.** Call `hybrid_recall(query, k=K_seed)` — default `K_seed = 10`.
2. **Extract seed entities.** For each seed memory, call `GraphStore::entities_linked_to_memory(memory_id)` (v03-graph-layer §4.2), which reads the `graph_memory_entity_mentions` join table (§4.1).
3. **Edge-hop expansion.** For each seed entity, fetch 1-hop edges (respecting `min_confidence` filter per `GraphQuery`); add connected entities and their memories. Pool capped at `K_pool` (default 100).
4. **Spread-activation scoring.** Each candidate scored by `seed_score × edge_distance_decay × actr_activation` (ACT-R mechanism reused from existing engramai unchanged).
5. **Deduplication.** A memory reachable via multiple paths takes its **max** score (sum would bias toward hubs).
6. **Fusion.** See §5.2.

Associative is also the **default fallback plan** (§3.4). In fallback mode it runs identically — no special "degraded" code path.

### 4.4 Abstract / L5 Knowledge Topic plan (GOAL-3.6, GOAL-3.7)

Goal: answer thematic / summary queries from **L5 Knowledge Topics** — synthesized, consolidated views — with traceability back to source memories and graph entities.

**L5 substrate (written elsewhere):** L5 Knowledge Topics are produced by the Knowledge Compiler background job, specified in `v03-resolution/design.md` §5bis. The retrieval feature **consumes** L5; it does not synthesize it on the read path. On this feature, synthesis is assumed to have already run for topics relevant to the query domain.

**Steps:**

1. **Topic candidate recall.** Vector + BM25 search over the `knowledge_topics` table (v03-graph-layer §4.1): `embedding` for vector similarity, `summary` for BM25. Both indexes are populated at synthesis time.
2. **Traceability expansion.** For each candidate topic, read `source_memories` and `contributing_entities` directly from the topic row (both are stored as JSON arrays per v03-graph-layer §4.1 schema). No joins needed — the topic row is self-contained for retrieval purposes.
3. **Rerank.** Optional cross-encoder rerank if a `Reranker` is configured (§5.3); default is fusion score alone.
4. **Result shape.** Each result is a `ScoredResult::Topic { topic: KnowledgeTopic, score: f64, source_memories: Vec<MemoryId>, contributing_entities: Vec<EntityId> }` (§6.2). `KnowledgeTopic` is the struct from v03-graph-layer §4.2.

**Affect-weighted clustering input (GOAL-3.7):** each `knowledge_topics` row carries a `cluster_weights` JSON field (set by the Knowledge Compiler at synthesis time, v03-resolution §5bis.3) encoding how affect biased the clustering. At retrieval time the Abstract plan **does not recompute** this — it respects it. Callers who want to see affect-weighting effects compare topic-set diffs across different `cluster_weights` configurations (benchmark-side concern, `v03-benchmarks`).

**If no L5 topics exist for the query domain** (e.g., fresh database, compiler hasn't run yet, or the candidate set's top-K topic-similarity scores are all below `config.l5_min_topic_score`): the plan **downgrades to Associative** with `downgrade_reason = "L5_unavailable"` in the trace. Returning nothing when L5 is empty would violate GOAL-3.6's "no silent degrade" posture — GUARD-2 says the system must never silently degrade; substrate-empty is a legitimate reason, and the outcome is surfaced via `RetrievalOutcome::DowngradedFromAbstract` (see §6.4). L5 is strictly **read-only** in v0.3; synthesis cost lives on the compiler's counters (v03-resolution), not on retrieval's. On-demand synthesis from the read path is **not** attempted in v0.3 (it would break the latency budget §7; operators run `compile_knowledge` explicitly or wait for the scheduled run).

### 4.5 Affective plan (GOAL-3.8, GOAL-3.14)

Goal: emotionally-biased recall — memories whose **write-time affect snapshot** is similar to the **current cognitive self-state** (or to an explicit affective query target).

**Mood-congruent recall** (Bower 1981 sense, per master DESIGN §5.3): the active self-state `s_now` biases scoring so that memories tagged with affect closer to `s_now` rank higher — without **ever gating results** (GOAL-3.14 / GUARD-6: cognitive state modulates ranking, never blocks).

**Steps:**

1. **Fetch current self-state.** `s_now: AffectVector` (valence, arousal, and the other axes defined elsewhere in engramai's cognitive-state module). If no self-state is tracked, plan downgrades to Associative.
2. **Candidate recall.** Standard `hybrid_recall(query, k=K_seed_affective)` for textually-relevant candidates (affective plan still needs text grounding — a query is a query). Default `K_seed_affective = 3 * requested_k`, capped at 60 (§7.3).
3. **Affect distance scoring.** For each candidate, compute `affect_similarity = 1 - distance(memory.affect_snapshot, s_now)` using a configurable distance metric (default: cosine over valence/arousal).
4. **Fusion.** `final = w_text * text_score + w_affect * affect_similarity + w_recency * recency`. Default `w_affect = 0.35`. **Crucially, `w_affect < 1.0` so high text_score can override low affect_similarity** — this is the "never block" guarantee: a textually-perfect match is never filtered out just because it has wrong affect.
5. **Rank-difference telemetry (GOAL-3.8).** On every Affective plan invocation, the plan **also** computes the same results under a neutral self-state (`s_neutral`) and reports the Kendall-tau correlation between the two rankings in the trace. GOAL-3.8 asserts this correlation should be `< 0.9` on the benchmark query set (i.e., self-state actually makes a visible difference). Computing the second ranking doubles cost; it runs when `GraphQuery.explain = true`, or at the configured sample rate `retrieval.affect_divergence_sample_rate` (§7.3; **default `0.01`** = 1% of Affective calls). Setting the rate to 0 degrades GOAL-3.8 production observability and is disallowed by the affect-divergence test (§9), which pins the rate to ≥ 0.01.

**GUARD-6 enforcement:** no memory is ever removed from the result set by the affective plan. Affect only reorders. A test in §9 verifies this invariant.

### 4.6 Bi-temporal projection (cross-cutting, GOAL-3.4, GOAL-3.5)

Bi-temporal validity applies across Factual, Episodic, and Hybrid plans whenever edges are surfaced. The projection rule is centralized here so it stays consistent across plans:

- **Default (current-state query):** edges with `valid_to IS NULL OR valid_to > now()` are included. Superseded edges (`valid_to ≤ now()`) are excluded from default results.
- **`as-of-T` query:** edges with `valid_from ≤ T AND (valid_to IS NULL OR valid_to > T)` are included. The rest are excluded. Edges that would be "superseded" according to the current clock but were valid at T are still returned — this is the as-of-T projection.
- **`include_superseded = true` opt-in (GOAL-3.5):** superseded edges are **also** returned, each marked with its `valid_to` timestamp and optionally the superseding edge's id. This is how history is accessed for schema-evolution review.

**Terminology bridge (storage ↔ API).** Retrieval's API uses "superseded" throughout (the action-verb framing). The storage layer in `v03-graph-layer` §4.1 names the state column `invalidated_at` / `invalidated_by` (the state framing) and the trait method `edges_of(subject, predicate, include_invalidated)` (§4.2). Both names refer to the same underlying rows: an edge with non-NULL `invalidated_at` is a superseded edge. The translation happens at the retrieval query-builder: `GraphQuery.include_superseded == true` ⇔ `GraphStore::edges_of(.., include_invalidated = true)`. The split is intentional — "supersede" is the operation (what happened), "invalidated" is the column name (the resulting state).

The projection is applied **as a filter at the storage layer** (WHERE clause in the edge query) — not post-hoc in memory. Schema specifics (`valid_from` / `valid_to` columns, indexes) live in `v03-graph-layer/design.md` §4.1 — this feature relies on them being present.

**GUARD-3 (hard):** bi-temporal invalidation never erases. A superseded edge is **always retrievable** via `include_superseded = true` or `as_of` query, forever. A test in §9 verifies that after N supersession operations, the full history is still queryable.

### 4.7 Hybrid plan (GOAL-3.1, fusion path)

Goal: execute multiple single-intent plans and fuse their result sets. Used when the classifier assigns `Hybrid` intent (§3.1) — ≥ 2 strong signals simultaneously.

**Steps:**

1. **Sub-plan selection.** Pick the 2 sub-plans corresponding to the two strongest signals (e.g., `entity` + `temporal` → `Factual + Episodic`; `abstract` + `affective` → `Abstract + Affective`). **Max 2 sub-plans** — 3+ explodes cost. When `|strong_signals| > 2`, keep the top-2 by signal score and record the dropped signals in `PlanTrace.hybrid_truncated: Vec<DroppedSignal>` (observability — preserves GUARD-2; see also `retrieval_hybrid_truncation_total` in §8.1).
2. **Parallel execution.** Sub-plans run concurrently (tokio `join!`). Each produces its own `Vec<ScoredResult>`.
3. **Heterogeneous result merge.** `Abstract` returns `TopicResult`s, others return `MemoryResult`s. The `ScoredResult` enum (§6.2) accommodates both. Hybrid results may be a mix.
4. **Reciprocal Rank Fusion (RRF).** Use existing `hybrid_search.rs::reciprocal_rank_fusion`. Memories/topics appearing in both sub-plans get a combined RRF score; single-plan candidates keep their RRF score. RRF is scale-invariant — no normalization needed.
5. **Top-K cutoff** per `GraphQuery.limit`.

Hybrid does **not** re-score — it reuses sub-plan scores via RRF. Cheapest correct fusion for heterogeneous candidate sets.

---

## 5. Fusion & Ranking

### 5.1 Signal sources

Every scored memory in v0.3 carries up to **five** per-memory sub-scores, depending on plan:

| Signal | Source | Range | Plans using it |
|---|---|---|---|
| `vector_score` | cosine similarity over embeddings (existing) | `[0, 1]` | Factual, Associative, Hybrid |
| `bm25_score` | existing `hybrid_search.rs` BM25 | `[0, ~20]` raw, normalized `[0, 1]` | all |
| `graph_score` | edge-distance decay from anchors | `[0, 1]` | Factual, Associative |
| `recency_score` | half-life decay of `memory.age` | `[0, 1]` | Temporal, all as tiebreaker |
| `actr_score` | existing ACT-R activation level | `[0, 1]` (normalized) | Associative |

### 5.2 Combination rule

Per-plan fusion weights (defaults; overridable via `RetrievalConfig`):

```
Factual:     final = 0.45 * graph_score + 0.40 * text_score + 0.15 * recency_score
Episodic:    final = 0.55 * text_score  + 0.30 * recency_score + 0.15 * graph_score (if present)
             (see "Missing signal normalization" below)
Associative: final = 0.40 * seed_score  + 0.35 * edge_distance + 0.25 * actr_score
Abstract:    final = 0.60 * topic_text_score + 0.25 * topic_actr_score + 0.15 * source_coverage
Affective:   final = 0.50 * text_score + 0.35 * affect_similarity + 0.15 * recency_score
Hybrid:      reciprocal rank fusion over sub-plan outputs (no weighted sum)
```

`text_score = max(vector_score, bm25_score)` — conservative aggregate, avoids double-counting vector and BM25 (they correlate heavily).

`source_coverage` (Abstract) = fraction of topic's source memories that also match query terms — biases toward topics with broad textual support.

**Missing signal normalization.** When a fusion component is absent (e.g., no graph score because the graph expansion step was skipped or returned empty), the remaining weights are **renormalized to sum to 1.0** by proportional scaling. This preserves score ranges in `[0, 1]` and keeps fused scores comparable across calls with and without the absent signal. The renormalization is deterministic and recorded in `FusionTrace`.

These weights are **not committed performance claims** — they are starting points. `v03-benchmarks` tunes them against the eval set. The design commitment is: **weighted sums of bounded signals** (except Hybrid/RRF), so behavior is inspectable and tunable without algorithmic rewrites.

### 5.3 Reranker contract

A plan may optionally hand its top-K candidates to a **reranker** before returning:

```rust
pub trait Reranker: Send + Sync {
    fn rerank(
        &self,
        query: &str,
        candidates: &[ScoredResult],
    ) -> Result<Vec<ScoredResult>, RetrievalError>;
}
```

Requirements on implementations:
- **Pure**: given the same `(query, candidates)` it must return the same ordering (important for §5.4).
- **Bounded latency**: implementations must honor a `Duration` budget or return early with partial rerank.
- **Score preservation**: the reranker may adjust scores but MUST keep them in `[0, 1]` and MUST NOT drop candidates (only reorder).

For v0.3 no reranker ships by default (the fusion rule above is the ranking). The trait exists so callers can plug in cross-encoders or LLM rerankers without modifying retrieval internals.

### 5.4 Determinism & reproducibility

**GOAL: given a frozen store snapshot, a fixed query, and a fixed `query_time`, every retrieval call returns byte-identical results.**

Requirements on implementations:
- No `rand::thread_rng()` or wall-clock calls in the retrieval path. Temporal plans accept an explicit `query_time: DateTime<Utc>` parameter (default to `Utc::now()` at the API boundary, but deterministic once inside).
- Ties in fusion score broken by `(memory_id ascending)` — a stable secondary key. This rule is a **hard-coded invariant** in v0.3 (no config knob); if future versions need alternate tie-break orders, a `TieBreakOrder` enum can be introduced then.
- Parallel sub-plans in Hybrid (§4.7) must fuse deterministically: RRF over sorted-by-id inputs.
- Floating-point: single-threaded per plan, so no reduction-order drift.

Reproducibility is a **testability requirement** (§9) and a **debuggability requirement** (the `explain()` trace must be replayable).

**`FusionConfig::locked()` — benchmarks handoff (r3).** v03-benchmarks §3.1 + §12 require the exact fusion weight set used during a benchmark run to be embedded in the reproducibility record. `FusionConfig::locked()` is the constructor that produces a pinned, inspectable `FusionConfig` instance — the same signal weights, reranker settings, and tie-break rules every call, with no environment / config-file dependency.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FusionConfig {
    /// Per-memory-type signal weights (GOAL-3.13). Sum to 1.0 per type.
    pub signal_weights: SignalWeightMatrix,
    /// RRF k-parameter for cross-plan fusion in Hybrid (§4.7).
    pub rrf_k: f64,
    /// Reranker cutoff; memories below this fused score are dropped.
    pub min_fused_score: f64,
    /// Semantic version of this config. `locked()` pins a specific version
    /// so that a benchmark reproducibility record can assert "same weights".
    pub version: &'static str,
}

impl FusionConfig {
    /// Canonical, frozen fusion configuration used by benchmarks and any
    /// caller that needs determinism. This is a pure function — no env vars,
    /// no config files, no build-time flags. Returns byte-identical output
    /// on every call within a single `engramai` version.
    ///
    /// Benchmark drivers embed the returned `FusionConfig` into the
    /// reproducibility record (v03-benchmarks §3.1) by serializing it as
    /// JSON; the benchmark CLI then re-runs with an asserted match.
    pub fn locked() -> Self;
}
```

**Contract with benchmarks (GOAL-3.13 handoff).**
- `locked()` output is stable across minor `engramai` versions within the same v0.x line. Changes bump `FusionConfig::version` and require a benchmark-record re-capture.
- The weight matrix is **dense** (no NaN, no Inf, all finite `f64` in `[0, 1]`).
- `PartialEq` lets benchmarks assert exact equality against the stored record without custom comparison logic.
- `locked()` is the **only** public constructor reachable from benchmark code. Other constructors (`from_env`, `from_file`) exist for production use but are explicitly out of scope for determinism guarantees.

---

## 6. Public API

### 6.1 Unchanged v0.2 surface (backward compatibility)

All of the following keep identical signatures and documented semantics:

- `Memory::recall(&self, query: &str, k: usize, ...) -> Result<Vec<RecallResult>, _>`
- `Memory::recall_recent(&self, k: usize, ...) -> Result<Vec<MemoryRecord>, _>`
- `Memory::recall_associated(&self, memory_id: &str, k: usize, ...) -> Result<Vec<RecallResult>, _>`
- `Memory::hybrid_recall(&self, query: &str, k: usize, ...) -> Result<Vec<RecallResult>, _>`

> **Naming note (GUARD-11 canonical decision).** The v0.2 public type is `Memory` (see `engramai/src/memory.rs`), and per-record results come back as `RecallResult` (scored) or `MemoryRecord` (raw). Earlier draft wording of `EngramMemory` / `ScoredMemory` / bare `Memory` in return position was pre-canonical — **all five v0.3 designs normalize on `Memory` (holder) + `MemoryRecord` (row) + `RecallResult` (scored row)**. Trailing `...` above abbreviates the existing `context` / `min_confidence` / `namespace` parameters; none of them change.

Behavior change: when the graph is populated (v0.3 store), these methods **may** internally consult edges to improve ranking. They remain free to bypass the graph entirely when the graph is empty (v0.2 databases, pre-migration). This is observable only through improved result quality — never through a breaking change in result shape.

### 6.2 New: `GraphQuery` structured API

```rust
#[derive(Debug, Clone)]
pub struct GraphQuery {
    pub text: String,
    pub intent: Option<Intent>,              // None = classifier decides (§3.2)
    pub limit: usize,                        // top-K cutoff
    pub time_window: Option<TimeWindow>,     // override temporal parse
    pub as_of: Option<DateTime<Utc>>,        // bi-temporal projection (§4.6)
    pub include_superseded: bool,            // GOAL-3.5 opt-in history
    pub entity_filter: Option<Vec<EntityId>>,
    pub min_confidence: Option<f64>,         // drop low-confidence edges
    pub tier: Option<MemoryTier>,            // GOAL-3.9 explicit tier scoping
    pub query_time: Option<DateTime<Utc>>,   // reproducibility (§5.4)
    pub explain: bool,                       // trace in response
}

#[derive(Debug, Clone)]
pub enum ScoredResult {
    Memory { record: MemoryRecord, score: f64, sub_scores: SubScores },
    Topic  { topic: KnowledgeTopic, score: f64, source_memories: Vec<MemoryId>,
             contributing_entities: Vec<EntityId> },
}

#[derive(Debug, Clone)]
pub struct GraphQueryResponse {
    pub results: Vec<ScoredResult>,
    pub plan_used: Intent,          // actual plan (may differ after downgrade)
    pub outcome: RetrievalOutcome,  // typed success/failure modes (§6.4)
    pub trace: Option<PlanTrace>,   // iff query.explain == true
}

impl Memory {
    pub async fn graph_query(
        &self,
        query: GraphQuery,
    ) -> Result<GraphQueryResponse, RetrievalError>;

    /// Deterministic-mode variant: equivalent to `graph_query` but pins fusion
    /// behavior to `FusionConfig::locked()` (§5.4). Intended for benchmarks,
    /// reproducibility records, and tests that need byte-identical output.
    pub async fn graph_query_locked(
        &self,
        query: GraphQuery,
    ) -> Result<GraphQueryResponse, RetrievalError>;
}
```

Design notes:
- `text` is the only required field; all others have sensible defaults matching existing `recall()` behavior.
- `graph_query` is **additive** — v0.2 `recall*` methods never call it internally.
- Variant name `ScoredResult::Memory` is the enum variant (namespaced inside `ScoredResult::`), not a collision with the `Memory` holder type — the two never appear in ambiguous positions.
- Returns `GraphQueryResponse` (not bare `Vec`) — future-proof without breaking change.
- `ScoredResult` is an enum to carry heterogeneous outputs (Abstract plan returns topics, others return memories; Hybrid can mix).
- Both `graph_query` and `graph_query_locked` accept the same `GraphQuery`; only the fusion configuration differs. `graph_query` uses the environment/config-file-derived `FusionConfig`; `graph_query_locked` uses `FusionConfig::locked()` (no env, no files, no flags).

### 6.2a Types referenced by the public API

The following types appear above (and in §6.3) and are defined here for implementer clarity. One-line definitions; full field sets live in the implementation.

- `Intent` — `enum Intent { Factual, Episodic, Abstract, Affective, Hybrid }`. Exactly 5 variants (§3.1). `Associative` is **not** an `Intent` — it is a plan-kind (§4.3) reached by downgrade.
- `TimeWindow` — `enum TimeWindow { None, At(DateTime<Utc>), Range { from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>> }, Relative(Duration) }`.
- `SignalWeightMatrix` — per-plan `HashMap<Intent, FusionWeights>` where `FusionWeights` lists the per-signal weights from §5.2 (all `f64` in `[0, 1]`, sum to 1.0 per plan).
- `RetrievalError` — `enum RetrievalError { Timeout, StoreUnavailable, ConfigError(String), ClassifierError(String), Internal(String) }`. Infrastructure failures only; business-logic "empty result" stays in `Ok(_)` with a typed `RetrievalOutcome` (§6.4).
- `AffectVector` — read-only re-export of the cognitive-state module's vector type (valence, arousal, and the other axes). Retrieval never constructs or mutates one.
- `SubScores` — struct of per-signal sub-score fields (`vector_score`, `bm25_score`, `graph_score`, `recency_score`, `actr_score`, `affect_similarity`), each `Option<f64>` in `[0, 1]`. Retrieval-internal (see §10 note).
- `DroppedSignal` — `struct DroppedSignal { kind: SignalKind, score: f64 }`. Emitted in `PlanTrace.hybrid_truncated` when Hybrid truncates ≥ 3 strong signals to 2 (§4.7).
- `LlmCost` — `struct LlmCost { calls: usize, prompt_tokens: usize, completion_tokens: usize, duration: Duration }`. Used by `ClassifierTrace.llm_cost: Option<LlmCost>` (None when rule-only).
- **Trace structs** (`ClassifierTrace`, `PlanDetail`, `Downgrade`, `FusionTrace`, `BiTemporalTrace`, `AffectTrace`) — observability-only structs carrying the fields referenced in the surrounding prose; exact shape is an implementation concern. Each derives `Serialize` for JSON export (§6.3). `FusionTrace` in particular records any renormalization applied under "Missing signal normalization" (§5.2).

### 6.3 New: `explain()` — observability surface

`GraphQueryResponse.trace: Option<PlanTrace>` is the observability API:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct PlanTrace {
    pub classifier: ClassifierTrace,      // signal scores, chosen intent, method, optional llm_cost
    pub plan: PlanDetail,                 // per-plan steps + timings + cost caps hit
    pub downgrades: Vec<Downgrade>,       // every §3.4 / §4.*.2 / §4.4 downgrade
    pub fusion: FusionTrace,              // weights + per-candidate sub-scores
    pub bi_temporal: Option<BiTemporalTrace>, // as-of-T projection details (§4.6)
    pub affect: Option<AffectTrace>,      // GOAL-3.8 rank-diff metric
    pub total_latency: Duration,
    pub per_stage_latency: Vec<(String, Duration)>,
}
```

`PlanTrace` serializes to JSON. Primary vehicle for: debugging, benchmark diffing, caller-side logging.

**Cost:** trace is only assembled when `query.explain == true`. Default `false` — no overhead on production hot path (GOAL-3.11).

**GOAL-3.13 — L5 cost isolation:** L5 synthesis is owned by the Knowledge Compiler (v03-resolution); the retrieval read path never invokes it in v0.3 (see §4.4 — L5 is read-only on this path). LLM calls on the retrieval side come only from the classifier Stage 2 fallback (§3.2) and are metered separately (see §8.1 `retrieval_classifier_llm_*`). Write-path LLM calls (resolution stages 3/4/5) and compiler L5-synthesis calls are counted by `v03-resolution` and never appear in retrieval traces. This enforces the independent-counting requirement.

### 6.4 Typed retrieval outcomes (GOAL-3.10)

Retrieval returns **typed outcomes**, not just "empty result set". Distinguishing the failure modes is the caller's leverage for graceful UX.

```rust
#[derive(Debug, Clone)]
pub enum RetrievalOutcome {
    Ok,                                     // results non-empty, normal success
    NoEntityFound { query_tokens: Vec<String> },
                                            // Factual: no query token resolved to any entity
    EntityFoundNoEdges { entities: Vec<EntityId> },
                                            // Factual: entities exist but no edges match
    NoMemoriesInWindow { start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>> },
                                            // Episodic: window is empty
    AmbiguousQuery { candidate_intents: Vec<Intent>, reason: String },
                                            // classifier LLM fallback returned multiple
                                            // plausible intents with near-tied confidence
    L5NotReady { missing_topic_domains: Vec<String> },
                                            // Abstract: synthesis hasn't covered this domain
    DowngradedFromAbstract { reason: String },
                                            // Abstract plan downgraded to Associative
                                            // (e.g., reason = "L5_unavailable")
    DowngradedFromEpisodic { reason: String },
                                            // Episodic plan downgraded to Associative
                                            // (e.g., reason = "no_time_expression")
    NoCognitiveState,                       // Affective: self-state absent, downgraded
}
```

A `RetrievalOutcome` ≠ `Ok` is **not** an `Err` — results may still be present (e.g., `EntityFoundNoEdges` may carry associative fallback results). `Err(RetrievalError)` is reserved for infrastructure failures: DB errors, timeout of the outer query, config errors. Business-logic "didn't find what you asked for" stays in `Ok(...)` with a typed outcome. This is the GUARD-6 instantiation for the read path (cognitive state / missing data never fails the call).

### 6.5 Tier API (GOAL-3.9)

Memory tiers (`Working`, `Core`, `Archived`) are already distinguished internally by engramai's trace-strength model. v0.3 exposes them as a **formal query surface**:

```rust
#[derive(Debug, Clone, Copy)]
pub enum MemoryTier {
    Working,   // hot: high short-term trace strength
    Core,      // warm: high long-term trace strength + recent activation
    Archived,  // cold: below activation threshold
}

impl Memory {
    pub async fn recall_tier(
        &self,
        tier: MemoryTier,
        query: &str,
        k: usize,
    ) -> Result<Vec<RecallResult>, _>;

    pub async fn list_tier(
        &self,
        tier: MemoryTier,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryRecord>, _>;
}
```

Also: `GraphQuery.tier: Option<MemoryTier>` scopes a structured query to a single tier. The tier mapping to the existing trace-strength thresholds is fixed in `RetrievalConfig` (Working: `short_term_strength ≥ τ_hot`; Core: `long_term_strength ≥ τ_warm AND recent_activation`; Archived: everything else).

### 6.6 Novel-predicate retrieval (GOAL-3.12)

Novel predicates (those outside the canonical predicate vocab, represented as `Predicate::Proposed(String)` per `v03-graph-layer` §3.3) are retrievable through the **same** `GraphQuery` surface — no separate code path. Factual plans that match query tokens to proposed predicates work transparently, and the trace records which predicate variant matched (`Canonical(...)` vs `Proposed(...)`).

A convenience method `list_proposed_predicates(limit) -> Vec<(String, usize)>` returns proposed predicates with their use-count, enabling **schema-evolution review** — operators can see what novel relations the system is encountering and decide whether to promote them to canonical.

---

## 7. Latency Budget

### 7.1 Budget structure (not absolute numbers)

Absolute latency numbers are **defined and enforced by `v03-benchmarks`**. This doc commits to the **structure**: each plan has a per-stage budget that sums to the plan's total budget, and each plan has a **hard cutoff** beyond which it returns partial results rather than blocking.

Per-plan stage budget shape:

```
Factual total = entity_resolution + edge_traversal + memory_lookup + rerank + fusion
Temporal total = time_parse + time_bounded_recall + optional_graph_filter + rerank + fusion
Associative total = seed_recall + entity_extract + edge_hop + scoring + fusion
Hybrid total = max(sub_plan_a, sub_plan_b) + rrf_fusion
```

Each stage has a `Duration` cap from `RetrievalConfig`. Cap violations trigger §7.2.

### 7.2 Cutoff behavior (partial results, never hang)

On stage timeout:

- **Before any results produced**: plan returns `Ok(vec![])` with `PlanTrace.downgrades` noting the timeout. Never returns an error — the caller gets "no results within budget" not "query failed".
- **After partial results**: plan returns what it has, trace flags the early termination.
- **Never**: the plan blocks past its budget. Total latency is hard-bounded at the plan's configured cap plus a small overhead (< 5ms for fusion even on partial inputs).

**Rationale:** retrieval latency is user-facing. A slow correct answer is worse than a fast partial answer in an interactive agent context. The `explain()` trace lets callers detect when they got partial results and retry with a larger budget if they care.

### 7.3 Cost caps (hard limits)

Independent of time budgets, each plan has cost caps:

- `max_anchors: 5` (Factual) — cap on entities resolved from query tokens
- `max_hops: 1` (v0.3 default) — cap on edge traversal depth
- `max_edges_visited: 500` — aggregate cap across all anchors
- `max_candidates: 1000` — cap on memories passed to rerank
- `K_seed: 10` (Associative) — seed recall size
- `K_pool: 100` (Associative) — candidate pool after edge-hop expansion
- `K_seed_affective: 3 * requested_k` (Affective, §4.5 step 2) — seed recall size for affect-weighted candidate pool, capped at **60** to bound cost
- `affect_divergence_sample_rate: 0.01` (Affective, §4.5 step 5) — fraction of Affective calls that compute the neutral-state comparison ranking for Kendall-tau telemetry (default 1%; tests pin ≥ 0.01)
- `τ_graph_filter: 0.3` (Episodic, §4.2 step 3) — entity-signal score above which an Episodic query still triggers a 1-hop graph expansion (chosen so episodic queries with weak entity hints still get graph enrichment; tuned against the routing benchmark)

Hitting a cap is recorded in the trace but is **not** an error. Caps are the mechanism by which cost stays bounded even on pathological queries.

---

## 8. Observability

Three surfaces:

**8.1 Metrics (Prometheus-style, always on):**
- `retrieval_queries_total{plan=...}` — counter per plan
- `retrieval_latency_seconds{plan=..., stage=...}` — histogram, p50/p95/p99
- `retrieval_downgrades_total{from=..., to=..., reason=...}` — counter
- `retrieval_cost_cap_hit_total{cap=...}` — counter (max_anchors, max_edges, etc.)
- `retrieval_empty_result_rate{plan=..., outcome=...}` — ratio by typed outcome (§6.4)
- `retrieval_classifier_method_total{method=heuristic|llm|override|timeout}` — GOAL-3.2 visibility
- `retrieval_bi_temporal_queries_total{mode=current|as_of|include_superseded}` — GOAL-3.4/3.5 usage
- `retrieval_classifier_llm_calls_total` — GOAL-3.13, classifier Stage 2 LLM call count (isolated from write-path and from compiler L5 synthesis)
- `retrieval_classifier_llm_tokens_total{direction=prompt|completion}` — GOAL-3.13, classifier LLM token usage
- `retrieval_classifier_llm_duration_seconds` — GOAL-3.13, classifier LLM latency histogram
- `retrieval_hybrid_truncation_total{dropped_kind=...}` — count of strong signals dropped when Hybrid caps at 2 sub-plans (see §4.7)
- `retrieval_affect_rank_divergence` — GOAL-3.8 gauge, last Kendall-tau (when sampled)

**8.2 Traces (opt-in via `query.explain`):**
Full `PlanTrace` (§6.3) as structured JSON. Includes every intermediate score, every downgrade, every stage timing. Primary debugging surface.

**8.3 Logs (sampled):**
Classifier mismatches (LLM fallback disagreed with heuristic) logged at INFO. Plan errors and timeouts logged at WARN. Nothing on the happy path to keep logs clean.

---

## 9. Testing Strategy

**Unit tests** (fast, deterministic, no DB beyond in-memory fixtures):
- Classifier: each signal in isolation, each routing branch, threshold boundaries
- Each plan's step functions: entity resolution, edge traversal, fusion arithmetic
- Bi-temporal projection: as-of-T correctness, superseded filter, `include_superseded` opt-in
- Typed outcomes (§6.4): construct each variant, assert serialization
- Reranker contract: mock implementation must satisfy purity + bound + score-preservation

**Property tests** (via `proptest`):
- Classifier totality: for any input string, always returns a `Intent` variant
- Determinism (§5.4): identical `(query, query_time, store_snapshot)` always yields byte-identical results
- GUARD-3: after N supersessions, all N historical edges retrievable via `include_superseded` or `as_of`
- GUARD-6: `Affective` plan never *removes* a candidate purely because of affect — affect reorders or downweights but never filters. Formally, for query `Q` and two self-states `S1`, `S2` differing on valence, `result_ids(Q, S1) ∪ result_ids(Q, S2) == candidate_pool(Q)` (up to k-truncation at the tail). The candidate pool is the Affective plan's own seed set from step 2 of §4.5 (`hybrid_recall(query, k=K_seed_affective)`), not the full Associative result set.

**Integration tests** (real DB, seeded fixtures):
- Each plan end-to-end: input query → expected entity/memory/topic IDs in results
- Downgrade paths: craft queries that force each downgrade reason, verify trace
- LLM fallback: mock LLM returns deterministic intent, verify routing
- L5-not-ready: query a domain with no synthesized topics → `RetrievalOutcome::L5NotReady` + associative fallback
- Novel-predicate retrieval (GOAL-3.12): insert `Predicate::Proposed(...)` edges, verify retrievable

**Benchmark-harness tests** (latency only, scored by `v03-benchmarks`):
- Per-plan cold/warm latency under configured budgets
- Hybrid-plan parallelism overhead vs. serial baseline
- `explain=true` overhead vs. `explain=false` (bounded)

**Routing accuracy test** (GOAL-3.1, acceptance criterion):
- Labeled benchmark set ≥ 50 queries across 5 intents
- Assert classifier routing accuracy ≥ 90% (as required)
- Test fails if accuracy drops below threshold — prevents silent regressions

**Affect-divergence test** (GOAL-3.8):
- ≥ 20 queries, run under two self-states with valence differing by ≥ 0.5
- Assert Kendall-tau(ranking_a, ranking_b) < 0.9
- Surfaces as a concrete regression signal if affect weighting gets broken
- **Tunability (per GOAL-3.8 "the exact metric and threshold may be tuned during implementation"):** metric and threshold are config values in `benchmark_config.toml` (keys `affect_divergence.metric = "kendall_tau"`, `affect_divergence.threshold = 0.9`, `affect_divergence.sample_rate ≥ 0.01`). Re-tuning during implementation updates the benchmark config, not this design doc.

---

## 10. Cross-feature References

- **`v03-graph-layer/design.md`**
  - §3: `Entity`, `Edge`, `Predicate` (inc. `Predicate::Proposed`), `KnowledgeTopic` types — consumed by all plans
  - §4.1: storage schema — `graph_edges` (`valid_from`, `valid_to`, `invalidated_at`), `graph_memory_entity_mentions` (memory↔entity provenance join, consumed by Associative plan §4.3), `knowledge_topics` (consumed directly by Abstract plan §4.4 — reads `source_memories`, `contributing_entities`, `cluster_weights`, `embedding`, `summary`)
  - §4.2: `GraphStore` trait — `get_entity`, `edges_of` / `edges_as_of` (bi-temporal, §4.6), `traverse`, `entities_linked_to_memory` / `memories_mentioning_entity` / `edges_sourced_from_memory` (memory-provenance lookups for Associative plan), `list_topics` / `get_topic` (Abstract plan)
  - **Note:** `SubScores` is a retrieval-internal type (§5.2 fusion breakdown); it is **not** defined in graph-layer. Earlier drafts mis-attributed it — corrected here.
- **`v03-resolution/design.md`**
  - Populates the graph that this feature queries
  - Owns the **Knowledge Compiler** background job (resolution §5bis) that synthesizes L5 topics. The Compiler reads memories + `graph_memory_entity_mentions`, clusters them, and writes `knowledge_topics` rows plus their mirrored `EntityKind::Topic` entity rows. Abstract plan (§4.4) consumes these rows; it never triggers synthesis on the read path (the downgrade to Associative with `downgrade_reason = "L5_unavailable"` is the sole behavior when topics are missing).
  - Shares the `graph_pipeline_runs` / `graph_resolution_traces` tables (schema in v03-graph-layer §4.1) for cross-run correlation between resolution and retrieval traces.
- **`v03-migration/design.md`**
  - Backfills entities/edges from v0.2 memories — until backfill runs, Factual/Abstract plans degrade to Associative (§3.4 fallback is correct behavior during migration)
- **`v03-benchmarks/design.md`**
  - Owns absolute latency numbers, routing-accuracy eval set, affect-divergence eval set
  - Uses `explain()` traces (§6.3) as its primary measurement input
- **Existing engramai modules (reused unchanged):**
  - `hybrid_search.rs::hybrid_recall`, `adaptive_hybrid_search`, `reciprocal_rank_fusion`
  - Existing `recall`, `recall_recent`, `recall_associated` methods on `Memory`
  - ACT-R activation module (read-only consumer)
  - Cognitive self-state module (read-only consumer for Affective plan §4.5)

---

## 11. Requirements Traceability Table

| GOAL | Satisfied by |
|---|---|
| GOAL-3.1 (auto-classify 5 intents, ≥ 90% accuracy) | §3.1, §3.2, §3.3, §4.7, §9 routing-accuracy test |
| GOAL-3.2 (heuristic + LLM fallback, method observable) | §3.2 (two-stage), §6.3 `classifier_method`, §8.1 metric |
| GOAL-3.3 (factual queries graph-grounded + provenance + bi-temporal) | §4.1, §4.6, §6.2 (`as_of`) |
| GOAL-3.4 (as-of-T queries) | §4.2 step 4, §4.6, §6.2 `as_of` field |
| GOAL-3.5 (superseded edges remain queryable) | §4.6 `include_superseded`, §6.2, GUARD-3 test in §9 |
| GOAL-3.6 (abstract queries → L5 topics with traces) | §4.4, §6.2 `ScoredResult::Topic` |
| GOAL-3.7 (affect-weighted clustering in L5) | §4.4 `cluster_weights` input (synthesis owned by resolution) |
| GOAL-3.8 (self-state biases ranking, Kendall-tau < 0.9 observable) | §4.5 step 5, §6.3 `AffectTrace`, §9 affect-divergence test |
| GOAL-3.9 (formal tier API: Working/Core/Archived) | §6.5 |
| GOAL-3.10 (typed failure modes, not empty-set collapse) | §6.4 `RetrievalOutcome` |
| GOAL-3.11 (opt-in explain trace) | §6.3 `query.explain` flag, cost-free default |
| GOAL-3.12 (novel predicates retrievable via same API) | §6.6 |
| GOAL-3.13 (L5 LLM cost counted independently) | §6.3 `l5_llm_calls`, §8.1 metric |
| GOAL-3.14 (cognitive state never blocks results) | §4.5 `w_affect < 1.0`, §6.4 `RetrievalOutcome::Ok` even on missing state, §9 GUARD-6 property test |
| GUARD-3 (bi-temporal invalidation never erases) | §4.6, §9 supersession-history test |
| GUARD-6 (cognitive state modulates, never gates) | §4.5, §6.4, §9 GUARD-6 test |
| GUARD-2 (never silent degrade) | §4.4 downgrade with reason, §6.4 `RetrievalOutcome` variants, §4.7 hybrid truncation trace |
| GUARD-8 (affect_snapshot immutable, cosine-only) | §4.5 reads write-time snapshot, no recomputation; cosine used in `affect_similarity` fusion term |

All 14 GOALs + 4 GUARDs covered.
