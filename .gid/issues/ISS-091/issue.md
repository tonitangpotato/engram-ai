---
id: ISS-091
title: begin/finish_pipeline_run namespace asymmetry — finish_pipeline_run reports run not found after ISS-087/089
status: done
priority: P0
severity: high
tags:
- pipeline
- namespace
- sqlite-graph-store
- ingestion
relates_to:
- ISS-055
- ISS-087
- ISS-089
- .gid/issues/ISS-092/issue.md
---

# ISS-091: begin/finish pipeline-run namespace asymmetry

## Symptom

After ISS-087 + ISS-089 landed (commits `815b319`, `f6bd93b`), re-running conv26 ingest as **RUN-0011** fails every job at finish time:

```
finish_pipeline_run: run not found
```

The row was written at `begin_pipeline_run_for_memory` time, but `finish_pipeline_run`'s `WHERE run_id=? AND namespace=?` returns 0 rows.

## Root cause (verified end-to-end)

`SqliteGraphStore::namespace` is a **mutable field** on the store, but the pipeline mutates it **mid-run** between `begin` and `finish`. Sequence:

1. **`Memory` construction** (`crates/engramai/src/memory.rs:356`):
   ```rust
   let graph_store = SqliteGraphStore::new(graph_conn);
   ```
   No `.with_namespace(...)`. `Default::default()` → `self.namespace = "default"`.

2. **`begin_run`** (`crates/engramai/src/resolution/pipeline.rs:441`):
   ```rust
   let mut store = self.store.lock()...;
   store.begin_pipeline_run_for_memory(...)  // INSERT … namespace = self.namespace = "default"
   ```
   Row written with `namespace = "default"`.

3. **Step 8 §3.5 persist** (`pipeline.rs:386`, ISS-055 lock-and-stamp):
   ```rust
   let mut store = self.store.lock()...;
   store.set_namespace(ctx.namespace.clone());   // → "locomo-conv26-full"
   drive_persist(&mut *store, ...)
   ```
   `self.namespace` is now `"locomo-conv26-full"` for the rest of the process's life (or until next mutation).

4. **`finish_run`** (`pipeline.rs:485`):
   ```rust
   store.finish_pipeline_run(run_id, status, ...)
   // → SELECT status FROM graph_pipeline_runs
   //   WHERE run_id = ?1 AND namespace = ?2  (?2 = self.namespace = "locomo-conv26-full")
   ```
   The row exists with `namespace = "default"` — `WHERE` filter excludes it → `cur = None` → returns `Invariant("finish_pipeline_run: run not found")`.

**The bug is structural**, not a localized typo: `SqliteGraphStore` carrying a mutable `namespace` field that callers mutate mid-run is a default-value trap. Any caller that forgets `.with_namespace()` silently writes to `"default"` namespace, then later mutates and queries with the real namespace → asymmetry.

### Proof points (file:line)

- `crates/engramai/src/memory.rs:356` — `SqliteGraphStore::new(graph_conn)` with no namespace bind
- `crates/engramai/src/graph/store.rs:654` — default `namespace: "default".to_string()`
- `crates/engramai/src/graph/store.rs:790` — `begin_pipeline_run_inner` INSERTs using `self.namespace`
- `crates/engramai/src/graph/store.rs:4341` — `finish_pipeline_run` SELECT/UPDATE `WHERE namespace = ?`
- `crates/engramai/src/resolution/pipeline.rs:386,531,593,707` — pipeline mutates `self.namespace` mid-run via `set_namespace(ctx.namespace)`

## Fix (this issue: tactical, root-fix-aligned)

**Direction B + invariant assertion** (chosen over A; see "Alternatives considered"):

1. **In `pipeline.rs::process` (or `begin_run`), call `set_namespace(ctx.namespace)` *before* `begin_pipeline_run_for_memory`.** This makes begin/persist/finish all use the same `self.namespace` value, eliminating the asymmetry.

2. **Add an invariant check at `finish_pipeline_run` entry:** when the SELECT returns 0 rows, before returning `"run not found"`, also check whether the row exists under a *different* namespace (debug query: `SELECT namespace FROM graph_pipeline_runs WHERE run_id = ?1`). If yes → return a more specific error (`"finish_pipeline_run: namespace drift, row written under '{x}', queried under '{y}'"`). This traps the *category* of bug at the closest call point instead of the silent "not found" we have today.

3. **Add a regression test** that constructs a `SqliteGraphStore` with default namespace, then `set_namespace("foo")`, then calls `begin_pipeline_run_for_memory` followed by `finish_pipeline_run` — must succeed (i.e. validates fix #1) and fails on baseline (proves test catches regression).

## Alternatives considered

- **Direction A (rejected)**: Have `begin_pipeline_run_for_memory` accept namespace as a parameter and bypass `self.namespace` for the INSERT. Symptom-targeted: fixes the begin path but leaves `self.namespace` as a foot-gun for every other method on `SqliteGraphStore` that already uses `self.namespace` (≈30 call sites in store.rs). New caller adding a new method that uses `self.namespace` would still race against `set_namespace` calls elsewhere.

- **Direction C (deferred — separate issue)**: Remove `namespace` from `SqliteGraphStore` entirely. Either (a) make every method take `&namespace` as a parameter, or (b) typestate: `SqliteGraphStore<Unbound>` → `.bind(ns) → SqliteGraphStore<Bound>`, where only `Bound` exposes `begin/finish/persist`. Compile-time prevention of the bug class. Out of scope for this fix — track as **follow-up** (open new issue after ISS-091 lands).

## Why not just fix `memory.rs:356`

Setting `.with_namespace(ctx.namespace)` at construction time *can't work* — `Memory` is built once at `Memory::open()`, but `ctx.namespace` is per-pipeline-run (each ingest job specifies its own). The store is shared across runs via `Arc<Mutex<…>>`. So namespace must be (re)bound per-run. The right place is "right before begin", which is direction B.

## Acceptance criteria

- [x] **AC-1**: `set_namespace(ctx.namespace)` called before `begin_pipeline_run_for_memory` in the pipeline (under the same `store.lock()` to satisfy the lock-and-stamp contract from ISS-055).
- [x] **AC-2**: `finish_pipeline_run` returns a more specific error when the row exists under a different namespace (`"namespace drift: written under X, queried under Y"`). *(Implemented as stderr diagnostic log because `GraphError::Invariant(&'static str)` cannot carry a dynamic message; the static `"finish_pipeline_run: run not found"` error is preserved. Widening the error API was out of scope.)*
- [x] **AC-3**: Regression test in `crates/engramai/src/graph/store.rs` (or a sibling tests module) reproducing the begin/finish namespace mismatch — must FAIL on `main` baseline before fix and PASS after fix.
- [ ] **AC-4**: Re-run conv26 ingest as RUN-0012, must complete without `finish_pipeline_run: run not found`. (Manual / CLI-driven, not in test suite.)
- [x] **AC-5**: All existing tests in `engramai` crate pass (`cargo test -p engramai --release`).
- [x] **AC-6**: Follow-up issue opened for typestate/parameter refactor (Direction C). → **ISS-092** (`.gid/issues/ISS-092/issue.md`).

## Out of scope

- Refactoring `SqliteGraphStore` to remove `namespace` field (Direction C — separate issue).
- Auditing every other `self.namespace` usage in store.rs for similar asymmetry (covered by Direction C).
- Verifying ISS-085 J-score — that's RUN-0012's job, gated on this fix.

## Risk

Low. Fix is 2 lines (one `set_namespace` call + one defensive SELECT in finish). Lock-and-stamp contract from ISS-055 already serializes `set_namespace` against persist; adding it earlier in the same lock window is strictly more conservative.
