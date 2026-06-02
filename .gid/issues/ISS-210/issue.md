---
title: storage.rs entity nodes-projection leaves last_seen/first_seen NULL → map_candidate_row crash drops anchor
status: in_progress
priority: 1
labels: bug, retrieval, entity-resolution, root-cause-locked
relates_to: ISS-209, ISS-204, ISS-205
---

# ISS-210: NULL last_seen on storage.rs entity nodes-projection crashes anchor resolution

## Summary

`Storage::insert_entity_node_row` (the `storage.rs` legacy entity write
path, reached via `upsert_entity`) projects the entity into the unified
`nodes` table **without setting `first_seen` / `last_seen`** — both
columns are left NULL. The resolution-pipeline write path
(`graph/store.rs`) DOES set them.

`GraphStore::map_candidate_row` reads `last_seen` via
`row.get::<_, f64>(3)?`, which returns
`InvalidColumnType(3, "last_seen", Null)` for these rows.
`GraphEntityResolver::resolve()` swallows the error
(`Err(_) => continue` at graph_entity_resolver.rs:285) and silently drops
that entity as an anchor candidate.

## Impact (the q0 chain)

This is the residual blocker after ISS-209 unified the caroline/Caroline
id (commit `ce9075fd`). With the split gone, ONE node `3b017c3e`
owns all 28 `occurred_on` edges incl the gold `2023-05-07` support-group
date. But that node was written through the `storage.rs` path → NULL
`last_seen` → `search_candidates('Caroline')` ERRORs → resolver returns
anchors `[support, group, support group, LGBTQ support group]` with
**Caroline absent**. Temporal reservation (ISS-205) calls
`edges_of(anchor, OccurredOn)`; with no Caroline anchor the gold edge is
never reserved → gold not in top-10 → conv-26-q0 answers "I don't know".

Confirmed: only **3 of 683** entity nodes have NULL `last_seen` — exactly
the 3 rows that also live in the legacy `entities` table (the storage.rs
write path). Caroline is one of them.

## Root cause (locked, SQL-verified on .tmpKQ4zq9/substrate.db)

`storage.rs::insert_entity_node_row` `INSERT OR IGNORE INTO nodes (...)`
omits `first_seen`/`last_seen` from the column list.
`graph/store.rs` (resolution pipeline) sets
`first_seen, last_seen, created_at, updated_at` and on conflict does
`last_seen = max(nodes.last_seen, excluded.last_seen)`.

## Fix (A + B + C)

- **A (root):** `storage.rs::insert_entity_node_row` — add
  `first_seen`/`last_seen` to the INSERT, set both to the `created_at` /
  `updated_at` timestamps passed in (matching the resolution pipeline's
  initial-write semantics).
- **B (harden):** `graph/store.rs::map_candidate_row` — read `last_seen`
  as `Option<f64>` and coalesce NULL → 0.0, so a stray NULL never crashes
  candidate mapping again.
- **C (observability):** `graph_entity_resolver.rs::resolve()` — log the
  swallowed `search_candidates` error instead of silently
  `Err(_) => continue`, so a future drop is visible.

## Acceptance criteria

- [ ] AC-1: `insert_entity_node_row` populates `first_seen`/`last_seen`;
      new entity nodes have non-NULL `last_seen`.
- [ ] AC-2: `map_candidate_row` coalesces NULL `last_seen` → 0.0 (unit
      test: NULL row maps without error).
- [ ] AC-3: resolver logs (not silently swallows) candidate-search errors.
- [ ] AC-4: `search_candidates('Caroline')` returns a candidate (no
      InvalidColumnType error) on a freshly-ingested conv-26 DB.
- [ ] AC-5: conv-26-q0 flips 0→1 and aggregate recovers to ≥ 0.3026
      baseline (`bash /tmp/iss209_q0arm.sh`).

## Evidence

- Probe `examples/iss209_anchor_caroline_probe.rs`:
  `search_candidates('Caroline')` => `InvalidColumnType(3,'last_seen',Null)`;
  `support`/`group`/`LGBTQ support group` => 1 candidate each.
- `sqlite3`: 3/683 entity nodes NULL `last_seen` = the 3 `entities`-table rows.
- q0 arm `2026-06-02T18-16-16Z_locomo`: q0=0.0, overall 0.2895.
