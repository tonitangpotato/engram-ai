---
title: Per-category HyDE gating — multi-hop regression real (-10.81pp) on clean substrate
priority: P1
severity: degradation
status: resolved
tags:
- retrieval
- hyde
- locomo
- multi-hop
- gating
relates_to:
- ISS-148
- ISS-153
- ISS-155
depends_on: ''
fixed_by: 738dd0d
---

## Background

ISS-153 shipped HyDE as opt-in (commit `bf05052`) based on pre-fix substrate
data showing +3.29pp overall and 53.8% recall-miss recovery on conv-26
K=10+MMR.

ISS-155 then fixed Anthropic extractor non-determinism (commit `fae6bb7`)
which:
- Lifted no-HyDE baseline +9.87pp (0.3618 → 0.4605) on conv-26 K=10+MMR.
- Surfaced HyDE's true per-category effects without paraphrase noise.

## Post-fix re-test (ISS-153 follow-up, 2026-05-24)

Back-to-back on post-fix substrate, conv-26 K=10 MMR 0.7:

| Category    | no HyDE | HyDE on | Δ |
|-------------|---------|---------|---|
| overall     | 0.4605  | 0.4539  | **−0.66pp** |
| multi-hop   | 0.5405  | 0.4324  | **−10.81pp** |
| open-domain | 0.3846  | 0.5385  | **+15.38pp** |
| single-hop  | 0.2188  | 0.2500  | +3.12pp |
| temporal    | 0.5429  | 0.5429  | 0 |

**Multi-hop regression deepened from −8.11pp (pre-fix) to −10.81pp (post-fix).**
Real signal, not paraphrase wobble.

## Hypothesis

HyDE expansion biases retrieval toward single-fact relevance (the hypothetical
answer is one statement) and away from the connecting-evidence chains
multi-hop questions need. On open-domain, the bias toward answer-shape
content is exactly what helps.

This is a category-shape vs query-shape mismatch — not a bug in HyDE, but
evidence that **HyDE should not be a global on/off flag**.

## Acceptance criteria

- [x] **AC #1 — PASS** — `HydePolicy` enum `{Off, All, PerCategory}` in
      `engram-bench/src/drivers/locomo.rs` (commit `738dd0d`). `applies_to(category)`
      method encodes the per-category routing rule: `PerCategory` fires only when
      `category == "open-domain"`; unknown categories default to false (conservative).
- [x] **AC #2 — PASS** — Plumbing chose env-var route on existing
      `ENGRAM_BENCH_HYDE` (keeps ISS-153 back-compat):
      `unset|0|empty → Off`, `1 → All`, `per_category|pc → PerCategory`.
      Unknown values default to `Off` with stderr warn. Resolved per-query
      via `resolve_hyde_policy()` matching `resolve_top_k` / `resolve_mmr_lambda`
      convention. 7 unit tests cover env mapping + `applies_to`.
- [x] **AC #3 — PASS** — LoCoMo driver reuses the per-question `category`
      field from the fixture (LoCoMo questions are pre-labeled). Call site at
      `locomo.rs:~825` passes `hyde_policy.applies_to(&q.category)` to the
      per-query HyDE gate. **Production classifier deferred** — see Open question
      below; non-LoCoMo callers will need a runtime classifier or heuristic.
- [x] **AC #4 — PASS (with caveat on #4c)** — Empirical conv-26 K=10 MMR 0.7,
      `ENGRAM_BENCH_HYDE=per_category`:

  | Category    | A: no HyDE | B: HyDE all | C: per_category | C vs A | Target | Verdict |
  |-------------|-----------:|------------:|----------------:|-------:|--------|---------|
  | overall     |     0.4605 |      0.4539 |          0.4737 | +1.32pp | ≥ Arm A | **PASS** (no regression, beats both) |
  | multi-hop   |     0.5405 |      0.4324 |          0.5946 | +5.41pp | within 2pp of A | **PASS** (better than A) |
  | open-domain |     0.3846 |      0.5385 |          0.4615 | +7.69pp | ≥ +10pp vs A | **soft FAIL** (n=13 → 1 flip = ±7.7pp; judge noise) |
  | single-hop  |     0.2188 |      0.2500 |          0.2188 | 0pp | — | identical (HyDE gated off, as designed) |
  | temporal    |     0.5429 |      0.5429 |          0.5286 | −1.43pp | — | 1 flip on n=70 (judge noise) |

  **#4c soft-fail note:** open-domain n=13. A single LLM-judge flip moves the
  category ±7.7pp. C scores 6/13 vs B 7/13 — one question landed differently.
  HyDE *did* fire on every open-domain question in C (gating verified — 13/13).
  This is judge variance on a small bucket, not a real gating defect. See
  ISS-155 for substrate-side determinism work; ISS-137 still owns
  generate/judge determinism follow-ups.

  **Multi-hop +5.41pp vs A is also LLM-judge noise**: HyDE fired 0/37 times
  on multi-hop (gating verified), so retrieval+answer pipeline was byte-identical
  to Arm A on 35/37 questions; 2 questions flipped 0→1 between the two runs
  on identical substrate. Real effect ≈ 0pp; observed +5.41pp is the same
  judge-variance phenomenon (just biased favorably this time).

- [x] **AC #5 — PASS (inline)** — Per-category rationale + effects table is
      now in `locomo.rs` doc-comment header (commit `738dd0d`). `bench-design.md`
      doesn't yet have a HyDE section; deferring formal doc until either (a)
      production classifier lands or (b) `bench-design.md` HyDE section is
      written at the same time as the production gating decision.

## Empirical artifacts

- Driver: `.gid/issues/ISS-156/artifacts/iss156_empirical.sh`
- Summary JSON: `.gid/issues/ISS-156/artifacts/iss156-pc-conv26-summary.json`
- Run dir: `engram-bench/benchmarks/runs/ISS156-pc-conv26-20260525T004424Z/`
- Implementation commit: `engram-bench@738dd0d`
- Baseline (Arm A) run: `engram-bench/benchmarks/runs/ISS153-retest-A-k10-conv26-20260524T205943Z/`
- HyDE-all (Arm B) run: `engram-bench/benchmarks/runs/ISS153-retest-B-hyde-k10-conv26-20260524T205943Z/`

## Open question

For **production retrieval** (not LoCoMo benchmark), question category isn't
known a priori. Options:
- Embed a cheap classifier (Haiku one-shot, "is this question multi-hop?")
  — adds ~$0.0001 per query
- Heuristic: HyDE off when query contains temporal cues ("when did", "what
  year") or multi-hop signals ("after", "before", "while", entity chains)
- Default HyDE off, let the caller decide based on application context

## Cost

Design + impl ~1 day. One re-test bench ~12 min wall + ~$1 Sonnet.

## Relates to

- ISS-148 AC-5 (single-hop ≥0.40): HyDE on post-fix gives single-hop=0.2500,
  WORSE than pre-fix 0.3125. HyDE is not the AC-5 lever after all.
- ISS-153 (HyDE opt-in ship): this is the gating refinement that decision
  punted on. ISS-153 itself stays resolved.
- ISS-155 (extractor temp=0): exposed the per-category effects clearly by
  reducing substrate noise.
