---
title: Phase B dual-write loses EntityKind variant for entity→nodes; blocks T29.5 entity-reader switch
status: fixed
priority: P1
severity: degradation
filed_by: rustclaw (overnight session 2026-05-14)
fixed_by: de6f675
relates_to:
  - v04-unified-substrate
  - T29.5
  - ISS-119
---

## Symptom

`dual_write_entity_to_nodes` (graph/store.rs:892) maps `Entity.kind: EntityKind` to `nodes.node_kind` but only as the binary bifurcation:

```rust
let node_kind: &str = match &entity.kind {
    crate::graph::entity::EntityKind::Topic => "topic",
    _ => "entity",
};
```

The original `EntityKind` variant (Person / Concept / Event / Place / Organization / …, plus Topic) is **only** preserved on the legacy side via `graph_entities.kind = kind_to_text(&entity.kind)`. On the unified side, every non-Topic variant collapses into the single string `"entity"`. There is no `attributes` JSON projection of `kind_label` either.

This blocks any unified read that needs to return the original `entity_type`:

- `EntityRecord.entity_type` is read by every consumer of `get_entity` / `find_entities` / `list_entities`.
- `entity_stats` returns a per-type breakdown.
- Find-by-type filters (e.g. "list all Person entities") cannot be satisfied from unified.

## Discovery context

Found during the 2026-05-13 → 14 overnight Phase D push, scoping T29.5 entity-reader switch. See `.gid/features/v04-unified-substrate/PHASE-D-READER-AUDIT.md` for the broader gap survey. This is the entity-side twin of ISS-119.

## Affected reader paths (blocked until fix lands)

- `storage.rs::get_entity` (4741)
- `storage.rs::find_entities` (4654)
- `storage.rs::list_entities` (4786)
- `storage.rs::entity_stats` (4854)

## Fix options

### Option A (preferred) — stamp `kind_label` into `attributes` JSON

1. In `dual_write_entity_to_nodes`, compute `kind_label = kind_to_text(&entity.kind)?.trim_matches('"')`.
2. Before serializing `attributes_json`, merge in the reserved key `_legacy_kind = kind_label`.
3. Reader-side helper `entity_record_from_node_row` reads `attributes` JSON and reconstructs `entity_type`.
4. Phase C backfill patch: `backfill_entity_kind_into_node_attributes` — walks `graph_entities`, copies `kind` into the corresponding `nodes.attributes._legacy_kind`. Idempotent.
5. Tests: round-trip ingest of each `EntityKind` variant, read back via unified path, assert `entity_type` matches.

Pros: contained, mirrors the ISS-119 strategy, no schema churn.
Cons: reserved keys in attributes JSON (same concern as ISS-119); reserved-key gate needs updating.

### Option B — widen `nodes.node_kind` to carry full variant

Drop the binary `'entity'` / `'topic'` and instead use the serialized `EntityKind` directly as `node_kind`. Every consumer of `node_kind` (filter clauses, indices, joins) needs updating.

Pros: cleanest data model.
Cons: large blast radius — `node_kind` is referenced across retrieval plans, KC, working-memory, future cognitive extensions. Doing this mid-Phase-D risks breaking T29.1–T29.4 readers that already shipped.

### Recommendation

Option A. Same rationale as ISS-119: smaller change, faster unblock, reversible.

## Acceptance criteria

- [ ] `dual_write_entity_to_nodes` stamps `_legacy_kind` (or equivalent reserved key) into `attributes` JSON.
- [ ] Reader-side helper reconstructs `EntityRecord.entity_type` from unified rows.
- [ ] Backfill driver patches existing dual-written rows.
- [ ] Round-trip test in `tests/iss120_entity_kind_round_trip.rs`: ingest one entity per `EntityKind` variant, read back via unified path, assert variant matches.
- [ ] Phase D §8.5 T29.5 entity readers unblocked.

## Open question

Should `entity_stats` per-type breakdown be re-defined against `node_kind` (binary) or against `_legacy_kind` (variant)? Consumers of `entity_stats` need to declare which level of granularity they want before this is implementation-ready.

## Out of scope

- Topic-specific containment edges (T15) — those work fine, `'topic'` survives node_kind dispatch.
- Entity-relation `kind` field — that lives on `edges` rows (T22), independent of this issue.
