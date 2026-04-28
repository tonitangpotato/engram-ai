# ISS-049: Retrieval orchestrator wires `Null*` stubs for all five plan executors — every LoCoMo QA returns 0/25 because no plan has a real data-source adapter

- **Status**: in_progress
- **Severity**: blocker (graph_query is the only public retrieval API in v0.3; with all five plans hard-wired to `Null*` collaborators it returns empty results for every non-Hybrid query, and Hybrid's `Null*` sub-plans return empty vecs from RRF — the entire retrieval surface is a no-op)
- **Filed**: 2026-04-28
- **Discovered during**: LoCoMo conv-26 QA evaluation against the post-ISS-048 binary — graph contained 136 entities + 101 edges (ISS-048 ingest acceptance run), but `graph_query` returned `results = []` for all 25 questions
- **Related**: ISS-048 (ingest pipeline now produces real graph data; this issue is why retrieval can't see it), `task:retr-impl-orchestrator-plan-execution` (the task that *should* have wired real adapters but instead wired `Null*` stubs and was marked `done`), v0.3 design §4.1–§4.5, GUARD-2

---

## Summary

The v0.3 retrieval orchestrator (`crates/engramai/src/retrieval/orchestrator.rs`)
is the single dispatch point that turns a `DispatchedQuery` into a
`Vec<ScoredResult> + RetrievalOutcome`. It contains a `match plan_kind`
with five arms — Factual, Episodic, Associative, Abstract, Affective —
plus a Hybrid arm that fans out into the same five via
`HybridDispatchExecutor`.

**Every one of those six call sites instantiates the plan with the
`Null*` collaborator** documented in each plan module as "inert default,
useful for unit tests / when the backend is absent":

| Plan | Trait | Null impl wired in `execute_plan` | Real adapter |
|---|---|---|---|
| Factual | `EntityResolver` | `NullEntityResolver` (`factual.rs:116`, `orchestrator.rs:502`, `orchestrator.rs:658`) | **does not exist** |
| Episodic | `EpisodicMemoryStore` | `NullEpisodicStore` via `EpisodicPlan::default()` (`episodic.rs:296`) | **does not exist** |
| Associative | `SeedRecaller` | `NullSeedRecaller` via `AssociativePlan::default()` (`associative.rs:223`) | **does not exist** |
| Abstract | `TopicSearcher` | `NullTopicSearcher` via `AbstractPlan::default()` (`abstract_l5.rs:282`) | **does not exist** |
| Affective | `AffectiveSeedRecaller` | `NullAffectiveSeedRecaller` via `AffectivePlan::default()` (`affective.rs:351`) | **does not exist** |

By contract every `Null*` impl returns an empty vec / "not found". So:

- Factual: `resolve()` returns `[]` → plan downgrades to
  `EntityFoundNoEdges { entities: [] }` → 0 results.
- Episodic: `memories_in_window()` returns `[]` → plan returns
  `EpisodicOutcome::Empty` → 0 results.
- Associative: `recall()` returns `[]` → plan returns
  `DowngradedNoSeeds` → 0 results.
- Abstract: `search()` returns `[]` → plan returns
  `DowngradedL5Unavailable` → 0 results.
- Affective: `recall()` returns `[]` → plan returns 0 candidates.
- Hybrid: each `SubPlanKind` arm runs the same Null-backed plan → RRF
  fuses six empty lists → `items = []`.

This is **not** a wiring bug between modules. The plans themselves work
correctly when handed a real collaborator (their unit tests confirm this
on hand-rolled stubs). The bug is that `execute_plan` and
`HybridDispatchExecutor::run` were committed with *test-only* defaults
on the production hot path.

LoCoMo conv-26 QA on commit ≥ ISS-048 binary:

```
graph: 136 entities, 101 edges, 0 unresolved-subject failures (ISS-048)
retrieval: 25 questions × graph_query → 0/25 hits (this issue)
```

This is the **third layer** of the LoCoMo retrieval iceberg: ISS-047
fixed failure-label propagation, ISS-048 fixed entity recall on
free-form ingest, ISS-049 fixes retrieval reading what's now correctly
indexed.

---

## Reproduction

After running ISS-048 fresh-ingest of LoCoMo conv-26 sessions 1-3:

```rust
let response = memory.graph_query(GraphQuery::new("Who is Caroline?")).await?;
assert!(response.results.is_empty()); // every question
```

`response.outcome` is one of:

- `EntityFoundNoEdges { entities: [] }` for entity-shaped queries (Factual)
- `Ok` with empty `results` for everything else
- `NoCognitiveState` for affect-tagged queries (since `self_state` is also
  unwired upstream — but even with self_state, the affective recaller is
  Null so it would still be empty)

There is no error. The orchestrator silently returns the
behaviorally-correct outcome for "no recall backend installed" — which
was the deferred state at the time `task:retr-impl-orchestrator-plan-execution`
was committed. The bug is that the *task was marked done* before the
deferral was lifted.

---

## Root cause: the "wired but stubbed" antipattern

`task:retr-impl-orchestrator-plan-execution` was scoped to
"Wire each Plan variant to its retrieval implementation: factual →
FactualBitemporalPlan, episodic → EpisodicRecallPlan, …". The
implementer interpreted "wire" as "instantiate the plan struct and call
`execute()`" — which is technically correct for the dispatch layer.
But the plan's *collaborator* (resolver / store / recaller / searcher)
is *also* a runtime dependency that must be plumbed from
`Memory::graph_query` → `execute_plan`. That second plumbing step was
left as `Null*` "for now" with a comment:

```rust
// orchestrator.rs:432
// **v0.3 collaborator slots are deferred.** The executor wires `Null*`
// implementations of every plan's recaller / resolver / store. That
// mirrors what `execute_plan` does for direct (non-Hybrid) plan dispatch
// — when a real recaller arrives in a later task, both call sites get
// upgraded together. Until then, Hybrid's `Null*` sub-plans return
// empty lists, and the RRF fusion step produces an empty `items` vec
// — which is the behaviorally-correct outcome for "no recall backend
// installed yet".
```

No follow-up task was filed for "the real recaller arrives in a later
task". The graph shows `task:retr-impl-orchestrator-plan-execution` as
`done`, the chain `retr-test-orchestrator-e2e` is `done`, and yet no
real adapter exists for any plan. This is a classic **state-deception
bug** (the same family as the one called out on
`task:retr-impl-graph-query-api` ⚠️ correction: "API surface complete,
but bodies are STUBS"). The fix below files the missing follow-up tasks
*and* the plumbing change to thread real collaborators in.

---

## Plan (root fix, no patches)

Four phases, total ~6–7h of focused work. Phase order is fixed —
phase 2 (signature change) must land before phase 3 (adapter
implementations) because the adapter types need a slot to be passed
through.

### Phase 1 — graph hygiene (~30 min)

1. File this issue (this file).
2. Reopen `task:retr-impl-orchestrator-plan-execution` from `done` →
   `in_progress` with a status correction note documenting the false-done
   (mirroring the existing pattern on `task:retr-impl-graph-query-api`).
   Reopen `task:retr-test-orchestrator-e2e` similarly — it cannot have
   passed acceptance with all `Null*` stubs.
3. Add 7 new task nodes (5 adapters + 1 plumbing + 1 acceptance):
   - `task:retr-impl-adapter-graph-entity-resolver` (Factual)
   - `task:retr-impl-adapter-storage-episodic-store` (Episodic)
   - `task:retr-impl-adapter-hybrid-seed-recaller` (Associative)
   - `task:retr-impl-adapter-graph-topic-searcher` (Abstract)
   - `task:retr-impl-adapter-hybrid-affective-seed-recaller` (Affective)
   - `task:retr-impl-orchestrator-collaborators` (signature change)
   - `task:retr-test-locomo-conv26-qa-acceptance` (end-to-end gate)
4. Wire dependency edges: adapters depend on `-collaborators` plumbing;
   acceptance depends on all adapters; reopened
   `-orchestrator-plan-execution` depends on all adapters; reopened
   `-orchestrator-e2e` depends on `-locomo-conv26-qa-acceptance`.

### Phase 2 — orchestrator signature change (~1.5–2h)

**Single breaking change**: introduce a `PlanCollaborators` struct that
bundles the five adapter trait objects, and thread it through
`execute_plan` + `HybridDispatchExecutor`.

```rust
// crates/engramai/src/retrieval/orchestrator.rs
pub(crate) struct PlanCollaborators<'a> {
    pub entity_resolver:    &'a dyn EntityResolver,
    pub episodic_store:     &'a dyn EpisodicMemoryStore,
    pub seed_recaller:      &'a dyn SeedRecaller,
    pub topic_searcher:     &'a dyn TopicSearcher,
    pub affective_recaller: &'a dyn AffectiveSeedRecaller,
}

pub(crate) fn execute_plan(
    dispatched: DispatchedQuery,
    graph: &dyn GraphRead,
    loader: &dyn RecordLoader,
    collaborators: &PlanCollaborators<'_>,   // NEW
    self_state: Option<SomaticFingerprint>,
) -> (Vec<ScoredResult>, RetrievalOutcome);
```

- All six call sites (5 direct + 1 in `HybridDispatchExecutor::run`)
  switch from `NullX` → `collaborators.x`.
- `Memory::graph_query` constructs a `PlanCollaborators` from a new
  `Memory::collaborators_for_query()` helper that builds the five real
  adapters (defined in phase 3).
- Tests in `orchestrator.rs` / `api.rs` that exercise the Null path
  switch to constructing an explicit `PlanCollaborators` of `Null*`
  values — no behavior change for them.
- `HybridDispatchExecutor` gains a `collaborators: &'a PlanCollaborators<'a>`
  field; the existing `_factual_budget` / `topics_by_uuid` / `self_state`
  fields stay.

This is the only signature change in the plan. No public API breaks
(`execute_plan` is `pub(crate)`).

### Phase 3 — adapter implementations (~3–4h)

All five adapters live in a new module
`crates/engramai/src/retrieval/adapters/` (one file per adapter +
`mod.rs`). Each adapter is a small struct that holds borrowed handles
to the collaborator's data sources and implements the corresponding
plan trait. **None of them mutate `Memory`** — see "Mutability
constraint" risk note below for why this works.

Implementation order follows dependency depth:

1. **`GraphEntityResolver`** (Factual) — easiest, pure read.
   ```rust
   pub struct GraphEntityResolver<'a> {
       pub graph: &'a dyn GraphRead,
   }
   impl EntityResolver for GraphEntityResolver<'_> {
       fn resolve(&self, query: &str) -> Vec<ResolvedAnchor> {
           // 1. Tokenize query (reuse stage_extract normalizer).
           // 2. For each candidate token: graph.find_entities_by_name(token, limit=5).
           // 3. Convert (entity_id, canonical_name, match_strength) → ResolvedAnchor.
           // 4. Sort by match_strength desc, dedupe by entity_id.
       }
   }
   ```
   Risk: `GraphRead` may not have `find_entities_by_name` yet — verify
   in `crates/engramai/src/graph/store.rs`. If absent, add it to the
   trait + sqlite impl as part of this adapter task.

2. **`StorageEpisodicStore`** (Episodic) — pure storage read.
   ```rust
   pub struct StorageEpisodicStore<'a> {
       pub storage: &'a Storage,
   }
   impl EpisodicMemoryStore for StorageEpisodicStore<'_> {
       fn memories_in_window(&self, window: &ResolvedWindow, limit: usize)
           -> Vec<MemoryId>
       { /* SELECT id FROM memories WHERE valid_from < window.end AND
            valid_to > window.start ORDER BY valid_from DESC LIMIT ? */ }

       fn memories_mentioning_entities(&self, entities: &[EntityId], limit: usize)
           -> Option<Vec<MemoryId>>
       { /* JOIN mentions ON entity_id IN (?) — return Some */ }
   }
   ```
   Risk: storage may not expose a "memories in time window" query yet.
   If absent, add it to `Storage` (one method, ~15 LOC).

3. **`HybridSeedRecaller`** (Associative) — wraps `hybrid_search`.
   ```rust
   pub struct HybridSeedRecaller<'a> {
       pub storage:    &'a Storage,
       pub embedding:  Option<&'a EmbeddingProvider>,
       pub model_id:   String,
   }
   impl SeedRecaller for HybridSeedRecaller<'_> {
       fn recall(&self, query: &GraphQuery, k_seed: usize)
           -> (Vec<SeedHit>, SeedRecallStatus)
       {
           let qvec = self.embedding
               .and_then(|e| e.embed(&query.text).ok());
           let opts = HybridSearchOpts { limit: k_seed, namespace: ... };
           let hits = crate::hybrid_search::hybrid_search(
               self.storage, qvec.as_deref(), &query.text, opts, &self.model_id,
           ).unwrap_or_default();
           // Convert HybridSearchResult → SeedHit
       }
   }
   ```
   This is the **mutability win** — `hybrid_search` is a free function
   over `&Storage`, and `EmbeddingProvider::embed` is `&self`. So the
   adapter can be constructed from `&Memory` without violating the
   `&self` constraint on `Memory::graph_query`.

4. **`GraphTopicSearcher`** (Abstract) — graph topic store read +
   vector + BM25 over `knowledge_topics` table.
   ```rust
   pub struct GraphTopicSearcher<'a> {
       pub graph:     &'a dyn GraphRead,
       pub embedding: Option<&'a EmbeddingProvider>,
   }
   ```
   Risk: `GraphRead` likely needs a `topic_vector_search(&self, qvec, ns,
   k)` and `topic_text_search(&self, q, ns, k)` method. Verify against
   schema; add if missing.

5. **`HybridAffectiveSeedRecaller`** (Affective) — same as Associative
   but reads `affect_snapshot` for each hit.
   ```rust
   pub struct HybridAffectiveSeedRecaller<'a> {
       pub storage:    &'a Storage,
       pub embedding:  Option<&'a EmbeddingProvider>,
       pub model_id:   String,
   }
   impl AffectiveSeedRecaller for HybridAffectiveSeedRecaller<'_> {
       fn recall(&self, query: &GraphQuery, top_k: usize)
           -> (Vec<AffectiveSeedHit>, AffectiveSeedStatus)
       {
           // Same hybrid_search as #3; for each hit do one
           // SELECT affect_snapshot FROM memories WHERE id = ?
           // (or batched IN (?)). Per design GUARD-6: rows without
           // affect_snapshot return Some(hit) with affect_snapshot: None
           // — MUST NOT be dropped.
       }
   }
   ```

Each adapter is one task. Each task: write the adapter, add unit tests
(deterministic against an in-memory `SqliteGraphStore` / `Storage`),
verify the corresponding plan's existing unit tests still pass with
the real adapter substituted via the `_collaborators` test helper.

### Phase 4 — end-to-end verification (~45 min)

1. Update `Memory::graph_query` to construct a `PlanCollaborators` from
   the five new adapters. Adapters borrow `&self.storage()`,
   `&self.embedding`, and the `&dyn GraphRead` already available inside
   `with_graph_read`. Lifetimes are scoped to the closure.
2. Run LoCoMo conv-26 QA against the rebuilt binary. Acceptance:
   - At least 18/25 questions return non-empty `results` (target: 22/25,
     stretch: ≥23/25 to match the team's baseline goal).
   - No question returns `RetrievalError`.
   - Plan distribution across the 25 queries is varied (not all
     downgrading to one plan), confirming the dispatcher is reaching
     each adapter under realistic loads.
3. Update v0.3 design §4 if any trait method shape changed during
   adapter implementation (low likelihood — traits are deliberately
   minimal).
4. Mark all 7 new tasks + 2 reopened tasks `done`. Close ISS-049.

---

## Risks & mitigations

| # | Risk | Mitigation |
|---|---|---|
| 1 | `GraphRead` trait missing methods (`find_entities_by_name`, `topic_vector_search`, …) | Verify upfront during phase-3 task kickoff; if missing, scope additions inside the adapter task, not as a separate task. Trait extension is additive (no breaking change). |
| 2 | `Storage::memories_in_window` missing | Same as #1; adapter task absorbs the storage method addition. |
| 3 | `Memory::hybrid_recall` mutability — does `&mut self` hide a real invariant we're now circumventing? | Audit shows `&mut` is for embedder cache mutation only. `EmbeddingProvider::embed(&self)` and `hybrid_search(&Storage, ...)` are both `&`-receivers. Adapters are safe. **Add a regression test** that runs two sequential `graph_query` calls and confirms the second sees the same graph state — proves no hidden mutation was needed. |
| 4 | Phase-2 signature change breaks `execute_plan` callers | `execute_plan` is `pub(crate)`. Only one caller exists (`Memory::graph_query`). Mechanical refactor. |
| 5 | Adapter unit tests need fixtures that mirror real ingest output | Reuse the `crates/engramai/src/graph/test_helpers::fresh_conn()` pattern that the existing orchestrator tests already use. Seed with 2-3 hand-built entities/edges/memories per test. |
| 6 | LoCoMo acceptance falls short of 18/25 even with real adapters | Indicates a deeper retrieval-quality issue (signal weights, fusion config, namespace mismatch) — file follow-up issue, **do not** add fixes to ISS-049 scope. ISS-049 is "real adapters connected"; quality tuning is a separate problem. |
| 7 | Hybrid sub-plan budget accounting (`_factual_budget`) currently TODO — does threading real collaborators expose this? | No. The budget split is orthogonal to whether the sub-plan reads real or null data. Stays deferred to `task:retr-impl-hybrid-budget` (not part of ISS-049). |
| 8 | `EntityKind::Other("unknown")` drafts from ISS-048 may not be discoverable by `GraphEntityResolver::resolve` if it filters by typed kinds | The resolver MUST match by canonical_name (and aliases) regardless of `EntityKind`. Add a unit test pinning this: ingest one `Other("unknown")` entity, confirm `resolve("unknown_name")` returns it. |

---

## Acceptance criteria

- All 5 plan executors in `execute_plan` instantiate with real adapter
  trait objects, not `Null*` stubs (verified by grep:
  `rg 'NullEntityResolver|NullEpisodicStore|NullSeedRecaller|NullTopicSearcher|NullAffectiveSeedRecaller'
   crates/engramai/src/retrieval/orchestrator.rs` returns 0 matches).
- Same for the 5 sub-plan arms in `HybridDispatchExecutor::run`.
- `cargo test -p engramai retrieval::` → all green; new adapter unit
  tests cover each plan's happy path against real seed data.
- LoCoMo conv-26 QA on the rebuilt binary against the ISS-048
  fresh-ingest DB returns ≥18/25 non-empty result sets (vs. 0/25
  pre-fix).
- The two reopened tasks (`-orchestrator-plan-execution`,
  `-orchestrator-e2e`) carry status-correction notes documenting the
  false-done; the new tasks are marked `done` only after the LoCoMo
  acceptance metric is logged in the issue.
- No `Null*` types deleted — they remain as legitimate test scaffolding
  per their own doc comments.
