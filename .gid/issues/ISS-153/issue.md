---
title: HyDE / hypothetical document expansion for retrieval recall miss
priority: P2
severity: enhancement
status: open
tags:
  - retrieval
  - query-expansion
  - hyde
  - locomo
relates_to:
  - ISS-148
  - ISS-151
  - ISS-152
depends_on: ISS-152
---

# ISS-153 — HyDE / hypothetical document expansion

## TL;DR

If ISS-152 (pool widening + λ sweep) doesn't move conv-26 single-hop
toward ≥ 0.40, the bottleneck is **embedding semantics**, not pool
size: the model puts short reactions ("Wow!", "Cool!") above
specific-fact episodes for short queries (see ISS-151 q3 walk-through).

HyDE attacks this by **generating a hypothetical answer first**, then
embedding the hypothesis (not the query) to retrieve. For
"What did Caroline research?" the hypothesis might be "Caroline
researched adoption agencies because she wanted to start a family"
— this hypothesis's embedding is much closer to the actual evidence
episode than the bare query.

## Background

- Original paper: Gao et al. 2022, "Precise Zero-Shot Dense Retrieval
  without Relevance Labels"
- Established at production scale in RAG systems since 2023
- Cheap to implement (one extra Anthropic call per query) but adds
  latency: ~500-800ms generation + 1 embedding call ≈ 1s overhead
  per query
- LoCoMo single-hop currently completes in ~2-4s per query — HyDE
  brings it to ~3-5s. Acceptable for a benchmark, marginal for
  production.

## Plan

### Phase 1 — wire as an opt-in env-var

Add `ENGRAM_BENCH_HYDE=1` flag. When set, before each LoCoMo query:

1. Call `llm_client::generate` with system prompt:
   ```
   You are a memory search assistant. Given a user question, generate
   a single concise hypothetical sentence that, if true, would be a
   direct answer. Do not hedge. Do not say "I don't know". Just write
   the hypothetical answer as plain text.
   ```
2. Use the generated sentence as the embedding input (or
   concatenate query + hypothesis).
3. Feed into existing retrieval pipeline unchanged.

### Phase 2 — measure

Run conv-26 K=10 λ=0.7 with HyDE on, compare against ISS-152's best
result. Re-run `recall_diag.py` to see specifically which queries
went from `recall_miss` to `evidence_in_pool`.

### Phase 3 — decision

If HyDE recovers ≥ 50% of the 14 recall-miss queries → ship as
configurable feature (NOT default-on; latency hit needs explicit opt-in).

If HyDE recovers < 25% → embedding model itself is too weak for
LoCoMo conv-26's conversational density. Next move is a stronger
embedding model swap (not in scope here).

## Implementation sketch

- Pure addition in engram-bench: new optional preprocessing step
  before `Memory::retrieve` is called.
- Zero changes to engramai crate — HyDE is a query-side concern.
- Add to llm_client.rs as a helper: `expand_query_via_hyde(query: &str) -> String`.

## Acceptance criteria

- [ ] `expand_query_via_hyde` helper shipped in engram-bench.
- [ ] `ENGRAM_BENCH_HYDE=1` opt-in flag plumbed through LoCoMo driver.
- [ ] One conv-26 K=10 λ=0.7 HyDE-on run completed.
- [ ] `recall_diag.py` re-run; report which of the 14 recall-miss
      queries got their gold into the pool.
- [ ] Decision documented in ISS-148.

## Non-goals

- Does NOT change default behaviour (opt-in only).
- Does NOT touch list-question expansion (ISS-154).
- Does NOT change the embedding model.

## Open questions

- **Multi-fact list questions** ("What books has Melanie read?"):
  HyDE generates one hypothesis. Does that hurt or help vs the bare
  query? Might need ISS-154 (list multi-sub-query) layered on top.
- **Hallucinated-fact pollution**: if HyDE invents a fact not in the
  conversation, will retrieval surface other (wrong) evidence near
  the hallucination? Mitigation: keep the original query in the
  pool too (concatenate hypothesis + original) so retrieval doesn't
  miss obvious matches.
