# ISS-047: closed-set failure-label allowlist mismatches pipeline producers — every stage failure rolls back the whole transaction

- **Status**: in_progress
- **Severity**: blocker (graph layer is functionally non-operational for any input that triggers any stage failure)
- **Filed**: 2026-04-28
- **Discovered during**: ISS-046 fresh-ingest verification on LoCoMo conv-26 (b806485 + 950159d)
- **Related**: ISS-046 (graph DB wiring — fixed), ISS-044 (pre-discovery), ISS-021 (subdim coverage)

---

## Summary

The closed-set validator [`validate_failure_closed_sets`](../../crates/engramai/src/graph/store.rs#L937)
in `apply_graph_delta` rejects every `StageFailureRow` produced by the v0.3
resolution pipeline. The allowlist (`audit::STAGE_*` and `audit::CATEGORY_*`
constants) was designed before the resolution pipeline finalized its label
vocabulary, and the two have **zero overlap**:

- Pipeline produces: stages `{ingest, entity_extract, edge_extract, resolve, persist}` with categories `{extractor_error, candidate_retrieval_error, canonical_fetch_error, unresolved_subject, unresolved_object, find_edges_error, apply_graph_delta_error, missing_canonical, unresolved_defer, queue_full}`.
- Allowlist accepts: stages `{entity_extract, edge_extract, dedup, persist, knowledge_compile}` with categories `{llm_timeout, llm_invalid_output, budget_exhausted, db_error, internal}`.

Because validation happens **inside the persist transaction** (store.rs:4861),
any rejected failure row aborts the whole `apply_graph_delta` call —
including the successfully extracted entities, edges, mentions, and predicate
registrations that should have been committed. The pipeline returns
`Err(GraphError::Invariant)` and the worker reports a stage error; nothing
lands in the graph at all.

This is **not** a "graceful degradation" failure mode (GOAL-2.3 — partial
results recorded, failure ledger captures the rest). It is a hard rollback:
one disguised data-failure-label string == one episode worth of graph work
discarded.

---

## How it was discovered

Running `engram store` against LoCoMo conv-26 raw conversations
(`docs/locomo10/sample_data/conv-26/messages.json`) after the ISS-046 fix:

```
✓ stored 1 message → memory id=…
graph_extraction_failures: 1 row { stage="resolve", error_category="unresolved_subject" }
graph_entities: 0 rows
graph_edges: 0 rows
```

Anthropic extractor returned `1 triple` (subject="Caroline Martinez",
predicate="said", object="…"). EntityExtractor (pattern-based, design §3.2)
found 0 entities — pattern catalog has no human-name patterns, by intent.
`resolve_edges` looked up "Caroline Martinez" against `entity_drafts`,
missed → recorded `StageFailure { stage: Resolve, kind: "unresolved_subject" }`.
`drive_persist` lifted that into `StageFailureRow { stage: "resolve",
error_category: "unresolved_subject" }` and called `apply_graph_delta`.

Inside the transaction, `validate_failure_closed_sets("resolve",
"unresolved_subject")` returned `Err(Invariant("…unknown stage…"))`. The
transaction rolled back. **No memory row was preserved in the graph layer
either** — even though the L1/L2 admission write to `memories.*` (which
happens before the pipeline) was committed, the v0.3 graph tables saw
nothing.

Repeated for every input. End-to-end ingest = 100% data loss in graph
layer.

---

## Root cause

Two separate label vocabularies evolved in parallel and were never
reconciled:

1. **`audit::STAGE_*` / `CATEGORY_*` constants** (`crates/engramai/src/graph/audit.rs`)
   were defined first, modeled on a hypothetical "extractor pipeline" with
   coarse error categories (timeout / invalid_output / budget / db / internal).
   The validator was wired into `apply_graph_delta` with these as the
   allowlist.

2. **`PipelineStage::as_str()` and `record_failure(stage, kind, …)` call
   sites** in `crates/engramai/src/resolution/{pipeline,stage_persist,
   stage_edge_extract}.rs` were written later as the pipeline took shape.
   Authors used short ad-hoc strings ("extractor_error",
   "unresolved_subject", …) for `kind`, which became `error_category` in
   `StageFailureRow` (stage_persist.rs:316).

Neither side referenced the other. No CI / unit test exercised an
end-to-end path where `ctx.failures` actually flowed through
`apply_graph_delta`'s validator. Single-stage tests fed the validator with
allowlist constants directly and looked clean.

The bug existed since the resolution pipeline was wired into apply_delta
(commit a772bce / 1757d20 era), but was masked because:
- pipeline tests inject failures via `ctx.failures.push(StageFailure::new(
  …, "llm_5xx", …))` with arbitrary kinds → `apply_graph_delta` is mocked.
- store tests validate the allowlist directly with `STAGE_*`/`CATEGORY_*`
  constants → never see real pipeline strings.
- T9 backfill (ISS-043) used migration-side constants from
  engramai-migrate, which has its own (correct, but partial) allowlist.

The first time the two vocabularies meet is on the live ingest path, in
production.

---

## Pipeline-side label inventory (full enumeration)

### Stages (from `PipelineStage::as_str()` in `resolution/context.rs:46`)

| Variant | as_str | In audit allowlist? |
|---|---|---|
| `Ingest` | `"ingest"` | ❌ missing |
| `EntityExtract` | `"entity_extract"` | ✅ present |
| `EdgeExtract` | `"edge_extract"` | ✅ present |
| `Resolve` | `"resolve"` | ❌ missing |
| `Persist` | `"persist"` | ✅ present |

### Categories (from grep of `record_failure(...)` and `failure_row(...)` calls)

| Category | Producer | In audit allowlist? |
|---|---|---|
| `"extractor_error"` | `stage_edge_extract.rs:56` (LLM extractor returned error) | ❌ missing |
| `"candidate_retrieval_error"` | `pipeline.rs:495` (`store.search_candidates` failed) | ❌ missing |
| `"canonical_fetch_error"` | `pipeline.rs:544` (`store.get_entity` failed) | ❌ missing |
| `"unresolved_subject"` | `pipeline.rs:617` (edge subject not in drafts/entities) | ❌ missing |
| `"unresolved_object"` | `pipeline.rs:634` (edge object EntityName not resolvable) | ❌ missing |
| `"find_edges_error"` | `pipeline.rs:661` (`store.find_edges` failed) | ❌ missing |
| `"apply_graph_delta_error"` | `stage_persist.rs:394` (LLM tie-break path errored) | ❌ missing |
| `"missing_canonical"` | `stage_persist.rs:218` (MergeInto without canonical row) | ❌ missing |
| `"unresolved_defer"` | `stage_persist.rs:240, 281` (DeferToLlm reached persist) | ❌ missing |
| `"queue_full"` | `queue.rs:116` (documented but not yet wired — §5.2 future) | ❌ missing |

Allowlist categories `{llm_timeout, llm_invalid_output, budget_exhausted,
db_error, internal}` are **never produced** by the live pipeline. They
exist only for forward-compatibility with hypothetical retry-aware error
classification, which has not been implemented.

### Knowledge-compile side (separate path, not affected)

`knowledge_compile/synthesis.rs:293` constructs `ExtractionFailure` rows
directly with `STAGE_KNOWLEDGE_COMPILE` and the existing allowlist
categories. That path validates correctly and is not part of this bug.

---

## Fix plan (root fix, no patch)

### 1. Extend audit allowlist to cover the full label vocabulary

`crates/engramai/src/graph/audit.rs` — add constants:

```rust
pub const STAGE_INGEST: &str = "ingest";
pub const STAGE_RESOLVE: &str = "resolve";

pub const CATEGORY_EXTRACTOR_ERROR: &str = "extractor_error";
pub const CATEGORY_CANDIDATE_RETRIEVAL_ERROR: &str = "candidate_retrieval_error";
pub const CATEGORY_CANONICAL_FETCH_ERROR: &str = "canonical_fetch_error";
pub const CATEGORY_UNRESOLVED_SUBJECT: &str = "unresolved_subject";
pub const CATEGORY_UNRESOLVED_OBJECT: &str = "unresolved_object";
pub const CATEGORY_FIND_EDGES_ERROR: &str = "find_edges_error";
pub const CATEGORY_APPLY_GRAPH_DELTA_ERROR: &str = "apply_graph_delta_error";
pub const CATEGORY_MISSING_CANONICAL: &str = "missing_canonical";
pub const CATEGORY_UNRESOLVED_DEFER: &str = "unresolved_defer";
pub const CATEGORY_QUEUE_FULL: &str = "queue_full";
```

Update `validate_failure_closed_sets` in `graph/store.rs:937` to include
all of them in `STAGES` and `CATEGORIES` arrays.

Mirror the same additions in `engramai-migrate/src/failure.rs` (its
`validate_stage` and `validate_error_category` functions, which were
already extended for `STAGE_TOPIC_CARRY_FORWARD`).

### 2. Replace string literals at call sites with constants

- `resolution/stage_edge_extract.rs:56` → `CATEGORY_EXTRACTOR_ERROR`
- `resolution/pipeline.rs:495,544,617,634,661` → respective constants
- `resolution/stage_persist.rs:218,240,281,394` → respective constants

This eliminates the drift surface — adding a new failure category in v0.4
will require touching the constants module, not finding-and-replacing
string literals.

### 3. End-to-end test that exercises the validator

Add an integration test in
`crates/engramai/tests/graph_extraction_failures_e2e.rs` (new file) that:

1. Spins up an in-memory pipeline with a triple extractor that returns
   one valid + one unresolvable triple.
2. Runs `pipeline.process(job)` end-to-end.
3. Asserts:
   - `apply_graph_delta` did **not** return `Err(Invariant)`.
   - `graph_extraction_failures` has exactly one row with
     `stage="resolve"`, `error_category="unresolved_subject"`.
   - The successfully resolved entity/edge **did** land in `graph_entities`
     and `graph_edges` (partial-completion semantics — GOAL-2.3).
4. Repeats for every other category that the pipeline can produce, by
   driving inputs that trigger each path.

This test is the regression fence: any future label drift will fail it.

### 4. Defensive consistency assertion (optional, follow-up)

Consider adding a `#[test]` in `audit.rs` that checks every variant of
`PipelineStage` (in resolution/context.rs) has a corresponding entry in
the audit `STAGES` array. This requires either pulling the enum into
audit (layer violation) or exposing a small `pipeline_stages_for_audit()`
helper. Defer this to a follow-up — the e2e test in step 3 catches the
same drift dynamically and is sufficient for now.

---

## Out of scope / follow-ups

- **Entity-extraction coverage gap** (the *next* bug after this lands).
  EntityExtractor is pattern-based; LoCoMo conversational text contains
  human names ("Caroline", "Tom") that the v0.2 pattern catalog never
  matches, so every conversational triple's subject is unresolvable. This
  is a known design tension (cheap pattern extraction vs full NER) tracked
  by ISS-021 (subdim coverage). After ISS-047 lands, re-run the LoCoMo
  ingest to see what the **real** entity coverage is — current 0% is
  100% masked by the rollback bug. ISS-021 may need to be re-prioritized
  upward depending on what the post-fix numbers show.

- **Categorize errors at the source** (long-term). Most pipeline failures
  fall into 2-3 real classes (transient/network, invalid-input, internal).
  The current category vocabulary is a leaky abstraction — every code path
  invents its own string. A future `enum FailureCategory` would be a
  cleaner design, but is a v0.4-scoped refactor that requires migration
  considerations (`graph_extraction_failures.error_category` is TEXT in
  the schema and existing rows would need rewriting).

---

## Acceptance criteria

- [ ] All 12 new constants land in `crates/engramai/src/graph/audit.rs`
- [ ] `validate_failure_closed_sets` accepts every value produced by the
      live pipeline (verified by exhaustive enumeration test)
- [ ] Every `record_failure(...)` and `failure_row(...)` call site uses a
      `CATEGORY_*` constant (no remaining string literals in resolution
      code — grep clean)
- [ ] `engramai-migrate/src/failure.rs` mirrors the same allowlist
- [ ] New e2e integration test passes: `cargo test -p engramai
      --test graph_extraction_failures_e2e`
- [ ] Existing test suite still passes: `cargo test -p engramai
      -p engramai-migrate`
- [ ] LoCoMo conv-26 fresh ingest produces non-zero `graph_entities` and
      `graph_edges` rows for the cases where extraction does succeed
      (i.e., the rollback no longer masks success)
