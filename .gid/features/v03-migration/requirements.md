# Requirements: Engram v0.3 — Migration

## Overview

This feature owns the **v0.2 → v0.3 migration path**: everything needed to take an existing engramai v0.2.2 SQLite database and produce a functioning v0.3 database. It covers pre-migration safety, schema transition, backfill of graph-layer data from existing memory content, phased rollout with observable gates, topic reconciliation, and rollback. It does NOT own the static data model (v03-graph-layer), the runtime write pipeline (v03-resolution), or the retrieval layer (v03-retrieval) — migration targets the model, reuses the pipeline for backfill, but defines neither.

**Parent:** `.gid/docs/requirements-v03.md` (master requirements — all GUARDs defined there, especially GUARD-10).

## Priority Levels

- **P0**: Core — required for migration to function at all
- **P1**: Important — needed for production-quality migration experience
- **P2**: Enhancement — improves migration UX or observability

## Goals

### Migration Core (GOAL-4.1 – GOAL-4.3)

- **GOAL-4.1** [P0]: A v0.2.2 database is migrated to v0.3 without data loss — every MemoryRecord, Hebbian link, Knowledge Compiler topic, and associated metadata present in the v0.2 database is queryable in the v0.3 database after migration completes. *(ref: DESIGN §8.1 schema migration + §1/G7 migration path)*

- **GOAL-4.2** [P0]: A full backup of the database is written to a separate file before any schema modification begins. If the backup cannot be written (disk full, permissions, I/O error), migration aborts with a clear error and the original database is untouched. An explicit opt-out flag suppresses the backup but produces a visible warning that recovery will not be possible. *(ref: DESIGN §8.1 pre-migration safety + GUARD-10 in master)*

- **GOAL-4.3** [P0]: Migration is idempotent — running the migration tool against an already-migrated v0.3 database completes successfully as a no-op (no duplicate rows, no schema errors, no side effects). Partial migrations that were interrupted are detected and can be safely resumed or re-run from the beginning without data corruption. *(ref: DESIGN §8.1 schema migration — non-destructive adds)*

### Backfill (GOAL-4.4 – GOAL-4.5)

- **GOAL-4.4** [P1]: Existing MemoryRecord content is processed through the entity/edge extraction pipeline to populate the graph layer. Extraction failures on individual records are surfaced per-record (record ID + error kind + stage), not silently skipped — the caller can distinguish "backfill complete with N failures" from "backfill complete, all succeeded." Failed records are retryable without re-processing already-succeeded records. *(ref: DESIGN §8.1 backfill + GUARD-2 never-silent-degrade in master)*

- **GOAL-4.5** [P1]: Migration exposes a progress API returning `(records_processed, records_total, records_succeeded, records_failed)`. Updates are emitted at least every 100 records processed or every 5 seconds, whichever comes first. Progress survives process restart (a resumed migration does not reset any of the counters to zero). Time-to-completion estimation is out of scope for the v0.3.0 migration tool. *(ref: DESIGN §9 Phase 5 migration + polish)*

### Topic Reconciliation (GOAL-4.6)

- **GOAL-4.6** [P1]: All v0.2 Knowledge Compiler topics are carried forward into the v0.3 L5 topic layer with a `legacy=true` flag and a provenance field pointing to their v0.2 source (topic id + creation timestamp). Post-migration, L5 re-synthesis may run in the background and produce new topics alongside the legacy ones; legacy topics are never deleted or overwritten by re-synthesis. A v0.2 topic that cannot be carried forward (e.g., corrupt record) is recorded as a failure per GUARD-2 — never silently dropped. *(ref: DESIGN §10 Q7 — resolved to preserve-plus-resynthesize direction + §3.6 Knowledge Topic kept from v0.2)*

### Backward Compatibility (GOAL-4.9)

- **GOAL-4.9** [P0]: v0.2 call sites using `store`, `recall`, `recall_recent`, and `recall_with_associations` compile against v0.3 without source changes (signatures preserved) and preserve their documented v0.2 behavior: `store(content)` returns the same kind of identifier, `recall(query)` returns ranked memories under the same ranking contract, and `recall_with_associations` surfaces Hebbian-linked memories. Internally, these methods may route through the v0.3 pipeline transparently. A dedicated backward-compatibility test suite exercises each of the four methods against both a v0.2 fixture database (migrated) and a fresh v0.3 database. *(ref: DESIGN §7.3 backward compatibility + GUARD-11 in master)*

### Rollback & Phased Rollout (GOAL-4.7 – GOAL-4.8)

- **GOAL-4.7** [P1]: Rollback from a migrated v0.3 database to v0.2 is possible using the pre-migration backup. A documented recovery procedure exists, is testable (can be exercised in CI or by an operator), and restores the database to its exact pre-migration state. *(ref: DESIGN §8.1 rollback + GUARD-10 in master)*

- **GOAL-4.8** [P1]: The migration rollout is phased (Phase 0 through Phase 5 per the design roadmap). Each phase has at least one explicit, observable gate condition that must be satisfied before advancing to the next phase. A phase can be paused mid-execution and resumed later without data corruption or loss of completed work within that phase. *(ref: DESIGN §9 Phase 0–5 roadmap + gate conditions)*

## Guards

All cross-cutting guards are defined in the master requirements document (`.gid/docs/requirements-v03.md`). The following guards are especially relevant to migration:

- **GUARD-10** [hard]: v0.2 database survives migration without data loss; pre-migration backup before any schema change; abort if backup fails; rollback possible. *(defined in master)*
- **GUARD-2** [hard]: No silent degradation — extraction failures during backfill must be surfaced. *(defined in master)*
- **GUARD-1** [hard]: Episodic completeness — migration must not lose episodic traces. *(defined in master)*
- **GUARD-3** [hard]: No retroactive silent rewrites — backfill creates new graph data, never overwrites existing v0.2 data without audit trail. *(defined in master)*
- **GUARD-9** [hard]: No new required external dependencies — migration tooling ships within the existing crate. *(defined in master)*

## Out of Scope

- Schema design for new v0.3 tables (owned by v03-graph-layer)
- The extraction/resolution pipeline itself (owned by v03-resolution — migration reuses it for backfill)
- Retrieval behavior changes post-migration (owned by v03-retrieval)
- Multi-version migration chains (e.g., v0.1 → v0.3) — only v0.2.2 → v0.3 is supported
- Automatic rollback (rollback is manual, from backup — no built-in "undo migration" command that reverse-engineers schema changes)
- Online migration with zero downtime for active writes — migration may require exclusive access to the database

## Dependencies

- **engramai v0.2.2 schema** — migration source format; must be fully understood to preserve all data
- **v03-graph-layer data model** — migration target; schema additions are defined there, migration applies them
- **v03-resolution pipeline** — backfill reuses the extraction/resolution pipeline to populate Entity/Edge tables from existing MemoryRecord content
- **SQLite (rusqlite)** — existing dependency; migration uses transactions for atomicity

---

**9 GOALs** (4 P0 / 5 P1 / 0 P2) — GUARDs defined in master (GUARD-10, GUARD-2, GUARD-1, GUARD-3, GUARD-9, GUARD-11 are migration-relevant).
