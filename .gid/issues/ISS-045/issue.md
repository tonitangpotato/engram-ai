# ISS-045: ~~Fresh-ingest path does not write v0.3 graph layer~~ → Closed (initial diagnosis incorrect)

**Status**: closed-superseded
**Filed**: 2026-04-27
**Closed**: 2026-04-27 (same session)
**Superseded by**: ISS-046 (the real gap)

## Initial diagnosis (incorrect)

I claimed `Memory::store_*` never writes to graph layer. Reading deeper:

- ✅ `Memory::with_pipeline_pool(...)` IS implemented (memory.rs:299) — installs WorkerPool + graph_store, hooks into v0.3 ingest
- ✅ `Memory::store_raw` Path A and Path B both call `enqueue_pipeline_job(id)` on `Inserted` outcomes (memory.rs:2739, 2847)
- ✅ Worker pool drains queue async, runs ResolutionPipeline, writes graph

The fresh-ingest write path is fully wired inside engramai. My earlier grep missed `enqueue_pipeline_job` because I was looking for `pipeline.resolve` directly.

## Real root cause (now in ISS-046)

`engram store` CLI does not call `Memory::with_pipeline_pool(...)` because it has no `--graph-db` flag and no plumbing for the WorkerPool. The capability exists; the CLI wrapper just never wires it up. LoCoMo ingest used `engram store` so v0.3 graph stayed empty → retrieval 0/25.

## Lesson

Read the full file before filing root-cause issues. `enqueue_pipeline_job` is the v0.3 ingest hook; `pipeline.resolve` is its implementation detail invoked by the worker, not the call site to grep for.
