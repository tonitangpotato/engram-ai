# Design: ISS-059 — Thread query namespace into Abstract plan

> Post-hoc design doc for the fix that landed in `crates/engramai/src/retrieval/orchestrator.rs` lines 640-647 and 940-948. Implementation is already on `main` (status: `in_review`); this document records the design rationale so the fix can be reviewed, audited, and extended without re-deriving the namespace propagation chain from scratch.

## 1 Overview

ISS-059 is the third (and final, in v0.3) entry in a chain of namespace-propagation fixes:

1. **ISS-049** wired the retrieval orchestrator's plan executors to real adapters (replacing `Null*` stubs). After Phase 3 landed, adapters could in principle scope SQL by namespace — but the namespace itself never reached them from the public API.
2. **ISS-056** added `Option<String> namespace` to `GraphQuery`, exposed `GraphQuery::with_namespace(...)`, and threaded the value through `Memory::graph_query` (`api.rs:406-466`) so the **directly-dispatched** plan executors (Factual, Episodic, Affective, Associative) read the per-query namespace instead of a hardcoded `"default"`.
3. **ISS-059** (this issue) closed the last two leaks: the **Abstract plan inputs**, constructed at two distinct sites inside `orchestrator.rs`, still read `namespace = "default"` even after ISS-056. The user-visible symptom was `RetrievalOutcome::DowngradedFromAbstract { reason: "L5_unavailable" }` returning empty result sets on conv-26 because the Abstract plan was scanning the wrong (empty) namespace before falling through to Associative fallback.

The fix is two surgical edits — one inside the Hybrid sub-plan dispatcher, one inside `execute_plan`'s direct Abstract arm — both reading `query.namespace.as_deref().unwrap_or("default")` instead of a literal `"default"`.

## 2 Goals & non-goals

### Goals

- **G-1.** Every code path that constructs an `AbstractPlanInputs` reads the same namespace value as the directly-dispatched non-Abstract plans (i.e., the value derived from `GraphQuery::namespace` per ISS-056).
- **G-2.** The `DowngradedFromAbstract { reason: "L5_unavailable" }` outcome is only emitted when the Abstract plan genuinely cannot find topics in the **caller-specified** namespace — never as a side-effect of scanning the wrong namespace.
- **G-3.** No public API change. `GraphQuery`, `RetrievalOutcome`, `AbstractPlanInputs`, `Memory::graph_query` keep their ISS-056 shape.

### Non-goals

- **NG-1.** Add multi-namespace dispatch (i.e., a single query fanning out across N namespaces). That is explicitly Phase 4 / future work per the comment at `api.rs:452-453`.
- **NG-2.** Change Associative-fallback behavior. ISS-059 only changes which namespace the Abstract plan scans first; the fallback chain itself is unchanged.
- **NG-3.** Backfill orchestrator unit-test infrastructure. AC #4 (orchestrator-level unit test for namespace propagation through Hybrid) is deferred — the test scaffolding does not yet exist for `DispatchedQuery::execute_plan` and building it is a separate scoped piece of work.
- **NG-4.** Re-ingest the LoCoMo smoke DB. AC #5 (validation re-run after fix) is deferred until the smoke DB has the `graph_entities` / `knowledge_topics` rows that conv-26 queries actually require — currently those tables are sparse and a green LoCoMo run is not achievable on the existing dump.

## 3 Namespace propagation chain (the picture)

The complete chain from public API down to a SQL `WHERE namespace = $1`:

```
caller code
  └─ GraphQuery::new("...")
       .with_namespace("conv26")          ← ISS-056: builder sets Option<String>
       │
       ▼
  Memory::graph_query(query)              ← api.rs:406
       │
       ├─ extract: let namespace = query.namespace.clone().unwrap_or_else(|| "default".into())   (line 409-411)
       │
       ▼
  DispatchedQuery::execute_plan(query, namespace_str, ...)  ← api.rs:464-486
       │
       ├─ For Factual / Episodic / Associative / Affective:
       │     direct adapter calls receive `namespace_str` ✅ (ISS-056 wiring)
       │
       └─ For Abstract:
            AbstractPlanInputs {
                query: &query,
                namespace: query.namespace.as_deref().unwrap_or("default"),   ← ISS-059 fix #2 (orchestrator.rs:945)
                budget,
            }
            │
            ▼
       AbstractPlan::execute(inputs, graph)
            │
            ▼
       GraphTopicSearcher::search(namespace, ...) ← real adapter from ISS-049 Phase 3
            │
            ▼
       SQL: SELECT … FROM knowledge_topics WHERE namespace = $1 …

  --- AND inside Hybrid: ---
  HybridDispatchExecutor::run(SubPlanKind::Abstract)   ← orchestrator.rs:553 (HybridSubPlanExecutor trait impl; "dispatch" is the conceptual role)
       │
       └─ AbstractPlanInputs {
              query: self.query,
              namespace: self.query.namespace.as_deref().unwrap_or("default"),   ← ISS-059 fix #1 (orchestrator.rs:643-647)
              budget: …,
          }
            │
            ▼
       (same as direct Abstract path)
```

Two key invariants the fix establishes:

- **I-1.** All four `AbstractPlanInputs::namespace` construction sites (Hybrid sub-plan + direct Abstract = the two extant sites; each fix is one site) read from `query.namespace`. There is no other path that constructs `AbstractPlanInputs`.
- **I-2.** `query.namespace.as_deref().unwrap_or("default")` is the canonical fallback expression. It matches the semantics in `api.rs:409-411` (`unwrap_or_else(|| "default".to_string())`) — same string, same `None` behavior, just expressed without an allocation because `AbstractPlanInputs::namespace: &str` borrows.

## 4 Construction sites — exact edits

Two sites are modified. Both inside `crates/engramai/src/retrieval/orchestrator.rs`.

> Note: line numbers in this section reflect the post-fix state on `main`; the issue.md references pre-fix lines (~622 / ~839) which shifted after the fix landed.

### 4.1 Site A — Hybrid sub-plan dispatcher (`HybridDispatchExecutor::run`, lines 640-647 — `run` is the `HybridSubPlanExecutor` trait method at orchestrator.rs:553; "dispatch" describes its conceptual role)

Hybrid is the dispatch kind that runs Factual + Abstract + Affective sub-plans concurrently and RRF-fuses them. The Abstract arm constructs its `AbstractPlanInputs` here.

**Before** (pre-ISS-059, conjectural reconstruction from history): hardcoded `namespace: "default"`.

**After** (current `main`):

```rust
let inputs = AbstractPlanInputs {
    query: self.query,
    // ISS-059: thread per-query namespace from `GraphQuery`
    // so Hybrid's Abstract sub-plan reads the same namespace
    // as the real adapters constructed in `Memory::graph_query`.
    namespace: self
        .query
        .namespace
        .as_deref()
        .unwrap_or("default"),
    budget: BudgetController::with_defaults(),
};
```

Why `self.query.namespace`: `HybridDispatchExecutor` holds `&GraphQuery` as `self.query`, populated by the parent `execute_plan` from the same `GraphQuery` that flowed through ISS-056's `api.rs` extraction. Reading from it here keeps the namespace consistent with the Factual / Affective sub-plans that Hybrid also dispatches.

### 4.2 Site B — Direct Abstract dispatch (`execute_plan` Abstract arm, lines 939-948)

This is the path taken when `DispatchedPlan::kind == PlanKind::Abstract` (i.e., Abstract was selected as the *primary* plan, not as a Hybrid sub-plan).

**After** (current `main`):

```rust
PlanKind::Abstract => {
    let inputs = AbstractPlanInputs {
        query: &query,
        // ISS-059: thread per-query namespace from `GraphQuery` so the
        // direct Abstract dispatch reads the same namespace as the
        // real `GraphTopicSearcher` adapter wired in
        // `Memory::graph_query`.
        namespace: query.namespace.as_deref().unwrap_or("default"),
        budget,
    };
    let plan = AbstractPlan::new(collaborators.topic_searcher);
    let result = plan.execute(inputs, graph);
    …
}
```

Why `query.namespace` (without `self.`): inside `execute_plan` `query` is owned-by-arg / re-borrowed. `&query` is what gets passed to `AbstractPlanInputs::query`; reading `query.namespace.as_deref()` from the same value ensures the input bundle is internally consistent.

### 4.3 Why two sites, not one helper

We considered factoring a helper like `fn abstract_inputs_for<'a>(query: &'a GraphQuery, budget: BudgetController) -> AbstractPlanInputs<'a>`. Rejected for v0.3 because:

- The two sites differ in `budget` source (`BudgetController::with_defaults()` for Hybrid sub-plan vs. inherited `budget` for direct dispatch).
- The Hybrid arm uses `self.query` (a stored `&GraphQuery`); the direct arm uses `query` (the owned arg). The lifetime gymnastics around a shared helper add more friction than the duplication saves.
- Two construction sites is small enough that grep-able comments (`// ISS-059`) at both make the relationship visible.

If a third Abstract-input construction site appears in v0.4 (e.g., a new dispatch kind), this trade-off should be revisited.

## 5 Mechanism: how `DowngradedFromAbstract` empties go away

Pre-fix failure mode:

1. Caller: `GraphQuery::new(q).with_namespace("conv26")`.
2. `Memory::graph_query` extracts `namespace = "conv26"` per ISS-056 ✅.
3. Direct Abstract path constructs `AbstractPlanInputs { namespace: "default", … }` ❌.
4. `AbstractPlan::execute` calls `topic_searcher.search("default", …)`.
5. Real `GraphTopicSearcher` adapter executes `SELECT … WHERE namespace = 'default' …`.
6. Zero rows (because conv-26's topics live under namespace `conv26`).
7. Plan emits `AbstractOutcome::DowngradedL5Unavailable` (the "L5 unavailable" branch in the orchestrator's match at lines 956-963).
8. `execute_plan` translates this to `RetrievalOutcome::DowngradedFromAbstract { reason: "L5_unavailable" }` and runs the Associative fallback — which also queries the wrong namespace pre-ISS-056, but ISS-056 already fixed Associative. Post-ISS-056 the fallback works against `conv26`, but the caller still observes `DowngradedFromAbstract` even when the Abstract plan *should* have succeeded.

Post-fix:

- Step 3 now reads `namespace: "conv26"`. Step 5's SQL hits the right rows. Step 7 emits `AbstractOutcome::Ok` when topics exist for the query under `conv26`. The `DowngradedL5Unavailable` outcome is reserved for cases where the caller's namespace genuinely lacks topics for that query.

This satisfies G-2: the downgrade outcome carries truthful semantics again.

## 6 Boundary — what ISS-059 explicitly does NOT touch

Carefully scoped to keep the diff small and reviewable:

- **B-1. Adapter wiring (ISS-049 territory).** Phase 3's `GraphTopicSearcher`, `GraphEntityResolver`, etc. construction sites in `Memory::graph_query` are unchanged. ISS-059 trusts that the adapters correctly use `namespace` once it reaches them.
- **B-2. `GraphQuery` shape (ISS-056 territory).** No new fields, no builder changes, no `Default` semantic shifts. The `Option<String> namespace` + `with_namespace(...)` API from ISS-056 is reused as-is.
- **B-3. `AbstractPlanInputs` shape.** The `namespace: &str` field already existed; ISS-059 only changes how callers populate it.
- **B-4. Fallback chain.** `run_associative_fallback`, the L5NotReady branch, and the `EmptyResultSet { reason: "associative_empty" }` terminal are not modified. Once Abstract returns its (now-correct) outcome, downstream behavior is identical.
- **B-5. Multi-namespace dispatch.** Out of scope per NG-1.
- **B-6. Telemetry.** `hybrid_sub_plan_outcome` log lines (added under ISS-063) emit at INFO and are unchanged. They will now log non-empty `items=N` for previously-empty Abstract sub-plans, which is the visible signal that the fix is working.
- **B-7. Stale module comment at `orchestrator.rs:499-505`.** The module-level comment still describes `Null*` collaborator stubs as current behavior; that statement was true pre-ISS-049 Phase 3 but is now false (real collaborators are wired via `PlanCollaborators`). The stale comment sits ~140 lines above the ISS-059 edits in the same file. Updating it is **out of ISS-059 scope** but should be filed as a doc-cleanup follow-up — a reviewer reading the comment in isolation will believe collaborators are still Null-stubbed, contradicting this fix's entire rationale.

## 7 Verification

### 7.1 Done

- **AC #1 — Both `AbstractPlanInputs` construction sites read `query.namespace`.** Verified by `grep -n "AbstractPlanInputs {" crates/engramai/src/retrieval/orchestrator.rs` returning exactly two hits, both immediately followed by `namespace: …query.namespace.as_deref().unwrap_or("default")…`.
- **AC #2 — `cargo build -p engramai` clean.** Implementation compiles on `main`.
- **AC #3 — Existing tests do not regress.** `cargo test -p engramai` shows three pre-existing failures, all of which predate ISS-059 and do not exercise the namespace propagation path that ISS-059 modifies (full triage in `memory/2026-04-29.md` 02:50 entry):
  1. `knowledge_compile` contract-drift failure #1 — exercises compile-stage entity extraction (ISS-047 scope), does not touch `AbstractPlanInputs` or `GraphQuery::namespace`.
  2. `knowledge_compile` contract-drift failure #2 — same module/scope as above.
  3. ISS-047 ledger-gap failure — predates the `GraphQuery::namespace` field (ISS-056) entirely and does not invoke the retrieval orchestrator.
  Marked as **pre-existing, not caused by ISS-059**. No new namespace-related failures introduced.
- **AC #6 — `DispatchOutcome::Hybrid` no_cognitive_state count.** Verified that the field exists and is incremented from the Abstract sub-plan path; ISS-059 does not change the counting logic.

### 7.2 Deferred (with rationale)

- **AC #4 — Orchestrator unit test asserting namespace propagation through Hybrid.** Deferred. `DispatchedQuery::execute_plan` lacks a unit-test fixture (it requires constructing a full `Collaborators` bundle with mock or real adapters, plus a `GraphStore` handle). Building that scaffold is a multi-hour piece of work that should be a separate issue, not bundled into ISS-059.
- **AC #5 — LoCoMo conv-26 re-run validating namespace fix.** Deferred. The smoke DB at `crates/engramai-bench/data/locomo-conv26-iss058.db` is missing the `graph_entities` and `knowledge_topics` rows that conv-26 abstract queries need. A fresh re-ingest under the correct namespace is required before the LoCoMo harness can validate the fix end-to-end.

The deferred ACs are tracked in the ISS-059 issue body itself. Closing ISS-059 to `done` should wait until those deferrals either ship or are reassigned to follow-up issues.

## 8 Failure modes & risks

- **F-1. Caller forgets `.with_namespace(...)`.** Behavior identical to pre-ISS-056: queries hit the literal `"default"` namespace. By design (ISS-056 chose `Option<String>` over a required field). Mitigation: documented on `GraphQuery::namespace` rustdoc (`api.rs:141-152`).
- **F-2. Future code adds a third `AbstractPlanInputs` construction site without threading namespace.** Caught by code review — both existing sites carry an `// ISS-059` comment that grep finds. No type-system enforcement currently exists; if a fourth site appears, consider promoting the helper described in §4.3.
- **F-3. `topic_searcher` adapter's SQL does not actually filter by `namespace`.** Out of ISS-059 scope. ISS-049 Phase 3 owns this; if violated, ISS-059's fix is moot. Worth a one-shot verification (`grep -n namespace crates/engramai/src/retrieval/adapters/graph_topic_searcher.rs`) but not gating ISS-059 review.
- **F-4. Dual namespace extraction (the latent fragility that caused ISS-059 in the first place).** The namespace is read from `query.namespace` at **two independent sites**: `api.rs:409-411` clones it into an owned `String` for adapter constructors (Factual / Episodic / Associative / Affective plans), while `orchestrator.rs:643-647` and `:945` re-read `query.namespace.as_deref()` for `AbstractPlanInputs`. Today both reads come from the same `GraphQuery` instance and are safe (I-2). But if a future refactor (a) mutates `query.namespace` between `api.rs` extraction and `execute_plan` entry, or (b) makes `dispatch()` consume or move `query.namespace`, the two sites silently diverge — one plan type queries one namespace, the rest query another, **reproducing exactly the ISS-059 bug class** (two sites independently constructing the same value, one drifting). This is the *pattern* that caused this bug, not just an incidental detail. Long-term fix: **FW-4** (a single `AbstractPlanInputs::new(query, budget)` constructor) closes the orchestrator side; threading a single `namespace: &str` through `execute_plan` (alongside the existing `namespace_str` parameter) would fully collapse both extraction sites into one.

## 9 Future work (referenced, not fixed here)

- **FW-1. Multi-namespace dispatch.** Single query → results from N namespaces, fused. Phase 4 per `api.rs:452-453` comment.
- **FW-2. Orchestrator unit-test infrastructure** so AC #4 has a home. Should be filed as a separate issue covering all retrieval-orchestrator unit tests, not just namespace.
- **FW-3. LoCoMo smoke DB re-ingest** with full graph rows so AC #5 can validate end-to-end.
- **FW-4. Static guard on `AbstractPlanInputs::namespace`.** E.g., make `AbstractPlanInputs::new(query, budget)` the only constructor and have it pull namespace from `query` automatically. Defers F-2.

## 10 Cross-references

- `engram:ISS-049` — Retrieval orchestrator wired Null stubs to real adapters (Phase 3, resolved).
- `engram:ISS-056` — `GraphQuery` namespace field + `with_namespace` builder + `api.rs` plumbing (resolved 2026-04-29).
- `engram:ISS-058` — LoCoMo conv-26 smoke harness that surfaced the symptom.
- `engram:ISS-060`, `engram:ISS-061` — earlier symptom reports tracing back to namespace propagation.
- `engram:ISS-063` — `hybrid_sub_plan_outcome` telemetry that made this bug observable post-fix.
