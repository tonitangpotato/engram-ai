---
id: ISS-117
title: "T29.4 Hebbian read-switch blocked — legacy/unified parity not achievable without resolving weight & dup divergence"
status: done
priority: P1
severity: design
labels: [v04-unified-substrate, phase-d, read-switch, t29-blocker, divergence]
created: 2026-05-13
resolved: 2026-05-13
resolution: option-a-root-fix
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


---

## Resolution (2026-05-13)

**Picked Option A — root fix at the writer.** Three changes shipped
in one commit:

### Writer changes (`crates/engramai/src/storage.rs`)

1. `record_coactivation` (no-NS): both branches that previously
   wrote the reverse direction row (`source=id2 AND target=id1`)
   now update only the canonical `(id1, id2)` row. `id1, id2` is
   already swapped to `(min, max)` at the top of the function.
2. `record_coactivation_ns` (namespaced): same change.
3. `record_cross_namespace_coactivation`: same change. Cross-NS
   canonical ordering uses `(ns1, id1) < (ns2, id2)`, unchanged.

### Reader changes

`get_hebbian_neighbors` updated to OR-match
`(source_id = ?1 OR target_id = ?1)` with a CASE selecting the
non-self endpoint. The other six hebbian readers
(`get_hebbian_links_weighted`, `get_hebbian_neighbors_ns`,
`top_associates`, `discover_cross_links`,
`get_cross_namespace_neighbors`, `get_all_cross_links`) already
OR-matched, so no SQL change for them — but they now return one
row per pair instead of two.

### Migration (`migrate_hebbian_canonical_rows`)

Idempotent one-shot migration runs at `Storage::new` /
`Storage::with_unified_substrate`. For every `(a, b)` pair where
both `(a, b)` and `(b, a)` exist, the migration:

- Merges the reverse row's metrics into the canonical `(min, max)`
  row: `strength = max`, `coactivation_count = sum`,
  `temporal_forward/backward = sum`, `created_at = min`.
- Deletes the reverse row (`source_id > target_id`).

Re-running on an already-canonical table is a no-op.

### Side benefits

Closing the writer asymmetry **incidentally fixes two pre-existing
legacy bugs**:

- `memory.rs:4412` recall scoring summed `strength` across the dup
  pair → 2× over-score on formed Hebbian neighbours. Now correct.
- `storage.rs:2708` `merge_hebbian_links` `transferred` count
  doubled because the donor-touching `links` Vec contained the
  reverse row → caller saw 2× transfer count. Now correct.

### Audited prod callers

Before changing writer/reader, every prod caller of
`get_hebbian_neighbors` / `get_hebbian_links_weighted` was audited
for direction sensitivity:

- `memory.rs:1426` (GWT spreading activation) — `dedup()` after push,
  same-id rows are adjacent in SQL output → dup-tolerant.
- `memory.rs:4412` (recall scoring) — sums strength → dup-sensitive
  but currently 2× over-scored. Fix removes the bug.
- `memory.rs:5486/5495/5848/5887/5895` — public API delegations to
  Storage methods, no scoring logic of their own.
- `promotion.rs:131` — `HashSet` dedup → dup-tolerant.
- `synthesis/cluster.rs:573` — `HashMap or_insert` on canonical pair
  → dup-tolerant.
- `storage.rs:2683` (`merge_hebbian_links`) — dup-sensitive
  (transferred count). Fix removes the bug.

No caller depends on the duplication semantics for correctness;
every dup-sensitive site is a pre-existing bug.

### Verification

- 1902/1902 lib pass
- 9/9 `iss117_canonical_hebbian` integration tests pass:
  - `record_coactivation_forms_single_canonical_row`
  - `record_coactivation_canonicalizes_id_order`
  - `get_neighbors_works_in_either_direction`
  - `get_hebbian_links_weighted_no_duplicates`
  - `record_coactivation_ns_forms_single_canonical_row`
  - `record_cross_namespace_coactivation_forms_single_canonical_row`
  - `migration_collapses_double_direction_rows` (with metric
    merge: max/sum/sum/sum/min)
  - `migration_is_idempotent`
  - `migration_leaves_single_direction_rows_alone`
- 26/26 Phase B dual-write tests pass (no regression on T14/ISS-116
  edges-mirror behavior)
- 42/42 Phase C verifier tests pass
- All Phase C backfill drivers + lifecycle paths green

### Unblocks

T29.4 (Phase D hebbian read-switch) — legacy row shape now matches
unified row shape, so contract tests for
`unified_substrate=true` reads against `edges WHERE
edge_kind='associative'` can assert byte equality with legacy
output. D2 (weight accumulation divergence: legacy `strength` caps
at 1.0, unified `weight` accumulates without cap) remains; that's
a design split that does not block read-switch parity as long as
contract tests don't compare weights, only row shape & neighbour
identity. Documented as a Phase D known divergence in §5.4
(followup: harmonise legacy strength or accept as permanent).

### WIP fate

`stash@{0}` (T29.4 part-1 WIP) dropped after this resolution — the
SQL switching pattern will be different now that legacy is
canonical (the if/else flag-gating in the stash is still valid,
but the legacy branch no longer needs DISTINCT or canonicalisation
band-aids).

### Follow-up (2026-05-13): cross-axis coverage gap exposed by ISS-118

The 9-test ISS-117 suite that landed in 4163f36 happened to pass only
by id choice. Every cross-NS test used pairs like `x_in_ns1`/`y_in_ns2`
where the id ordering ("x" < "y") agrees with the namespace ordering
("ns1" < "ns2"). Under that pair the migration's pre-ISS-118 raw-id
DELETE coincidentally agreed with the writer's `(ns, id)` tuple
canonical rule, hiding the bug.

ISS-118 (`5eff26b`) added three regression tests that exercise the
cross-axis case (id-order inverts ns-order). Going forward, every
cross-NS hebbian test pair should either:

1. use ids whose lex-order inverts the namespace lex-order, or
2. exercise both directions explicitly,

so that future regressions don't slip past via accidental id choice.

T29.4 part-1..4 contract tests do NOT have this exposure because they
operate on same-namespace pairs (where the tuple comparison collapses
to id comparison), but the cross-NS readers (part-5, part-6) MUST use
cross-axis pairs in their contract tests.
