---
title: link_memory_entity has no Phase B dual-write to edges (T23 mirror gap)
status: fixed
priority: P1
labels:
- v04-unified-substrate
- phase-b
- dual-write
relates_to:
- ISS-121
- ISS-122
---

## Problem

`Storage::link_memory_entity` (crates/engramai/src/storage.rs:4911) inserts
into the legacy `memory_entities` table only. There is **no** symmetric
write to the unified `edges` table, even though T23 (Phase C backfill)
projects `memory_entities` rows into `edges` with one of three
`(edge_kind, predicate)` pairs:

  * `(provenance,  mentions)`   — role = "mention" / "" / unknown
  * `(structural, subject_of)`  — role = "subject"
  * `(structural, object_of)`   — role = "object"

This is the **same gap** ISS-121 (soft_delete) and ISS-122
(upsert_entity) closed for their respective writers. Without it,
between Phase D flipping read paths to `unified_substrate=on` and
Phase E stopping legacy writes, any newly extracted entity mention
is **invisible to unified reads**:

  * Mention is INSERTed into `memory_entities` ✅
  * Mention is NOT projected into `edges` ❌
  * `list_entities` under unified-substrate returns `mention_count=0`
    for that entity even though the legacy column would show > 0.

## Surface

Production call sites (all in `memory.rs`):
  * `2477` — store_raw resolution path (candidate)
  * `2526` — store_raw resolution path (existing)
  * `2609` — store_raw fallback path
  * `6093` — re-extraction path

Test call sites in `lifecycle.rs` (199, 200).

## Fix Plan

1. Reuse `role_to_kind_predicate` logic from
   `substrate/backfill.rs::role_to_kind_predicate` (already canonical).
2. In `link_memory_entity`:
   * Compute `(edge_kind, predicate, _normalized)` from `role`.
   * Compute deterministic edge id via the same hash recipe T23 uses:
     `"memory_entities|{memory_id}|{entity_id}|{role}|{edge_kind}|{predicate}"`.
   * Stamp `attributes` with `{"role": "<raw>"}` iff role was
     normalized (preserves the T23 round-trip contract).
   * INSERT into `edges` via the existing
     `Storage::insert_structural_edge_row` / `insert_provenance_edge_row`
     helpers (FK pre-check, INSERT OR IGNORE on deterministic id).
3. Wrap both writes in a single transaction so they cannot diverge
   under partial failure (matches ISS-122 contract).
4. Contract test in `tests/iss123_link_memory_entity_dual_write.rs`:
   * "mention" role → `(provenance, mentions)` edge present
   * "subject" role → `(structural, subject_of)` edge present
   * "object" role → `(structural, object_of)` edge present
   * unknown role → `(provenance, mentions)` + `attributes.role` stamped
   * idempotent re-link (INSERT OR IGNORE)

## Blocker for

T29.5 part-3 (`list_entities` unified read) cannot get a meaningful
mention_count under unified-substrate without this fix, because new
mentions written via `link_memory_entity` after Phase B was thought
"complete" would be missing from edges. Tests for part-3 currently
have to seed both tables manually, which masks this gap.

## Out of scope

Phase E (T34–T37) will remove the legacy write — at that point this
function should write to `edges` only.
