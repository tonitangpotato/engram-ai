---
id: ISS-113
title: 'Phase C verifier I4 content spot-check: extend coverage from T19 to T20-T25 drivers'
status: done
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

## Resolution (2026-05-13)

Shipped across 3 commits:

- 423fb48 — T20 memory_embeddings (compound-key sampler + byte-equal
  BLOB comparison)
- d24a46a — T21 entities (FINDING-1 column-wins regression guard)
  + T25 synthesis_provenance (gate_scores nested-object round-trip)
- 36d5bbb — T22 entity_relations + T23 memory_entities (role split
  + endpoint direction) + T24 hebbian_links (canonical-pair SUM
  lower-bound check)

All 7 Phase C drivers now have I4 content spot-check coverage.
Total: 22 → 42 verifier tests, no regressions in 1877 lib tests or
73 Phase C backfill tests.

### Design discovery

T23 implementation surfaced a spec drift: design §3.3 line 320
documents `subject_of: entity → memory` for role='subject', but the
T23 driver (substrate/backfill.rs:1457) writes `memory → entity` for
ALL roles, and the existing
`t23_subject_role_writes_structural_subject_of` integration test
locks the as-built behavior. Verifier matches as-built. Docs fix
tracked separately (not in scope for ISS-113).

### Visibility changes

- `substrate::backfill::uuid_from_hash` and `role_to_kind_predicate`
  changed from private to `pub(crate)`. Used by the verifier to share
  the deterministic-id formula and role map. Public API surface
  unchanged.

### Coverage shape per driver

- **Pass-through (field-equal)**: T19 (memories), T20 (embeddings),
  T21 (entities — incl. FINDING-1 column-wins), T25 (synthesis
  provenance — incl. parsed-JSON gate_scores)
- **Merge-semantics (existence + shape)**: T22 (entity_relations —
  predicate-collision guard for T22 vs T23), T23 (memory_entities —
  role-mapped (kind, predicate), as-built endpoint direction),
  T24 (hebbian_links — canonicalized pair, SUM lower-bound on
  counter fields)

Phase D (read-switch) can now proceed against a content-verified
parity gate.
