---
id: ISS-202
title: memory→entity edges never written to unified `edges` table on ingest — memories are graph islands, multi-hop/entity recall structurally impossible
status: open
priority: P0
severity: architecture-defect
tags:
- unified-substrate
- graph
- retrieval
- entity
- ingest
- root-cause
- locomo
- single-hop
created: 2026-06-01
relates_to:
- ISS-164
- ISS-070
- ISS-179
- ISS-148
- ISS-197
- ISS-199
- .gid/issues/ISS-203/issue.md
---

# ISS-202: memory→entity link is stranded in a legacy side-table; the unified graph never sees it

> **One-line root cause:** the unified substrate's whole premise is "one
> graph". But on ingest, the memory→entity link (which memory mentions
> which entity) is written **only** to the legacy `memory_entities` table.
> It is **never** written into the unified `edges` table as a structural
> edge. Result: entity↔entity edges are rich, but every memory node is an
> **orphan island** — disconnected from the entity graph. Any retrieval
> path that tries to reach a fact via its entities (multi-hop, entity
> channel, graph traversal) **cannot**, because the memory→entity hop
> does not exist in the graph it would traverse.

## How this was found — the conv-26 q104 autopsy

- **Query:** "What book did Caroline recommend to Melanie?"
- **Gold:** `"Becoming Nicole"`. **Score:** 0.0 (model answered "I don't know").
- Routed to the **`factual`** plan.

### Live-DB evidence (`.tmpa0Kbrm/substrate.db`, 454 memory nodes, 2026-06-01)

1. **The fact IS stored correctly.** memory `ad15485c` =
   `"Caroline read and loved 'Becoming Nicole' by Amy Ellis Nutt, a true
   story about a trans girl and her family"`, with a full 3072-dim
   nomic-embed-text embedding. → **NOT an extraction problem.**

2. **The fact never reached the top-10 context.** Retrieval returned
   `"Melanie is reading a book that Caroline recommended previously"`
   (no title) at rank 1. The gold memory uses the words "read and loved",
   not "recommend", so by pure single-vector cosine it sits far from the
   query and is crowded out by ~17 other "book/read" memories.
   → The answer LLM genuinely never saw the title; its "I don't know" is
   correct given its context. **NOT a prompt/generation problem.**

3. **THE SMOKING GUN (corrected after deeper autopsy 2026-05-31).**
   The entity↔entity structural graph IS rich (789 edges), AND the gold
   memory's provenance IS captured — but **in the legacy table only**. The
   unified `edges` table threw the provenance away:

   | table | structural edges | with `memory_id`/`source_memory_id` set |
   |---|---|---|
   | legacy `graph_edges` | 789 | **789 (100% set)** |
   | unified `edges` | 789 | **0 (100% NULL)** |

   The 789 unified structural edges all have **`source_memory_id = NULL`**.

4. **The gold memory's graph is PERFECT in legacy.** memory `ad15485c`
   has 4 edges, all carrying `memory_id=ad15485c`:
   - `Caroline --uses--> Becoming Nicole`
   - `Becoming Nicole --is_a--> true story`
   - `Becoming Nicole --related_to--> trans girl`
   - `Amy Ellis Nutt --implements--> Becoming Nicole`

   And 5 mention rows (Caroline, Becoming Nicole, Amy Ellis Nutt, true
   story, trans girl). So the entity↔entity edges carry the gold memory's
   provenance — the bridge from `Caroline` to the gold memory EXISTS.

5. **Why retrieval still can't reach it.** The `factual` plan
   (`retrieval/plans/factual.rs:588`) anchors on entities (e.g. `Caroline`),
   calls `traverse_anchors` → `edges_of(Caroline)`, finds the
   `Caroline --uses--> Becoming Nicole` edge, and **seeds the candidate
   pool from `edge.memory_id`**. In legacy mode that's `ad15485c` → gold
   enters the pool. **Under unified reads, `edge.memory_id` maps from
   `source_memory_id`, which is NULL → zero seeds → gold never enters the
   pool → "I don't know".** The retrieval mechanism is correct; the data it
   reads is empty.

6. **Why `source_memory_id` is NULL — and why the reason is now stale.**
   `graph/store.rs::dual_write_edge_to_edges` (~L1337) **hardcodes
   `source_memory_id = NULL`** ("Phase-B NULL", R4) because at the time
   `edges.source_memory_id` FK-targets `nodes(id)` and memory rows weren't
   reliably in `nodes`. **That concern is now obsolete:** the live DB has
   **454 memory nodes** (ISS-197/199 migrated memory rows into `nodes`).
   So `source_memory_id` CAN be populated from the legacy
   `graph_edges.memory_id` value already in hand at write time.

## Code-level location (why it happens)

**The PRIMARY defect is a single dropped column on the edge dual-write path.**

1. **Entity↔entity structural edges DO get written on ingest** via the
   resolution persist path (`resolution/stage_persist.rs::build_delta` →
   `apply_graph_delta` → `dual_write_edge_to_edges`). They carry the source
   memory id in `graph_edges.memory_id`.

2. **But the unified mirror drops it.** `graph/store.rs` (~L1337,
   `dual_write_edge_to_edges` field mapping) explicitly sets
   `source_memory_id = NULL` ("Phase-B NULL", R4). The legacy
   `graph_edges.memory_id` value is right there in the same write but is
   not copied across.

3. **The read side reverse-maps `memory_id ← source_memory_id`**
   (`graph/store.rs` `get_edge_unified`, ~L878; same in the `edges_of`
   unified path). So under unified reads, every traversed edge reports
   `memory_id = None`.

4. **The `factual` plan seeds candidates from `edge.memory_id`**
   (`retrieval/plans/factual.rs:588`). NULL provenance → empty seed set →
   the gold memory never enters the candidate pool.

**This is NOT primarily about missing memory→entity edges.** The factual
plan does not traverse a memory→entity hop — it traverses entity↔entity
edges and reads each edge's *source memory* provenance. The fix is to
**populate `source_memory_id`**, not to invent a new edge kind.

## Why this is THE lever (not prompt, not generation, not bisect)

- conv-26 single-hop currently floors at **0.031** (ISS198-smoke). A large
  share of these are "the fact is stored, but single-vector cosine can't
  surface it; it needs an entity bridge" — exactly q104's shape.
- ISS-164 (entity channel) was falsified at +0 on single-fact. **This issue
  explains why:** the entity channel queried a memory→entity relationship
  that does not exist in the graph. It wasn't the wrong idea; it was built
  on a missing edge.
- ISS-070 (multi-hop dispatcher) would also be a no-op for q104 for the same
  reason: `store.traverse()` BFS over the graph cannot cross a
  memory→entity hop that isn't there.

**Multi-hop / entity recall cannot work until memory→entity edges exist in
the unified graph.** This is the prerequisite.

## Root fix (the comprehensive one)

**Primary (one-line, surgical):** in
`graph/store.rs::dual_write_edge_to_edges` (~L1337), copy
`graph_edges.memory_id` into `edges.source_memory_id` instead of hardcoding
NULL. The value is already in hand at write time; the FK concern that
motivated NULL is stale (454 memory rows now live in `nodes`). This single
change reconnects all 789 structural edges to their source memories on the
unified read path, which is what `factual.rs:588` seeds from.

**Backfill (for already-ingested data):** a migration that copies
`graph_edges.memory_id → edges.source_memory_id` keyed on edge `id`, so
existing DBs don't require re-ingestion to benefit. (Mirrors the
ISS-198/199 FK-repoint migration pattern.)

**Verify before assuming "done":** building the edge provenance is
necessary-but-not-sufficient. Once seeded, the gold memory still goes
through recall→rank→generation. Expected to *unlock* the class of
entity-bridge questions, not instantly hit 0.6. Must verify via same-DB
A/B (AC-4).

## Secondary defects found during the autopsy (file as sub-issues)

- **(a) Entity canonicalization broken.** Two failures, both SQL-confirmed
  on the live DB:
  - **Case-fold merge fails:** `"caroline"` (node `1d11ce4c`, from
    `DictionaryMatch`, `attributes.entity_type=person`) is a SEPARATE node
    from `"Caroline"` (node `ce689add`, `_legacy_kind=person`,
    `kind_source=DictionaryMatch`). Same person, two nodes — and crucially
    the gold edge `Caroline --uses--> Becoming Nicole` hangs off `ce689add`,
    while a query anchoring on the lowercased form would miss it.
  - **Possessive/prepositional phrases become standalone entities:** ~20
    `"Caroline's X"` nodes (`Caroline's advice`, `Caroline's artwork`,
    `Caroline's city`, `Caroline's group`, `Caroline's paintings`,
    `Caroline's journey as a trans woman`, …) plus prepositional forms
    (`conversation with Caroline`, `support from Caroline`). Same pattern
    for `Melanie's *`. The resolution pipeline
    (`resolution/pipeline.rs` + `entities.rs`) does not (i) case-fold for
    merge, nor (ii) strip possessive/prepositional wrappers to the head
    noun. This fragments one person into dozens of nodes, diluting the
    entity bridge even after the primary fix lands.
- **(b) Predicates are code-flavored.** Relations are `"uses"`, `"is_a"`,
  `"implements"` — the extraction prompt was built for *code* graphs, not
  conversational memory. `"Caroline --uses--> Becoming Nicole"` is nonsense
  semantically (should be `read` / `recommended`).
- **(c) Three-way data scatter.** memory→entity link lives in
  `memory_entities` (220) AND `graph_memory_entity_mentions` (1461) AND
  should-be `edges` (0). Needs consolidation under the unified substrate.

## Acceptance criteria

- **AC-1** `dual_write_edge_to_edges` populates `source_memory_id` from
  `graph_edges.memory_id`. Post-ingest on conv-26:
  `SELECT count(*) FROM edges WHERE edge_kind='structural' AND
  source_memory_id IS NOT NULL` ≈ 789 (parity with legacy
  `graph_edges.memory_id` non-NULL count), and the gold memory `ad15485c`
  is reachable: the `Caroline --uses--> Becoming Nicole` edge in `edges`
  reports `source_memory_id='ad15485c'`.
- **AC-2** Backfill migration copies `graph_edges.memory_id →
  edges.source_memory_id` (keyed on edge id) for already-ingested DBs;
  idempotent; existing tests green.
- **AC-3** Retrieval probe: under unified reads, anchoring the `factual`
  plan on `Caroline` seeds `ad15485c` into the candidate pool via
  `traverse_anchors → edges_of(Caroline) → edge.memory_id` (it currently
  seeds nothing). Verified by candidate-dump on q104.
- **AC-4** conv-26 same-DB A/B (source_memory_id-on vs current) shows
  single-hop lift with no multi-hop regression. Same-DB A/B only
  (run-to-run reingestion variance is large — see ISS-191 lesson).
- **AC-5** Secondary defects (a)/(b)/(c) filed as their own issues with
  evidence; this issue stays scoped to the `source_memory_id` provenance gap.

## Open questions — RESOLVED (2026-05-31)

**Q1 — dedicated `mentions` edge kind vs overload `subject_of`/`object_of`?**
→ **NEITHER is needed.** Deeper autopsy showed the `factual` plan never
traverses a memory→entity hop. It traverses entity↔entity structural edges
and seeds candidates from each edge's *source memory* provenance
(`edge.memory_id`, `factual.rs:588`). The bridge `Caroline --uses-->
Becoming Nicole` already carries `memory_id=ad15485c` in legacy. The fix is
to stop dropping that value when mirroring to unified `edges`. No new edge
kind, no `subject_of`/`object_of` writes. (The DELETE path at
`storage.rs:4927` already references both predicate families, so the schema
tolerates either — but neither is the live retrieval lever.)

**Q2 — `graph_memory_entity_mentions` (1461) vs `memory_entities` (220) as
source of truth?** → **They are two different tables with different
population paths; the cardinality gap is explained, not a bug:**
- `memory_entities` (220 rows, **3 distinct entity_ids** = caroline/melanie/go,
  lowercased canonical; 217 memories) — written by the legacy
  `storage.rs:11776` INSERT. A thin legacy shim, only the few canonicalized
  head entities.
- `graph_memory_entity_mentions` (1461 rows, **699 distinct entity_ids**, 453
  memories) — written by the resolution pipeline persist; comprehensive,
  every extracted mention including phrase-entities.

→ **Neither is the source of truth for the primary fix.** The primary fix
copies provenance from `graph_edges.memory_id` (already 100% populated), not
from either mention table. The mention tables are only relevant to secondary
defect (c) (consolidation) and to a future memory→entity-edge feature if one
is ever needed — at which point `graph_memory_entity_mentions` is the
comprehensive source.
