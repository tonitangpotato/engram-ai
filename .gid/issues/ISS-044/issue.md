---
id: "ISS-044"
title: "Wire MigrationOrchestrator::run_backfill to PipelineRecordProcessor"
status: open
priority: P1
created: 2026-04-27
component: crates/engramai-migrate/src/cli.rs
related: [v03-migration, ISS-043]
---

# ISS-044: Wire MigrationOrchestrator::run_backfill to PipelineRecordProcessor

**Status:** 🔴 Open
**Severity:** P1 — blocks any end-to-end v0.3 retrieval test on real data

## Discovery

While running the post-`d991715` LoCoMo conv-26 smoke test (sessions 1-3, 32 memories ingested via CLI), `engram migrate --accept-forward-only` failed with:

```
✗ migration failed: invariant violated during verification:
  phase4: backfill not yet implemented (T9 blocked); use --gate phase2 to
  run schema-only, or --dry-run to plan the migration
  exit code: 1
  error tag: MIG_INTERNAL_ERROR
```

`--dry-run` confirms the orchestrator can see the data:

```
counts.pre        : memories=32 kc_topics=0 entities=0 edges=0 topics=0
backfill          : 0/32 processed, 0 succeeded, 0 failed
⚠ phase4: backfill stubbed (T9 blocked); projected 32 memory rows would be processed
```

## Root cause

`PipelineRecordProcessor` shipped in commit `e458540` (T9), but
`MigrationOrchestrator::run_backfill` in `crates/engramai-migrate/src/cli.rs:564-595`
still has its pre-T9 stub body that errors out on live runs:

```rust
fn run_backfill(&mut self, conn: &Connection) -> Result<(), MigrationError> {
    // STUB: T9 (`task:mig-impl-backfill-perrecord`) is blocked on
    // `ResolutionPipeline::resolve_for_backfill` not yet existing.
    // ...
    Err(MigrationError::InvariantViolated(
        "phase4: backfill not yet implemented (T9 blocked); ...".to_string(),
    ))
    // ...
}
```

The stub's premise is now false:
- ✅ `ResolutionPipeline::resolve_for_backfill` exists (`crates/engramai/src/resolution/pipeline.rs:744`)
- ✅ `PipelineRecordProcessor::process_one` exists and is tested
- ❌ The orchestrator doesn't invoke either

## What's required

### a) Replace stub body with real iteration

```rust
fn run_backfill(&mut self, conn: &Connection) -> Result<(), MigrationError> {
    // 1. Open v0.3 graph store (path derived from main DB or --graph-db flag)
    // 2. Build a BackfillResolver impl wrapping ResolutionPipeline::resolve_for_backfill
    //    — needs extractor (Anthropic) credentials plumbed in
    // 3. Build PipelineRecordProcessor with that resolver + namespace
    // 4. Iterate `memories` table in deterministic order (id ASC)
    //    — respect resume from checkpoint if --resume
    // 5. For each row → process_one(conn, row, &graph_store, &checkpoint)
    // 6. Aggregate BackfillReport (records_total, succeeded, failed, skipped_idempotent)
    // 7. Surface per-record failures into graph_extraction_failures table
    // 8. Honor --stop-on-failure
}
```

### b) Decide the graph-DB path policy

Today the CLI has no `--graph-db` flag. Options:
- **Auto-derive**: `<db>.graph.db` next to the v0.2 DB (mirrors what tests do)
- **Explicit flag**: `--graph-db <path>` (more flexible, less surprising)
- Recommendation: explicit flag, default to `<db>.graph.db` if unset

### c) Plumb extractor credentials into migrate CLI

`resolve_for_backfill` needs an extractor. Today only `engram store --extractor anthropic --oauth --auth-token TOKEN` accepts these. The migrate CLI needs the same flags or it needs to read from a config file / env (`ANTHROPIC_API_KEY`, etc.).

### d) Wire BackfillCheckpoint to the migration_state table

`PipelineRecordProcessor` consumes a checkpoint store. Either:
- The migrate orchestrator owns checkpoint state in `migration_state` and adapts to the processor's interface, OR
- The processor's checkpoint store is the same `migration_state` rows (simpler — single source of truth)

## Acceptance criteria

- [ ] `engram migrate --accept-forward-only` against a populated v0.2 DB completes Phase 4 successfully (no stub error)
- [ ] After migration, `entities` and `edges` tables are populated proportional to ingested memories
- [ ] `engram migrate --resume` after Ctrl-C correctly picks up from last checkpoint
- [ ] `engram migrate --retry-failed` re-processes rows in `graph_extraction_failures`
- [ ] `--stop-on-failure` aborts on first per-record failure
- [ ] Idempotent: running migrate twice doesn't double-write edges (relies on T9's `(memory_id, delta_hash)` key)
- [ ] LoCoMo conv-26 smoke (`.gid/issues/_smoke-locomo-2026-04-27/`) shows non-zero hit rate after rerun

## Out of scope

- Refactoring `apply_graph_delta` to accept borrowed `&Transaction` — that's ISS-043 and a separate concern
- Changing the resolution pipeline semantics
- Adding new migration phases

## Effort estimate

~1-2 days. The mechanical wiring is straightforward; the time goes into:
- Extractor credential plumbing (CLI flag surface decisions)
- Checkpoint-store interface adapter (migration_state ↔ BackfillCheckpoint)
- Integration tests on a real ingested DB (not just unit tests with mocks)

## References

- T9 processor: `crates/engramai-migrate/src/processor.rs`
- Stub site: `crates/engramai-migrate/src/cli.rs:564`
- Resolution pipeline: `crates/engramai/src/resolution/pipeline.rs:744`
- Smoke test repro: `.gid/issues/_smoke-locomo-2026-04-27/`
- Related: ISS-043 (literal single-tx atomicity follow-up)

## Notes for whoever implements this

Don't combine this with ISS-043. Land the wiring first (this issue), then refactor the tx model. Otherwise the diff is too large to review and a regression hides too easily.
