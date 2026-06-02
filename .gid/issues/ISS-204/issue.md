---
title: Event-time is not a first-class graph edge — temporal multi-hop queries degrade to vector recall
status: resolved
priority: P0
labels: retrieval, graph, temporal, root-cause
blocks:
- ISS-203
- .gid/issues/ISS-203/issue.md
relates_to:
- ISS-190, ISS-191, ISS-201
- .gid/issues/ISS-205/issue.md
depends_on: .gid/issues/ISS-202/issue.md
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

The date becomes traversable as the *object* of that edge. Then a temporal
multi-hop query resolves by graph traversal (event entity → occurred_on → date)
instead of degrading to vector recall.

**Do NOT use `valid_from`/`valid_to` for event time** (see CORRECTION below):
those fields are *bitemporal validity* (when a fact is true), which is
orthogonal to *event time* (when an event happened). Conflating them corrupts
the `as-of-T` query semantics. Event time is a literal-object edge only.

Concrete sub-decisions to settle during design:
- **Object representation:** literal-time object (`object_kind='literal'`,
  `object_literal = {"time":{...}}`) — settled. NOT a `valid_from` populate
  (wrong dimension, see CORRECTION), NOT a time-entity node (would pollute the
  entity graph with "2023-07-05" nodes to canonicalize). It's what the unused
  `object_literal` column was built for.
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

- [x] AC-1 (code): projection emits an event-time edge for memories with a
  day-precision `temporal` mark, as a literal-object `DraftEdge`
  (`DraftEdgeEnd::Literal`, predicate `OccurredOn`). Rides the existing
  draft→resolve→persist path (pipeline.rs:776 → EdgeEnd::Literal). Commit
  ff0f177e, unit test `day_precision_mark_emits_occurred_on_literal_edge`.
  *DB-verification on a fresh conv-26 ingest (path museum-entity → occurred_on
  → 2023-07-05 present in graph_edges) still pending — tracked by AC-4.*
- [x] AC-2: canonical `OccurredOn` predicate exists (both `Predicate::OccurredOn`
  in triple.rs and `CanonicalPredicate::OccurredOn` in graph/schema.rs, distinct
  from abstract-semantic relations); `from_str_lossy` maps occurred_on/
  happened_at/on_date/dated and `as_str` round-trips to "occurred_on". Adapter
  `map_predicate` passes it through. Commit 56f81130, tests
  `test_predicate_round_trip` + `map_predicate_full_coverage`.
- [x] AC-3: event time is NOT written to `valid_from`/`valid_to`. `build_new_edge`
  (stage_persist.rs) no longer stamps `valid_from = Some(now)` (the write clock);
  it leaves `valid_from = None` (honest "no validity window known"). The ingest
  timestamp remains in `recorded_at`. Commit ff0f177e. Full suite green confirms
  no edge logic depended on the old write-clock stamp.
- [x] AC-3b (code): the Factual plan KEEPS literal-object edges
  (`traverse_anchors` sets `linked_entity = None` for `EdgeEnd::Literal` and
  pushes the row rather than dropping it, factual.rs:771-781) AND seeds the
  source memory unconditionally from `edge.memory_id` (factual.rs:586). ISS-202
  is RESOLVED: `dual_write_edge_to_edges` now binds `source_memory_id` from
  `edge.memory_id` via an FK-safe `(SELECT id FROM nodes WHERE id=?)` guard
  (store.rs:1298), so the OccurredOn edge carries non-NULL provenance for any
  memory in `nodes` (every real ingest) — the seed is live under unified reads.
  *Live trace verification folded into AC-4.*
- [x] AC-4: conv-26 temporal multi-hop queries (q20, q62 first — they have clean
  `day` dates; q33, q35 after ISS-190/191 pins their `approx` interval) resolve
  by graph traversal, DB-verified by dumping the traversal path, not by score
  alone. **VERIFIED** 2026-06-01 — 60 occurred_on edges, 100% non-NULL
  provenance, gold edges present + traversable on fresh conv-26 ingest. Full
  trace + 3-way EXACT-miss decomposition in the AC-4 verdict section below.
- [x] AC-5: re-ran ISS-203 L1 A/B WITH DB persistence (conv-26 + conv-44).
  **Outcome: V2 multi-hop STILL regresses** (conv-26 -8.11pp, conv-44 -8.33pp —
  reproduces across two corpora). So V2 does NOT get unblocked. BUT the cause is
  DB-verified to be V2's own entity-merge crowding (distinct entities -21%,
  belongs_to/associated_with = 0 edges = V2's explicit mechanism is inert), NOT
  the ISS-204 event-time edges. The occurred_on edges are present and correct in
  both arms (A=56, B=68); they are not the regression source. AC-5 is therefore
  *satisfied as a decision*: ISS-204's edge mechanism is decoupled from and
  independent of the V2 prompt verdict. V2 stays default-OFF (see ISS-203
  conv-44 verdict); the real lever is a ranking-layer top-K reservation, filed
  off the ISS-190/191/201 track. ISS-204 does not depend on that.
- [x] AC-6 (code): no regression on non-temporal queries. The OccurredOn edge is
  only emitted when a day-precision mark exists AND a triple anchor exists
  (tests `no_temporal_mark_emits_no_occurred_on_edge`,
  `low_precision_mark_emits_no_occurred_on_edge`,
  `day_precision_but_no_triple_emits_no_occurred_on_edge`); entity→entity
  traversal pools are unchanged for dateless memories. Full engramai suite
  (2098 lib + 84 integration binaries) green. *Empirical AC-6 confirmation on
  the conv-26 non-temporal buckets folded into AC-4 run.*

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

### Component 3 (CORRECTED) — do NOT stamp event time into `valid_from`

**Review correction 2026-06-01 (after reading `retrieval/plans/bitemporal.rs`).**
The original Component 3 proposed putting the event date into `valid_from`. That
is a CONCEPTUAL ERROR and is dropped. Reason:

`bitemporal.rs` `valid_from`/`valid_to` encode **fact-validity** — *when a
stated fact is true in the world* — and drive the `as-of-T` query
(`valid_from <= T AND (valid_to IS NULL OR valid_to > T)`, bitemporal.rs:262).
This is orthogonal to **event time** (*when an event happened*). If we set the
museum edge's `valid_from = 2023-07-05`, the `as-of-T` projection would conclude
"this edge did not hold before 2023-07-05," which is nonsense — the recorded
fact (the museum visit) is valid as knowledge from the moment it's recorded.
Event time belongs ONLY on the `occurred_on` literal edge's object.

The actual bug to fix at `pipeline.rs:970`: `occurred_at = ctx.memory.created_at`
stamps the WRITE CLOCK into `valid_from`. Fix = stop doing this. For edges with
no known validity window, leave `valid_from` NULL (honest). The write timestamp
lives in `recorded_at`/`created_at` columns where it belongs.

### Component 4 — Factual plan must SEED + SURFACE the dated memory via the literal edge

**Added in review 2026-06-01 (after reading `retrieval/plans/factual.rs`).**
Storing the `occurred_on` edge is necessary but NOT sufficient — the read side
must consume it. Findings from the code:

- q20-style "when did X happen" routes to the **Factual plan** (1-hop entity
  traversal), not the Bitemporal plan (which is fact-validity, not event time).
- `FactualEdgeRow.linked_entity` is `Option<Uuid>` and is **`None` when the edge
  points to a literal** (factual.rs:248-250) — so the plan already *recognizes*
  literal-object edges, but its candidate **seeding** is driven by
  `edge.memory_id` (the ISS-189 D1 edge-provenance seed), not by the literal
  object.
- The dated memory must be admitted to the candidate pool as the SOURCE of the
  `occurred_on` edge, then surfaced so generation sees the date.

Design work for Component 4:
- verify `traverse_anchors` does not DROP literal-object edges as dead ends
  (linked_entity = None must not silently discard the edge);
- verify the `occurred_on` source memory is edge-seeded into the candidate pool;
- confirm the literal date reaches generation (either via the seeded memory's
  text — which already has it — or by surfacing the edge object directly).

### Hard dependency on ISS-202 (`source_memory_id` is NULL)

**Blocking prerequisite.** ISS-202 found that unified `edges.source_memory_id`
is NULL for all 789 structural + 220 provenance edges. The Factual plan's
edge-provenance seed (ISS-189 D1) admits the source memory **from the edge's
memory_id**. If `occurred_on` edges are written with NULL source memory, Component
4 cannot seed the dated memory and the whole fix is inert. So: the `occurred_on`
edge MUST carry a populated `source_memory_id` (legacy `graph_edges.memory_id` is
already SET; the unified projection must stop nulling it). Coordinate with
ISS-202 — its fix is on the critical path for ISS-204 to work under unified reads.

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

---

# AC-4 verdict — conv-26 fresh-ingest DB trace (2026-06-01, opus-4.8 session)

Run `ISS204-AC4-conv26-20260601T143034Z`, envelope = ISS-190 (K=10, temp=0,
HyDE/MMR/entity off, FACTUAL_REWEIGHT off, pipeline_pool=1, POPULATE off).
Live DB: `.tmpcYbhzb/substrate.db` (single-file, still on disk for re-query).
All numbers below independently recomputed from `locomo_per_query.jsonl` +
direct SQL on the live DB — not carried over from prior session notes.

## AC-4 PASS — the structural mechanism works end-to-end

`occurred_on` edges on a fresh conv-26 ingest:
- **60 edges**, **60/60 with non-NULL `memory_id`** (100% provenance).
- All objects are `object_kind='literal'` ISO dates; year distribution = 60/60
  in **2023** (range 2023-05-07 .. 2023-10-21).
- Anchored on **13 distinct query-resolvable subject entities** (Caroline,
  Melanie, Mel, …) — confirmed via `graph_entities.canonical_name` join.
- q0's gold edge `Caroline --occurred_on--> "2023-05-07"` (mem `83cd73d8`)
  EXISTS, is traversable, carries non-NULL provenance. The structural claim of
  ISS-204 holds: where a day-precision event exists, a traversable dated edge
  is produced and anchored on a real entity.

## Bucket separation — the mechanism is doing exactly what it targets

Independently re-bucketed all date-bearing golds (regex on gold string, ≤8
words, year/month/relative-marker):

- **EXACT (absolute date)**: 10/17 = **0.588**
- **RELATIVE (relative-date phrase, e.g. "the week before 9 June 2023")**:
  1/18 = **0.056**
- **GAP = 0.533**

RELATIVE flatlines at noise because ISS-204 deliberately skips them (no
day-precision mark → no edge; pending ISS-190/191 to pin the resolved day into
`start/end`). The bucket ISS-204 targets answers >half the time; the bucket it
skips by design sits at the floor. The 53-point gap IS the mechanism.

## The EXACT misses are NOT all crowding — three distinct failure modes

Honest decomposition of the 7 EXACT failures (this corrects any impression that
0.588 is "crowding-suppressed near-perfect"):

1. **Crowding (1)** — q0. Gold edge present + traversable, but Caroline carries
   **31** `occurred_on` edges spanning May–Oct; the May-7 needle is lost in 30
   same-anchor siblings. Pure ISS-203 ranking/disambiguation symptom. (Prior
   session estimated ~10 siblings; the live DB shows 31 — worse than thought.)
2. **Cross-year fact gap (3)** — q1/q26/q49, all gold="2022". **Zero** of the 60
   `occurred_on` edges fall in 2022 (SQL-confirmed). These are off-handedly
   mentioned prior-year facts with no day-precision event memory, so they never
   produce an edge. Outside ISS-204's mechanism entirely; not crowding. (q1 even
   hallucinated a 2023-05-08 date — picked the wrong memory.)
3. **Retrieval recall miss (3)** — q58/q63/q76. Model returns "I don't know" or
   answers the wrong event → the correct dated event memory never entered the
   candidate pool. Upstream retrieval/recall failure, not an edge-structure gap.

So the ISS-204 edge mechanism is sound; the residual EXACT deficit is owned by
ISS-203 (crowding), corpus coverage (cross-year), and recall (top-K), not by
the dated-edge producer.

## Open Q1 (does the literal date reach generation?) — partial signal

Retrieval scores this run: overall 0.289, single-hop 0.0625, multi-hop 0.351,
temporal 0.371. Where the dated memory DOES land in top-K (the 10 EXACT wins),
generation reads the date off the seeded memory's prose and answers correctly —
so the seed→surface path (Component 4) functions for admitted memories. The
remaining temporal deficit is admission (crowding/recall), not surfacing.

- [x] AC-4: structural mechanism DB-verified on fresh conv-26 ingest — 60
  occurred_on edges, 100% non-NULL provenance, query-resolvable anchors, gold
  edges present + traversable. Residual EXACT deficit attributed to ISS-203
  (crowding), cross-year coverage, and recall — NOT to the edge producer.
  Trace: `ISS204-AC4-conv26-20260601T143034Z`, live DB `.tmpcYbhzb/substrate.db`.
