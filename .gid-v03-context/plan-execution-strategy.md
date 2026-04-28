# Plan Execution Strategy — `task:retr-impl-orchestrator-plan-execution`

> **Created:** 2026-04-27
> **Mandate from potato:** "不要任何 patch 任何简单化直接好好规划一步到位"
> **Status:** Planning (no code yet)

## 0. Task Scope (from graph)

Wire each `PlanKind` variant produced by `dispatch()` to its concrete plan implementation in `Memory::graph_query`. Each plan produces typed candidates → adapter converts to `Vec<ScoredResult>` with populated `SubScores` → fusion ranks → top-K → `GraphQueryResponse`.

**Note**: task description in graph says "5 plan variants" but the implementation has **6 PlanKind leaves** (Factual, Episodic, Associative, Abstract, Affective, Hybrid). The dispatcher already handles all 6. Plan execution must therefore wire all 6.

## 1. Architectural Reality Check

### What's already done (verified)
- ✅ `dispatch()` produces `DispatchedQuery { intent, plan_kind, classifier_method, signal_scores, context, query }`
- ✅ All 6 plans exist with `execute()` methods + `Null*` collaborator stubs
- ✅ Fusion: `fuse_and_rank(intent, cfg, Vec<ScoredResult>) → Vec<ScoredResult>` is wired
- ✅ `BudgetController` is thread-safely shared via `Arc<Mutex<_>>` in `PlanContext`
- ✅ Hybrid plan has `HybridSubPlanExecutor` trait + `StubExecutor` for testing fan-out

### What's missing (this task must produce)
- ❌ Per-plan **adapter functions**: `Plan*Result → Vec<ScoredResult>` (6 adapters)
- ❌ Per-plan **collaborator threading**: where does each plan get its `EntityResolver`/`SeedRecaller`/`TopicSearcher`/`AffectiveSeedRecaller`/`EpisodicMemoryStore`/`HybridSubPlanExecutor` from?
- ❌ **GraphRead access**: `Memory` doesn't currently expose its `SqliteGraphStore` for retrieval. The plan executor needs `&dyn GraphRead`.
- ❌ **Hybrid sub-plan executor**: real impl that recursively calls 2 of the other plans (Hybrid is the only fan-out plan).
- ❌ **Result conversion**: plans produce `MemoryId`/`KnowledgeTopic`, but `ScoredResult::Memory` carries `MemoryRecord`. Need a memory-id → record loader.
- ❌ **`Memory::graph_query` body**: the dispatch + execute + fuse + assemble pipeline.

### Architectural tension to resolve cleanly (NOT patch around)

**Memory doesn't own retrieval collaborators.** Currently `Memory` has fields for storage, embedding, extractor, etc. — but no `entity_resolver: Arc<dyn EntityResolver>`, no `topic_searcher`, etc. The plans are written against traits with `Null*` defaults.

**Two paths:**

**(A) Add collaborator slots to `Memory`** with `Null*` defaults — concrete impls land in later tasks (graph-layer / resolution).
- Pros: clean DI, each later task adds a real impl by setting the slot.
- Cons: 6 new `Memory` fields just for retrieval; widens public surface.

**(B) Construct collaborators per-call from `Memory` internals** (lookup-style, `Memory::entity_resolver()` returns Arc).
- Pros: no new struct fields; collaborators stay inside retrieval.
- Cons: hides the wire-up; harder to swap for tests.

**Decision: (A) — collaborator slots on `Memory`.** Reasons:
1. Test ergonomics: tests can override one collaborator without subclassing `Memory`.
2. Matches the `extractor`/`embedding`/`triple_extractor` pattern already used for v0.2 collaborators.
3. The slots are `Option<Arc<dyn Trait>>` with `Null*` fallback at use-site — zero new mandatory dependencies.

**Memory new fields** (all `Option<Arc<dyn Trait>>`, default `None`):
- `entity_resolver: Option<Arc<dyn EntityResolver>>`
- `seed_recaller: Option<Arc<dyn SeedRecaller>>`
- `topic_searcher: Option<Arc<dyn TopicSearcher>>`
- `affective_seed_recaller: Option<Arc<dyn AffectiveSeedRecaller>>`
- `episodic_store: Option<Arc<dyn EpisodicMemoryStore>>`
- `hybrid_executor: Option<Arc<dyn HybridSubPlanExecutor>>` — see §5

Plus a `GraphRead` accessor (the `SqliteGraphStore` already lives in resolution wiring; `Memory::graph_store_arc()` exposes it on demand). If no `job_queue` is set (v0.2-compat mode), retrieval can construct an ephemeral `SqliteGraphStore` from `Memory::storage`'s connection — but that's a **separate concern**: §6.

## 2. Execution Order (Plan)

```
Step 1: GraphRead access on Memory          (foundation — every plan needs it)
Step 2: Per-plan adapters (6 functions)     (typed candidates → ScoredResult)
Step 3: Memory collaborator slots + setters (DI surface)
Step 4: HybridSubPlanExecutor real impl     (recursive: calls 2 sibling plans)
Step 5: Memory::graph_query body            (the orchestrator)
Step 6: Memory::graph_query_locked body     (just calls graph_query with locked fusion config)
Step 7: Tests                               (per-plan smoke + outcome variants)
```

**Each step is committable independently** — but written in this order so each has its dependencies in place. No "TODO: fix later" markers.

## 3. Step Details

### Step 1 — GraphRead access on Memory

**Decision (revised 2026-04-27):** The strategy doc originally proposed a v0.2-compat fallback that constructs a fresh `SqliteGraphStore` from `Memory::storage` on the fly. That fallback **cannot work from `&self`**: `SqliteGraphStore::new` takes `&mut Connection`, but `with_graph_read` is called from `&self` (graph_query is an `&self` method by design — concurrent reads must not require a `&mut Memory`). The `&mut self` graph helpers (`graph_mut`, `extraction_status`) are unavailable here. Building the v0.2-compat fallback would require a parallel connection or refactoring `SqliteGraphStore` to accept `&Connection` for read paths — both out of scope.

**Resolution: single-source mode.** `Memory` stashes the `Arc<Mutex<SqliteGraphStore<'static>>>` at `with_pipeline_pool` time (and via a new `with_graph_store` lightweight constructor for tests/v0.3-without-pipeline). When the slot is `None`, `with_graph_read` returns a clear `RetrievalError::Internal` — this matches the existing pattern for v0.2-compat retrieval (callers without v0.3 graph features get a typed downgrade, not a silent empty result).

**New `Memory` field:**
```rust
graph_store: Option<Arc<Mutex<SqliteGraphStore<'static>>>>,
```

**Constructors that populate it:**
1. `with_pipeline_pool` — clone `store_arc` into the field before passing to `ResolutionPipeline::new`.
2. `with_graph_store` (new, lightweight) — leak a connection + wrap, install. For tests and callers that want graph_query without the full resolution pipeline.

**Accessor:**
```rust
/// Borrow the graph store for the duration of a closure.
///
/// Used by retrieval plans that need `&dyn GraphRead` (Factual,
/// Associative, Abstract). The closure-style API is mandatory because
/// `SqliteGraphStore<'a>` borrows a connection — we cannot return an
/// owned reference.
///
/// Returns `RetrievalError::Internal` if no graph store is installed
/// (v0.2-compat `Memory::new()` without `with_pipeline_pool` or
/// `with_graph_store`). Future tasks may add a v0.2-compat fallback by
/// refactoring `SqliteGraphStore` to accept `&Connection` for read paths.
pub fn with_graph_read<R>(
    &self,
    f: impl FnOnce(&dyn GraphRead) -> R,
) -> Result<R, RetrievalError>
```

### Step 2 — Per-plan adapters

**File**: `crates/engramai/src/retrieval/orchestrator.rs` (new — ~300 lines)

Six `pub(crate) fn` adapters. Each takes the plan's typed result + the original `GraphQuery` + a `MemoryRecord` lookup closure, and returns `Vec<ScoredResult>`.

Adapter signatures (one per plan):

```rust
pub(crate) fn factual_to_scored(
    result: FactualPlanResult,
    record_loader: &dyn RecordLoader,
) -> Vec<ScoredResult>

pub(crate) fn episodic_to_scored(...) -> Vec<ScoredResult>
pub(crate) fn associative_to_scored(...) -> Vec<ScoredResult>
pub(crate) fn abstract_to_scored(...) -> Vec<ScoredResult>  // ScoredResult::Topic
pub(crate) fn affective_to_scored(...) -> Vec<ScoredResult>
pub(crate) fn hybrid_to_scored(...) -> Vec<ScoredResult>    // mixed Memory + Topic
```

Each adapter populates `SubScores` faithfully — only the signals the source plan emitted are `Some`, the rest stay `None` (per §6.2a docstring).

`RecordLoader` trait — single method `fn load(&self, id: MemoryId) -> Option<MemoryRecord>`. Production impl wraps `Memory::storage.recall_by_id`. Test impl is a `HashMap`.

### Step 3 — Memory collaborator slots

Add 6 `Option<Arc<dyn Trait>>` fields to `Memory`. Default `None` in **all** constructors. Add public setters:

```rust
pub fn set_entity_resolver(&mut self, r: Arc<dyn EntityResolver>);
pub fn set_seed_recaller(&mut self, r: Arc<dyn SeedRecaller>);
pub fn set_topic_searcher(&mut self, s: Arc<dyn TopicSearcher>);
pub fn set_affective_seed_recaller(&mut self, r: Arc<dyn AffectiveSeedRecaller>);
pub fn set_episodic_store(&mut self, s: Arc<dyn EpisodicMemoryStore>);
pub fn set_hybrid_executor(&mut self, e: Arc<dyn HybridSubPlanExecutor>);
```

At `graph_query` call sites, fall back to `Null*` if slot is `None`. This means a brand-new `Memory::new()` can call `graph_query` without crashing — it just gets the typed downgrade outcomes (e.g., `FactualOutcome::DowngradedNoEntity` because `NullEntityResolver` returns no anchors). That's correct, observable behavior, not a patch.

### Step 4 — HybridSubPlanExecutor real impl

**File**: `crates/engramai/src/retrieval/orchestrator.rs` (continued)

```rust
pub(crate) struct HybridDispatchExecutor<'a> {
    memory: &'a Memory,
    query: &'a GraphQuery,
    context: &'a PlanContext,
    record_loader: &'a dyn RecordLoader,
}

impl HybridSubPlanExecutor for HybridDispatchExecutor<'_> {
    fn run(&mut self, kind: SubPlanKind) -> SubPlanResult {
        // Map SubPlanKind → call the corresponding plan with its
        // collaborator + budget + result adapter. Returns SubPlanResult
        // with HybridItems (memories or topics).
    }
}
```

Each `SubPlanKind` arm:
- Build that plan's `*PlanInputs` from the shared `query` + sub-budget
- Call plan's `execute()`
- Adapt the typed result into `Vec<HybridItem>` (Hybrid's own item type, not `ScoredResult` — fusion is RRF, not weighted-sum)

**Recursion concern**: `Hybrid` could in theory pick `Hybrid` as a sub-plan. Per design §4.7, `selected` comes from signals, and the `Hybrid` sub-plan kind is not in `SubPlanKind` (verified — it's the leaf wrapper). So no infinite recursion.

### Step 5 — Memory::graph_query body

```rust
pub async fn graph_query(&self, query: GraphQuery) -> Result<GraphQueryResponse, RetrievalError> {
    // (A) Dispatch — already wired
    let classifier = HeuristicClassifier::with_null_lookup();
    let dispatched = dispatch(query, &classifier);

    // (B) Plan execute — match on plan_kind, call adapter, get Vec<ScoredResult>
    let (raw_results, plan_outcome) = self.execute_plan(&dispatched)?;

    // (C) Fusion — fuse_and_rank applies per-intent weights + top-K
    let fused = fusion::fuse_and_rank(
        dispatched.intent,
        &FusionConfig::default(),  // locked() variant for graph_query_locked
        raw_results,
    );

    // (D) Top-K cutoff
    let results = fused.into_iter().take(dispatched.context.limit).collect();

    // (E) Trace assembly (only if explain) — placeholder until task:retr-impl-explain-trace
    let trace = if dispatched.context.explain {
        Some(PlanTrace::placeholder(&dispatched))
    } else {
        None
    };

    Ok(GraphQueryResponse {
        results,
        plan_used: dispatched.intent,  // §3.1 invariant: intent, not plan_kind
        outcome: plan_outcome,
        trace,
    })
}
```

`execute_plan(&dispatched) -> Result<(Vec<ScoredResult>, RetrievalOutcome), RetrievalError>` is a private method — one big `match dispatched.plan_kind` with 6 arms. Each arm:
1. Build plan inputs
2. Lock budget mutex, take owned BudgetController for this stage
3. Resolve collaborator: `self.entity_resolver.clone().unwrap_or_else(|| Arc::new(NullEntityResolver))`
4. Call plan `execute()`
5. Map plan-specific outcome → `RetrievalOutcome`
6. Call adapter to convert candidates → `Vec<ScoredResult>`
7. Return tuple

### Step 6 — Memory::graph_query_locked body

```rust
pub async fn graph_query_locked(&self, query: GraphQuery) -> Result<GraphQueryResponse, RetrievalError> {
    self.graph_query_with_config(query, FusionConfig::locked()).await
}
```

Refactor §5 to take `cfg: FusionConfig` as a parameter; both public methods delegate. **This is not a patch** — it's the right abstraction.

### Step 7 — Tests (in `orchestrator.rs` `#[cfg(test)] mod tests`)

Six "happy path" tests, one per `PlanKind`:
- Use `Memory::new(":memory:")` + appropriate test collaborator(s)
- Submit a query that routes to the target `PlanKind`
- Assert `response.plan_used == expected_intent`
- Assert `response.outcome == RetrievalOutcome::Ok` (or the right typed variant)
- Assert `response.results` length matches collaborator output

Plus six "downgrade outcome" tests:
- Each plan's primary downgrade path produces the right `RetrievalOutcome::Downgraded*`
- Empty `Memory` (all collaborators `None` / `Null*`) — every plan produces an empty results list and a non-`Ok` outcome.

Plus two "Hybrid fan-out" tests:
- Two strong signals → 2 sub-plans run, RRF fuses, top-K applied
- Three strong signals → top 2 picked, third in `dropped`, surfaced via `RetrievalOutcome` field

**Total: ~14 new tests.**

## 4. File Inventory (what gets created/modified)

**Created:**
- `crates/engramai/src/retrieval/orchestrator.rs` (~600 lines: 6 adapters + HybridDispatchExecutor + RecordLoader trait + execute_plan + tests)

**Modified:**
- `crates/engramai/src/retrieval/api.rs` — replace `graph_query` and `graph_query_locked` stub bodies (~80 lines net add)
- `crates/engramai/src/retrieval/mod.rs` — add `pub mod orchestrator;` (1 line)
- `crates/engramai/src/memory.rs` — add 6 `Option<Arc<dyn Trait>>` fields + 6 setters + `with_graph_read` (~120 lines)

**Total estimated change**: ~800 lines (within Rule 1's "main agent does it directly" envelope, not delegated).

## 5. Out of Scope (deferred to sibling tasks — confirmed by docstrings)

- **Real classifier LLM fallback** → `task:retr-impl-classifier-llm-fallback`
- **Full `PlanTrace`** → `task:retr-impl-explain-trace` (we emit a placeholder)
- **Real budget cost caps wiring** → `task:retr-impl-budget-cutoff` (we use defaults)
- **Production `EntityResolver`/`SeedRecaller`/etc. concrete impls** → graph-layer / resolution feature tasks. We thread the trait slots; implementations land elsewhere.
- **Affect-divergence Kendall-tau telemetry** → already in `affective.rs`, just plumb it through.

## 6. Risk Register

1. **`SqliteGraphStore<'static>` + `Arc<Mutex<_>>`**: locking concurrency could deadlock if a plan acquires the graph mutex twice. Mitigation: each plan call site does `let store = arc.lock().unwrap();` once, holds for the whole `execute()`, releases at end.

2. **`HybridSubPlanExecutor::run` is sync** but the outer `graph_query` is `async fn`. Plans don't `.await` anything today — fine. If a sub-plan later needs async I/O, we change `HybridSubPlanExecutor::run` to `async` then; not this task.

3. **`MemoryRecord` lookup may fail** (memory id from associative seed expansion no longer exists due to forgetting). Adapters drop these silently — that's correct (a missing memory shouldn't surface as an error).

4. **`PlanContext.budget` is `Arc<Mutex<BudgetController>>`** but plan signatures take `&mut BudgetController` directly. We `.lock()` and pass `&mut *guard` to plan — single-owner today, Hybrid will re-think when fan-out is async.

## 7. Acceptance Checklist

- [ ] All 6 PlanKind variants execute end-to-end without panic.
- [ ] Each plan's `RetrievalOutcome::Downgraded*` is reachable from `graph_query` callers.
- [ ] `response.plan_used` always matches the original `Intent` (§3.1 invariant), even when `plan_kind == Associative` from a Factual downgrade.
- [ ] `graph_query_locked` produces byte-identical output across repeated calls (determinism check via `FusionConfig::locked()`).
- [ ] Hybrid sub-plan truncation (>2 strong signals) is observable in the response outcome.
- [ ] Workspace tests: 2324 pre-existing → 2338 post-task (14 new tests, no regressions).
- [ ] No new clippy warnings.
- [ ] Skip-aware benchmarks no longer produce BLOCK on the `not yet implemented` substring (the message goes away — replaced by real execution).

---

## Decision Log

- **2026-04-27**: Chose Path (A) — explicit collaborator slots on `Memory` — over Path (B). Reason: matches existing `extractor`/`triple_extractor` pattern; cleaner test DI.
- **2026-04-27**: Confirmed task description's "5 plan variants" was imprecise; implementation needs all 6 PlanKind leaves. The dispatch task already established the 6-leaf invariant — plan execution must follow.
- **2026-04-27**: New `orchestrator.rs` file rather than expanding `api.rs` — keeps adapters + executor + RecordLoader localized. `api.rs` stays a public-surface contract file.
