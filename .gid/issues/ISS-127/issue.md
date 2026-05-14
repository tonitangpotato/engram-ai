---
title: Storage::insert_triple_entity bypasses ISS-123 dual-write — triple-derived entities not reflected in nodes/edges under unified_substrate=true
status: open
priority: P2
severity: degradation
labels: [v04-unified-substrate, phase-b, dual-write, triple-extraction]
relates_to: [ISS-123]
discovered_during: T26a
---

# Summary

`Storage::store_triples` (the writer T26a's backfill driver delegates to) calls `Storage::insert_triple_entity` for each subject/object endpoint. `insert_triple_entity` writes directly to the legacy `entities` and `memory_entities` tables via raw SQL — it does **not** go through the dual-write helpers that ISS-123 (`link_memory_entity`) and T13 (`insert_entity`) wired up for the v0.4 unified substrate.

Result: under `unified_substrate=true`, every triple-derived entity is present in `entities` / `memory_entities` (legacy) but missing from `nodes(kind='entity')` / `edges(kind='provenance', predicate='mentions')`. Read-switch consumers (T29.5 entity readers, T29.3 embeddings) will miss these entities until a subsequent backfill (T21 entities, T23 memory_entities) is run.

# Reproduction

1. Open a DB with `unified_substrate=true`.
2. Call `Storage::store_triples("mem-1", &[Triple {subject: "alpha", predicate: …, object: "beta", …}])`.
3. Assert `SELECT COUNT(*) FROM entities WHERE name IN ('alpha','beta')` returns 2.
4. Assert `SELECT COUNT(*) FROM nodes WHERE node_kind='entity' AND id IN ('triple-<hash_alpha>', 'triple-<hash_beta>')` returns **0** (bug).

# Why this slipped through

T13 dual-write was scoped to `ResolutionPipeline::insert_entity` and `apply_graph_delta`. ISS-123 dual-write was scoped to `link_memory_entity`. `insert_triple_entity` is a **third writer path** that nobody noticed when those two were audited, because:

- It's a private method on `Storage` — easy to miss in a grep for `entities` writers.
- It's only invoked from `store_triples`, which itself is only called from the consolidation path (`memory.rs:4849`).
- The Phase B writer audit (2026-05-14, ISS-119–126) focused on the public API surface, not internal helpers chained off of it.

# Root fix

Refactor `insert_triple_entity` to call the existing helpers:

- For the entity row: `dual_write_entity_to_nodes(self.conn(), entity_id, "concept", "triple", &metadata)` (same shape as T13).
- For the memory_entities link: route through `link_memory_entity(memory_id, entity_id, "triple")` — this already dual-writes to `edges(kind='provenance', predicate='mentions')` per ISS-123.

This is a small, surgical change: replace ~15 lines of raw SQL with two helper calls. The two helpers were extracted exactly for cases like this.

# Test plan

- New contract test in `tests/v04_phase_b_dual_write.rs`: call `store_triples`, assert `entities`/`memory_entities` AND `nodes`/`edges` rows both land with consistent ids.
- Existing T26a tests already exercise `store_triples` indirectly — once the fix lands, the T26a-with-flag-on path will produce correct unified-substrate state without any change to the backfill driver itself.

# Out of scope

- Backfilling existing DBs where `store_triples` has already run before this fix: covered by re-running T21 (entities → nodes) and T23 (memory_entities → edges) backfills. No new driver needed.
- Triple structure (subject—predicate→object) as a structural edge: still deliberately out of scope per T29.5; the `triples` table remains the source of truth for the relation itself.

# Severity rationale

P2 / degradation, not P1, because:
- The fix is mechanical (≤30 LOC).
- Under flag-off (default), there is no observable bug.
- Under flag-on, T21 + T23 backfills already converge state after the fact.
- New triples emitted *between* this fix landing and the next backfill run would be missing, but the consolidation path is not currently triggering on every memory write in production.

# Discovery context

Found during T26a (v04-unified-substrate §8.4) implementation while reading `store_triples` to understand whether the backfill driver's delegation cascades through dual-write. The driver itself is correct; the writer it delegates to is the gap.
