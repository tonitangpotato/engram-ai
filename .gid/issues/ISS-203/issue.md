---
id: ISS-203
title: Entity canonicalization fragments one person into dozens of nodes — case-fold merge fails + possessive/prepositional phrases become standalone entities
status: open
priority: P1
severity: data-quality
tags: [unified-substrate, graph, entity, canonicalization, resolution, locomo]
created: 2026-05-31
relates_to: [ISS-202]
parent: ISS-202
---

# ISS-203: entity canonicalization fragments one person into dozens of nodes

> **One-line:** the resolution pipeline does not (a) case-fold for merge,
> nor (b) strip possessive/prepositional wrappers to the head noun. One
> person ("Caroline") becomes 20+ separate entity nodes. Even after
> ISS-202 reconnects edge provenance, the entity bridge stays diluted
> because anchor resolution can land on the wrong fragment.

## Relationship to ISS-202

ISS-202 is the **primary** fix: it repopulates `edges.source_memory_id` so
the `factual` plan can seed the gold memory from a traversed edge. That
unlocks the *mechanism*. ISS-203 is the **amplifier**: it ensures the
anchor the plan resolves on, and the entity the gold edge hangs off, are
the **same node**. Both are needed for the entity bridge to fire reliably.
ISS-202 lands first (single-point, directly measurable). ISS-203 value is
quantified only after ISS-202 is in.

## Evidence (live DB `.tmpa0Kbrm/substrate.db`, conv-26, 2026-05-31)

### Defect (a) — case-fold merge fails

Two nodes for the same person:

| node id | content | attributes |
|---|---|---|
| `1d11ce4c…` | `caroline` | `{"entity_type":"person"}` (DictionaryMatch) |
| `ce689add…` | `Caroline` | `{"_legacy_kind":"person","kind_source":"DictionaryMatch"}` |

The gold edge for conv-26 q104 (`Caroline --uses--> Becoming Nicole`,
memory_id `ad15485c`) hangs off `ce689add`. A query that resolves its
anchor to the lowercase `1d11ce4c` would miss the bridge entirely.

### Defect (b) — possessive / prepositional phrases as standalone entities

~20 `"Caroline's X"` nodes, e.g.:

```
Caroline's advice          (artifact? no — TripleHint)
Caroline's artwork         (artifact)
Caroline's city            (place)
Caroline's commitment to LGBTQ rights advocacy
Caroline's drive
Caroline's experience
Caroline's group
Caroline's happiness
Caroline's identity
Caroline's inspiration to make art
Caroline's journey as a trans woman
Caroline's motivation
Caroline's own experience of being helped
Caroline's paintings
Caroline's support
Caroline's talk
…
```

Plus prepositional forms: `conversation with Caroline`,
`support from Caroline`, `bothering Caroline`. Same pattern for
`Melanie's *`. None are stripped to the head noun (`Caroline`/`Melanie`)
nor linked back to the canonical person node.

### Scale of fragmentation

- `nodes` entity rows on conv-26: **694**
- `graph_memory_entity_mentions` distinct entity_ids: **699**
- Many of these 694–699 are phrase fragments of a handful of real people.

## Code location to investigate

- `resolution/pipeline.rs` — the resolution stage ordering; where entity
  drafts are matched against existing canonical entities.
- `resolution/entities.rs` — the §3.4.3 entity decision algebra
  (CreateNew vs MergeInto). This is where (i) case-folding for the match
  key should happen, and (ii) a head-noun-extraction / possessive-strip
  normalizer should run before the match.
- The extractor that emits `"Caroline's X"` as an entity span (upstream of
  resolution) — decide whether to strip at extraction or at resolution.
  Leaning toward **resolution-time normalization** (single chokepoint,
  keeps the extractor dumb).

## Proposed fix direction (NOT locked — investigate first)

1. **Case-fold the match key** (not the stored display name) so
   `caroline` and `Caroline` resolve to the same canonical node. Preserve
   original casing in `content`/display; fold only for the dedup key.
2. **Possessive/prepositional normalizer** before the match: strip
   `"X's Y"` → consider `X` as the head person AND `Y` as a separate
   concept linked via a possessive edge (or drop `Y` if it's not a real
   entity). Strip `"<prep> X"` (with/from/about Caroline) → `X`.
3. **Merge migration** for already-ingested DBs (re-point edges + mentions
   from fragment nodes to the canonical node, then soft-delete fragments).

## Acceptance criteria

- **AC-1** `caroline` and `Caroline` resolve to ONE canonical entity node
  (case-fold match key). Display casing preserved.
- **AC-2** `"Caroline's X"` / `"<prep> Caroline"` spans no longer create
  standalone person-fragment nodes; the head person resolves to the
  canonical node.
- **AC-3** Merge migration for existing DBs re-points edges + mentions and
  soft-deletes fragments; idempotent.
- **AC-4** conv-26 same-DB A/B (post-ISS-202) shows the entity-bridge
  questions (q104 class) benefit from de-fragmentation, with no regression.
- **AC-5** Entity-node count on conv-26 drops materially (fragments
  collapsed); spot-check that no two display-distinct real entities were
  wrongly merged.

## Out of scope

- Predicate quality (`uses`/`is_a`/`implements` being code-flavored rather
  than conversational `read`/`recommended`) — that is a separate defect
  (ISS-202 secondary (b)); file independently if pursued.
