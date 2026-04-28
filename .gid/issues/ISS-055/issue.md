# ISS-055: Pipeline namespace propagation broken — `PipelineConfig::namespace` is always empty, so resolution writes graph rows under the wrong namespace and retrieval finds nothing

- **Status**: implemented (pending conv-26 acceptance re-run)
- **Severity**: critical (blocks ISS-051, ISS-053, ISS-054 — every retrieval plan that depends on graph entities/edges/topics returns empty for non-trivial namespaces; the LoCoMo conv-26 measurement showed 17/17 Factual queries downgrading to `DowngradedFromAssociativeNoSeeds` purely because of this)
- **Filed**: 2026-04-28
- **Discovered during**: ISS-049 Phase 4 LoCoMo conv-26 acceptance, after fixing the dead `outcome` mapping in `Orchestrator::execute_associative_inner` (commit 2026-04-28). With the mapping fix, the orchestrator now reports the *real* outcome distribution; 17/17 Factual queries surfaced as `DowngradedFromAssociativeNoSeeds`. Investigation traced the empty seeds to graph entities being written under namespace `""` while the user's `--ns conv26` query reads from `"conv26"` — no overlap → no seeds.
- **Related**: ISS-050 (sibling: `with_pipeline_pool` graph-store namespace was hardcoded to `"default"` in an earlier patch — this issue is the *worker* path, ISS-050 was the *store wiring* path; both must be fixed coherently). ISS-051 (Abstract plan empty: `compile_knowledge` stub — but even after the stub is replaced, topics will be empty for the same namespace reason). ISS-053 (Associative empty seeds — *symptom* of ISS-055). ISS-054 (Affective missing table — *separate* root cause, not blocked on ISS-055).

---

## Summary

The resolution pipeline carries a per-instance `PipelineConfig.namespace`
field (`crates/engramai/src/resolution/pipeline.rs:195`) that gets passed
to `retrieve_candidates` (`pipeline.rs:501`) and into every graph-store
write path during a `ResolutionPipeline::process` run. **This field is
constructed via `PipelineConfig::default()` in `Memory::with_pipeline_pool`
(`memory.rs:~410`) and is never overwritten with the actual user namespace.**
Consequently:

- All entities/edges produced by the resolution worker land under
  `namespace=""` (empty string).
- All `store_raw` admissions correctly write to the user's namespace
  (e.g. `conv26`) — the `memories` table is fine.
- All retrieval plans that scope by namespace (Associative seed lookup,
  Abstract topic lookup once ISS-051 lands) read from the user namespace
  (e.g. `conv26`) and find zero rows.

The bug is invisible in the "default" namespace case (everything is
`""`-vs-`""` or `"default"`-vs-`"default"` depending on which hardcode
you hit) and only surfaces under non-default `--ns` flags. LoCoMo's
per-conversation isolation (`--ns conv26`) is the canonical reproducer.

---

## Reproduction

```
# Fresh DB, ingest under non-default namespace
engram --database /tmp/iss055.db --graph-database /tmp/iss055-graph.db \
  store "Caroline lives in Berlin" --ns conv26
# Wait for resolution worker to drain (or use --sync flag)

# Inspect: memories row IS in the right namespace
sqlite3 /tmp/iss055.db "SELECT id, namespace FROM memories;"
# id=...  namespace=conv26   ← correct

# Inspect: graph entities are in WRONG namespace
sqlite3 /tmp/iss055-graph.db "SELECT id, namespace FROM entities;"
# id=...  namespace=          ← BUG: should be conv26

# Recall under correct namespace finds nothing
engram --database /tmp/iss055.db --graph-database /tmp/iss055-graph.db \
  recall "Where does Caroline live?" --ns conv26
# Outcome: DowngradedFromAssociativeNoSeeds, hits=0

# Recall under empty namespace "succeeds" (proves the data is there,
# under the wrong key)
engram --database /tmp/iss055.db --graph-database /tmp/iss055-graph.db \
  recall "Where does Caroline live?" --ns ""
# Outcome: Ok, hits>0
```

---

## Root cause

`Memory::with_pipeline_pool` constructs the `ResolutionPipeline` with
`PipelineConfig::default()`:

```rust
// memory.rs (around line 405-411)
let pipeline = ResolutionPipeline::new(
    memory_reader,
    entity_extractor,
    triple_extractor,
    store_arc,
    PipelineConfig::default(),   // ← namespace = ""
);
```

But the user's namespace is **not knowable at this construction site** —
`with_pipeline_pool` is called once per `Memory` instance, while
namespace is a per-`store_raw` parameter. The pipeline has no API surface
today to receive the namespace alongside the `PipelineJob`.

**The architectural gap:**

- `PipelineJob::initial(memory_id, episode_id)` carries no namespace.
- `ResolutionPipeline::process(job)` reads the memory row from the
  `MemoryReader` (which has access to the row's `namespace` column!) but
  ignores it; resolves and writes using `self.config.namespace`.
- `self.config.namespace` is fixed at construction time and is `""`.

So the namespace is *available* (on the memory row) but *not threaded
through* to the resolution write path.

---

## Impact

**Direct (currently observable):**

- 17/17 Factual queries on LoCoMo conv-26 downgrade to
  `DowngradedFromAssociativeNoSeeds` (after ISS-055-precursor outcome
  mapping fix). The seeds-by-namespace lookup finds zero entities for
  `--ns conv26` because all conv-26 entities are stored under `""`.
- Any multi-tenant or multi-conversation deployment of engram v0.3
  has identical breakage.

**Transitive (blocks):**

- **ISS-051** — Even when `compile_knowledge` is replaced with the real
  K1→K3 body, it will read entities under `--ns conv26` (the user's
  intent) and find zero, because the worker wrote them under `""`.
  Topic compilation will produce zero topics. Abstract plan stays dead.
- **ISS-053** — Associative empty-seeds is the *direct symptom* of this
  bug; it has no independent root cause. Once ISS-055 is fixed, ISS-053
  should self-resolve (verify on conv-26 acceptance).
- **ISS-054** — Affective plan returns `NoCognitiveState`. This is a
  *separate* root cause (missing table) but won't be testable end-to-end
  until ISS-055 unblocks the namespace path.

**Hybrid plan:** also affected — its sub-plans inherit the same namespace
mismatch. `HybridPlanFailed` (2/22 in conv-26) is partially attributable
to this bug.

---

## Fix plan (proposed)

Two design options. Pick one before implementation.

### Option A — Carry namespace on the job

1. Extend `PipelineJob` to include `namespace: String` (alongside
   `memory_id` and `episode_id`).
2. `enqueue_pipeline_job` in `memory.rs:808` reads the namespace from
   the just-admitted `StorageMeta` (or from the row via a follow-up
   read) and stamps it onto the job.
3. `ResolutionPipeline::process` uses `job.namespace` instead of
   `self.config.namespace` for `retrieve_candidates` and all graph
   writes.
4. Remove `PipelineConfig::namespace` (or keep as fallback for tests).

**Pro:** namespace travels with the job — no ambient state, no per-job
config mutation. Crash-recoverable (queued jobs in `graph_pipeline_runs`
have the namespace persisted).

**Con:** schema change to `PipelineJob` + crash-recovery row format.

### Option B — Read namespace from the memory row

1. `ResolutionPipeline::process` already reads the memory row via
   `MemoryReader`. Extend `MemoryReader::fetch` to return the row's
   `namespace` column.
2. Use that namespace for the rest of the resolution flow.
3. `PipelineConfig::namespace` becomes vestigial → remove.

**Pro:** no `PipelineJob` schema change; namespace is sourced from the
canonical place (the memory row itself).

**Con:** one extra column on the reader return type; namespace is
implicit (not visible in the queue).

### Recommendation

**Option B.** The memory row is already authoritative for namespace;
duplicating it on the job invites drift. Crash recovery is unaffected
(memory row has the namespace; reader picks it up on replay).

---

## Acceptance criteria

1. After ingest with `--ns conv26`, `SELECT namespace FROM entities` in
   the graph DB returns `conv26` for all rows produced by the resolution
   worker.
2. LoCoMo conv-26 acceptance run: `outcome:DowngradedFromAssociativeNoSeeds`
   count drops from 17/22 to ≤2/22 (residual genuine empty-seed cases
   only — queries asking about entities not in the corpus).
3. `outcome:Ok` count rises to ≥10/22 on Factual queries (subject to
   ranking quality, which is ISS-049's scope, not ISS-055's).
4. Single-namespace tests (existing v0.3 unit tests in `pipeline.rs`)
   continue to pass — namespace `""` and `"default"` flow correctly.
5. New regression test: `tests/it_pipeline_namespace.rs` — ingest under
   `--ns alpha`, verify entities written under `alpha`; ingest under
   `--ns beta`, verify isolation; query under `alpha` retrieves only
   alpha rows.

---

## Investigation log

### 2026-04-28 — Discovery via outcome mapping fix

Started ISS-049 Phase 4 LoCoMo conv-26 acceptance run with the wired
`GraphTopicSearcher`. Initial results showed 22/22 queries returning
`outcome:Ok` with `hits=0` — suspicious. Investigation revealed:

1. **Dead match arm in `Orchestrator::execute_associative_inner`**
   (`crates/engramai/src/orchestration/orchestrator.rs`): the
   `AssociativeOutcome` → `RetrievalOutcome` mapping had a fallthrough
   that collapsed `Empty`, `DowngradedNoSeeds`, and `Cutoff` all into
   `RetrievalOutcome::Ok`. **Fixed** in same session — typed mapping
   now distinguishes all four variants.

2. **After mapping fix, re-ran conv-26**:
   - `outcome:DowngradedFromAssociativeNoSeeds`: **17/22** (Factual)
   - `outcome:DowngradedFromAbstractCompileFailed`: **4/22** (Abstract → ISS-051)
   - `outcome:NoCognitiveState`: **2/22** (Affective → ISS-054)
   - `outcome:HybridPlanFailed`: **2/22** (Hybrid)

3. **Investigated empty seeds**: SQL audit on
   `/tmp/iss049-phase4-locomo-graph.db`:
   - `SELECT COUNT(*) FROM entities WHERE namespace='conv26'` → **0**
   - `SELECT COUNT(*) FROM entities` → **>0**
   - `SELECT DISTINCT namespace FROM entities` → `""` (empty string)

4. **Traced to `PipelineConfig::default()`**: `with_pipeline_pool`
   constructs the pipeline with default config, never threading the
   user's namespace through. Confirmed by reading
   `crates/engramai/src/memory.rs:299-411` and
   `crates/engramai/src/resolution/pipeline.rs:186-210`.

### 2026-04-28 — Outcome mapping fix (precursor, already committed)

Before discovering ISS-055, fixed a dead match arm in the orchestrator
that was masking the real outcome distribution. This fix is **not**
ISS-055 — it's a separate observability bug whose resolution made
ISS-055 visible. Recorded for traceability:

- File: `crates/engramai/src/orchestration/orchestrator.rs`
- Change: `AssociativeOutcome::{Empty, DowngradedNoSeeds, Cutoff, Ok, EntityFoundNoEdges}` now map to typed `RetrievalOutcome` variants instead of falling through to `Ok`.
- Verification: re-ran `locomo_conv26_retrieval` binary → outcome
  histogram now shows the four distinct downgrade categories above.

---

## Decisions pending

- **Approve Option A vs B.** Default recommendation: B.
- **Scope of ISS-055 vs ISS-050.** ISS-050 was filed earlier for the
  hardcoded `"default"` in `with_pipeline_pool`'s graph store wiring.
  Both bugs share the same architectural gap (namespace not threaded
  through pipeline construction). Should ISS-055 supersede ISS-050, or
  should they be fixed as one PR? Recommendation: merge into ISS-055,
  mark ISS-050 as "duplicate of ISS-055" (broader scope) and link.

---

## Notes

- Outcome mapping fix is the *precursor* that made this issue visible.
  Without it, ISS-055 would have stayed masked behind `Ok+hits=0` for
  another release cycle. Worth a brief retro on "observability bugs that
  hide root causes" — but separately, not in this issue.
- This is a textbook case of "the fix unmasked the real bug" — exactly
  the reason `outcome` typing matters for v0.3.

---

## Implementation Record (2026-04-28)

**Chosen design:** Option B (read namespace from the memory row).

**Changes:**

1. `MemoryReader::fetch()` now returns `(MemoryRecord, String)` — the
   second element is the row's `namespace` column, sourced via new
   `fetch_memory_record_with_namespace()` in `storage.rs`.
2. `BackfillResolver::resolve_for_backfill()` accepts an explicit
   `namespace: &str` parameter (replaces previous reliance on
   `PipelineConfig::namespace` ambient state).
3. `PipelineContext` extended with `namespace: String` field — threaded
   through every resolution stage.
4. `GraphWrite::set_namespace()` added; `ResolutionPipeline::process`
   stamps the namespace on the graph store before each delta persist,
   candidate retrieval, entity lookup, and edge resolution.
5. `engramai-migrate::processor` updated to the new tuple-returning
   reader signature.

**Test coverage:**

- New file: `crates/engramai/tests/iss055_pipeline_namespace_test.rs`
  - `empty_namespace_ingest_still_works` — backward compatibility with
    namespace `""`.
  - `entities_written_under_user_namespace` — ingest under `--ns alpha`,
    verify entities/edges land under `alpha`.
  - `alpha_and_beta_namespaces_are_isolated` — multi-tenant isolation
    in the same DB.
- All 3 new tests pass.
- Full workspace suite: **2099 tests pass** (203 resolution / 1758
  engramai / 153 migrate / others).

**Acceptance criteria status:**

- [x] AC1 — entities table reflects user namespace (covered by
  `entities_written_under_user_namespace` test).
- [x] AC4 — single-namespace tests still green (full suite passes).
- [x] AC5 — new regression test exists and passes.
- [ ] AC2/AC3 — LoCoMo conv-26 outcome distribution: **pending re-ingest
      under `--ns conv26` + re-run of `locomo_conv26_retrieval`**. The
      existing smoke DBs were ingested before this fix and contain the
      buggy namespace data; they cannot validate the fix without a fresh
      ingest cycle. Tracked separately for the next acceptance pass.

**ISS-050 disposition:** superseded by ISS-055 (same architectural gap,
broader fix). To be closed-as-duplicate.
