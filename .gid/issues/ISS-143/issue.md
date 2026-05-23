---
id: ISS-143
title: Benchmark — scale LoCoMo driver from conv-26-only to full 10-conversation run
status: open
priority: P1
severity: gap
labels: [benchmark, locomo, engram-bench, sota-alignment]
relates_to: [ISS-138, ISS-139, ISS-141, ISS-142]
depends_on: []
filed: 2026-05-23
filed_by: rustclaw
---

## Problem

`engram-bench` LoCoMo driver currently ingests + queries **only
conv-26** out of the 10-conversation LoCoMo benchmark. All reported
"LoCoMo accuracy" numbers (RUN ISS-137, ISS-138, ISS-139) are
single-conversation scores on 152 questions, not the 1500+ questions
across all 10 conversations the Mem0 / Zep papers report on.

This causes three concrete problems:

### 1. Apples-to-oranges SOTA comparison

Mem0 paper (Table 1, 2025-04) reports on full LoCoMo, all 10 convs,
LLM-as-judge J-score:

- Mem0g (graph): **68.4%** overall
- Mem0:         66.9%
- LangMem:      ~58%
- Full-context: ~73% (upper bound)

Our P0 ship gate `GOAL-5.1 ≥ 0.685` was calibrated against these
full-LoCoMo numbers. But we measure on conv-26 alone. Possible
outcomes:

- conv-26 is harder than average → we're actually closer to SOTA than
  reported (44.7% on conv-26 might be 55%+ on full set)
- conv-26 is easier than average → we're further from SOTA than reported
- conv-26 has unusual category distribution (it does — see point 2)
  → our per-category numbers don't reflect substrate's general behavior

Until we measure on the same denominator, comparisons to Mem0 / Zep
SOTA are unfounded.

### 2. Category-distribution artifacts skew failure analysis

conv-26 single-hop gold answer distribution (single-hop, 32 questions):

- list-form (comma-separated): **19/32 (59%)**
- short single fact:           12/32 (38%)
- long phrase:                  1/32 (3%)

vs conv-26 multi-hop (37 questions):

- list-form:        **0/37 (0%)**
- short single fact: 19/37 (51%)
- long phrase:      18/37 (49%)

This category-distribution mismatch is what causes the counterintuitive
"single-hop is 4× worse than multi-hop" result in conv-26 — single-hop
is dominated by list-questions that LLM-judge scores all-or-nothing.

Mem0 paper Table 1 (full LoCoMo, J-score):

- single-hop: 67.13%
- multi-hop:  51.15%

i.e. **single-hop > multi-hop**, opposite of what we see on conv-26.
This strongly suggests the conv-26-only setup gives a misleading
picture of where our substrate is weak. Without full-LoCoMo
measurement, we can't tell which category to prioritize for
ISS-141 / ISS-142 / ISS-139 follow-ups.

### 3. Optimization on a single-conversation overfits

Every retrieval tuning decision so far (K=10 vs K=5, MMR λ=0.7,
namespace-per-conv) has been validated on conv-26. A change that
helps conv-26 may not generalize. We're optimizing on the training
set with no held-out signal.

## Proposed work

### Phase A: enumerate + ingest all 10 conversations

Driver currently filters to conv-26. Lift the filter; ingest each
conversation into its own namespace (`locomo-conv-XX`), preserving
the existing per-conv-namespace isolation.

LoCoMo dataset has 10 conversations of varying length (~15-50
sessions each). Total ingest cost is ~10× current cost (~$5 of
embedding + minor extraction depending on flags).

Pseudocode:

```rust
for conv in dataset.conversations.iter() {
    let ns = format!("locomo-{}", conv.id);
    memory.create_namespace(&ns)?;
    for session in &conv.sessions {
        for turn in &session.turns {
            memory.add_raw(&ns, turn.text, turn.meta).await?;
        }
    }
}
```

### Phase B: query loop across all gold questions

For each (conv_id, question, gold) in dataset.gold_questions, run
retrieval scoped to namespace `locomo-{conv_id}`. Aggregate by
category across all conversations.

Output schema:

```json
{
  "overall": 0.xxxx,
  "by_category": {...},
  "by_conversation": {
    "conv-0":  {"n": 156, "overall": 0.xxxx, "by_category": {...}},
    "conv-1":  {"n": 143, ...},
    ...
    "conv-26": {"n": 152, ...}
  },
  "n_queries": 1540
}
```

This preserves backward-compat with conv-26-only runs (just inspect
`by_conversation["conv-26"]`).

### Phase C: re-validate every prior result on full LoCoMo

Re-run the canonical baselines on the full 10-conv setup:

1. K=5 legacy temp=0 (ISS-137 equivalent on full set)
2. K=10 baseline (ISS-138 equivalent on full set)
3. K=10 + MMR λ=0.7 (ISS-139 equivalent on full set)

Establishes the *real* baseline. All future ISS-141 (HyDE) /
ISS-142 (list-aware) work measured against this.

## Acceptance criteria

1. `engram-bench locomo` driver ingests + queries all 10 LoCoMo
   conversations by default. Flag `--only-conv <id>` preserved for
   single-conversation runs.
2. `locomo_summary.json` output gains a `by_conversation` block
   alongside existing `by_category` block.
3. `locomo_per_query.jsonl` rows gain `conversation_id` field
   (currently only `id` like `"conv-26-q0"` — split out conv
   explicitly for cleaner aggregation).
4. Three canonical baselines (K=5, K=10, K=10+MMR λ=0.7) re-run on
   full LoCoMo, archived under
   `benchmarks/runs/ISS143-baseline-full-locomo-*/`. Numbers committed
   to `engram-bench/README.md` SOTA comparison table.
5. ISS-137 / ISS-138 / ISS-139 issue files updated with a
   "full-LoCoMo follow-up" section noting the conv-26-only caveat
   and pointing at the ISS-143 full-set numbers.
6. **Decision gate:** if full-LoCoMo overall is within ±2pp of
   conv-26 overall (i.e. conv-26 was representative), continue using
   conv-26 for fast smoke iteration and full-LoCoMo for milestone
   gates. If full-LoCoMo differs by >2pp, switch smoke iteration to
   a smaller multi-conv sample (3 convs) to reduce overfitting risk.

## Cost estimate

- Ingest: ~10× current cost ≈ $5 (one-time per substrate version)
- Per full-LoCoMo eval: 152q × 10 ≈ 1540q × ($0.005 retrieve +
  $0.015 judge) ≈ $30/run
- Three baseline runs at Phase C: ~$90 one-time
- Ongoing: every milestone-gate run on full set is $30. Smoke
  iteration stays on conv-26 at $3.5/run.

## Out of scope

- Multi-conv cross-talk / shared-knowledge experiments (each conv
  stays in isolated namespace; LoCoMo benchmark assumes this).
- Re-implementing LoCoMo dataset loader in a different format.
- Adding Mem0-style memory consolidation between sessions (separate
  feature, file separately if needed).
