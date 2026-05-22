---
id: ISS-134
title: Single source of truth for embedding dimensions (consolidate 3 constants)
status: todo
priority: P2
kind: design-debt
labels: [substrate, embeddings, tech-debt]
relates_to: [ISS-132]
---

# ISS-134 — Single source of truth for embedding dimensions

## Problem

Three independent constants currently encode "the embedding dim":

| Site | Constant | Value | Authority |
|---|---|---|---|
| `crates/engramai/src/embeddings.rs:42` | `EmbeddingConfig::default().dimensions` | `768` | **Ground truth** — matches `nomic-embed-text` model |
| `crates/engramai/src/graph/store.rs:668` | `DEFAULT_ENTITY_EMBEDDING_DIM` | `768` | Persisted-row invariant (graph_entities.embedding BLOB length) |
| `crates/engramai/src/resolution/mod.rs:145` | `DEFAULT_EMBEDDING_DIM` | `384` (post-ISS-132: `768`) | Resolution-pipeline placeholder dim |

They are all `pub const` with no compile-time relationship. The comment on
`resolution::DEFAULT_EMBEDDING_DIM` originally claimed "matches nomic-embed-text"
which was false (nomic is 768, the constant was 384) — this is the regression
ISS-132 patches.

ISS-132 fixes the immediate breakage by flipping resolution's constant to 768,
but does **not** establish a single source of truth. If someone later swaps the
embedder to a 1024-dim model (e.g. `mxbai-embed-large`), they have to remember
to update three files in lockstep. The next mismatch is just a matter of time.

## Root cause

No canonical constant. Each subsystem (embedder config, graph storage,
resolution pipeline) picked its own "default" without a shared anchor.

## Proposed fix

Establish `crate::embeddings::EmbeddingConfig::default().dimensions` as the
sole authority and have all other sites reference it (transitively or via a
single re-export).

Options:

- **(A) `pub const DEFAULT_EMBEDDING_DIM` at crate root** that other modules
  import — keeps `const`-ness but requires manual sync with `EmbeddingConfig::default()`.
- **(B) Make `graph` + `resolution` constants `fn`s** that return
  `EmbeddingConfig::default().dimensions` — runtime cost is one Default
  construction, but eliminates the duplication entirely. Most callers already
  do this lookup at startup, not in hot loops.
- **(C) Make embedding dim a workspace-level config** read from `engram.toml`
  or env at startup, with the embedder being the source of truth at runtime
  (no `pub const` anywhere). Cleanest, but bigger refactor.

Lean: **Option B** for now (lowest risk, no API churn, works with feature
flags). C is the "right" answer long-term but should be folded into v0.5
config consolidation work, not bolted on now.

## Acceptance criteria

- [ ] Single canonical place for "default embedding dim" — others reference it.
- [ ] Adding a new model (e.g. 1024-dim) requires editing exactly **one** file.
- [ ] No `pub const` or magic number 768 (or 384) appears in resolution or
  graph layers — only in the canonical site.
- [ ] Test added: assert that all three legacy constant sites (if kept for
  back-compat) agree on dim at compile time or test time.
- [ ] `apply_graph_delta` invariant violations cannot be triggered by mismatched
  defaults again (regression test from ISS-132 still passes after refactor).

## Non-goals

- Making dim configurable per-call (callers can already do this via
  `SqliteGraphStore::with_embedding_dim` + `EmbeddingConfig { dimensions, .. }`).
- Removing `with_embedding_dim` / explicit-dim APIs — they remain for tests
  and migration scenarios.
- Changing the actual value 768 — that's nomic's dimension, ground truth.

## References

- ISS-132 — immediate fix (resolution dim 384 → 768)
- ISS-044 — backfill regression that surfaced this
- `crates/engramai/src/embeddings.rs:42` — current ground truth
- `crates/engramai/src/graph/store.rs:668` — graph store constant
- `crates/engramai/src/resolution/mod.rs:145` — resolution constant
