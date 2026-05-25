---
title: Memory::with_graph_store ignores config.embedding.dimensions — graph store always uses default 768d
status: open
priority: P1
severity: bug
category: graph
created: 2026-05-25
relates:
- ISS-033
- ISS-157
depends_on: ''
---

## Summary

`Memory::with_graph_store(path)` constructs `SqliteGraphStore::new(conn)`
which seeds `embedding_dim` from `crate::embeddings::default_embedding_dim()`
(= 768, hardcoded via `EmbeddingConfig::default().dimensions`).

The user's actual embedder config — `self.config.embedding.dimensions` —
is **never threaded into the graph store**.

If a caller configures a non-768d embedder (e.g. `bge-large` 1024d,
`text-embedding-3-small` 1536d) via `Memory::with_embedding`, then calls
`with_graph_store`, every subsequent entity-embedding write/read returns
`GraphError::Invariant("entity embedding dim mismatch")` once a real
entity vector reaches the graph layer.

## Reproduction

```rust
let mut cfg = MemoryConfig::default();
cfg.embedding = EmbeddingConfig {
    provider: "ollama".into(),
    model: "bge-large".into(),
    host: "http://localhost:11434".into(),
    dimensions: 1024,
    timeout_secs: 30,
    api_key: None,
};
let mem = Memory::new(":memory:", Some(cfg))?
    .with_graph_store(graph_path)?;
// mem.graph_store.embedding_dim == 768, not 1024
// Any path that calls SqliteGraphStore::search_candidates with a 1024d
// query embedding now errors: "entity embedding dim mismatch".
```

## Why it didn't bite us yet

Every production / bench caller has used the default 768d `nomic-embed-text`,
so `default_embedding_dim()` happened to match `config.embedding.dimensions`.
ISS-033 added `with_embedding_dim()` as the documented override, but
`Memory::with_graph_store` doesn't call it.

ISS-157 (single-hop weapon B) is the first concrete need to swap embedders
to a 1024d model; this defect blocks that experiment.

## Fix

Thread `self.config.embedding.dimensions` into the graph-store builder:

```rust
// crates/engramai/src/memory.rs:531
let store = SqliteGraphStore::new(graph_conn)
    .with_embedding_dim(self.config.embedding.dimensions);
```

That's the one-line fix. The same threading should happen in any other
graph-store construction path (`with_pipeline_pool` etc. — audit).

## Acceptance criteria

- [ ] AC #1 — `Memory::with_graph_store` propagates
      `self.config.embedding.dimensions` to the constructed
      `SqliteGraphStore`. Same for any other Memory-construction path that
      builds a graph store.
- [ ] AC #2 — Regression test: construct Memory with a non-default
      `EmbeddingConfig.dimensions` (e.g. 1024), install graph store, write
      an entity with a 1024d embedding, confirm no `Invariant` error
      surfaces from `search_candidates` or `upsert_entity_embedding`.
- [ ] AC #3 — Audit all `SqliteGraphStore::new` call sites in
      `crates/engramai/src/` outside tests. Either thread the configured
      dim or document why the default is correct for that path.

## Notes

- Discovered while implementing ISS-157 weapon B (embedder swap experiment).
- The schema in `graph_entities` stores embeddings as opaque BLOB; the
  dim is enforced app-side in the codec. So no schema migration needed —
  just thread the config through.
- ISS-033 originally introduced `with_embedding_dim()` as a setter and
  documented it as the override path. The miss was not wiring
  `Memory::with_graph_store` to actually call it.

## Related

- ISS-033 — original embedding-dim plumbing
- ISS-157 — blocked by this for embedder swap experiment
