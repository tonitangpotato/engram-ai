# v0.4 Unified Substrate — Requirements

**Status**: DRAFT — derived from `design.md` 2026-05-14
**Author**: claude (rustclaw session 2026-05-14)
**Derived from**: `design.md` §3, §4, §5, §6 (this document is the testable GOAL-set; design.md is the architectural source-of-truth)

---

## 0. How to read this document

Each `GOAL-N` is a single testable invariant. `GUARD-N` are constraints that must hold across the whole substrate (cross-cutting). `GOAL`s map 1:1 onto tasks in `design.md §8` where possible; `GUARD`s are typically validated by the verifier (T27) and the row-count parity CI test (T17).

The intent is that an outside engineer can read this file in 10 minutes, then audit the implementation against the GOALs without needing to read 2200 lines of design.

---

## 1. Schema (terminal shape)

### GOAL-1.1 — Two-table substrate
The system stores every conceptual unit in `nodes` (TEXT id, node_kind, namespace, attributes JSON, scalar columns for created_at, updated_at, importance, superseded_by, fts_rowid) and every relation in `edges` (TEXT id, src, dst, edge_kind, predicate, weight, confidence, attributes JSON, created_at, updated_at). The full-text index lives in `nodes_fts`; embeddings live in `node_embeddings` (multi-model). All other tables are audit or sidecar.

### GOAL-1.2 — Closed `node_kind` enum
`nodes.node_kind` is one of: `memory | entity | topic | insight | episode | plan`. New kinds require a schema bump + writer-layer code. `Other(String)` exists as a forward-compat escape hatch in the Rust type (`NodeKind::Other`) but is never written by application code.

### GOAL-1.3 — Closed `edge_kind` enum
`edges.edge_kind` is one of: `structural | provenance | associative | containment | causal`. `(edge_kind, predicate)` together form the closed-vocabulary key for the verifier's I1 fingerprint.

### GOAL-1.4 — Audit tables retained
`backfill_runs`, `triple_backfill_checkpoint`, `synthesis_provenance`, `hebbian_links` (during dual-write), `entities` / `memory_entities` / `entity_relations` (during Phase B/C) all retain their independent schemas. They are read-only after Phase E; Phase F drops them.

### GOAL-1.5 — Namespace isolation
Every `nodes` row, every `node_embeddings` row, and every cross-NS read query honors a `namespace` column. Hebbian cross-NS (`signal_source != node.namespace`) is the only legitimate cross-namespace edge writer.

### GOAL-1.6 — Supersession is self-referential on `nodes`
`nodes.superseded_by` is a nullable FK to `nodes.id` in the same row's namespace. `''` (empty string) is the legacy sentinel and must round-trip to SQL `NULL` on dual-write and backfill.

### GOAL-1.7 — `fts_rowid` monotonic
`nodes.fts_rowid` is a monotonic integer allocated from the `fts_rowid_counter` singleton helper. Never reused even after `nodes_fts` row delete.

---

## 2. Writer behaviors

### GOAL-2.1 — Single canonical memory writer
`Storage::add` is the only path that creates a `nodes(kind='memory')` row. `store_raw` flows through `add`. Dual-write extracts the `nodes` row via the single helper `Storage::insert_memory_node_row`, used by both Phase B dual-write (T12) and Phase C backfill (T19).

### GOAL-2.2 — Phase B dual-write contract: every mutator dual-writes
Every public writer on `Storage` that mutates `memories`, `entities`, `entity_relations`, `memory_entities`, `memory_embeddings`, `synthesis_provenance`, or `hebbian_links` also writes the matching `nodes` / `edges` / `node_embeddings` row in the same transaction. The Phase B writer audit (ISS-119–126, closed 2026-05-14) enumerates the covered call sites.

### GOAL-2.3 — UPDATE family dual-writes (ISS-124)
`Storage::update`, `update_content`, `update_importance` all dual-update `nodes` for the corresponding columns. Updates preserve `attributes` JSON shim keys not owned by the writer (the "shim-key preservation" contract).

### GOAL-2.4 — DELETE family dual-DELETEs (ISS-125, ISS-126)
`Storage::delete_embedding` dual-DELETEs `node_embeddings`. `Storage::delete` (hard delete) cascades through `edges` → `node_embeddings` → `nodes` in the correct order for the RESTRICT FK constraint on `nodes`.

### GOAL-2.5 — Hebbian dual-write per signal_source
`record_association`, `record_coactivation_ns`, and `record_cross_namespace_coactivation` all dual-write to `edges(edge_kind='associative', predicate='co_activated')`. The deterministic edge id includes `signal_source` so future row-identity changes (§4.3) are forward-compatible.

### GOAL-2.6 — KC and synthesis dual-write
`KnowledgeCompiler::persist_cluster` writes the topic node via `dual_write_entity_to_nodes` (deriving `node_kind` from `EntityKind`), and the containment via `GraphWrite::upsert_topic_containment`. Synthesis provenance edges are dual-written via `Storage::insert_provenance_edge_row` with `edge_id = legacy.id` pass-through (no hash) so re-emission after backfill collides on PK.

### GOAL-2.7 — Confidence policy is legacy-column-wins
When the legacy table has a `confidence` column (synthesis_provenance, future drivers), dual-write and backfill pass it through unchanged. When the legacy table lacks the column (entity_relations, memory_entities, hebbian_links), the default is `1.0`. The policy string is recorded in `backfill_runs.notes.confidence_policy` for every driver.

---

## 3. Backfill behaviors (Phase C)

### GOAL-3.1 — Every legacy table has a backfill driver
T19 (memories), T20 (memory_embeddings), T21 (entities), T22 (entity_relations), T23 (memory_entities), T24 (hebbian_links), T25 (synthesis_provenance) are all idempotent SQL-set-based drivers. T26a (triples) is the one iterator-state-bearing driver because it calls an external LLM extractor.

### GOAL-3.2 — Drivers are idempotent
Re-invoking any Phase C driver on a fully-backfilled DB produces `rows_skipped_existing = rows_read`, `rows_inserted = 0`. The verifier's I3 invariant asserts this.

### GOAL-3.3 — Drivers preserve byte-equal parity with Phase B dual-write
For every row that could land via either path, the unified-side row produced by Phase C backfill is byte-equal (on scalar fields) to the row produced by Phase B dual-write. Tests assert this directly (e.g. `t19_backfill_byte_equal_with_dual_write`).

### GOAL-3.4 — Audit row per driver invocation
Every driver invocation opens a `backfill_runs` row before starting (`finished_at = NULL`) and updates it on completion. The counter invariant `rows_read = rows_inserted + rows_skipped_existing + rows_failed` is asserted by `BackfillRun::assert_counter_invariant`.

### GOAL-3.5 — Resumability for the triple driver (T26a)
`backfill_triples_from_memories` writes a `triple_backfill_checkpoint` row after every successful memory and resumes from `last_memory_id` on restart. A crashed run leaves `status='in_progress'`; the next invocation picks up the cursor.

### GOAL-3.6 — Verifier covers all 7 drivers
`substrate::verify::verify_phase_c_parity` runs I1 (count parity per driver via `Fingerprint` discriminator), I2 (audit row consistency), I4 (content spot-check via seeded sampling). I3 (idempotency, gated, runs drivers again) is on the `_mut` entry point; the read-only entry point cannot trigger driver re-execution.

---

## 4. Read-switch behaviors (Phase D)

### GOAL-4.1 — Single flag gates all read switches
`StorageConfig::unified_substrate` is the boolean that switches every read path from legacy to unified tables. Default is `false`. T32 flips the default — HARD-GATED, not part of Phase D delivery.

### GOAL-4.2 — Read switches preserve return-type contract
Every read-switched function returns the same Rust type before and after the switch (e.g. `search_fts` still returns `Vec<MemoryRecord>`, even though under flag-on it now JOINs `nodes` → `memories` for the column data).

### GOAL-4.3 — Coverage matrix
Read switches landed for: subscriptions (T29.1), synthesis_provenance (T29.2), embeddings (T29.3), Hebbian (T29.4, 4 readers + cross-axis ISS-118 root fix), entity readers (T29.5, 4 parts), FTS (T29.6 — `memories_fts` → `nodes_fts`). Triple readers and remaining `SELECT FROM memories` reads (T29.7) are deferred to Phase F prep; under T12 dual-write, the legacy table remains the source of truth for unswitched reads.

### GOAL-4.4 — Contract parity tests
Every read-switch ships ≥3 contract tests: parity (flag-off result == flag-on result on a populated DB), namespace-specific, edge cases (deleted, superseded, empty).

---

## 5. Writer queue (per design §6)

### GOAL-5.1 — Single-consumer dedicated thread
All mutations go through a single OS thread that owns the SQLite write side. Readers never block on the writer.

### GOAL-5.2 — Priority lanes
The queue supports HIGH / NORMAL / LOW lanes. User-facing writes (memory ingest, explicit updates) go HIGH; Hebbian coalescing and backfill use LOW.

### GOAL-5.3 — Compound-op atomicity
A `WriteOp::Compound(Vec<WriteOp>)` variant executes its children in a single SQLite transaction. Used for: memory + its embeddings + entity links in one ingest.

### GOAL-5.4 — Throughput ceiling documented
The writer can sustain ≥10k ops/sec on commodity hardware (single SSD, SQLite WAL). Verified by a microbenchmark in `benches/writer_queue.rs` (deliverable for T46).

### GOAL-5.5 — Crash recovery via write journal
On startup, the writer scans `write_journal` for unflushed ops and replays them. The journal is append-only; flushed ops are pruned by a vacuum task.

---

## 6. Cross-cutting GUARDs

### GUARD-1 — Additive-only through Phase E
No legacy table is dropped or renamed until Phase F. Phase A–D migrations are strictly additive: `CREATE TABLE IF NOT EXISTS`, `ALTER TABLE ADD COLUMN`, `CREATE INDEX IF NOT EXISTS`. Existing rows are never mutated by migration code.

### GUARD-2 — Idempotent migrations
Every `migrate_*` function in `storage.rs` is safe to re-run. Re-opening a DB on the same engram version is a no-op. Re-opening a DB on a newer version runs only the new migrations.

### GUARD-3 — Schema version is phased
`engram_meta.schema_version` is a string with phase semantics: `0.4-additive` (Phase A complete), `0.4-dual-write` (Phase B complete), `0.4-unified` (Phase E complete — legacy writes stopped). Tooling splits on `-` for ordering.

### GUARD-4 — No silent data loss
No code path discards a legacy column without explicitly recording the field in the unified-side `attributes` JSON or accepting it as obsolete in a doc comment. The verifier's I4 spot-check parses `attributes` and asserts round-trip for every documented field.

### GUARD-5 — Single source of truth per writer path
Each dual-write helper (`insert_memory_node_row`, `dual_write_entity_to_nodes`, `dual_write_edge_to_edges`, `insert_node_embedding_row`, `insert_structural_edge_row`, `insert_provenance_edge_row`, `insert_associative_edge_row`) is the **only** code that translates a legacy row shape into the unified-side row. Phase B writers and Phase C backfill drivers both call the helper; raw INSERT statements that bypass the helper are bugs (see ISS-127 for an open instance).

### GUARD-6 — Counter invariant per backfill
For every `BackfillRun`: `rows_read = rows_inserted + rows_skipped_existing + rows_failed`. Asserted via `BackfillRun::assert_counter_invariant` on every driver exit.

### GUARD-7 — No live external API calls in test suite
Every backfill driver test injects a mock/noop LLM extractor. The T26a contract tests use `NoopTripleExtractor` and `CountingMockExtractor`. `cargo test` never reaches a real Anthropic or Ollama endpoint.

### GUARD-8 — Production flag default stays opt-in until T33 closes
`StorageConfig::unified_substrate` defaults to `false` until the production observation period (T33, 1 week) completes after T32 flips the default. Pre-T26c backfill, recall under flag-on is degraded for pre-dual-write rows.

---

## 7. Acceptance per phase

- **Phase A complete (T05–T11):** Fresh DB opens with all four unified tables + indexes + FTS triggers. Legacy DB opens without touching old rows. Schema version = `0.4-additive`. — **ACHIEVED 2026-05.**
- **Phase B complete (T12–T18 + ISS-119–126):** Every public mutator on `Storage` dual-writes. Row-count parity CI test green. LoCoMo benchmark unchanged with dual-write enabled. — **ACHIEVED 2026-05-14.**
- **Phase C complete (T19–T26a):** Seven idempotent SQL-set drivers + one resumable LLM driver. Verifier I1/I2/I4 passing on a populated DB. — **ACHIEVED 2026-05-14.**
- **Phase D complete (T28, T29.1–T29.6):** Read switch wired behind `unified_substrate` flag for: subscriptions, synthesis_provenance, embeddings, Hebbian (4 readers), entities (4 readers), FTS. T29.7 deferred. — **ACHIEVED 2026-05-14.**
- **Phase E (T34–T37):** legacy writes removed once T32 flip + T33 observation succeed. — **NOT STARTED, HARD-GATED.**
- **Phase F (T64–T68):** legacy tables dropped. — **NOT STARTED, HARD-GATED.**

---

## 8. Out of scope for v0.4

- Cross-process writer queue (single-binary scope).
- Distributed SQLite (single-node scope).
- Vector index acceleration (linear scan + filter is acceptable through v0.4; vector ANN is v0.5).
- Real-time invalidation across multiple reader processes (single-process scope).
- Triple structure as a structural edge — `triples` table remains source of truth; T29.5 explicitly scopes out triple readers.
