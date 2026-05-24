---
title: List-question multi-sub-query expansion for partial-list recall
priority: P2
severity: enhancement
status: open
tags:
  - retrieval
  - query-expansion
  - list-question
  - locomo
relates_to:
  - ISS-148
  - ISS-151
  - ISS-152
  - ISS-153
---

# ISS-154 — List-question multi-sub-query expansion

## TL;DR

Independent of ISS-153 (HyDE), the 9 partial-list failures in conv-26
single-hop (ISS-151 bucket B) have a different shape: gold is a list
(e.g. `pottery, camping, painting, swimming`), top-K contains only 1
item from that list, judge gives 0 score.

Naive fix: detect list-questions ("what activities", "what books",
"what items"), generate N sub-queries (one per likely category), run
each, union results before fusion. Costs 1 LLM call (classify +
expand) plus N retrieval calls. Roughly +1-2s per list-question.

## Background

Affects 9 of 25 conv-26 single-hop fails (36%). All:

- q15 `pottery, camping, painting, swimming` — recovered: pottery
- q24 `Running, pottery` — recovered: running
- q38 `Pottery, painting, camping, museum, swimming, hiking` — recovered: painting
- q51 `Horse, sunset, sunrise` — recovered: sunrise
- q52 `Oliver, Luna, Bailey` — recovered: oliver
- q60 `clarinet and violin` — recovered: clarinet
- q61 `Summer Sounds, Matt Patterson` — recovered: sounds
- q65 `Changes to her body, losing unsupportive friends` — recovered: friends
- q78 `Figurines, shoes` — recovered: figurines

Pattern: in every case the top-K is dominated by repeated mentions
of the FIRST item the conversation introduced. Embedding clustering
collapses around it.

## Plan

### Phase 1 — list-question detection

Heuristic classifier (no LLM): match query against patterns like
"what X has Y", "what X does Y", "what kinds of", "what activities",
"list", etc. Fast, deterministic, easy to tune.

Alternative: LLM-based intent classifier. More accurate but adds
latency to every query (not just lists).

Recommended: **heuristic first**, escalate to LLM only if heuristic
recall is too low.

### Phase 2 — sub-query generation

For detected list-questions, one Anthropic call:

```
System: You are a memory search assistant. The user asked a
list-style question. Generate 3-5 distinct hypothetical answer
phrases that each represent ONE possible item in the answer list.
Output as JSON array of strings.

User: What activities does Melanie partake in?
Assistant: ["Melanie enjoys pottery", "Melanie goes camping",
            "Melanie paints", "Melanie swims",
            "Melanie does crafts"]
```

### Phase 3 — multi-retrieve + union

Run retrieval for each sub-query (K_seed each), union the candidate
pools, dedupe by memory_id, then run fusion + MMR over the unioned
pool. Final top-K from the union.

### Phase 4 — measure

Run conv-26 K=10 λ=0.7 with multi-sub-query on. Re-run recall_diag.
Target: recover ≥ 5 of 9 partial-list cases (≥ 55%).

## Implementation sketch

Pure engram-bench addition. Zero engramai changes. New module:
`engram-bench/src/query_expansion/list_questions.rs`.

```rust
pub struct ExpandedQuery {
    pub original: String,
    pub sub_queries: Vec<String>,  // empty if not a list-question
}

pub fn detect_and_expand(query: &str) -> ExpandedQuery { /* ... */ }
```

LocomoDriver: when `ExpandedQuery::sub_queries.len() > 0`, run
retrieval N times, union, dedupe, then proceed normally.

## Acceptance criteria

- [ ] Heuristic list-question detector implemented + unit-tested.
- [ ] Sub-query generation via Anthropic helper.
- [ ] Multi-retrieve + union path in LocomoDriver, gated on opt-in
      env var `ENGRAM_BENCH_LIST_EXPAND=1`.
- [ ] One conv-26 K=10 λ=0.7 list-expand-on run completed.
- [ ] `recall_diag.py` re-run; report which of the 9 partial-list
      queries became full-list.

## Non-goals

- Does NOT touch HyDE (ISS-153).
- Does NOT change default behaviour (opt-in only).
- Heuristic detector is intentionally crude — exhaustive coverage
  is not a Phase 1 goal.

## Open questions

- **Sub-query count**: 3, 5, or "let LLM decide"? Start at 5,
  measure latency vs recall.
- **Cost**: 1 LLM call per list-question × ~30% of conv-26 queries
  ≈ +50 calls per full conv. Acceptable for bench, monitor latency.
- **Stacking with HyDE**: HyDE generates 1 hypothesis, list-expand
  generates N. Don't compose them naively — pick one path based on
  query type.
