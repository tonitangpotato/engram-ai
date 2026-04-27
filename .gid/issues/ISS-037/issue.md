---
id: "ISS-037"
title: "v03-resolution: Pipeline Wire-up Blockers (Connection Ownership, Storage Privacy, Sync Bound)"
status: open
priority: P1
created: 2026-04-26
---

# ISS-037: v03-resolution Pipeline Wire-up Blockers

**Status:** Open
**Priority:** P1 (blocks v03-resolution end-to-end integration)
**Created:** 2026-04-26
**Affects:** `crates/engramai/src/memory.rs`, `crates/engramai/src/storage.rs`, `crates/engramai/src/resolution/pipeline.rs`

---

## Context

`ResolutionPipeline<S: GraphStore + 'static>` was completed 2026-04-26 (645 LOC, commit `a065d0d`). The orchestration layer impls `JobProcessor`, ties all 6 stages together, and is generic-safe.

Wiring it into `Memory::store_raw` so enqueued `PipelineJob::initial(...)` jobs are *actually drained* by a `WorkerPool` requires solving three structural problems discovered while attempting wire-up tonight.

## Blocker 1: `SqliteGraphStore<'a>` cannot satisfy `'static`

**Problem:** `SqliteGraphStore<'a> { conn: &'a mut Connection, .. }` borrows the connection. Pipeline requires `S: GraphStore + 'static` because `WorkerPool` holds `Arc<dyn JobProcessor>` (implicit `'static`).

**Resolution path (root fix):**
Use `Box::leak(Box::new(Connection::open(path)?))` to obtain `&'static mut Connection`, then construct `SqliteGraphStore<'static>`. The leaked connection lives = process lifetime, which matches its actual usage. Rusqlite `Connection: Send`, so `SqliteGraphStore<'static>: Send`.

**Why not refactor `SqliteGraphStore` to own the Connection:** 6614-line file, 155 call sites, 9 explicit type annotations. Blast radius too large for the value gained. The leak is correct semantics — pipeline connection IS process-lifetime — not a hack.

**Estimated effort:** ~5 lines in builder.

## Blocker 2: `Memory: !Sync`, blocks `Arc<dyn MemoryReader>`

**Problem:** `MemoryReader: Send + Sync`. `Memory` contains `Storage { conn: Connection }`. `Connection: Send` but not `Sync`. So `Memory: Send` but `!Sync` → cannot be wrapped in `Arc<dyn MemoryReader + Sync>` for sharing across worker threads.

**Resolution path:**
Introduce a dedicated `SqliteMemoryReader` type that holds `Mutex<Connection>` (a *separate* connection from `Memory.storage.conn`, opened against the same db file — sqlite WAL mode handles concurrency). Reader is constructed alongside the worker pool.

**Sub-blocker:** SQL for fetching `MemoryRecord` lives inside `Storage::get` (`storage.rs:1037`) which is only callable via `&self` on a `Storage` instance — and `Storage` owns its `Connection`. Two options:
- (a) Refactor `Storage::get` to take `&Connection` parameter (cleaner; ~30 lines), then `SqliteMemoryReader` reuses it
- (b) Duplicate the SQL inside `SqliteMemoryReader::fetch` (faster; risk of drift)

**Recommendation:** (a). The SQL + `row_to_record` mapper is ~50 lines and absolutely should not be duplicated.

**Estimated effort:** Storage refactor ~30 LOC + SqliteMemoryReader ~50 LOC = ~80 LOC.

## Blocker 3: Missing `impl MemoryReader for ...`

**Problem:** `pipeline.rs` line 86 says "*The blanket impl below covers `crate::memory::Memory` for production callers*" but no impl exists.

**Resolution path:** Falls out of Blocker 2 — `SqliteMemoryReader` IS that production impl.

## Plan (next session)

Sequence:

1. **Refactor `Storage::get` and `Storage::get_by_ids`** to expose internal SQL via a free function `fn fetch_memory_record(conn: &Connection, id: &str) -> Result<Option<MemoryRecord>, rusqlite::Error>` (Blocker 2a)
2. **Implement `SqliteMemoryReader`** in `resolution/memory_reader.rs` holding `Mutex<Connection>` — opens its own connection to the same db path
3. **Add `Memory::with_pipeline_pool(triple_extractor: Box<dyn TripleExtractor>) -> Self` builder** that:
   - Opens leaked `&'static mut Connection` for graph operations
   - Constructs `SqliteGraphStore<'static>` → `Arc<Mutex<...>>`
   - Constructs `SqliteMemoryReader` against same db path
   - Builds `ResolutionPipeline`
   - Wraps in `Arc<dyn JobProcessor>`
   - Constructs `BoundedJobQueue`
   - Calls `WorkerPool::start(...)`
   - Stores pool handle in `Memory` (new field) for shutdown
4. **Wire `Memory::shutdown()` or `Drop`** to call `WorkerPool::shutdown(deadline)` — drain queue cleanly
5. **End-to-end test:** `Memory::with_pipeline_pool` → `store_raw("Alice met Bob in Paris")` → wait for pipeline → assert graph has Alice/Bob/Paris entities + relations (mock `TripleExtractor` for determinism)

**Estimated total:** ~250 LOC implementation + ~150 LOC test.

## Decision Log

- **Considered**: refactor `SqliteGraphStore` to own `Connection` (generic over `BorrowMut<Connection>`). Rejected: 155 call sites + 6614-line file is too much blast radius for what is fundamentally a wire-up problem, not a Connection-ownership problem.
- **Considered**: Mock-only end-to-end test (no SQLite). Rejected: doesn't validate real pipeline + sqlite integration; defeats the purpose of an end-to-end test. Mock will still be used inside the test for `TripleExtractor` (LLM determinism).

## Related

- Pipeline orchestration commit: `a065d0d` (2026-04-26)
- v03-resolution feature: `.gid/features/v03-resolution/`
- Worker pool: `crates/engramai/src/resolution/worker.rs`
- Pipeline impl: `crates/engramai/src/resolution/pipeline.rs`
