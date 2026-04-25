# Design: Engram v0.3 — Resolution Pipeline

> **Feature:** v03-resolution
> **GOAL namespace:** GOAL-2.X
> **Master design:** `docs/DESIGN-v0.3.md` (§4 Write path)
> **Requirements:** `.gid/features/v03-resolution/requirements.md`
> **Depends on:** `.gid/features/v03-graph-layer/design.md` (types + storage + CRUD)
> **Status:** Draft for review

## 1. Overview + Scope Boundary

The resolution pipeline is the **write path**. It converts an ingested `MemoryRecord` (v0.2 episodic content passed through `store_raw`) into a populated slice of the v0.3 semantic graph: canonical `Entity` nodes and typed bi-temporal `Edge` rows, linked back to the memory row through provenance columns. The pipeline is staged — extract, resolve, persist — and the contract at the boundary is tight: an ingested memory either produces a complete, atomic graph delta or records a structured extraction failure that downstream code can inspect. No half-writes, no silent skip.

> **Naming note (GUARD-11 canonical decision).** The v0.2 holder type is `Memory` (in `engramai/src/memory.rs`); a single memory row is `MemoryRecord` (in `engramai/src/types.rs`). Earlier drafts used `EngramMemory` for the record type — that spelling is **not** present in the code and has been normalized to `MemoryRecord` here. See v03-retrieval §6.1 for the full canonical naming table.

**In scope.**

- The Stage 3–5 orchestration from master DESIGN-v0.3.md §4: extraction → multi-signal fusion → edge resolution, glued into a single pipeline driver.
- Adapters that reuse the existing v0.2 surface — `EntityExtractor` / `ExtractedEntity` from `entities.rs`, `TripleExtractor` / `Triple` / `Predicate` from `triple_extractor.rs` — and map their output into the v0.3 `Entity` / `Edge` / `Predicate` types from v03-graph-layer §3.
- Resolution / dedup decision logic: candidate retrieval, fusion decision path (merge / create / defer), edge ADD/UPDATE/NONE decision, and the non-destructive UPDATE rewrite (GOAL-2.10).
- The atomic persist step (single SQLite transaction that writes memory row + entities + edges).
- **Preserve-plus-resynthesize** behavior for re-extraction (§4, r1 review requirement): re-running the pipeline on an already-resolved memory is a merge, not a replace.
- Async / background execution mode for `store_raw` (GUARD-1: L1/L2 must succeed even when resolution fails).
- Public API additions (re-extract trigger, status introspection) and observability hooks.

**Out of scope — deferred to other features.**

- **Data model.** `Entity`, `Edge`, `Predicate`, `EdgeEnd`, `ResolutionMethod`, `EntityAlias` are defined in `v03-graph-layer/design.md` §3 and are referenced, not redefined.
- **Storage / SQL.** The `graph_entities`, `graph_edges`, `graph_entity_aliases`, `graph_predicates`, `graph_extraction_failures` tables and the `GraphStore` trait live in `v03-graph-layer/design.md` §4. The resolution pipeline is a *caller* of `GraphStore`, never a direct SQL writer for graph tables.
- **CRUD API.** `GraphStore::upsert_entity`, `insert_edge`, `invalidate_edge`, `merge_entities`, `record_extraction_failure` are defined in `v03-graph-layer/design.md` §5. This document specifies the *call sequence*, not the method signatures.
- **Read path.** Dual-level retrieval, mood-congruent recall, query classification → `v03-retrieval` (GOAL-3.X).
- **Migration backfill.** v0.2 → v0.3 backfill uses this pipeline but is orchestrated by `v03-migration` (GOAL-4.X).
- **Consolidation-time edge audit.** The §6 audit step in master DESIGN is a *consolidation* concern, not a write-path concern. The resolution pipeline is forward-only.
- **Fusion weight tuning.** Initial weights are guesses; empirical tuning is a benchmarks / §8 concern.

**Reader orientation.** Section §3 is the structural story (what happens in order, stage by stage). Section §4 is the r1-driven correctness story (how re-extraction behaves). Section §5 is the execution-model story (sync vs async, what fails independently). §6 is the public surface. §7–§9 are observability / error / test.

## 2. Requirements Coverage

| GOAL      | Priority | Satisfied by section(s)                 | Notes                                                                                 |
| --------- | -------- | --------------------------------------- | ------------------------------------------------------------------------------------- |
| GOAL-2.1  | P0       | §3.1, §5.1, §8.2                        | Ingestion dedup key + idempotent re-run; `store_raw` enqueues exactly once.           |
| GOAL-2.2  | P0       | §3.5, §8.1                              | Single SQLite transaction wraps entity + edge + memory writes per episode.            |
| GOAL-2.3  | P0       | §6.3, §7, §8.1                          | Structured `ExtractionFailure` row per failed stage; queryable via status API.        |
| GOAL-2.4  | P1       | §3.4, §5.3, §8.3                        | In-process serialization of overlapping candidate sets; merge_entities re-entry safe. |
| GOAL-2.5  | P1       | §3.4.1                                  | Bounded top-K candidate retrieval from `GraphStore`.                                  |
| GOAL-2.6  | P0       | §3.4.2                                  | Fusion combines s1–s8 signals into one confidence; per-signal contributions observable. |
| GOAL-2.7  | P0       | §3.4.3, §7                              | Decision path (merge / create / defer) recorded on `ResolutionTrace`.                 |
| GOAL-2.8  | P1       | §3.4.2 (s8)                             | Somatic fingerprint distance participates in fusion; surfaced in trace.               |
| GOAL-2.9  | P0       | §3.4.4                                  | Edge ADD / UPDATE / NONE decision with cheap-path short-circuit before LLM.           |
| GOAL-2.10 | P0       | §3.4.4, §3.5, §4                        | Supersession sets `invalidated_at` + `invalidated_by` + successor `supersedes`; never deletes. |
| GOAL-2.11 | P1       | §7                                      | Per-stage LLM call counter on `ResolutionTrace`; rolling window metric.               |
| GOAL-2.12 | P1       | §3.4.4, §4                              | Retro-evolution produces a new edge version with provenance to triggering episode; original preserved. |
| GOAL-2.13 | P2       | §3.3.2                                  | Proposed predicate path preserves raw label; novel-predicate counter emitted.         |
| GOAL-2.14 | P1       | §7                                      | Rolling avg LLM-calls-per-episode computed over N≥100; warn threshold configurable.   |
| GOAL-3.6 *(cross-feature)* | P1 | §5bis                              | Produce-side of L5 knowledge topics (consumed by v03-retrieval §4.4). Owned here by architectural proximity (shares write-path worker infrastructure). |
| GOAL-3.7 *(cross-feature)* | P1 | §5bis.1, §5bis.7                   | Cost isolation: compiler runs as separate scheduled job with its own LLM-call metric namespace (`knowledge_compile_*`). |

GUARD alignment (from master):

- **GUARD-1, GUARD-2** — §5 (async separation so resolution failure never blocks L1/L2) + §8.1 (every failure surfaced).
- **GUARD-3** — §3.4.4 + §4 (supersession never erases).
- **GUARD-6** — §3 (affect *annotates* via s8, never gates admission).
- **GUARD-8** — §3.4.2 (pipeline reads `Episode.affect_snapshot`, never recomputes it).
- **GUARD-9** (hard) — no new deps — §3 pipeline uses only crates already in the v0.2 workspace (`engramai`, `crossbeam`, `tokio`, `anthropic-sdk`, `rusqlite`); §5bis compile job uses the same Anthropic / Ollama LLM providers as the classifier/extractor. No new external crates are introduced by this design.
- **GUARD-11** (hard) — v0.2 API compat — §3.1 `store_raw` signature unchanged; §6.1 same return type; §9.4 regression tests assert existing `crates/engramai/tests/` pass unmodified.
- **GUARD-12** — §7 (LLM call budget telemetry). **Budget tiers (reconciling GUARD-12 / GOAL-2.14):**
  - **Target** (ship gate): avg ≤ 3 LLM calls per episode (happy path: classifier 1 + extractor 1 + rare LLM tiebreaker ≤ 1 avg). This is the v03-benchmarks GOAL-5.4 ship gate.
  - **Warn** (soft alarm): rolling avg > 4 over N≥100 episodes — triggers `resolution_llm_calls_over_budget_total` counter and operator-visible warning (§7).
  - **Fail**: no hard cap is set. Budget violations are advisory only, consistent with GUARD-12 being `soft` severity.

## 3. Pipeline Stages

The pipeline is a linear sequence of five stages, each a pure function over an evolving `PipelineContext`. Every stage is independently observable (span + counter, §7) and independently failure-recordable (§8). The stages map onto master DESIGN §4.1 thus: §3.1 = Stage 1 (episode admission, already covered by v0.2 `store_raw`), §3.2 = Stage 3 extraction (entity half), §3.3 = Stage 3 extraction (edge half), §3.4 = Stages 4–5 (resolution), §3.5 = Stage 6 (persist).

```
MemoryRecord (raw)
       │
       ▼
┌──────────────────┐
│ 3.1 Ingestion    │  v0.2-compat: store_raw returns here (sync path)
│ memory row draft │
└────────┬─────────┘
         │ enqueue for background
         ▼
┌──────────────────┐
│ 3.2 Entity extract│  reuses EntityExtractor; emits ExtractedEntity[]
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 3.3 Edge extract  │  reuses TripleExtractor; emits Triple[]
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 3.4 Resolve / dedup
│  a. candidate retrieve
│  b. multi-signal fusion (s1–s8)
│  c. entity decision
│  d. edge decision (ADD / UPDATE / NONE)
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ 3.5 Persist       │  one SQLite transaction: memory+entities+edges
└────────┬─────────┘
         │
         ▼
   ResolutionTrace (persisted + observable, §7)
```

The context passed between stages:

```rust
/// Threaded through every stage. Mutated in place; read-only to the extractors
/// and read-write to the resolution / persist stages.
pub(crate) struct PipelineContext {
    pub memory: MemoryRecord,                 // v0.2 type, extended with episode_id
    pub episode_id: Uuid,                     // L1 anchor (created in §3.1)
    pub affect_snapshot: Option<SomaticFingerprint>, // copied, never recomputed (GUARD-8)

    // Filled by §3.2
    pub extracted_entities: Vec<ExtractedEntity>,
    // Filled by §3.3
    pub extracted_triples: Vec<Triple>,

    // Filled by §3.4
    pub entity_resolutions: Vec<EntityResolution>, // one per extracted entity
    pub edge_resolutions: Vec<EdgeResolution>,     // one per extracted triple

    // Accumulators
    pub trace: ResolutionTrace,                    // §7
    pub failures: Vec<StageFailure>,               // §8
}
```

### 3.1 Ingestion

The v0.2 `store_raw(content: &str, …) -> Result<MemoryId>` call is the entry point and its **signature does not change** (GUARD-11, r1 review). What *does* change is the post-write behavior: after the v0.2 admission write (L1 Episode row + L2 MemoryRecord row) completes, `store_raw` enqueues a `PipelineJob { memory_id, episode_id }` onto a bounded in-process queue and returns. The v0.2 synchronous contract — "the memory is stored when `store_raw` returns" — is preserved; the new v0.3 contract adds — "the graph projection of that memory is populated *eventually*, asynchronously, and its status is introspectable via `extraction_status(memory_id)`."

Ingestion responsibilities:

1. Mint the `Episode.id` (`Uuid::now_v7` for time-ordered inserts).
2. Capture `affect_snapshot` from the current `AffectState` (immutable from here on — GUARD-8).
3. Write L1 + L2 rows via the existing v0.2 admission path (atomic, one transaction).
4. Enqueue the `PipelineJob`. Enqueue failure (queue full, see §5.2) is **non-fatal** — it records a `StageFailure { stage: Ingest, kind: QueueFull }` but the v0.2 admission remains committed.
5. Return `MemoryId` to the caller.

**Idempotence key (GOAL-2.1).** `PipelineJob` carries `(memory_id, episode_id)`. The pipeline driver maintains a `SET running` and a `SET completed` (both persisted in a lightweight `graph_pipeline_runs` table). Re-enqueue of an already-running or already-completed job is dropped at dequeue time with a debug log. A re-extract (explicit, §6.2) uses a distinct `PipelineJob { .., mode: ReExtract }` that is never dropped; see §4.

### 3.2 Entity Extraction

Reuses the existing `EntityExtractor` from `crates/engramai/src/entities.rs` unchanged. The v0.2 extractor runs Aho-Corasick + regex scans and returns `Vec<ExtractedEntity>`. The pipeline wraps it; it does not replace it.

```rust
// crates/engramai/src/resolution/stage_extract.rs
pub(crate) fn extract_entities(
    extractor: &EntityExtractor,
    ctx: &mut PipelineContext,
) -> Result<(), StageFailure> {
    let span = tracing::info_span!("resolution.stage.entity_extract").entered();
    let out = extractor.extract(&ctx.memory.content);
    ctx.trace.entities_extracted = out.len() as u32;
    ctx.extracted_entities = out;
    Ok(())
}
```

**Adapter boundary — `ExtractedEntity` → `Entity` (draft).** This stage does *not* mint `Entity` rows. It only collects `ExtractedEntity` mentions. The upgrade to canonical `Entity` happens in §3.4.3. The adapter function is:

```rust
/// Build a *draft* v0.3 Entity from a v0.2 mention. The caller fills in
/// `id` (either a new Uuid or a reused canonical id from resolution) and
/// first_seen / last_seen from ctx.
pub(crate) fn draft_entity_from_mention(
    m: &ExtractedEntity,
    ctx: &PipelineContext,
) -> DraftEntity {
    DraftEntity {
        canonical_name: m.name.clone(),
        kind: map_entity_type(&m.entity_type), // EntityType -> EntityKind
        aliases: vec![m.normalized.clone()],
        first_seen: ctx.memory.occurred_at,
        last_seen: ctx.memory.occurred_at,
        somatic_fingerprint: ctx.affect_snapshot.clone(),
    }
}
```

The `map_entity_type` function is a total mapping from v0.2 `EntityType` (see `crates/engramai/src/entities.rs::EntityType`) to v0.3 `EntityKind` (`v03-graph-layer/design.md` §3.1). The current mapping:

| v0.2 `EntityType` | v0.3 `EntityKind` | Notes                                           |
| ----------------- | ----------------- | ----------------------------------------------- |
| `Person`          | `Person`          | lossless                                        |
| `Organization`   | `Organization`   | lossless                                        |
| `Project`         | `Organization`   | projects treated as organizational entities    |
| `Technology`      | `Artifact`        | subtype loss (Technology is a kind of Artifact) |
| `Concept`         | `Concept`         | lossless                                        |
| `File`            | `Artifact`        | subtype loss                                    |
| `Url`             | `Artifact`        | subtype loss                                    |
| `Other(s)`        | `Other(s)`        | string payload preserved verbatim               |

The mapping is total and lossless on identity (every v0.2 mention produces exactly one v0.3 kind); kind-specific subtypes (File vs Url vs Technology) are preserved in `Entity.summary` / alias rows when precision matters. If new v0.2 variants are added, the mapping must be extended here and in code simultaneously (this is a GUARD-11 compat-surface concern).

**Stage 3 extraction context (master §4.2).** When the extractor supports graph-context-aware prompting (future LLM-backed extractor), this stage additionally fetches recent-neighborhood entity names via `GraphStore::top_k_recent_for_session(session_id)` and threads them into the prompt. For the v0.2 `EntityExtractor` (pattern-based), there is no prompt, so this enrichment is a no-op. The enrichment hook exists so §3.2 can upgrade to the LLM extractor without structural change.

### 3.3 Edge Extraction

Reuses the existing `AnthropicTripleExtractor` / `OllamaTripleExtractor` (`TripleExtractor` trait, `triple_extractor.rs`). v0.2's extractor returns `Vec<Triple>` with `Predicate` values drawn from a small enum (IsA / PartOf / Uses / DependsOn / CausedBy / LeadsTo / Implements / Contradicts / RelatedTo).

```rust
pub(crate) fn extract_edges(
    extractor: &dyn TripleExtractor,
    ctx: &mut PipelineContext,
) -> Result<(), StageFailure> {
    let span = tracing::info_span!("resolution.stage.edge_extract").entered();
    match extractor.extract_triples(&ctx.memory.content) {
        Ok(triples) => {
            ctx.trace.edges_extracted = triples.len() as u32;
            ctx.trace.llm_calls_edge_extract += 1;
            ctx.extracted_triples = triples;
            Ok(())
        }
        Err(e) => Err(StageFailure {
            stage: PipelineStage::EdgeExtract,
            kind: classify_llm_error(&e),
            message: e.to_string(),
        }),
    }
}
```

#### 3.3.1 Predicate Normalization (adapter)

v0.2 `Predicate` (9 variants) maps into v0.3 `Predicate::Canonical(CanonicalPredicate)` via an exhaustive match. The v0.3 `CanonicalPredicate` enum is a superset (`v03-graph-layer/design.md` §3.3). Mapping is:

| v0.2 `Predicate` | v0.3 `Predicate`                             |
| ---------------- | -------------------------------------------- |
| `IsA`            | `Canonical(CanonicalPredicate::IsA)`         |
| `PartOf`         | `Canonical(CanonicalPredicate::PartOf)`      |
| `Uses`           | `Canonical(CanonicalPredicate::Uses)`        |
| `DependsOn`      | `Canonical(CanonicalPredicate::DependsOn)`   |
| `CausedBy`       | `Canonical(CanonicalPredicate::CausedBy)`    |
| `LeadsTo`        | `Canonical(CanonicalPredicate::LeadsTo)`     |
| `Implements`     | `Canonical(CanonicalPredicate::Implements)` |
| `Contradicts`    | `Canonical(CanonicalPredicate::Contradicts)` |
| `RelatedTo`      | `Canonical(CanonicalPredicate::RelatedTo)`   |

#### 3.3.2 Proposed predicates (GOAL-2.13)

Future extractors may emit predicates not in the canonical catalog. When a triple's predicate string fails canonical lookup, it becomes `Predicate::Proposed(label)` preserving the verbatim LLM label. The pipeline emits a `novel_predicate_emitted` counter (§7) tagged with the label. **Automatic promotion is out of scope for v0.3** (deferred to v0.4 per ISS-031). The v0.2 `Predicate::from_str_lossy` already falls back to `RelatedTo`; the v0.3 pipeline intercepts *before* that fallback, preserves the original string as `Proposed`, and only falls back to `RelatedTo` if the caller explicitly asks for strict-canonical mode.

### 3.4 Resolution / Dedup

This is the heart of the pipeline and the most subtle stage. It has four sub-steps, run per-entity and then per-edge.

#### 3.4.1 Candidate Retrieval (GOAL-2.5)

For each `ExtractedEntity` mention:

```rust
let candidates: Vec<Entity> = graph_store.search_candidates(
    &mention.normalized,
    mention_kind,                      // restricts to same EntityKind
    top_k,                             // config, default 10
    ctx.memory.session_id.as_deref(),
)?;
```

`search_candidates` is defined in `v03-graph-layer/design.md` §5 and uses: (a) exact alias match on `graph_entity_aliases`, (b) embedding similarity over `Entity.canonical_name + summary`, (c) recency boost for entities with `last_seen` in the current session. The K is bounded (`config.candidate_top_k`, default 10, hard cap 50).

**Cap enforcement.** `candidate_top_k` is clamped at runtime: `top_k = min(config.candidate_top_k, 50)`. A warning is logged (once per process) if the user-configured value exceeded the cap. Config validation does NOT fail on over-cap values — this is forward-compatible (future versions may raise the cap), and silent clamping is preferable to boot failure on a tunable parameter.

#### 3.4.2 Multi-Signal Fusion (GOAL-2.6, GOAL-2.8)

For each `(mention, candidate)` pair, compute eight signals per master DESIGN §4.3:

| signal | name                | source                                                              |
| ------ | ------------------- | ------------------------------------------------------------------- |
| s1     | semantic_similarity | cosine(mention_embedding, candidate.embedding)                      |
| s2     | name_match          | Jaro-Winkler / normalized exact-match over aliases                  |
| s3     | graph_context       | overlap of co-mentioned entities between this memory and candidate  |
| s4     | recency             | decayed `candidate.last_seen` relative to `memory.occurred_at`      |
| s5     | cooccurrence        | Hebbian link strength from v0.2 `hebbian_links` for cand's entities |
| s6     | affective_continuity| distance between this memory's valence and cand's aggregate valence |
| s7     | identity_hint       | structural hints (same session speaker, same file path)             |
| s8     | somatic_match       | 1 − euclidean(ctx.affect_snapshot, candidate.somatic_fingerprint)   |

Fusion:

```rust
confidence = Σᵢ wᵢ · sᵢ         // weights in config, sum to 1.0, initial guesses per master §8.3
```

Each per-signal value and the final confidence are captured on `ResolutionTrace` (§7), making per-signal contribution individually observable (GOAL-2.6, GOAL-2.8). If any signal is unavailable (missing embedding, no candidate fingerprint), its weight is redistributed proportionally across the remaining signals; the fact that a signal was absent is recorded (`trace.signals_missing[s]`). `affect_snapshot` is read from `ctx`, never recomputed (GUARD-8).

**Redistribution formula (deterministic, per GOAL-2.11 / GUARD-4).** Let `M` be the set of missing signals with total weight `sum_missing = Σ_{j ∈ M} w_j`. For each present signal `i`:

```
w_i' = w_i / (1 - sum_missing)
```

Equivalently, `w_i' = w_i + w_i · sum_missing / sum_present`. This preserves the *relative* importance among present signals (proportional redistribution), not uniform redistribution.

Example: if `s2 (w=0.15)` and `s5 (w=0.20)` are missing, `sum_missing = 0.35`, and each present `w_i` scales by `1 / (1 − 0.35) = 1.538…`. If all signals are missing (degenerate), the mention is treated as `Decision::CreateNew` with `confidence = 0.0` and `StageFailure { kind: NoFusionSignals }` recorded (never silent — GUARD-2).

#### 3.4.3 Entity Decision (GOAL-2.7)

Per mention, after fusion over all candidates:

```rust
let best = candidates_scored.iter().max_by(|a, b| a.conf.cmp(&b.conf));
let decision = match best {
    Some(c) if c.conf >= config.merge_threshold     => Decision::MergeInto(c.id),
    Some(c) if c.conf >= config.defer_threshold     => Decision::DeferToLlm(c.id),
    _                                                => Decision::CreateNew,
};
```

Thresholds are config, not hardcoded (per GOAL-2.7 "thresholds are design/tuning decisions, not requirements"). `Decision::DeferToLlm` triggers one Stage-5-style mem0 prompt that chooses between MERGE and CREATE; the LLM result feeds back into the same `Decision` enum. The full record is an `EntityResolution`:

```rust
pub struct EntityResolution {
    pub mention: ExtractedEntity,
    pub candidates: Vec<ScoredCandidate>,   // id + per-signal scores + conf
    pub decision: Decision,
    pub method: ResolutionMethod,           // Automatic | LlmTieBreaker | AgentCurated | Migrated
    pub assigned_entity_id: Uuid,           // filled by §3.5
}
```

`EntityResolution` is appended to `ctx.entity_resolutions`. After all mentions are resolved, `ctx.entity_resolutions` defines the set of canonical IDs referenced by later edge resolution.

#### 3.4.4 Edge Decision (GOAL-2.9, GOAL-2.10, GOAL-2.12)

For each extracted `Triple`, map subject and object to canonical entity IDs using the per-mention resolutions from §3.4.3. (If object is a literal — no matching mention — keep it as `EdgeEnd::Literal`.) Then for each `(subject_id, predicate, object)` lookup existing edges in the graph:

```rust
let existing = graph_store.find_edges(subject_id, &predicate, &object, /*valid_only=*/ true)?;
let decision = match (existing.as_slice(), new_edge_confidence) {
    ([], _)                      => EdgeDecision::Add,
    ([prior], conf)
        if prior.object == object && conf > prior.confidence + ε
                                 => EdgeDecision::Update { supersedes: prior.id },
    ([prior], _) if prior.object != object
                                 => EdgeDecision::Update { supersedes: prior.id },
    ([prior], _) if equals_within_tolerance(prior, triple)
                                 => EdgeDecision::None,      // redundant
    (_many, _)                   => EdgeDecision::DeferToLlm, // mem0 prompt, master §4.4
};
```

**UPDATE is non-destructive (GOAL-2.10, GUARD-3).** "Update" never mutates the prior row. It produces a new `Edge` row with a fresh `id`, sets `supersedes = Some(prior.id)`, and calls `GraphStore::invalidate_edge(prior.id, new.id, now)` which sets `prior.invalidated_at = now` and `prior.invalidated_by = Some(new.id)`. The invalidation is the *only* permitted mutation on a prior edge, and it is itself append-only in audit metadata.

**Retro-evolution (GOAL-2.12).** When this pipeline runs in `mode = ReExtract` or when a later episode's `affect_snapshot` differs sharply from the original (distance > `config.retro_evolution_threshold`) for an edge whose subject has `identity_confidence` drifting down, the pipeline emits a *new* edge version with `supersedes = prior.id` and `provenance.triggered_by_episode = ctx.episode_id`. The original remains queryable. This is triggered only when the incoming triple matches the prior subject+predicate; it is not a scan of all prior edges.

### 3.5 Persist

Single transaction via `Storage::with_graph_tx` (defined in v03-graph-layer §4.3). Writes, in order:

1. All `EntityResolution` rows → `GraphStore::upsert_entity(...)` for `MergeInto` (updates `last_seen`, `activation`, optionally refreshes `somatic_fingerprint`) or `GraphStore::insert_entity(...)` for `CreateNew`. Alias rows inserted in bulk.
2. All `EdgeDecision::Update` → `GraphStore::insert_edge(new)` then `GraphStore::invalidate_edge(prior_id, new.id, now)`. Order matters — successor row must exist before prior is marked invalidated so foreign-key `invalidated_by` is satisfiable.
3. All `EdgeDecision::Add` → `GraphStore::insert_edge`.
4. Memory-row back-linking: `GraphStore::link_memory_to_entities(memory_id, &assigned_entity_ids)` writes provenance rows in `graph_memory_entity_mentions`.
5. `ResolutionTrace` + any `StageFailure` rows flushed in the same transaction.
6. `graph_pipeline_runs` row marked `completed_at = now`.

If any step fails, the transaction rolls back, `graph_pipeline_runs` records `failed_at = now` with the error, and the job is not retried automatically (per requirements "out of scope" — retry is operator-driven). GOAL-2.2 is satisfied: either all graph writes succeed, or none do — but the L1/L2 writes from §3.1 are already committed in a prior transaction and are untouched (GUARD-1).

## 4. Preserve-Plus-Resynthesize

_This section is the r1 review response for open question Q7 as it applies to the write path._

### 4.1 Problem

Re-running the resolution pipeline on a memory that has already been resolved must not silently destroy prior work. Two classes of prior work are at risk:

1. **Manually-curated graph state** — an agent tool call (Letta-style) or an operator has hand-edited an entity (renamed it, set `identity_confidence = 1.0`, merged two entities) or hand-asserted an edge (`AgentCurated` resolution method). A naive re-extract that overwrites with LLM output would erase that work.
2. **Prior automatic edges on the same memory** — even fully-automatic prior edges represent observable, probably-correct facts. A re-extract that re-runs with a slightly different LLM temperature or a newer prompt could produce a partially-different edge set. Wholesale deletion and replacement would lose edges that are still valid and would violate GUARD-3 (no silent erasure).

The pipeline runs in `mode = ReExtract` under three circumstances: (a) operator-triggered `reextract --memory <id>` (see §6.2), (b) operator-triggered `reextract --failed` replaying memories that recorded an `ExtractionFailure`, (c) v03-migration backfill for the subset of memories that had partial v0.2 resolution data. In all three, the correct behavior is **additive merge**, not replace.

### 4.2 Strategy — Additive Merge, Never Destructive

The re-extraction run executes §3.1–§3.5 with these differences:

**(a) Ingestion (§3.1) is skipped.** The memory row already exists. The episode ID is reused.

**(b) Entity resolution (§3.4.3) is diff-biased toward existing.** Before fusion, the pipeline fetches `GraphStore::entities_linked_to_memory(memory_id)` — the set of entities *currently* linked to this memory. These entities are injected into the candidate pool with a +δ bonus on s3 (graph_context) so a re-extract naturally re-resolves to the same canonical IDs when the content is unchanged. This is idempotence by construction, not by luck (GOAL-2.1).

**(c) Edge resolution (§3.4.4) uses a three-way diff.** For each `(subject, predicate, object)` triple, the decision considers: the *newly extracted* triple, the *existing* edge(s) currently linking this memory to the graph (`GraphStore::edges_sourced_from_memory(memory_id)`), and the *current graph state* for that subject+predicate. The decision table:

| new_extraction | existing edge on this memory            | current graph state        | action                                                                  |
| -------------- | --------------------------------------- | -------------------------- | ----------------------------------------------------------------------- |
| present        | present, identical                      | present, valid, identical  | **Skip** — no-op, no new row, no invalidation. Idempotent.              |
| present        | present, `method = AgentCurated`        | any                        | **Preserve** — keep the curated edge; record `trace.preserved_curated`. Do NOT create a new edge. |
| present        | present, automatic, different conf > ε | any                        | **Supersede** (reason = `confidence_changed`): emit a *new* edge row with `supersedes = prior.id`; set `prior.superseded_by = new.id`. Both rows remain in storage (append-only per GUARD-3). Only applied when new conf strictly exceeds prior conf by a configurable margin; otherwise skip. |
| present        | absent                                  | absent                     | **Add** — normal §3.4.4 insert.                                         |
| present        | absent                                  | present elsewhere, valid   | **Link** — insert a new `Edge` row with the same `(subject, predicate, object)` but a fresh `id` and `source_memory_id = this.memory_id`. Does not invalidate the other edge; just adds independent provenance. |
| absent         | present                                 | present, valid             | **Preserve** — do not invalidate. The extractor's silence is not evidence of contradiction. Record `trace.extractor_silent_on = [prior.id]`. |
| absent         | present, `method = AgentCurated`        | any                        | **Preserve** — same, plus log at info level.                            |
| absent         | absent                                  | any                        | N/A — nothing to diff.                                                  |

**The critical row is "absent / present / valid → Preserve".** Re-extraction NEVER causes an edge to be invalidated just because the new LLM pass didn't surface it. Invalidation must come from a *contradicting* observation, not from *silence*. This is the direct r1 requirement.

**(d) Entity-level preservation.** Entity fields with explicit user-verified provenance are preserved:

- `identity_confidence` — only raised by re-extract, never lowered. (Contradicting evidence is a separate *consolidation* concern.)
- `attributes.history` — append-only; re-extract may add, never remove.
- `canonical_name` — only changed if the new resolution produces a strictly higher-confidence assignment AND the current name was not agent-curated.
- `agent_affect`, `somatic_fingerprint` — recomputed as aggregates over the (now-identical) mention set; numerically may change slightly due to weight updates, which is acceptable per GUARD-8 (entity aggregates are explicitly not immutable; only *episode* snapshots are).
- `user_verified: bool` flag (if set on the entity) — never cleared.

**(e) Deletion is out of scope.** The resolution pipeline *cannot* delete entities or edges. Deletion is an explicit API call (`GraphStore::hard_delete_entity`, `hard_delete_edge`) that lives in v03-graph-layer §5 and is guarded by an agent-tool contract. Re-extraction may only *add rows* or *mark prior rows invalidated* (and even the latter only in the "contradicting" rows of the decision table above).

**Legend — supersede semantics.** "Supersede" creates a new edge row with `supersedes = prior_edge.id` and sets `prior_edge.superseded_by = new_edge.id`. Both edges remain in storage (append-only per GUARD-3). Older edges are filterable via bi-temporal queries (`valid_only = true`) but are never silently dropped from persistent storage.

### 4.2.1 Tradeoff: Silence Is Not Delete

The core contract of §4.2 — **extractor silence is not a delete signal** — is a deliberate design choice with operator-visible consequences:

- ✅ **Pro: Re-extracts are stable and safe.** Extractor volatility (different LLM temperature, updated prompt, model upgrade) does not churn the graph. Two re-runs on unchanged content produce the same graph state (modulo `updated_at`).
- ❌ **Con: Facts cannot be retracted by extraction alone.** To record "fact X is no longer true" at time T, the system requires either **(a)** an explicit contradicting edge from a new episode (e.g., "Alice left Acme" explicitly extracted from a later memory), or **(b)** agent curation via `GraphStore::invalidate_edge(edge_id, reason)`.

This tradeoff was chosen because extractor output is not statistically reliable enough to distinguish "the fact became untrue" from "the extractor missed it this time". The bi-temporal model (GUARD-3) preserves the historical view regardless: a retracted-via-contradiction edge remains queryable with `include_invalidated = true`.

Operators expecting "re-extract refreshes the knowledge graph" should read this as **re-extract refines, it does not refresh**. Stale facts live forever unless explicitly contradicted or curated.

### 4.3 Provenance of Re-Extract Runs

Every row inserted during a `ReExtract` run carries `provenance.reextract_run_id = ctx.pipeline_run_id` in its JSON metadata. This gives operators an audit query: "show me everything that changed in the pipeline re-run from 2026-04-24T12:00Z," including which edges were preserved unchanged (recorded in `ResolutionTrace.preserved_edge_ids`) and which were superseded.

### 4.4 Invariant Summary (the r1 contract)

1. Re-extraction is **idempotent** on unchanged content: two runs produce the same graph state modulo `updated_at` timestamps.
2. Re-extraction is **monotonic** with respect to curated data: no curated field, alias, or edge is ever removed or overwritten.
3. Re-extraction **never invalidates an edge** from the absence of a new extraction alone; invalidation requires an active contradiction in the new triple set.
4. Every preserve / merge / supersede decision is recorded in `ResolutionTrace` and is queryable by memory_id and by pipeline_run_id.

## 5. Batching & Async

### 5.1 Execution Model

`store_raw` is a **fast, synchronous** call: L1 + L2 writes happen inline; the resolution pipeline runs in a background worker pool. The split exists to satisfy GUARD-1 (episodic completeness regardless of L4 failure) and to meet the v0.2 latency contract that callers of `store_raw` expect.

```
 caller ──► store_raw(content) ──► [L1+L2 commit] ──► enqueue PipelineJob ──► return MemoryId
                                                             │
                                                             ▼
                                             ┌─────────────────────────────┐
                                             │ bounded crossbeam channel   │
                                             │  capacity = config.queue_cap│
                                             └──────────────┬──────────────┘
                                                            │
                                               ┌────────────┴────────────┐
                                               ▼                         ▼
                                  resolution_worker[0]   ...   resolution_worker[N-1]
                                       (runs §3.2 … §3.5 for one job)
```

Worker pool defaults: `N = 1` (single-writer serializes SQLite writes and simplifies reasoning; multi-writer is possible since SQLite WAL allows it, but fusion over overlapping candidate sets is easier to reason about with N=1). Configurable via `ResolutionConfig::worker_count` up to a small cap (8). GOAL-2.4 (in-process concurrency correctness) is satisfied when N>1 by serializing on a per-session lock: two jobs with the same `session_id` are dispatched to the same worker in order of enqueue, so overlapping entity-candidate sets are resolved sequentially. Cross-session jobs run in parallel.

#### 5.1.1 Concurrency Details

**Worker dispatch.** `worker_id = hash(session_id) % N` (FxHash for speed + determinism). Memories with no `session_id` (standalone ingestions) use `worker_id = hash(memory_id) % N`, treating each memory as a singleton session. This preserves the session-affinity invariant (same session → same worker → sequential processing of overlapping candidate sets) without a special case for the standalone path.

**Worker crash recovery.** If a worker panics or the process restarts mid-job:
- Queued jobs (never picked up) are re-enqueued on startup by re-scanning `graph_pipeline_runs` rows with `status = queued`.
- In-flight jobs (picked up but not committed) are marked `Failed(worker_crashed)` on restart and require operator replay via `reextract --failed` (GOAL-2.3 surfaces the failure reason). Automatic retry of in-flight-on-crash is out of scope (per requirements "Out of Scope: automatic retry").

**Testing.** The GOAL-2.4 property test runs with `N ∈ {1, 2, 4}` to cover single-worker, inter-worker, and fan-out paths. The N=1 case is a trivial sanity check; N=2 exercises the session-affinity dispatch; N=4 stresses the cross-session parallel path.

### 5.2 Backpressure and Queue Semantics

The queue is bounded (`config.queue_cap`, default 10_000 jobs). When full:

- `store_raw` still succeeds on L1/L2. The enqueue failure is recorded as a `StageFailure { stage: Ingest, kind: QueueFull }` on the memory row and also incremented as a `pipeline_queue_full_total` counter (§7).
- The operator is surfaced via telemetry warning. The memory's `extraction_status` returns `Pending(queue_full)` and is recoverable via `reextract --pending`.
- This is a **visible degradation**, per GUARD-2. No silent drop.

Queue ordering is FIFO with two exceptions: (a) `PipelineJob { mode: ReExtract }` jobs are dispatched at normal priority but marked non-droppable, (b) shutdown drains the queue up to a configurable deadline before the worker pool stops.

#### 5.2.1 Backlog Observability (operator footgun note)

There is no upper bound on `Pending(queue_full)` depth across time — sustained high-ingest + slow-extract periods can accumulate millions of Pending memories. This is acceptable for v0.3 MVP (GUARD-1 correctly prioritizes episodic write availability over graph-extraction throughput; ingest is *never* blocked by backlog), but it creates an operational footgun if unmonitored: `reextract --pending` on a large backlog can take hours.

Operators MUST monitor (§7):
- `resolution_pending_memories_total{reason}` — current count of Pending memories by reason (`queue_full`, `awaiting_worker`).
- `resolution_pending_memory_oldest_age_seconds` — age of the oldest Pending memory. Alert if this exceeds the extraction SLO.

Sustained growth in `resolution_pending_memories_total{reason="queue_full"}` indicates the worker pool is under-provisioned for the ingest rate. Remediations (in order of preference): scale `worker_count`, raise `queue_cap`, or accept degradation and schedule `reextract --pending` batches during low-ingest windows. Per GUARD-1 there is **no automatic backpressure on `store_raw`** — ingest is never blocked.

### 5.3 Failure Handling

A pipeline failure (any stage from §3.2 to §3.5) does **not** remove the memory row. Specifically:

- The L1/L2 writes from §3.1 remain committed (GUARD-1).
- A `graph_extraction_failures` row is written (v03-graph-layer §4.1) with the stage, error kind, message, and timestamp.
- The `graph_pipeline_runs` row for this job is marked `status = failed`.
- `extraction_status(memory_id)` now reports `Failed { stage, kind, at }` — the failure is *queryable data*, not just a log line (GUARD-2).
- Partial results from earlier stages are **not** persisted. If entity extraction succeeded but edge resolution failed, neither is written — we do not want to present a half-extracted graph to readers who cannot tell it is half.

Automatic retry is **out of scope** per requirements. Operator replays via `reextract --failed` (§6.2).

### 5.4 Batching Opportunity (non-required)

When multiple queued jobs share a session, the worker MAY batch candidate-retrieval calls across them (one SQL query returns candidates for several mentions). This is a performance optimization; it is not a correctness requirement and may be added after MVP.

## 5bis. Knowledge Compiler (L5 Synthesis)

> **Owner of GOAL-3.6 / GOAL-3.7 produce-side.** The retrieval feature (v03-retrieval §4.4) *consumes* L5 knowledge topics; this section defines the background job that *produces* them. Storage schema (`knowledge_topics`) lives in v03-graph-layer §4.1.
>
> **Satisfies (cross-feature):** GOAL-3.6 (L5 on-demand synthesis — produce-side) and GOAL-3.7 (L5 cost isolation) from the v03-retrieval namespace. Ownership lives here by architectural proximity — the compiler shares the write-path worker infrastructure, provider abstractions, and audit surface (`graph_pipeline_runs`). See §2 traceability table for the cross-feature GOAL rows.

### 5bis.1 Execution Model

The Knowledge Compiler is a **periodic background job** — not part of the per-write pipeline. It runs on a schedule (default: every `compiler_interval_hours`, config-defaulted to 24h; also triggerable on demand via a public `compile_knowledge()` method, §6.2) and is independent of the resolution worker pool described in §5.1. This separation is deliberate:

- Per-write latency is preserved (GUARD-11 / v0.2 `store_raw` contract).
- Compilation is expensive (embedding + LLM summarization) and must be amortizable across many memories.
- Failure of compilation must not block ingest (GUARD-1).

```
                     ┌──────────────────────────────────────┐
                     │ Knowledge Compiler (separate task)   │
                     │  scheduled | on-demand                │
                     └──────────────┬───────────────────────┘
                                    │
          ┌─────────────────────────┼──────────────────────────┐
          ▼                         ▼                          ▼
  Stage K1: select     Stage K2: cluster           Stage K3: synthesize
  candidate memories   candidate memories          & persist topics
  (since last run)     into proto-topics           (embedding, summary, FKs)
```

A compiler run writes a `graph_pipeline_runs` row with `kind = 'knowledge_compile'` (schema: v03-graph-layer §4.1), and per-decision rows into `graph_resolution_traces`. This reuses the same audit surface as resolution runs — operators see compilation in the same ledger as extraction.

### 5bis.2 Stage K1 — Candidate Selection

Compile-candidate memories are those:

- Added or modified since the last successful compiler run (`last_synthesized_at` watermark per namespace, stored in a small compiler-state row — sub-schema of `graph_pipeline_runs.output_summary` for the previous successful run).
- Reaching an activation / importance threshold (`config.compile_min_importance`, default 0.3).
- Owned by the namespace being compiled (runs are per-namespace).

Cap: `config.compile_max_candidates_per_run` (default 5000) — prevents pathological first-run costs; excess memories roll to the next run.

### 5bis.3 Stage K2 — Clustering

Clustering groups the candidate memories into proto-topics. The algorithm is pluggable behind a `Clusterer` trait:

```rust
pub trait Clusterer {
    /// Returns groups of memory_ids belonging to the same proto-topic.
    /// `cluster_weights` per group is an opaque JSON record the clusterer
    /// hands to the synthesizer — e.g. affect biases (GOAL-3.7), density
    /// metrics, or membership confidences. Persisted verbatim on each topic row.
    fn cluster(
        &self,
        memories: &[MemoryRef],
        affect_bias: Option<&AffectWeights>,
    ) -> Result<Vec<ProtoCluster>, ClusterError>;
}

pub struct ProtoCluster {
    pub memory_ids: Vec<MemoryId>,
    pub cluster_weights: serde_json::Value,
}
```

Default implementation: embedding-space clustering (HDBSCAN over memory embeddings) with optional affect-reweighting per GOAL-3.7. Affect reweighting is a **scoring bias at clustering time only** — it changes which memories end up grouped, and the resulting bias is recorded in `cluster_weights` so retrieval (v03-retrieval §4.4) can surface it in `PlanTrace.affect` without recomputing.

**Non-goal:** clustering algorithm is not part of this feature's contract. Any `Clusterer` implementation that satisfies determinism (same inputs → same groups) is acceptable. The default HDBSCAN implementation is owned by engramai's clustering module (existing), not defined here.

### 5bis.4 Stage K3 — Synthesis and Persistence

For each `ProtoCluster`:

1. **Contributing-entities aggregation.** Union `graph_memory_entity_mentions.entity_id` across `memory_ids` → `contributing_entities` (deduped).
2. **LLM summary.** Call the summarizer LLM with the memory contents + contributing-entity names → short title + multi-sentence summary. The call is retried with backoff on transient errors; persistent failure records a `graph_extraction_failures` row with `stage = 'knowledge_compile'` and the cluster's memory list (so the operator can reattempt), but does **not** block other clusters in the run.
3. **Topic embedding.** Embed the summary using the same embedder as memory embeddings (same dimensionality, same model version) so retrieval's vector search can pool topics and memories in a single index.
4. **Persist atomically.** In one `GraphStore::with_transaction`:
   - Upsert an `EntityKind::Topic` entity with the topic's UUID (v03-graph-layer §3.1).
   - Insert the `knowledge_topics` row (v03-graph-layer §4.1) sharing that UUID, with `source_memories`, `contributing_entities`, `cluster_weights`, `embedding`, `synthesis_run_id = run_id`, `synthesized_at = now()`.
   - Write per-topic `graph_resolution_traces` rows (`stage = 'persist'`, `decision = 'new'` or `'superseded'` if this compile run supersedes a prior topic).

**Supersession semantics (GUARD-3).** If an existing live topic overlaps with a new proto-cluster (`|source_memories ∩ new_source_memories| / |new_source_memories| ≥ config.topic_supersede_threshold`, default 0.5), the new topic is persisted with `superseded_by = None` and the old topic's row is updated with `superseded_by = new.topic_id`, `superseded_at = now()`. The old topic is **never deleted** — retrievable via `list_topics(include_superseded = true)` (v03-graph-layer §4.2). This preserves schema-evolution history symmetric with edge supersession.

### 5bis.5 Failure Handling

Same contract as §5.3 with two additions:

- A failed compile run **does not erase** previously-synthesized topics (GUARD-3). Readers continue to see the last good topic set.
- If a compile run times out mid-way (`config.compile_max_duration`, default 1h), completed clusters are kept (each cluster was persisted in its own inner transaction after the atomic write of stage K3.4), and the run is marked `status = 'failed'` with `error_detail = 'timeout, N clusters completed'`. The next run resumes from the high-water memory_id.

### 5bis.6 Public Surface

- **`Memory::compile_knowledge(namespace: &str) -> Result<CompileReport, EngramError>`** — on-demand trigger (§6.2). Returns aggregate stats (topics written, memories synthesized over, LLM calls). The background scheduler uses the same entry point.
- **`Memory::list_knowledge_topics(namespace, include_superseded, limit)`** — thin re-export over `GraphStore::list_topics`. Useful for operators; retrieval uses `GraphStore` directly.

### 5bis.7 Cost Isolation (GOAL-3.13)

LLM calls made by the compiler are tagged with `purpose = "knowledge_compile"` and counted separately from retrieval-time LLM calls. Metrics:

- `knowledge_compile_llm_calls_total{model=...}` — counter.
- `knowledge_compile_duration_seconds` — histogram.
- `knowledge_compile_topics_written_total` — counter per run.

v03-retrieval §6.3's `PlanTrace.l5_llm_calls` counter is therefore **always 0 on the happy path** (compilation is pre-run); it becomes non-zero only if a retrieval triggers an on-demand synthesis (rare, currently gated off by default — retrieval prefers "downgrade to Associative" on L5 miss per v03-retrieval §4.4).

## 6. Public API

### 6.1 `store_raw` — v0.2 Compat Preserved (GUARD-11, r1)

```rust
// crates/engramai/src/lib.rs — signature UNCHANGED from v0.2
impl Memory {
    pub fn store_raw(&mut self, content: &str) -> Result<MemoryId, EngramError>;
}
```

Behavioral change from v0.2: after the existing admission path completes, the resolution pipeline is enqueued. The returned `MemoryId` is valid immediately; `recall(...)` works immediately on the v0.2 layers. Graph-aware queries on the new `recall_graph(...)` API (v03-retrieval) may return sparse results until the pipeline finishes. This is a **non-breaking** change: v0.2 consumers see identical behavior.

### 6.2 New — Explicit Re-Extract Trigger

```rust
impl Memory {
    /// Enqueue a memory for re-extraction. Runs §4 preserve-plus-resynthesize.
    /// Returns the pipeline_run_id that can be polled via extraction_status.
    pub fn reextract(&mut self, memory_id: &MemoryId) -> Result<Uuid, EngramError>;

    /// Enqueue all memories in a Failed or Pending(queue_full) state.
    /// Returns the count enqueued.
    pub fn reextract_failed(&mut self) -> Result<usize, EngramError>;
}
```

Both are operator-facing (exposed on the `engramai` CLI as `engramai reextract`) and are explicitly NOT auto-triggered. `reextract` on a memory whose last run is `running` or `pending` returns `Err(AlreadyQueued)` without duplicating work (idempotence, GOAL-2.1).

**Knowledge Compiler trigger (§5bis):**

```rust
impl Memory {
    /// Run the Knowledge Compiler against `namespace`. Returns a report of
    /// how many topics were written. Runs §5bis stages K1→K3. Re-entrant per
    /// namespace: calling twice concurrently on the same namespace returns
    /// `Err(AlreadyRunning)` on the second call.
    pub fn compile_knowledge(&mut self, namespace: &str) -> Result<CompileReport, EngramError>;

    /// List currently-live L5 knowledge topics in `namespace`. Pass
    /// `include_superseded = true` to see the history (GUARD-3).
    /// Thin wrapper over `GraphStore::list_topics` (v03-graph-layer §4.2).
    pub fn list_knowledge_topics(
        &self,
        namespace: &str,
        include_superseded: bool,
        limit: usize,
    ) -> Result<Vec<KnowledgeTopic>, EngramError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct CompileReport {
    pub run_id: Uuid,
    pub candidates_considered: usize,
    pub clusters_formed: usize,
    pub topics_written: usize,
    pub topics_superseded: usize,
    pub llm_calls: usize,
    pub duration: Duration,
}
```

The background scheduler (if enabled via `ResolutionConfig::compiler_schedule`) invokes `compile_knowledge` on every active namespace at the configured interval.

### 6.3 New — Introspection

```rust
#[derive(Debug, Clone, Serialize)]
pub enum ExtractionStatus {
    NotStarted,
    Pending { since: DateTime<Utc>, queue_full: bool },
    Running { started_at: DateTime<Utc>, run_id: Uuid },
    Completed {
        completed_at: DateTime<Utc>,
        run_id: Uuid,
        trace: ResolutionTraceSummary,
        /// `false` when extraction ran successfully but produced 0 mentions / 0 edges
        /// (§3.3.2 "no semantic content"). Lets operators filter empty-done states
        /// without re-scanning the graph.
        had_semantic_content: bool,
    },
    Failed { failed_at: DateTime<Utc>, stage: PipelineStage, kind: FailureKind, message: String },
}

impl Memory {
    pub fn extraction_status(&self, memory_id: &MemoryId) -> Result<ExtractionStatus, EngramError>;

    /// Per-stage counts from the most recent completed run.
    pub fn resolution_trace(&self, memory_id: &MemoryId) -> Result<Option<ResolutionTrace>, EngramError>;
}
```

These satisfy GOAL-2.3 (structured queryable failure metadata) and GOAL-2.7 (callers can inspect, for any assignment, why the path was chosen).

### 6.4 `ResolutionStats` — Per-Call Counter Snapshot (r3 — benchmarks handoff)

**Motivation (v03-benchmarks §3.3, §12).** The cost gate asserts LLM call counts per stage without SQL introspection. The trace rows in §7.1 are written on a background flush; benchmarks need a synchronous, per-call counter snapshot returned from the write path. `ResolutionStats` is that snapshot, surfaced via `ingest_with_stats()` (test/benchmark variant of `store_raw()`).

```rust
/// Per-`store_raw()`-call counters, surfaced via `ingest_with_stats()` in
/// tests and benchmarks. Mirrors the stage counts in `ResolutionTrace`
/// (§7.1) but is returned synchronously *before* the trace row is flushed,
/// so benchmark drivers can read it without a DB round-trip.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ResolutionStats {
    pub run_id: Uuid,

    // ---- LLM call counters (GOAL-2.11, benchmarks §3.3 primary signal) ----
    /// LLM calls made in §3.2 entity extraction (0 if cached / heuristic-only).
    pub extract_llm_calls: u32,
    /// LLM calls made in §3.3 triple / predicate extraction.
    pub triple_llm_calls: u32,
    /// LLM calls made in §3.4.3 entity-decision arbitration (deferred tie-breaker).
    pub resolve_llm_calls: u32,
    /// LLM calls made in §5bis Knowledge Compiler summarization (0 unless a
    /// compile_knowledge call completed during this ingest; typically 0 on the
    /// hot write path).
    pub compile_llm_calls: u32,

    // ---- Per-stage outputs (redundant with ResolutionTrace — kept for
    //      benchmarks that want structural assertions without joining). ----
    pub entities_extracted: u32,
    pub edges_extracted: u32,
    pub entities_merged: u32,
    pub edges_invalidated: u32,
    pub stage_durations_us: StageDurations,  // per-stage wall-clock
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct StageDurations {
    pub extract_us: u64,
    pub resolve_us: u64,
    pub persist_us: u64,
}
```

**Contract with benchmarks (GOAL-2.11 handoff).**
- Every `store_raw()` / `ingest_with_stats()` call populates a fresh `ResolutionStats`; counters start at zero and are monotonically non-decreasing *within* a single call.
- Counters reset between calls (no accumulation across episodes).
- `ResolutionStats` is `Send + Sync` and cheap to clone (small POD).
- Field names in this struct are the **public contract** for the benchmark cost gate; renaming a field is a breaking change (gate threshold ≤ 3 total LLM calls stays the same, but the driver reads fields by name).

**Access path — two forms:**

```rust
impl Memory {
    /// Returns stats alongside the memory id. Preferred form for benchmarks
    /// and any caller that wants synchronous visibility.
    pub fn ingest_with_stats(
        &mut self,
        content: &str,
    ) -> Result<(MemoryId, ResolutionStats), EngramError>;
}
```

Legacy callers of `store_raw` (§6.1) continue to get just `MemoryId` — `ResolutionStats` is opt-in via the new method. `store_raw` internally computes the same stats but discards them (zero overhead for non-benchmark callers; the counters are already maintained for the trace row).

### 6.5 `resolve_for_backfill` — Migration Handoff (r3 — migration handoff)

**Motivation (v03-migration §5.2, §12).** Historic v0.2 memories need graph extraction without creating new L1 episodes (episodes anchor *new* ingestions; historic rows retain their original timestamps / NULL episode mapping per master DESIGN §8.1). Migration also needs synchronous per-record execution for clean checkpoint semantics. `resolve_for_backfill` is the pipeline entry point that differs from normal `resolve()` only in those two respects.

```rust
impl ResolutionPipeline {
    /// Backfill variant of the write path. Semantically equivalent to the
    /// normal pipeline (§3) with two differences:
    ///   1. No new L1 Episode is created. `PipelineContext.episode_id` is
    ///      populated from `memory.episode_id` if present, else left NULL.
    ///      (Historic memories may have NULL episode_id; that is expected.)
    ///   2. Execution is always synchronous (no enqueue + drain). Returns
    ///      the full `GraphDelta` (defined in v03-graph-layer §5bis) to the
    ///      caller, which is responsible for atomic persistence via
    ///      `GraphStore::apply_graph_delta` (v03-graph-layer §5bis).
    ///
    /// Idempotence: re-running on the same `MemoryRecord` must produce an
    /// equivalent `GraphDelta` (same entities / edges / mentions, modulo
    /// Uuids for newly-created entities which are keyed by canonical name).
    /// Migration uses this property for checkpoint resume (§5.2).
    pub fn resolve_for_backfill(
        &mut self,
        memory: &MemoryRecord,
    ) -> Result<GraphDelta, PipelineError>;
}
```

**Why a distinct method (vs a flag on `resolve()`).** Flags on the hot write path attract debt. The two differences (no episode, forced sync) are semantic, not configuration — a migration run should not be able to accidentally spawn episodes, and a normal ingest should not be able to accidentally skip episode creation. Two named methods, one boolean removed from the surface.

**Stats returned:** `resolve_for_backfill` does **not** return `ResolutionStats`. Migration already has its own per-record checkpoint / progress telemetry (v03-migration §7) and does not drive the cost gate (benchmarks §12 note: "resolve_for_backfill not consumed here"). If migration later wants per-record LLM cost visibility, add a `(GraphDelta, ResolutionStats)` variant; out of scope for r3.

## 7. Observability

### 7.1 `ResolutionTrace`

A trace row is written per pipeline run (`graph_resolution_traces` table, owned in v03-graph-layer). Fields:

```rust
pub struct ResolutionTrace {
    pub run_id: Uuid,
    pub memory_id: MemoryId,
    pub episode_id: Uuid,
    pub mode: PipelineMode,                  // Initial | ReExtract
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,

    // Per-stage counts (GOAL-2.11)
    pub entities_extracted: u32,
    pub edges_extracted: u32,
    pub entities_merged: u32,
    pub entities_created: u32,
    pub edges_added: u32,
    pub edges_superseded: u32,
    pub edges_preserved: u32,                // §4 preserve cases

    // LLM call budget (GOAL-2.11, GOAL-2.14, GUARD-12)
    pub llm_calls_edge_extract: u32,
    pub llm_calls_entity_tiebreak: u32,
    pub llm_calls_edge_tiebreak: u32,
    pub llm_calls_total: u32,                // sum, convenience

    // Per-entity details for GOAL-2.6 / GOAL-2.7 / GOAL-2.8
    pub entity_decisions: Vec<EntityDecisionRecord>, // signals s1–s8, winner, method
    pub edge_decisions: Vec<EdgeDecisionRecord>,     // add/update/preserve/skip + reason
    pub novel_predicates: Vec<String>,               // GOAL-2.13

    // Affect provenance (GUARD-8)
    pub affect_snapshot_source: AffectSource,        // EpisodeSnapshot | NoAffectAvailable
}
```

### 7.2 Trace Subtype Definitions

The following shapes back the trace fields above. They are minimal sketches (not complete Rust); implementers fill in derives and private fields as needed. These definitions are referenced as the public contract for trace assertions in §9.

```rust
/// Provenance of the affect snapshot consulted by fusion (GUARD-8).
pub enum AffectSource {
    /// Normal case: affect read from `Episode.affect_snapshot` for `memory_id`.
    EpisodeSnapshot { memory_id: MemoryId },
    /// No affect available — e.g., legacy v0.2 memory migrated without affect,
    /// or backfill of historic rows pre-affect-state. Reason is human-readable.
    NoAffectAvailable { reason: String },
}

pub enum SignalKind { S1Semantic, S2Name, S3GraphContext, S4Recency, S5Cooccur, S6Affective, S7Identity, S8Somatic }

pub struct SignalsSummary {
    pub present: Vec<SignalKind>,
    pub missing: Vec<SignalKind>,
}

pub enum EntityDecision { MergeInto(EntityId), CreateNew, LinkExisting(EntityId) }

pub struct EntityDecisionRecord {
    pub mention: String,
    pub decision: EntityDecision,
    pub method: ResolutionMethod,      // Automatic | LlmTieBreaker | AgentCurated | Migrated
    pub confidence: f32,
    pub candidate_count: usize,
    pub signals: SignalsSummary,
}

pub enum EdgeAction { Add, Supersede { prior_id: EdgeId }, Preserve, Skip }

pub struct EdgeDecisionRecord {
    pub subject_id: EntityId,
    pub predicate: Predicate,
    pub action: EdgeAction,
    pub reason: String,                 // e.g. "identical", "confidence_changed", "extractor_silent"
}

pub enum Stage { Classify, Extract, ResolveEntities, ResolveEdges, Persist }

pub struct StageFailure {
    pub stage: Stage,
    pub error_kind: String,
    pub retryable: bool,
}

/// Aggregate mix of decision methods over all entity decisions in this run.
/// Used by §7.2 metrics to track the automatic-vs-LLM ratio over time.
pub struct DecisionMix {
    pub automatic: u32,
    pub llm_tiebreaker: u32,
    pub agent_curated: u32,
    pub migrated: u32,
}
```

These shapes are not required to be wire-stable across minor versions (the trace is an operator artifact, not a public API), but the field names in `EntityDecisionRecord` and `EdgeDecisionRecord` are the assertion surface for §9.3 property tests.

### 7.2 Metrics

Exposed via the existing `write_stats.rs` counter interface:

| metric                                 | type          | labels                  | notes                            |
| -------------------------------------- | ------------- | ----------------------- | -------------------------------- |
| `resolution_entities_extracted`        | histogram     | source_kind             | per memory                       |
| `resolution_edges_extracted`           | histogram     | source_kind             | per memory                       |
| `resolution_entities_merged_total`     | counter       | method                  | Automatic / LlmTieBreaker / ... |
| `resolution_edges_superseded_total`    | counter       | predicate_kind          | canonical vs proposed            |
| `resolution_latency_ms`                | histogram     | stage                   | one observation per stage        |
| `resolution_failures_total`            | counter       | stage, kind             | structured error classes         |
| `resolution_llm_calls_per_episode`     | histogram     | stage                   | GOAL-2.11                        |
| `resolution_llm_calls_rolling_avg`     | gauge         | window=N                | GOAL-2.14; warn when > 4         |
| `resolution_novel_predicates_total`    | counter       | label                   | GOAL-2.13                        |
| `resolution_preserved_curated_total`   | counter       | —                       | §4.2 monotonic-curation check    |
| `pipeline_queue_depth`                 | gauge         | —                       | §5.2                             |
| `pipeline_queue_full_total`            | counter       | —                       | §5.2                             |
| `resolution_pending_memories_total`    | gauge         | reason                  | backlog visibility (§5.2.1); reason ∈ {`queue_full`, `awaiting_worker`} |
| `resolution_pending_memory_oldest_age_seconds` | gauge | —                       | age of oldest Pending memory — alert threshold per §5.2.1 |
| `resolution_llm_calls_over_budget_total` | counter     | —                       | incremented when rolling avg of LLM calls per episode exceeds the GOAL-2.14 warn threshold (> 4) |

### 7.3 Traces

One `tracing::info_span!` per stage: `resolution.stage.{ingest,entity_extract,edge_extract,resolve,persist}`. Spans are nested under a parent `resolution.run` span carrying `run_id`, `memory_id`, `episode_id`, `mode`. Each stage span records its elapsed time and any stage failure.

## 8. Error Handling

### 8.1 Partial Failure Semantics

The pipeline is **all-or-nothing per memory on the graph side** (GOAL-2.2). Within a single run:

- Entity extraction (§3.2) is deterministic and only fails on internal bugs. If it panics or errors, the run aborts before any LLM call; cost is zero.
- Edge extraction (§3.3) fails on LLM error. The run aborts. No entity or edge rows are written. A `StageFailure { stage: EdgeExtract, kind: LlmTimeout|RateLimit|Other }` is recorded.
- Resolution (§3.4) may make LLM tie-breaker calls. `config.on_tiebreak_failure` controls behavior when the tiebreaker fails (timeout, error):
  - **`Conservative` (default)** — defaults the decision to `CreateNew` for entities and `Add` for edges. Trace entry flags `tiebreak_failed = true` with `method = Automatic`, `confidence = low`. Preserves the successful classifier + extractor work; accepts the risk of a duplicate entity that the agent can merge later via `agent_curate_entity` (§6.3). Satisfies GUARD-2 via the visible trace entry — this is not silent degradation.
  - **`Abort`** — the run aborts, the memory enters `Failed(tiebreak_unavailable)` state. Loses the extractor work but prevents any possibility of a duplicate entity. Operator replays via `reextract --failed`. Choose this mode when duplicate-free invariants matter more than throughput.

  Default is `Conservative` because it preserves extraction work (the usual production cost concern); operators who need strict duplicate-free invariants opt into `Abort`.
- Persist (§3.5) only fails on storage error (disk full, SQLite lock, FK violation). Transaction rollback. No partial graph state.

### 8.2 Idempotence (GOAL-2.1)

Pipeline re-runs are safe because:

- `graph_pipeline_runs` tracks `(memory_id, run_id)`. A duplicate enqueue for a completed memory is dropped at dequeue.
- Re-extract is explicitly a new `run_id` and follows §4 additive-merge rules.
- `insert_edge` relies on `Edge.id = Uuid::new_v4()` so two runs that produce the "same" edge produce distinct rows (both valid provenance anchors), but §4.2 "present/identical" skip rule prevents that at the pipeline level for the re-extract case.
- `upsert_entity` is keyed on canonical id, so repeated upserts converge.

### 8.3 Concurrency

Within the single-process writer model (GOAL-2.4 scope), concurrency correctness is provided by:

- Session-affinity dispatch so overlapping candidate sets are serialized (§5.1).
- SQLite WAL + single-writer transaction discipline for the persist stage.
- `merge_entities` on the graph store takes an exclusive transaction and is safe to call concurrently; re-pointing aliases is atomic within it.

Cross-process concurrency is out of scope; external callers must serialize access (per master §1/NG1).

## 9. Testing Strategy

### 9.1 Unit — Per Stage

- `stage_extract.rs`: given fixtures of `MemoryRecord { content, .. }` + a seeded `EntityExtractor`, assert `ctx.extracted_entities` has expected names + types. No LLM calls; reuses v0.2 extractor tests.
- Predicate normalization (§3.3.1): property test that every v0.2 `Predicate` round-trips through the adapter to a canonical v0.3 `Predicate::Canonical(...)`.
- Fusion (§3.4.2): table-driven tests for each of s1–s8 in isolation (mock candidate, known mention), then weighted-sum combinations. Missing-signal redistribution is its own test.
- Decision logic (§3.4.3 / §3.4.4): exhaustive tests of the decision matrix and of the §4.2 preserve-plus-resynthesize table. Every row of the §4.2 table is a named test case.
- Persist (§3.5): transaction rollback on injected SQLite error leaves zero graph rows.

### 9.2 Integration

- End-to-end `store_raw` → poll `extraction_status` to `Completed` → assert `graph_entities` and `graph_edges` contain expected rows. Runs against a fixed seed and mocked `TripleExtractor` returning scripted output.
- Latency budget: 95th-percentile store_raw synchronous path < 50ms on a warm DB (the async enqueue is the only added work); pipeline background latency < 1s per memory under mocked LLM.
- Failure surfacing: inject LLM timeout → `extraction_status` returns `Failed { stage: EdgeExtract, kind: LlmTimeout }`; memory still queryable via v0.2 recall.

### 9.3 Property

- **Re-extraction idempotence.** `for any memory m: run(m); snapshot_a = graph_state(m); run_reextract(m); snapshot_b = graph_state(m); assert snapshot_a.semantic_eq(snapshot_b)` where `semantic_eq` ignores `updated_at`. Run with shrinking on content strings.
- **Monotonic curation.** Hand-curate an edge on memory m, then run_reextract(m) arbitrarily many times, assert the curated edge's `id`, `confidence`, `method = AgentCurated`, and `invalidated_at = None` are unchanged. §4.4 invariant 2.
- **No-invalidation-on-silence.** Run(m) producing edges E; then re-run with a scripted extractor that returns zero triples; assert every edge in E still has `invalidated_at = None`. §4.4 invariant 3.
- **Atomicity.** Inject a panic during persist (§3.5) step 3; assert graph state is identical to pre-run (no orphan entities, no orphan edges).

### 9.4 Regression

- The existing v0.2 test corpus (`crates/engramai/tests/`) MUST continue to pass unmodified. `store_raw` behavior surface is unchanged (GUARD-11).

## 10. Cross-Feature References

This design depends on and coordinates with:

- **v03-graph-layer/design.md**
  - §3.1 `Entity` struct and `EntityKind` enum — consumed by §3.2 adapter and §3.4.3 decision.
  - §3.2 `Edge`, `EdgeEnd`, `ResolutionMethod` — consumed by §3.4.4 and §3.5.
  - §3.3 `Predicate`, `CanonicalPredicate` — consumed by §3.3.1 adapter and §3.3.2 proposed-predicate path.
  - §3.4 `EntityAlias` and merge semantics — consumed by §3.4.3 when a deferred tie-breaker concludes MERGE.
  - §4.1 SQLite tables (`graph_entities`, `graph_edges`, `graph_entity_aliases`, `graph_predicates`, `graph_extraction_failures`, plus the pipeline-owned `graph_pipeline_runs`, `graph_resolution_traces`, `graph_memory_entity_mentions` tables defined there) — written by §3.5 via `GraphStore`.
  - §4.2 `GraphStore` trait (`search_candidates`, `upsert_entity`, `insert_edge`, `invalidate_edge`, `merge_entities`, `link_memory_to_entities`, `record_extraction_failure`) — the sole write interface for §3.5.
  - §4.3 transaction boundaries — §3.5 uses `Storage::with_graph_tx` directly.
  - §6 telemetry body-signal bus — §7 metrics emit into this bus; the boundary is one-way.
  - §7 error model (`GraphError`) — mapped into `StageFailure.kind` in §8.

- **v03-retrieval/design.md** (read-only consumer of graph state produced here)
  - Reads the graph state this pipeline produces. No shared code on the write path. Candidate-retrieval in §3.4.1 uses a different `search_candidates` call than query-time recall (different ranking profile), but both sit on top of the same embedding / alias indices.

- **v03-migration/design.md** (✅ r3 handoff acknowledged)
  - Reuses §3 as a backfill engine: migration walks v0.2 `memories` rows and enqueues `PipelineJob { mode: Backfill }` for each. `Backfill` mode behaves like `ReExtract` (§4) with an additional rule: `Entity.method = Migrated` for newly created entities so they are distinguishable in audit.
  - **Handoff type:** `resolve_for_backfill(memory: &MemoryRecord) -> Result<GraphDelta, PipelineError>` defined in §6.5 above. Differs from normal `resolve()` in two ways: (a) no L1 episode creation, (b) forced synchronous execution. Returns `GraphDelta` (owned by v03-graph-layer §5bis) which migration persists via `GraphStore::apply_graph_delta`. Idempotent re-run on same input produces equivalent delta — basis for migration's checkpoint-resume.

- **v03-benchmarks/design.md** (✅ r3 handoff acknowledged)
  - Measures `resolution_llm_calls_rolling_avg` (§7) and `resolution_latency_ms` against the LOCOMO corpus. The ship-gate target (avg ≤ 3 over the fixed benchmark suite) lives there; the runtime warn threshold (avg ≤ 4 over rolling window N≥100) lives here (GOAL-2.14).
  - **Handoff type:** `ResolutionStats` defined in §6.4 above is the public per-call counter surface benchmarks read. Accessed via `Memory::ingest_with_stats(content) -> Result<(MemoryId, ResolutionStats), _>`. Field names in `ResolutionStats` are the public contract for benchmarks §3.3 cost gate — renames require coordinated change.

- **Existing v0.2 modules** (unchanged; referenced, not modified)
  - `crates/engramai/src/entities.rs` — `EntityExtractor`, `ExtractedEntity`, `EntityType`. Source of truth for Stage 3 entity extraction.
  - `crates/engramai/src/triple_extractor.rs` — `TripleExtractor` trait, `AnthropicTripleExtractor`, `OllamaTripleExtractor`. Source of truth for Stage 3 edge extraction.
  - `crates/engramai/src/triple.rs` — `Triple`, `Predicate` (v0.2), `TripleSource`. Adapter input for §3.3.1.
  - `crates/engramai/src/extractor.rs` — `ExtractedFact`, `AnthropicExtractor`. Not on the graph extraction path but shares the `TokenProvider` abstraction for LLM auth.
  - `crates/engramai/src/storage.rs` — extended (not replaced) with graph tables per v03-graph-layer §4.
  - `write_stats.rs` — existing counter sink; §7 metrics flow through it.

- **Master DESIGN-v0.3.md**
  - §4.1 — pipeline overview; §3 is the direct realization.
  - §4.2 — Stage 3 extraction with graph context; §3.2 extension hook.
  - §4.3 — Stage 4 multi-signal fusion; §3.4.2 lifts this verbatim.
  - §4.4 — Stage 5 mem0-style edge resolution; §3.4.4 lifts this.
  - §4.5 — extraction failure handling; §5.3 + §8 implement.
  - §6 — consolidation (retro-evolution specifically); §3.4.4 "retro-evolution" rule references this.
  - §3.7 — cognitive-state boundary rules; §3, §7 respect these (read-only from Affect, emit to Telemetry).

- **Master requirements.md GUARDs**
  - GUARD-1 (episodic completeness) → §5.1, §5.3, §8.1.
  - GUARD-2 (never silent degrade) → §5.2, §5.3, §6.3, §8.1.
  - GUARD-3 (no erasure on invalidation) → §3.4.4, §3.5, §4.
  - GUARD-6 (cognition never gates writes) → §3 overall.
  - GUARD-8 (affect snapshot immutability) → §3.1, §3.4.2.
  - GUARD-11 (v0.2 API compat) → §6.1.
  - GUARD-12 (LLM cost telemetry) → §7.

---

*End of design.*
