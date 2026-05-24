---
title: Ollama embedding non-determinism adds ~5-10pp wobble per LoCoMo run
priority: P2
severity: bug
status: open
tags:
- retrieval
- embedding
- ollama
- locomo
- variance
- determinism
relates_to:
- ISS-137
- ISS-148
- ISS-150
- ISS-152
depends_on: ''
---

# ISS-155 — Ollama embedding non-determinism

## TL;DR

Across runs with **identical code, fixture, and environment**, the
Ollama embedding endpoint produces non-deterministic vectors. Same
input text → different float output → different dedup-merge decisions
during ingest → different graph topology → different retrieval
candidates → ~5-10pp wobble in LoCoMo conv-26 judge score.

This is the next-biggest variance source after judge temperature
(addressed in ISS-137 by pinning judge `temperature=0`).

## Evidence

ISS-152 Run A vs ISS-150 baseline used:

- Same engram commit chain (engram `894dcb1` over `3253d49`, with
  `894dcb1` only adding `Option<...>` override fields **defaulted
  `None`** — code path is byte-identical when overrides are unset)
- Same fixture (conv-26 `39e7df4ea492…`)
- Same env (judge temp=0, K=10, MMR λ=0.7, default `K_seed`, default
  `bm25_pool`)
- Same machine, same Ollama daemon, same engram-bench commit `df3c8d1`

Yet:

| metric | ISS-150 (baseline) | ISS-152 Run A | Δ |
|---|---|---|---|
| overall | 0.4408 | 0.3618 | **−7.9pp** |
| single-hop | 0.2188 | 0.1562 | −6.3pp |
| multi-hop | 0.6216 | 0.3243 | **−29.7pp** |
| open-domain | 0.3077 | 0.3846 | +7.7pp |
| temporal | 0.5000 | 0.4714 | −2.9pp |

Per-query diff (152 questions, both runs):

- 110 same score
- 13 flipped up (Run A correct, ISS-150 wrong)
- **29 flipped down** (Run A wrong, ISS-150 correct)
  - Of which **13 are HARD** — Run A returned "I don't know" where
    ISS-150 had answered correctly (clear retrieval failure, not
    just judge disagreement)
  - 16 are soft (both runs answered, judge differed)

### Root cause signal — different dedup-merge events

Both runs had **exactly one** `Dedup: merging` log line, but on
**different memory IDs**:

- ISS-150: merged `10f710b1...` at cosine similarity `0.9535`
- ISS-152 Run A: merged `1241fe04...` at cosine similarity `0.9529`

The ingest pipeline reads episodes in the same order, calls Ollama
on the same text, and runs the same merge threshold (0.95). The
**only** way two different memory IDs cross the merge threshold on
different runs is that Ollama returned different vectors for at
least one episode pair, pushing similarities across the threshold
in different directions.

Different dedup decisions → different node count → different
embedding-index occupants → different top-K candidates → different
LoCoMo answers.

## Why this matters

1. **ISS-148 acceptance gate** asks for conv-26 single-hop ≥ 0.40.
   If the noise floor is ±5pp, a single-run pass/fail signal is
   meaningless — need N=3 averaging at minimum.
2. **A/B comparisons across commits are unsafe** unless we
   serialise ingest determinism (or always re-ingest both arms
   from the same Ollama session, which we mostly do but it's
   fragile).
3. **ISS-152 sweep results survive only because the trend was
   monotonic** (A 0.3618 → B 0.2895 → C 0.1842, single-hop
   0.16 → 0.13 → 0.03) — that's far outside the ±5-10pp noise
   band so the directional conclusion ("pool widening hurts") is
   robust. But smaller effects can't be measured at all today.

## Hypotheses

In order of cost to test:

1. **Ollama options API — `temperature=0` + `seed` at request time.**
   Cheap. The embedding model probably already runs effectively
   temperature-0 (no sampling), but the seed parameter may
   stabilise tie-breaking inside the kernel. First attempt.
2. **Ollama daemon-level GPU non-determinism.** Some GPU kernels
   (especially attention / softmax) are non-deterministic under
   default CUDA settings. Check if Ollama exposes a
   `OLLAMA_DETERMINISTIC` or equivalent env knob.
3. **Embedding model itself is non-deterministic.** Less likely
   for inference-only models but possible if it uses dropout
   or stochastic pooling. Would need a model swap.
4. **Float precision drift in batch processing.** If Ollama
   batches embeddings differently across runs, mat-mul order can
   change low-order bits → cross-threshold differences for pairs
   near 0.95 cosine.

## Plan

### Phase 1 — diagnose

- Write a tiny harness in `engram-bench` that calls Ollama embed
  on the same text 10× and dumps the resulting vectors.
- Compute pairwise L2 distance across the 10 vectors. If all zero
  → Ollama itself is deterministic and the problem is upstream
  (ingest order, or something else in engramai). If non-zero
  → confirmed Ollama-side.
- Check magnitude: is the variation in the noise floor
  (1e-6 level, harmless) or large (1e-3 level, threshold-crossing)?

### Phase 2 — fix attempt #1: Ollama options

- Add `{"options": {"temperature": 0, "seed": 42}}` to the
  embedding request body in engramai's Ollama client.
- Re-run the Phase 1 harness. If now bit-identical → ship.
- Re-run conv-26 K=10 baseline 3× to confirm < 1pp stdev across runs.

### Phase 3 — fix attempt #2: pin merge similarity high enough that
embedding noise can't flip merges

- If Phase 2 doesn't help (i.e. Ollama floats vary but well under
  0.001), the issue is that the merge threshold sits *exactly* in
  the noise band for some episode pairs.
- Either raise the merge threshold to 0.97 (fewer merges, but
  none cross-threshold) or add hysteresis (require ≥2 consecutive
  pairs above threshold).
- This is more invasive — held in reserve.

### Phase 4 — if nothing works

Swap embedding model. Candidates:

- `text-embedding-3-small` (OpenAI) — known deterministic, $0.02
  per 1M tokens
- `bge-small-en-v1.5` via candle (local, deterministic CPU
  inference)
- `nomic-embed-text` via Ollama with explicit determinism options

## Acceptance criteria

- [ ] Reproduce non-determinism with a minimal Ollama-only harness.
- [ ] Document the float-level magnitude (is it 1e-6 noise or
      1e-3 drift?).
- [ ] Attempt fix #1 (Ollama options API).
- [ ] Validate: 3× conv-26 K=10 runs with the fix, stdev ≤ 1pp on
      overall judge score.
- [ ] If fix #1 fails, escalate to fix #2 or #4.

## Non-goals

- Does not change retrieval algorithm (ISS-152 / ISS-153 own that).
- Does not retroactively re-baseline ISS-148 / ISS-150 / ISS-152.
  Once a determinism fix lands, the new floor becomes canonical.
- Does not address judge variance (already fixed by ISS-137).

## Predecessor

ISS-137 (judge temperature=0) cut the *judge-side* variance from
9.49pp stdev down to 0.66pp. That fix was orthogonal — it pinned
the LLM grader, not the retrieval input. This issue is the
*retrieval-input* side of the same variance problem.
