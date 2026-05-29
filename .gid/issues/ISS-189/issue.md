---
id: ISS-189
title: "Factual plan discards 1-hop edge provenance, falls back to recorded_at-truncated flat anchor recall — answer episode dropped before fusion"
status: open
priority: P0
severity: degradation
tags: [retrieval, factual-plan, graph-traversal, recall, locomo, root-cause]
created: 2026-05-29
relates_to: [ISS-105, ISS-149, ISS-175, ISS-147, ISS-152, ISS-188]
---

# ISS-189: Factual plan throws away its own graph edge traversal

> **Supersedes the v1 6-bug hypothesis** (archived at
> `artifacts/issue-v1-6bug-hypothesis-SUPERSEDED.md`). The v1 analysis was
> built on a **mislabeled dump file** (`conv-44-q29-prefusion-hybrid.jsonl`
> contained a different hybrid query's 10 rows, not q29's factual pool) and
> reached a string of contradictory conclusions. This v2 is grounded in
> **direct SQL inspection of the persisted graph** plus the actual source code
> path, not dump artifacts.

## Why this issue exists

10+ retrieval-side levers were falsified in one week (MMR, BM25 pool widening,
entity channel, factual reweight, cross-encoder, populate-embeddings, …). Every
one tuned **fusion / ranking**. None worked, because for the failing questions
the answer episode is dropped **before fusion ever runs** — inside the Factual
plan's candidate recall, at a SQL `LIMIT`.

## The canonical failure: conv-44-q29

- **Query:** "Which year did Audrey adopt the first three of her dogs?"
- **Gold:** `Pepper, Precious, Panda, and Pixie`
- **Predicted:** "I don't know. The memories mention that Audrey has dogs … but
  their names are not provided."
- **Category:** single-hop. **Score:** 0.0.
- **Plan:** `execute_plan ENTER plan_kind=factual` (NOT hybrid — the v1 claim
  was wrong), `outcome=Ok items=287`.

The answer lives in episode **`a8b823f4`**: *"Audrey has three pets named
Pepper, Precious, and Panda that she has owned for 3 years."*

## Every layer is correct — except recall truncation

Verified by SQL against the persisted graph
(`/var/folders/…/T/.tmpAZKa5X/{graph,substrate}.db`, conv-44, 705 memories,
839 entity nodes, 1915 provenance rows):

1. **Extraction — CORRECT.** Pepper / Precious / Panda / Pixie all exist as
   `graph_entities` rows.
2. **Coreference — CORRECT.** Three `graph_edges` rows exist:
   `Pepper --part_of--> Audrey`, `Precious --part_of--> Audrey`,
   `Panda --part_of--> Audrey`, each `confidence=0.85`.
3. **Edge provenance — CORRECT.** All three edges carry
   **`memory_id = a8b823f4`** — the graph itself records that the answer
   episode is the source of the Audrey↔dogs relation.
4. **Memory→entity provenance — CORRECT.** `a8b823f4` is tagged to Audrey,
   Pepper, Precious, Panda in `graph_memory_entity_mentions`.
5. **Resolver — CORRECT.** `graph_entity_aliases` has `audrey → Audrey`;
   `GraphEntityResolver::resolve("Audrey")` → the right `graph_entities.id`.
6. **`memories_mentioning_entity(Audrey)` — CORRECT.** Returns 524 memories,
   **including `a8b823f4`**.

So the data and the recall primitive are both fine. The defect is the
**truncation strategy** layered on top.

## Root cause (SQL + code, double-verified)

The Factual plan (`crates/engramai/src/retrieval/plans/factual.rs`) has two
stages that are **disconnected**:

### Stage 2 — 1-hop edge traversal (does the right thing, then drops it)

```rust
let edges = traverse_anchors(graph, &anchors, ...)?;   // edges carry memory_id
let mut linked_entities: BTreeSet<Uuid> = BTreeSet::new();
for row in &edges {
    if let Some(eid) = row.linked_entity {
        if !anchor_ids.contains(&eid) {
            linked_entities.insert(eid);               // collects Pepper/Precious/Panda
        }
    }
}
```

It reaches Pepper/Precious/Panda and **each edge carries
`memory_id = a8b823f4`** — the exact answer episode. But the loop extracts
only `linked_entity`; the **`memory_id` is silently discarded**.

### Stage 3 — memory candidate lookup (abandons Stage 2's work)

```rust
// comment says: Search set = anchors ∪ linked_entities
for anchor in &anchors {                               // ← only anchors, NOT linked_entities
    let hits = graph.memories_mentioning_entity(anchor.entity_id, limit)?;
    ...
}
```

- The comment promises `anchors ∪ linked_entities`, and the code **does**
  iterate both (a `for anchor in &anchors` loop at factual.rs:549 followed by
  a `for linked in &linked_entities` loop at factual.rs:568). *Correction:* an
  earlier draft of this issue (and the compaction summary) claimed
  `linked_entities` was never iterated — that was a stale read. The real
  defects are D1 (discarded edge provenance) and D3 (recency truncation), not
  a missing loop. See "Three independent defects" below for the corrected
  analysis.
- `limit = effective_limit = (OVERFETCH_RATIO=3 × requested_k=10) /
  anchors.len()` (ISS-105). With multiple resolved anchors this is **~6**.
- `memories_mentioning_entity` (graph/store.rs:3304) orders by
  **`recorded_at DESC, memory_id ASC` then `LIMIT ?`**.

### The kill

`a8b823f4` is an **early** conversation episode. Within Audrey's 524 mentions
ordered `recorded_at DESC`, it ranks **519 / 524**. With `LIMIT ≈ 6` (newest
only), it is cut. **The answer episode never enters the candidate pool**, so
fusion/ranking/generation never see it — which is exactly why every
fusion-side lever failed.

```sql
-- rank of the answer episode within Audrey's mentions, SQL's own ordering
WITH ranked AS (
  SELECT memory_id, ROW_NUMBER() OVER (ORDER BY recorded_at DESC, memory_id ASC) rk
  FROM graph_memory_entity_mentions
  WHERE entity_id = (SELECT id FROM graph_entities WHERE canonical_name='Audrey'))
SELECT rk FROM ranked WHERE memory_id='a8b823f4';  -- → 519 (of 524)
```

## Two real defects (D0 is the true root; D1 is necessary but insufficient; D3 amplifies)

- **D0 (TRUE ROOT) — outgoing-only edge traversal.** Stage 2
  `traverse_anchors` walked only `edges_of(anchor)` (SQL `WHERE subject_id =
  anchor`, store.rs:2584) — i.e. edges where the anchor is the *subject*. But
  for asymmetric predicates with no stored inverse (e.g. `PartOf` =
  `Directed{inverse:None}`, schema.rs:160), the episode that establishes the
  relationship is recorded on the edge pointing *at* the anchor. **Empirically
  verified** (SQL + `iss189_probe` against the leaked conv-44 graph.db): the
  answer episode `a8b823f4` ("Audrey has three pets Pepper/Precious/Panda owned
  for 3 years") sits on **3 INCOMING edges** — `Pepper --part_of--> Audrey`,
  `Precious --part_of--> Audrey`, `Panda --part_of--> Audrey` (Audrey is the
  *object*). Audrey's outgoing edges carry 131 distinct memory_ids, **none** is
  `a8b823f4` (`COUNT=0`); the incoming count is 3. So an outgoing-only walk can
  never reach the answer, and D1's edge seed has nothing to seed from.
- **D1 (necessary, insufficient) — discarded edge provenance.** Stage 2
  `edges[].memory_id` was thrown away and candidates re-derived from a flat
  recency scan. The D1 seed (admit `edges[].memory_id` to the candidate map) is
  correct and shipped — but on its own it only covers OUTGOING edges, so it does
  **not** fix conv-44-q0. D0 + D1 together do.
- **D3 (AMPLIFIER) — semantic truncation by `recorded_at`.** Independently of
  D0/D1, `memories_mentioning_entity` (graph/store.rs:3304) uses
  `ORDER BY recorded_at DESC LIMIT ?` with a tiny per-anchor limit. Recency is
  the wrong axis for factual queries. Once D0+D1 admit the answer via the
  incoming-edge seed, D3 is no longer load-bearing for q0. Re-axing toward
  confidence/relevance remains a worthwhile follow-up, out of scope here.

  *Withdrawn — D2 "unused linked_entities":* the original v2 issue listed a
  third defect (linked_entities collected but never iterated). Code inspection
  (factual.rs:568) shows the loop exists. D2 is not a defect.

## Why this is a graph-based fix, NOT a vector/BM25 fix

The answer is reachable by **1-hop edge navigation**: the query anchor (Audrey)
has `part_of` edges from Pepper/Precious/Panda, and those edges carry
`memory_id = a8b823f4`. A graph-native recall walks the edges and returns the
episodes the edges point at. No similarity, no BM25, no recency sort needed —
the episode enters the pool because it is **structurally connected to the
query anchor**, which is the entire premise of an entity-anchored plan.

Adding a vector/BM25 candidate-generation channel (an earlier proposal) would
have masked this by brute-forcing the episode back in via lexical/semantic
match — but that contradicts the graph-based design and leaves the real defect
(recall ignoring its own traversal) in place.

## Implemented fix (graph-based, shipped)

Option A (near-term, minimal, fixes q0 now). Two coordinated changes:

1. **Traverse INCOMING edges too (D0 root fix — SHIPPED).** Added
   `GraphRead::edges_into(object, predicate, include_invalidated)` to the trait
   and `SqliteGraphStore` (mirror of `edges_of` on the object side; SQL
   `WHERE object_entity_id = ?`, backed by the existing
   `idx_graph_edges_object_entity` index). `traverse_anchors` (factual.rs) now
   gathers both directions: outgoing via `edges_of` and incoming via
   `edges_into`. For an incoming edge the neighbor is the *subject*, so
   `FactualEdgeRow.linked_entity = edge.subject_id`. The `At(t)` as-of mode
   applies the bi-temporal filter to incoming rows post-hoc (no object-side
   as-of primitive needed).
2. **Seed candidates from Stage 2 edge provenance (D1 — SHIPPED).** Every edge
   in `edges` (now including incoming edges) contributes its `memory_id` to the
   candidate map, attributed to the edge's `anchor_id`. `Edge.memory_id`
   (edge.rs:150) is carried whole through `FactualEdgeRow.projected.edge`, so no
   struct plumbing was needed. The recency-scan loops still run afterward for
   neighborhood breadth, but can no longer silently discard the answer.
3. **D3 (recency truncation) — deferred.** D0+D1 make the answer episode immune
   to the recency cap because it enters via the incoming-edge seed, not the flat
   scan. Out of scope for this minimal fix.

**Empirical confirmation** (`iss189_probe` against the leaked conv-44 graph.db):
pre-fix the answer `a8b823f4` was NOT in the candidate pool (209 outgoing edges,
287 candidates); post-fix it IS (`238 edges, seeded=169, pool=297`,
`ANSWER episode a8b823f4 in candidate pool? YES`).

### Deep fix (Option B) — STILL TODO

The data-model root is that `PartOf` is `Directed{inverse:None}` (schema.rs:160;
TODO comment at 157-158 already flags adding inverse `ContainedBy`). With a
stored inverse, an outgoing walk from Audrey would naturally reach the dogs and
Option A's incoming traversal would become a redundant safety net rather than
the load-bearing path. Scope: add `ContainedBy` as `PartOf`'s inverse in the
schema, decide whether to backfill existing `graph_edges` or only
forward-resolve new edges. Tracked as a follow-up; not blocking q0.

## Acceptance criteria

- **AC-1.** [PASS] `iss189_ac1_edge_memory_id_seeds_candidate` — Stage 3
  candidate set includes the `memory_id` carried by a Stage 2 OUTGOING edge,
  attributed to its anchor.
- **AC-2.** [PASS] `iss189_ac2_answer_survives_recency_truncation` — an answer
  episode that ranks below the recency-truncation limit on a dense anchor still
  enters the pool via edge seeding.
- **AC-3.** [PASS] `iss189_ac3_seeding_is_additive` — edge seeding coexists with
  the recency scan; both seeded and scanned candidates appear.
- **AC-3b.** [PASS] `iss189_ac4_incoming_edge_memory_id_seeds_candidate` — the
  conv-44-q0 topology in miniature: answer episode on an INCOMING `part_of`
  edge (anchor on the object side, no outgoing edge carrying it) enters the
  pool, attributed to the anchor; the incoming edge's subject appears as a
  linked entity.
- **AC-3c.** [PASS] `iss189_ac5_incoming_edge_respects_predicate_filter` —
  incoming-edge traversal honors `predicate_filter`; non-matching incoming
  edges are excluded.
- **AC-4.** conv-44-q0 ("Which year did Audrey adopt the first three of her
  dogs?", gold 2020) scores 1.0 under the ISS-161 Arm A envelope
  (K=10, temp=0, HyDE off, MMR off, entity_channel off, pipeline_pool=1).
  [validation running — STAMP 20260529T131853Z]
- **AC-5.** No regression on conv-44 overall vs the CONV44-baseline
  (`20260529T060701Z`, overall 0.2276). [validation running]

## Evidence artifacts

- SQL probes: this session, against `/var/folders/…/T/.tmpAZKa5X/`.
- Baseline: `engram-bench/benchmarks/runs/CONV44-baseline-20260529T060701Z/`.
- Code: `factual.rs` Stage 2 (~L457) + Stage 3 (~L492); `memories_mentioning_entity`
  at `graph/store.rs:3304` (`ORDER BY recorded_at DESC ... LIMIT`).
- v1 superseded analysis: `artifacts/issue-v1-6bug-hypothesis-SUPERSEDED.md`.
