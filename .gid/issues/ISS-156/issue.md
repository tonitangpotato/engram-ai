---
title: Per-category HyDE gating — multi-hop regression real (-10.81pp) on clean substrate
priority: P1
severity: degradation
status: open
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

- [ ] AC #1 — Design per-category gating mechanism in `GraphQuery` /
      `LocomoDriver`. Three policies:
      - `off` (default, matches today's behavior)
      - `all` (current `ENGRAM_BENCH_HYDE=1` behavior)
      - `per_category` (new: on for open-domain, off for multi-hop, neutral
        for single-hop / temporal)
- [ ] AC #2 — Plumbing: `ENGRAM_BENCH_HYDE_GATING={off,all,per_category}` env
      var OR a richer `GraphQuery.hyde_policy` enum field.
- [ ] AC #3 — Question-category routing in LocomoDriver: classify before
      retrieval, apply policy. (Either re-use the existing per-question
      `category` field from the LoCoMo fixture, or add a runtime classifier
      for general use.)
- [ ] AC #4 — Empirical: re-run conv-26 K=10 MMR 0.7 with `per_category`
      gating. Target: overall ≥ Arm A (no regression), multi-hop within 2pp
      of no-HyDE, open-domain ≥ +10pp vs no-HyDE.
- [ ] AC #5 — Document the per-category rationale + measured effects in
      `bench-design.md` HyDE section.

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
