---
id: ISS-142
title: Retrieval — list-aware multi-query expansion for set-valued questions
status: open
priority: P2
severity: degradation
labels: [retrieval, query-expansion, list-question, locomo]
relates_to: [ISS-138, ISS-139, ISS-141]
depends_on: []
filed: 2026-05-23
filed_by: rustclaw
---

## Problem

LoCoMo conv-26 single-hop failure analysis (after K=10 + MMR λ=0.7,
RUN ISS-139, overall 0.4474):

- single-hop:  5/32 = 15.6%
- multi-hop:  23/37 = 62.2%

Single-hop is **4× worse than multi-hop** in this conversation. Failure
mode breakdown (27 single-hop failures):

| Mode | Count | % |
|---|---|---|
| "I don't know" (retrieval miss) | 14 | 52% |
| List incomplete (gold has N items, pred has <N) | 10 | 37% |
| Wrong fact | 3 | 11% |

The **list-incomplete bucket (37%)** is the failure class this issue
attacks. Gold answer form, single-hop:

- Comma-separated list: **19/32 (59%)**
- Short single fact:    12/32 (38%)

In contrast, multi-hop has **0/37 list-form golds**. The "single-hop
is worse than multi-hop" anomaly in conv-26 is largely driven by
single-hop being heavily list-question while multi-hop is single-fact
or long-phrase, and LLM-as-judge scoring lists with all-or-nothing
semantics (partial credit ≈ 0).

### Concrete failure example

`conv-26-q15`:
- Gold: `pottery, camping, painting, swimming` (4 items)
- Pred: `Based on the memories, Melanie signed up for a pottery class…` (1 item)
- Verdict: **No** — "predicted answer is incomplete; only covers 1 of 4 activities"

Each of the 4 items is mentioned in a **different episode** in the
conversation (pottery class one week, camping trip another, etc.).
Top-10 retrieval with a single query "what activities does Melanie
enjoy" returns 10 candidates dominated by the most-embedding-similar
single item (pottery, mentioned multiple times across sessions),
leaving the other 3 items off the candidate pool entirely.

MMR helps a little — diversity reranking can pull in non-pottery
candidates — but MMR operates on the candidate pool that retrieval
already returned. If 3 of the 4 list items never enter the K=10
candidate pool, MMR cannot rescue them. RUN ISS-139 confirms this:
the 10 list-incomplete failures persisted under MMR @ λ=0.7.

## Proposed fix: list-aware multi-query expansion

When a question is **set-valued** (asks for a list / multiple items),
issue multiple semantically diverse sub-queries and union the results
before MMR/re-ranking.

### Step 1: detect set-valued questions

Classifier flags Q as list-valued if any of:

- LLM intent-classification pass: "does this question ask for a list,
  set, or multiple items?" (1 Haiku call, ~$0.0002, 1-token output)
- Surface heuristic fallback: question contains "what * does X do",
  "what * does X enjoy", "list", "all the", "name * that X", "what kinds",
  etc.

Cost: classifier runs only on the harness side, same layering decision
as ISS-141 HyDE — query-rewriting lives **above** retrieval, not inside
it. Retrieval stays LLM-free, deterministic, cacheable.

### Step 2: generate diverse sub-queries

For Q = "what activities does Melanie enjoy?", LLM generates N (default
3-5) hypothetical answers that cover **different** items:

- "Melanie enjoys pottery"
- "Melanie likes outdoor activities like camping and hiking"
- "Melanie does creative things like painting"

Each is embedded and retrieves top-(K/N) candidates. Union + dedupe
before passing to MMR.

This is **HyDE generalized to N hypotheticals** explicitly tuned for
list-question coverage. Differs from ISS-141 HyDE-multi-H by:

- ISS-141 multi-H targets disambiguation (3-5 plausible answer
  *shapes*, all about the same single fact)
- ISS-142 multi-query targets *set coverage* (3-5 hypotheticals each
  pointing at a *different* list item)

### Step 3: union + MMR

Combine candidate pools (dedupe by episode_id), then run existing MMR
reranker (ISS-139) on the unioned pool with the original query Q as
the relevance anchor. MMR will naturally prefer diverse items in
top-K-final, which is exactly what list-question scoring rewards.

## Acceptance criteria

1. List-question detector implemented in `engram-bench` driver
   (`src/query_expansion/list_detector.rs`). Default OFF behind
   `enable_list_aware` config flag.
2. Multi-query expansion (3 hypotheticals default, configurable)
   generated via Haiku, retrieved in parallel, unioned, deduped.
3. List-question conv-26 single-hop subset:
   - **List-incomplete failures drop from 10 → ≤4** (≥60% reduction)
   - Single-hop overall on conv-26: **≥0.35** (current 0.156 with
     MMR @ λ=0.7 = +19.4pp absolute target)
4. Non-list single-hop and multi-hop do **NOT** regress (set-valued
   detector classifies them as scalar, expansion skipped, results
   byte-identical to ISS-139 baseline)
5. Cost: +$0.002/query on list-flagged queries (1 detect call + 1 gen
   call for 3 hypotheticals). LoCoMo 152q with ~25% list-flagged =
   ~38 expansions × $0.002 = +$0.08/run.

## Layering decision

Same as ISS-141: query rewriting lives in the **harness** (`engram-bench`),
not in `engramai`. Retrieval substrate stays LLM-free.

If a future production caller wants list-aware retrieval, file a
follow-up to add this under the proposed `QueryExpander` trait from
ISS-141 ("Where HyDE actually lives" section).

## Order in roadmap

After ISS-141 HyDE lands. Rationale: HyDE is the general primitive
("rewrite Q in document-space"). List-aware is a *specialization* of
HyDE for set-valued queries. Building list-aware on top of HyDE
infrastructure (LLM client wiring, expansion-pipeline plumbing,
union+dedupe code) is cheaper than building it standalone.

## Alternatives considered

**Lower MMR λ aggressively (e.g., λ=0.3)** — would force more
diversity at the cost of top-1 relevance. λ-sweep ISS-139 showed
λ<0.5 starts flipping correct top-1 results. Not viable as a general
fix; the fundamental issue is missing candidates, not bad ordering.

**Larger K (e.g., K=50)** — increases the chance all list items end
up in the candidate pool, but ISS-138 evidence shows K=10→K=50 gave
+0.49pp overall (noise), with open-domain regression. The
embedding-similarity ranking is already saturated past K=10; the
missing list items aren't ranked 11-50 either, they're not in the
neighborhood at all.

**Per-item retrieval (extract list candidates from a structured
parse of conversation)** — much higher engineering cost (requires
structured ingestion that knows "this is a list of activities"); HyDE
multi-query is a strict superset of this idea at zero ingest cost.

## Risk

- **Detector false positives on scalar questions** — wastes $0.0006
  per FP (1 detect + 1 gen). Tolerable.
- **Detector false negatives on list questions** — falls back to
  current behavior, no regression. Tolerable.
- **Hypothetical answers hallucinate facts not in substrate** — same
  risk as ISS-141 HyDE. Original Q retrieval is unioned in as a
  safety net.

## Out of scope

- Auto-extracting structured "Melanie's activities = [...]" lists at
  ingest time. That's a separate substrate-side feature.
- Changing LLM-as-judge to grant partial credit on lists. Out of our
  control (judge protocol is fixed by LoCoMo benchmark spec).
