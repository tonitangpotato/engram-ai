---
id: "ISS-043"
title: "Restore literal single-tx atomicity in PipelineRecordProcessor (T9)"
status: open
priority: P2
created: 2026-04-27
component: crates/engramai-migrate/src/processor.rs
related: [v03-migration, v03-graph-layer]
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
