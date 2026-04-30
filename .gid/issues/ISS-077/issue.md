---
title: Same-episode duplicate mentions miss in-batch dedup → CreateNew → entity duplication
status: open
priority: P1
labels: resolution, dedup, root-cause
discovered_during: ISS-076 / ISS-075 root-fix planning, 2026-04-30
relates_to:
  - ISS-075
  - ISS-076
---

# ISS-077: Same-Episode Mention Dedup Missing

## TL;DR

Inside a single episode, if the same canonical name is mentioned twice, **the second mention does not see the first**. `retrieve_candidates` queries the graph store, but the first mention has not yet been persisted (we are still inside `resolve_entities`). Result: both mentions go down the `CreateNew` path → two distinct entities → mention edges split → spreading activation can never bridge them.

This is **independent of ISS-075** (no embedding) and **independent of ISS-076** (id mismatch). Even after both fixes, in-episode duplicates will still split unless we add an in-batch lookup.

## Where

- File: `crates/engramai/src/resolution/pipeline.rs`
- Fn: `Pipeline::resolve_entities` (around line 492)
- The `decisions: Vec<EntityResolution>` accumulates resolved drafts but is never consulted by subsequent `retrieve_candidates` calls in the same loop.

## Reproduction

A LoCoMo conv-26 episode that mentions "Caroline" twice in the same session is a candidate:
1. Draft 1 ("Caroline") → `retrieve_candidates(name=Caroline, embedding=None)` → store empty for this episode → `CreateNew(uuid_A)`.
2. Draft 2 ("Caroline") → `retrieve_candidates` again → store still empty (draft 1 not yet persisted) → `CreateNew(uuid_B)`.
3. `apply_graph_delta` writes both. Two Caroline entities, two disjoint mention sets.

The 27-Caroline cogmembench symptom is partly this: even within sessions, repeated mentions split.

## Root fix

Add an in-batch resolution map inside `resolve_entities`:

```rust
let mut in_batch: HashMap<(EntityKind, String), Uuid> = HashMap::new();
for draft in drafts {
    let key = (draft.kind, normalize(&draft.canonical_name));
    if let Some(&existing_id) = in_batch.get(&key) {
        decisions.push(EntityResolution::ExistingInBatch(existing_id));
        continue;
    }
    let decision = match retrieve_candidates(...) { ... };
    if let EntityResolution::CreateNew(id) = &decision {
        in_batch.insert(key, *id);
    }
    // also seed in_batch from MergeWith / aliases so subsequent same-batch mentions hit
    decisions.push(decision);
}
```

Notes:
- "normalize" should match how aliases normalize (lowercase, trim, collapse whitespace) — reuse the same fn as the alias resolver, do not invent a new one.
- For `MergeWith(existing_id)` decisions, also seed the map so a same-batch repeat doesn't trigger another store query.
- `ExistingInBatch` is just a tag for telemetry; structurally it produces the same `mention → entity_id` edge as `MergeWith`.

## Why P1, not P0

P0 is reserved for ISS-076/075 because those are the headline cause of the 27-Caroline symptom (cross-episode split). Fixing those alone will move metrics. ISS-077 is the long-tail dedup completeness issue that will start dominating after the headline fix.

## Acceptance

- Unit test in `pipeline.rs`: feed a synthetic episode with two `"Caroline" / Person` drafts (no embeddings). Assert the resulting `GraphDelta` has exactly **one** new entity, two `MENTIONS` edges to it.
- After ISS-077 lands, re-run cogmembench LoCoMo conv-26 and confirm zero same-episode duplicate Caroline (count by `(canonical_name, kind, episode_id)` group).

## History

- 2026-04-30: filed during ISS-076 / ISS-075 root-fix discussion. Discovered while reading `resolve_entities` and noticing `retrieve_candidates` only queries the store, never the in-progress `decisions` vec.
