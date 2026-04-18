# Review: ISS-005 Task Breakdown — Synthesis Engine

**Reviewer**: RustClaw (Opus 4.6)  
**Date**: 2026-04-14  
**Source**: tasks-engine.yaml (12 nodes) + design-1-engine.md + requirements-1-engine.md  
**Round**: 1

---

## Phase 1: Individual Task Quality

### 🔴 Critical

**[Check #2] FINDING-1: synth-types and synth-mod should be one task**
synth-mod is ~10 lines (mod.rs + 1 line in lib.rs). It has no independent value and cannot be tested without synth-types. Creating empty submodules that `cargo check` passes is not a meaningful deliverable — it's a file-creation step inside synth-types.

Suggested fix: Merge synth-mod into synth-types. The combined task is still well under 4 hours. Update all edges from synth-mod to point to synth-types instead.

---

**[Check #1] FINDING-2: synth-insight missing LLM provider interface specification**
The task says "generate_insight(request, llm_provider)" but doesn't specify what `llm_provider` is. The codebase has `trait MemoryExtractor` for LLM calls (extractor.rs) but insight generation needs a *different* interface — extractor does structured fact extraction, not open-ended synthesis. The task needs to specify:
- What trait/type does the LLM provider implement?
- Does it reuse `MemoryExtractor` or define a new async fn?
- How is it injected into the synthesis engine?

Design §4.1 shows `SynthesisRequest` but doesn't name a provider trait. Design §7.1 `SynthesisEngine::synthesize()` takes `&mut Storage` but not an LLM provider parameter — it must be a field on the engine struct.

Suggested fix: Add to synth-insight description: "Define a `SynthesisLlmProvider` trait with `fn generate(&self, prompt: &str, config: &SynthesisConfig) -> Result<String>`. The DefaultSynthesisEngine holds an `Option<Box<dyn SynthesisLlmProvider>>`. When None, insight generation is skipped per §9 LLM Graceful Degradation." Also add a note to synth-engine that it takes the LLM provider in its constructor.

---

**[Check #5] FINDING-3: synth-cluster does too many things (single responsibility violation)**
synth-cluster combines:
1. Pairwise signal computation (4 queries + scoring)
2. Connected components clustering algorithm
3. Cluster splitting/filtering
4. Quality score computation
5. Emotional modulation

Items 1-4 are core clustering (~300 lines). Item 5 (emotional modulation) is a cross-cutting concern with its own config struct and is architecturally separate (design §2.4 is its own section). But the bigger issue is that items 1-3 involve *four different database query patterns* each requiring their own test fixtures. This is a 4+ hour task.

Suggested fix: Split into two tasks:
- `synth-cluster-core`: Pairwise scoring + connected components + splitting + quality. (~250 lines)
- `synth-cluster-emotional`: Emotional modulation (boost + prioritization + prompt context flag). (~80 lines, depends on synth-cluster-core)

This also improves parallelism — emotional modulation is applied *after* clustering, so another developer can start on it once cluster-core outputs MemoryCluster.

---

### 🟡 Important

**[Check #1] FINDING-4: synth-provenance mixes two different concerns**
The task combines:
1. Storage layer methods (SQL table creation, CRUD operations) — belongs in storage.rs
2. Business logic (undo_synthesis with transaction semantics, importance restoration) — belongs in synthesis/provenance.rs

The description even acknowledges this: "add to storage.rs or new file". This ambiguity means the implementer has to make an architectural decision that should be pre-decided.

Suggested fix: Clarify: SQL table creation and raw CRUD go in `src/storage.rs` (consistent with existing pattern — all SQL is in storage.rs). The `undo_synthesis()` orchestration logic (which calls multiple storage methods in a transaction) goes in `src/synthesis/provenance.rs`. The provenance.rs file calls storage methods, not raw SQL.

---

**[Check #1] FINDING-5: synth-engine acceptance criteria too vague**
"Integration test with mock LLM runs full pipeline" — which test? Where? Who writes it? The task produces engine.rs but the test is in synth-integration-tests (a separate task). So the actual acceptance criterion for synth-engine is: `cargo check` passes and the engine compiles with all components wired. That's weak — there's no way to verify the orchestration logic works until the integration test task runs much later.

Suggested fix: Add inline acceptance: "At minimum, write a `#[cfg(test)]` test in engine.rs that creates a DefaultSynthesisEngine with a mock LLM, feeds it 2 clusters (one SYNTHESIZE, one SKIP), and verifies the report counts. This doesn't require synth-integration-tests."

---

**[Check #1] FINDING-6: synth-consolidate references GUARD-3 but GUARD-3 doesn't exist in requirements-1-engine.md**
The task says "synthesis only runs if config enabled == true (GUARD-3)". Requirements-1-engine.md defines GUARD-1, GUARD-2, GUARD-4, GUARD-5, GUARD-6. GUARD-3 (Backward Compatibility) is in the *master* requirements.md, not the engine requirements. The task should reference the correct document.

Suggested fix: Change to "GUARD-3: Backward Compatibility (see master requirements.md)" to make the cross-reference explicit.

---

**[Check #3] FINDING-7: synth-unit-tests has no clear acceptance criteria**
"cargo test -- synthesis passes, all decision paths covered" — "all decision paths" is not verifiable without a checklist. How many paths? Gate alone has 9 rules × 2 exception cases = ~13 paths. Insight validation has 7 checks. Cluster has candidate filtering + split + discard conditions.

Suggested fix: Add explicit test count targets:
- gate: 9 primary paths + 2 exceptions = 11 tests minimum
- insight validation: 7 checks × (pass + fail) = 14 tests minimum
- cluster: candidate filtering (4 conditions) + split + discard + connected components = 7 tests minimum
- provenance: record/query/undo/chain = 4 tests minimum
- Total floor: ~36 unit tests

---

## Phase 2: Dependencies & Ordering

### 🔴 Critical

**[Check #7] FINDING-8: synth-insight has a hidden dependency on synth-provenance**
synth-insight's `store_insight()` creates a MemoryRecord with `is_synthesis` metadata. But the design §4.4 says "store the synthesized insight...and immediately record provenance (§5)". In the current breakdown, insight storage and provenance recording happen in the *engine* orchestration (synth-engine), which is correct. However, synth-insight's `store_insight()` function will need to know the provenance table exists to verify the insight isn't a duplicate (§4.3 check: "not already a synthesis output"). This check queries synthesis_provenance.

Actually, looking more carefully — the `store_insight` function in synth-insight should NOT call provenance. The engine does that. But the description says "store_insight(storage, output, cluster)" which implies it does the storing. This creates ambiguity: does synth-insight store the insight, or does synth-engine?

Suggested fix: Clarify responsibility boundary. synth-insight should expose:
1. `build_prompt()` — pure function
2. `call_llm()` — LLM interaction
3. `validate_output()` — validation checks
4. `compute_importance()` — importance calculation

But NOT `store_insight()`. Storage happens in the engine's transaction (insight + provenance + demotion = one atomic commit per GUARD-1). Remove store_insight from synth-insight, add it to synth-engine's description.

---

### 🟡 Important

**[Check #6] FINDING-9: synth-gate depends_on synth-cluster — is this a real dependency?**
Gate check operates on a `MemoryCluster` struct (passed in). It doesn't call clustering functions — it receives a cluster and decides. The dependency on synth-cluster means gate can't start until cluster is done, creating an unnecessary serial bottleneck.

The actual dependency is: gate needs the *type* `MemoryCluster`, which lives in synth-types.

Suggested fix: Remove `synth-gate → synth-cluster` dependency. Gate only needs synth-types (for MemoryCluster, GateConfig, etc.) and synth-mod. This allows synth-gate and synth-cluster to run in parallel after types are ready.

---

**[Check #10] FINDING-10: synth-incremental's dependency chain is suboptimal**
synth-incremental depends on synth-cluster + synth-provenance. But incremental operations also need to *trigger gate re-evaluation* (§6.1: "the cluster is re-evaluated by the gate") and *flag stale insights* which requires modifying provenance records. The full dependency should be cluster + provenance + gate.

Suggested fix: Add `synth-incremental → synth-gate` dependency edge. The incremental code needs to call gate check on updated clusters.

---

## Phase 3: Coverage & Traceability

### 🔴 Critical

**[Check #11] FINDING-11: Missing task for LLM Graceful Degradation (requirements §6.F)**
Requirements §6.F and design §9 specify: "When no LLM provider is configured: cluster discovery and gate check complete normally; synthesis phase is skipped (not errored); result indicates 'LLM unavailable'; warning log emitted."

No task covers this. It's a cross-cutting behavior that affects synth-engine (skip synthesis phase) and synth-consolidate (wire in graceful behavior). This is not an edge case — it's required behavior for users who don't configure an LLM.

Suggested fix: Either add a dedicated task `synth-graceful-degradation` or explicitly add this to synth-engine's description as a required acceptance criterion: "When LLM provider is None: discover_clusters + gate still run, synthesize() returns report with 0 insights and a BudgetExhausted or LlmUnavailable indication."

---

**[Check #11] FINDING-12: Missing task for provenance table migration**
synth-consolidate mentions "Ensure provenance table is created in init_tables()" but this is a critical SQL migration that deserves explicit coverage. The provenance table (§5.1) has specific columns and indices. It needs to be created idempotently in Storage::init() alongside existing table creation. This is NOT in synth-provenance (which does the CRUD methods) or synth-consolidate (which does consolidation wiring).

Suggested fix: Add to synth-provenance's description explicitly: "Add `CREATE TABLE IF NOT EXISTS synthesis_provenance (...)` and relevant indices to Storage::init() or a migration function. Include index on insight_id and source_id for O(1) lookups (GOAL-4)."

---

### 🟡 Important

**[Check #12] FINDING-13: No task maps to GUARD-1 (No Data Loss) transaction semantics**
GUARD-1 requires atomic per-insight transactions: INSERT insight + INSERT provenance + UPDATE source importances all in one transaction. Design §9 is explicit about this. But no individual task owns this requirement — synth-engine mentions "each cluster in try block, errors collected in report, no partial state" but doesn't specify the transaction boundary.

Suggested fix: Add to synth-engine description: "Each insight creation is a single SQLite transaction: BEGIN → insert insight → insert provenance records → update source importances (demotion) → COMMIT. If any step fails, ROLLBACK — no partial state. This is GUARD-1's core invariant." And add a corresponding integration test case.

---

**[Check #14] FINDING-14: No documentation task**
The synthesis engine introduces a new public API (`SynthesisEngine` trait, `SynthesisSettings` config). Users need to know how to enable it, configure it, and understand what it does. At minimum: doc comments on public types, and a usage example in lib.rs docstring or a README section.

Suggested fix: Add task `synth-docs`: "Add rustdoc comments to all public types in synthesis/. Add a 'Knowledge Synthesis' section to lib.rs module docs showing basic usage: enable in config, run consolidate(), check report. Add doctest showing config construction."

---

## Phase 4: Estimation & Risk

### 🟡 Important

**[Check #17] FINDING-15: synth-cluster is the highest-risk task with deepest external dependency**
synth-cluster depends on:
1. Entity indexing (B1) — not yet implemented
2. Hebbian links table schema — existing but query patterns not tested for this use case
3. Embedding similarity — existing but batch operations (`get_all_embeddings`) may be slow at scale
4. Temporal proximity — new computation not in existing codebase

Risk: O(n²) pairwise comparisons on all candidate memories. For 10K memories (SC-4 target), that's 50M pairs. The connected-components step is fine, but the pairwise scoring step needs optimization (e.g., only compute pairs where at least one signal exists — Hebbian link OR shared entity — rather than all×all).

Suggested fix: Add a note to synth-cluster: "Performance risk: naive all-pairs scoring is O(n²). Optimize: only score pairs connected by ≥1 signal (Hebbian link exists OR shared entity). Build candidate pairs from signal sources, not from full cross-product. Target: SC-4 requires <60s for 10K memories."

---

## Phase 5: Consistency

### 🟢 Minor

**[Check #20] FINDING-16: Inconsistent scope of test tasks**
synth-unit-tests says "tests in each module via #[cfg(test)] mod tests" — inline tests. But synth-integration-tests says "tests/synthesis_integration_test.rs" — separate file. This is actually correct (unit=inline, integration=separate) but the naming is misleading. synth-unit-tests sounds like a *separate* task but actually means "add tests inside the modules that were already created by synth-cluster/gate/insight/provenance."

Suggested fix: Rename synth-unit-tests to `synth-module-tests` and clarify: "This task adds #[cfg(test)] sections to the existing files created by synth-cluster, synth-gate, synth-insight, synth-provenance. NOT a separate file." Or better: fold unit test requirements into each component task's acceptance criteria (each task is responsible for its own tests).

---

**[Check #20] FINDING-17: Mixed "estimated LOC" approach**
synth-types says "~350 lines", synth-cluster "~350 lines", synth-gate "~250 lines" — but these are in the YAML descriptions, not in task metadata. Some tasks (synth-mod) have no estimate. Estimates should be consistent.

Suggested fix: Add `estimated_loc` to node metadata for all tasks. Minor cleanup.

---

## 📊 Coverage Matrix

| Design Section | Task(s) | GOAL/GUARD | Status |
|---|---|---|---|
| §2 Cluster Discovery | synth-cluster | GOAL-1 | ✅ Covered |
| §2.4 Emotional Modulation | synth-cluster (bundled) | Note A | ⚠️ Bundled with cluster (FINDING-3) |
| §2.5 Candidate Filtering | synth-cluster | GOAL-1 | ✅ Covered |
| §3 Gate Check | synth-gate | GOAL-3 | ✅ Covered |
| §3.3 Gate Telemetry | synth-gate | GOAL-3 | ✅ Covered (GateResult in gate output) |
| §4 Insight Generation | synth-insight | GOAL-2 | ⚠️ LLM interface undefined (FINDING-2) |
| §4.3 Validation | synth-insight | GOAL-2 | ✅ Covered (7 checks listed) |
| §4.4-4.5 Insight Storage | synth-insight/synth-engine | GOAL-2, GUARD-5 | ⚠️ Ownership ambiguous (FINDING-8) |
| §5 Provenance | synth-provenance | GOAL-4 | ✅ Covered |
| §5.1 Table Schema | synth-provenance/synth-consolidate | GOAL-4 | ⚠️ Split across tasks (FINDING-12) |
| §5.3 Reversibility | synth-provenance | GUARD-1 | ✅ Covered |
| §6 Incremental | synth-incremental | §6 | ⚠️ Missing gate dep (FINDING-10) |
| §7 Public API | synth-engine | — | ✅ Covered |
| §8 Config | synth-types + synth-consolidate | — | ✅ Covered |
| §9 Error Handling | synth-engine | GUARD-1 | ⚠️ Transaction not explicit (FINDING-13) |
| GUARD-1 (No Data Loss) | synth-engine | GUARD-1 | ⚠️ Transaction boundary missing (FINDING-13) |
| GUARD-2 (Cost Control) | synth-engine | GUARD-2 | ✅ Covered (budget enforcement) |
| GUARD-4 (No New Deps) | all tasks | GUARD-4 | ✅ Covered (no external deps) |
| GUARD-5 (Insight Identity) | synth-insight | GUARD-5 | ✅ Covered (MemoryRecord with metadata) |
| GUARD-6 (Signal Reuse) | synth-cluster, synth-gate | GUARD-6 | ✅ Covered |
| §6.F LLM Degradation | — | §6.F | ❌ Missing (FINDING-11) |
| Documentation | — | — | ❌ Missing (FINDING-14) |

## 🔗 Dependency Graph Issues

- `synth-gate → synth-cluster`: **False dependency** — gate receives a cluster struct, doesn't call cluster functions (FINDING-9)
- `synth-incremental` missing dep on `synth-gate`: incremental triggers gate re-evaluation (FINDING-10)
- `synth-mod` is not a real task — merge into synth-types (FINDING-1)
- Critical path: synth-types → synth-cluster → synth-engine → synth-consolidate → synth-integration-tests (5 tasks)
- After FINDING-9 fix, critical path shortens: synth-types → synth-cluster → synth-engine → ... (gate runs parallel with cluster)

## ✅ Passed Checks

- Check #4: Actionable verbs ✅ — all tasks start with Create/Implement/Write/Integrate
- Check #8: No circular dependencies ✅ — DAG is clean
- Check #9: Critical path reasonable ✅ — 5 layers, can be shortened to 4 with FINDING-9
- Check #13: Test tasks exist ✅ — unit + integration separate tasks
- Check #15: No cleanup tasks needed ✅ — new module, no old code to remove
- Check #16: Complexity distribution ✅ — tasks range 80-350 lines, reasonable
- Check #18: No unknown blockers ✅ — B1 entity indexing is tracked in graph
- Check #19: Parallel workstreams ✅ — insight and provenance can run in parallel
- Check #21: Granularity mostly consistent ✅ — except synth-mod (FINDING-1)
- Check #22: Status accuracy ✅ — all todo, no code exists yet

---

## Summary

- **Total tasks**: 12 (1 feature + 11 implementation) → **14 after apply** (1 feature + 13 implementation)
- **🔴 Critical**: 4 (FINDING-1, 2, 3, 11)
- **🟡 Important**: 8 (FINDING-4, 5, 6, 7, 8, 9, 10, 13, 14, 15)
- **🟢 Minor**: 2 (FINDING-16, 17)
- **Coverage gaps**: LLM graceful degradation (FINDING-11), documentation (FINDING-14)
- **Dependency issues**: 1 false dep (gate→cluster), 1 missing dep (incremental→gate)

## ✅ All 17 Findings Applied (2026-04-14)

| Finding | Status | Change |
|---|---|---|
| FINDING-1 | ✅ Applied | synth-mod merged into synth-types |
| FINDING-2 | ✅ Applied | SynthesisLlmProvider trait defined in synth-insight |
| FINDING-3 | ✅ Applied | synth-cluster split → synth-cluster-core + synth-cluster-emotional |
| FINDING-4 | ✅ Applied | SQL in storage.rs, orchestration in provenance.rs |
| FINDING-5 | ✅ Applied | Inline #[cfg(test)] tests added to synth-engine acceptance |
| FINDING-6 | ✅ Applied | GUARD-3 ref corrected to master requirements.md |
| FINDING-7 | ✅ Applied | Test count targets: ≥54 unit tests across modules |
| FINDING-8 | ✅ Applied | store_insight removed from synth-insight, engine owns storage |
| FINDING-9 | ✅ Applied | gate→cluster dep removed, gate only depends on types |
| FINDING-10 | ✅ Applied | synth-incremental → synth-gate dep added |
| FINDING-11 | ✅ Applied | LLM graceful degradation in synth-engine + integration test |
| FINDING-12 | ✅ Applied | Provenance table migration explicit in synth-provenance |
| FINDING-13 | ✅ Applied | GUARD-1 transaction boundary in synth-engine |
| FINDING-14 | ✅ Applied | synth-docs task added |
| FINDING-15 | ✅ Applied | O(n²) perf warning + optimization strategy in synth-cluster-core |
| FINDING-16 | ✅ Applied | Renamed synth-unit-tests → synth-module-tests, clarified inline |
| FINDING-17 | ✅ Applied | estimated_loc in metadata for all tasks |
