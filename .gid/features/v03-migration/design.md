# Design: Engram v0.3 — Migration

> **Feature:** v03-migration
> **GOAL namespace:** GOAL-4.X
> **Master design:** `docs/DESIGN-v0.3.md` §8 (Migration from v0.2) + §9 Phase 5
> **Requirements:** `.gid/features/v03-migration/requirements.md`
> **Depends on:** `v03-graph-layer/design.md` (schema being migrated *to*), `v03-resolution/design.md` (backfill pipeline reused), `v0.2.2` engramai schema (migrated *from*)
> **Status:** Draft (2026-04-24)

---

## 1. Scope & Non-Scope

This feature owns the **v0.2.2 → v0.3 migration path**: everything needed to take an existing engramai v0.2.2 SQLite database on disk and produce a functioning v0.3 database, safely and resumably. It is a *caller* of v03-graph-layer (uses its schema) and v03-resolution (reuses its pipeline for backfill) — it defines neither.

**In scope.**

- **Pre-migration safety** — full SQLite file backup before any schema change; abort-on-failure; explicit `--no-backup` opt-out with visible warning (GOAL-4.2, GUARD-10).
- **Schema transition** — additive DDL that introduces the v0.3 `graph_*` tables and `knowledge_topics` table without modifying or destroying v0.2 tables (`memories`, `hebbian_links`, v0.2 `entities`, `entity_relations`, `kc_topic_pages`, `kc_compilation_records`, `kc_compilation_sources`). Minimal column additions on `memories` per master DESIGN-v0.3 §8.1 (`episode_id`, `entity_ids`, `edge_ids`, `confidence`).
- **Idempotency** — running `engramai migrate` twice is a no-op; a partially-interrupted migration is resumable (GOAL-4.3).
- **Backfill orchestrator** — drives v03-resolution's pipeline across every existing MemoryRecord to populate `graph_entities` / `graph_edges` / `graph_memory_entity_mentions`, surfacing per-record failures (GOAL-4.4, GUARD-2).
- **Progress API + CLI** — `(processed, total, succeeded, failed)` counters, emitted ≥ every 100 records or 5 s, persisted across process restarts (GOAL-4.5).
- **Topic reconciliation (preserve-plus-resynthesize)** — every `kc_topic_pages` row is carried forward into `knowledge_topics` with `legacy=true` + provenance; re-synthesis from v03-resolution §5bis creates *new* topics alongside, never overwriting (GOAL-4.6).
- **Backward compatibility shim** — `store`, `recall`, `recall_recent`, `recall_with_associations` keep v0.2 signatures and documented behavior on both migrated-v0.2 and fresh-v0.3 databases (GOAL-4.9, GUARD-11).
- **Rollback procedure** — documented, testable `mv` from the pre-migration backup; no in-place schema reversal attempted (GOAL-4.7, GUARD-10).
- **Phased rollout structure** — Phase 0–5 ladder with observable gates; each phase pausable/resumable (GOAL-4.8).
- **Forward-only migration direction** — migration is one-way (v0.2.2 → v0.3). Downgrade (v0.3 → v0.2) is not supported in-place. Rollback requires restoring from the pre-migration backup created in Phase 1 (see §8.3).

**Out of scope — owned elsewhere.**

- **Schema design of new tables** — owned by v03-graph-layer §4. Migration applies the DDL defined there; it does not redesign or extend it.
- **The resolution pipeline itself** — owned by v03-resolution. Backfill invokes `Pipeline::resolve(memory_record)` as a library call; it does not reimplement extraction, fusion, or edge resolution.
- **Retrieval changes post-migration** — owned by v03-retrieval. A migrated database simply becomes the input to the v0.3 read path.
- **Multi-version chains** — only `v0.2.2 → v0.3` is supported. Users on v0.1 or earlier must upgrade through v0.2.2 first.
- **Automatic in-place rollback** — rollback is *restore from backup*. No command attempts to reverse-engineer the schema changes.
- **Online migration with zero downtime for active writes** — migration requires exclusive access to the database file for the schema-transition phase. Backfill is pause/resumable, but exclusive access is required for the DDL step.
- **Fusion weight tuning & benchmarks** — owned by v03-benchmarks (re-running LOCOMO / LongMemEval, grid search, etc.).

**Reader orientation.** §3 is the structural story (phases and gates). §4 is the DDL story. §5 is the backfill mechanics. §6 is topic reconciliation — the one place migration makes a data-model decision (preserve-plus-resynthesize). §7–§8 are the safety/compat stories (contracts we must not break). §9–§11 are surface + errors + tests.

---

## 2. Requirements Coverage

All 9 `GOAL-4.X` requirements and the migration-relevant guards are satisfied. Full traceability table in **§13**. Summary:

| GOAL     | Priority | Satisfied by section(s)            | Notes                                                                                      |
| -------- | -------- | ---------------------------------- | ------------------------------------------------------------------------------------------ |
| GOAL-4.1 | P0       | §4.1, §4.2, §5, §11.1              | Every v0.2 row reachable post-migration; verified by fixture replay test.                  |
| GOAL-4.2 | P0       | §8.1, §8.2, §10.1                  | Backup-before-DDL; abort on I/O failure; `--no-backup` produces a visible warning.         |
| GOAL-4.3 | P0       | §4.3, §4.4, §5.4, §11.2            | Schema handshake + idempotent DDL + resumable backfill checkpoint.                         |
| GOAL-4.4 | P1       | §5.2, §5.3, §10.2                  | Reuses v03-resolution `record_extraction_failure`; per-record retry without re-processing. |
| GOAL-4.5 | P1       | §5.5, §9.2, §9.3                   | `MigrationProgress` struct + emit cadence + persisted counters.                            |
| GOAL-4.6 | P1       | §6.1, §6.2, §6.3, §6.4             | `kc_topic_pages → knowledge_topics` with `legacy=true`; re-synthesis produces new topics.  |
| GOAL-4.7 | P1       | §8.1, §8.3, §8.4, §11.4            | Backup exists + `mv` drill exercised in CI.                                                |
| GOAL-4.8 | P1       | §3.1, §3.2, §3.3                   | Phase 0–5 with gate predicates; checkpoint table persists progress within a phase.         |
| GOAL-4.9 | P0       | §7.1, §7.2, §7.3, §11.5            | Signature-preserving shim + compat matrix + dual fixture suite (v0.2-migrated + fresh).    |

Guard linkage:

- **GUARD-10** (v0.2 survives; backup-before-change; rollback possible) — §4.2, §8.1, §8.3, §8.4.
- **GUARD-2** (no silent degradation) — §5.3 surfaces every backfill failure per-record; §6.4 surfaces topic carry-forward failures.
- **GUARD-1** (episodic completeness) — §4.2 forbids dropping/modifying `memories` or `hebbian_links` columns; only additive columns per master DESIGN §8.1.
- **GUARD-3** (no retroactive silent rewrites) — §5.2 writes only to new `graph_*` tables; existing v0.2 `entities` / `entity_relations` untouched per v03-graph-layer §2.
- **GUARD-9** (no new required external deps) — §9.1 `engramai migrate` is a subcommand of the existing binary; all machinery uses already-present crates (`rusqlite`, `serde`, `tracing`).
- **GUARD-11** (backward compat) — §7 in full.

---

## 3. Phased Rollout

### 3.1 Phase taxonomy

The migration is structured as **six phases** (0–5), ordered. Each phase has a single *responsibility*, a set of *inputs* it assumes from earlier phases, and an *observable gate* that must be satisfied before the next phase may begin. The taxonomy aligns 1:1 with master DESIGN-v0.3 §9 (the project roadmap), but re-interpreted for the migration-tool perspective: here, each "phase" is a step the tool performs on an existing database, not a development milestone.

- **Phase 0 — Pre-flight.** Detect current schema version, verify database is v0.2.2 (reject earlier versions with a clear error), check free disk space ≥ 1.1× current DB size (backup room + safety margin), acquire exclusive file lock. No writes.
- **Phase 1 — Backup.** Write `{db_path}.pre-v03.bak` (full SQLite file copy via `VACUUM INTO`, which guarantees consistency) unless `--no-backup` was passed. Abort migration if backup write fails. On `--no-backup`, emit a `WARN` log + stderr banner: "No pre-migration backup. Rollback will not be possible."
- **Phase 2 — Schema transition.** Execute the additive DDL from §4.2 inside a single `IMMEDIATE` transaction. Record `schema_version = 3` on commit. Pure DDL; no data motion.
- **Phase 3 — Topic carry-forward.** Copy every `kc_topic_pages` row into `knowledge_topics` with `legacy = 1` and a provenance JSON blob pointing to the v0.2 source (§6). This runs before backfill because it's a fast, bounded-size operation and its failures must not be starved by a long backfill.
- **Phase 4 — Backfill.** Iterate `memories` rows, invoke v03-resolution pipeline per record, write resulting entities/edges/mentions into `graph_*` tables. Pausable + resumable (§5.4). Individual record failures are surfaced via `graph_extraction_failures` (§5.3).
- **Phase 5 — Verify + finalize.** Invariant checks: no orphan edges, every memory with `entity_ids != '[]'` has at least one mention row, `schema_version` is 3, topic count matches. On pass, emit `migration_complete = true`. On fail, preserve state and report.

Phases 0–2 run serially with no resumption: if they fail, the tool aborts and the operator must restore from backup (or fix the condition and re-run from Phase 0). Phases 3–5 are *resumable* — progress is persisted in a checkpoint table (§5.4) so a killed/crashed tool can resume exactly where it left off.

### 3.2 Phase gate conditions

Each phase exposes a predicate that returns `true` iff the phase's post-condition holds. The CLI's `--gate <phase>` flag runs the tool up to and including that phase and then stops, letting an operator inspect before advancing. A CI-oriented `--all` flag runs through Phase 5 non-stop.

| Phase | Gate predicate (observable, verifiable post-condition)                                                                                                                            |
| ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0     | `schema_version == 2` AND `free_disk_bytes >= 1.1 * db_file_bytes` AND exclusive lock held.                                                                                       |
| 1     | `exists({db_path}.pre-v03.bak)` AND `sha256({db_path}.pre-v03.bak) != sha256(empty)` AND backup file size ≥ original (SQLite file size). Skipped (but logged) under `--no-backup`. |
| 2     | `schema_version == 3` AND all tables from §4.2 exist AND `PRAGMA integrity_check` returns `ok`.                                                                                   |
| 3     | `count(knowledge_topics WHERE legacy = 1) == count(kc_topic_pages)` OR the difference is accounted for in `graph_extraction_failures` with `stage = 'topic_carry_forward'`.       |
| 4     | `checkpoint.records_processed == count(memories)` AND `records_processed == records_succeeded + records_failed`.                                                                  |
| 5     | All Phase 5 invariant checks return green; `migration_complete` flag persisted.                                                                                                   |

Gate predicates are **not just internal assertions** — each is exposed via `engramai migrate --status` so an operator can inspect the database at any time and know exactly which phase's post-condition holds.

### 3.3 Pause / resume semantics

A phase may be paused in two ways:

1. **Cooperative pause** — the operator sends SIGINT. The current phase finishes its in-flight unit of work (e.g., the current memory record in Phase 4), commits its checkpoint, and exits with code `EXIT_PAUSED = 2`.
2. **Hard interrupt / crash** — the process dies. Any in-flight SQLite transaction rolls back (because we use `IMMEDIATE` transactions). The last committed checkpoint (§5.4) reflects the last successfully persisted record.

On resume (`engramai migrate --resume`), the tool reads `migration_state` (§5.4), determines the last completed phase, and restarts from the *next* phase. Within Phases 4–5, resume re-enters the phase at the checkpointed record and continues. Phases 0–2 are short enough that resume re-runs them from scratch (they are idempotent — backup write skips if file already exists, DDL is `CREATE TABLE IF NOT EXISTS`, schema-version upgrade is conditional).

**Invariant.** A resumed migration produces the same final state as an uninterrupted one. This is enforced by tests (§11.3): run migration, kill at random points, resume, diff against a non-interrupted baseline.

### 3.4 Cross-feature contract (handoff surface)

Migration depends on specific methods from sibling features. This subsection enumerates every handoff — what migration calls, the required signature, idempotency guarantee, and error contract — so the sibling design teams have a crisp surface to formalize as public API. Each entry cites the owning feature design section and is mirrored by a "Handoff request" bullet in §12.

| Handoff target                                       | Required signature                                                                            | Idempotency guarantee                                                                 | Error contract                                                                                 | Owning section              |
| ---------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- | --------------------------- |
| Atomic per-record graph persist                      | `GraphStore::apply_graph_delta(memory_id: &str, delta: &GraphDelta) -> Result<(), GraphError>` | Re-applying the same `(memory_id, delta)` is a no-op (upsert on entities, `INSERT OR IGNORE` on edges/mentions). | Returns `Err(GraphError::Fatal)` only on storage/IO failure; schema violations surface as `GraphError::Data` without partial commit. | v03-graph-layer §5          |
| Backfill pipeline entry point                        | `ResolutionPipeline::resolve_for_backfill(memory: &MemoryRecord) -> Result<GraphDelta, PipelineError>` | Deterministic: same input → same `GraphDelta` (modulo LLM nondeterminism, which is bounded by fusion). Re-running is safe. | `PipelineError::ExtractionFailure(fail)` for per-record failures (data, not control); `PipelineError::Fatal(e)` for storage/IO aborts. | v03-resolution §6           |
| Extraction failure audit write                       | `GraphStore::record_extraction_failure(&ExtractionFailure) -> Result<(), GraphError>`         | Keyed on `(memory_id, stage, kind)`; duplicate inserts are `INSERT OR REPLACE` with `occurred_at` refreshed. | `Err(GraphError::Fatal)` only on storage failure; never fails on duplicate.                    | v03-graph-layer §5          |
| Topic upsert with legacy flag                        | `GraphStore::upsert_topic(topic: &KnowledgeTopic) -> Result<(), GraphError>`                  | Keyed on `topic_id`; `legacy = 1` rows never overwrite `legacy = 0` rows or vice versa (per §6.2 invariant). | `Err(GraphError::Fatal)` on storage; `Err(GraphError::Data)` on `legacy`-flag violation.       | v03-graph-layer §5          |
| Entity kind for topics                               | `EntityKind::Topic` enum variant                                                              | Stable enum value across v0.3.x (part of persisted schema).                           | n/a (type-level)                                                                               | v03-graph-layer §4.1        |
| Failure-kind taxonomy addition                       | `ExtractionStage::TopicCarryForward` enum variant                                             | Stable enum value across v0.3.x.                                                      | n/a (type-level)                                                                               | v03-graph-layer §4.1 (+ migration adds the variant) |
| Pipeline error taxonomy                              | `PipelineError::{ExtractionFailure, Fatal}` variants                                          | Stable across v0.3.x.                                                                 | See row 2.                                                                                     | v03-resolution §8           |

**Change management.** Any change to the signatures, idempotency guarantees, or error contracts in this table requires a coordinated PR across migration + the owning sibling — a sibling-side FINDING will be raised to formalize each row as *public* API (today, several of these are documented as "existing" or "new" but not explicitly marked public). Until those FINDINGs are resolved, this table is the migration-side contract of record; the first implementer must not silently invent alternative signatures.

### 3.5 Migration lock

The design requires exclusive database access during migration (GUARD-10 safety, §1 "Out of scope: online migration"). SQLite's file-level locking serializes writes but does not prevent two migration processes from interleaving: both could open the file, both could pass Phase 0's `schema_version == 2` check, and both could step through phases in an arbitrary order. To prevent this, migration acquires an explicit application-level lock before any work.

**Lock table (created idempotently in Phase 0, before the `schema_version` handshake):**

```sql
CREATE TABLE IF NOT EXISTS migration_lock (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    pid         INTEGER NOT NULL,
    hostname    TEXT    NOT NULL,
    started_at  TEXT    NOT NULL,
    phase       TEXT    NOT NULL,    -- updated at phase transitions
    tool_version TEXT   NOT NULL
);
```

**Acquisition protocol (Phase 0):**

1. `INSERT OR FAIL INTO migration_lock (id, pid, hostname, started_at, phase, tool_version) VALUES (1, ?, ?, ?, 'PreFlight', ?)` — atomic; a second migrator sees `SQLITE_CONSTRAINT` and fails fast.
2. On `SQLITE_CONSTRAINT`, read the existing row and enter **stale-lock detection**:
   - If `hostname` matches the current host **and** `pid` is not a live process (`kill(pid, 0) == ESRCH`), the lock is stale — report it to the operator and exit with `EXIT_LOCK_STALE`, instructing them to pass `--force-unlock` after confirming no other migration is running.
   - If `hostname` differs, we cannot prove liveness remotely — always require `--force-unlock` with an explicit `--i-know-what-im-doing` confirmation.
   - Otherwise (same host, live PID), exit with `EXIT_LOCK_HELD` and print the holder's `pid`, `started_at`, `phase`.
3. `--force-unlock` deletes the row unconditionally and retries the `INSERT OR FAIL`. Use is logged to `migration_state.provenance` for post-hoc audit.

**Release protocol:**

- On **clean exit** (success, `EXIT_GATE_REACHED`, `EXIT_PAUSED`), the row is deleted in the same transaction as the final checkpoint write. A `--resume` run re-acquires a fresh lock.
- On **crash**, the row remains. Stale-lock detection (above) handles recovery.

**Phase updates:** at each phase transition, migration updates `migration_lock.phase` (in the same transaction as the checkpoint advance). This gives `engramai migrate --status` a real-time view of the current phase without having to inspect `migration_state` — and it is also the datum rendered in the `MIG_LOCK_HELD` error message.

**What this does not prevent:** this lock protects against *migration-vs-migration* races, not *migration-vs-normal-traffic* races. Operators must still stop any engramai consumer/writer before migrating; the CLI's preflight banner warns of this. A future v0.3.x may add a broader "engramai write lock" but that's out of scope here.

---

## 4. Schema Transition

### 4.1 v0.2 source inventory

Before migration, the following v0.2.2 tables exist in the source database (per `engramai` v0.2 `storage.rs` and `compiler/storage.rs`):

- **`memories`** — primary MemoryRecord store. Contains content, embedding, metadata JSON, created_at, namespace, etc. **Kept untouched** by migration except for additive columns (§4.2).
- **`hebbian_links`** — associative links between memory IDs. **Kept untouched**; v0.3 may gain an optional `entity_pair` column per master DESIGN §8.1, added as additive DDL.
- **`entities`** (v0.2) — legacy entity table. **Kept untouched, unused by v0.3 read path.** v0.3 introduces a parallel `graph_entities` namespace per v03-graph-layer §2. The v0.2 `entities` table is neither read nor written by v0.3 code; it exists only so v0.2 downgrade paths (via restore-from-backup) still work. **Lifecycle**: `entities` (and the parallel `entity_relations`, `kc_*` tables) are retained through all v0.3.x releases as rollback anchors. They are scheduled for removal in the v0.4.0 migration (which, at that point, will be free of rollback obligations toward v0.2). Size impact — on a 50k-memory DB, the legacy tables typically add <5% to file size — is documented in the operational runbook.
- **`entity_relations`** (v0.2) — legacy entity-relation table. Same treatment as v0.2 `entities`: untouched, unused, parallel.
- **`kc_topic_pages`**, **`kc_compilation_records`**, **`kc_compilation_sources`** — v0.2 Knowledge Compiler tables. `kc_topic_pages` rows are *copied* (not moved) into the new `knowledge_topics` table (§6); the `kc_*` tables themselves remain for audit and rollback-safety.
- **`schema_version`** — single-row table storing the integer schema version. v0.2.2 databases have `schema_version = 2`.

Migration's invariant on v0.2 tables: **no ALTER TABLE that drops or renames columns, no DELETE, no UPDATE on existing rows.** The only modifications permitted are `ALTER TABLE ADD COLUMN` for the additive columns listed in §4.2 — SQLite guarantees these are safe on populated tables.

### 4.2 v0.3 additions (non-destructive)

The complete DDL applied in Phase 2 is enumerated below. *Schema for each new table is owned by v03-graph-layer §4* — this section lists the DDL that must execute and defers the column-by-column definition to the owning design.

**New tables (created via `CREATE TABLE IF NOT EXISTS`, full definitions in v03-graph-layer §4):**

1. `graph_entities` — canonical entity nodes (v03-graph-layer §4.1).
2. `graph_entity_aliases` — canonical↔alias mapping (v03-graph-layer §4.1).
3. `graph_edges` — bi-temporal typed edges (v03-graph-layer §4.1).
4. `graph_predicates` — hybrid-schema predicate registry (v03-graph-layer §4.1).
5. `graph_extraction_failures` — per-record, per-stage failure audit (v03-graph-layer §4.1).
6. `graph_memory_entity_mentions` — memory↔entity provenance bridge (v03-graph-layer §4.1).
7. `knowledge_topics` — L5 topic layer with `legacy` flag (v03-graph-layer §4.1).
8. `episodes` — L1 episode rows (v03-graph-layer §4.1).
9. `affect_mood_history` — per-domain mood history (v03-graph-layer §4.1, master DESIGN §3.7).

**Additive columns on existing tables (via `ALTER TABLE ... ADD COLUMN`):**

- `memories.episode_id INTEGER` — default `NULL`; backfilled in Phase 4 for rows that map to episodes; may remain NULL for v0.2 memories with no episode (audited via extraction-failure table).
- `memories.entity_ids TEXT DEFAULT '[]'` — JSON array of `graph_entities.id` values; populated by backfill.
- `memories.edge_ids TEXT DEFAULT '[]'` — JSON array of `graph_edges.id` values; populated by backfill.
- `memories.confidence REAL` — default `NULL`; optional extraction-confidence per master DESIGN §3.2.
- `hebbian_links.entity_pair TEXT` — default `NULL`; optional entity-pair annotation. Not populated by v0.3.0 migration (deferred to consolidation pass, out of scope here).

**Schema rename (pure, no data change) per master DESIGN §8.1:**

- `entities.valence` → `entities.agent_affect` — *only applied to the v0.2 `entities` table* if the column `valence` exists. This rename is a one-shot `ALTER TABLE RENAME COLUMN` (SQLite ≥ 3.25) executed conditionally. The v0.3 `graph_entities` already uses `agent_affect` (no rename needed).

All DDL in Phase 2 runs inside a single `BEGIN IMMEDIATE ... COMMIT` transaction. If any statement fails, the whole batch rolls back and `schema_version` remains at 2 — the database is unchanged.

### 4.3 `schema_version` pragma + handshake

The `schema_version` table is the migration's source of truth for database version. Protocol:

```sql
CREATE TABLE IF NOT EXISTS schema_version (
    version     INTEGER PRIMARY KEY,
    updated_at  TEXT    NOT NULL
);
```

**Phase 0 handshake:**

1. Read `MAX(version)` from `schema_version`.
2. If the table is missing or empty, inspect DB: if `memories` table exists, assume v0.2.2 (pre-dates schema_version tracking) and insert `(2, <now>)`. Else, assume fresh database — no migration needed, exit cleanly.
3. If `version == 2`, proceed to Phase 1.
4. If `version == 3`, detect mode:
   - If `--resume`: jump to the last-checkpointed phase (§5.4).
   - Otherwise: print "Database is already v0.3. Nothing to do." and exit 0 (idempotency per GOAL-4.3).
5. If `version ∈ {0, 1}` or any other value, abort with `ErrUnsupportedVersion`.

**Phase 2 commit:**

On successful DDL batch, insert `(3, <now>)` into `schema_version`. This single INSERT is the atomic switch from "v0.2 DB" to "v0.3 DB" — any tool that reads `MAX(version)` afterward sees 3.

**Version policy.** `schema_version` is a monotonically-increasing integer. Policy:

- Each minor engramai release (v0.3.0, v0.4.0, …) may introduce at most one `schema_version` bump. Within a minor series (v0.3.0 → v0.3.x), the schema version does not change — migrations that add capabilities without schema changes are handled in-binary without bumping.
- Migrations are **additive, not subtractive**: a new `schema_version` may add tables/columns but must not remove what a previous version required (until a later migration with an explicit deprecation path, per the lifecycle note in §4.1 about v0.2 legacy tables).
- **Skip-version upgrades are not supported.** A user on v0.1 cannot jump to v0.3 directly — they must upgrade through v0.2.2 first (out-of-scope for this feature; see §1). This rule composes cleanly: to go from v0.N to v0.M (M > N+1), run each intermediate migration in order.
- `schema_version = 3` is the v0.3.0 terminal value. v0.3.1+ remains at `schema_version = 3` unless a future schema change is required.

### 4.4 Idempotency proof obligations

Idempotency (GOAL-4.3) is enforced at three layers, each independently provable:

- **DDL layer.** Every `CREATE TABLE` / `CREATE INDEX` uses `IF NOT EXISTS`. Every `ALTER TABLE ADD COLUMN` is wrapped in a guard that first checks `PRAGMA table_info(...)` for the column name. Re-running Phase 2 against a v0.3 DB is a no-op.
- **Data-motion layer.** Phase 3 (topic carry-forward) uses `INSERT OR IGNORE INTO knowledge_topics (topic_id, ...) SELECT ...` keyed on `topic_id` (which is stable: equal to the v0.2 `kc_topic_pages.id`). Phase 4 (backfill) keys on `memories.id` via a checkpoint row (§5.4) — an already-processed memory is skipped.
- **State layer.** `schema_version` and the checkpoint table (§5.4) together guarantee that restarting the tool after any point of failure produces the same final state as a clean run. Tested by the randomized-interrupt suite (§11.3).

The proof that "twice = once" is the union of these three: DDL is self-idempotent, data motion is idempotent by primary-key suppression, and state tracking skips already-done work. No phase depends on "it wasn't run before" for correctness.

---

## 5. Backfill

### 5.1 Orchestrator

The backfill orchestrator drives v03-resolution's pipeline across every MemoryRecord in the source database to produce graph entities, edges, and memory↔entity provenance rows. It is the only part of migration that does non-trivial work; everything else is bookkeeping.

```rust
pub(crate) struct BackfillOrchestrator {
    store: Arc<dyn GraphStore>,               // from v03-graph-layer §5
    pipeline: Arc<ResolutionPipeline>,        // from v03-resolution §3
    checkpoint: CheckpointStore,              // §5.4
    progress_tx: ProgressEmitter,             // §5.5
    cfg: BackfillConfig,
}

pub(crate) struct BackfillConfig {
    pub batch_size: usize,          // default 100
    pub emit_every_records: usize,  // default 100 (GOAL-4.5)
    pub emit_every_duration: Duration, // default 5s (GOAL-4.5)
    pub on_record_failure: FailurePolicy,  // §5.3
}
```

The orchestrator's `run()` method is the Phase-4 entry point. It:

1. Reads `checkpoint.last_processed_memory_id` (`-1` if fresh).
2. Opens a streaming cursor over `SELECT id, content, metadata, created_at FROM memories WHERE id > ? ORDER BY id ASC`.
3. For each record, calls `process_one()` (§5.2), updates the checkpoint, emits progress.
4. On exhaustion, signals Phase-4 gate (§3.2).

The orchestrator never holds more than one `memories` row in memory at a time. Batch boundaries exist only for progress emission — each record is committed independently so a crash does not lose more than the in-flight record.

### 5.2 Per-record pipeline invocation

For each `MemoryRecord`, backfill reuses v03-resolution's pipeline as a library call. This is the single place where the two features compose:

```rust
fn process_one(&self, rec: MemoryRecord) -> Result<RecordOutcome> {
    match self.pipeline.resolve_for_backfill(&rec) {
        Ok(delta) => {
            // Atomic persist: entities + edges + mentions + memory column update.
            self.store.apply_graph_delta(&rec.id, &delta)?;
            self.checkpoint.advance(rec.id, Outcome::Succeeded)?;
            Ok(RecordOutcome::Succeeded { entity_count: delta.entities.len(), edge_count: delta.edges.len() })
        }
        Err(PipelineError::ExtractionFailure(fail)) => {
            // Surface per GOAL-4.4 + GUARD-2; never silent.
            self.store.record_extraction_failure(&fail)?;   // v03-graph-layer §5
            self.checkpoint.advance(rec.id, Outcome::Failed)?;
            Ok(RecordOutcome::Failed { record_id: rec.id, kind: fail.kind, stage: fail.stage })
        }
        Err(PipelineError::Fatal(e)) => {
            // Storage/IO failure — not per-record, cannot be surfaced as extraction failure.
            // Abort backfill; preserve checkpoint; operator must investigate.
            Err(MigrationError::BackfillFatal(e))
        }
    }
}
```

Key contract points:

- **Backfill-specific pipeline entry point.** `resolve_for_backfill` is a variant of the normal `resolve` (v03-resolution §6) that **does not emit an Episode** (L1 episodes are created only for *new* writes; historic memories retain whatever episode mapping master DESIGN §8.1 prescribes — typically NULL until explicit backfill). It also disables the async execution mode (migration always runs synchronously per record for clean checkpoint semantics).
- **Atomic per-record persistence.** `apply_graph_delta` (a new method on `GraphStore` exposed for migration; defined in v03-graph-layer §5) wraps entity upserts, edge inserts, mention rows, and the `memories` row's `entity_ids`/`edge_ids` update in a single SQLite transaction. Either the whole record's graph state lands, or none of it does.
- **No update of v0.2 rows' content or created_at.** Backfill only writes new graph rows and adds the JSON `entity_ids` / `edge_ids` pointers — existing `memories.content`, `memories.embedding`, `memories.metadata`, `memories.created_at` are not touched (GUARD-3).
- **Per-record failures are data, not control flow.** A `RecordOutcome::Failed` is a normal return value. Only true fatal errors (disk full, corrupt SQLite page) propagate as `Err`.

### 5.3 Failure surfacing & retry

Per GOAL-4.4 and GUARD-2, extraction failures during backfill **must be visible per-record**. The mechanism:

- Every per-record failure writes a row to `graph_extraction_failures` (owned by v03-graph-layer §4.1; schema includes `episode_id`, `memory_id`, `stage`, `kind`, `message`, `occurred_at`, `resolved_at`).
- The `MigrationProgress` struct (§9.2) carries `records_failed` as a first-class counter, distinct from `records_succeeded`. The CLI displays `N succeeded, M failed` — an operator can immediately tell "0 failed" from "5 failed."
- Failure rows are **retryable**. The `engramai migrate --retry-failed` subcommand scans `graph_extraction_failures WHERE resolved_at IS NULL` and re-invokes the pipeline on just those memory IDs. Successful retries set `resolved_at` and update checkpoint counters; failures refresh the row with a new `occurred_at`.

**Failure-policy knob.** `BackfillConfig::on_record_failure` controls orchestrator behavior when a per-record failure occurs:

- `FailurePolicy::Continue` (default) — log + record the failure, advance checkpoint, continue to the next record. Optimized for "get as much of the DB migrated as possible, review failures later."
- `FailurePolicy::Stop` — log + record, then stop Phase 4 with `EXIT_FAILURES_PRESENT`. Optimized for "I want a clean migration — stop on the first problem."

The CLI default is `Continue`; operators who want stricter behavior pass `--stop-on-failure`.

### 5.4 Checkpoint + resume

Backfill persists progress in a dedicated table so crashes and cooperative pauses never re-process completed records:

```sql
CREATE TABLE IF NOT EXISTS migration_state (
    id                       INTEGER PRIMARY KEY CHECK (id = 1),
    current_phase            TEXT    NOT NULL,         -- e.g. 'Phase4'
    last_processed_memory_id INTEGER,                  -- -1 sentinel = none yet
    records_processed        INTEGER NOT NULL DEFAULT 0,
    records_succeeded        INTEGER NOT NULL DEFAULT 0,
    records_failed           INTEGER NOT NULL DEFAULT 0,
    started_at               TEXT    NOT NULL,
    updated_at               TEXT    NOT NULL,
    migration_complete       INTEGER NOT NULL DEFAULT 0
);
```

Invariants:

- **Single row.** Enforced by `CHECK (id = 1)`. Migration state is a singleton per database.
- **Monotone counters.** `records_processed`, `records_succeeded`, `records_failed` only increase. `records_processed = records_succeeded + records_failed` is a tested invariant.
- **Advance-after-commit.** The checkpoint is updated *inside the same transaction* as the per-record graph writes in §5.2 — a record is only marked "processed" if its graph delta successfully persisted, preventing lost-record scenarios on crash.

On `--resume`, the orchestrator reads `current_phase`. If it's `Phase4`, it seeks `memories.id > last_processed_memory_id` and continues. The resumed run **does not reset any counters** — they accumulate across restarts, satisfying the "progress survives process restart" clause in GOAL-4.5.

**Phase digests for integrity across pauses.** The `schema_version` row alone cannot detect tampering or partial-write corruption between phases (e.g., migration paused overnight and the DB was opened by an external tool that wrote something, or Phase 3 committed the checkpoint but crashed before fsync of the topic data). To catch these, each completed phase writes a **digest row** into the `migration_state` table that summarizes the content that phase produced:

```sql
CREATE TABLE IF NOT EXISTS migration_phase_digest (
    phase       TEXT    PRIMARY KEY,     -- 'Phase2', 'Phase3', 'Phase4'
    completed_at TEXT   NOT NULL,
    row_counts  TEXT    NOT NULL,        -- JSON: {"graph_entities": 47213, "graph_edges": 102914, ...}
    content_hash TEXT   NOT NULL         -- hex sha256 of a stable canonicalization (see below)
);
```

`content_hash` is computed as `sha256` over a stable serialization of the phase's output, chosen to be cheap and sensitive to the modifications that matter:

- **Phase 2 (schema):** hash of the sorted list of `(table_name, column_name, column_type)` tuples from `PRAGMA table_info` for every `graph_*` + `knowledge_topics` + `episodes` + `affect_mood_history` table.
- **Phase 3 (topics):** `SELECT hex(sha256(group_concat(topic_id || '|' || version || '|' || status, char(30)) ORDER BY topic_id)) FROM knowledge_topics WHERE legacy = 1`.
- **Phase 4 (backfill):** two digests — one over `graph_entities` `(id, canonical_name, kind)` sorted by `id`, and one over `graph_edges` `(id, subject_id, predicate, object_id)` sorted by `id`. Stored as JSON `{"entities": "...", "edges": "..."}` in `content_hash`.

**On resume (or re-run after pause):**

1. For every completed phase (`Phase2`, `Phase3`, …), recompute the digest.
2. Compare to the stored value.
3. On mismatch, abort with `MIG_CHECKPOINT_DIGEST_MISMATCH` (§10.4 error catalog). The operator must either (a) restore from the pre-migration backup and restart, or (b) pass `--ignore-digest-mismatch` after investigating — this flag is documented as an escape hatch for cases where the operator has deliberately modified the DB between phases (rare, generally a mistake).

Digest computation is part of the phase gate predicate (§3.2): the Phase N gate now reads "post-condition holds AND `migration_phase_digest` row present AND recomputation matches." A gate that passes at phase-end but fails on resume is a caught corruption event, not silent propagation.

### 5.5 Progress emission

Progress is emitted on two triggers, whichever fires first (GOAL-4.5):

- **Record-count trigger.** Every `emit_every_records` processed records (default 100).
- **Wall-clock trigger.** Every `emit_every_duration` elapsed since the last emission (default 5 s).

Each emission produces a `MigrationProgress` snapshot (§9.2) and delivers it through two channels:

1. **CLI stdout** — a single-line progress update (or a TTY progress bar if stdout is a tty). Format: `[Phase 4/5] 12340/50000 records | succeeded=12298 failed=42 | 123.4 rec/s`.
2. **Programmatic callback** — a `Box<dyn Fn(&MigrationProgress) + Send>` registered via the library API (`MigrateOptions::on_progress`). Lets embedders pipe progress to their own telemetry/UI.

Emission is best-effort and never blocks backfill: if the callback panics or the stdout pipe is broken, the error is logged once and further emissions on that channel are skipped. Progress emission failures are *never* recorded in `graph_extraction_failures` — they are not data failures.

**Telemetry / progress output format.** The progress surface has three channels, explicitly specified so container and automation wrappers can consume it reliably:

1. **Stderr (default, TTY-detected).** When stderr is a TTY, a one-line progress bar redrawn in place (carriage-return based) with the same `[Phase X/5] …` format as above. When stderr is not a TTY (e.g., piped to a log collector), the same line is emitted once per emission without CR overwrite — safe for append-only logs.
2. **Stdout, `--json` / `--format=json` mode.** Newline-delimited JSON (NDJSON) objects with schema `{ "event": "progress", "phase": "Phase4", "phase_index": 4, "total_phases": 6, "records_processed": 12340, "records_total": 50000, "records_succeeded": 12298, "records_failed": 42, "elapsed_ms": 98712, "rss_mb": 312 }`. One object per emission; the final object has `"event": "complete"` with the full summary from §9.4. In JSON mode, no progress lines are written to stderr (stderr remains reserved for WARN/ERROR-level structured logs).
3. **Persistent `migration_log` table.** Every emission also appends a row to an in-DB table:
   ```sql
   CREATE TABLE IF NOT EXISTS migration_log (
       id          INTEGER PRIMARY KEY AUTOINCREMENT,
       emitted_at  TEXT    NOT NULL,
       phase       TEXT    NOT NULL,
       records_processed INTEGER, records_succeeded INTEGER, records_failed INTEGER,
       rss_mb      REAL,
       message     TEXT
   );
   ```
   This survives process restart and is the post-hoc-analysis surface (`engramai migrate --status --log` prints it). Rows are retained across the whole migration run; the table is truncated only on mid-rollback (§8.5) or manually.

The three channels are populated together; operators and embedders choose which to consume. All three carry the same underlying `MigrationProgress` struct (§9.2) — the table is a projection, the NDJSON is a direct serde serialization, the human line is a format of selected fields.

**Time-to-completion estimation is explicitly out of scope for v0.3.0** (per GOAL-4.5 final sentence). The progress struct carries counts, not ETAs. A future v0.3.x may add ETA based on `records_processed / elapsed_secs`.

### 5.6 Resource model (memory, concurrency, time budget)

Backfill is the dominant resource consumer of migration. Its defaults must be safe on a single-user laptop with a 500k-memory database (potato's working target within the year) and tunable on larger deployments. The following defaults and bounds are the design contract; all are exposed as `BackfillConfig` fields (§5.1 extension) and surfaced as CLI flags.

| Resource                          | Default                          | Tunable (CLI / config)        | Rationale / bound                                                                                             |
| --------------------------------- | -------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------- |
| Batch size                        | 500 records                      | `--batch-size`                | Balances checkpoint frequency (smaller = more fsyncs, more resume-friendly) vs per-batch LLM throughput.      |
| Concurrency                       | `max(1, num_cpus / 2)` workers   | `--concurrency`               | Leaves headroom for the LLM client pool and the SQLite writer. Single-writer SQLite caps useful concurrency. |
| Memory ceiling (soft)             | 512 MiB RSS                      | `--mem-ceiling-mb`            | Checked via `proc_self_status` (Linux) / `task_info` (macOS) between batches; on breach, orchestrator halves concurrency and logs a warning before continuing.  |
| Streaming cursor window           | 1 record at a time               | not tunable                   | Orchestrator holds one `MemoryRecord` at a time (§5.1) regardless of batch size; batch size controls checkpoint cadence, not memory residency. |
| WAL checkpoint cadence            | Every 10 batches (`PRAGMA wal_checkpoint(PASSIVE)`) | `--wal-checkpoint-every`      | Prevents WAL runaway on large DBs; PASSIVE avoids blocking readers.                                           |
| Per-batch wall-clock timeout      | 60 s                             | `--batch-timeout-secs`        | A batch that exceeds this triggers a `WARN` log and a single retry; second timeout = `MIG_BATCH_STUCK` with resumable state.  |
| Total wall-clock budget           | unbounded by default             | `--max-wall-secs`             | Optional guard for CI/automation; on breach, migration pauses cleanly with `EXIT_PAUSED`.                     |

**Memory model.** Backfill is streaming: the orchestrator holds at most one `MemoryRecord`, its `GraphDelta`, and a bounded LRU of recently-seen entity canonicalization candidates (default cap 10k entries, ~80 MiB). There is no "load all memories into RAM" code path. The 512 MiB soft ceiling therefore covers normal operation with healthy headroom; exceeding it indicates either a pathological record (huge `content`) or a leak — both worth alerting on.

**Concurrency model.** Workers are a bounded `rayon`-or-equivalent pool; the SQLite writer is a single logical actor (all `apply_graph_delta` calls funnel through one connection to avoid write contention). Worker count thus sets pipeline-extraction parallelism (LLM-bound), not storage parallelism. This matches the observation that migration is LLM-latency-dominated, not disk-bound, on realistic DBs.

**Stuck-detection.** If a batch exceeds `batch_timeout_secs` twice, migration aborts with `MIG_BATCH_STUCK` (error taxonomy, §10.4 below) and preserves the checkpoint. The operator can `--resume` after investigating (e.g., pipeline call hanging on a specific record; they can then `--retry-failed` that record after it's been flagged via a manually-inserted failure row).

**Interaction with checkpoint digests (§6.X below).** Digest computation is O(table rows) and runs only at phase boundaries, not per-batch; it is not a hot-path cost. On a 500k-memory DB, each digest pass is expected to be <5 s.

---

## 6. Topic Reconciliation

### 6.1 v0.2 → v0.3 topic mapping

v0.2's Knowledge Compiler stores topics in `kc_topic_pages` (schema: `id TEXT PRIMARY KEY, title, content, summary, status, version, quality_score, compilation_count, tags JSON, source_memory_ids JSON, created_at, updated_at`). v0.3 introduces `knowledge_topics` (owned by v03-graph-layer §4.1), which has a slimmer role — it's a bridge from `graph_entities` (where `EntityKind::Topic` lives) to topic metadata, with `namespace`, `legacy` flag, `provenance`, `superseded_by`, and a quality field.

Field-by-field mapping for carry-forward (Phase 3):

| v0.2 `kc_topic_pages`  | v0.3 `knowledge_topics` / `graph_entities`                                | Notes                                                                          |
| ---------------------- | ------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `id` (TEXT)            | `graph_entities.id` (BLOB, deterministic UUID5 derived from `topic_id` string) + `knowledge_topics.topic_id` | Bridge via `EntityKind::Topic` row (see §6.2).    |
| `title`                | `graph_entities.canonical_name`                                           | Stored on the entity side (the entity *is* the topic identity).               |
| `content`              | `knowledge_topics.content`                                                | Full topic body.                                                               |
| `summary`              | `knowledge_topics.summary`                                                |                                                                                |
| `status`               | `knowledge_topics.status`                                                 | v0.2 statuses (`Active`, `Superseded`, etc.) map 1:1 — v0.3 adds no new ones. |
| `version`              | `knowledge_topics.version`                                                |                                                                                |
| `quality_score`        | `knowledge_topics.quality_score`                                          |                                                                                |
| `compilation_count`    | `knowledge_topics.compilation_count`                                      |                                                                                |
| `tags` JSON            | `knowledge_topics.tags` JSON                                              | Preserved verbatim.                                                            |
| `source_memory_ids` JSON | `knowledge_topics.source_memory_ids` JSON                               | Points into `memories.id` — still valid post-migration (memories table untouched). |
| `created_at`, `updated_at` | same                                                                  |                                                                                |

Two rows are written per v0.2 topic:

1. A `graph_entities` row with `kind = EntityKind::Topic`, `canonical_name = title`, `created_at = v0.2 created_at`, `last_seen = v0.2 updated_at`, generated deterministically from the v0.2 topic ID so re-runs produce the same UUID (idempotency).
2. A `knowledge_topics` row keyed by the same ID, with `legacy = 1` and `provenance` populated per §6.2.

Both rows are written in a single transaction per topic; a failure rolls back both.

### 6.2 `legacy=true` + provenance

Every carried-forward topic is marked:

- **`legacy = 1`** — a boolean flag (SQLite `INTEGER NOT NULL DEFAULT 0`) on `knowledge_topics` distinguishing v0.2-migrated topics from v0.3-native (KC-regenerated) ones.
- **`provenance`** — a JSON blob documenting the origin:

  ```json
  {
    "source": "v0.2_kc_topic_pages",
    "v02_topic_id": "<original kc_topic_pages.id>",
    "v02_created_at": "<ISO-8601>",
    "v02_updated_at": "<ISO-8601>",
    "migrated_at": "<ISO-8601>"
  }
  ```

This JSON is stored in the `knowledge_topics.provenance` TEXT column (v03-graph-layer §4.1). It is **not** a free-form audit log — its shape is fixed by §6.2 so tools can parse it reliably. Downstream code that wants to display "imported from v0.2" badges, or that wants to exclude legacy topics from certain queries (e.g., "show me only freshly-synthesized insights"), filters on `WHERE legacy = 1` and inspects `provenance` for context.

**Legacy topics are immutable after Phase 3.** The migration tool is the only code permitted to write rows with `legacy = 1`. v03-resolution's Knowledge Compiler (§5bis) is forbidden from writing `legacy = 1` and must set `legacy = 0` on all topics it creates. This invariant is enforced by a `CHECK` constraint documented in v03-graph-layer §4.1 *or* by a trigger (decision deferred to v03-graph-layer implementation).

### 6.3 Re-synthesis alongside legacy

Post-migration, v03-resolution's Knowledge Compiler (§5bis) may run in the background and produce new topics. Those topics:

- Are written with `legacy = 0` and `provenance = NULL` (or a v0.3-native provenance blob naming the cluster source).
- **Never overwrite or delete** any row with `legacy = 1`. Re-synthesis creates new rows.
- May cite the same underlying `graph_entities` (topic entity) as a legacy topic when KC's clustering identifies the same theme — but each KC output is a separate `knowledge_topics` row.
- May supersede a legacy topic via `superseded_by` (v03-graph-layer §4.1 field on `knowledge_topics`), but **supersession is additive**: the legacy row stays in place with `superseded_by` pointing to the newer row. Reading the legacy row is still valid — consumers just know a newer version exists. This mirrors the edge invalidation pattern from v03-graph-layer §3 (non-destructive UPDATE) and resolution §4 (preserve-plus-resynthesize).

This is the **preserve-plus-resynthesize** contract: legacy topics are forever, newer synthesis is layered on top, and the `legacy` + `superseded_by` flags together tell a consumer "this is the original, and here's what replaced it (if anything)."

### 6.4 Failure policy (GUARD-2 compliance)

A v0.2 topic may fail to carry forward for three reasons:

1. **Corrupt source row.** `kc_topic_pages.content` is NULL, tags JSON is malformed, etc. Non-retryable without manual fix.
2. **Constraint violation on insert.** E.g., `graph_entities.canonical_name` collision with a previously-migrated topic that shares a title. Resolvable by namespace or ID suffix logic (chosen at Phase-3 implementation time).
3. **I/O failure mid-transaction.** Disk full, DB locked. Retryable.

For every failed topic, migration writes a row to `graph_extraction_failures` with:

- `stage = 'topic_carry_forward'`
- `memory_id = NULL` (topics are not per-memory)
- `kind` = one of `{CorruptSource, ConstraintViolation, IoFailure}`
- `message` = human-readable detail including the v0.2 `topic_id`

The Phase-3 gate predicate (§3.2) is satisfied iff `count(knowledge_topics WHERE legacy = 1) + count(graph_extraction_failures WHERE stage = 'topic_carry_forward' AND resolved_at IS NULL) == count(kc_topic_pages)` — i.e., every v0.2 topic is *either* migrated *or* accounted for as a surfaced failure. No topic is silently dropped (GUARD-2).

Retryable failures are cleared via `engramai migrate --retry-failed` (same command as backfill retries — the orchestrator dispatches on `stage`).

---

## 7. Backward Compatibility

### 7.1 Signature preservation

GOAL-4.9 mandates that v0.2 call sites compile against v0.3 without source changes. The four methods in scope:

- `store(content: &str) -> Result<MemoryId>`
- `recall(query: &str) -> Result<Vec<RankedMemory>>`
- `recall_recent(limit: usize) -> Result<Vec<MemoryRecord>>`
- `recall_with_associations(query: &str) -> Result<AssociativeResult>`

These signatures are **frozen for v0.3**. The v0.3 impl block on `Memory` keeps them verbatim, including return types (`MemoryId` type kept, `RankedMemory` fields kept — v0.3 may add new fields only via additive, `#[non_exhaustive]`-compatible patterns, and only if they do not break existing field access patterns in published v0.2 downstream code).

**New v0.3 surface is strictly additive:** `GraphQuery`, `explain`, `store_raw` with `StorageMeta`, etc. are new methods on the same `Memory` type — they do not replace anything.

### 7.2 Behavioral contract table

| Method                      | v0.2 documented behavior                                                             | v0.3 behavior on migrated-v0.2 DB                                                                                         | v0.3 behavior on fresh-v0.3 DB                                                                                  |
| --------------------------- | ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `store(content)`            | Writes a memory row, returns its ID; embedding computed in background.               | Routes through v03-resolution `store_raw` with default `StorageMeta`; v0.2 call sites see the same ID and same timing.     | Same as migrated path — identical behavior.                                                                     |
| `recall(query)`             | Returns vector-ranked memories with ACT-R activation adjustment.                     | Routes through v03-retrieval's default plan (v03-retrieval §4); ranking contract preserved (dot-product vector + ACT-R).   | Same.                                                                                                           |
| `recall_recent(limit)`      | Returns N most-recently-stored memories, newest first.                               | Unchanged — simple time-ordered SELECT on `memories.created_at`.                                                          | Same.                                                                                                           |
| `recall_with_associations`  | Returns memories + their Hebbian-linked neighbors.                                   | Unchanged — reads `hebbian_links` table directly, same query as v0.2.                                                     | Same.                                                                                                           |

**The ranking contract.** For `recall`, "same ranking contract" means: on identical inputs, v0.3 must produce rankings that do not regress against v0.2 on the v0.2 compat-fixture test set (§11.5). New signals (graph-edge distance, affect congruence) may improve rankings but must never *reorder* the top-K in a way that breaks a v0.2 caller's documented expectations. If an improvement would reorder, it must be opt-in via the new `GraphQuery` surface, not the legacy `recall`.

### 7.3 Compatibility test matrix

A dedicated integration suite exercises all four methods against two database states (fresh-v0.3, migrated-v0.2), with assertions on return types, ordering, and side effects. Matrix:

| Method                      | Fresh-v0.3 test       | Migrated-v0.2 test          |
| --------------------------- | --------------------- | --------------------------- |
| `store`                     | `test_store_fresh`    | `test_store_migrated`       |
| `recall`                    | `test_recall_fresh`   | `test_recall_migrated`      |
| `recall_recent`             | `test_recent_fresh`   | `test_recent_migrated`      |
| `recall_with_associations`  | `test_assoc_fresh`    | `test_assoc_migrated`       |

Each row is two test functions (8 total for this matrix). The migrated-v0.2 tests start from a checked-in v0.2 fixture database (§11.1), run `engramai migrate`, and assert. See §11.5 for details.

---

## 8. Rollback & Pre-Migration Safety

### 8.1 Backup write protocol

Phase 1 writes `{db_path}.pre-v03.bak` using SQLite's `VACUUM INTO`, which produces a consistent file-level snapshot even when the source database is being opened (SQLite holds a transactional read to build the copy):

```rust
fn write_backup(db: &Connection, db_path: &Path) -> Result<PathBuf> {
    let backup_path = db_path.with_extension(format!("{}.pre-v03.bak",
        db_path.extension().and_then(|s| s.to_str()).unwrap_or("db")));

    // Refuse to overwrite an existing backup unless --force-backup-overwrite.
    if backup_path.exists() {
        return Err(MigrationError::BackupExists(backup_path));
    }

    // Pre-flight: free disk space ≥ current DB size × 1.1 (already checked in Phase 0,
    // re-checked here because time has passed).
    check_free_space(&backup_path, required_bytes(db)? * 11 / 10)?;

    db.execute_batch(&format!(
        "VACUUM INTO '{}'",
        backup_path.to_string_lossy().replace('\'', "''")
    ))?;

    // Post-condition: file exists, non-empty, openable as SQLite.
    verify_backup_readable(&backup_path)?;

    Ok(backup_path)
}
```

**Post-conditions** (all verified before Phase 1 gate passes):

1. `{db_path}.pre-v03.bak` exists.
2. File size ≥ original database size (SQLite `VACUUM INTO` never produces a smaller file for a non-trivial DB — if it does, that's an error).
3. File is openable as a SQLite database (quick handshake: `PRAGMA integrity_check` returns `ok`).
4. The SHA-256 of the backup's `schema_version` row equals the source's (equivalent-content sanity check).

**On backup failure** — any I/O error, any unmet post-condition — migration aborts with `ErrBackupFailed` and the source database is **untouched** (no DDL has run yet). The abort code is distinct from normal errors so scripts can detect "no migration attempted, safe to retry after fixing disk."

### 8.2 `--no-backup` opt-out

Operators who have external backups (snapshots, ZFS, etc.) may skip the backup step with `--no-backup`. When this flag is passed:

1. Phase 1 is skipped entirely.
2. A `WARN`-level log line is emitted: `no_backup=true, rollback_from_bak_will_not_be_possible`.
3. A banner is written to stderr (not stdout, so pipeline-oriented tools still get clean stdout):
   ```
   ⚠️  engramai migrate: --no-backup is set.
       No pre-migration backup will be written.
       If migration fails or you need to roll back, you must restore from
       an external backup (snapshot, filesystem copy, etc.).
       Press Ctrl-C in the next 5 seconds to abort.
   ```
4. A 5-second grace period (`--no-grace` removes it for CI) lets the operator abort.
5. The `migration_state.provenance` JSON (yes, migration_state gets a provenance blob too) records `no_backup: true, warned_at: <ts>`.

This flag is **opt-out, not default.** The design deliberately makes "no backup" loud: it's a foot-gun and the CLI's job is to make sure the operator knows it's loaded.

### 8.3 Manual rollback procedure

Rollback from v0.3 to v0.2 is **restore from the backup file.** There is no in-place "undo migration" command (per "Out of Scope" in requirements). The documented procedure:

```bash
# 1. Stop engramai (whatever is keeping the DB open).
$ systemctl stop my-engram-consumer       # or equivalent

# 2. Verify the backup exists and is readable.
$ sqlite3 /var/lib/engramai/data.db.pre-v03.bak "PRAGMA integrity_check;"
ok

# 3. Replace the live DB with the backup.
$ mv /var/lib/engramai/data.db /var/lib/engramai/data.db.v03-failed
$ cp /var/lib/engramai/data.db.pre-v03.bak /var/lib/engramai/data.db

# 4. Verify schema_version is back to 2.
$ sqlite3 /var/lib/engramai/data.db "SELECT MAX(version) FROM schema_version;"
2

# 5. Restart engramai on the previous v0.2.x binary.
$ systemctl start my-engram-consumer
```

This procedure is:

- **Documented in-tree** at `docs/migration-rollback.md` (written as part of Phase 5 of the v0.3.0 release), linked from the CLI output (`engramai migrate --help` includes the URL).
- **Executable as a script** — `scripts/rollback-from-backup.sh` ships in the repo and is the body of the CI drill (§8.4).
- **Idempotent** — re-running the script with a rollback already applied is a no-op (detected via `schema_version == 2`).

**Why not an `engramai migrate --rollback` command?** Two reasons:

1. Rollback needs to happen while the process that would run such a command is *not* running (exclusive DB lock). A script that operates at the filesystem level is the right abstraction.
2. An in-tool rollback would need to reverse DDL, which SQLite does not support cleanly (can't drop a column that has data; can drop tables but that's just deleting). The backup-restore approach sidesteps the need.

### 8.4 Rollback testability

Per GOAL-4.7 ("testable — can be exercised in CI"), a rollback drill runs on every CI build:

1. Create a v0.2 fixture database (see §11.1).
2. Run `engramai migrate` through Phase 5 (full migration).
3. Assert: `schema_version == 3`, backfill counters correct, topic count correct.
4. Run `scripts/rollback-from-backup.sh` against the migrated DB.
5. Assert: `schema_version == 2`, DB hash equals the original fixture hash (modulo SQLite page metadata — we hash the content, not the file).
6. Open the rolled-back DB with a v0.2.2 reader harness and verify a canonical `recall` query returns the same results as on the pristine fixture.

Step 6 is the strongest test: it proves not just that the file looks identical but that v0.2 code actually reads the rolled-back DB correctly. Any future change that breaks this test blocks the merge.

### 8.5 Mid-migration rollback (partial-state recovery)

§8.3 and §8.4 cover full rollback from the pre-migration backup. This subsection handles the **middle case**: the operator has started migration but wants to roll back before it completes — e.g., Phase 4 crashed at batch 237 of 500, or the operator lost patience, or a bug was discovered that requires re-running from scratch.

**Rollback feasibility by phase reached:**

| Last phase entered | Rollback path                                             | Safe without backup? |
| ------------------ | --------------------------------------------------------- | -------------------- |
| Phase 0 (preflight)| No DB change; just exit. Drop `migration_lock` row.       | Yes (trivial).       |
| Phase 1 (backup)   | Delete `.pre-v03.bak` if desired; exit.                   | Yes.                 |
| Phase 2 (DDL)      | `mid-rollback` recipe below — drops new tables, reverts `schema_version` to 2, removes additive columns via table-swap if the operator wants a pristine v0.2 DB. | Yes.                 |
| Phase 3 (topics)   | `mid-rollback` recipe — also truncates `knowledge_topics`. | Yes.                 |
| Phase 4 (backfill) | `mid-rollback` recipe — also truncates `graph_entities`, `graph_edges`, `graph_memory_entity_mentions`, `graph_extraction_failures`; clears `memories.entity_ids` / `edge_ids` / `episode_id` / `confidence` back to their defaults. | Yes.                 |
| Phase 5 (verify)   | Phase 5 is read-only. Same as Phase 4 rollback.           | Yes.                 |
| **After Phase 5 completes** (migration_complete = 1) | **Full rollback from backup required** (§8.3). v0.2 behavioral compatibility is preserved, but reverting the DB shape requires the backup. | **No — backup needed.** |

**`engramai migrate --mid-rollback` recipe** (applied in a single `IMMEDIATE` transaction):

1. Assert `migration_state.migration_complete == 0`. If 1, abort with `MIG_ROLLBACK_COMPLETED_MIGRATION` and direct the operator to §8.3.
2. `DELETE FROM graph_entities; DELETE FROM graph_entity_aliases; DELETE FROM graph_edges; DELETE FROM graph_predicates; DELETE FROM graph_memory_entity_mentions; DELETE FROM graph_extraction_failures; DELETE FROM knowledge_topics; DELETE FROM episodes; DELETE FROM affect_mood_history;` — all idempotent, none read by v0.2 code paths.
3. `UPDATE memories SET entity_ids = '[]', edge_ids = '[]', episode_id = NULL, confidence = NULL;` — reverts the four additive columns to their default values. The columns themselves remain (SQLite cannot drop columns cleanly; `ALTER TABLE DROP COLUMN` exists but requires table rebuild). This is acceptable: v0.2 code does not read these columns, so their presence is invisible.
4. `DELETE FROM migration_state; DELETE FROM migration_phase_digest; DELETE FROM migration_lock;`
5. `UPDATE schema_version SET version = 2 WHERE version = 3;` (and delete any `version = 3` row if the single-row schema was violated).
6. Commit.

Post-recipe, the DB is structurally v0.3 (new tables and columns exist but are empty/default) and behaviorally v0.2 (v0.3 read paths find no graph data; v0.2 read paths see the original `memories` and `hebbian_links` unchanged). The operator can either re-run `engramai migrate` or, if they want the file to look pristine, restore from the backup per §8.3.

**Boundary with §8.3.** The design makes the boundary explicit: once Phase 5 has set `migration_complete = 1`, the mid-rollback recipe is disabled. Rolling back a completed migration is a backup-restore operation, not a table-truncation operation — because a completed migration is the supported-for-production state, and reverting it semantically means "undo the schema version," which is §8.3's job.

### 8.6 Full rollback checklist (restore from backup)

The procedure in §8.3 assumes a sophisticated operator. For direct use in runbooks and incident response, here is a 5-step checklist mirroring exactly what the §8.4 CI drill runs — copy-pastable, with the commands the operator should run and the assertions they should verify.

```
╔══════════════════════════════════════════════════════════════════════════╗
║ FULL ROLLBACK FROM PRE-MIGRATION BACKUP                                  ║
╚══════════════════════════════════════════════════════════════════════════╝

Step 1 — Stop all engramai processes holding the DB.
  $ pkill -f engramai        # or: systemctl stop <your-service>
  Verify no processes remain:
  $ lsof /var/lib/engramai/data.db    # should return nothing

Step 2 — Move aside the migrated (or partially-migrated) DB.
  $ mv /var/lib/engramai/data.db         /var/lib/engramai/data.db.v03-failed
  $ mv /var/lib/engramai/data.db-wal     /var/lib/engramai/data.db-wal.v03-failed 2>/dev/null || true
  $ mv /var/lib/engramai/data.db-shm     /var/lib/engramai/data.db-shm.v03-failed 2>/dev/null || true

Step 3 — Verify the backup and copy it into place.
  $ sqlite3 /var/lib/engramai/data.db.pre-v03.bak "PRAGMA integrity_check;"
  → expect: ok
  $ cp /var/lib/engramai/data.db.pre-v03.bak /var/lib/engramai/data.db

Step 4 — Verify schema_version is 2 and the v0.2 tables look right.
  $ sqlite3 /var/lib/engramai/data.db "SELECT MAX(version) FROM schema_version;"
  → expect: 2
  $ sqlite3 /var/lib/engramai/data.db "SELECT COUNT(*) FROM memories;"
  → expect: same count as pre-migration

Step 5 — Restart engramai on the v0.2.x binary (not v0.3.x).
  $ systemctl start <your-service>   # running the pre-upgrade binary
  Verify normal recall works against the restored DB.
```

The script `scripts/rollback-from-backup.sh` implements exactly this checklist; it is idempotent and is the body of the §8.4 CI drill.

---

## 9. CLI & Progress API

### 9.1 `engramai migrate` command surface

The migration tool is a subcommand of the existing `engramai` binary (GUARD-9 — no new external deps, no new binary):

```
engramai migrate [OPTIONS]

OPTIONS:
  --db <PATH>                  Path to the SQLite database (required)
  --no-backup                  Skip pre-migration backup (see warnings)
  --no-grace                   Skip the 5-second grace period after --no-backup banner
  --accept-forward-only        Acknowledge migration is not in-place reversible
  --resume                     Resume an interrupted migration
  --retry-failed               Re-process records in graph_extraction_failures
  --stop-on-failure            Abort Phase 4 on first per-record failure (default: continue)
  --gate <PHASE>               Run up to and including PHASE (0–5) then stop
  --status                     Print current migration status and exit
  --dry-run                    Print what would be done, execute nothing (see §9.1a for per-phase semantics)
  -v, --verbose                Increase log verbosity (repeatable)
```

**§9.1a Dry-run depth (per-phase semantics).** `--dry-run` is the operator's main safety net; its depth must be explicit so "it passed dry-run" gives calibrated confidence. For each phase, dry-run performs one of three depths:

| Phase | Dry-run depth                                         | What it catches                                                                 | What it does NOT catch                                        |
| ----- | ----------------------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| 0 Pre-flight | **Full** — runs all checks read-only             | Wrong `schema_version`, insufficient disk, stale lock, corrupt source DB        | n/a (this phase is inherently read-only)                      |
| 1 Backup     | **Plan-only** — checks write permission at `.bak` path, does not write | Permission errors, wrong path                                                   | I/O errors mid-write, disk full during backup                 |
| 2 Schema DDL | **Against `:memory:` replica** — full DDL batch executed on a fresh in-memory DB seeded with `sqlite3 source.db .schema` and `.dump` | SQL syntax errors, constraint violations, missing prerequisite tables | Disk-specific failures (fsync, FS full) on the real file      |
| 3 Topics     | **Read-only on source** — runs the `INSERT OR IGNORE … SELECT …` as a plain `SELECT` and counts projected rows; reports legacy-flag distribution that *would* result | Topic-schema mismatches, NULL violations, duplicate-key clashes                 | Write-side constraint violations that only trigger under real INSERT (rare — we plan these out) |
| 4 Backfill   | **Sampled (configurable)** — runs `resolve_for_backfill` on a `--dry-run-sample N` (default 50, `N=0` disables) randomly-selected `memories` rows, reports per-sample outcomes, does not write any graph rows | Pipeline explosions on representative records, LLM-call failures, schema-delta mismatches          | Per-record failures on non-sampled rows (statistical — caller should size `N` to their risk appetite) |
| 5 Verify     | **Full** — re-runs all gate predicates as read-only queries against the (un-modified) source | n/a (Phase 5 is inherently read-only)                                           | n/a                                                          |

Dry-run always exits cleanly after Phase 5's checks, writes a `--format=json` summary with a top-level `"dry_run": true` flag, and never modifies the source DB, the backup file, or the lock table (Phase 0's lock acquisition uses `SELECT` only during dry-run and does not insert). Its final exit code is 0 on pass, `EXIT_DRY_RUN_WOULD_FAIL` on projected failure; it never returns `EXIT_PAUSED` or `EXIT_FAILURES_PRESENT` (those codes are reserved for live runs).

**Known-not-caught failures** (surface explicitly in the dry-run report footer): transient disk failures, LLM-call failures on non-sampled records, races with external writers. Operators are instructed that "dry-run OK" is a strong but not complete signal.

Exit codes:

| Code | Meaning                                                                                 |
| ---- | --------------------------------------------------------------------------------------- |
| 0    | Migration completed successfully; DB is v0.3.                                           |
| 1    | Generic failure (unexpected error, stack trace logged).                                 |
| 2    | `EXIT_PAUSED` — cooperative pause (SIGINT); resume with `--resume`.                     |
| 3    | `EXIT_FAILURES_PRESENT` — Phase 4 completed but `records_failed > 0` (with `--stop-on-failure`, this is raised as soon as the first failure occurs). |
| 4    | `EXIT_UNSUPPORTED_VERSION` — source DB is not v0.2.2.                                   |
| 5    | `EXIT_BACKUP_FAILED` — Phase 1 could not write the backup.                              |
| 6    | `EXIT_GATE_REACHED` — stopped cleanly at `--gate` boundary.                             |

`--status` is the introspection entry point. It prints:

```
engramai migrate --status
  Database:           /var/lib/engramai/data.db
  Schema version:     3 (migration in progress)
  Current phase:      Phase4 (backfill)
  Backup:             /var/lib/engramai/data.db.pre-v03.bak (exists, 142.3 MiB)
  Progress:           12340 / 50000 records processed
                      12298 succeeded, 42 failed (retryable: 42)
  Started:            2026-04-24T14:02:11Z (3h14m ago)
  Last progress at:   2026-04-24T17:14:07Z
  Pause state:        running
  Gate status:        Phase 0 ✓ | Phase 1 ✓ | Phase 2 ✓ | Phase 3 ✓ | Phase 4 … | Phase 5 pending
```

### 9.2 Progress API struct

The library-facing progress struct:

```rust
#[derive(Debug, Clone)]
pub struct MigrationProgress {
    /// Currently-executing phase.
    pub phase: MigrationPhase,
    /// Total records in the source DB (computed once at Phase 4 start).
    pub records_total: u64,
    /// Records the pipeline has attempted.
    pub records_processed: u64,
    /// Records where the pipeline produced a graph delta and persisted it.
    pub records_succeeded: u64,
    /// Records that surfaced an extraction failure.
    pub records_failed: u64,
    /// When the overall migration started (persists across --resume).
    pub started_at: DateTime<Utc>,
    /// When this snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// True iff Phase 5 gate passed.
    pub migration_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationPhase {
    PreFlight,          // Phase 0
    Backup,             // Phase 1
    SchemaTransition,   // Phase 2
    TopicCarryForward,  // Phase 3
    Backfill,           // Phase 4
    Verify,             // Phase 5
    Complete,
}
```

**Invariants on `MigrationProgress`** (asserted in tests):

- `records_processed == records_succeeded + records_failed` always.
- `records_processed <= records_total` always.
- `records_total` is stable once set (Phase-4 start); it does not grow mid-backfill even if new memories are written during the run (which is disallowed by the exclusive lock, but the invariant is defensive).
- `started_at` only changes on a fresh (non-resume) run.

### 9.3 Output channels (stdout + callback)

Two progress sinks:

- **CLI stdout.** The binary formats `MigrationProgress` into either a single-line summary (non-tty) or a dynamic progress bar (tty, using the `indicatif` crate already in the engramai dep tree — no new dep).
- **Programmatic callback.** The library API for embedders:

  ```rust
  pub struct MigrateOptions {
      pub no_backup: bool,
      pub resume: bool,
      pub stop_on_failure: bool,
      pub gate: Option<MigrationPhase>,
      pub on_progress: Option<Arc<dyn Fn(&MigrationProgress) + Send + Sync>>,
  }

  pub fn migrate(db_path: &Path, opts: MigrateOptions) -> Result<MigrationReport, MigrationError>;
  ```

  The callback is invoked on the emission cadence from §5.5. Callback panics are caught (`catch_unwind`) and downgraded to log warnings so a buggy embedder cannot crash migration.

Both channels see the same `MigrationProgress`. The CLI uses the callback mechanism internally (the default `on_progress` is a closure that updates the indicatif bar) — this guarantees parity between CLI output and library embedders.

`MigrationReport` (returned from `migrate()`):

```rust
pub struct MigrationReport {
    pub final_progress: MigrationProgress,
    pub backup_path: Option<PathBuf>,
    pub duration: Duration,
    pub phases_completed: Vec<MigrationPhase>,
    pub topic_carry_forward: TopicCarryForwardReport,  // per §6
}
```

### 9.4 Structured output — `--format=json` (r3, benchmarks handoff)

**Motivation (v03-benchmarks §12).** The integrity harness (benchmarks §6) parses migration's final report to assert pre/post-migration counts (records / entities / edges / topics, legacy-flag distribution, extraction-failure count). Parsing ad-hoc human-readable CLI output is fragile; `--format=json` is the stable machine-readable surface benchmarks consume.

```
engramai migrate --format=<FORMAT>

FORMAT:
  human   (default) — the human-readable progress + summary shown in §9.1
  json    — single JSON object written to stdout at migration completion
            (and on --status, with current snapshot)
```

**JSON schema (stable under r3; breaking changes bump the top-level `schema_version`):**

```json
{
  "schema_version": "1.0",
  "tool_version": "<engramai crate version>",
  "db_path": "/var/lib/engramai/data.db",
  "started_at": "2026-04-24T14:02:11Z",
  "completed_at": "2026-04-24T17:14:07Z",
  "duration_secs": 11516,
  "migration_complete": true,
  "final_phase": "Complete",
  "phases_completed": ["PreFlight", "Backup", "SchemaTransition",
                      "TopicCarryForward", "Backfill", "Verify"],
  "backup_path": "/var/lib/engramai/data.db.pre-v03.bak",
  "counts": {
    "pre": {
      "memories": 50000,
      "kc_topic_pages": 312,
      "entities": 0,
      "edges": 0,
      "knowledge_topics": 0
    },
    "post": {
      "memories": 50000,
      "entities": 47213,
      "edges": 102914,
      "knowledge_topics": 312,
      "knowledge_topics_legacy": 312,
      "knowledge_topics_synthesized": 0,
      "graph_memory_entity_mentions": 184022,
      "graph_extraction_failures": 42
    }
  },
  "backfill": {
    "records_total": 50000,
    "records_processed": 50000,
    "records_succeeded": 49958,
    "records_failed": 42,
    "records_failed_retryable": 42,
    "records_failed_permanent": 0
  },
  "topic_carry_forward": {
    "source_rows": 312,
    "carried_forward": 312,
    "skipped_corrupt": 0,
    "legacy_flag_set": 312
  },
  "warnings": [],
  "errors": []
}
```

**Contract with benchmarks (GOAL-11 integrity gate handoff):**
- `counts.pre` / `counts.post` field names are the public contract. Additions are backward-compatible; renames require `schema_version` bump.
- `records_succeeded + records_failed == records_processed` always. Benchmarks asserts this invariant as a smoke test before running the full integrity comparison.
- `knowledge_topics_legacy + knowledge_topics_synthesized == knowledge_topics` always (partition property).
- `--format=json` never interleaves progress updates on stdout. Progress still goes to stderr (TTY-detected) or is suppressed in JSON mode; only the final object is written to stdout. Benchmarks relies on this for safe parsing via `serde_json::from_reader`.
- `--status --format=json` is supported and emits the same schema with `migration_complete: false` and phases in progress.

**Why this lives in migration (not benchmarks).** Benchmarks is a consumer; migration is the producer of the counts. Putting the schema definition here keeps a single source of truth and lets migration's own tests assert the schema is emitted correctly without a cross-feature test dependency.

---

## 10. Error Model

### 10.1 Abort classes

Errors that cause the entire migration to abort (no further phases attempted), mapped to exit codes (§9.1):

- **`ErrUnsupportedVersion`** (exit 4) — source DB is not v0.2.2 (older or newer than supported).
- **`ErrBackupFailed`** (exit 5) — Phase 1 backup could not be written / verified.
- **`ErrDdlFailed`** — Phase 2 DDL batch failed; the IMMEDIATE transaction rolls back, leaving the DB in its pre-Phase-2 state.
- **`ErrInvariantViolated`** — Phase 5 verification failed. The DB is still v0.3 (DDL applied, backfill done) but a structural invariant is wrong. Operator must investigate; do not clear this by re-running.
- **`ErrCorruptSource`** — source database fails `PRAGMA integrity_check` before any write attempt (Phase 0).
- **`BackfillFatal(io_or_storage_error)`** — a non-per-record fatal error during Phase 4 (e.g., disk full, DB file became read-only). Checkpoint is preserved; operator can resume after fixing.

Each abort class is a distinct `MigrationError` variant carrying context (file path, row ID, inner error chain). The CLI prints the variant name + chained causes; the library returns `Err(MigrationError::...)` for the caller to pattern-match.

### 10.2 Per-record failure class

Per-record failures during Phase 4 (backfill) or Phase 3 (topic carry-forward) are **not** aborts — they are data surfaced via `graph_extraction_failures`:

```rust
pub struct ExtractionFailure {
    pub memory_id: Option<MemoryId>,   // None for topic-stage failures
    pub stage: ExtractionStage,         // Ingest | Extract | Resolve | Persist | TopicCarryForward
    pub kind: FailureKind,              // EmptyContent | LlmTimeout | SchemaViolation | IoError | CorruptSource | ConstraintViolation | ...
    pub message: String,
    pub occurred_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}
```

The `stage` and `kind` enums are owned by v03-graph-layer §4.1 (in the `graph_extraction_failures` table definition) and v03-resolution §8 (in the pipeline error taxonomy). Migration adds `ExtractionStage::TopicCarryForward` as the only new variant specific to this feature.

### 10.3 Recoverability table

| Error                            | Class          | Recovery action                                                                          |
| -------------------------------- | -------------- | ---------------------------------------------------------------------------------------- |
| `ErrUnsupportedVersion`          | abort          | Upgrade source to v0.2.2 first.                                                          |
| `ErrBackupFailed`                | abort          | Free disk space or fix permissions; re-run. Source DB unchanged.                         |
| `ErrDdlFailed`                   | abort          | Inspect SQLite log; typically a schema collision from partially-applied prior migration. `--resume` handles most cases. |
| `ErrInvariantViolated`           | abort          | File a bug; restore from backup; do not continue.                                        |
| `ErrCorruptSource`               | abort          | Repair source DB (SQLite `.recover`); re-run.                                            |
| `BackfillFatal`                  | abort w/ state | Fix underlying issue (disk, permissions); run `--resume` to continue from checkpoint.    |
| Per-record `ExtractionFailure`   | data           | `engramai migrate --retry-failed` after investigation or source fixup.                   |
| SIGINT                           | pause w/ state | `engramai migrate --resume`.                                                              |

### 10.4 Error catalog (user-facing taxonomy)

Every failure surfaced by the CLI maps to a stable `(exit_code, error_tag, user_action, is_resumable)` tuple. The catalog is the operator-facing contract: CLI messages quote the `error_tag` verbatim, issue templates pre-fill it, and the operational runbook is indexed by it. Adding new rows is a minor-version change; renumbering or renaming is a major-version change.

| Exit | Error tag                             | Condition                                                                              | User action                                                                                                     | Resumable? |
| ---- | ------------------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ---------- |
| 0    | —                                     | Success.                                                                               | None.                                                                                                           | n/a        |
| 1    | `MIG_INTERNAL_ERROR`                  | Unexpected error (bug). Stack trace logged.                                            | File an issue with the printed report + log excerpt.                                                            | Maybe      |
| 2    | `MIG_PAUSED`                          | Cooperative SIGINT pause.                                                              | `engramai migrate --resume` when ready.                                                                         | Yes        |
| 3    | `MIG_FAILURES_PRESENT`                | Phase 4 finished with `records_failed > 0`.                                            | Inspect `graph_extraction_failures`; run `engramai migrate --retry-failed` after investigation.                 | Partial (retry-failed) |
| 4    | `MIG_UNSUPPORTED_VERSION`             | Source DB `schema_version` is not 2.                                                   | Upgrade source to v0.2.2 first; `engramai migrate` does not support multi-hop chains.                           | No         |
| 5    | `MIG_BACKUP_FAILED`                   | Phase 1 could not write `.pre-v03.bak`.                                                | Free disk / fix permissions; re-run. Source DB is untouched.                                                    | Yes (re-run from scratch) |
| 6    | `MIG_GATE_REACHED`                    | Stopped at `--gate <phase>` as requested.                                              | Inspect DB; re-run without `--gate` or with a later `--gate` to continue.                                       | Yes        |
| 7    | `MIG_LOCK_HELD`                       | `migration_lock` held by a live PID on this host.                                      | Wait for the other migration, or stop it. Example: `Error MIG_LOCK_HELD: another migration is running (pid 1234).` | No (must wait) |
| 8    | `MIG_LOCK_STALE`                      | `migration_lock` held by a dead PID (same host) or foreign host.                       | Verify no other migration is running, then `engramai migrate --force-unlock`.                                   | Yes (after --force-unlock) |
| 9    | `MIG_CHECKPOINT_DIGEST_MISMATCH`      | A completed phase's recomputed digest differs from the stored one (§5.4).              | Investigate; restore from backup (§8.6) is the safe default. `--ignore-digest-mismatch` exists as an escape hatch. | Yes (with flag) |
| 10   | `MIG_DISK_FULL`                       | Write failure due to insufficient disk space (backfill, WAL, or DDL).                  | Free disk (target: ≥ 1.1× DB size free); `engramai migrate --resume`.                                           | Yes        |
| 11   | `MIG_CORRUPT_SOURCE`                  | `PRAGMA integrity_check` failed on source DB before any write.                         | Run SQLite `.recover`; re-run migration.                                                                        | Yes (after repair) |
| 12   | `MIG_BATCH_STUCK`                     | Backfill batch exceeded `--batch-timeout-secs` twice.                                  | Inspect the last-attempted `memories.id`; re-run with `--retry-failed` or skip the record manually.             | Yes        |
| 13   | `MIG_ROLLBACK_COMPLETED_MIGRATION`    | `--mid-rollback` was invoked on a DB where `migration_complete = 1`.                   | Use full rollback from backup instead (§8.6).                                                                   | n/a        |
| 14   | `MIG_DRY_RUN_WOULD_FAIL`              | Dry-run projected a failure (constraint, schema, sampled-backfill failure).            | Read dry-run report; fix the projected cause; re-run dry-run.                                                   | n/a        |

**Message format.** The CLI always prints `Error <TAG>: <human message>` followed by `Suggested action: <action from table>` and, if applicable, `Resumable: yes — run 'engramai migrate --resume' after fixing the cause.` This format is also emitted as a structured object on the NDJSON channel: `{"event": "error", "tag": "MIG_LOCK_HELD", "exit_code": 7, "resumable": false, "message": "...", "suggested_action": "..."}`.

**Stability contract.** Exit codes and error tags are stable within a minor-version series. Automation may key on either; the tag is recommended (more expressive, survives exit-code renumbering in future majors).

---

## 11. Testing Strategy

### 11.1 v0.2 fixture database

A checked-in v0.2.2 fixture database (`crates/engramai/tests/fixtures/v02_sample.db`) seeds the migration test suite. Construction (one-time, manually produced):

1. Install engramai v0.2.2 (the published crate version).
2. Programmatically populate:
   - **100 `memories` rows** — mix of content styles (short factual, long narrative, multi-sentence), varied namespaces, varied `created_at` timestamps spanning ~6 months.
   - **~300 `hebbian_links`** — co-occurrence pairs with realistic weight distribution.
   - **5 `entities`** and **~15 `entity_relations`** — v0.2 legacy entity data that migration must leave untouched.
   - **10 `kc_topic_pages`** — with varied `source_memory_ids`, tags, and one "Superseded" row to exercise status mapping.
   - Correct `schema_version = 2` row.

3. Run `PRAGMA integrity_check` — must return `ok`.
4. Compute SHA-256 of the file, record in `tests/fixtures/v02_sample.db.sha256`.

The fixture is binary and checked into git LFS (already used by engramai for benchmark fixtures — GUARD-9 compliance, no new deps). A script `scripts/regen-v02-fixture.sh` reproduces it from scratch for future regeneration.

### 11.2 Idempotency tests

- **`test_migrate_twice_is_noop`**: Copy fixture → migrate → assert success → migrate again → assert second run reports "already v0.3" and exits 0 without modifying the file (file hash unchanged after second run).
- **`test_ddl_guard_add_column`**: Unit test for the `ALTER TABLE ADD COLUMN` guard — run on a DB that already has the column, verify no error.
- **`test_topic_insert_or_ignore`**: Run Phase 3 twice on the same fixture; assert `knowledge_topics` row count is 10 after each run (no duplicates).

### 11.3 Resume/interrupt tests

- **`test_resume_after_sigint_phase4`**: Start migration with a small `batch_size` (e.g., 10). After ~3 emissions, send SIGINT to the test subprocess. Verify exit code 2 and state file. Run `--resume`. Verify final DB state equals the output of an uninterrupted run (byte-for-byte on `graph_*` tables, modulo `occurred_at` timestamps which we normalize before comparison).
- **`test_random_kill_resume_matches_clean`**: Randomized fuzz: seed RNG, kill the subprocess at N random points during Phase 4 (hard kill, not SIGINT), `--resume` after each, verify final state matches the uninterrupted baseline. Run with 20 seeds in CI.
- **`test_resume_counters_monotone`**: Assert that across a sequence of (run, interrupt, resume, interrupt, resume, complete), `records_processed` and `records_succeeded` are monotone nondecreasing.
- **`test_phase_gate_stops_cleanly`**: `--gate Phase2` → assert exit code 6 and `schema_version == 3` but `count(knowledge_topics) == 0` (Phase 3 did not run).

### 11.4 Rollback drill (CI)

Implementation of §8.4. One test in the standard test suite (not a separate binary):

- **`test_rollback_from_backup`**:
  1. Copy fixture → pristine SHA recorded.
  2. Run full migration (exit 0).
  3. Assert backup exists.
  4. Simulate rollback procedure in-process: close the Connection, replace file, reopen.
  5. Assert `schema_version == 2`.
  6. Run a v0.2-style `recall("<canonical query>")` using a v0.2-compatible reader (`Memory::open_v02_compat_mode`, a read-only v0.2 emulation layer).
  7. Assert recall results match the pre-migration result captured in step 1.

Run this on every CI build; a regression here is a blocker.

### 11.5 Backward-compat suite (GOAL-4.9)

The 8-test matrix from §7.3, split into two modules:

**`tests/compat_fresh_v03.rs`** — four tests on a fresh-v0.3 DB:

```rust
#[test] fn test_store_fresh() { /* store() returns MemoryId; row exists in memories */ }
#[test] fn test_recall_fresh() { /* recall() returns ranked results; ranking contract holds */ }
#[test] fn test_recent_fresh() { /* recall_recent(N) returns N most recent */ }
#[test] fn test_assoc_fresh() { /* recall_with_associations returns Hebbian neighbors */ }
```

**`tests/compat_migrated_v02.rs`** — same four tests, but setup migrates the v0.2 fixture first:

```rust
fn migrated_db() -> Memory {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::copy("tests/fixtures/v02_sample.db", tmp.path()).unwrap();
    engramai::migrate(tmp.path(), Default::default()).unwrap();
    Memory::open(tmp.path()).unwrap()
}

#[test] fn test_store_migrated() { /* on migrated db, same assertions as fresh */ }
// ... three more
```

**Ranking-contract test** (expanded): a curated set of 30 (query, expected-top-5-memory-ids) pairs derived from the v0.2 fixture, **stratified as 10 exact-match queries, 10 semantic-similarity queries, and 10 multi-hop / association-bearing queries**. This size and stratification is chosen to detect a >10% precision@5 regression at p<0.05 against the v0.2 baseline, assuming binomially-distributed per-pair pass/fail — derived from a one-page power-analysis memo kept alongside the fixture (`tests/fixtures/ranking-fixture-power.md`). If future work needs tighter sensitivity (e.g., >5% regression detection), expand to ~120 pairs per the same stratification. The test asserts that v0.3 `recall` on the migrated DB returns the same top-5 IDs as v0.2 did pre-migration. Minor reordering within top-5 is allowed with a tolerance (Kendall-τ ≥ 0.8); top-1 must always match. A regression here blocks the merge.

Cross-test invariant: the backward-compat suite must pass with both the default `MigrateOptions` and with `--no-backup` enabled — backup presence must not affect backfill correctness.

### 11.6 Disk-full during resume

A test case exercises the "disk fills up mid-resume" failure mode, which is a real production concern (WAL growth during backfill can exceed the operator's estimate).

- **`test_resume_disk_full_surfaces_mig_disk_full`**: On Linux, mount a small tmpfs (e.g., 32 MiB) into the test working directory and place the v0.2 fixture there; start migration with a batch size sized so the WAL is projected to exceed available space mid-backfill. Assert:
  1. Migration aborts cleanly with exit code 10 (`MIG_DISK_FULL`).
  2. The `migration_state` row reflects the last successfully committed record (checkpoint not corrupted).
  3. After growing the tmpfs (or moving to a larger volume) and running `engramai migrate --resume`, the migration completes successfully and byte-identical (modulo timestamps) to an uninterrupted run.
- On macOS (which lacks tmpfs), the test uses a sparse-file loopback at a tiny size and skips with a `#[cfg_attr(target_os = "macos", ignore)]` fallback — the Linux CI invocation is the authoritative coverage.

### 11.7 Post-migration concurrent-read smoke test

A test that stresses the migrated DB under realistic read load, catching issues like stale SQLite statistics, WAL contention, or unindexed query paths that the happy-path migration-finished test would miss.

- **`test_concurrent_reads_post_migration`**: Migrate the v0.2 fixture. Spawn 4 concurrent reader tasks (tokio or std threads, matching the v0.3 retrieval runtime model) each issuing varied `recall` / `recall_recent` / `recall_with_associations` queries drawn from the ranking-fixture pool (§11.5) in a loop for 10 seconds. Assert:
  1. Zero errors returned by any reader.
  2. `PRAGMA integrity_check` after the 10-second stress returns `ok`.
  3. No reader's p99 latency exceeds 3× the single-reader baseline (catches obvious contention regressions; not a performance benchmark).
  4. `PRAGMA wal_checkpoint(TRUNCATE)` succeeds afterward (catches WAL-stuck bugs).

This test is a smoke test, not a benchmark — performance characterization is owned by v03-benchmarks. Its purpose is to ensure that migration output is immediately usable under concurrent load.

---

## 12. Cross-Feature References

This section documents how migration composes with the other v0.3 features. Each reference is a **call-out** (migration invoking the other feature) or a **constraint** (migration honoring the other feature's invariants). All references are point-in-time with respect to the sibling designs cited.

**v03-graph-layer** — referenced for **schema and CRUD**:

- `graph_entities`, `graph_entity_aliases`, `graph_edges`, `graph_predicates`, `graph_extraction_failures`, `graph_memory_entity_mentions`, `knowledge_topics`, `episodes`, `affect_mood_history` — table definitions (v03-graph-layer §4.1). Migration applies the DDL exactly as defined there (§4.2 of this doc).
- `GraphStore::upsert_entity`, `insert_edge`, `record_extraction_failure`, `upsert_topic` — existing CRUD methods in v03-graph-layer §5 (trait signature at line ~595). Backfill calls these, never writes SQL directly.
- `EntityKind::Topic` — used by topic reconciliation (§6.1) to create entity rows representing topics.
- **Handoff request (new methods this doc asks v03-graph-layer to add):**
  - `GraphStore::apply_graph_delta(memory_id: &str, delta: &GraphDelta) -> Result<(), GraphError>` — atomic per-record persist (entities + edges + mentions + `memories` row column update) used by backfill (§5.2). Without this, §5.2 would need to stitch multiple CRUD calls into a transaction itself; moving it to `GraphStore` keeps transaction boundaries on the owner.
- **Constraint honored**: v0.2 `entities` / `entity_relations` tables are never read or written by migration code (v03-graph-layer §2 parallel-namespace rule).

**v03-resolution** — referenced for **backfill pipeline reuse**:

- `PipelineError::ExtractionFailure` and `PipelineError::Fatal` — the error taxonomy used by §5.2 for per-record vs abort disambiguation (from v03-resolution §8).
- **Handoff request (new method this doc asks v03-resolution to add):**
  - `ResolutionPipeline::resolve_for_backfill(memory: &MemoryRecord) -> Result<GraphDelta, PipelineError>` — a backfill-specific pipeline entry point (differs from the normal `resolve` by: no episode creation, forced-sync execution, idempotent re-run per §5.2).
- **Constraint honored**: resolution's preserve-plus-resynthesize semantics (v03-resolution §4) apply to backfill re-runs — re-processing a memory merges into existing graph state, never overwrites.

**v03-resolution §5bis (Knowledge Compiler)** — referenced for **post-migration synthesis**:

- Migration carries v0.2 topics forward with `legacy = 1`. Subsequent KC runs (scheduled per v03-resolution §5bis) produce `legacy = 0` topics alongside; see §6.3. This is the only cross-feature behavior that happens *after* migration completes — the design contract is that KC respects the `legacy` flag invariant from §6.2.

**v03-retrieval** — referenced for **backward-compat behavior only**:

- `recall`, `recall_recent`, `recall_with_associations` route through v03-retrieval's default plan (v03-retrieval §4) on the migrated DB. Migration does not constrain retrieval's behavior beyond the ranking-contract test (§11.5, GOAL-4.9).

**v03-benchmarks** — referenced as a **downstream consumer**:

- Post-migration, the benchmarks suite (v03-benchmarks) runs LOCOMO / LongMemEval against the migrated DB to establish the v0.3 baseline (master DESIGN §8.3). Migration does not invoke benchmarks; it produces the input state they expect.

---

## 13. Requirements Traceability

| Requirement | Priority | Design section(s)                    | Test(s)                                                                      |
| ----------- | -------- | ------------------------------------ | ---------------------------------------------------------------------------- |
| GOAL-4.1    | P0       | §4.1, §4.2, §5.1, §5.2, §6           | `test_store_migrated`, `test_recall_migrated`, fixture replay in §11.5       |
| GOAL-4.2    | P0       | §8.1, §8.2, §10.1                    | `test_backup_written_before_ddl`, `test_no_backup_warns`, `test_backup_fail_aborts` |
| GOAL-4.3    | P0       | §4.3, §4.4, §5.4                     | `test_migrate_twice_is_noop`, `test_ddl_guard_add_column`                    |
| GOAL-4.4    | P1       | §5.2, §5.3, §10.2                    | `test_per_record_failure_surfaced`, `test_retry_failed_resolves`             |
| GOAL-4.5    | P1       | §5.5, §9.2, §9.3                     | `test_progress_cadence`, `test_progress_survives_resume`                     |
| GOAL-4.6    | P1       | §6.1, §6.2, §6.3, §6.4               | `test_topic_carry_forward`, `test_legacy_flag_preserved`, `test_resynth_alongside_legacy` |
| GOAL-4.7    | P1       | §8.1, §8.3, §8.4                     | `test_rollback_from_backup` (§11.4)                                          |
| GOAL-4.8    | P1       | §3.1, §3.2, §3.3, §5.4               | `test_phase_gate_stops_cleanly`, `test_resume_after_sigint_phase4`           |
| GOAL-4.9    | P0       | §7.1, §7.2, §7.3                     | `compat_fresh_v03.rs` (4 tests) + `compat_migrated_v02.rs` (4 tests) — §11.5 |

| Guard     | Honored by                                                                                             |
| --------- | ------------------------------------------------------------------------------------------------------ |
| GUARD-10  | §4.2 (non-destructive DDL only), §8.1 (backup-before-change), §8.3 (documented rollback), §8.4 (CI drill) |
| GUARD-2   | §5.3 (per-record failure surfacing), §6.4 (topic-stage failure surfacing)                              |
| GUARD-1   | §4.1 (`memories` / `hebbian_links` untouched except additive columns)                                  |
| GUARD-3   | §5.2 (backfill writes only to new `graph_*` tables; never mutates existing v0.2 rows)                  |
| GUARD-9   | §9.1 (subcommand of existing binary, no new deps)                                                      |
| GUARD-11  | §7.1 (signature preservation), §7.2 (behavioral contract), §7.3 (compat matrix), §11.5 (dual suite)   |

**Coverage audit.** Every GOAL-4.X has at least one satisfying section and one verifying test. Every migration-relevant guard has at least one enforcing mechanism. No section of this design introduces behavior not traceable back to a requirement.

---

*End of v03-migration/design.md*
