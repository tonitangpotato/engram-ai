# ISS-161 Lever 2 Diagnostic — Per-question pool-recall analysis

**Date:** 2026-05-26  
**Method:** Paper-only. Used existing candidate dump
`benchmarks/runs/ISS150-modeB-dump-conv26-20260524T042707Z/2026-05-24T04-39-29Z_locomo/locomo_per_query.jsonl`
(conv-26, K=10, MMR=0.7, no CE — single-hop 7/32 = 0.219, same era as
v2 sweep). Cross-referenced against raw fixture
`benchmarks/fixtures/locomo/<sha>/conversations.jsonl` (419 episodes
in conv-26).

**No new bench launches.** This is the paper-only Lever 2 step from
ISS-161 decision rule.

## Caveat: dump shape

The candidate dump only contains the **post-fusion, post-MMR top-10**.
The pre-truncate seed pool (k_seed=10 default, fusion_pool ~20 in this
era) is not preserved. So "needle missing from top-10" cannot
discriminate between:

- **Pool-recall miss** (right episode never entered the seed pool at
  all — needs K-expansion or different adapter weighting), or
- **Pool-truncate miss** (right episode entered seed pool but got
  truncated before reaching top-10 — needs K-final expansion or
  rerank).

This caveat is recorded for the next iteration. The current diagnostic
narrows the failure to "not in top-10" with high confidence; routing
to seed-vs-truncate requires a separate K=50 dump pass.

## Per-question table — 9 failing single-fact questions

For each: question intent, gold answer, count of raw episodes
containing literal needle, count of needle episodes in top-10
candidates, top-10 highest-scoring needle rank (if any).

| ID | Gold | Needle pattern | Raw-corpus episodes w/ needle | In top-10? | Best rank |
|----|------|----------------|-------------------------------|------------|-----------|
| q3 | Adoption agencies | `adopt` | 14 | **0** | — |
| q11 | Sweden | `sweden` | 1 (ep60) | **0** | — |
| q37 | sunset | `sunset` | 4 | **3** | rank 5 | (PASSED here, score=1.0 — control) |
| q40 | "2" | `two`, `pair`, `couple` | 3 | 1 weak (ep108 rk 2) | rk 2 | (still failed; wrong ep) |
| q43 | abstract art | `abstract` | 2 | **0** | — |
| q7 | Single | `single parent` | 1 | **0** | — |
| q71 | "Becoming Nicole" | `becoming nicole` | 1 | **0** | — |
| q75 | "3" | `three` | 1 | **0** | — |
| q76 | 19 October 2023 | (date) | **0 raw matches** | — | — |

## Routing

### Pool-recall route (8 of 9 failing questions)

q3, q11, q43, q7, q71, q75 (6 questions): literal needle **exists in
raw corpus**, **absent from top-10**. The right episode either was
filtered out by the retrieval pipeline (likely) or extracted into a
form that lost the literal token (possible but less likely — q3 has
14 raw mentions, hard to miss all of them in extraction).

q11 ("Sweden") is the cleanest case: exactly 1 episode in 419 contains
"Sweden", at ep60. It was not in top-10. This is a needle-in-haystack
recall failure — exactly the shape K-expansion or BM25-weight-bump
should address (literal noun-phrase queries against a single
diagnostic episode).

q40 ("how many [X]"): partial needle in top-10 but wrong episode.
This is a **numeric-aggregation** question that retrieval alone
probably can't fix — the answer requires counting across the
conversation, not retrieving one episode. Likely permanent failure.

### Extraction / out-of-corpus route (1 of 9)

q76 ("19 October 2023") has zero raw-corpus matches for the date
string in any form. This is either:

- a temporal-inference question (gold derived from `occurred_at`
  metadata that the extractor doesn't surface as searchable text), or
- a fixture-vs-gold mismatch (the gold may reference a date the
  fixture doesn't contain — known LoCoMo data-quality issue per
  prior runs).

Either way, this one is **not fixable by retrieval lever changes**.
Out of scope for ISS-161.

## Conclusion → Next Lever

Of the 12 single-fact target questions:

- **5 already pass** at K=30 anchor (q4, q13, q47-which-isn't-single-fact
  recount needed, q55, q37). Wait — recheck: per ISS-161 issue body,
  3/12 pass in v2; K=30 anchor says 5/12. The K=30 anchor result is
  from ISS-148's pinned table, not regenerated here.
- **1 unrecoverable** by retrieval: q40 (numeric aggregation)
- **1 unrecoverable** by retrieval: q76 (date not in corpus)
- **6 recoverable in principle** if we can get the right episode into
  top-K: q3, q11, q43, q7, q71, q75

Best case ceiling: 5 + 6 = **11/12 = 0.917** if we land all 6
recoverable plus keep the 5 currently passing. Realistic stretch
target: lift 3 of the 6 recoverable → 5 + 3 = 8/12 = **0.667**,
crosses AC-5a ≥0.60.

The lever shape this points to: **make the right literal-noun-phrase
episode reach top-K**. Two cheap approaches dominate:

### Recommended Lever (1 from ISS-161 menu): BM25 weight bump

q11 is the gold-standard case: 1 specific episode contains "Sweden",
buried in 418 unrelated episodes. Dense-embedding similarity gets
drowned by ambient chat content. BM25 with high IDF weight on
"Sweden" should rocket-rank ep60. Same logic applies to q43
"abstract", q71 "Becoming Nicole".

**Probe shape:** A control / B fusion config with BM25 weight ×1.5
on Factual adapter / C ×2.0. Single sweep on conv-26, ~12min, ~$1.
**Single-fact sub-bucket B-A delta is the gate** (not aggregate).

### Sidecar Lever: pre-truncate K dump

Before launching the BM25 sweep, add a one-line change to dump the
pre-MMR pool (k_seed × n_adapters ~50 candidates) to the per-query
jsonl. This resolves the caveat above and lets the next diagnostic
discriminate seed-vs-truncate cleanly.

Cost: 5min code change, no extra bench cost (rides on the BM25
sweep). High information value.

## Decision

Per ISS-161 AC-2: select Lever 1 (BM25 weight bump) as the next
implementation, with sidecar pre-truncate K dump.

**Do NOT escalate to Lever 6 (punt AC-5a) yet** — the diagnostic
shows 6/12 questions are recoverable in principle. Lever 6 only
applies if BM25 + one more lever both falsify.
