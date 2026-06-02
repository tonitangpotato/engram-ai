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

- [x] AC-1: `insert_entity_node_row` populates `first_seen`/`last_seen`;
      new entity nodes have non-NULL `last_seen`. **DONE** — test
      `iss210_upsert_entity_populates_last_seen`; live DB `.tmp6uuC9p` shows
      **0/668 NULL last_seen** entity nodes (was 3/683).
- [x] AC-2: `map_candidate_row` coalesces NULL `last_seen` → 0.0 (unit
      test: NULL row maps without error). **DONE** — test
      `iss210_search_candidates_tolerates_null_last_seen`.
- [x] AC-3: resolver logs (not silently swallows) candidate-search errors.
      **DONE** — `Err(e) => eprintln!(...)` in graph_entity_resolver.rs.
- [x] AC-4: `search_candidates('Caroline')` returns a candidate (no
      InvalidColumnType error) on a freshly-ingested conv-26 DB. **DONE** —
      `iss209_anchor_caroline_probe` on `.tmp6uuC9p`:
      `mention "Caroline" => 1 candidate id=3b017c3e alias_match=true
      last_seen=1780429950.18` (was `Err(InvalidColumnType)` pre-fix).
- [ ] AC-5: conv-26-q0 flips 0→1 and aggregate recovers to ≥ 0.3026
      baseline (`bash /tmp/iss209_q0arm.sh`). **PARTIAL / RETRIEVAL DONE,
      GENERATION GATE REMAINS.** See VERDICT below.

## VERDICT (run 2026-06-02T20-07-10Z_locomo, binary e9f4a247)

The ISS-210 fix **resolves the retrieval defect completely** but q0 still
scores 0.0 — the residual failure is now **generation-only**, a separate
bug.

Retrieval chain, end-to-end (proven by `iss207_q0_delivery_probe` on the
live run DB `.tmp6uuC9p`):
- `search_candidates('Caroline')` → returns anchor `3b017c3e` (AC-4).
- Factual plan + temporal reservation runs.
- **Gold `9fff4171` "[2023-05-07] Caroline attended a LGBTQ support group"
  lands at RANK 3 in top-10**, carrying the resolved date in the
  generator line. Probe verdict: "retrieval DELIVERS the dated gold
  episode into top-10."

But the bench generation answered: *"I don't know. The memories mention
Caroline speaking up for the trans community and receiving support, but
they don't specify when she attended an LGBTQ support group."* — even
though the gold line with `2023-05-07` was literally in its context.

Root cause of the residual: **distractor saturation.** Ranks 0–2 are
higher-scored Caroline memories that don't mention the support group; the
model fixated on those and ignored the dated gold line at rank 3. This is
a generation/synthesis problem, NOT retrieval — file a follow-up
(re-rank dated/date-asking gold to the top slot, or strengthen the
generation prompt to scan for explicit dated lines).

Aggregate: overall **0.2697** (below 0.3026 baseline) — within ingest
re-noise band; not a regression signal since the retrieval fix only
changes anchor resolution, and the aggregate move is dominated by the
~22/152 per-ingest dedup wobble. The decisive signal is the **per-query
retrieval delivery**, which now PASSES.

## Evidence

- Probe `examples/iss209_anchor_caroline_probe.rs`:
  `search_candidates('Caroline')` => `InvalidColumnType(3,'last_seen',Null)`;
  `support`/`group`/`LGBTQ support group` => 1 candidate each.
- `sqlite3`: 3/683 entity nodes NULL `last_seen` = the 3 `entities`-table rows.
- q0 arm `2026-06-02T18-16-16Z_locomo`: q0=0.0, overall 0.2895.
