---
id: ISS-092
title: Refactor SqliteGraphStore::namespace — remove mutable field, prevent namespace-drift bug class at compile time (Direction C, ISS-091 follow-up)
status: todo
priority: P2
severity: medium
tags: [refactor, sqlite-graph-store, namespace, type-safety, ergonomics]
relates_to: [ISS-055, ISS-091]
---

# ISS-092: Remove `SqliteGraphStore::namespace` mutable field — eliminate the bug class

## Background

ISS-091 fixed the **specific** begin/finish_pipeline_run namespace asymmetry by stamping `set_namespace(ctx.namespace)` before `begin_pipeline_run_for_memory` (under the same lock window per ISS-055 lock-and-stamp contract). That's a **tactical** fix — it patches one call site.

The **structural** problem remains: `SqliteGraphStore` carries a `namespace: String` field that's mutated mid-flight by `set_namespace(...)`. ~30 call sites in `crates/engramai/src/graph/store.rs` read `self.namespace` directly. Any future caller that:

1. Forgets to `.with_namespace()` at construction → silently writes to `"default"` namespace.
2. Calls a write/read method without first stamping the per-job namespace → operates against whatever the previous caller left in `self.namespace`.
3. Adds a new method using `self.namespace` → re-introduces the same drift bug class ISS-091 just patched.

This is a **default-value foot-gun**. ISS-091's tactical fix doesn't prevent the next instance.

## Goal

Make namespace-drift impossible at compile time (or at minimum, impossible to silently misuse at runtime).

## Two viable directions

### Option A — `namespace` as a per-call parameter

Remove `namespace: String` from `SqliteGraphStore`. Every method that needs it takes `namespace: &str` as a parameter:

```rust
fn begin_pipeline_run_for_memory(&mut self, namespace: &str, kind: ..., ...) -> Result<Uuid, GraphError>;
fn finish_pipeline_run(&mut self, namespace: &str, run_id: Uuid, ...) -> Result<(), GraphError>;
// ... and so on for ~30 methods that touch namespace
```

**Pros:**
- Conceptually simple — store is stateless w.r.t. namespace.
- Hard to call wrong: if you don't pass namespace, it doesn't compile.
- Matches the pipeline's actual design: namespace is per-job, not per-store.

**Cons:**
- Touches every namespace-dependent method signature in `GraphRead` + `GraphWrite` traits + every impl + every caller.
- Trait-object users (`Box<dyn GraphWrite>`) need every call site updated — large surface.
- Some methods take `self.namespace` indirectly via helpers — those need refactoring too.

### Option B — Typestate: `SqliteGraphStore<Unbound>` → `SqliteGraphStore<Bound>`

Keep an internal namespace field but make it part of the type:

```rust
pub struct SqliteGraphStore<'a, NS = Unbound> { ... }
pub struct Unbound;
pub struct Bound(String);

impl<'a> SqliteGraphStore<'a, Unbound> {
    pub fn new(conn: ...) -> Self { ... }
    pub fn bind(self, namespace: String) -> SqliteGraphStore<'a, Bound> { ... }
}

impl<'a> SqliteGraphStore<'a, Bound> {
    // Only this state exposes begin/finish/persist methods.
    // `set_namespace` becomes `rebind(self, ns) -> Self` — consuming, returning a new Bound.
}
```

**Pros:**
- Compile-time prevention: "use store before binding namespace" is a type error.
- `rebind` is consuming → no mid-flight mutation under shared ownership; if you rebind mid-job, you lose the old store, making the bug structurally impossible.
- Encodes the actual invariant in the type system.

**Cons:**
- Trait-object callers can't carry the typestate parameter — `dyn GraphWrite` would need to be `dyn GraphWrite<Bound>` everywhere, or the trait would split (`GraphWriteBound` vs `GraphWriteUnbound`).
- More machinery; harder to read for someone unfamiliar with typestate patterns.
- Existing `Arc<Mutex<SqliteGraphStore<'static>>>` usage in `Memory` (where the same store is shared across runs) doesn't fit a consuming `rebind` cleanly — would need `Arc<Mutex<Either<Unbound, Bound>>>` or similar.

### Recommendation

**Lean Option A** (per-call parameter) for these reasons:
- It matches what pipeline.rs already does conceptually (namespace lives on `JobContext`, not the store).
- Trait surface is verbose but mechanically simple — large find-and-replace, no clever generics.
- Removes ALL mutable state related to namespace from the store, not just the begin/finish window.
- Typestate approach (Option B) collides with the `Arc<Mutex<…>>` sharing pattern — would force a broader rearchitecture.

But this is a big refactor; design discussion required before committing.

## Acceptance criteria

- [ ] **AC-1**: Decision recorded for Option A vs B (in this issue or a sibling design doc) with reasoning.
- [ ] **AC-2**: `SqliteGraphStore` no longer has a mutable `namespace` field (Option A) OR `set_namespace`/mid-flight mutation removed (Option B with consuming rebind).
- [ ] **AC-3**: ISS-091's `set_namespace(ctx.namespace)` stamping in `pipeline.rs` becomes unnecessary — the new API makes it impossible to call begin/finish without specifying namespace per-call.
- [ ] **AC-4**: All ~30 `self.namespace` call sites in `crates/engramai/src/graph/store.rs` migrated.
- [ ] **AC-5**: ISS-091's regression test (`finish_pipeline_run_errors_when_namespace_drifted_since_begin`) becomes either a compile error (Option A — can't construct the drift state) or removed/replaced with an equivalent runtime check.
- [ ] **AC-6**: Full `cargo test -p engramai --release` passes.
- [ ] **AC-7**: ISS-091's stderr `"namespace drift"` diagnostic in `finish_pipeline_run` removed (no longer reachable).

## Out of scope

- Other store implementations (only `SqliteGraphStore` is in scope; in-memory test stubs follow once the trait is settled).
- Performance — this is a correctness/ergonomics refactor, not perf.
- ISS-091's `eprintln!` diagnostic — removed as part of this work, not patched separately.

## Risk

**High**. Touches the core trait surface (`GraphRead` + `GraphWrite`) and every caller of those traits across `engramai`, `engram-cli`, `cogmembench` adapters, and tests. Suggested approach:

1. Branch off `main` (post-ISS-091).
2. Sketch the new trait signatures (Option A) in a design doc.
3. Migrate one method at a time (start with `begin_pipeline_run_for_memory` since it's already in everyone's head from ISS-091).
4. Each migration step keeps tests passing — no big-bang.
5. Last step: remove the `namespace` field + `set_namespace` from the trait + impl.

## Why P2 (not P0/P1)

ISS-091's tactical fix is in production (commit `38c38fe`). Conv26 ingest works. No active blocker. This refactor is **defense-in-depth** — preventing future bugs of the same shape — not fixing a current break. P2 = important but not urgent. Can be picked up after current eval/J-score work (ISS-085) settles.
