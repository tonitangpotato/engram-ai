---
id: ISS-070
title: "Multi-hop plan dispatcher has no graph traversal — falls through to single-shot Factual/Abstract"
status: open
priority: P0
filed: 2026-04-29
filed_by: rustclaw
labels: [retrieval, multi-hop, dispatcher, graph-traversal, locomo, evaluation]
relates_to: [ISS-068, ISS-069]
source: RUN-0006
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

## Acceptance

- A `MultiHop` (or path/subgraph) plan exists in `dispatch.rs`.
- LoCoMo cat=1 hit@5 on conv-26 ≥ 33% (1/3) at D1, ≥ 33% at D2/D3.
- Per-category breakdown in RUN-NNNN reports cat=1 separately.

## References

- `.gid/eval-runs/RUN-0006.md` — surfacing run.
- `.gid/eval-runs/RUN-0005.md` — substrate.
- `crates/engramai/src/retrieval/dispatch.rs` (PlanKind enum).
- `crates/engramai/src/retrieval/plans/hybrid.rs` (SubPlanKind).
- `.gid/features/v03-retrieval/design.md` (current plan taxonomy).
