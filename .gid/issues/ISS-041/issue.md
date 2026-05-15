---
id: ISS-041
title: Episode struct definition (v0.3 ingestion contract)
status: done
priority: P1
created: 2026-04-26
component: crates/engramai/src/resolution/
related:
- v03-resolution
- v03-graph-layer
fixed_by: aa51bbd
---

# ISS-041: `Episode` struct — v0.3 ingestion input contract

**Status:** 🔴 Open

## Background

`v03-graph-layer` design §5 specifies:

```rust
impl Memory {
    pub fn add_episode(&mut self, ep: Episode) -> Result<Uuid, EngramError>;
}
```

But no `Episode` struct exists in the codebase. Episodes currently exist only as `Uuid` identifiers in `graph_episodes` table; their *content* (text, metadata) is implicit/external.

## What's missing

A concrete `Episode` value type — what the user hands to `add_episode`. Likely shape (to be confirmed in v03-resolution design):

```rust
pub struct Episode {
    pub id: Option<Uuid>,           // None → generated; Some → idempotency key
    pub text: String,                // raw episodic content
    pub session_id: Option<Uuid>,    // for session-affinity routing (§5.1)
    pub when: Option<DateTime<Utc>>, // None → now()
    pub metadata: serde_json::Value, // arbitrary user metadata
}
```

This belongs in the **resolution** layer (`crates/engramai/src/resolution/`), not the graph layer — episodes are an ingestion concern. The graph layer only sees `Uuid` references.

## Why deferred

The design for `v03-resolution` hasn't been written yet (per `gid_tasks` on the engram graph). Defining `Episode` in `graph-impl-memory-api` would:

1. Pre-commit to a shape before the resolution design exists
2. Force `Memory::add_episode` to live in graph layer when it's really an ingestion entry point
3. Block on `ResolutionStats`, `compile_knowledge`, etc. that share the same design context

## Resolution path

1. v03-resolution design phase defines `Episode` shape (likely §3 of that doc)
2. Implementation lands in `crates/engramai/src/resolution/types.rs` or similar
3. `Memory::add_episode` is added in `task:res-impl-memory-api`, not here

## Acceptance criteria

- [ ] `Episode` struct defined with all required fields for ingestion
- [ ] Serde round-trip tested
- [ ] `Memory::add_episode(ep: Episode) -> Result<Uuid, EngramError>` lands in `task:res-impl-memory-api`

## Closure (2026-05-15)

**Status: done.** Struct shipped commit `aa51bbd` (feat(resolution):
Episode + ReextractReport public types). AC #1 (struct defined with
required fields) and AC #2 (serde round-trip tested) satisfied:

- `crates/engramai/src/resolution/episode.rs` — `Episode`
- Test suite: resolution::episode::tests (6 tests) — all pass on trunk 2026-05-15

**AC #3 (Memory::* method lands) split out as ISS-133** — wiring
the struct to a Memory-level API needs separate design work
(async semantics, worker reachability, idempotence routing).
Bundling it here was overreach; the struct deliverable stands on
its own and is properly done.

`fixed_by: aa51bbd`. Status: in_review → done. Memory API
follow-up: ISS-133.
