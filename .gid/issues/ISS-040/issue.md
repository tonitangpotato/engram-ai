---
id: "ISS-040"
title: "Memory::graph(&self) requires SqliteGraphStore<C: Borrow<Connection>> generic refactor"
status: open
priority: P2
created: 2026-04-26
component: crates/engramai/src/graph/store.rs
related: [v03-graph-layer]
---

# ISS-040: `Memory::graph(&self) -> &dyn GraphRead` blocked on storage-layer refactor

**Status:** 🔴 Open

## Background

`v03-graph-layer` design §5 specifies:

```rust
impl Memory {
    pub fn graph(&self) -> &dyn GraphRead;            // shared borrow, multiple readers
    pub fn graph_mut(&mut self) -> &mut dyn GraphWrite; // exclusive borrow
}
```

The intent: hand out a read-only graph view through `&self`, leveraging SQLite WAL for read concurrency.

## Why this can't ship in `graph-impl-memory-api`

`SqliteGraphStore<'a>` is defined as:

```rust
pub struct SqliteGraphStore<'a> {
    pub(crate) conn: &'a mut rusqlite::Connection,
    ...
}
```

The `&'a mut Connection` field cannot be produced from `&self` on `Memory`, even though every method in `impl GraphRead for SqliteGraphStore` uses only `self.conn.prepare_cached(...)` / `self.conn.prepare(...)` — both of which are `&Connection` methods. The `&mut` is over-constraint inherited from the writer side.

Existing precedent in `memory.rs` (`Memory::extraction_status`) already takes `&mut self` for what is morally a read, with an apologetic doc comment, for the same reason.

## Root fix (this issue)

Genericize `SqliteGraphStore` over its connection borrow:

```rust
use std::borrow::Borrow;

pub struct SqliteGraphStoreT<C: Borrow<rusqlite::Connection>> {
    conn: C,
    namespace: String,
    sink: Box<dyn TelemetrySink>,
    watermark: WatermarkTracker,
    embedding_dim: usize,
}

pub type SqliteGraphStore<'a>  = SqliteGraphStoreT<&'a mut rusqlite::Connection>;
pub type SqliteGraphReader<'a> = SqliteGraphStoreT<&'a rusqlite::Connection>;

// Read methods: any C: Borrow<Connection>
impl<C: Borrow<rusqlite::Connection>> GraphRead for SqliteGraphStoreT<C> {
    fn get_entity(&self, id: Uuid) -> ... {
        let mut stmt = self.conn.borrow().prepare_cached(...)?;
        ...
    }
    // ... all 30 methods identical except `self.conn.X` → `self.conn.borrow().X`
}

// Write methods: only when C = &mut Connection
impl<'a> GraphWrite for SqliteGraphStoreT<&'a mut rusqlite::Connection> {
    ...
}
```

29 call sites of `self.conn.prepare*` in the read impl block (lines 1173–3097 of `graph/store.rs`). Mechanical rewrite.

Then in `Memory`:

```rust
pub fn graph(&self) -> SqliteGraphReader<'_> {
    SqliteGraphReader::new(self.storage.connection())
        .with_namespace(...)
        .with_embedding_dim(...)
}
```

Note the return type is `impl GraphRead + '_` shape (concrete `SqliteGraphReader<'_>`), not `&dyn GraphRead` — the design's `&dyn GraphRead` signature can't be honored without an even larger refactor (the reader can't be stored *inside* `Memory` since it would self-borrow). A concrete-type return is strictly more useful (devirtualized) and still satisfies the design's spirit.

Also need: `Storage::connection(&self) -> &Connection` accessor (currently only `connection_mut` exists).

## Scope estimate

- 1× generic param + 2 type aliases on `SqliteGraphStore`
- 29× `self.conn.X` → `self.conn.borrow().X` in `impl GraphRead` block
- 1× `impl GraphRead` signature change (`impl<'a>` → `impl<C: Borrow<...>>`)
- `impl GraphWrite` signature pinned to `&mut Connection` variant — no body changes
- `Storage::connection(&self) -> &Connection` accessor on the storage facade
- `Memory::graph(&self) -> SqliteGraphReader<'_>` accessor

Risk: low. All read methods already only use `&Connection` semantics; this surfaces that statically.

## Why deferred from `graph-impl-memory-api`

That task is scoped as "additive Memory API." This refactor touches the storage core (~2000-line impl block). Mixing them violates atomicity / makes review harder. Splitting:

- `graph-impl-memory-api` (current): adds `graph_mut`, 5 convenience methods (all `&mut self` for now), no storage-layer churn.
- `ISS-040` (this issue): does the storage-layer split, then upgrades convenience methods + `graph()` to `&self`.

## Acceptance criteria

- [ ] `SqliteGraphStore` is generic over `Borrow<Connection>`
- [ ] `SqliteGraphReader<'a>` type alias exists, satisfies `GraphRead`
- [ ] `Storage::connection(&self) -> &Connection` exists
- [ ] `Memory::graph(&self) -> SqliteGraphReader<'_>` exists and is wired
- [ ] All 5 convenience methods (`get_entity`, `find_entity`, `neighbors`, `edges_as_of`, `list_failed_episodes`) downgrade from `&mut self` to `&self`
- [ ] No behavior change in writes; full graph test suite green
- [ ] At least one test: two concurrent `graph()` borrows compile and read independently
