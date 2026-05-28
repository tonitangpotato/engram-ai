---
id: ISS-100
title: Port LOCOMO LLM-as-judge into engram-bench (decouple from cogmembench)
status: resolved
priority: P0
severity: high
labels:
- v0.3
- ship-gate
- benchmarks
- leg-1
created: 2026-05-02
relates_to:
- GOAL-5.1
- GOAL-5.2
- ISS-046
- ISS-058
fixed_by: engram-bench:16cb376
resolved: 2026-05-23
---

# Port LOCOMO LLM-as-judge into engram-bench

## Why

engram v0.3 ship gate (GOAL-5.1: LOCOMO ≥ 68.5%) currently has no measurement
path of its own. The only way to get a J-score number is to run
`cogmembench/benchmarks/locomo/engram_adapter.py`, which:

1. Pollutes cogmembench's neutrality (cogmembench should be a multi-system
   benchmark, not engram's internal ship-gate harness).
2. Is currently broken (`reindex --ns` flag mismatch with current engram CLI).
3. Couples leg-1 (defense / beat baselines) to leg-2 (publish own benchmark).

Both legs need LOCOMO but in different roles. They must use different code
paths.

## What

Port the LLM-as-judge pipeline from `cogmembench/benchmarks/common/{evaluator,llm}.py`
(~174 lines Python) into the standalone `engram-bench` repo (split out 2026-05-02; was `crates/engram-bench/`) as ~200 lines Rust.

After this lands, `engram-bench` measures its own LOCOMO J-score with no
runtime dependency on cogmembench.

## Acceptance criteria

- [x] `../engram-bench/src/llm_client.rs` exists, smoke test passes
      ("Reply Yes" → response starts with "Yes")
- [x] `../engram-bench/src/scorers/locomo_judge.rs` exists with
      `judge_answer` + `generate_answer`, prompts byte-for-byte from cogmembench
- [x] `drivers/locomo.rs` modified: retrieve → generate_answer → judge_answer
      → record, with JSONL checkpointing
- [x] Smoke test (25 questions on conv-26) verdict-mismatch ≤ 1/25 vs
      cogmembench's `engram_adapter.py`
- [x] Full 199-question run produces `.gid/eval-runs/RUN-NNNN/` with
      results.json + summary.md, including J-score, evidence_recall,
      latency, token cost
- [x] No new dep added to `crates/engramai/Cargo.toml` (GUARD-9)

## Out of scope

See `docs/v0.3-ship-plan-2026-05-02.md` "What This Plan Does NOT Do":
- Two-DB consolidation
- cogmembench fixes
- LongMemEval judge port
- Changing 68.5% threshold

## Plan reference

`docs/v0.3-ship-plan-2026-05-02.md` (5-step plan, 1 day + 1 overnight run).
