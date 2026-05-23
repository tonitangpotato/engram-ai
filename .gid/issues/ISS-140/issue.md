---
id: ISS-140
title: 'Retrieval: add cross-encoder re-ranker stage (top-50 → top-K) to fix Mode-A ranking errors'
status: open
priority: P1
severity: degradation
labels:
- retrieval
- ranking
- reranker
- locomo
- cross-encoder
relates_to:
- ISS-069
- ISS-138
- ISS-139
filed: 2026-05-23
filed_by: rustclaw
depends_on: .gid/issues/ISS-138/issue.md
---

## Problem

Evidence often IS in the candidate pool but ranked too deep. ISS-069
2026-05-23 evidence table:

| K | single-hop recall | open-domain recall |
|---|-------------------|---------------------|
| 5 | 9.3% | 20.0% |
| 10 | 17.3% | 30.0% |
| 50 | 26.7% | 40.0% |

Raising K is wasteful: 90% of K=50 candidates aren't evidence, they
just slot the K=5 slots out of the way. **The evidence is reachable,
but the ranker can't tell it from distractors.**

### Why bi-encoder retrieval is structurally limited

Current stack is: bi-encoder embedding → cosine in vector index → RRF
fusion across plans (factual / temporal / causal / etc). All three
stages score candidates **independently** — query embedded once,
candidate embedded once, similarity computed via dot product. No
interaction between query terms and candidate terms.

Cross-encoder (CE) scores `(query, candidate)` jointly via a small
transformer pass. It can detect:

- "Caroline researches X" vs "Melanie does research" (semantic role
  alignment — current top failure mode in q3, q43, q70, etc.)
- "did Caroline go to support group" (gold = Caroline went) vs
  "Melanie asked about support group" (distractor — wrong actor)
- Tense/aspect mismatch (q3 "did" past, evidence "is researching"
  present-progressive)

These are exactly the patterns visible in 2026-05-23 conv-26 failure
traces.

## Proposed architecture

```
query → bi-encoder → vector retrieval → fusion (RRF) → top-50 candidates
                                                            ↓
                                                       cross-encoder
                                                       (q, c_i) pairs
                                                            ↓
                                                        top-K (5-10)
                                                            ↓
                                                       generator
```

Two implementation options:

### Option A: Lightweight CE (preferred for v1)

- Model: `cross-encoder/ms-marco-MiniLM-L-6-v2` (HuggingFace) or local
  `bge-reranker-base` (~110M params, runs CPU)
- ONNX-export → `onnxruntime` crate (already in tree? if not, +1 dep)
- Per-query: 50 CE passes × ~5ms each = 250ms latency overhead
- No LLM cost — local inference

### Option B: LLM-judge reranker

- Use Haiku to score 50 candidates against query
- +$0.002/query × 152 queries = $0.30/run (doubles LoCoMo cost)
- Higher quality but more expensive + slower

**Recommendation:** Start with Option A. If recall doesn't recover to
≥50% on single-hop, escalate to Option B for ablation.

## Acceptance criteria

1. Re-ranker plugged in behind `enable_reranker` config flag (default off)
2. Candidate pool widened to ~50 before re-rank step
3. With re-ranker on, single-hop recall@5 (final K) rises from 9.3% to
   ≥ 25% — same target as ISS-139 since both target Mode A
4. Multi-hop recall does NOT regress below 60% @ K=5 (currently 62.2%)
5. Per-query latency budget: ≤ +500ms p95 with re-ranker on
6. Three temp=0 LoCoMo runs: J-score ≥ 0.48 (vs 0.40 baseline)

## Risk

- **Latency** — if CE is slow at conv-scale (419 episodes × 152 queries
  × 50 candidates × CE time = 3.2M CE calls per run), batch carefully
- **Domain mismatch** — MS MARCO-trained re-rankers are tuned on web
  search queries; LoCoMo is conversational. May need fine-tuning on
  LoCoMo-style data if results disappoint
- **Combinatorics with ISS-139** — MMR + re-ranker order matters.
  Probably: pool=50 → CE rerank → MMR top-K. Test both orderings

## Order in roadmap

After ISS-138 (cheap K=10 baseline) and likely after ISS-139 (MMR is
cheaper, no model deps). This is the bigger lever but also the bigger
build cost. Plumbing onnxruntime through engramai is non-trivial.

## Alternative: skip re-ranker, do learned-to-rank from scratch

If LLM-judge ablation (Option B) shows >>Option A, that's a signal the
bi-encoder is fundamentally too weak and we should train a small LtR
model on LoCoMo-style data using Anthropic/OpenAI as supervision. Out
of scope for first cut.
