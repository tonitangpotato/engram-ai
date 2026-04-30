---
issue: ISS-040
title: "SqliteGraphStore<C: Borrow<Connection>> generic refactor — design"
status: REVISED
blocker: public API evolution decision (Memory::graph_mut) + interaction with existing with_graph_read
author: rustclaw (autopilot 2026-04-29 A7.4)
---

# Design: SqliteGraphStore generic over connection ownership

> **Status: REVISED.** This is a proposal, not approved. The autopilot
> A7.3 threshold check tripped (27 type-level callsites + Memory::graph_mut
> is public API). Implementation is deferred until potato signs off on the
> public-API evolution strategy in §6 below.
>
> **Revision note (r1 review):** §1, §4, §6 substantially rewritten after
> review found two critical factual errors: `Storage::connection(&self)`
> already exists (not a prereq), and `graph_store` field is actively populated
> via `Box::leak` (not dead code). The core refactor (Borrow/BorrowMut split)
> remains sound, but the design's framing and motivation have been corrected.

## 1. Problem statement

Today (`crates/engramai/src/graph/store.rs:620`):

```rust
pub struct SqliteGraphStore<'a> {
    pub(crate) conn: &'a mut rusqlite::Connection,
    // ...
}
impl<'a> SqliteGraphStore<'a> {
    pub fn new(conn: &'a mut rusqlite::Connection) -> Self { ... }
}
```

Two access paths exist in `Memory` today:

1. **`Memory::graph_mut(&mut self)`** — the primary accessor, returns
   `SqliteGraphStore<'_>` borrowing `storage.connection_mut()`. Used by
   direct callers in prod and tests.

2. **`Memory::with_graph_read(&self, f: impl FnOnce(&dyn GraphRead) -> R)`**
   — a closure-based read-only accessor that borrows the `graph_store:
   Option<Arc<Mutex<SqliteGraphStore<'static>>>>` field through a mutex
   guard. This field is **actively populated** by `with_pipeline_pool`
   (memory.rs:356) and `with_graph_store` (memory.rs:516), both of which
   achieve `'static` by leaking a connection via `Box::leak(Box::new(conn))`.
   This is the production read path for v0.3 retrieval plans.

The `Box::leak` + `Arc<Mutex<...>>` + closure pattern works, but it is a
heavyweight workaround for a simple problem: `SqliteGraphStore` requires
`&mut Connection` even for read-only operations. The real consequences:

1. **`Box::leak` is a permanent memory leak** — the leaked connection is
   never reclaimed. This is acceptable for long-lived `Memory` instances
   but is a design smell and prevents clean shutdown.

2. **The closure-based `with_graph_read` API is ergonomically poor** —
   callers cannot hold a graph reader across `.await` points or return
   it from a method. Every consumer must restructure code into a closure.

3. **Two separate connections for reads vs writes** — the leaked connection
   in `graph_store` and `storage.connection_mut()` are *different*
   connections. Under WAL mode this is safe for read/write isolation, but
   it means reads may see stale data until the writer commits.

4. **`graph_mut(&mut self)` forces `&mut` propagation** — read-only callers
   that don't use `with_graph_read` (e.g., `extraction_status`) must take
   `&mut self` purely to satisfy the borrow checker, which transitively
   forces every up-stack caller to `&mut`.

What ISS-040 enables: a **simpler alternative** where `Memory::graph(&self)`
returns a concrete `SqliteGraphStoreReader<'_>` borrowing the existing
`Storage::connection(&self)` accessor (storage.rs:300, already shipped and
actively used by ~20 callers). This would:

- Eliminate the need for `Box::leak` in new code paths
- Provide a direct, non-closure graph reader from `&self`
- Coexist with or eventually replace `with_graph_read`

```rust
// Owned (today's case, kept compatible)
let mut store: SqliteGraphStore<rusqlite::Connection> = SqliteGraphStore::new(conn);

// Borrowed read-only (new, simplifies Memory::graph(&self))
let store: SqliteGraphStore<&rusqlite::Connection> = SqliteGraphStore::borrowed(&conn);
```

…with the same trait impls (`GraphRead`, `GraphWrite`)
gated by what `C: Borrow<Connection>` vs `BorrowMut<Connection>` permits.

## 2. Proposed signature

```rust
use std::borrow::{Borrow, BorrowMut};

pub struct SqliteGraphStore<C: Borrow<rusqlite::Connection>> {
    pub(crate) conn: C,
    pub(crate) namespace: String,
    pub(crate) sink: Box<dyn TelemetrySink>,
    pub(crate) watermark: WatermarkTracker,
    pub(crate) predicate_use_buffer: HashMap<String, u64>,
    pub(crate) embedding_dim: usize,
}

// All read methods only need `Borrow`
impl<C: Borrow<rusqlite::Connection>> SqliteGraphStore<C> {
    pub fn borrowed(conn: C) -> Self { ... }
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self { ... }
    pub fn with_sink(mut self, sink: Box<dyn TelemetrySink>) -> Self { ... }
    pub fn with_embedding_dim(mut self, dim: usize) -> Self { ... }

    // GraphRead methods (resolve_alias, get_entity, traverse, …) live here
    fn conn(&self) -> &rusqlite::Connection { self.conn.borrow() }
}

// Mutating methods need `BorrowMut`
impl<C: BorrowMut<rusqlite::Connection>> SqliteGraphStore<C> {
    pub fn new(conn: C) -> Self { ... }              // kept for compat (alias for ::owned in §3)
    fn conn_mut(&mut self) -> &mut rusqlite::Connection { self.conn.borrow_mut() }

    // GraphWrite methods (apply_graph_delta, upsert_entity, …) live here
}
```

**Trait impl split (matches existing module split):**

- `impl<C: Borrow<Connection>> GraphRead for SqliteGraphStore<C>`
- `impl<C: BorrowMut<Connection>> GraphWrite for SqliteGraphStore<C>`
- `GraphStore` is blanket-implemented for any `T: GraphWrite` (via
  `impl<T: GraphWrite + ?Sized> GraphStore for T {}` at store.rs:603),
  so only `GraphRead` and `GraphWrite` impls are needed — no explicit
  `impl GraphStore` line.

This is the same pattern `rusqlite::Connection` and similar libraries
use; nothing exotic.

## 3. Backwards-compat: type aliases + ::new keeps working

Existing callers use one of two shapes — both must keep compiling:

```rust
// Shape A (most common, ~140 callsites):
let mut store = SqliteGraphStore::new(&mut conn);

// Shape B (struct field with explicit lifetime, used in test helpers):
fn helper<'a>(conn: &'a mut Connection) -> SqliteGraphStore<'a> { ... }
```

**Compat strategy:**

```rust
/// Owned-mut alias: matches today's `SqliteGraphStore<'a>` where `&'a mut Connection`
/// is the only ownership shape. Provided so downstream code that names the
/// concrete type continues to compile after the generic param is added.
pub type SqliteGraphStoreOwned<'a> = SqliteGraphStore<&'a mut rusqlite::Connection>;

/// Read-only borrow alias: new shape, unlocks `Memory::graph(&self)`.
pub type SqliteGraphStoreReader<'a> = SqliteGraphStore<&'a rusqlite::Connection>;
```

> **Naming note:** The `Owned` / `Reader` naming is asymmetric — "Owned"
> describes ownership while "Reader" describes capability. More consistent
> alternatives: `SqliteGraphStoreMut` / `SqliteGraphStoreRef` (borrow-based)
> or `SqliteGraphStoreWriter` / `SqliteGraphStoreReader` (capability-based).
> Final naming can be decided during implementation.

`SqliteGraphStore::new` keeps its current signature (`&mut Connection -> Self`)
because of type inference: when the caller writes `SqliteGraphStore::new(&mut conn)`,
the compiler resolves `C = &mut Connection` automatically. So **Shape A
callers do not change** — zero diff in 140+ test/prod callsites.

Shape B (explicit `<'a>` in signatures) becomes:

```rust
fn helper<'a>(conn: &'a mut Connection) -> SqliteGraphStoreOwned<'a> { ... }
```

…a one-token rename. There are 27 such sites (see callsites.txt).

**Naming trade-off note:** The issue (`issue.md`) proposes an alternative
naming strategy: rename the struct to `SqliteGraphStoreT<C>` and keep
`SqliteGraphStore<'a>` as a type alias — achieving zero-diff for Group 2
(27 sites) as well. This design chose to keep the struct name
`SqliteGraphStore<C>` because: (a) the `T` suffix is a non-idiomatic Rust
naming convention, and (b) 27 mechanical renames are low-risk and
sed-able. However, if minimizing diff is the priority, the issue's
`SqliteGraphStoreT` approach is strictly more backward-compatible.
Decision deferred to implementation.

## 4. Memory::graph(&self) — the payoff

After this refactor:

```rust
impl Memory {
    /// Read-only graph view. Available with `&self` (no exclusive borrow).
    pub fn graph(&self) -> SqliteGraphStoreReader<'_> {
        SqliteGraphStore::borrowed(self.storage.connection())
            .with_namespace(self.namespace.clone())
            .with_embedding_dim(self.embedding_dim)
    }

    /// Mutating graph view. Unchanged signature, unchanged semantics.
    pub fn graph_mut(&mut self) -> SqliteGraphStoreOwned<'_> {
        SqliteGraphStore::new(self.storage.connection_mut())
            .with_namespace(self.namespace.clone())
            .with_embedding_dim(self.embedding_dim)
    }
}
```

Uses existing `Storage::connection(&self) -> &Connection` (storage.rs:300),
which is already shipped and actively used by ~20 callers across tests and
prod code. No new accessor needed.

### 4.1. Relationship with existing `with_graph_read` / `Arc<Mutex<...>>` pattern

After this refactor, `Memory` will have **two** read-only graph access paths:

1. **`Memory::graph(&self) -> SqliteGraphStoreReader<'_>`** (new) — borrows
   `storage.connection()`. Direct, no mutex, no closure. Uses the *main*
   storage connection.

2. **`Memory::with_graph_read(&self, f: impl FnOnce(&dyn GraphRead) -> R)`**
   (existing) — borrows the `graph_store: Arc<Mutex<SqliteGraphStore<'static>>>`
   field through a closure + mutex guard. Uses a *separate leaked connection*
   installed by `with_pipeline_pool` or `with_graph_store`.

**Key difference:** These may read from **different connections** — the main
storage connection vs the leaked graph-db connection. Under WAL mode both
see committed state, but the leaked connection may point to a *different
database file* (when `with_pipeline_pool(graph_db_path)` is called with a
path distinct from the main memories DB).

**Migration strategy (requires potato decision):**

- **Option A (coexist):** Keep `with_graph_read` for retrieval plans that
  use the dedicated graph-db connection. `Memory::graph(&self)` serves
  callers that want to read the *main* DB's graph tables. Document the
  distinction.
- **Option B (deprecate `with_graph_read`):** Migrate retrieval plan callers
  to `Memory::graph(&self)`. This only works if the graph tables are in
  the main DB (not a separate file). Would eliminate the `Box::leak`
  pattern entirely.
- **Option C (replace internals):** Keep the `with_graph_read` API surface
  but reimplement it using the new generic store internally, removing the
  `Box::leak`. The `Arc<Mutex<...>>` field would hold a
  `SqliteGraphStore<Connection>` (owned) instead of `SqliteGraphStore<'static>`.

## 5. Callsite migration plan

(From `engram/.gid/issues/ISS-040/callsites.txt`, 2026-04-29 grep.)

**Group 1 — zero-diff (140+ sites):** every `SqliteGraphStore::new(&mut conn)`
or `::with_namespace(...)` chain in tests and prod. Type inference handles it.

**Group 2 — type signatures (27 sites, 5 files):** rename `SqliteGraphStore<'a>`
in `fn` signatures and struct fields to `SqliteGraphStoreOwned<'a>`. Pure
mechanical sed-able rewrite. Files:

- `crates/engramai/src/graph/store.rs` (9, includes definition)
- `crates/engramai/src/memory.rs` (12)
- `crates/engramai-migrate/src/cli.rs` (3)
- `crates/engramai-migrate/src/processor.rs` (2)
- `crates/engramai/src/graph/test_helpers.rs` (1)

**Group 3 — new read-only consumers (post-refactor work):** retrieval,
scoring, knowledge_compile read paths can opt in to `Memory::graph(&self)`
incrementally. Out of scope for ISS-040; tracked as follow-up.

## 6. ⚠️ Open questions for potato (BLOCKER)

These are the reasons A7 stays deferred until you weigh in:

**Q1. Public API: keep `Memory::graph_mut` signature stable?**
   - Option A (recommended): `graph_mut` returns `SqliteGraphStoreOwned<'_>`
     (= today's `SqliteGraphStore<'_>` semantically). Add new `graph(&self)
     -> SqliteGraphStoreReader<'_>`. **Source-compatible** for downstream
     code; only the type *spelling* in error messages changes.
   - Option B: rename `graph_mut` → `graph_writer`, add `graph_reader`. Cleaner
     names but breaks every existing `memory.graph_mut()` caller. Requires
     semver bump.

**~~Q2. Storage::connection(&self) accessor — safe to add?~~**
   ✅ **Closed — already exists.** `Storage::connection(&self) -> &Connection`
   is at `storage.rs:300` and has ~20 active callers in tests and prod.
   No action needed.

**~~Q3. Migrate engramai-migrate crate too, or pin it to old API?~~**
   ✅ **Closed — answered in §3/§5.** `::new(&mut conn)` keeps working via
   type inference (zero changes for method calls). The 3 type-signature
   sites in engramai-migrate need only a mechanical one-token rename to
   `SqliteGraphStoreOwned<'a>`. No potato input needed.

**Q4. Trait split: separate `GraphRead` / `GraphWrite` in the public surface?**
   - Today `GraphStore: GraphRead + GraphWrite` is one trait users import.
     After refactor, code that only needs reads can `use GraphRead` alone.
     Document this in the module docs?

**Q5. (NEW) Relationship between `Memory::graph(&self)` and `Memory::with_graph_read`?**
   - See §4.1 for full analysis. The new `graph(&self)` and existing
     `with_graph_read` use **different connections** (main storage vs leaked
     graph-db). Three options:
     - Option A: Coexist — document the two-connection semantics.
     - Option B: Deprecate `with_graph_read` — only works if graph tables
       are in the main DB.
     - Option C: Replace `with_graph_read` internals using the new generic
       store, eliminating `Box::leak`.
   - This is load-bearing for scope: Option A is zero extra work, Option C
     could add 1–2 hours.

## 7. Test plan

1. **Existing tests must pass unchanged.** All 140+ `SqliteGraphStore::new`
   callsites should compile and pass with zero edits. This is the
   regression gate.
2. **New: `Memory::graph(&self)` smoke test.** Open a Memory, call
   `mem.graph().get_entity(some_id)?` from a `&Memory` borrow. Compile-time
   evidence that read-only view works without exclusive borrow.
3. **New: concurrent-read sanity test.** Hold two `mem.graph()` views in
   the same scope; both can call read methods. (Will require `&Memory`
   to be `Send + Sync` if we want cross-thread; today Memory is single-
   threaded so just verify the borrow checker accepts it.)
4. **New: GraphWrite trait bound test.** Construct `SqliteGraphStore<&Connection>`
   (read-only borrow). Assert that calling a write method (e.g.
   `apply_graph_delta`) is a **compile error**. Use `compile_fail` doctest
   or `trybuild`.
5. **engramai-migrate integration test.** Run the existing migrate
   integration test unchanged; should pass without code edits.
6. **New: `SqliteGraphStoreOwned<'a>` drop-in test.** Verify that a function
   returning `SqliteGraphStoreOwned<'a>` compiles and behaves identically
   to old `SqliteGraphStore<'a>`. Covers Shape B callsite regression.

## 8. Acceptance criteria

- [ ] `cargo check -p engramai` passes after generic refactor
- [ ] `cargo check -p engramai-migrate` passes with **zero** edits to that crate
- [ ] All 281+ existing tests pass
- [ ] `Memory::graph(&self) -> SqliteGraphStoreReader<'_>` exists and is callable
- [ ] Compile-fail test: writing through `SqliteGraphStore<&Connection>` is rejected
- [ ] No new `clippy` warnings
- [ ] Docs updated in `graph/store.rs` module header explaining the generic
- [ ] potato signs off on Q1, Q4, and Q5 in §6

## 9. Out of scope

- Migrating retrieval/scoring/knowledge_compile to use `&self` read APIs —
  separate follow-up issue once `Memory::graph` lands.
- Reworking the active `graph_store: Option<Arc<Mutex<...>>>` field and
  `with_graph_read` accessor — the interaction with the new `graph(&self)`
  is a design decision (see §4.1, §6 Q5), not part of the core refactor.
  May be addressed in-scope if potato chooses Option B or C.
- Breaking `graph_mut` rename (Option B in §6 Q1) — only if potato
  explicitly chooses it.

## 10. Estimated effort (post-unblock)

- Implementation: ~2 hours (mechanical, mostly type-signature renames)
- Tests: ~1 hour (4 new tests + 1 compile-fail)
- Review: ~30 min
- **Subtotal: half a day** once §6 questions are answered.
- **If `with_graph_read` migration is chosen (§4.1 Option B/C):** add 1–2
  hours for deciding interaction with `graph_store` field, migrating
  retrieval plan callers, and updating/removing `Box::leak` patterns in
  `with_pipeline_pool` / `with_graph_store`.

Without sign-off → 0 hours, this stays at design-only.

## 11. Revision history

| Date | Rev | Changes |
|------|-----|---------|
| 2026-04-29 | r0 | Initial design (autopilot A7.4) |
| 2026-04-29 | r1 | **Major revision** after review found 2 critical factual errors. See below. |

### r1 revision details (applied from `.gid/issues/ISS-040/reviews/r1.md`)

**FINDING-1 (CRITICAL) — `Storage::connection(&self)` already exists:**
§4 falsely claimed this accessor needed to be added as a prereq. Reality:
`pub fn connection(&self) -> &Connection` exists at `storage.rs:300` with
~20 active callers. §4 prereq paragraph removed; §6 Q2 closed as
"already exists."

**FINDING-2 (CRITICAL) — `graph_store` field is NOT dead code:**
§1 falsely claimed the `graph_store: Option<Arc<Mutex<SqliteGraphStore<'static>>>>`
field is "always `None`" / dead code. Reality: it is actively populated by
`with_pipeline_pool` (memory.rs:356) and `with_graph_store` (memory.rs:516)
via `Box::leak(Box::new(conn))`, and `with_graph_read(&self)` (memory.rs:553)
provides the production read-only accessor for v0.3 retrieval plans.
**This means the design's core framing was wrong** — ISS-040's value is
simplifying the existing `Box::leak` workaround, not unlocking a capability
that doesn't exist. §1 fully rewritten; §4.1 added to address the
`with_graph_read` interaction; §6 Q5 added as new blocker question.

**FINDING-3 (IMPORTANT) — Redundant `impl GraphStore` line:**
§2 trait impl section corrected. `GraphStore` has a blanket impl
(`impl<T: GraphWrite + ?Sized> GraphStore for T {}`) so explicit impl is
dead code. Removed the line; added explanatory note.

**FINDING-4 (IMPORTANT) — `with_graph_read` / `Arc<Mutex<...>>` unaddressed:**
New §4.1 added with full analysis of the two-connection problem and three
migration options (coexist / deprecate / replace internals). Added §6 Q5
as a new blocker question requiring potato's decision.

**FINDING-5 (IMPORTANT) — Naming inconsistency with issue.md:**
Trade-off note added to §3 explaining why the design chose `SqliteGraphStore<C>`
(keep struct name) over the issue's `SqliteGraphStoreT<C>` (rename struct),
despite the latter being strictly more backward-compatible.

**FINDING-6 (IMPORTANT) — Q2/Q3 were not real blockers:**
§6 Q2 closed (accessor already exists). §6 Q3 closed (self-answered in §3/§5).
Blocker count reduced from 4 to 3 (Q1, Q4, new Q5).

**FINDING-7 (MINOR) — Missing test for `SqliteGraphStoreOwned` alias:**
§7 test 6 added: verify `SqliteGraphStoreOwned<'a>` is a drop-in for old
`SqliteGraphStore<'a>` in Shape B callsites.

**FINDING-8 (MINOR) — Asymmetric alias naming:**
Note added to §3 alias definitions acknowledging the `Owned`/`Reader`
asymmetry and suggesting alternatives (`Mut`/`Ref` or `Writer`/`Reader`).

**FINDING-9 (MINOR) — Effort estimate doesn't account for `with_graph_read`:**
§10 updated with conditional estimate: +1–2 hours if Option B/C chosen
for §4.1 `with_graph_read` migration.

**Overall assessment:** The core refactor (Borrow/BorrowMut generic split)
remains sound and is the right approach. However, the design needs potato's
input on §6 Q5 (relationship with `with_graph_read`) before implementation
can proceed. The design's status has been changed from `BLOCKED` to
`REVISED` to reflect that the document has been substantially corrected
but still requires sign-off.
