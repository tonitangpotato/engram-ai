# Requirements: Engram v0.3 — Benchmarks & Ship Gates

> **Feature:** v03-benchmarks
> **Module prefix:** GOAL-5.X
> **Master doc:** `.gid/docs/requirements-v03.md` (GUARDs live there)
> **Design source:** `docs/DESIGN-v0.3.md` §11 (success criteria) + §1/G3 (cost measurement) + §7.3 (backward compatibility)

## Overview

The benchmarks feature owns the **ship/no-ship gates** for v0.3 — the quantitative criteria that determine whether a build is release-quality. It cuts across resolution (LLM cost), retrieval (recall quality), and migration (test preservation) because each ship gate measures an end-to-end property that no single pipeline feature can assert on its own. This feature does NOT own the pipeline stages themselves (v03-resolution, v03-retrieval, v03-migration own those); it owns the **measurement harness and the numeric targets**.

Every GOAL in this feature must map to a concrete benchmark suite or regression test with pass/fail thresholds — no prose-only success criteria.

## Priority Levels

- **P0**: Ship gate — v0.3.0 cannot release if this criterion is not met
- **P1**: Quality gate — should be met before release, but a documented failure can be accepted with justification
- **P2**: Observability — does not gate release, supports tuning and ongoing monitoring

## Goals

### Recall Quality Gates

- **GOAL-5.1** [P0]: The LOCOMO benchmark suite runs end-to-end against a v0.3 build and reports an overall score. The v0.3.0 release gate is: LOCOMO overall score ≥ 68.5% (parity with mem0's published baseline). A build that fails this gate cannot be released as v0.3.0. *(ref: DESIGN §11 success criteria, bullet 1)*

- **GOAL-5.2** [P0]: The LOCOMO temporal-category sub-score is reported separately from the overall score. The v0.3.0 release gate is: LOCOMO temporal ≥ Graphiti's published temporal number. Temporal regression against Graphiti blocks release. *(ref: DESIGN §11 success criteria, bullet 2)*

- **GOAL-5.3** [P0]: The LongMemEval benchmark suite runs against v0.3 and produces a comparable score to the v0.2 baseline measured on the same suite. The v0.3.0 release gate is: LongMemEval overall ≥ v0.2 baseline + 15 percentage points. The v0.2 baseline number is captured and committed to the repository before v0.3 implementation begins, so the delta is measurable. *(ref: DESIGN §11 success criteria, bullet 3)*

### Cost Gate

- **GOAL-5.4** [P0]: Average LLM calls per episode is measured over a benchmark run of N = 500 episodes (LOCOMO test set + rustclaw production trace) using the counters exposed by GOAL-2.11. The v0.3.0 release gate is: average ≤ 3 LLM calls per episode over this N=500 run. This is the ship-gate instantiation of GUARD-12's rolling-window observability. *(ref: DESIGN §11 success criteria, bullet 4 + §1/G3 + GUARD-12)*

### Test Preservation Gate

- **GOAL-5.5** [P0]: The v0.2 test suite (~280 tests) runs against a v0.3 build with the migration applied to its fixtures. The v0.3.0 release gate is: 100% of v0.2 tests pass post-migration — any test that fails must be either (a) explicitly documented as intentionally broken with a rationale, or (b) fixed before release. The "~280" count is resolved to an exact number from the v0.2.2 tag before v0.3 development begins. *(ref: DESIGN §11 success criteria, bullet 5 + §7.3 backward compatibility)*

### Cognitive-Feature Integration Gate

- **GOAL-5.6** [P1]: A regression-test suite demonstrates that interoceptive, metacognition, and affect features each measurably affect retrieval ranking in the expected direction (e.g., mood-congruent recall produces different top-K under different self-states — cross-ref GOAL-3.8; metacognition confidence affects result filtering). The suite runs on every CI build and fails if any of the three features no longer influence ranking. *(ref: DESIGN §11 success criteria, bullet 7)*

### Migration Data-Integrity Gate

- **GOAL-5.7** [P1]: The migration tool is exercised end-to-end against the rustclaw production engram database (`engram-memory.db`) as part of the release qualification. Required outcome: migration completes without data loss (no MemoryRecord, Hebbian link, or Knowledge Compiler topic lost — cross-ref GOAL-4.1), and post-migration queries that worked pre-migration still return equivalent results on a fixed query set of size ≥ 20. *(ref: DESIGN §11 success criteria, bullet 6)*

### Benchmark Reproducibility

- **GOAL-5.8** [P2]: Each benchmark run emits a reproducibility record — commit SHA, dataset versions, fusion weights, model identifiers, and raw per-query scores — committed to the repository alongside the summary number. Anyone can re-run a historical benchmark by checking out the commit SHA and replaying with the recorded config. *(ref: DESIGN §8.3 "Report before/after scores; freeze weights" + general reproducibility norm)*

## Guards

All cross-cutting GUARDs are defined in the master requirements document (`.gid/docs/requirements-v03.md`). Of particular relevance:

- **GUARD-2** [hard]: Never silent degrade — a failed benchmark must surface as a visible gate failure, never be silently skipped or downgraded.
- **GUARD-9** [hard]: No new required external dependency — the benchmark harness uses existing dependencies (the LOCOMO and LongMemEval datasets may be downloaded as test fixtures, but the crate's runtime dependency set is unchanged).
- **GUARD-12** [soft]: LLM call budget observability — GOAL-5.4 is the ship-gate instantiation of this measurement.

## Out of Scope

- **Individual pipeline correctness** — whether each resolution stage produces the right output is owned by v03-resolution
- **Retrieval correctness at the per-query level** — owned by v03-retrieval; this feature only measures aggregate recall numbers
- **Fusion weight tuning methodology** — tuning is a design-level concern (DESIGN §8.3); this feature measures the frozen-weight outcome
- **Continuous production observability** — ongoing cost/quality dashboards in deployed systems; this feature covers pre-release benchmarks, not production telemetry (which overlaps with GUARD-12)
- **Fuzz or stress testing beyond LOCOMO/LongMemEval** — not in v0.3.0 release gates
- **Multi-version benchmark chains** (v0.1 → v0.2 → v0.3 regression tracking) — only v0.2 vs v0.3 comparison is required

## Dependencies

- **v03-resolution** — provides the LLM call counters (GOAL-2.11) that GOAL-5.4 aggregates
- **v03-retrieval** — provides the query API that LOCOMO and LongMemEval drive
- **v03-migration** — provides the migration tool that GOAL-5.5 and GOAL-5.7 exercise
- **LOCOMO dataset** — external test fixture (downloaded as part of benchmark setup, not a runtime dep)
- **LongMemEval dataset** — external test fixture
- **rustclaw production engram-memory.db** — real-world migration target for GOAL-5.7
- **v0.2.2 baseline numbers** — must be captured and committed before v0.3 development to make deltas measurable (precondition for GOAL-5.3, GOAL-5.5)

## References

- Master requirements: `.gid/docs/requirements-v03.md`
- Design document: `docs/DESIGN-v0.3.md` §11 (success criteria), §1/G3 (cost measurement), §7.3 (backward compatibility), §8.3 (fusion weight tuning methodology)

---

**8 GOALs** (5 P0 / 2 P1 / 1 P2) — GUARDs in master doc
