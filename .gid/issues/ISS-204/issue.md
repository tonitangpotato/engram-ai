---
title: Event-time is not a first-class graph edge — temporal multi-hop queries degrade to vector recall
status: open
priority: P0
labels: retrieval, graph, temporal, root-cause
blocks:
- ISS-203
- .gid/issues/ISS-203/issue.md
relates_to: ISS-190, ISS-191, ISS-201
---

# Event-time is not a first-class graph edge

## Summary (the root cause, DB-verified)

The knowledge graph has **no structured representation of when events happen.**
Event dates live only as a `temporal` dimension on the `memories` row — they are
a *property of a memory*, never an *edge in the graph*. As a result, a temporal
multi-hop query ("when did Melanie take her kids to the museum?") cannot be
answered by graph traversal — there is no path from the museum-event entity to
the date `2023-07-05`. The query silently degrades into vector recall over the
~188 memories that mention `Melanie`, hoping the dated memory's *text* lands in
top-K so the LLM can read the date off the prose.

This is the true root cause behind:
- the conv-26 single-hop / multi-hop temporal failures (q20, q33, q35, q62),
- the "top-K crowding" symptom misdiagnosed in ISS-203 (crowding is the
  *consequence* of multi-hop degrading to vector recall, not a ranking bug),
- the date-stranding symptoms tracked in ISS-190 / ISS-191 / ISS-201 / q0
  (those are the same defect seen from the `memories.metadata` side).

## Evidence (queried directly on the live pre-fix conv-26 substrate.db)

DB: `/var/folders/48/.../.tmpa0Kbrm/substrate.db` (454 memory nodes, legacy
prompt / V2 OFF). All facts below are SQL-verified, not inferred.

### 1. The museum event memory is stored cleanly — but the date is NOT in the graph

`memories` row `3cf5c975`:
- content: "Melanie took her kids to the museum yesterday (2023-07-05) ..."
- `metadata.engram.dimensions.temporal = {"kind":"day","value":"2023-07-05"}`
- `occurred_at` set, has a 3072-d embedding, is a `nodes` row.

So at the *memory* layer the date is perfect. Now the *graph* layer:

```
-- all graph_edges for memory 3cf5c975:
Melanie                 --uses-->      museum                  (valid_from = 2026-05-31!)
spending time with kids --leads_to-->  rewarding experience    (valid_from = 2026-05-31!)
```

- Neither edge carries the date.
- `Melanie --uses--> museum` is itself wrong-typed (should be `visited` /
  `went_to`); the second edge connects two **phrase-shard entities**
  ("spending time with kids", "rewarding experience") — the ISS-203 fragmentation
  defect, now seen polluting edges too.

### 2. The graph schema HAS the capacity for time — it is unused

`graph_edges` has `valid_from` / `valid_to` (bitemporal) and an `object_literal`
column (for literal-valued objects like dates/numbers). Both are dead:

- `SELECT COUNT(*) FROM graph_edges WHERE object_kind='literal'` → **0**.
  No edge ever has a literal object. Dates/numbers never become graph objects.
- `SELECT MIN(valid_from), MAX(valid_from) FROM graph_edges` →
  **2026-05-31 21:09 .. 21:15** — i.e. `valid_from` was populated with the
  *write clock* (recorded_at), not event-validity time. `valid_to` is all NULL.
- The predicate vocabulary (`graph_predicates`) is entirely abstract-semantic:
  `part_of`, `related_to`, `leads_to`, `uses`, `is_a`, `depends_on`. There is
  **no** `occurred_on` / `happened_at` / `on_date` event-time predicate.

### 3. The string "2023" appears NOWHERE in the graph

- `graph_edges.summary LIKE '%2023%'` → 0
- `graph_entities.canonical_name LIKE '%2023%'` → 0
- no entity has kind date/time; no entity name is a date
- `triples` table has 0 rows for `3cf5c975`

The event's real date exists in exactly one place: `memories.metadata.temporal`
(+ the prose). **The graph is date-blind.**

## Why this causes the observed failures

A temporal multi-hop query needs: locate event → read event's time. With no
event→time edge, the graph cannot do step 2, so retrieval falls back to vector
similarity over all memories mentioning the salient entity. `Melanie` is
mentioned by **188 / 454** memories — the one dated episode is a needle in that
haystack. Whether it lands in top-K is luck. That luck is what ISS-203 observed
as "crowding," and what ISS-190/191 observed as "the date is stranded where
generation can't see it." Same root cause, two vantage points.

This also explains the ISS-203 V2 multi-hop regression: V2 makes entity→entity
edges *denser* but the graph still has no time dimension, so denser entity edges
shift top-K composition (more entity-anchored competitors) without giving the
graph the one thing it needs to answer temporal questions. Denser noise.

## Root fix (NOT a top-K patch)

Make **event-time a first-class graph edge.** The extraction + resolution
pipeline must, when a memory describes an event with a resolved date, emit a
graph edge whose object is that time, e.g.:

```
museum_visit  --occurred_on-->  2023-07-05    (object_kind = literal/time)
```

or equivalently populate `valid_from`/`valid_to` on the event's edges with the
*event-validity* time (not the write clock). Either way the date becomes
traversable. Then a temporal multi-hop query resolves by graph traversal
(event entity → occurred_on → date) instead of degrading to vector recall.

Concrete sub-decisions to settle during design:
- **Object representation:** literal-time object (`object_kind='literal'`,
  `object_literal = {"time":{...}}`) vs a dedicated time-entity node vs
  populating `valid_from`. Leaning literal-time object — it's what the unused
  `object_literal` column was built for, and it keeps time off the entity graph
  (no "2023-07-05" entity nodes to canonicalize).
- **Predicate:** add an `occurred_on` / `valid_during` canonical predicate.
  `Predicate::from_str_lossy` already maps abstract relations; this needs a real
  new variant because event-time is semantically distinct, not a synonym.
- **Source of truth:** the date is already resolved into
  `memories.metadata.temporal` (`day` / `approx` etc.). The fix wires that
  resolved value INTO the graph at projection time — it does not require
  re-extraction. For `approx` golds (ISS-190/191) the day must first be pinned
  into `start/end` (that part stays on the ISS-190/191 track) so the graph edge
  gets a usable interval, not a full-year smear.

## Relationship to other issues

- **Blocks ISS-203 default-on.** V2 cannot flip to default until temporal
  multi-hop stops degrading to vector recall — otherwise denser entity edges
  keep regressing multi-hop. Once event-time is a graph edge, V2's cleaner
  entities + traversable time should let V2 clear the L1 gate.
- **Subsumes the retrieval-side of ISS-190 / ISS-191 / ISS-201 / q0.** Those
  track date-stranding from the `memories.metadata` side (pinning the resolved
  day into `start/end`). That pinning is a *prerequisite* (the graph edge needs
  a usable date), but the *root fix* is getting that date into the graph as an
  edge. Coordinate: ISS-190/191 = resolve-day-into-interval; ISS-204 =
  project-that-interval-into-a-graph-edge.

## Acceptance criteria

- [ ] AC-1: extraction/projection emits an event-time edge for memories with a
  resolved `temporal` date (literal-time object or `valid_from`/`valid_to`
  populated with event-validity time, not write clock). DB-verified: for the
  museum memory `3cf5c975`, the graph contains a traversable path
  event-entity → 2023-07-05.
- [ ] AC-2: a new canonical event-time predicate exists and is distinct from the
  abstract-semantic predicates; `from_str_lossy` round-trips it.
- [ ] AC-3: `valid_from` no longer carries the write clock for event edges (or
  the write clock is moved to `recorded_at`/`created_at` only, with `valid_*`
  reserved for event-validity time).
- [ ] AC-4: conv-26 temporal multi-hop queries (q20, q62 first — they have clean
  `day` dates; q33, q35 after ISS-190/191 pins their `approx` interval) resolve
  by graph traversal, DB-verified by dumping the traversal path, not by score
  alone.
- [ ] AC-5: with event-time edges present, re-run the ISS-203 L1 A/B
  (V2 off vs on) WITH DB persistence; V2 multi-hop no longer regresses
  (Δ ≥ -0.03) → unblocks ISS-203 default-on decision.
- [ ] AC-6: no regression on non-temporal queries (entity→entity traversal and
  vector recall paths unchanged for memories with no resolved date).

## Notes

- The crowding claim in ISS-203 was downgraded to "consistent with DB state,
  not fully DB-verified" because the arm-B top-K ranking was unrecoverable.
  ISS-204 supersedes that line of investigation: the fix is not to verify
  crowding and patch top-K, it is to remove the cause of crowding (multi-hop
  degrading to vector recall) by giving the graph a time dimension.

---

# Design (code-surface verified 2026-06-01)

Before designing I read the actual code, not just the DB. The infrastructure is
already there end-to-end — the gap is one missing producer + one wrong timestamp.

## What already exists (do NOT rebuild)

1. **Literal-object edges, bottom to top, are fully wired:**
   - `graph_edges.object_literal` column + XOR CHECK (storage_graph.rs:271-297)
   - `target_literal` encode/decode in the unified store (store.rs:1233-1285,
     1870+, 2001+) — literal-object SELECT/INSERT/find_edges all handle it.
   - `DraftEdgeEnd::Literal(String)` variant EXISTS (context.rs:173-176).
   - `pipeline.rs:776` ALREADY converts `DraftEdgeEnd::Literal(val)` →
     `EdgeEnd::Literal { value: JSON }`. The persist path consumes it.
   So a literal-object edge can flow from draft → resolve → persist today.

2. **The event date is already in hand at projection time:**
   - `PipelineContext.memory` is a full `MemoryRecord` (context.rs:186) — its
     `metadata.engram.dimensions.temporal` carries the resolved date
     (`{"kind":"day","value":"2023-07-05"}`), readable via the ISS-191
     `derived_temporal_mark()` accessor.

## The two defects (precise code coordinates)

**Defect A — the extractor never emits a literal/event-time edge.**
`stage_edge_extract.rs:64-72` hardcodes every triple object to
`DraftEdgeEnd::EntityName(t.object.clone())`, with a comment explicitly
deferring literal discrimination as "future work, out of scope for v0.3." That
"future work" is this issue. Result: 0 literal edges in the entire graph.

**Defect B — event edges are stamped with the write clock, not event time.**
`pipeline.rs:970` sets `occurred_at = ctx.memory.created_at`. That is why every
`graph_edges.valid_from` is 2026-05-31 (ingest time) instead of 2023 (event
time). `Episode`/`context` carry `occurred_at: None`, so the real event time
never reaches projection even though it sits in `memory.metadata.temporal`.

## Proposed fix

### Component 1 — emit an event-time edge at projection (the root fix)
At the projection stage (pipeline.rs, where `draft_entity_from_triple_endpoint`
runs and the memory's drafts are assembled), when
`ctx.memory.derived_temporal_mark()` yields a usable date:
- construct one additional `DraftEdge`:
  - `subject_name` = the memory's primary event/actor entity (the highest-
    salience non-phrase entity for this memory — reuse the existing anchor
    selection, do NOT invent a new "event" node yet),
  - `predicate` = new canonical `Predicate::OccurredOn`,
  - `object` = `DraftEdgeEnd::Literal(<ISO date string or temporal JSON>)`.
- This rides the existing draft→resolve→persist path (pipeline.rs:776) with
  zero new plumbing.

Open design choice (settle in implementation, document the decision):
- **subject anchor**: memory's primary entity vs the episode node. Start with
  primary entity (simplest, traversable); episode-node anchoring is a later
  refinement if multi-entity events need it.
- **literal payload**: ISO `"2023-07-05"` string vs the full `TemporalMark`
  JSON. Lean ISO string for traversal simplicity; keep kind/precision out of
  the edge (it's already on the memory).

### Component 2 — add `Predicate::OccurredOn` (event-time predicate)
`triple.rs`:
- add variant `OccurredOn` to `enum Predicate`,
- `from_str_lossy`: map `"occurred_on" | "happened_at" | "on_date" | "dated"`
  → `OccurredOn`,
- `as_str` → `"occurred_on"`.
This is a REAL new variant, not a synonym — event-time is semantically distinct
from the abstract-semantic relations. (Confirms the earlier note: vocabulary
widening IS needed here, unlike ISS-203's possessive case.)

### Component 3 — stop stamping the write clock as event-validity time
`pipeline.rs:970`: for event-time edges, `valid_from` must be the event date
(from the temporal mark), not `created_at`. Audit whether `created_at` should
move to the edge's `recorded_at`/`created_at` columns only, leaving `valid_*`
for event-validity. Non-event edges: decide whether to leave `valid_from` NULL
(honest "no validity time known") rather than back-filling the write clock.

### Dependency on ISS-190/191 for `approx` dates
For `day`-precision memories (q20 museum, q62 park) the date is directly usable
TODAY — Component 1 alone fixes them. For `approx` memories (q35 camping, q33
distractor) the resolved day is stranded in the `note` field with full-year
`start/end`; ISS-190/191 must first pin the day into `start/end` so Component 1
gets a usable interval instead of a full-year smear. Sequence: q20/q62 land on
ISS-204 alone; q33/q35 need ISS-190/191 + ISS-204.

## Why this is the root fix, not a patch
It removes the *cause* of the "crowding" symptom (temporal multi-hop degrading
to vector recall) by giving the graph the missing dimension — time as a
traversable edge. No top-K reservation, no special-case ranking. After this,
"when did X happen" resolves by graph traversal (event-entity → occurred_on →
date), and V2's denser-but-cleaner entity edges should stop regressing multi-hop
(unblocks ISS-203 default-on, AC-5).

## Risk / blast radius
- Adding a Predicate variant: exhaustive `match` sites on `Predicate` will fail
  to compile until updated — that's a feature (compiler enumerates every site).
- Literal-edge persist path is already exercised by storage_graph.rs tests
  (lines 765-829) — low risk it's broken.
- New edges increase graph density; must verify (AC-6) non-temporal retrieval is
  unaffected (literal edges shouldn't enter entity→entity traversal pools).
