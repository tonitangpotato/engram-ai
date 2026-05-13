---
title: Decide fate of unused answer_gen/ module in engram-bench
status: todo
priority: P2
labels: [tech-debt, engram-bench]
relates_to: [ISS-100]
---

# Decide fate of unused `answer_gen/` module in engram-bench

## Context

Discovered while implementing ISS-100 Step 1 (2026-05-04): the
`engram-bench/src/answer_gen/` module is **dead code** — 1214 lines
across 4 files, registered in `src/lib.rs:79` but with **zero call
sites** anywhere else in the crate (verified via `grep -rn answer_gen src/
--include="*.rs"` → only the `pub mod` line matches).

```
src/answer_gen/
├── extractor.rs       (674 lines) — Anthropic client + retry policy
├── locomo_prompt.txt           — committed prompt + SHA-256 lock
├── mod.rs            (169 lines) — public types: AnswerExtractor, RetrievedRecord, …
├── prompt.rs         (105 lines)
└── token.rs          (266 lines) — OAuth + API-key dual backend
```

## What `answer_gen/` does

Per its module docstring, it implements **short-answer extraction** for
LOCOMO **EM (exact-match) scoring**:

1. Take top-K retrieved records.
2. Truncate to a token budget (8000).
3. Send to Haiku 3 with a frozen prompt + `temperature=0`,
   `max_tokens=64`, `stop_sequences=["\n\n"]`.
4. Get back a short answer string.
5. Feed that into `scorers/locomo.rs` (normalised exact match).

This is the **EM path**: it solves "raw retrieved context → short
string the EM scorer can match against gold".

## Why it's dead

ISS-100 (the LOCOMO ship gate driver) chose the **LLM-as-judge path
instead** of the EM path:

- Ship gate's J-score (mem0 paper §5.2 parity) requires
  Sonnet 4.5 LLM-judging the predicted answer against gold ("Yes"/"No")
  → identical to `cogmembench/benchmarks/common/evaluator.py`.
- EM-on-extracted-short-answer is a **different metric** producing
  different numbers — it cannot satisfy GOAL-5.1 (mem0 J-score parity).

So the new ship-gate driver writes `llm_client.rs` + `scorers/
locomo_judge.rs` from scratch (Sonnet 4.5, 300/50 max_tokens) and
doesn't touch `answer_gen/`.

## Options

**Option A — Delete it.**
- Pro: removes 1214 lines of dead code, simplifies crate surface.
- Pro: prevents future contributors from assuming it's active.
- Con: loses the OAuth+API-key dual-backend code (`token.rs`) which is
  cleaner than what `llm_client.rs` ended up with — but that's solvable
  by porting the cleaner pattern into `llm_client.rs`.
- Con: throws away the `locomo_prompt.txt` + SHA-256-lock pattern
  (design §6.1 reproducibility). The pattern is good even if this
  specific use isn't.

**Option B — Repurpose for an EM-baseline scorer.**
- LOCOMO's published numbers include both EM and J-score. Keeping
  EM would let us report both, useful for cross-validation against
  papers that report only EM.
- Means writing a new driver path: `locomo_em` driver + EM scorer
  consumer of `answer_gen`.
- Cost: another LLM call per question (~$0 on OAuth, but +30min
  wall-clock per run). Probably not worth it unless we hit a paper we
  want to reproduce exactly.

**Option C — Refactor: extract the reusable bits, delete the rest.**
- Extract `token.rs` (OAuth + API-key dual backend) → shared
  `llm/auth.rs` used by both `llm_client.rs` and any future LLM call
  sites.
- Extract `prompt.rs` (committed prompt + SHA-256 lock pattern) →
  shared `llm/prompt_lock.rs`. Apply to the LOCOMO judge prompt for
  reproducibility (design §6.1).
- Delete `extractor.rs` and `mod.rs` (the EM-specific glue).

## Recommendation

**Option C**, but **not now** — only after ISS-100 ships and we have a
real, working LLM-judge path. Premature refactoring against a not-yet-
working reference is what produced this dead module in the first place.

## Acceptance

When this issue is picked up:

- [ ] Decide A / B / C (or another option) with potato
- [ ] If C: extract reusable modules and migrate `llm_client.rs`
- [ ] If A or C: delete dead files; update `lib.rs`
- [ ] Verify `cargo build -p engram-bench` succeeds and `cargo build
      -p engramai` still succeeds with engram-bench absent (GUARD-9)
- [ ] No new test suites needed unless Option B is chosen

## Provenance

- Discovered: 2026-05-04 during ISS-100 Step 1 implementation
- Verified dead: `grep -rn 'answer_gen\|AnswerExtractor' src/ --include='*.rs'`
  returned only the `pub mod` registration line
- Decision recorded: ship plan goes around `answer_gen/`, not through it
