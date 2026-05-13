---
id: ISS-113
title: "Phase C verifier I4 content spot-check: extend coverage from T19 to T20-T25 drivers"
status: open
priority: P2
severity: minor
labels:
  - v04-substrate
  - phase-c
  - verifier
  - follow-up
relates_to:
  - v04-unified-substrate
  - ISS-112
---

# Phase C verifier I4 content spot-check: extend coverage from T19 to T20-T25 drivers

## Context

T27 (Phase C parity verifier) ships in `crates/engramai/src/substrate/verify.rs` with five
invariants:

- **I1** count parity (all 7 drivers)
- **I2** audit row consistency (`backfill_runs` sum invariant)
- **I3** idempotency (gated, all 7 drivers)
- **I4** deterministic content spot-check — **T19 (memories) ONLY**
- **I5** FK closure (edges → nodes)

The design.md §8.4 spec literal — "counts + content spot-check" — is satisfied: I1 covers
all 7, I4 covers T19. But I4's purpose is to catch silent field corruption that pure row
counts would miss. Limiting it to T19 leaves the other 6 drivers protected only by
counts.

## What's missing

I4 needs per-driver spot-check helpers for:

### Pass-through drivers (simple)

These project legacy rows 1:1 into the unified tables, so a spot-check is straightforward:
sample legacy ids, fetch both sides, compare critical fields.

- **T20** `memory_embeddings → node_embeddings`: scalar fields (`memory_id`, `model`,
  `dim`, `created_at`) + blob equality on the vector payload.
- **T21** `entities → nodes(node_kind='entity')`: scalar fields + JSON-parsed
  `attributes` (T21 FINDING-1: `column.entity_type` wins over `metadata.entity_type` —
  the spot-check must assert that direction).
- **T25** `synthesis_provenance → edges(provenance/derived_from)`: legacy.id
  pass-through, scalar fields, JSON-parsed `attributes.gate_scores` round-trip (must
  parse to nested object, not quoted string).

### Merge-semantics drivers (existence-only)

These collapse many legacy rows into one unified row, so byte-equal comparison is
impossible. The right shape of assertion is "the unified row that should cover this
legacy row exists with the right `(kind, predicate, endpoints, namespace)` shape":

- **T22** `entity_relations → edges(structural minus T23 preds)`: sample legacy rows,
  resolve each to its deterministic edge id, assert the edges row exists with the right
  `edge_kind='structural'` and predicate NOT IN T23's set.
- **T23** `memory_entities → edges(union of provenance/mentions + structural/{subject,object}_of)`:
  sample legacy rows, resolve via role split (§3.3), assert the edges row exists with
  the role-mapped `(edge_kind, predicate)`.
- **T24** `hebbian_links → edges(associative/co_activated)`: sample legacy rows, resolve
  the canonical `(min, max)` pair, assert the edges row exists. Counter fields (`weight`,
  `coactivation_count`, `temporal_forward`, `temporal_backward`) are SUMs across legacy
  rows so cannot be field-equality-checked; instead assert they are `>=` the per-row
  legacy value (the SUM invariant gives a lower bound).

## Why not done now

- T19 helper alone is ~120 lines. Six more helpers + tests is ~600-800 lines.
- Spec is satisfied literally with T19-only.
- The merge-semantics three (T22/T23/T24) need a different assertion shape than
  pass-through, so the helper signatures will diverge — it's not a copy-paste exercise.

## Acceptance

- [ ] `spot_check_node_embeddings()` helper + tests (T20)
- [ ] `spot_check_entities()` helper + tests (T21, asserts column-wins direction)
- [ ] `spot_check_synthesis_provenance()` helper + tests (T25)
- [ ] `existence_check_entity_relations()` helper + tests (T22)
- [ ] `existence_check_memory_entities()` helper + tests (T23, role split)
- [ ] `existence_check_hebbian_links()` helper + tests (T24, SUM lower-bound)
- [ ] Each driver's helper wired into `check_content_spot_check()` dispatch
- [ ] Documented in module-level doc comment (currently states "I4 covers T19 only")
- [ ] Existing tests still pass (1877 lib + 73 phase C backfill + 20 verifier)

## Not in scope

- Changing the I4 sample size default (currently 32; tune separately if needed).
- Adding new invariants beyond I1-I5.
- Production rollout decisions — verifier is a tool, not a gate.

## Dependencies

None. T19-T25 are complete; verifier scaffolding is in place. This is pure additive work
inside `crates/engramai/src/substrate/verify.rs`.

## References

- `crates/engramai/src/substrate/verify.rs` — verifier module, `spot_check_memories()`
  as the prototype helper
- `crates/engramai/tests/v04_phase_c_verifier.rs` — five `t27_i4_*` tests as the test
  shape prototype
- `.gid/features/v04-unified-substrate/design.md` §3.3 (role split), §4.3 (Hebbian
  canonicalization), §5.3 (edge id derivation)
