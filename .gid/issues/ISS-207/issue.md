---
id: ISS-207
title: Hybrid factual sub-plan emits candidates in memory_id order, not relevance order — bypasses factual_to_scored for all factual-via-hybrid queries
status: open
priority: P1
severity: retrieval-quality
tags:
- retrieval
- ranking
- factual-plan
- hybrid-plan
- rrf
- architecture
created: 2026-06-02
relates_to:
- ISS-205
- ISS-189
- ISS-192
---

# ISS-207: hybrid factual sub-plan ignores per-item relevance scoring

> **One-line:** When a query routes through the **hybrid** plan, its
> Factual sub-plan arm collapses `FactualMemoryRow` to bare
> `HybridItem::Memory(memory_id)` in **BTreeMap (id) order**, never calling
> `factual_to_scored`. The hybrid fuser (`fuse_rrf`) is purely rank-based
> (`1/(k+rank)`), so the candidates' RRF contribution is driven by an
> arbitrary id ordering instead of their graph/breadth/recency relevance.
> Every factual query that routes via hybrid silently loses the Factual
> plan's ranking signal.

## Background

`factual_to_scored` (the **pure-Factual** route at `orchestrator.rs:~1176`)
maps each `FactualMemoryRow` to a `ScoredResult` with a real graph_score
derived from breadth, the ISS-189 edge-provenance band, the ISS-192
edge-seed band, and the ISS-205 reserved privilege. Final ranking sorts on
that score.

The **hybrid** route (`orchestrator.rs:~1426`) instead asks each sub-plan
for a list of `HybridItem`s and fuses them with reciprocal-rank fusion
(`hybrid.rs:fuse_rrf`, `1/(k+rank)`). The Factual sub-plan arm
(`orchestrator.rs:~934`) emits:

```rust
r.memories.into_iter().map(|m| HybridItem::Memory(m.memory_id)).collect()
```

`FactualPlanResult.memories` is a `BTreeMap`, so this list is in
**memory_id order** — not relevance order. The hybrid fuser then assigns
RRF ranks by that arbitrary position. The graph_score computed (or
computable) for each row is discarded.

## Why this is latent / partially mitigated

ISS-205 (commit `a949d0d5`) added `partition_factual_reserved_first`, which
re-orders the emitted items so **reserved** rows lead, then **edge_seeded**,
then the rest. That rescues the temporal-reservation case (the dated
episode now gets a rank-0 RRF position). But within each tier the order is
still incoming (id) order, and the broader relevance signal
(breadth/recency/the full graph_score) is still ignored for hybrid-routed
factual queries.

## Root cause

`PlanCollaborators` (`orchestrator.rs:92`) has no `RecordLoader`, no bm25
index, and no `query_embedding` available at the `HybridDispatchExecutor`
(`orchestrator.rs:~850`) layer. `factual_to_scored` needs all three. So the
hybrid sub-plan arm cannot call `factual_to_scored` without threading those
collaborators down into the executor — an architecture change beyond
ISS-205's scope.

## Candidate fixes (decide during this issue)

1. **Order by graph_score before mapping to HybridItem.** Compute the
   row-local graph_score (breadth + ISS-189/192 bands + reserved) inside the
   hybrid arm — this needs only the row flags + breadth, not the loader —
   and sort emitted items by it descending. Cheap, no collaborator
   threading. Does **not** give vector/bm25 fusion within the sub-plan but
   restores the Factual *graph* ranking signal. Likely the right scoped fix.

2. **Thread loader + bm25 + query_embedding into `HybridDispatchExecutor`**
   and call `factual_to_scored`, then feed scored results into RRF by their
   score-derived rank. Fully correct but a larger refactor; verify it does
   not double-count vector/bm25 (hybrid already runs a vector/bm25 channel
   alongside the Factual sub-plan).

## Acceptance criteria

- **AC-1:** The hybrid Factual sub-plan arm emits items ordered by a
  relevance signal (at minimum the row-local graph_score: breadth +
  reserved + edge_seeded bands), not BTreeMap id order.
- **AC-2:** Pin the ordering with a unit test analogous to
  `partition_factual_reserved_first_tests` — a higher graph_score row
  precedes a lower one regardless of incoming id order.
- **AC-3:** A conv-26 / conv-44 A/B sweep shows no regression on
  multi-hop/open-domain and ≥0 net on single-fact/temporal vs the
  ISS-205 baseline (`a949d0d5`).
- **AC-4:** Decide and document whether option 1 or 2 is taken, and (if 1)
  whether the loader-threading refactor is deferred to a separate issue.

## Notes

- Out of ISS-205 scope by explicit decision (2026-06-02). ISS-205 only
  rescued the reserved-tier ordering; this issue covers the general
  relevance-ordering defect for all factual-via-hybrid queries.
- Bug site: `crates/engramai/src/retrieval/orchestrator.rs` —
  `dispatch_sub_plan` Factual arm (~line 934), now routed through
  `partition_factual_reserved_first` (~line 1727).
