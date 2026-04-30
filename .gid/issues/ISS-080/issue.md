---
title: Worker pool swallows pipeline job errors silently
status: fixed
priority: P1
labels: [observability, resolution-pipeline, silent-failure]
relates_to: [ISS-055, ISS-075, ISS-076]
---

# Worker pool swallows pipeline job errors silently

## Summary

`crates/engramai/src/resolution/worker.rs::worker_loop` discards every `ProcessError` returned by `JobProcessor::process`. Failed jobs only increment a counter — no `tracing::error!`, no `warn!`, no propagation back to the caller. This makes any failure in the resolution pipeline (dim mismatch, namespace mismatch, embedder errors, etc.) completely invisible at runtime.

## Reproduction

1. Construct a `Memory` via `Memory::with_pipeline_pool` where the embedder dimension and the `SqliteGraphStore` dimension disagree (e.g. default_embedder = 384, store = 768).
2. Call `memory.add(...)` with N items.
3. Observe:
   - `memory.add` returns `Ok(())` for every call.
   - SQLite `memories` table contains all N rows (raw write happens upstream of the pipeline).
   - `entities` / `aliases` tables are empty (or only partially populated).
   - **stderr/logs show nothing.** The worker pool just increments `jobs_failed`.

## Root Cause

`crates/engramai/src/resolution/worker.rs:471` (current `worker_loop`):

```rust
WorkerMsg::Job(job) => {
    let result = processor.process(job);
    match result {
        Ok(()) => { stats.jobs_processed.fetch_add(1, Ordering::Relaxed); }
        Err(_) => { stats.jobs_failed.fetch_add(1, Ordering::Relaxed); }  // ← error dropped
    }
    stats.jobs_in_flight.fetch_sub(1, Ordering::Relaxed);
}
```

Notes:
- `Err(_)` doesn't even bind the error.
- A grep of the whole file finds **zero** `tracing::*!` / `log::*!` / `warn!` / `error!` calls.
- `Memory::add` → `BoundedJobQueue::try_enqueue` returns `Ok(())` as soon as the job is in the queue, so the failure is fully decoupled from the caller's success path.

## Impact

- Every silent failure in the resolution pipeline manifests as "memory.add succeeded but the graph is empty/partial".
- Forces every debugging session to start with "is the worker pool eating errors?" instead of "what is the actual error?".
- Hides regressions: any future dim/namespace/schema mismatch will land as a silent bug in production.

## Related

- **ISS-055** — `Memory::with_pipeline_pool` uses `PipelineConfig::default()` with `namespace=""`. Same shape of defect (defaults don't match real schema), but at the namespace layer.
- **ISS-075 / ISS-076** — recent `default_embedder()` rework (IdentityEmbedder(384)) intersects with `SqliteGraphStore::new` (often 768). The dim mismatch surfaced by ISS-080 was the trigger that exposed the silent-swallow bug.

## Acceptance Criteria

1. `worker_loop` logs every job failure with `tracing::error!`, including:
   - the error (`%e` via `Display`),
   - the `memory_id` from the job,
   - the worker index `_idx`.
2. `ProcessError`'s `Display` impl renders the underlying cause (no `Err(_)`-style opaque error).
3. After the fix, reproducing the dim-mismatch case from above produces a visible `tracing::error!` line on stderr identifying the failing pipeline stage.
4. No change to the success path / counters / public API of `WorkerPool`.
5. Unit test in `worker.rs` (or sibling test module) that asserts a failing `JobProcessor` causes the worker loop to log via `tracing` — captured via `tracing-subscriber` test layer or equivalent — and still increments `jobs_failed`.

## Out of Scope

- Fixing the underlying dim mismatch (separate issue / part of ISS-075/076 follow-up).
- Re-architecting error propagation back to `Memory::add` (would require changing the async boundary; track separately if needed).
- Adding metrics export (Prometheus etc.) for `jobs_failed` — pure logging fix here.

## Notes

Discovered while debugging an ingest run that wrote 113 memory rows but produced an empty entity graph. The dim mismatch was guessed correctly only after manual code inspection — the silent swallow is what made the diagnosis expensive.

## Resolution

**Fixed in `crates/engramai/src/resolution/worker.rs::worker_loop` (~line 489).**

Diff summary:
- `Err(_)` → `Err(e)` (binds the error).
- Captured `memory_id = job.memory_id.clone()` before `processor.process(job)` (job is moved by the call).
- Added `log::error!("pipeline job failed: worker={_idx} memory_id={memory_id} error={e}", ...)` immediately before incrementing `jobs_failed`.
- `ProcessError`'s `Display` impl already renders the underlying carrier string (`stage failure: {m}` etc.) — no change needed there.

**Test added: `failures_are_logged_iss080`** (worker.rs tests module).

Installs a `DispatchLogger` (one global, since `log::set_logger` is install-once) forwarding to a per-test buffer via a `Mutex<Option<Arc<Mutex<Vec<String>>>>>` slot. Drives a forced failure through the worker pool and asserts the captured log line contains:
- `"pipeline job failed"` (literal prefix)
- `"bad-mem"` (the memory_id)
- `"stage failure: forced"` (the underlying `ProcessError::Stage` Display output)

This locks in all three acceptance fields (memory_id present, error rendered, stable prefix) so silent-swallow regressions trip the test.

**Verification:**
- `cargo test -p engramai --lib resolution::worker` → 9/9 passed (was 8 before this fix).
- `cargo test -p engramai --lib` → 1810 passed, 0 failed, no regressions.
- `cargo build -p engramai --lib` → clean (only pre-existing unrelated warnings in `retrieval/orchestrator.rs`).

**Out-of-scope items confirmed not addressed (per Issue):**
- Underlying dim mismatch (with_pipeline_pool default_embedder=384 vs SqliteGraphStore=768) — left for ISS-075/076 follow-up.
- Error propagation back through `Memory::add`'s sync return path — would require re-architecting the async boundary, tracked separately if needed.
