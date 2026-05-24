---
title: HyDE / hypothetical document expansion for retrieval recall miss
priority: P1
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
depends_on: ''
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

---

## Implementation log

### 2026-05-24 — Phase 1 shipped

Wired HyDE behind `ENGRAM_BENCH_HYDE=1` in `engram-bench@293115d`.

**engram-bench changes:**
- `src/llm_client.rs`: `expand_query_via_hyde(question) -> Result<HydeExpansion, LlmError>`, system prompt pinned as `HYDE_SYSTEM_PROMPT` const, `DEFAULT_MAX_TOKENS_HYDE = 120`.
- `src/drivers/locomo.rs`:
  - `resolve_hyde_enabled()` strict `matches!("1")` resolver.
  - `QueryTiming.hyde_ms` + `hyde_tokens` fields (added to `total_ms()`).
  - Per-query 3a.0 HyDE block before `graph_query_locked` — concatenates `"{hypothesis}\n\n{question}"` for retrieval, preserves bare `q.question` for `generate_answer` / `judge_answer`.
  - On HyDE failure: stderr warn + bare-question fallback (don't abort the run).
  - `PerQueryRow` extended with `latency_hyde_ms: Option<f64>` + `tokens_hyde: Option<u64>`, both with `skip_serializing_if = "Option::is_none"` — default-off JSONL is byte-identical to pre-HyDE envelope.

**Engramai changes:** zero. HyDE is purely a query-side concern, no
retrieval-pipeline changes.

**Tests:** engram-bench `183/183 lib` green; engramai `1946/1946 lib` green.

**Decisions deferred to first run:**
- Single hypothesis vs N=3 ensemble — Phase 1 ships single, Phase 2 may A/B vs ensemble if recall recovery < 50%.
- Reuse existing Anthropic Sonnet session vs spinning Haiku — Phase 1 reuses `call_llm` (Sonnet via `LLM_MODEL`), matches judge model. If cost becomes the gate, swap to Haiku in a Phase-2 follow-up.

**Next:** full conv-26 K=10 λ=0.7 HYDE=1 run + re-run `recall_diag.py` to count which of the 14 ISS-151 recall-miss queries got their gold into the pool. Decision tree per Phase 3.
