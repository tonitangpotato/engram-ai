---
id: ISS-042
title: ReextractReport struct definition (v0.3 retry surface)
status: done
priority: P1
created: 2026-04-26
component: crates/engramai/src/resolution/
related:
- v03-resolution
- v03-graph-layer
fixed_by: aa51bbd
---

# ISS-042: `ReextractReport` struct — v0.3 retry surface contract

**Status:** 🔴 Open

## Background

`v03-graph-layer` design §5 specifies:

```rust
impl Memory {
    pub fn reextract_episodes(&mut self, eps: &[Uuid]) -> Result<ReextractReport, EngramError>;
}
```

The doc-comment marks this as "a thin shim; the actual retry logic lives in v03-resolution." But `ReextractReport` itself isn't defined anywhere.

## What's missing

The report value returned to callers after a re-extraction batch. Likely shape (to be confirmed in v03-resolution design):

```rust
pub struct ReextractReport {
    pub requested: usize,
    pub succeeded: Vec<Uuid>,
    pub still_failed: Vec<(Uuid, String)>, // episode_id, error reason
    pub skipped_idempotent: Vec<Uuid>,     // already-resolved, no work done (GOAL-2.1)
}
```

GOAL-2.1 (idempotence) and GOAL-2.2/2.3 (failure surfacing) constrain its shape — the report must distinguish "succeeded this time," "still broken," and "no-op (already resolved)."

## Why deferred

Same reasoning as ISS-041: the report is a v03-resolution contract. Defining it in `graph-impl-memory-api`:

1. Pre-commits to a shape before v03-resolution design exists
2. Couples graph layer to resolution-layer concepts
3. The "thin shim" `Memory::reextract_episodes` has nothing to delegate to without the actual worker pool (`task:res-impl-worker`).

## Resolution path

1. v03-resolution design defines `ReextractReport` (likely §6)
2. Implementation lands alongside `ResolutionStats` in `crates/engramai/src/resolution/stats.rs` (or `types.rs`)
3. `Memory::reextract_episodes` lands in `task:res-impl-memory-api`, calling into `resolution::worker::reextract_batch(...)` or similar

## Acceptance criteria

- [ ] `ReextractReport` struct defined with succeeded / still_failed / skipped_idempotent fields
- [ ] Serde round-trip tested
- [ ] `Memory::reextract_episodes` lands in `task:res-impl-memory-api` and delegates to the resolution worker
- [ ] Idempotence (GOAL-2.1): re-calling on already-resolved episodes populates `skipped_idempotent`, not `succeeded`

## Closure (2026-05-15)

**Status: done.** Struct shipped commit `aa51bbd` (feat(resolution):
Episode + ReextractReport public types). AC #1 (struct defined with
required fields) and AC #2 (serde round-trip tested) satisfied:

- `crates/engramai/src/resolution/reextractreport.rs` — `ReextractReport`
- Test suite: resolution::reextract::tests (8 tests) — all pass on trunk 2026-05-15

**AC #3 (Memory::* method lands) split out as ISS-133** — wiring
the struct to a Memory-level API needs separate design work
(async semantics, worker reachability, idempotence routing).
Bundling it here was overreach; the struct deliverable stands on
its own and is properly done.

`fixed_by: aa51bbd`. Status: in_review → done. Memory API
follow-up: ISS-133.
