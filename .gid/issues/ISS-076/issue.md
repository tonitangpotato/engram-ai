---
id: "ISS-076"
title: "All graph_edges endpoint UUIDs are dangling — edges have no resolvable subject or object entity"
status: open
priority: P0
labels: [resolution, edges, root-cause, v0.3, locomo]
relates_to: [ISS-075, ISS-072, ISS-068]
---

# ISS-076: graph_edges endpoint UUIDs do not match any graph_entities row

## TL;DR

In RUN-0007 substrate (and likely all prior v0.3 substrates), every edge in `graph_edges` references `subject_id` and `object_entity_id` UUIDs that **do not exist in `graph_entities`**. 125 live edges, 88 distinct subject UUIDs, 113 distinct object UUIDs — zero of them join back to an entity row.

This is independent of (and worse than) ISS-075 (dedup failure). Even after ISS-075 is fixed, this bug means edges still can't be traversed from any entity.

## Evidence

```
Edge subject IDs found in entities table:
  distinct_subjects = 88
  subjects_in_entities_table = 0   ← all dangling

Edge object_entity IDs found in entities table:
  distinct_objects = 113
  objects_in_entities_table = 0    ← all dangling
```

Source: `.gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db`, queried with `LEFT JOIN graph_entities ON e.subject_id = ent.id`.

Sample edge rows show predicate_label populated correctly (`leads_to`, `related_to`, `part_of`, `uses`) but BLOB UUID columns rendered as garbage when JOINed — confirming the IDs are real UUIDs, just pointing at entities that don't exist (or exist under different IDs).

## Hypothesis on root cause

The resolution pipeline has two stages that allocate entity IDs:

1. **`stage_extract`** parses LLM triples into `(subject_mention, predicate, object_mention)`. Each mention gets an internal ID (or text key) for downstream stages.
2. **`stage_persist`** writes to both `graph_entities` and `graph_edges`.

The dangling-UUIDs pattern strongly suggests **the IDs assigned to mentions during extract/resolve are not the same IDs that get inserted into `graph_entities` at persist time**, but **are** the IDs written into `graph_edges.subject_id` / `object_entity_id`. Likely candidates:

- Two parallel ID allocation paths (one for the entity row, one for the edge endpoint), drifting under some condition.
- Edge persist runs before entity persist commits, and falls back to "transient" mention IDs that never get reconciled.
- The CreateNew short-circuit (ISS-075) generates a fresh `Entity::new()` UUID at persist time, but `resolve_edges` already cached the *pre-resolution mention's* UUID — so subject_id ≠ entity.id.

The third hypothesis is the most likely given ISS-075 — when every entity is `CreateNew`, the "pre-resolution mention UUID" the edge stage saw is never the "freshly allocated entity UUID" the persist stage wrote.

## Verification (need to confirm before fixing)

- Read `pipeline.rs::resolve_edges` and `stage_persist.rs` together. Identify where edge subject/object IDs come from and where entity IDs come from.
- Print pipeline trace for a single conv-26 turn and check whether `EntityResolution::new_id` matches the `Edge::subject_id` for triples whose subject is that entity.

## Why this matters for benchmarks

- Spreading activation walks from anchor entities along edges. If `entity.id` ≠ any `edge.subject_id`, walks terminate immediately at the anchor — explaining why retrieval has been treating Caroline as a leaf node despite 27 mention copies and many "X-related-to-Caroline" triples.
- Edge-based retrieval signals (predicate frequency, relation-typed traversal) are unusable until this is fixed.
- Hebbian co-activation links between entities can't be derived from edges either.

## Acceptance criteria

On a fresh ingest of LoCoMo conv-26:

- [AC-1] `SELECT COUNT(*) FROM graph_edges e WHERE NOT EXISTS (SELECT 1 FROM graph_entities ent WHERE ent.id = e.subject_id) AND e.invalidated_at IS NULL` = 0 (no dangling subjects)
- [AC-2] Same query for `object_entity_id` (where `object_kind = 'entity'`) = 0 (no dangling objects)
- [AC-3] At least one Caroline entity has `>0` outgoing edges (after ISS-075 dedup fix; before that, at least one *of the 27* should).

## Out of scope

- ISS-075 (pipeline never writes alias/embedding) — separate root cause; both must be fixed.
- ISS-074 (entity enrichment fields default) — orthogonal.
- Spreading activation algorithm itself — runs fine *if* the graph is consistent; this issue is about graph consistency.

## Verification command (current state)

```bash
sqlite3 .gid/eval-runs/RUN-0007-substrate/locomo-conv26-iss072.graph.db \
  "SELECT
     (SELECT COUNT(*) FROM graph_edges e LEFT JOIN graph_entities ent ON e.subject_id=ent.id 
        WHERE ent.id IS NULL AND e.invalidated_at IS NULL) AS dangling_subjects,
     (SELECT COUNT(*) FROM graph_edges e LEFT JOIN graph_entities ent ON e.object_entity_id=ent.id 
        WHERE ent.id IS NULL AND e.invalidated_at IS NULL AND e.object_kind='entity') AS dangling_objects;"
# Expected (current): 125 | 113
# Expected (fixed):   0   | 0
```
