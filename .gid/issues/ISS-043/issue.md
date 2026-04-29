---
id: ISS-043
title: Restore literal single-tx atomicity in PipelineRecordProcessor (T9)
status: wontfix
priority: P2
created: 2026-04-27
component: crates/engramai-migrate/src/processor.rs
related:
- v03-migration
- v03-graph-layer
---

# ISS-043: Restore literal single-transaction atomicity in T9 backfill processor

**Status:** 🔴 Open
**Severity:** Low (semantic equivalence already achieved; this is a doc/contract alignment)

## Background

The v0.3 migration design (`.gid/features/v03-migration/design.md` §5.2) specifies:

> A single SQLite transaction wraps `apply_graph_delta` + checkpoint advance.

The T9 implementation (`crates/engramai-migrate/src/processor.rs`, landed 2026-04-27 in commit `e458540`) achieves **equivalent end-to-end semantics** but does *not* literally satisfy "single transaction wraps apply + checkpoint advance."

## What's actually happening

`GraphStore::apply_graph_delta` opens and commits its own SQLite transaction internally. It does not accept a borrowed `&Transaction`. So `PipelineRecordProcessor::process_one` cannot wrap both calls in a single outer tx — the apply tx commits before the checkpoint advance can join it.

Current implementation uses **two-step commit + idempotent replay**:

1. `apply_graph_delta` commits its tx (graph delta durable).
2. `update_backfill_progress` commits checkpoint advance in a separate tx.
3. **Crash window:** if the process dies between step 1 and step 2, the checkpoint still points at the previous record. On restart, the same record is re-fetched.
4. Re-application is safe because `apply_graph_delta` keys on `(memory_id, delta_hash)` and short-circuits to `ApplyReport::already_applied_marker()` — no duplicate edges, no double-write.
5. Checkpoint then advances normally in step 2.

**Net behavior:**
- ✅ At-least-once apply (per record) — by replay
- ✅ Exactly-once durable effect — by idempotency key
- ✅ Monotone checkpoint counters — never moves backward
- ✅ "Advance after commit" property — preserved
- ❌ Literal single-tx contract — violated (two commits, not one)

## Why it landed this way

`apply_graph_delta`'s signature was already established by `v03-graph-layer` before T9 was implemented. Refactoring it to accept a borrowed transaction would have:
- Touched every call site (resolution pipeline, tests, fixtures)
- Required carving out a new `apply_graph_delta_in_tx(&Transaction, ...)` variant
- Pushed T9 outside its scope window

Given the semantic equivalence (idempotency makes replay safe), the T9 author chose to ship the working implementation and file this follow-up.

## What this issue tracks

Refactor `GraphStore::apply_graph_delta` to support a borrowed-transaction variant so `PipelineRecordProcessor` can satisfy the literal contract:

```rust
// Target shape (sketch)
impl GraphStore {
    pub fn apply_graph_delta_in_tx(
        &self,
        tx: &rusqlite::Transaction,
        delta: &GraphDelta,
    ) -> Result<ApplyReport, EngramError>;

    // Existing method becomes a thin wrapper:
    pub fn apply_graph_delta(&self, delta: &GraphDelta) -> Result<ApplyReport, EngramError> {
        let tx = self.conn.transaction()?;
        let report = self.apply_graph_delta_in_tx(&tx, delta)?;
        tx.commit()?;
        Ok(report)
    }
}
```

Then `PipelineRecordProcessor::process_one` opens one outer tx, calls `_in_tx`, advances checkpoint, commits — restoring the literal single-tx atomicity stated in the design.

## Acceptance criteria

- [ ] `GraphStore::apply_graph_delta_in_tx(&Transaction, &GraphDelta)` exists and is the primary implementation
- [ ] `GraphStore::apply_graph_delta(&GraphDelta)` is a 3-line wrapper (open tx, call _in_tx, commit)
- [ ] `PipelineRecordProcessor::process_one` opens a single outer tx wrapping both apply and checkpoint advance
- [ ] All existing tests pass (idempotency tests in particular: `test_replay_after_apply_skips`)
- [ ] design.md §5.2 implementation note (added 2026-04-27) is removed — literal contract restored
- [ ] No regression in T9 throughput (idempotency check overhead should disappear in happy path since replay branch is no longer hit on normal operation)

## Out of scope

- Changing the idempotency key `(memory_id, delta_hash)` — keep it as belt-and-suspenders for crash-recovery cases
- Changing checkpoint format
- Touching v03-resolution `resolve_for_backfill`

## Effort estimate

~1 day. The mechanical refactor of `apply_graph_delta` is small; the risk lives in call sites in v03-resolution that may pass `GraphStore` by `&self` and not have a tx ready. Audit before starting.

## References

- Design: `.gid/features/v03-migration/design.md` §5.2 (with 2026-04-27 implementation note)
- Implementation: `crates/engramai-migrate/src/processor.rs` (search for "Atomicity model" docstring)
- Landing commit: `e458540 feat(migrate): T9 PipelineRecordProcessor`
- Related: `task:mig-impl-record-processor` (closed)

## Tracking task

`task:mig-followup-graph-delta-borrowed-tx` (in `.gid-v03-context/graph.db`)

---

## 2026-04-29 Resolution: wontfix-by-design

Investigated under autopilot A8 (rustclaw/tasks/2026-04-29-autopilot.md, items A8.1–A8.6).

**Verdict:** literal "single-tx wraps apply + checkpoint advance" is not feasible in the prod call path, and the existing per-record-commit + idempotent-replay design is the *correct* contract — not a workaround.

### Why literal fix is impossible (A8.3 finding)

Prod path uses **two separate SQLite database files** with two independent `Connection`s:

- v0.2 main DB (passed via `--db-path`, where memories + checkpoint table live)
- graph DB (passed via `--graph-db`, where `apply_graph_delta` writes nodes/edges)

A single `BEGIN`/`COMMIT` cannot span two SQLite Connections on two different files. SQLite has no native 2PC. Refactoring `apply_graph_delta` to accept a borrowed `&Transaction` (the original proposed fix) does not help because there's no shared tx to borrow against.

Only the unit-test path (`graph_store: None`, `apply_delta_through_migration_conn`) reuses one Connection and could nominally support a single tx — but writing a "K-1 records rolled back" test (A8.5) for that path would contradict the prod contract. Per-record-commit + idempotency-as-atomicity is what should be tested, and that *is* tested.

### Test evidence (A8.6)

- `cargo test -p engramai`: 2058 / 0 / 8 ignored
- `cargo test -p engramai-migrate`: 206 / 0 / 11 ignored

Atomicity-equivalent semantics covered by:

- `processor::tests::process_one_idempotent_on_replay`
- `processor::tests::process_one_extraction_failure_advances_checkpoint`
- `failure::tests::record_failure_is_idempotent_on_replay`
- `checkpoint::tests::checkpoint_update_inside_transaction_rolls_back_on_abort`
- `checkpoint::tests::checkpoint_update_inside_transaction_persists_on_commit`
- `schema::tests::phase2_atomic_alter_rollback_when_version_step_fails_due_to_corrupt_pre_state`
- `schema::tests::phase2_idempotent_on_replay`
- `phase_machine::tests::run_aborts_on_phase_executor_error_and_leaves_checkpoint_at_failed_phase`
- `phase_machine::tests::run_resumes_from_checkpointed_phase_and_skips_completed_phases`
- integration: `test_backfill_idempotent_on_v03_db`, `test_resume_after_gate_continues_from_checkpoint`, `test_rollback_idempotent_on_double_restore`

There are zero tests asserting "strict batch atomicity" — confirming current contract is *idempotent replay*, not *atomic batch*.

### Follow-up (lightweight, non-blocking)

File a doc-only task to update `processor.rs` rustdoc and `design.md §5.2` to state the *as-built* contract:

> Atomicity contract: per-record commit on the graph DB, followed by per-record checkpoint advance on the main DB. Crash mid-record is recovered on replay via idempotency keys (`memory_id`, `delta_hash`); crash between graph commit and checkpoint advance is recovered by re-processing the same record (no double-write because of idempotency).

That sentence becomes the contract; this issue is closed.
