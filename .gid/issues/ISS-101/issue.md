---
title: LOCOMO driver missing answer-extraction step (predicted = full memory.content)
status: resolved
severity: blocker
priority: P0
feature: v03-benchmarks
repo: engram-bench
blocks: GOAL-5.1,GOAL-5.2
tags: v0.3,benchmarks,locomo,design-gap
fixed_by: engram-bench:16cb376
---

# LOCOMO driver missing answer-extraction step

## Summary

The LOCOMO driver in `engram-bench/src/drivers/locomo.rs` skips the
**answer-extraction** step that LOCOMO's official scoring protocol assumes.
Instead of generating a short answer from the retrieved context, the driver
plugs the full `record.content` of the top-1 retrieved memory into the EM
scorer. This produces ~0% on any non-trivial fixture even when retrieval
is correct — the scorer is being asked to exact-match a paragraph against
a phrase.

## Evidence

**Driver (engram-bench/src/drivers/locomo.rs ~L330-345):**

```rust
let predicted = match resp {
    Ok(response) => match response.results.first() {
        Some(ScoredResult::Memory { record, .. }) => record.content.clone(),
        Some(ScoredResult::Topic { .. }) => String::new(),
        None => String::new(),
    },
    ...
};
```

**Smoke test** (`drivers::locomo::tests::run_impl_emits_artifacts_and_gates`,
fixture: 1 episode `"alice met bob in 2020"`, 1 question gold=`"2020"`):

- Pipeline runs end-to-end without panic
- Artifacts (`locomo_summary.json`, `locomo_per_query.jsonl`, repro record) emitted
- Both GOAL-5.1 / GOAL-5.2 gates evaluated
- `overall = 0.0` because:
  - `predicted = "alice met bob in 2020"` (full memory content)
  - `gold = "2020"`
  - `EM(normalize(predicted), normalize(gold)) = 0`

Retrieval is correct (top-1 is the right memory). The 0.0 is **a driver
bug, not a recall failure**.

## Root Cause — Design-Level Gap

`v03-benchmarks/design.md` §3.1 step 3 (line 92):

> "call `Memory::graph_query(...)`, capture the typed `RetrievalOutcome`,
>  and extract the answer string per LOCOMO's scoring conventions."

That last clause is hand-waved. Upstream LOCOMO's scoring conventions
require a **GPT-4 answer-generation step**: feed the retrieved top-K
context into an LLM, generate a short answer, then EM-compare against
gold. This is how Mem0 (41.8%), Graphiti-temporal (~50%), and other
public LOCOMO baselines are produced.

The current driver implements steps 1 (ingest), 2 (retrieve), and 4
(score) but skips step 3 (generate). It passes raw retrieval output
directly to step 4.

This is **not a single missed implementation detail** — design.md never
specified what answer extraction should look like. So this issue covers
both the design-doc fix AND the driver fix.

## Layer Status (cite-before-claim, all verified 2026-05-02)

| Layer | File | Status |
|---|---|---|
| Retrieval (engram core) | `engramai::retrieval::api::graph_query_locked` | ✅ returns non-empty top-K |
| Driver replay loop | `engram-bench/src/drivers/locomo.rs::replay_conversation` (~L271-380, 1014 LOC total) | ⚠️ ingest+query+latency correct; answer-extraction missing |
| Answer generation | (no module) | ❌ missing |
| Scorer (EM) | `engram-bench/src/scorers/locomo.rs` (471 LOC) | ✅ faithful Rust port of upstream Python `normalize_answer` + `exact_match_score`, pinned to 50-question parity fixture |

## Fix Options

### Option A — LLM-as-judge answer generation (~250-300 LOC)

New module `engram-bench/src/answer_gen/` with:

- `AnswerGenerator` trait
- One impl wrapping Anthropic / OpenAI client (model choice surfaced in repro record)
- Prompt: top-K retrieved context + question → short answer
- Driver call site (`replay_conversation`) wires generator between
  `graph_query_locked` and EM scorer

**Pros:**
- Numbers directly comparable to Mem0 / Graphiti / cogmembench J-score
- Matches upstream LOCOMO protocol
- Future-proof for LongMemEval (also LLM-judged)

**Cons:**
- Adds LLM API dependency to bench (cost, network, determinism)
- Need repro-record entry for model + prompt + temperature

### Option B — Substring containment (~30 LOC)

In driver, change predicted-extraction to:

```rust
let predicted = if record.content.contains(&gold) { gold.clone() } else { record.content.clone() };
```

Or make the scorer support a `containment_match` mode in addition to EM.

**Pros:**
- Tiny change; produces a non-zero recall signal immediately
- No LLM dependency
- Useful as a sanity-check baseline

**Cons:**
- ❌ **Not comparable to public LOCOMO baselines** — different metric semantics
- Cannot be the headline number in v0.3 ship-gate

### Option C — Top-K context + LLM answer (~250 LOC)

Same as A but driver passes top-K (not top-1) context to the generator.
This is the standard mem0 / Graphiti pattern.

**Pros:** Matches the most-cited public methodology
**Cons:** Same as A, plus prompt-engineering cost for context window

## Decision

To be made (A vs B vs C). Recommendation pending discussion:

- **B alone** → fast signal but disqualified from headline numbers
- **A or C** → real ship-gate-grade measurement
- **B then A/C** → fast iteration, but the B numbers must not appear in
  release notes / paper

## Related

- Blocks: GOAL-5.1, GOAL-5.2 (v03-benchmarks ship gate)
- Repo: code lives in `/Users/potato/clawd/projects/engram-bench/` (split
  out of engram by commit `d54a3e1`)
- Design fix needed: `engram/.gid/features/v03-benchmarks/design.md` §3.1 step 3
- Adjacent prior work: cogmembench J-score adapter (Tier 3 in
  `.gid/docs/locomo-protocol.md`, "not yet wired")

## Acceptance Criteria

- [x] Decision recorded (A / B / C / B-then-A) with rationale
- [x] `design.md` §3.1 step 3 rewritten to specify the chosen
      answer-extraction mechanism explicitly (no more "per LOCOMO's
      scoring conventions") — see §3.1 step 3b + §3.1.1 (prompt pinned)
- [x] Driver implementation lands in `engram-bench` with answer-extraction
      step wired between retrieve and score — `score_judged` + `generate_answer` +
      `judge_answer` in `drivers/locomo.rs`, smoke test at L2456 passes
- [x] Smoke test produces non-trivial score on the existing fixture
      (`alice met bob in 2020` / gold=`2020`) — exact threshold depends
      on chosen option
- [x] Repro record captures answer-extraction config (model, prompt hash,
      etc. for A/C; mode flag for B) — `ReproRecord::answer_extraction.prompt_sha256`
      in `harness/repro.rs`, surfaced from `answer_gen/extractor.rs`
- [x] If A/C: cost & latency budget for one full LOCOMO run documented —
      per-query `generate_ms`/`judge_ms`/`generate_tokens`/`judge_tokens` in `locomo_per_query.jsonl`

## Resolution

Resolved via ISS-100 port (commit `16cb376` in engram-bench, 12 files +3035 LoC).
Full generate→judge pipeline replaced the original `predicted = record.content`
shortcut. The 0.0 fixture score is no longer the driver bug it once was; ISS-101
status was simply not flipped at the time. Verified 2026-05-28 via cite-before-claim
spot-check of driver code, design §3.1 step 3b, and `harness::repro::ReproRecord`.
