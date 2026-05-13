---
id: ISS-117
title: "T29.4 Hebbian read-switch blocked — legacy/unified parity not achievable without resolving weight & dup divergence"
status: open
priority: P1
severity: design
labels: [v04-unified-substrate, phase-d, read-switch, t29-blocker, divergence]
created: 2026-05-13
blocks: T29.4
---

# T29.4 Hebbian read-switch blocked

## Summary

Attempted T29.4 part-1 (switching `get_hebbian_neighbors` /
`get_hebbian_links_weighted` to read from `edges WHERE
edge_kind='associative'`) and discovered that the legacy and unified
substrates produce **non-equivalent reader output** in two
independent ways. Bug-for-bug equivalence — the implicit Phase D
contract from §5.4 — is not achievable with the current
T14/ISS-116 writer behavior.

WIP stashed (`git stash list` → "T29.4 part1 WIP — discovered
weight/dup divergence …"); main is back to clean.

## Divergences observed

### D1. Row-shape divergence: legacy double-row vs unified canonical

`record_coactivation` writes two `hebbian_links` rows for a formed
link (one for each direction), but Phase B
`dual_write_hebbian_to_edges` canonicalises to one `edges` row
keyed by `(min(a,b), max(a,b))`.

Effect on `get_hebbian_links_weighted("a")`:

- Legacy SQL: `WHERE (source_id=? OR target_id=?) AND strength > 0`
  → matches **both** rows → returns `[(b, 1.0), (b, 1.0)]` for a
  single formed neighbour. **Pre-existing legacy duplicate bug.**
- Unified SQL: matches the one canonical row → returns `[(b, w)]`.

Test repro: see WIP stash test
`t29_4_get_hebbian_links_weighted_matches_across_paths` — fails
with `left: ["b", "b", "c", "c"], right: ["b", "c"]`.

### D2. Weight-semantics divergence (pre-existing, ISS-116 documented)

`record_coactivation`:
- Legacy `strength`: starts at 0 during tracking; jumps to 1.0 on
  threshold cross; +0.1 per call after, capped at 1.0.
- Unified `weight`: starts at 0.1 on first call; accumulates +0.1
  per call, no cap.

After N>10 coactivations: legacy strength=1.0, unified weight ≈
N×0.1. Already documented in `record_coactivation` rustdoc and the
ISS-116 issue body. **Not a bug; an explicit design split.**

But it interacts with D1 multiplicatively: legacy callers that sum
weights (e.g. `memory.rs:4412` recall scoring) get a doubled sum
from D1 *and* a different sum from D2. Switching the read flag
silently rescales every Hebbian-influenced recall score.

## Why Phase D §5.4 "one plan at a time" can't hide this

The §5.4 contract assumes legacy and unified return equivalent
result sets so callers don't change behavior the moment the flag
flips. For Hebbian readers, D1 + D2 make equivalence impossible:
either the unified side under-returns (one row instead of two), or
we make the unified side artificially emit duplicate rows (which
would be encoding a legacy bug in the new substrate — wrong root
fix).

## What needs to happen before T29.4 can land

Three options, in order of cleanliness:

### Option A (root fix): align writer policies first

Change `record_coactivation` (and `_ns`, `_cross_namespace`) to
**not** write a reverse row on threshold cross — make legacy
single-canonical too. Add a migration to dedupe existing legacy
rows. Then unified and legacy both return one row per pair, and
the readers can switch cleanly.

Cost: schema migration + writer behavior change + audit of every
existing legacy reader for direction sensitivity.

### Option B (read-side normalization): dedupe in legacy reader

Add `DISTINCT` (or canonical CASE in the SELECT) to legacy
`get_hebbian_*` SQL so it returns one row per pair like unified
does. Fixes D1. D2 still needs separate resolution.

Cost: changes legacy reader output, which may break callers
that depend on the duplication semantics (need audit — none of
the 8 prod callsites obviously do, but worth checking).

### Option C (defer): pin the current behavior, switch later

Accept that `get_hebbian_*` readers are not switchable in Phase D
without one of A/B above. Skip these readers in T29.4 and proceed
with the remaining four (`get_hebbian_neighbors_ns`,
`discover_cross_links`, `get_cross_namespace_neighbors`,
`get_all_cross_links`) — but they share the same row-shape
divergence so they'll hit the same wall. So Option C is really:
**T29.4 is blocked until A or B lands**.

## Recommendation

Option A — root fix. The legacy double-row pattern is a holdover
from a pre-Phase-B design where direction mattered; T14 already
collapsed directions on the unified side. Aligning legacy to match
is the natural step and unblocks all 7 hebbian readers at once.

Tracks the same audit-before-implement lesson from ISS-115/116:
fixing the writer asymmetry first is cheaper than working around
it in every reader.

## Out of scope

- ISS-118 (entity dual-WRITE gap — `link_memory_entity` /
  `upsert_entity_relation`) — separate from this; will surface at
  T29.5.

## Acceptance for closing

When `record_coactivation*` writers no longer create reverse legacy
rows (Option A) OR `get_hebbian_*` legacy readers dedupe by
canonical pair (Option B), and a parity test passes showing
`get_hebbian_links_weighted("a")` returns the same Vec on legacy
and unified handles to the same DB file across `record_coactivation`
+ `record_association` formed links + tracking-phase rows.

