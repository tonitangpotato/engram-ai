---
title: Phase B dual-write loses contradicts/contradicted_by for memory→nodes; blocks T29.7 hot retrieval switch
status: open
priority: P1
severity: degradation
filed_by: rustclaw (overnight session 2026-05-14)
relates_to:
  - v04-unified-substrate
  - T29.7
---

## Symptom

The `row_to_record_impl` decoder (storage.rs:7154) reads `contradicts` and `contradicted_by` from the row by name. Both columns exist on `memories` but **not** on `nodes`. The T12 dual-write helper `insert_memory_node_row` (storage.rs:1744) projects `memories.metadata` into `nodes.attributes` but does **not** project `contradicts` or `contradicted_by` anywhere. A `SELECT * FROM nodes WHERE node_kind='memory'` therefore cannot reconstruct a complete `MemoryRecord` — at minimum it loses `contradicted_by`.

This silently breaks two downstream consumers when unified-substrate reads switch on:

1. `crates/engramai/src/confidence.rs:75` / `:181` — confidence calculation branches on `record.contradicted_by.is_some()` to apply a contradiction penalty.
2. `crates/engramai/src/models/actr.rs:92` — ACT-R activation function applies a `penalty = if record.contradicted_by.is_some() { … }` term.

If a unified `search_fts` / `fetch_memory_record` / `fetch_recent` returns `contradicted_by = None` for memories that legitimately were contradicted, both confidence and activation get inflated → wrong ranking, wrong promotion decisions, silently degraded recall quality. No test currently catches this because all T12 dual-write tests assert byte-equal *writes*, not read-side reconstruction.

## Discovery context

Found during the 2026-05-13 → 14 overnight Phase D push, while scoping the hot retrieval reader switch (T29.7 in the revised plan; was a single T29 bullet before sub-task split). See `.gid/features/v04-unified-substrate/PHASE-D-READER-AUDIT.md` for the full reader-by-reader gap analysis.

## Why it wasn't caught earlier

- T12 acceptance tests asserted "byte-equal parity of writes via dual-write" — a writer-side guarantee.
- T17 row-count parity tested cardinality only.
- T13/T14/T15/T16 expanded the dual-write surface but did not add per-column round-trip tests against the decoder.
- T29.1–T29.4 read switches all returned id-or-tuple types that did not exercise `row_to_record_impl`.

## Affected files (reader side, all blocked until fix lands)

- `storage.rs::all` (2245)
- `storage.rs::all_in_namespace` (3356)
- `storage.rs::get_by_ids` (2261)
- `storage.rs::search_fts` (2527)
- `storage.rs::search_fts_ns` (3296)
- `storage.rs::fetch_recent` (2562)
- `storage.rs::search_by_type` (2592)
- `storage.rs::search_by_type_ns` (2468)
- `storage.rs::list_superseded` (3241)
- `storage.rs::list_deleted` (3725)
- `storage.rs::fetch_memory_record` (7090, free fn)
- `storage.rs::fetch_memory_record_with_namespace` (7113)
- `storage.rs::get_memories_by_ids` (6376)

## Fix options

### Option A (preferred) — stamp into `attributes` JSON

1. In `insert_memory_node_row`, when building the attributes JSON, merge in two reserved keys: `_legacy_contradicts` and `_legacy_contradicted_by` (only when the source field is non-empty).
2. Add a `row_to_record_from_node_impl(row)` decoder companion to `row_to_record_impl`. It reads the unified columns it can map directly and parses the attributes JSON for the reserved keys.
3. Add a Phase C backfill patch: `backfill_contradicts_into_node_attributes` — walks legacy `memories` rows where `contradicts != ''` or `contradicted_by != ''`, merges those fields into the corresponding `nodes.attributes`. Idempotent.
4. Tests: round-trip ingest with `contradicted_by = Some(...)`, dual-write, read back via unified path, assert `contradicted_by.is_some()` survives.

Pros: no schema migration, contained blast radius, fits the existing "attributes is the JSON sidecar" pattern.
Cons: special reserved keys leak schema concerns into JSON; reserved-key gate (graph/store.rs `validate_attributes`) needs updating to allow these as system-owned.

### Option B — add typed columns to `nodes`

`ALTER TABLE nodes ADD COLUMN contradicts TEXT`, `ALTER TABLE nodes ADD COLUMN contradicted_by TEXT`. Dual-write fills them. Decoder reads them directly.

Pros: cleaner data model, no JSON parsing overhead.
Cons: schema churn touches every reader/writer of `nodes`; risks introducing more divergence at exactly the moment we're trying to consolidate.

### Recommendation

Option A. Smaller change, lets us unblock Phase D quickly, and the reserved-key convention can be lifted into a proper schema later if it proves load-bearing.

## Acceptance criteria

- [ ] `insert_memory_node_row` projects non-empty `contradicts` / `contradicted_by` into `attributes` JSON under reserved keys.
- [ ] New decoder `row_to_record_from_node_impl` round-trips both fields.
- [ ] Backfill driver patches existing dual-written rows that were written before this fix.
- [ ] Round-trip test in `tests/iss119_contradicts_round_trip.rs`: ingest memory with `contradicts: Some("X")`, `contradicted_by: Some("Y")`, dual-write, query via unified-substrate read path, assert both survive.
- [ ] Confidence / ACT-R penalty tests: ingest contradicted memory, route through unified reader, verify penalty fires.
- [ ] Phase D §8.5 T29.7 unblocked.

## Out of scope

- T29.4 hebbian readers (already shipped; their return types do not touch this).
- `metadata` field round-trip — already works (T12 projects `record.metadata` into `attributes` already).
