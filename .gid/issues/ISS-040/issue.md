---
id: "ISS-040"
title: "Memory::graph(&self) blocked: SqliteGraphStore couples read & write capabilities to one &mut Connection — split into reader/writer types"
status: open
priority: P2
created: 2026-04-26
updated: 2026-04-29
component: crates/engramai/src/graph/store.rs
related: [v03-graph-layer]
---

# ISS-040: Split `SqliteGraphStore` into `SqliteGraphReader` (`&Connection`) + `SqliteGraphStore` (`&mut Connection`)

**Status:** 🔴 Open
**Decision date:** 2026-04-29 (potato + RustClaw, root-cause review)

## TL;DR

`SqliteGraphStore<'a>` currently holds `&'a mut Connection` and implements **both** `GraphRead` and `GraphWrite`. This forces every read path to take an exclusive borrow of `Memory`, even though rusqlite 0.32's read APIs (`prepare`, `prepare_cached`) are `&self` and would be perfectly happy with `&Connection`. The `&mut` is over-constraint inherited from the writer side; it leaks all the way out to `Memory::graph(&mut self)` and prevents the design's `Memory::graph(&self)` shared-borrow shape.

**Root fix (not a patch):** capabilities split. Introduce `SqliteGraphReader<'a>` over `&'a Connection` carrying the read methods; keep `SqliteGraphStore<'a>` over `&'a mut Connection` for writes (and reuse). Move read SQL into a `mod queries` of free functions (`fn get_entity(conn: &Connection, namespace: &str, ...)`); both structs delegate to it. Zero generics in public signatures, no `Borrow<C>` tag depth, no runtime branching.

## Background

`v03-graph-layer` design §5 asks for:

```rust
impl Memory {
    pub fn graph(&self) -> &dyn GraphRead;
    pub fn graph_mut(&mut self) -> &mut dyn GraphWrite;
}
```

Intent: shared-borrow read paths exploit SQLite WAL read concurrency; only writes serialize.

Current shape (`crates/engramai/src/graph/store.rs:620`):

```rust
pub struct SqliteGraphStore<'a> {
    pub(crate) conn: &'a mut rusqlite::Connection,
    namespace: String,
    sink: Box<dyn TelemetrySink>,
    watermark: WatermarkTracker,
    embedding_dim: usize,
}
impl<'a> GraphRead  for SqliteGraphStore<'a> { /* 30 methods */ }
impl<'a> GraphWrite for SqliteGraphStore<'a> { /* mutating methods */ }
```

`Memory::graph_mut` exists (`memory.rs:657`). `Memory::graph(&self)` does not, because constructing `SqliteGraphStore<'_>` requires `&mut Connection`, which `&self` can't yield.

## Root-cause analysis

Three rusqlite/Rust facts establish the design space:

1. **rusqlite 0.32:** `Connection::prepare` and `Connection::prepare_cached` take `&self`. Read paths do **not** need `&mut Connection`. (Verified in `src/graph/store.rs:1235` — `self.conn.prepare_cached(...)` compiles inside `&self` methods today only because `&'a mut Connection` reborrows to `&Connection` automatically; the `&mut` is dead weight on the read side.)
2. **rusqlite 0.32:** `Connection::transaction(&mut self)` and `savepoint(&mut self)` need exclusive access. Write paths **do** need `&mut Connection`. Used in 10 places in `store.rs`.
3. **Rust:** A field of type `&'a mut Connection` cannot be created from `&self` of the owning struct. Period.

Conclusion: the **type system needs to encode the read/write capability split**, not paper over it. Today `SqliteGraphStore` conflates the two — the writer requirement (`&mut`) propagates to readers via the shared field. Fix is to separate concerns at the type level so each capability gets the borrow it actually needs.

## Why not the obvious "make it generic" fix

The earlier proposal (preserved below in *Rejected alternatives*) was:

```rust
pub struct SqliteGraphStoreT<C: Borrow<Connection>> { conn: C, ... }
type SqliteGraphReader<'a> = SqliteGraphStoreT<&'a Connection>;
type SqliteGraphStore<'a>  = SqliteGraphStoreT<&'a mut Connection>;
```

This works mechanically but pays an architectural cost we don't want:

1. **Tag depth.** `Borrow<Connection>` infects every helper, every wrapper, every future trait bound that touches the store. Anything generic over "a graph store" has to repeat `<C: Borrow<Connection>>`. Type inference suffers at call sites.
2. **Trait-object hostile.** `Box<dyn GraphRead>` becomes awkward — you'd be boxing a generic `SqliteGraphStoreT<&Connection>`, and any future async/dispatch boundary has to materialize the `C` parameter.
3. **Public-signature noise.** `Memory::graph(&self) -> SqliteGraphReader<'_>` is fine as an alias, but doc, error messages, IDE hover all show `SqliteGraphStoreT<&rusqlite::Connection>` underneath. Confusing to readers.
4. **Mixed read/write semantics in one type.** A `SqliteGraphStoreT<&Connection>` accidentally exposing a write-shaped method (because the impl bound was forgotten) is a silent footgun. Splitting types makes "can this write?" a compile-time fact at type-name level.

Concretely: the generic version is a *patch on the symptom* (one type that pretends to be two via a trait bound). The split is a *root fix* (two types because there are two capabilities).

## Recommended design (root fix)

### Step 1 — Extract read SQL into free functions

```rust
// crates/engramai/src/graph/queries.rs (new module)
use rusqlite::Connection;
use uuid::Uuid;
use crate::graph::{Entity, Edge, GraphError, ...};

pub(crate) fn get_entity(
    conn: &Connection,
    namespace: &str,
    embedding_dim: usize,
    id: Uuid,
) -> Result<Option<Entity>, GraphError> {
    let mut stmt = conn.prepare_cached(
        "SELECT ... FROM graph_entities WHERE id = ?1 AND namespace = ?2"
    )?;
    /* unchanged decode logic */
}

pub(crate) fn find_entity(conn: &Connection, namespace: &str, ...) -> Result<...> { ... }
pub(crate) fn neighbors(conn: &Connection, namespace: &str, ...) -> Result<...> { ... }
// ... all 30 GraphRead methods
```

This is the **real root fix**: SQL was bound to a struct that didn't need to own it. The 53 `self.conn.X` sites become a single import surface.

### Step 2 — Reader type over `&Connection`

```rust
pub struct SqliteGraphReader<'a> {
    conn: &'a rusqlite::Connection,
    namespace: String,
    embedding_dim: usize,
    // Note: no `sink`, no `watermark` — those are write-side telemetry.
    //       If reads need observability we add a `&'a dyn ReadSink` later.
}

impl<'a> SqliteGraphReader<'a> {
    pub fn new(conn: &'a Connection) -> Self { ... }
    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self { ... }
    pub fn with_embedding_dim(mut self, d: usize) -> Self { ... }
}

impl<'a> GraphRead for SqliteGraphReader<'a> {
    fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError> {
        queries::get_entity(self.conn, &self.namespace, self.embedding_dim, id)
    }
    // ... 30 thin delegators
}
```

### Step 3 — Writer reuses queries, retains `&mut Connection`

```rust
pub struct SqliteGraphStore<'a> {
    conn: &'a mut rusqlite::Connection,
    namespace: String,
    sink: Box<dyn TelemetrySink>,
    watermark: WatermarkTracker,
    embedding_dim: usize,
}

impl<'a> SqliteGraphStore<'a> {
    /// Cheap downgrade view for read paths inside write code.
    pub fn as_reader(&self) -> SqliteGraphReader<'_> {
        SqliteGraphReader {
            conn: self.conn,            // &'a mut Connection auto-reborrows to &Connection
            namespace: self.namespace.clone(),
            embedding_dim: self.embedding_dim,
        }
    }
}

// Reads: delegate to the same free functions. No code duplication.
impl<'a> GraphRead for SqliteGraphStore<'a> {
    fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError> {
        queries::get_entity(self.conn, &self.namespace, self.embedding_dim, id)
    }
    // ... 30 thin delegators (identical to reader's, because they call the same fns)
}

impl<'a> GraphWrite for SqliteGraphStore<'a> {
    /* unchanged — all `self.conn.transaction()` / inserts / updates */
}
```

Optional further DRY: make `SqliteGraphStore` `Deref<Target = SqliteGraphReader<'_>>`. Decided **against** at design time: `Deref` between unrelated capability types is a known footgun; explicit `as_reader()` keeps the boundary visible. (Open to reversing this if churn at call sites turns out painful — see Open Questions Q3.)

### Step 4 — Memory API

```rust
impl Memory {
    pub fn graph(&self) -> SqliteGraphReader<'_> {
        SqliteGraphReader::new(self.storage.connection())
            .with_namespace(self.namespace.clone())
            .with_embedding_dim(self.embedding_dim)
    }

    pub fn graph_mut(&mut self) -> SqliteGraphStore<'_> {
        // unchanged from today
    }
}
```

Note: returns concrete `SqliteGraphReader<'_>`, **not** `&dyn GraphRead`. The design's `&dyn GraphRead` shape is unimplementable without making `Memory` self-referential (the reader can't be stored *in* `Memory`). Concrete return is strictly better — devirtualized, no allocation, still satisfies the design's intent of "shared-borrow read access via `&self`."

`Storage::connection(&self) -> &Connection` already exists (`storage.rs:300`). The earlier issue text claiming this needs to be added was wrong.

### Step 5 — Convenience methods

The five methods currently on `Memory` (`get_entity`, `find_entity`, `neighbors`, `edges_as_of`, `list_failed_episodes`) — 13 external call sites — move from `&mut self` to `&self`:

```rust
impl Memory {
    pub fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError> {
        self.graph().get_entity(id)
    }
    // ... etc.
}
```

Call-site change: `memory.get_entity(id)?` works the same; the difference is `Memory` no longer takes exclusive borrow during the call. Concurrent reads now compose.

## Why this is root-cause, not a patch

- **Capabilities are types**, not trait bounds bolted on a single type. `SqliteGraphReader` exists ↔ "you can read the graph." `SqliteGraphStore` exists ↔ "you can read **and** write." Compile-time, no runtime branching.
- **No generic noise.** Public signatures stay concrete. No `Borrow<Connection>` tag in any helper, trait, or wrapper.
- **SQL is shared, not duplicated.** `mod queries` is the single source of truth for read SQL. Both structs delegate. (Today the read impl block is one struct's responsibility; tomorrow it's a free-function module — strictly more reusable.)
- **The `&` vs `&mut` distinction in Rust already encodes read-vs-write.** This refactor aligns the storage-layer types with that distinction instead of fighting it.
- **Trait-object friendly.** `Box<dyn GraphRead>` works trivially with both concrete types.
- **Future-proof.** When read-only telemetry (`ReadSink`) becomes a thing, it lands cleanly on `SqliteGraphReader` only; writers keep their existing `TelemetrySink`. No bound proliferation.

## Implementation plan

**Phase 1 — Extract & introduce reader (no external API change).**

1. Create `crates/engramai/src/graph/queries.rs`. Move all 30 read SQL bodies into free functions taking `(conn: &Connection, namespace: &str, embedding_dim: usize, ...)`. Existing `impl GraphRead for SqliteGraphStore` becomes 30 thin delegators. Run full test suite — must be green before next step.
2. Add `pub struct SqliteGraphReader<'a>` + `impl GraphRead`. Does not yet appear in any public API.
3. Add `Memory::graph(&self) -> SqliteGraphReader<'_>`.
4. Add a smoke test: two concurrent `memory.graph()` borrows compile and run side by side; one `memory.graph_mut()` excludes them.

**Phase 2 — Migrate convenience methods.**

5. Downgrade the 5 `Memory` convenience methods from `&mut self` → `&self`. Update 13 external call sites if they relied on the exclusive borrow side-effect (they shouldn't — these are pure reads).
6. Rerun tests.

**Phase 3 — Optional cleanup.**

7. Audit for code that does `memory.graph_mut().some_read_op()` and migrate to `memory.graph().some_read_op()` where the `&mut` was incidental, not required.
8. Decide on `as_reader()` ergonomics in write paths — if call sites are noisy, revisit `Deref` decision.

**Risk:** low. Phase 1 is mechanical and behavior-preserving (all read SQL was already `&self`-compatible). Phase 2 is signature relaxation with no logic change. Phase 3 is opt-in cleanup.

## Acceptance criteria

- [ ] `crates/engramai/src/graph/queries.rs` exists with 30 read free functions; `impl GraphRead for SqliteGraphStore` delegates to it
- [ ] `pub struct SqliteGraphReader<'a>` exists, implements `GraphRead`, holds `&'a Connection` (no `&mut`)
- [ ] `Memory::graph(&self) -> SqliteGraphReader<'_>` exists and is wired
- [ ] `SqliteGraphStore::as_reader(&self) -> SqliteGraphReader<'_>` exists for write-path read needs
- [ ] All 5 `Memory` convenience methods downgrade to `&self`
- [ ] Test: two concurrent `memory.graph()` borrows compile and read independently
- [ ] Test: `memory.graph_mut()` borrow excludes any concurrent `memory.graph()`
- [ ] Full graph + memory test suite green; no behavior change in writes
- [ ] No `Borrow<C>` / `BorrowMut<C>` bound appears anywhere in `engramai`

## Rejected alternatives

### A. `SqliteGraphStoreT<C: Borrow<Connection>>` generic over the connection borrow
(See "Why not the obvious 'make it generic' fix" above.) Patches the symptom by hiding two capabilities under one type with a trait bound. Tag depth pollutes every downstream helper. Public signatures show generic noise. Trait-object hostile. **Rejected as patch, not root fix.**

### B. `Cow<'a, Connection>` / `enum { Shared(&Connection), Exclusive(&mut Connection) }`
Each read method matches the enum at runtime; writes panic on `Shared` arm. Converts a compile-time capability check into a runtime check. **Rejected as architecturally wrong** — the type system already gives us this distinction for free.

### C. Wrap `Connection` in `Arc<Mutex<...>>` inside `Storage`
Solves the borrow problem by globally serializing access. Defeats SQLite WAL read concurrency entirely. **Rejected on perf and on principle** — adding a lock to dodge a borrow-check issue is the textbook anti-pattern.

## Open questions (deferred to implementation, not blockers)

1. **`graph_mut()` public API surface.** Today it returns concrete `SqliteGraphStore<'_>`. Should it stay concrete or shift to `&mut dyn GraphWrite`? Recommendation: **stay concrete** (consistency with `graph()`, devirtualized). Revisit only if a real polymorphism need appears.
2. **Read-side telemetry.** `SqliteGraphReader` deliberately omits `TelemetrySink`. If/when reads need telemetry, add a `ReadSink` to the reader struct only — don't merge sinks. Tracked separately.
3. **`Deref` from `SqliteGraphStore` to `SqliteGraphReader`.** Initially **off** (explicit `as_reader()`). Re-evaluate if write-path code is noticeably more verbose post-Phase-2.

These do not block Phase 1 — they're refinement decisions for Phase 3 and beyond.

## Why deferred from `graph-impl-memory-api`

That task was scoped as additive Memory API surface. This refactor restructures the storage core (~2000-line impl block, 30 method extractions, new module). Mixing breaks atomicity and review. Splitting:

- `graph-impl-memory-api` (current): adds `graph_mut`, 5 convenience methods (all `&mut self` for now), no storage-layer churn.
- `ISS-040` (this issue): does the capability split per Phase 1+2 above; convenience methods upgrade to `&self`.

## References

- `crates/engramai/src/graph/store.rs:620` — current struct definition
- `crates/engramai/src/graph/store.rs:1233` — `impl GraphRead`
- `crates/engramai/src/graph/store.rs:3171` — `impl GraphWrite`
- `crates/engramai/src/memory.rs:657` — current `graph_mut`
- `crates/engramai/src/memory.rs:592` — `extraction_status`, the existing precedent for "morally read but takes `&mut`"
- `crates/engramai/src/storage.rs:300` — `Storage::connection(&self) -> &Connection` (already exists)
- `v03-graph-layer` design §5 — `Memory::graph` / `graph_mut` signature spec
- Design discussion: 2026-04-29 RustClaw root-cause review with potato — "no patch, no tag depth, root facts"
