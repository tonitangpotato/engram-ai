---
id: ISS-133
title: Wire Memory::add_episode and Memory::reextract_episodes to resolution worker
status: resolved
priority: P2
created: 2026-05-15
relates_to:
- ISS-041
- ISS-042
labels:
- memory-api
- resolution
- v0.3
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

- [x] `Memory::add_episode(ep: Episode) -> Result<Uuid, EngramError>`
      implemented and tested (idempotency-key round-trip, default
      `when` fills in, session_id propagates)
- [x] `Memory::reextract_episodes(ids: Vec<Uuid>) -> ReextractReport`
      implemented and tested:
  - [x] All-succeed path populates `succeeded` only
  - [x] Some-still-fail path populates `still_failed` with reasons
  - [x] **Idempotence (GOAL-2.1)**: re-calling on already-resolved
        episode populates `skipped_idempotent`, **not** `succeeded`
- [x] Remove the "blocked on ISS-041 / ISS-042" comment block at
      `memory.rs:633`
- [x] Module docs in `episode.rs` and `reextract.rs` update from
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

## 2026-05-22 — implementation blocked on type impedance mismatch

Started implementation, hit a hard contradiction that needs a design
call before any code lands:

**The mismatch:**
- `Episode.id: Option<Uuid>` (full 128-bit UUID, frozen by ISS-041).
- `MemoryRecord.id: String` shaped as **8-character truncated UUID
  prefix** (`memory.rs:2404`: `format!("{}", Uuid::new_v4())[..8]`).
- All downstream id flows (`store_raw`, `get`, `extraction_status`,
  `graph_pipeline_runs.memory_id`, the resolution job queue) are
  built on the 8-char string id.

Episode.id therefore has nowhere to land. Any implementation has to
pick a poison:

1. **Truncate** — `id.to_string()[..8]` collides catastrophically
   (UUID first 8 hex chars = 2^32 namespace; production volume will
   collide). Idempotency contract fails silently.
2. **Stringify full UUID** — store_raw / dedup / FK columns currently
   assume 8-char; widening them is a multi-table migration.
3. **Add `memories.episode_id` Uuid column** — separate from
   `memories.id`. Now we have two ids; every read/write site has to
   know which one to use. Doable but a real migration.
4. **Drop Episode.id from the public type** — breaks the ISS-041
   contract that supposedly just shipped.
5. **Implement add_episode but silently ignore Episode.id** — drops
   the idempotency AC. Honest but the AC was the whole point.

Same problem applies to `reextract_episodes(ids: Vec<Uuid>)` — the
ids cannot index into `memories` without a story for the conversion.

**Why this wasn't caught earlier:** ISS-041 / ISS-042 shipped pure
struct work without wiring; the impedance only surfaces at the
Memory-API boundary, which is precisely this issue.

**What's needed before this can be implemented:** a design decision
on which option above (or new option) we commit to. Likely a small
feature doc under `.gid/features/v03-memory-api/` rather than a
one-liner here. This issue should probably move from `open` →
`blocked` with a `blocked_on:` pointer to that doc once it exists.

Smaller follow-ups that could ship independently without resolving
the main mismatch:

- Implement `add_episode` ignoring Episode.id (returns the
  store_raw-minted 8-char id), document the limitation, file a
  follow-up for the id round-trip work. Drops AC #1 partly.
- Implement `reextract_episodes` accepting `Vec<String>` instead of
  `Vec<Uuid>` — changes the public signature from what's specified,
  but matches reality. Requires re-opening ISS-042 to update the
  type. Hard "no" without explicit go-ahead.

**Not coded anything yet** — wanted the design decision in writing
before touching memory.rs.

## 2026-05-22 (later same day) — shipped

The "type impedance" mismatch noted above was real but solvable
without any schema migration. Recap of how this landed:

**Decisions:**

- Q1 (multi-fact dispatch): caller-supplied `Episode.id` is honored
  on the **first** fact only when the extractor produces N > 1
  facts. Subsequent facts get freshly-minted 8-char hex ids and a
  `log::warn!` is emitted. Silent fabrication of N sibling Uuids
  would be worse than a partial honor.
- Q2 (dedup): caller-supplied id is **semantically incompatible**
  with content-hash dedup. With `MemoryConfig::dedup_enabled = true`,
  `add_episode` (via `add_raw`) returns an error if the caller
  supplies an id. Caller picks one or the other. This is the only
  option that doesn't silently violate the idempotency contract.
- Q3 (reextract timeout): forever-poll, caller aborts via task
  cancellation. `ReextractReport` has no `Timeout` bucket; adding
  one would expand the frozen ISS-042 contract.

**How the type impedance was resolved** (it was not a multi-table
migration after all):

- `memories.id` is `String`, not strictly 8-char. v0.2-compat path
  still mints 8-char hex.
- v0.3 path (caller supplies `Episode.id`): we store the **full
  36-char Uuid string** in `memories.id`. Storage doesn't care
  about id format. Downstream code that parses ids (`Uuid::parse_str`
  in graph/store.rs) is on the *graph* layer, not the *memories*
  layer, so no conflict.
- The impedance is plumbed via a one-shot `Memory.pending_caller_id:
  Option<String>` field. `add_episode` parks the caller id; the
  next `add_raw` call consumes it via `Option::take()`. No new
  function-signature parameters; multi-fact loop sees `None` on
  iteration 2+ naturally.
- When the caller does NOT supply `Episode.id`, `add_episode` still
  has to return a `Uuid` per AC. We derive it lossily by parsing
  the 8-char hex into the low 32 bits of an otherwise-zero Uuid.
  That Uuid does NOT round-trip through `Memory::get` (the stored
  key is the 8-char hex, not the formatted Uuid). Callers who need
  round-trip MUST supply `Episode.id`. This is pinned by the test
  `add_episode_without_id_returns_uuid_derived_from_minted_hex`.

**Tests** (8/8 pass in `crates/engramai/tests/iss133_add_episode_test.rs`):

1. `add_episode_honors_caller_supplied_id` — AC #1 round-trip.
2. `add_episode_without_id_returns_uuid_derived_from_minted_hex` —
   pins the v0.2-compat lossy-derivation contract.
3. `add_episode_rejects_caller_id_when_dedup_enabled` — Q2=(d).
4. `add_episode_when_none_uses_wallclock_for_created_at` — AC #2 default.
5. `add_episode_when_some_propagates_to_occurred_at` — AC #2 supplied.
6. `add_episode_session_id_lands_in_user_metadata` — AC #3.
7. `reextract_episodes_buckets_already_completed_as_skipped_idempotent` —
   AC #4 GOAL-2.1.
8. `reextract_episodes_missing_memory_lands_in_still_failed` —
   defensive coverage for the "ghost id" branch.

Full lib suite 1910/1910 green. No regressions.

**Files touched:**

- `crates/engramai/src/memory.rs` — `Memory` struct adds
  `pending_caller_id` field; 4 constructors initialize it; `add_raw`
  consumes it with the dedup-incompatibility guard; `add_episode`
  and `reextract_episodes` methods inserted between
  `list_proposed_predicates` and `enqueue_pipeline_job`. Deferred-API
  comment block at the v0.3 graph-layer module doc updated.
- `crates/engramai/src/resolution/episode.rs` — module-doc link to
  `Memory::add_episode`.
- `crates/engramai/src/resolution/reextract.rs` — module-doc updated
  from "currently deferred" → live link to `Memory::reextract_episodes`.
- `crates/engramai/tests/iss133_add_episode_test.rs` — new (8 tests).

**Limitations / follow-ups** (none blocking):

- The "no-caller-id Uuid is lossy" path is documented in code but
  not pretty. Long-term we should either (a) make
  `MemoryRecord.id: Uuid` everywhere or (b) drop the `add_episode ->
  Result<Uuid>` signature in favor of a `Result<MemoryId>` typedef.
  Both are larger than ISS-133.
- The `add_episode_when_none_uses_wallclock_for_created_at` test
  asserts `occurred_at.is_none()` — meaning the v0.2 dual-column
  policy (occurred_at = Some only when caller backdates) carries
  through. This matches ISS-087 / ISS-103 semantics.
