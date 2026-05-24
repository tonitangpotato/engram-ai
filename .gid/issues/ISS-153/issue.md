---
title: HyDE / hypothetical document expansion for retrieval recall miss
priority: P1
severity: enhancement
status: in_review
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

- [x] `expand_query_via_hyde` helper shipped in engram-bench. (293115d, model fix 1a73024)
- [x] `ENGRAM_BENCH_HYDE=1` opt-in flag plumbed through LoCoMo driver.
- [x] One conv-26 K=10 λ=0.7 HyDE-on run completed. (ISS153-...150452Z)
- [x] `recall_diag.py` re-run; report which of the 14 recall-miss
      queries got their gold into the pool. (14/26 = 53.8% recovered, 0 introduced — see Phase 2 results)
- [x] Decision documented in ISS-148.

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

## Phase 2 results (2026-05-24 conv-26 K=10 λ=0.7)

**Run.** ISS153-hyde-on-conv26-l0.7-20260524T150452Z (PID 7304, 34min wall, model=claude-haiku-4-5-20251001).

**HyDE call health.** 152/152 succeeded (zero `expansion failed` warnings). Latency mean 1917ms / median 1513ms / p95 3434ms. Total HyDE tokens 17670 across 152 calls (mean 116/call, near the 120 cap).

**Headline accuracy (HyDE vs ISS-152 Run A baseline).**

| category    | HyDE   | base   | delta    |
|-------------|--------|--------|----------|
| overall     | 0.3947 | 0.3618 | +0.0329  |
| single-hop  | 0.3125 | 0.1562 | **+0.1562** |
| temporal    | 0.5143 | 0.4714 | +0.0429  |
| open-domain | 0.3846 | 0.3846 | 0.0000   |
| multi-hop   | 0.2432 | 0.3243 | -0.0811  |

Single-hop **doubled** (+15.6pp) — clears ISS-148 AC-5 floor (≥0.40)? **No — still 0.3125 < 0.40.** Closer, not there. Multi-hop regressed -8.1pp (HyDE's hypothesis can mislead when the question requires composing facts the model can't guess).

> Caveat: baseline is single-run, and ISS-155 documents ~5-10pp wobble per run from Ollama embed non-determinism. Single-hop +15.6pp is well outside that wobble band; multi-hop -8.1pp is at the edge — would need replication to be confident the regression is real, not noise.

**Phase 3 recall-diag.** Re-ran ISS-151 recall_diag on both dirs.

- Baseline single-hop recall-misses: **26** (script counts more strictly than the original 14 cited at ISS-151 filing time)
- HyDE single-hop recall-misses: **12**
- Recovered: **14 / 26 = 53.8%**
- Newly introduced: **0**

Per ISS-153 decision tree (≥50% recovered → ship): **ship HyDE as opt-in feature**.

### Phase 3 verdict

- ✅ AC-1 cleared: ≥50% of baseline recall-misses recovered, zero new misses introduced
- ⚠️ AC-5 (ISS-148): still below 0.40 single-hop. HyDE alone is necessary-not-sufficient.
- ❌ Multi-hop regression: HyDE in current form hurts multi-hop. Either (a) gate HyDE per category, (b) tune HYDE_SYSTEM_PROMPT for compositional questions, or (c) accept tradeoff if multi-hop wobble is within Ollama-noise envelope.

### Next actions

1. Flip ISS-153 status → in_review and file Phase 3 follow-ups: per-category gating, N=3 ensemble experiment, prompt tuning.
2. Move to ISS-155 Phase 1 diagnostic (Ollama embed determinism harness) — Ollama daemon now free.
3. Defer "make HyDE default-on" decision until (a) ISS-155 noise floor known and (b) multi-hop regression replicated or ruled noise.

**Cost.** ~17.7k Haiku output tokens per full 152q run. At Anthropic's published Haiku 4.5 rates this is sub-cent territory. No quota issue observed at this rate.

**Failed-run forensic.** First Haiku attempt (run #2, PID 6836) used `claude-3-haiku-20240307` — that legacy id 404s on OAuth Max Plan. Fixed to `claude-haiku-4-5-20251001` in engram-bench commit `1a73024`. Dir renamed `.FAILED-404-haiku-id` for reference.
