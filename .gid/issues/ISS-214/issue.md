---
fixed_by: engram-bench:9066903
status: resolved
---
project: engram
---
title: Remove orphaned answer_gen render_prompt/extractor path (dead code from lever-1/2)
status: resolved
priority: 3
labels: dead-code, cleanup, generation, conv-26-q0
relates_to: ISS-212
fixed_by: engram-bench:9066903
---

# ISS-214: remove the orphaned answer_gen render path

## Why

ISS-212 discovered that the `answer_gen` render path
(`answer_gen::render_prompt` → `answer_gen::render_extractor_prompt`) is
**never invoked** during a LoCoMo bench run. The live answer-generation
path is `generate_answer → render_generate_prompt` (now
`render_answer_prompt`) in `scorers/locomo_judge.rs`.

The lever-1/lever-2 work (commits `84f869b`, `49471b6`) appended date
guidance into this orphaned path; it rendered into a string that was
thrown away (proven by raw byte-grep of the release binaries — guidance
text absent). Option A (`51639eb`) moved the guidance to the live path and
is kept; the orphaned render code is now pure dead weight.

It survives only because the lib tests exercise it directly, giving a false
green.

## Scope

- [x] Identify everything reachable only from `answer_gen::render_prompt` /
      `render_extractor_prompt` and not from any live driver path.
- [x] Keep `LOCOMO_DATE_GUIDANCE` — relocated into its sole live consumer
      `scorers/locomo_judge.rs` (Option B / position 1: no dedicated module
      for a single-consumer const).
- [x] Delete the orphaned render functions + their tests-that-test-nothing.
- [x] Confirm no live binary path loses behavior (live-path guidance tests
      pass under `scorers::locomo_judge`; q0 still 1.0 under unified).

## Acceptance

- [x] Orphaned `answer_gen` render functions removed. **Scope was wider
      than render_prompt alone**: the entire `answer_gen/` module (1349
      lines — mod/prompt/extractor/token + locomo_prompt.txt) had zero
      live callers and was deleted (Option B, agreed with potato). Only
      `LOCOMO_DATE_GUIDANCE` was live; relocated, not deleted.
- [x] `LOCOMO_DATE_GUIDANCE` still reaches the model on date-asking queries
      — `render_answer_prompt` consumes it locally; 2 guidance-content
      tests moved with it and pass.
- [x] Full engram-bench lib test suite green: **196 passed, 0 failed**
      (drop from 214 = deleted tests-that-tested-nothing).
- [x] No reduction in conv-26-q0: regression arm (binary mtime 07:33,
      post-deletion) → **conv-26-q0 score 1.0, verdict Yes** (predicted
      `2023-05-07`, gold `7 May 2023`); aggregate overall 0.3026 within
      ingest-noise band, no regression.

## Resolution (engram-bench `9066903`)

Option B executed: relocated `LOCOMO_DATE_GUIDANCE` + `locomo_date_guidance.txt`
+ its 2 content tests into `scorers/locomo_judge.rs` (sole consumer), then
deleted the whole `answer_gen/` module and dropped `pub mod answer_gen` from
`lib.rs`. `LocomoAnswerExtractor`/`token.rs` were superseded ISS-100
scaffolding with zero callers. Verified: build green, 196 lib tests pass,
q0 holds at 1.0 under unified reads.

## Karpathy note

This is a delete-only cleanup. Do NOT "while I'm here" refactor the live
`render_answer_prompt` or the mem0-parity `render_generate_prompt` — those
are load-bearing and parity-pinned (ISS-100). Scope is strictly: remove
the dead `answer_gen` render path.
