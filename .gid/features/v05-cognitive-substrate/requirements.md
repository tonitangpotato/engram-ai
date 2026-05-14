# v0.5 Cognitive Substrate — Requirements

> Derived from `design.md`. Each GOAL/GUARD has a one-line acceptance test referenced by the action plan in `design.md` §3.

## Interoception (§2.1)

- **GOAL-I1** Per-domain rolling statistics (mean, std, sample_count, window) live in `nodes.attributes` of `node_kind='interoceptive_domain'`; no separate `anomaly_baselines` table is created.
- **GOAL-I2** Baseline signals (every ingest/recall/action emits one) mutate the domain node in-memory but do **not** produce a write per signal. Only the rolling stats are persisted; raw signal stream never touches disk.
- **GOAL-I3** Anomaly signals (z-score above threshold OR triggering a regulation action) persist as `node_kind='anomaly_event'` rows with attributes `{domain, metric, raw_value, z_score, window_stats_snapshot, triggered_regulation, rationale}` and edges `observed_in_domain` + `triggered_by`.
- **GOAL-I4** Somatic markers form by walking ≥N anomaly_events on a domain that share a pattern signature; a `node_kind='somatic_marker'` is written with `derived_from` edges to the contributing anomaly_events (full audit trail).
- **GOAL-I5** Marker → situation linkage: each `somatic_marker` has `evoked_by` edges (`edge_kind='associative', predicate='evoked_by'`) to the memory/entity nodes that triggered its formation.
- **GUARD-I6** Baseline mutation never produces a write op; verified by writer instrumentation showing 0 writes per baseline signal in a 1000-signal stress test.

## Empathy bus (§2.2)

- **GOAL-E1** Each SOUL.md drive loads on startup as a `node_kind='drive'` row with attributes `{name, weight, embedding, source: 'soul'|'derived', last_reinforced}`.
- **GOAL-E2** Every ingested memory produces a `WriteDriveAlignment` op scoring it against active drives; alignment edges (`edge_kind='associative', predicate='aligns_with'`) persist for scores above threshold.
- **GOAL-E3** Per-domain valence trend lives in the §2.1 interoceptive_domain node's `attributes.valence_window`; there is **no** parallel valence_accumulator table or node kind.
- **GOAL-E4** Each heartbeat action outcome persists as `node_kind='action_outcome'` with edges `triggered_by_drive` + `involves_memory`.
- **GOAL-E5** Every file write by `bus/mod_io.rs` (SOUL.md/HEARTBEAT.md/IDENTITY.md) produces a `node_kind='external_write'` audit node with `target_file` + `content_hash` attributes, written **before** the file mutation.
- **GUARD-E6** Subscription model uses `node_kind='subscription'` nodes and `notifies` edges; no `subscriptions` table is added.

## Working memory (§2.3)

- **GOAL-W1** WM is an in-process `Vec<NodeRef>` (length cap ~32) with recency scores; mutating WM never touches disk in steady state.
- **GOAL-W2** A `wm_snapshot` is written **only** when triggered by a downstream event — either a metacog `feedback_event` (§2.4) or an `anomaly_event` (§2.1). No periodic snapshots, no idle-driven snapshots.
- **GOAL-W3** `wm_snapshot` attributes include `slot_contents`, `slot_scores`, `drive_state`, and `wm_state ∈ {cold_start, warm}`. Edges: `captured_during → <trigger event node>`.
- **GOAL-W4** `WmState` flips from `cold_start` to `warm` on the first of: (a) metacog loop completes one cycle, or (b) a prior-session `wm_snapshot` is loaded back into the ring buffer. Read-only thereafter for session lifetime.
- **GUARD-W5** A `wm_snapshot` and its triggering event commit in the same SQL transaction; verified by injecting a failure between the two writes and asserting the snapshot row does not exist.

## Metacognition (§2.4)

- **GOAL-M1** Each metacog evaluation persists as `node_kind='metacog_feedback'` with attributes `{score, dimension, evaluator, rationale, timestamp}` and an `evaluates` edge to the target node (memory / synthesis / retrieval_trace).
- **GOAL-M2** `dimension` is restricted to the closed set `{recall_accuracy, synthesis_quality, channel_effectiveness, retrieval_relevance}` at the type-system level.
- **GOAL-M3** Aggregate views are derived (single SQL aggregate query); no materialized rollup table unless query latency demands it.
- **GOAL-M4** Each retrieval-plan execution persists as `node_kind='retrieval_trace'`; metacog feedback edges can point at it.
- **GUARD-M5** Low metacog scores in dimension X are visible to interoception (§2.1) so somatic-marker formation can fire on "I tend to be wrong about this kind of question" patterns — closed-loop integration verified by end-to-end test.

## Dimensional signature (§2.5)

- **GOAL-D1** Tier 1 scalar fields (`core_fact`, `valence`, `domain`, `confidence`, `type_weights`) live in `nodes.attributes` of every memory row and are accessible via single-row read.
- **GOAL-D2** Tier 2 narrative fields (`participants`, `temporal`, `location`, `context`, `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`) project to `node_kind='dimension_<field>'` nodes; memory→dimension references are `edge_kind='containment', predicate='describes_<field>'` edges.
- **GOAL-D3** Tier 3 tags project to `node_kind='tag'` nodes; memory→tag is `edge_kind='containment', predicate='tagged'`. Re-ingesting the same tag is a SQL no-op (partial UNIQUE index on `containment` edges enforces).
- **GOAL-D4** `dimension_access.rs` API surface is preserved: `dims.valence()`, `dims.location()`, `dims.tags()` etc. continue to work, backed by the unified schema. No caller code changes required.
- **GOAL-D5** Dimension nodes deduplicate across memories — 40 memories at "Caroline's house" produce 40 edges + 1 dimension node, not 40 strings.
- **GOAL-D6** Write amplification budget: per-memory write op count stays ≤ 2.4× the v0.4 baseline at P50 and ≤ 1.9× at P95, measured by writer instrumentation on a representative 1000-memory ingest stream.
- **GUARD-D7** Resolution-pipeline canonicalization (v04 §4.2) applies to dimension values — `"Caroline's house"` and `"Caroline house"` resolve to the same dimension node.

## Cross-cutting

- **GUARD-X1** All v0.5 features depend on v04 Phase A-D being complete (substrate online + read-switch wired). No v0.5 task may add a legacy-only write path.
- **GUARD-X2** All v0.5 writes go through either the writer-queue (once it lands) or direct SQL with the same dual-write contract as v04 Phase B (which is currently the only contract since the queue is parked).
- **GUARD-X3** Single-namespace only. Cross-namespace cognitive aggregation (e.g. drive_alignment across multiple namespaces) is **out of scope** for v0.5 and listed under design.md §4.
- **GUARD-X4** No new SQL tables. Every v0.5 capability must reduce to new `node_kind` / `edge_kind` values, new attribute fields, or new writer ops. If a capability cannot reduce, escalate to design review before implementing.

## Acceptance gates (per phase)

- **Phase 1 — Interoception** (T45-T48): GOAL-I1..I5, GUARD-I6 all hold; synthetic-spike test passes.
- **Phase 2 — Empathy bus** (T49-T50): GOAL-E1..E5, GUARD-E6 all hold; golden-file replay shows zero event loss vs. pre-migration baseline.
- **Phase 3 — Working memory** (T51-T53): GOAL-W1..W4, GUARD-W5 all hold; 100-mutation test produces exactly one snapshot.
- **Phase 4 — Metacognition** (T54-T55): GOAL-M1..M4, GUARD-M5 all hold; closed-loop test (metacog low score → somatic marker forms) passes.
- **Phase 5 — Dimensional signature** (T56-T59): GOAL-D1..D6, GUARD-D7 all hold; `dimension_access.rs` shim benches within 2× current accessor cost.

## Out of scope

- Writer-queue infrastructure (v04 §8.15, T61-T68). Parked.
- v0.2 KC retirement (v04 §8.14, T60). Belongs to v0.4 cleanup.
- Cross-namespace cognitive aggregation.
- New SQL tables.
- Reverse migrations (rolling back v0.5 nodes/edges into pre-v0.4 schema).
