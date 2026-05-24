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

## Phase 1 diagnostic results (2026-05-24)

Ran `examples/iss155_ollama_determinism.rs` N=10 per arm, same input
text (a synthesized LoCoMo-style episode about Caroline & adoption).

```
Arm A: defaults (no options block) — matches engramai
  max_pairwise_L2 = 0.000000e0
  max_coord_diff  = 0.000000e0
  min_cosine      = 1.000000000
  → all 10 vectors bit-identical

Arm B: options { temperature: 0, seed: 42 }
  max_pairwise_L2 = 0.000000e0
  max_coord_diff  = 0.000000e0
  min_cosine      = 1.000000000
  → all 10 vectors bit-identical

Cross-arm: A[0] vs B[0]
  L2 = 0.000000e0, cosine = 1.000000000
  → options block has no effect; A and B converge to the same vector
```

Model: `nomic-embed-text`, dim=768, host `http://localhost:11434`.

### Verdict: **Ollama is deterministic on identical input.**

This **falsifies ISS-155 Phase 2** (the `options{temperature=0,seed=42}`
hypothesis): even without the block, repeated calls return bit-identical
vectors. The options block doesn't change anything because the model is
already at its determinism floor.

Therefore the ~5-10pp inter-run wobble observed in ISS-137 (and at the
dedup-ID flip between ISS-150 and ISS-152 Run A) **cannot be Ollama
daemon-level non-determinism on identical inputs**.

### Revised suspect list

1. **Ingest ordering / dedup race condition** — different episode arrival
   order leads to different merge cascade, which produces different
   stored memory IDs and therefore different retrieval pools downstream.
   This is the strongest remaining suspect (matches the ISS-150 vs
   ISS-152-Run-A symptom: same content, different memory IDs at sim
   ~0.953 — right at the dedup threshold).
2. **Anthropic non-determinism in extraction/normalization** — the
   ingestion pipeline calls Haiku for fact extraction. Even with
   `temperature=0` upstream calls can still vary; if extracted triples
   differ across runs, downstream embeddings differ.
3. **LLM-judge variance** — ISS-137 already fixed Sonnet generator
   temp=0; need to verify judge step is also pinned.
4. **Floating-point reductions in dedup similarity** — if cosine is
   computed by a non-associative sum order (e.g., parallel reduction),
   threshold-near pairs can flip merge decisions.

### Updated plan

- ❌ Phase 2 (Ollama options block) — **falsified, drop**
- ✅ Phase 1 (diagnostic) — **done, falsified Phase 2 hypothesis**
- ⏭ Phase 3 (raise merge threshold) — still valid; sidesteps whichever
  upstream source causes the boundary flip
- ⏭ Phase 4 (swap embedding model) — still valid as a last resort
- 🆕 Phase 1b — diagnose **ingest order / Anthropic-extraction**
  reproducibility. Re-run identical conv-26 ingestion twice and diff
  stored memory IDs + embeddings table.

Phase 1b is the cheapest next probe and most likely to find the real
culprit. Will file separately if symptoms confirm.

## Phase 1b diagnostic results (2026-05-24) — ROOT CAUSE FOUND

Wrote `engram-bench/examples/iss155_phase1b_ingest_repro.rs` (commit
engram-bench `1f7a9f1`) — re-ingests conv-26 twice into two fresh
in-memory substrates, snapshots `(id, content_sha256, embedding_sha256)`
from each, diffs.

### Run summary

- **Arm A**: 968s wall, 457 memory rows, 457 embeddings, 457 distinct
  content hashes
- **Arm B**: 887s wall, 455 memory rows, 455 embeddings, 455 distinct
  content hashes
- **261 fully identical** (same content_sha256, same embedding_sha256)
- **196 a-only + 194 b-only content hashes** = **390 divergent rows
  (42.8% of all stored memories)**
- **0 embedding mismatches on shared content** — confirms Phase 1:
  Ollama is bit-deterministic when given identical input

### Root cause

The Anthropic extractor in `engramai/src/extractor.rs` (lines 381–392)
sends `/v1/messages` requests with **no `temperature` field**:

```rust
let body = serde_json::json!({
    "model": self.config.model,
    "max_tokens": self.config.max_tokens,
    "messages": [{"role": "user", "content": prompt}]
});
```

Anthropic's API default is `temperature = 1.0`. Spot-check of the
390 divergent rows shows the variance is at the paraphrase level
(not the semantic / fact level):

| Arm A row | Arm B row | Jaccard |
|-----------|-----------|---------|
| `Caroline found transgender stories inspiring and felt happy and thankful for support received` | `Caroline found transgender stories inspiring and felt happy and thankful for the support received` | 0.92 |
| `Melanie realizes that self-care is important and that looking after herself…` | `Melanie realizes self-care is important and recognizes that looking after herself…` | 0.94 |
| `Caroline is researching adoption agencies because she dreams of having a family…` | `Caroline is researching adoption agencies as part of pursuing her dream to have a family…` | 0.48 |

This is exactly what temp=1.0 produces: same semantic content, slightly
different surface form. The cascade:

1. Different surface form → different content hash → different `id`
2. Different surface form → embeddings differ (different bytes in)
3. Dedup similarity threshold (currently 0.85?) flips on a subset of
   pairs → different merge cascade
4. Different stored memory IDs → different retrieval pool → different
   top-K → different generated answers → different LLM-judge verdicts
5. End result: ~5-10pp inter-run accuracy wobble (ISS-137)

### Fix

**Add `"temperature": 0` to the extractor request body in
`engramai/src/extractor.rs` line ~382.** Two lines of code.

```rust
let body = serde_json::json!({
    "model": self.config.model,
    "max_tokens": self.config.max_tokens,
    "temperature": 0,                              // <-- ADD
    "messages": [{"role": "user", "content": prompt}]
});
```

(Also check `triple_extractor.rs:251` for the same pattern.)

### Caveats / what this does NOT fully fix

Even at temp=0, the same prompt + same input can still produce slightly
different output across providers due to:

- Token sampling determinism is not guaranteed on Anthropic's side
  even at temp=0 (their docs say "temperature=0 produces *near*
  deterministic output")
- Batch position / load conditions can change tokenizer paths

So we should not expect Phase 1b after the fix to produce 100%
identical content. Realistic expectation: **divergence drops from
~43% to <5%**. That's enough to take the dedup decision out of the
boundary-flip regime that's been driving the ISS-150 vs ISS-152-Run-A
disagreement.

If the fix doesn't get us to <5% divergence:

- Phase 3 (raise merge threshold from 0.85 → 0.92 or so) becomes the
  next lever — fewer pairs near the boundary, fewer flips.
- Phase 4 (swap embedding model) only kicks in if the *embedding*
  similarity distribution is the problem; Phase 1+1b show it isn't.

### Updated plan

- ✅ Phase 1 — Ollama deterministic on identical input (committed `dc063ea`)
- ✅ Phase 1b — extractor is the wobble source (this commit)
- 🔥 **Phase 2 (new)** — set `temperature=0` on extractor request body.
  Should be a 5-minute fix. Then re-run Phase 1b to verify divergence
  drops to <5%.
- ⏸ Phase 3 (raise merge threshold) — keep as backup if Phase 2
  doesn't fully close the gap
- ❌ Phase 4 (swap embedding model) — drop, not the bottleneck

### Cost note

This phase cost ~30 min wall + 2x extractor pass over 419 conv-26
episodes ≈ 838 Haiku calls. Budget already committed; future reruns
of Phase 1b for validation are cheap (~$0.50 / run).

## Phase 2 validation (2026-05-24, post-fix Phase 1b rerun)

**Fix landed:** engram `fae6bb7` — added `"temperature": 0` to
`AnthropicExtractor` request body in `engramai/src/extractor.rs:386`.
Unit tests (34/34) pass.

**Validation run:** same fixture (conv-26, 419 episodes), same dual
fresh-substrate ingest, with the patched binary.

```
Arm A: 456 rows, wall 925.6s
Arm B: 455 rows, wall 899.6s
Fully identical content hashes: 417
A-only: 39 (8.55% of A)
B-only: 38 (8.35% of B)
Avg divergence: 8.45%
Embedding mismatches on shared: 0
```

### Compare

| Metric | Pre-fix | Post-fix | Δ |
|---|---|---|---|
| Total rows (A / B) | 457 / 455 | 456 / 455 | ~stable |
| A-only / B-only | 196 / 194 | 39 / 38 | −80% |
| Avg divergence | 42.8% | **8.5%** | **−80.3%** |
| Embedding mismatch on shared | 0 | 0 | unchanged |

### Verdict

**Partial PASS — major improvement, target not fully met.**

- Divergence dropped from 42.8% → 8.5%. That's an **80% reduction**,
  enough to lift dedup decisions out of the worst boundary-flip regime
  driving ISS-150 vs ISS-152-Run-A disagreement.
- However, the original spec target was **<5%**. At 8.5% we're still
  above that floor.
- This matches the prior caveat: temperature=0 on Anthropic is
  "near-deterministic", not bit-exact.

### Next decision

Two paths:

1. **Ship the fix as-is, defer Phase 3.** Argument: 8.5% paraphrase
   divergence is probably well below the dedup similarity threshold
   for most pairs. The remaining wobble is likely *within* dedup
   tolerance and won't flip cluster membership. We'd need a downstream
   bench run (RUN-T31-equivalent) to confirm the LoCoMo inter-run
   stdev has actually fallen from 9.5pp → ≤2.5pp.

2. **Run Phase 3 too.** Raise `dedup` cosine similarity merge
   threshold from current ~0.85 → 0.92. Targets the boundary-flip
   mechanism directly: at 8.5% input divergence, raising the threshold
   means even paraphrase variants need to cosine-match very closely
   before merging, reducing cluster-membership churn.

**Recommendation:** ship `fae6bb7` now, then run a 3× LoCoMo at temp=0
to measure new inter-run stdev empirically before deciding on Phase 3.
That's cheap (3 × ~12min = 36min) and gives us hard numbers instead
of theoretical reasoning about boundary-flip behavior.

### Cost

Validation rerun: ~30 min wall, ~838 Haiku calls, ~$1.

### Artifacts

- pre-fix diff: `/tmp/iss155_phase1b.json` (42.8% divergence)
- post-fix diff: `/tmp/iss155_phase1b_validate.json` (8.5% divergence)
- harness: `engram-bench/examples/iss155_phase1b_ingest_repro.rs`
