# v03-resolution — Implementation Plan

> Phasing for §5 (Batching & Async) execution model. Design is in `design.md` (r3, approved). This file is implementation-only — fills the gap between design ("there is a worker pool") and code ("here is how D1 lands").
>
> Not part of design review. Updated as phases land.

## Decision Log (pre-coding)

These are implementation decisions the design left open. Each one was considered against alternatives.

### D-1. Connection ownership: per-worker connection via `DbHandle`

**Decision.** Each worker thread owns its own `rusqlite::Connection`, opened from a shared `DbHandle` that carries only the DB path + PRAGMA convention. Schema migrations stay in `Storage::new` (primary connection only). `DbHandle::open_connection()` opens a new connection and applies PRAGMAs but **does not** run migrations.

**Why not Arc<Mutex<Storage>>.** Serializes all reads + writes, including main-thread `recall` against worker `persist`. Violates GUARD-1 ("ingest never blocked by L4") and design §5.1 ("Cross-session jobs run in parallel" requires concurrent connections, not a serialized one).

**Why not channel-based ("send job to a single SQLite-owning thread").** Over-engineered for in-process concurrency. SQLite WAL is the channel. Adds an extra hop and a bespoke message protocol where rusqlite already provides one (the connection itself).

**Why not just give Worker a `String` path.** Two reasons:

1. PRAGMA drift. PRAGMA setup currently lives in `Storage::new`. If Worker re-opens with `Connection::open(path)` and copies PRAGMA code, future changes to the PRAGMA set (e.g. add `synchronous=NORMAL`) will silently desync between primary and workers. Single source of truth required.
2. `:memory:` detection. `:memory:` databases are per-connection — a worker opening a secondary `:memory:` connection gets an empty DB, not the primary's data. This is a footgun that needs one canonical detection point, not a string check scattered across modules.

`DbHandle` is the abstraction that makes both single-source-of-truth.

**Why not extend `Storage` with `open_secondary()`.** `Storage::new` runs ~12 schema migrations. A secondary `Storage` would re-run them on every worker startup — idempotent but wasteful, and conceptually wrong (`Storage` represents "this process's schema-owning store"; secondary connections aren't that). Keeping `Storage` single-purpose and introducing `DbHandle` for "connection factory" matches the actual responsibilities.

**Implication for `:memory:`.** Worker pool is meaningless on in-memory DBs (separate connection = separate empty DB). When `DbHandle::is_in_memory()` is true, `Memory::start_resolution_workers()` forces `worker_count = 0` and runs the resolution pipeline inline (synchronous, on the caller's thread). Logs an info-level note. Tests that need worker behavior must use a tempfile DB. This is **not** an error path — `:memory:` for unit tests is the most common case.

### D-2. `ResolutionConfig` lives in `engramai::resolution::config`

New struct. Default: `worker_count = 1`, `queue_capacity = 1024`. Cap `worker_count ≤ 8` per design §5.1. Constructed from `MemoryConfig` (single field added there: `resolution: ResolutionConfig`).

### D-3. Worker lifecycle: explicit start/stop, no Drop magic

`Memory::start_resolution_workers()` spawns N OS threads (not tokio tasks — workers are CPU+SQLite, not async). `Memory::shutdown_resolution_workers()` closes the queue, joins threads with a 5s timeout, force-detaches on timeout (logged as warning). Exposed on `Memory` so callers control lifecycle; no implicit Drop-based shutdown because join-on-Drop in tests has bitten us before (panicking thread + Drop = deadlock).

For v0.2-compat callers that never call `start_resolution_workers`, queue is unbounded and inline mode runs (covered by D-1 fallback).

---

## Phasing

Each phase is independently mergeable + testable. Sub-agent delegation rule: each phase ≤ 300 LOC output, otherwise split.

### D1a — Connection abstraction + single-worker skeleton

**Scope:** ~250 LOC.

- Add `DbHandle { path: PathBuf }` in `storage.rs` (new sibling of `Storage`).
  - `DbHandle::open_connection() -> Result<Connection, rusqlite::Error>` — sets PRAGMAs, no migrations.
  - `DbHandle::is_in_memory() -> bool`.
  - PRAGMA setup extracted into `fn apply_pragmas(&Connection)` — called by both `Storage::new` and `DbHandle::open_connection`. Single source of truth.
- `Storage` stashes a clone of its `DbHandle`; expose `Storage::handle() -> &DbHandle`.
- `ResolutionConfig` struct in new `resolution/config.rs` with the fields above.
- `Worker` struct in new `resolution/worker.rs`:
  - Owns: `Receiver<PipelineJob>`, its own `Connection` (opened from handle), and the existing stage functions (drive_extract / drive_resolve / drive_persist — already exist per `resolution/queue.rs`).
  - `run()` loop: dequeue → run stages → loop. On dequeue error (queue closed), exit cleanly.
- **No Memory integration in D1a.** Worker is constructible + drivable from a test harness only.
- **One integration test** (`tests/resolution_worker_d1a.rs`):
  - Open tempfile DB → wrap in `DbHandle` → spawn 1 worker → push 1 scripted `PipelineJob` → assert graph rows materialize → close queue → join worker.

**Acceptance:** test passes. Worker can be black-box driven from a test without touching `Memory`. PRAGMA code exists in exactly one place.

**Out of scope:** Memory integration, dispatch (always worker 0), `:memory:` handling, shutdown semantics beyond "queue closed → exit".

### D1b — Memory integration + lifecycle

**Scope:** ~200 LOC.

- `MemoryConfig` gains `resolution: ResolutionConfig`.
- `Memory::start_resolution_workers(&mut self)`:
  - If `handle.is_in_memory()` → log info "in-memory DB: running resolution inline, workers disabled" → set internal flag → return Ok.
  - Else → spawn `config.resolution.worker_count` threads, each with its own connection from `self.storage.handle()`. Stash JoinHandles in `Memory::resolution_workers: Vec<JoinHandle<()>>`.
- `Memory::shutdown_resolution_workers(&mut self)`:
  - Close the job sender → join with 5s timeout per worker → log + force-detach on timeout.
- `store_raw` already enqueues `PipelineJob` per existing code (queue.rs). Verify this path: in worker mode, push to channel; in inline mode (in-memory or worker_count=0), call stages inline on the caller thread (already the v0.2-compat fallback path — verify it still works).
- **Integration tests** (in `tests/resolution_pipeline_d1b.rs`):
  1. Tempfile DB, worker_count=1: `store_raw` → poll `extraction_status` until `Completed` → assert graph rows. End-to-end.
  2. `:memory:` DB: `start_resolution_workers` succeeds, log captured shows inline-mode notice, `store_raw` still produces graph rows synchronously.
  3. Shutdown test: `start` → enqueue 5 jobs → `shutdown` → assert all 5 either completed or surface `Failed { kind: WorkerShutdown }`. No hangs, no orphan threads.

**Acceptance:** 3 tests pass. v0.2 compat tests still green (regression guard per §9.4).

**Out of scope:** N>1 dispatch, session-affinity, crash recovery.

### D2 — Multi-worker dispatch + session affinity

**Scope:** TBD (estimate 300–400 LOC; if it exceeds, split D2a/D2b on the dispatch / recovery boundary).

- Replace single-channel design with N-receiver fan-out using `worker_id = hash(session_id) % N` (FxHash). Per design §5.1.1.
- Standalone-memory case: `worker_id = hash(memory_id) % N`.
- Worker crash recovery on startup: scan `graph_pipeline_runs` for `queued` → re-enqueue. In-flight → mark `Failed(worker_crashed)`.
- Property test from §9.3 / GOAL-2.4 across N ∈ {1, 2, 4}.

**Acceptance:** GOAL-2.4 property test passes at N=4. No flakes over 100 runs.

---

## Open Questions (to resolve before D2, not blocking D1)

1. **Queue type.** Design §5.1 says "bounded crossbeam channel". Crossbeam is already a transitive dep via rayon — confirm before adding direct dep.
2. **Telemetry timing.** Where exactly does the body-signal bus emit happen for worker-internal stages? §3.6 implies stage-level emission; needs a concrete `WriteStatsSink` plumbing decision in D1b.
3. **Backpressure on queue full.** Design §5.2 covers this; map to `crossbeam::SendError` handling in D1b's `store_raw` path.

---

*Phase status updated as work lands. Last updated: D1a not started.*
