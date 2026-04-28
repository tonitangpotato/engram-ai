# ISS-058: `engram migrate` writes graph rows to main DB, not `--graph-db` (split-brain with read store)

**Status**: open
**Priority**: P1 (correctness — migrate output goes to wrong file when `--graph-db` is separate)
**Filed**: 2026-04-28
**Filed by**: rustclaw (during ISS-044 e2e test fixup)
**Related**: ISS-044 (closed — backfill wiring), ISS-046 (closed — `engram store` symmetry)

## Symptom

When running `engram migrate` against a v0.2 DB with a separate `--graph-db` path
(default: `<main>.graph.db` — a different file from the main DB):

- Phase 4 backfill processes records and reports success
- `apply_graph_delta` returns `entities_upserted > 0`, `mentions_inserted > 0`
- But the rows land in the **main DB** (`v02.db`), not in `<v02>.graph.db`
- `<v02>.graph.db` is created with the v0.3 schema but stays empty
- The pipeline's read-side store (used during dedup candidate lookup) **does** point
  at `<v02>.graph.db` — so dedup queries see an empty graph forever

## Reproduction

`crates/engramai-migrate/tests/iss044_backfill.rs` — first iteration of the
test queried `graph_db` for `entities`, found nothing, failed. Currently
the test asserts on `main_db` for `graph_entities` and passes. The test
file documents the workaround inline (search for "split-brain").

## Root cause

`crates/engramai-migrate/src/cli.rs` and `processor.rs` use **two separate
SqliteGraphStore connections**:

1. **Read-side store** (`cli.rs::run_backfill` ~line 765):
   - Opens `graph_db_path` (default `<main>.graph.db`)
   - Wrapped in `Arc<Mutex<SqliteGraphStore<'static>>>`
   - Passed into `ResolutionPipeline::new(...)` as the `graph_store` arg
   - Used during `resolve_for_backfill` for candidate lookup

2. **Write-side store** (`processor.rs::apply_delta_through_migration_conn`):
   - Constructs `SqliteGraphStore::new(conn)` over the **bf_conn** = `main_db_path`
   - Calls `apply_graph_delta(delta)` on it
   - All entity/edge/mention writes land here

These are two physically different SQLite files when `graph_db_path != main_db_path`.

The reason ISS-044 needed `init_graph_tables(&bf_conn)` (cli.rs:863) at all is
exactly this: the writes hit `bf_conn`, so its DB needs the graph schema. But
that "fix" silently committed the split-brain — it papers over the symptom
(`no such table`) rather than addressing the real issue (writes are pointed at
the wrong DB).

## Why this is wrong

ISS-046 established the contract: graph layer reads and writes go to
**graph_db_path**. `engram store`'s pipeline pool already does this correctly
(both `with_pipeline_pool` and `with_graph_store` use the same path).

`engram migrate` should match the same contract:

- Either: route `apply_graph_delta` through the pipeline's read-side store
  (the `Arc<Mutex<SqliteGraphStore>>` over `graph_conn`) → writes land in
  `graph_db_path`
- Or: declare `graph_db_path == main_db_path` for migrate (same-file mode
  always) → drop the `--graph-db` flag entirely from migrate

The former is the v0.3 architectural contract. The latter is a retreat.

## Side effects

- **Dedup is broken across `migrate`** — read store sees empty graph_db,
  every `resolve_for_backfill` call thinks it's a fresh entity, no merges
  ever happen. The processor's `entities_merged: 0` is observed in every
  delta in the e2e test.
- **`engram retrieve` (which reads from `graph_db`) sees an empty graph**
  on a freshly migrated v0.2 DB — same class of bug as ISS-046 in reverse.
  Workaround: post-migrate, run a one-shot copy of `graph_*` tables from
  main_db into graph_db. Ugly.
- **Tests must assert against main_db**, not graph_db, until fixed —
  baked into ISS-044 e2e tests.

## Acceptance criteria for fix

- [ ] After `engram migrate --graph-db /tmp/g.db --accept-forward-only v02.db`,
      `/tmp/g.db` has `graph_entities` populated proportional to memories
- [ ] `v02.db` has the schema tables (idempotent init still on bf_conn for
      cross-file FK reasons) but they may be empty (write path goes to
      graph_db only)
- [ ] Dedup observed: re-running migrate on a v0.2 DB with overlapping
      content shows `entities_merged > 0` (currently always 0)
- [ ] ISS-044 e2e tests updated to assert on graph_db, not main_db
- [ ] Same-file mode (`--graph-db == <main>` or both default to same file)
      still works (regression check)

## Hypothesis for fix shape

Replace `apply_delta_through_migration_conn(conn, &delta)` in `processor.rs`
with a `BackfillResolver`-style hook that owns `Arc<Mutex<SqliteGraphStore>>`
over `graph_conn`, so the persist call uses the same store that read-side
queries use. The per-record `bf_conn` is then only used for checkpoint /
failure / migration_state writes — which is what the design §3.3 actually
says ("phase machine's foreground conn is reserved for migration_state /
lock writes").

Two-phase commit across two SQLite files is *not* required: graph_db
writes commit in their own tx, then bf_conn writes the checkpoint advance
in its own tx. On crash between them, the next run's `already_applied`
check on `graph_applied_deltas` (in graph_db) makes replay safe.
