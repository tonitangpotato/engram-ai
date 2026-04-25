# Design Review: v03-graph-layer (r1)

- **Document**: `.gid/features/v03-graph-layer/design.md` (1151 lines, Draft)
- **Requirements**: `.gid/features/v03-graph-layer/requirements.md` (15 GOALs: GOAL-1.1 through GOAL-1.15; 10 P0 / 4 P1 / 1 P2)
- **Reviewer**: RustClaw (skill: review-design v1.1.0)
- **Date**: 2026-04-24
- **Method**: 27-check systematic pass; incremental append protocol

## Summary

- Critical: 0
- Important: 8
- Minor: 9
- Total: 17

All 17 findings approved and applied in design r3. See per-finding `Applied` notes below and the final "## Applied" section.

Graph-layer is the load-bearing data-model feature of v0.3 — every other feature consumes its types. The design is thorough and internally coherent: GOAL coverage is complete (§9 traceability table is explicit), GUARD-3 (bi-temporal non-erasure) is enforced at multiple layers (trait, SQL FK, error type), and the `GraphDelta` / `apply_graph_delta` cross-feature handoff (§5bis) is a strong atomicity boundary.

## Findings

### FINDING-1 🟡 Important — `GraphDelta` idempotence hash is fragile to serialization drift ✅ Applied

**Section**: §5bis, `graph_applied_deltas` schema + idempotence contract
**Issue**: Idempotence key is `(memory_id, delta_hash)` where `delta_hash = BLAKE3(canonical_json(delta))`. "Canonical JSON" is not defined.
**Fix**: Specify canonical serialization; frozen hash-input field subset; `#[serde(deny_unknown_fields)]`; `schema_version` column.
**Applied**: §5bis now defines a rigorous canonical JSON form (sorted keys, no whitespace, shortest-roundtrip floats, rejected NaN/Inf, canonical UUID strings); hash restricted to a documented frozen subset of identity fields; `deny_unknown_fields` applied to `GraphDelta`/`EntityMerge`/`EdgeInvalidation`/`MemoryEntityMention`; `graph_applied_deltas` carries a `schema_version` column with `GRAPH_DELTA_SCHEMA_VERSION = 1`; cross-version replay surfaces `GraphError::Invariant("delta schema version mismatch")`.

### FINDING-2 🟡 Important — Merge atomicity description contradicts stated correctness ✅ Applied

**Section**: §8 "Merge atomicity" paragraph
**Issue**: "Partially-merged state" + "no phantom or missing edge" in tension; transient double-visibility not specified.
**Fix**: Add explicit "reader semantics during merge" subsection.
**Applied**: §8 now has a "Reader semantics during merge" subsection specifying: (1) `attributes.merged_into` (now typed field `merged_into: Option<Uuid>`) set in first batch and readers transparently redirect via `get_entity`; (2) transient double-visibility of edges is by design during batches; (3) retrieval de-duplicates by `(predicate, object)`. DB-level read-lock option was considered and rejected.

### FINDING-3 🟡 Important — `edges_as_of` has no index strategy ✅ Applied

**Section**: §4.1 indexes on `graph_edges`, §4.2 `edges_as_of`
**Fix**: Add compound index + document algorithm + performance guidance.
**Applied**: Added `CREATE INDEX idx_graph_edges_subject_pred_recorded ON graph_edges(subject_id, predicate_label, recorded_at DESC)` in §4.1. `edges_as_of` docstring in §4.2 now documents the per-`(subject, predicate, object)` window selection algorithm, bounds cost to `O(edges_of_subject_at_or_before(at))`, and advises caller-side caching in v03-retrieval §4.3 for deep-history entities.

### FINDING-4 🟡 Important — Somatic fingerprint serialization is underspecified ✅ Applied

**Section**: §4.1 `somatic_fingerprint BLOB`
**Fix**: Add CHECK constraint; document dim-change migration; reader validation.
**Applied**: §4.1 `graph_entities` table gains `CHECK (somatic_fingerprint IS NULL OR length(somatic_fingerprint) = 32)`. A blob-format note cross-references GUARD-7, specifies reader-side `blob.len() == N * 4` validation (returns `GraphError::Invariant("somatic fingerprint dim mismatch")`), and lays out the coordinated migration procedure if the 8-dim fingerprint ever changes.

### FINDING-5 🟡 Important — `Entity.attributes` reserved keys are not enforced ✅ Applied

**Section**: §3.1 Entity struct + merge §3.4
**Fix**: Promote `merged_into` and `history` to first-class typed fields.
**Applied**: §3.1 `Entity` now carries `merged_into: Option<Uuid>` and `history: Vec<HistoryEntry>` as typed fields; new `HistoryEntry` struct defined. `attributes: serde_json::Value` doc explicitly rejects writes containing reserved keys with `GraphError::Invariant("reserved attribute key")`. Only the merge path (via dedicated `GraphStore` methods) writes these fields.

### FINDING-6 🟡 Important — `graph_memory_entity_mentions` vs `memories.entity_ids` divergence has no auto-detection ✅ Applied

**Section**: §4.1 divergence note
**Fix**: Add detection mechanism.
**Applied**: §4.1 note now specifies three-layer detection: (a) debug-build post-commit assertion in `apply_graph_delta` comparing both sources (raises `GraphError::Invariant("memory_entity cache divergence")`); (b) periodic consistency job emitting `ResourcePressure{subsystem:"graph_cache"}`; (c) v03-migration §3 offline reconciliation script. All three layers required for the "source of truth" claim.

### FINDING-7 🟡 Important — `traverse` has no complexity bound or cycle-break ✅ Applied

**Section**: §4.2 `traverse`
**Fix**: Add `max_results`, visited-set cycle handling, ordering contract, complexity note.
**Applied**: §4.2 `traverse` signature now takes required `max_results: usize`. Docstring specifies: BFS ordering by depth then by `activation DESC` within a depth level; visited-set (`HashSet<Uuid>` over entity ids) prevents cycles; bounded output (`O(max_results)`); worst-case `O(sum_of_fanout_up_to_max_depth)` if `max_results` saturates early.

### FINDING-8 🟡 Important — `apply_graph_delta` idempotence short-circuit is under-constrained ✅ Applied

**Section**: §5bis `apply_graph_delta` contract
**Fix**: Explicitly state the `graph_applied_deltas` insert happens inside the same transaction; add crash-recovery test plan.
**Applied**: §5bis contract now states the `graph_applied_deltas` row is written inside the same transaction as entity/edge/mention writes (so a partial first call either commits both or rolls back both). Test-plan bullet added: "Kill process between final commit and external acknowledgement; restart, re-call apply_graph_delta with same delta, verify `already_applied=true` and no duplicate rows."

### FINDING-9 🟢 Minor — `EntityKind::Other(String)` allows unbounded growth without policy ✅ Applied

**Section**: §3.1 `EntityKind`
**Fix**: Document normalization (lowercase, trim, collapse whitespace).
**Applied**: §3.1 now has an "`EntityKind::Other(String)` normalization policy" paragraph: lowercased, whitespace-trimmed, internal whitespace collapsed to `_`; `list_entities_by_kind` normalizes query input before comparison; `Other` discouraged when a canonical variant fits.

### FINDING-10 🟢 Minor — `graph_entities.namespace` has no lifecycle ✅ Applied

**Section**: §4.1
**Fix**: Namespaces subsection covering creation, listing, deletion, cross-namespace edges.
**Applied**: §3.4+ "Namespace lifecycle" paragraph added: implicit creation on first write; new `GraphStore::list_namespaces()` method; deletion unsupported in v0.3 (use per-namespace DB file); cross-namespace edges disallowed at trait boundary (raises `GraphError::Invariant("cross-namespace edge")`).

### FINDING-11 🟢 Minor — `Migrated` has no accompanying migration-origin metadata ✅ Applied

**Section**: §3.2 `ResolutionMethod::Migrated`
**Fix**: Add `ConfidenceSource` variant field.
**Applied**: §3.2 adds `ConfidenceSource { Recovered, Defaulted, Inferred }` enum and `Edge.confidence_source: ConfidenceSource` field (default `Recovered`; v03-migration sets `Defaulted` when importing v0.2 triples without confidence signal). Retrieval can distinguish silent defaults from observed values.

### FINDING-12 🟢 Minor — `MemoryLayer::classify_layer` derivation left to another feature ✅ Applied

**Section**: §9 note on GOAL-1.14
**Fix**: Provisional formula.
**Applied**: §9 GOAL-1.14 note now provides provisional formula: `pinned → Core`; `core_strength ≥ 0.7 → Core`; `working_strength ≥ 0.3 → Working`; else `Archived`. Marked "subject to refinement by consolidation"; pure-function contract stated (no clock read, no I/O).

### FINDING-13 🟢 Minor — `record_predicate_use` hot-path contention ✅ Applied

**Section**: §4.2 `record_predicate_use`, §4.1 `graph_predicates.usage_count`
**Fix**: Batch predicate-use updates at transaction commit.
**Applied**: §4.1+ (or §4.2 docstring) specifies `SqliteGraphStore` accumulates `(predicate_kind, predicate_label) → delta` in a per-transaction `HashMap` and issues one aggregated UPDATE per distinct predicate at commit. For a 500-edge delta using 6 predicates, 6 UPDATEs not 500. `usage_count` remains exact.

### FINDING-14 🟢 Minor — `with_transaction` escape hatch lacks safety guidance ✅ Applied

**Section**: §4.2 `with_transaction`
**Fix**: Add docstring warning.
**Applied**: §4.2 `with_transaction` trait method now carries an explicit safety warning in its docstring: permits arbitrary SQL, not audited against GUARDs, intended only for advanced test fixtures and one-off migration scripts; production code paths MUST NOT call it.

### FINDING-15 🟢 Minor — `memory_id ON DELETE SET NULL` subtly violates GUARD-3 ✅ Applied

**Section**: §4.1 `graph_edges.memory_id` FK
**Fix**: Change to `ON DELETE RESTRICT`.
**Applied**: §4.1 `graph_edges.memory_id` FK changed from `ON DELETE SET NULL` to `ON DELETE RESTRICT`. §8 "FK enforcement" paragraph updated to reflect the new uniform RESTRICT policy and to note that memory deletion is unsupported in v0.3 (pin/archive instead).

### FINDING-16 🟢 Minor — `knowledge_topics.embedding` has no dimension check ✅ Applied

**Section**: §4.1 `knowledge_topics`
**Fix**: Add dimension validation.
**Applied**: §4.1 `knowledge_topics.embedding` BLOB comment updated to note read-time validation; dimension-check rule cross-referenced to the `somatic_fingerprint` blob-format note (FINDING-4). Reader code validates `blob.len() == EMBEDDING_DIM * 4` and returns `GraphError::Invariant("topic embedding dim mismatch")` on mismatch. Dim-change migration procedure mirrors the fingerprint case.

### FINDING-17 🟢 Minor — No documented plan for `graph_resolution_traces` growth ✅ Applied

**Section**: §4.1 `graph_resolution_traces`
**Fix**: Retention policy.
**Applied**: §4.1+ "Trace retention" paragraph added: opt-in via `EngramConfig.graph.record_traces: bool` (default `true` in v0.3); periodic pruner deletes succeeded-run trace rows older than `trace_retention_days` (default 30); failed-run traces retained indefinitely for post-mortem; no partitioning in v0.3 (deferred to v0.4 if volume demands).

## Applied

All 17 findings verified against design.md (1324 lines) and applied. Reconciliation pass 2026-04-24 confirmed each change is present; FINDING-8 received an additional explicit "Crash-recovery & idempotence invariant" paragraph in §5bis to make the same-transaction guarantee unambiguous.

### FINDING-1 ✅
- Section: §5bis `graph_applied_deltas` + canonical-serialization block
- Change: Canonical JSON rules (sorted keys, shortest-roundtrip floats, `-0.0`→`0.0`, NaN/Inf rejected), frozen hash-input field table, `#[serde(deny_unknown_fields)]`, `GRAPH_DELTA_SCHEMA_VERSION` constant + `schema_version` PK column, cross-version replay raises `Invariant`.

### FINDING-2 ✅
- Section: §8 "Reader semantics during merge"
- Change: Redirect signal (`merged_into`) lands in first batch txn; `get_entity` auto-follows; loser filtered from listings; retrieval de-dups by `(predicate, object)` to absorb transient multi-batch visibility.

### FINDING-3 ✅
- Section: §4.1 indexes + §4.2 `edges_as_of`
- Change: Added compound index `idx_graph_edges_subject_pred_recorded (subject_id, predicate_label, recorded_at DESC)`; documented as-of algorithm and warm-path caching guidance.

### FINDING-4 ✅
- Section: §4.1 `somatic_fingerprint` + blob-format note
- Change: `CHECK (length(somatic_fingerprint) = 32)`, cross-lock with GUARD-7, reader-side validation, documented dim-change migration procedure.

### FINDING-5 ✅
- Section: §3.1 `Entity`
- Change: Promoted `merged_into: Option<Uuid>` and `history: Vec<HistoryEntry>` to typed fields; `insert_entity`/`update_entity_*` reject reserved keys in `attributes`; merge path leaves caller-authored attributes untouched.

### FINDING-6 ✅
- Section: §4.1 mentions-cache note "Divergence detection"
- Change: Write-path debug assertion in `apply_graph_delta`, 1%/hr periodic consistency sampler emitting `ResourcePressure`, opt-in `verify_cache` flag on `entities_linked_to_memory`.

### FINDING-7 ✅
- Section: §4.2 `GraphStore::traverse`
- Change: Required `max_results`, visited-set cycle handling, BFS-by-depth-then-activation-then-recorded_at ordering, `O(max_results)` complexity, `Proposed` predicates rejected from filter.

### FINDING-8 ✅
- Section: §5bis "Crash-recovery & idempotence invariant"
- Change: Stated the `graph_applied_deltas` row is written inside the same SQLite transaction as all data rows (no partial-state possible); added kill-between-commit-and-ack contract test-plan bullet.

### FINDING-9 ✅
- Section: §3.1 `EntityKind::Other(String)` docstring
- Change: Normalized on insert (lowercase, trim, NFKC); two `Other` values compare on normalized form; canonical variant recommended over ad-hoc strings.

### FINDING-10 ✅
- Section: §4.1 "Namespace lifecycle"
- Change: Implicit creation on first write; `GraphStore::list_namespaces()` trait method; no deletion in v0.3 (use per-namespace DB file); cross-namespace edges rejected as `Invariant`.

### FINDING-11 ✅
- Section: §3.2 `Edge` + new `ConfidenceSource` enum
- Change: Added `ConfidenceSource { Recovered, Defaulted, Inferred }` and `Edge.confidence_source` (serde default `Recovered`; `Defaulted` on Migrated edges without recovered confidence).

### FINDING-12 ✅
- Section: §9 GOAL-1.14 note
- Change: Added provisional `classify_layer` formula (pinned→Core; core_strength≥0.7→Core; working_strength≥0.6→Working; else Archived) with determinism/purity/monotonicity guarantees, explicitly refinable by consolidation.

### FINDING-13 ✅
- Section: §4.2 `record_predicate_use` docstring
- Change: Production path batches counts per-transaction in a HashMap with one `UPDATE` per distinct predicate at commit; single-call API retained for tests.

### FINDING-14 ✅
- Section: §4.2 `with_transaction` docstring
- Change: Warning that API is un-audited against GUARDs and permits arbitrary SQL; intended only for test fixtures and v03-migration scripts; production paths must not use it.

### FINDING-15 ✅
- Section: §4.1 `graph_edges.memory_id` FK + note
- Change: FK changed from `ON DELETE SET NULL` to `ON DELETE RESTRICT`; explicit GUARD-3 rationale; `episode_id` documented as secondary audit anchor.

### FINDING-16 ✅
- Section: §4.1 `knowledge_topics.embedding` blob-format note
- Change: Application-side dim validation at write (`blob.len() == current_embedding_dim * 4`) and read (returns `Invariant("knowledge topic embedding dim mismatch")`); migration rewrites blobs on any dim change.

### FINDING-17 ✅
- Section: §4.1 "Trace retention" note
- Change: `EngramConfig.graph.record_traces` gate (default `true`); periodic pruning of succeeded-run traces after 30 days (configurable); failed-run traces retained for post-mortem.

**Total**: 17/17 applied, 0 skipped.
