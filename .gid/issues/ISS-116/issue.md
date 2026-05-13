---
id: ISS-116
title: "Phase B dual-WRITE gaps in hebbian_links writers — record_coactivation, decay, merge"
status: in_progress
priority: P1
severity: degradation
labels: [v04-unified-substrate, phase-b, dual-write, t29-blocker]
created: 2026-05-13
blocks: T29.4
---

# Phase B dual-WRITE gaps in hebbian_links writers

## Summary

Phase B T14 (commits 3a43406 + 0f8f3fa + d7c5613) shipped
`dual_write_hebbian_to_edges` and wired it into **three** of the
hebbian_links writers:

- `record_coactivation_ns`  (storage.rs:3672)
- `record_cross_namespace_coactivation` (storage.rs:3778)
- `record_association` (storage.rs:5387)

Audit during ISS-115 closure found **four additional hebbian_links
writers** that were not wired, leaving Phase D unified reads of the
hebbian neighborhood at risk of phantom/missing-row drift once T29.4
flips the read-switch for hebbian readers.

This is the **same shape** as ISS-115 (writer/deleter asymmetry) but
on the writer side: every prod path that mutates `hebbian_links` must
also mutate `edges` (edge_kind='associative') so the two row sets stay
in lockstep until Phase D fully promotes `edges` to source-of-truth.

## Missing dual-write writers

### 1. `record_coactivation` (storage.rs:2508) — `&mut self`, namespace-agnostic

Live prod caller: `synthesis/engine.rs:694` (synthesis flush path),
plus lifecycle tests. SQL operations:

- UPDATE strength+0.1 (already-formed link)
- UPDATE strength→1.0 (threshold crossing)
- INSERT strength=1.0 (reverse link on threshold crossing)
- INSERT strength=0.0, count=1 (tracking phase)

No `dual_write_hebbian_to_edges` call.

**Behavior choice**: mirror `record_coactivation_ns` — wrap body in
`Transaction`, append **one unconditional** dual-write call at the
end with `namespace="default"`, `signal_source="corecall"`,
`signal_detail="{}"`, `delta_weight=0.1`. This matches the existing
namespaced writer's policy (every recall increments edges.weight by
0.1, regardless of which legacy branch fired). Documented semantic
divergence between legacy (strength=0 during tracking) and edges
(weight accumulates from the first recall) is **pre-existing** in
T14's namespaced writer and intentionally preserved for consistency
across the three coactivation writers — see follow-up note below.

### 2. `decay_hebbian_links` (storage.rs:2575) — `&self` (will need `&mut`)

Bulk multiplicative decay:
`UPDATE hebbian_links SET strength = strength * ? WHERE strength > 0`.

Missing mirror:
`UPDATE edges SET weight = weight * ? WHERE edge_kind='associative' AND weight > 0`.

### 3. `decay_hebbian_links_differential` (storage.rs:2659) — `&self`

Bulk differential decay with `CASE WHEN signal_source = …` predicate
selecting per-signal decay rates. Schema impedance: `hebbian_links`
has a real `signal_source` column; `edges` stores it inside the
`attributes` JSON.

Mirror approach:
```sql
UPDATE edges
SET    weight = weight * (CASE
         WHEN json_extract(attributes, '$.signal_source') = 'corecall'  THEN ?1
         WHEN json_extract(attributes, '$.signal_source') = 'entity'    THEN ?2
         WHEN json_extract(attributes, '$.signal_source') = 'temporal'  THEN ?3
         ELSE ?4
       END)
WHERE  edge_kind = 'associative' AND weight > 0;
```

Slower than a column-backed predicate but fully correct. Optimisation
(materialised generated column / partial index by signal_source) is a
separate concern (FOLLOWUP-ISS-116-perf).

### 4. `merge_hebbian_links` (storage.rs:2596) — `&self` (will need `&mut`)

Donor-merge cleanup: when memory `donor_id` is merged into `keep_id`,
re-point all hebbian_links rows where source_id=donor_id or
target_id=donor_id over to keep_id, summing weights where the keep
side already had a link to the same neighbour.

This is the most complex of the four mirrors. The unified-side
equivalent must:

1. For every edges row with `edge_kind='associative'` and
   `(source_id=donor_id OR target_id=donor_id)`, derive
   `other_id` = the non-donor endpoint, then
2. UPSERT into edges with `(keep_id, other_id)` canonicalised
   (`lo,hi`) summing weights through the existing
   `ON CONFLICT (… , json_extract(signal_source))` path, then
3. DELETE the donor-side rows.

A single-statement SQL is awkward because the canonicalisation
swap-on-write requires per-row decision; safest is read-merge-write
inside the same transaction. Will gate `merge_hebbian_links` to
`&mut self` and run both legacy and unified mutations in one tx.

## Out of scope

- `migrate_hebbian_signals` (storage.rs:1243) is a one-shot schema
  migration that backfills `signal_source='corecall'` on pre-Phase-B
  rows. Phase B dual-write puts `signal_source` into `edges.attributes`
  for every new row, so there is no missing mirror — old rows
  (pre-migration) have no edges counterpart at all (created before
  Phase B). Out of scope.
- `link_memory_entity` / `upsert_entity_relation` — separate
  writer-gap on `memory_entities` / `entity_relations` tables. Will
  file ISS-117 when T29.5 entity read-switch surfaces it.

## Acceptance

- All four writers run in a transaction that wraps both the legacy
  hebbian_links mutation and the matching edges mutation.
- A new test file (or expansion of `v04_phase_b_dual_write`) pins
  per-writer parity: after the call, every hebbian_links row touched
  has a 1:1 edges row with the same endpoints, summed weight, and
  signal_source.
- 1902/1902 lib pass preserved (+ new tests).
- 21/21 v04_phase_b_dual_write + 42/42 v04_phase_c_verifier preserved.
- Document semantic divergence in T14 dual-write (tracking-phase
  strength=0 vs edges-weight accumulation) — pre-existing, pinned by
  test, deferred resolution.

## Why not deferred to T29.4 implementation

ISS-115 just established the precedent that writer/deleter asymmetry
must be closed **before** the matching read-switch — same logic
applies here. Leaving the four writers asymmetric would mean T29.4
hebbian read-switch lands on an `edges` table that's both **missing
rows** (from `record_coactivation` synthesis-driven coactivations not
flowing through ns variant) and **over-weighted** (legacy decay
multiplies strength, edges weight stays put), producing arbitrary
correctness regressions in neighbor-strength-sensitive recall paths.

Filed and fixed in one PR with ISS-116 referencing this issue from
each commit message.
