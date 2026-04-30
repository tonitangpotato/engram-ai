---
id: ISS-070
title: Multi-hop plan dispatcher has no graph traversal — falls through to single-shot Factual/Abstract
status: open
priority: P0
filed: 2026-04-29
filed_by: rustclaw
labels:
- retrieval
- multi-hop
- dispatcher
- graph-traversal
- locomo
- evaluation
relates_to:
- ISS-068
- ISS-069
source: RUN-0006
depends_on: .gid/issues/ISS-072/issue.md
---

# Multi-hop retrieval has no graph traversal in the executor

## Summary

LoCoMo cat=1 (multi-hop) hit@5 on conv-26 RUN-0006 substrate is
**0/3 (0%)** across D1/D2/D3 — every multi-hop query misses. Other
single-hop categories sit at 33–86% on the same substrate. The
asymmetry isn't a tuning problem; the executor has **no multi-hop
plan**. `PlanKind` in `crates/engramai/src/retrieval/dispatch.rs`
enumerates only `Factual / Episodic / Abstract_L5 / Affective /
Bitemporal / Associative / Hybrid`. Hybrid means "≥2 sub-plans
combined via RRF" — it is *not* graph traversal. Multi-hop intents
get classified into one of the seven and run as a single-shot
top-k retrieval against the candidate pool.

## Context: but we have a graph, right?

Yes — and that's exactly the gap. The substrate stores edges
(temporal, causal, associative) and RUN-0006 confirmed
ISS-068's fix admitted 19 previously-dropped turns into the
candidate pool. But:

- The **storage layer has a graph**.
- The **retrieval executor never walks it.**

Every plan implementation in `crates/engramai/src/retrieval/plans/`
issues a single hybrid_recall (or its variant), then sorts. None of
them does "seed → expand 1 hop → re-rank → expand again". For a
query like *"Who introduced Alex to the person Mira mentioned at
the Tuesday meeting?"* the correct answer requires
text→entity→edge→entity hops; the current executor returns the
turn most textually similar to the question and stops.

## Evidence

- **RUN-0006.md** (`.gid/eval-runs/RUN-0006.md`), per-category table:
  `cat=1 Multi-hop n=3 hits=0 (0.0%)` across all three depths.
- **dispatch.rs:61** — `PlanKind` enum lists 7 plans, no `MultiHop`.
- **plans/hybrid.rs:71** — `SubPlanKind` is the same 7 minus Hybrid;
  Hybrid composes single-shot plans, doesn't traverse.
- **plans/associative.rs** is the closest thing to graph-aware
  retrieval, but it's seed-and-rank over Hebbian links, not
  iterative path expansion.

## Hypothesis (root cause)

The v03 retrieval design (`.gid/features/v03-retrieval/design.md`)
specified plans by *cognitive function* (factual / affective /
bitemporal / etc.), not by *graph topology* (single-hop /
multi-hop / path). Multi-hop fell off because the LoCoMo category
taxonomy and the engram plan taxonomy are orthogonal. There is no
plan whose contract is "expand the seed set along graph edges
until the answer is reachable."

## Reproduction

```bash
cd /Users/potato/clawd/projects/engram
cargo run --release --example locomo_conv26_retrieval
# observe: cat=1 hits 0/3 in the per-category breakdown
```

## Fix sketch (not committing to one yet)

Three plausible directions, in increasing scope:

1. **Cheap:** Treat multi-hop as Hybrid(Factual + Associative) with
   higher k and edge-weighted re-ranking. Probably gets 1–2 of the
   3 cat=1 queries on conv-26. Doesn't generalize.
2. **Right:** Add `PlanKind::MultiHop` with an explicit
   beam-search executor: seed via hybrid_recall, expand along
   typed edges (causal / temporal / associative), score paths,
   return top-k *paths* (or path-leaf turns).
3. **Architectural:** Decouple the plan taxonomy from cognitive
   function — introduce a `Topology` axis (single-shot / path /
   subgraph) orthogonal to `Intent` (factual / affective / ...).
   Most plans become Topology=single-shot; multi-hop is the first
   Topology=path plan.

Direction 2 is the smallest fix that plausibly closes the LoCoMo
gap and lays groundwork for 3.

## Out of scope

- **Not** a ranking issue (that's ISS-069).
- **Not** a recall-pool issue (that was ISS-068, fixed).
- **Not** a classifier issue per se — even with perfect intent
  classification, the executor has nowhere to dispatch a multi-hop
  query *to*.

## Design decision (2026-04-29)

After reviewing the three sketches above and checking the LoCoMo
hit judgment logic in `examples/locomo_conv26_retrieval.rs:206–222`,
**direction 2 is committed** with these specifics:

**Why not direction 1.** Single-shot hybrid_recall + edge-weighted
re-rank can't surface the *target* turn for a query like "the person
Mira mentioned at Tuesday's meeting was introduced to Alex by whom?"
The target turn's text doesn't co-occur with all the anchors. Re-rank
can't promote what was never recalled. With only 3 cat=1 queries on
conv-26, a partial fix from direction 1 (1/3 → 2/3) is indistinguishable
from noise; we'd have to run full LoCoMo to evaluate, which is expensive.

**Why not direction 3 yet.** Topology⊥Intent dual-axis is the right
shape, but premature with one path-style plan. YAGNI: needs ≥2-3
non-single-shot plans to validate the abstraction. Refactoring all
7 existing plans before validating that path traversal even helps
on engram is paying upfront for a guess.

**What we're building.** Direction 2 with direction-3 seams preserved:

1. **`PlanKind::MultiHop` as an independent variant** — own dispatch
   path, own metrics row, own outcome labels. Cat=1 telemetry stays
   independently observable in RUN-NNNN.
2. **Beam search executor**, not BFS / DFS / random walk. BFS explodes,
   DFS over-commits in sparse graphs, random walk is high-variance on
   small substrates. Beam width + depth are observable hyperparameters.
3. **`allowed_edges: Vec<EdgeKind>` parameterized** at the executor
   level. Future path/subgraph plans can reuse the executor with
   different edge filters. (This is the direction-3 seam — kept narrow.)
4. **No PathScorer trait yet.** The LoCoMo hit judgment is set-membership
   over the flattened candidate pool: `hit = any(turn ∈ resp.results :
   turn.dia_id ∈ evidence_set)` (see `locomo_conv26_retrieval.rs:209–217`).
   This means the executor can flatten visited turns into a pool and
   rank by hybrid score — no path-level scoring needed for v1.
   PathScorer abstraction is deferred until a benchmark requires
   "return best path" rather than "return turns on good paths" (e.g.
   LongMemEval).

### Algorithm sketch

```
seed_turns = hybrid_recall(query, k=beam_width)
visited = seed_turns
frontier = seed_turns

for hop in 1..=max_depth {
    next = []
    for turn in frontier {
        for edge in graph.edges_from(turn).filter(|e| allowed_edges.contains(e.kind)) {
            if edge.target ∉ visited {
                next.push((edge.target, score(turn) * decay^hop * edge.weight))
            }
        }
    }
    frontier = top_k(next, beam_width)  // per-layer beam pruning
    visited.extend(frontier)
}

return top_k(visited, query.top_k)  // final ranking by hybrid score
```

### Inputs / config

```rust
struct MultiHopInputs {
    seed_query: Embedding,
    beam_width: usize,           // default 8
    max_depth: usize,            // default 3
    allowed_edges: Vec<EdgeKind>, // default: causal + temporal + associative
    decay: f32,                  // default 0.7 — penalty per hop
}
```

### Routing into MultiHop

The classifier needs a `multi_hop_intent` heuristic. First-pass detection:

- LoCoMo metadata (when running benchmarks) — direct `category == 1` route
- In production: query-text patterns — relational referents
  ("the one who...", "the person ... mentioned", "before/after that",
  "introduced ... to") + presence of multiple distinct entity anchors

Classifier additions are part of this issue's scope.

## Acceptance

- `PlanKind::MultiHop` exists in `dispatch.rs:61` PlanKind enum.
- `plans/multi_hop.rs` implements the beam search executor.
- Classifier routes multi-hop intents to MultiHop plan; fallthrough
  to existing plans is preserved for non-multi-hop queries.
- LoCoMo cat=1 hit@5 on conv-26 **D1 ≥ 67% (2/3)** is the real target;
  **D1 ≥ 33% (1/3)** is the minimum bar (below this = root cause not
  closed). D2/D3 ≥ 33% acceptable (graph density drops with depth
  partitioning).
- Per-category breakdown in RUN-NNNN reports `MultiHop` plan rows
  separately from Hybrid/Factual/etc.
- No regression on cat=2/3/4 hit rates from RUN-0006 baseline (those
  queries should still route to their existing plans).

## References

- `.gid/eval-runs/RUN-0006.md` — surfacing run.
- `.gid/eval-runs/RUN-0005.md` — substrate.
- `crates/engramai/src/retrieval/dispatch.rs` (PlanKind enum).
- `crates/engramai/src/retrieval/plans/hybrid.rs` (SubPlanKind).
- `.gid/features/v03-retrieval/design.md` (current plan taxonomy).
