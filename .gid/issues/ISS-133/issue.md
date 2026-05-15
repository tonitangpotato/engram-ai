---
id: ISS-133
title: Wire Memory::add_episode and Memory::reextract_episodes to resolution worker
status: open
priority: P2
created: 2026-05-15
relates_to: [ISS-041, ISS-042]
labels: [memory-api, resolution, v0.3]
---

# Problem

The public input/output types `Episode` (ISS-041) and `ReextractReport`
(ISS-042) shipped in commit `aa51bbd` and are fully tested. The
**Memory-level API surface** that consumes them has not.

`crates/engramai/src/memory.rs:633` still carries the deferred-API
note:

```text
// * `Memory::add_episode(ep: Episode)` — blocked on ISS-041 (the
//   `Episode` value type)
// * `Memory::reextract_episodes(eps) -> ReextractReport` — blocked
//   on ISS-042
```

Those blocks are now lifted (the types exist and are tested). What's
missing is the wire-up to the resolution worker.

# What's required

1. **`Memory::add_episode(ep: Episode) -> Result<Uuid, EngramError>`**
   - Dispatch to `crate::resolution::pipeline` for ingestion
   - Honor `ep.id` as idempotency key (re-call returns same Uuid,
     does not double-write)
   - Honor `ep.when` (None → `Utc::now()`)
   - Honor `ep.session_id` for session-affinity routing per
     `v03-resolution/design.md` §5.1

2. **`Memory::reextract_episodes(ids: Vec<Uuid>) -> ReextractReport`**
   - Look up each id, route to resolution worker for retry
   - Populate `report.succeeded` / `still_failed` / `skipped_idempotent`
     correctly per GOAL-2.1 (idempotence) and GOAL-2.2/2.3 (failure
     surfacing) from v03-resolution requirements
   - `skipped_idempotent` is the critical correctness gate — if the
     episode already resolved cleanly, it must show up there, NOT in
     `succeeded`

# Why deferred from ISS-041 / ISS-042

Both original issues had a Memory-API AC item (#3 in each). Closing
both issues required two pieces of work:

- (a) Define the public types — done in `aa51bbd`, struct work was
  self-contained and could ship without touching Memory.
- (b) Wire the types to Memory — needs design clarity on async
  semantics, error mapping, and how the resolution worker is
  reachable from `Memory` (it's `Arc<RwLock<Memory>>` in most
  callers, the worker is a separate task).

(a) is done; ISS-041 and ISS-042 close on the struct deliverable.
This issue (ISS-133) tracks (b) as a separate, focused unit of
work with its own design needs.

# Acceptance criteria

- [ ] `Memory::add_episode(ep: Episode) -> Result<Uuid, EngramError>`
      implemented and tested (idempotency-key round-trip, default
      `when` fills in, session_id propagates)
- [ ] `Memory::reextract_episodes(ids: Vec<Uuid>) -> ReextractReport`
      implemented and tested:
  - [ ] All-succeed path populates `succeeded` only
  - [ ] Some-still-fail path populates `still_failed` with reasons
  - [ ] **Idempotence (GOAL-2.1)**: re-calling on already-resolved
        episode populates `skipped_idempotent`, **not** `succeeded`
- [ ] Remove the "blocked on ISS-041 / ISS-042" comment block at
      `memory.rs:633`
- [ ] Module docs in `episode.rs` and `reextract.rs` update from
      "currently deferred" → reference the Memory method

# Out of scope

- Changes to the `Episode` or `ReextractReport` shape — they're
  frozen by ISS-041 / ISS-042 closing
- The resolution worker internals (assumed working, this is just
  the Memory-side API surface)

# References

- ISS-041 — Episode struct (closed: struct shipped, Memory wire
  delegated here)
- ISS-042 — ReextractReport struct (closed: struct shipped, Memory
  wire delegated here)
- `crates/engramai/src/memory.rs:633-637` — current deferred-API
  comment block
- v03-resolution/design.md — full ingestion + retry contract
- `crates/engramai/src/resolution/episode.rs:200-` and
  `crates/engramai/src/resolution/reextract.rs:120-` — the existing
  test suites that constrain the wire-up
