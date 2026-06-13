---
id: ISS-141
title: 'Retrieval: HyDE / query expansion to break Mode-B recall ceiling for multi-hop and rare-keyword queries'
status: open
priority: P2
severity: degradation
labels:
- retrieval
- query-expansion
- hyde
- locomo
relates_to:
- ISS-069
- ISS-138
- ISS-140
- engram:ISS-142
- engram:ISS-143
- .gid/issues/ISS-225/issue.md
filed: 2026-05-23
filed_by: rustclaw
depends_on: .gid/issues/ISS-140/issue.md
---

## Problem

ISS-069 2026-05-23 recall@K table shows multi-hop **saturates at 70.3%
by K=10 and doesn't move at K=50**. Same for temporal: 53.5% ceiling
at K=30. Roughly 30% of multi-hop evidence and 47% of temporal evidence
is **not in top-50 at all** — the bi-encoder doesn't see these
candidates as similar to the query, period.

Raising K, MMR, re-ranking — all of those operate on the candidate pool
the retriever already returned. They don't help if the right turn never
enters the pool.

### Why bi-encoder embedding fails on certain queries

Cases where query and evidence share little surface vocabulary:

- **Multi-hop intermediate steps** — Question is at one abstraction
  level, evidence is at another. Question "When did Caroline come out?"
  may need evidence "Caroline: I told my mom about Sarah last night"
  where neither "come out" nor a date is explicit.
- **Negative phrasing** — Question "What does Melanie NOT eat?"
  embedding aligns poorly with evidence that simply omits a food.
- **Topic-shift questions** — Question "Did Caroline change her mind
  about adoption?" needs both pre-change and post-change turns; current
  retrieval only finds one.

## Proposed fix: HyDE (Hypothetical Document Embedding)

Standard HyDE flow:

1. Take user query Q
2. LLM generates a hypothetical short answer / document H that *would*
   answer Q (1-2 sentences)
3. Embed H instead of (or in addition to) Q
4. Retrieve from substrate

The key insight: H lives in **document-space** like the substrate
content, while Q lives in **question-space**. Embedding model treats
them very differently. Faiss similarity between Q and an answer-shaped
turn is unreliable; similarity between H (an answer-shaped synthetic)
and an answer-shaped turn is much higher.

### HyDE variants worth testing

- **Single H**: 1 LLM call/query, 1 embedding, 1 retrieve
- **Multi-H** (3-5 hypothetical answers, retrieve N each, union+dedupe):
  catches multiple possible answer shapes for ambiguous queries
- **HyDE + original Q** (fuse both retrievals): safer fallback when H
  hallucinates wildly

## Cost

- Per-query LLM cost: ~$0.001 with Haiku (1 short generation)
- Per-152q LoCoMo run: +$0.15
- Adds ~500ms p95 latency from the LLM call. Can be parallelized with
  the embedding lookup so isn't pure overhead

## Acceptance criteria

1. HyDE step plugged in behind `enable_hyde` config flag in
   `engram-bench` LoCoMo driver config (NOT in `engramai` — see
   "Layering decision" above). Default off. Reuses the engram-bench
   Haiku client from the answer-generation/judge path; no new LLM
   dependency in `engramai`.
2. With HyDE on:
   - Multi-hop recall@10 ≥ 80% (current ceiling 70.3%)
   - Temporal recall@10 ≥ 60% (current ceiling 53.5% @ K=30)
3. Single-hop and open-domain do NOT regress (HyDE can hurt simple
   queries where the literal question is already a great embedding)
4. Three temp=0 LoCoMo runs with HyDE: overall J-score ≥ 0.50
5. Cost analysis confirms +$0.15/run is acceptable for CI runs

## Risk

- **Hallucinated H** — LLM may invent details that pull retrieval
  toward irrelevant content. Mitigation: HyDE + original Q fusion, so
  worst case retrieval is just the original
- **LLM dependence** — Adds an LLM hop to the *retrieval* path, which
  currently is LLM-free. See "Layering decision" below.

## Layering decision (2026-05-23) — HyDE lives in the query layer, NOT in retrieval

Decided up-front because this boundary affects design across both
`engramai` (engine) and `engram-bench` (harness).

**Decision:** retrieval is `(query: String, top_k) -> Vec<Candidate>`,
deterministic, LLM-free, cacheable. HyDE is a pre-retrieval
**query-rewriting** step that lives *above* retrieval. The retrieval
adapter never knows whether the string it receives is the literal
user question or a hypothetical-document expansion.

### Why retrieval must stay LLM-free

- **Cacheability.** Same query at two timestamps → same candidates.
  Embedding lookup is deterministic given a frozen index. Once an LLM
  enters retrieval, the same query produces different results in
  different sessions (LLM stochasticity, model-version drift, prompt
  cache invalidation). Loses the benchmark substrate property — every
  LoCoMo run becomes a different experiment.
- **Latency predictability.** Embedding lookup is 5–50ms p99. LLM call
  is 200–2000ms p99. If retrieval can call LLM, p99 latency contract
  collapses by ~50×. Downstream code (graph fanout, MMR, re-ranker)
  budget breaks.
- **Test surface.** `engramai` unit tests currently mock zero LLM
  calls because retrieval is pure. Adding LLM into retrieval forces
  every retrieval-touching test to gain a mock-LLM fixture. Massive
  test-surface explosion for a feature that's actually just "rewrite
  the query first."
- **SOUL alignment.** Engram's working-through-substrate principle:
  substrate operations are deterministic mechanical primitives
  (insert, recall, walk). Cognition (which includes deciding *what*
  to recall) lives in the cognitive layer that calls into substrate.
  HyDE is cognition, not substrate.

### Where HyDE actually lives

Two layers, depending on caller:

**For `engram-bench` (the harness):** HyDE is a step in the LoCoMo
driver, between question parsing and `memory.graph_query_locked(…)`:

```rust
// in src/drivers/locomo.rs, before the retrieve call
let retrieval_query = if config.enable_hyde {
    hyde::expand(&q.question, &haiku_client).await?
} else {
    q.question.clone()
};
let candidates = memory.graph_query_locked(
    GraphQuery::new(retrieval_query).with_limit(top_k)
).await?;
```

`engramai` sees zero diff. `engram-bench` owns a `hyde` module
(probably under `src/query_expansion/`). LLM client is the one
engram-bench already has for the judge.

**For `engramai` later (if production callers want HyDE):** add a
`QueryExpander` trait *parallel to* retrieval, not inside it:

```rust
pub trait QueryExpander: Send + Sync {
    async fn expand(&self, query: &str) -> Result<Vec<String>>;
}

// Caller composes:
let expanded = expander.expand(&q).await?;
let candidates = retriever.retrieve(&expanded[0], top_k).await?;
// Optionally fuse retrievals from each expanded query.
```

Implementations live in a new `engramai-query` crate (or
`engramai::query` module gated behind a `query-expansion` feature
flag), never in `engramai::retrieval` or `engramai::graph::store`.

### Implication for ISS-141 acceptance criteria

- AC #1 originally said "`enable_hyde` config flag" — keep that, but
  the flag lives in `engram-bench`'s LoCoMo driver config, not in
  `engramai`. AC text amended below to clarify.
- HyDE in `engramai` core is **out of scope for ISS-141**. If a
  production caller (LoCoMo's been the only target so far) eventually
  needs HyDE, file a follow-up issue that adds the `QueryExpander`
  trait per the design above.

## Order in roadmap

Last of the 4 retrieval improvements (ISS-138, ISS-139, ISS-140 first):

- ISS-138 K=10 — cheap, no model deps, measures ceiling under current
  ranking
- ISS-139 MMR — fixes diversity within candidate pool
- ISS-140 re-ranker — fixes ordering within candidate pool
- **ISS-141 HyDE (this issue) — only one that grows the candidate pool**

Order matters: do ISS-138/139/140 first because if they get LoCoMo to
≥0.55 we may not need HyDE. ISS-141 is the highest-cost / highest-risk
intervention.

## Alternative considered

**Query expansion via WordNet / synonym graphs**: cheaper but much
weaker. ISS-069 failures are not vocabulary problems; they're semantic
structure problems. WordNet won't help "Caroline researches" → "Caroline
researching adoption" because both forms are already in the embedding's
training distribution.
