# RUN-0018 — ISS-100 Layer 2 e2e (occurred_at driver path live)

**Date:** 2026-05-05
**Issue:** ISS-100 (LoCoMo Layer 2 — occurred_at temporal grounding)
**Pipeline:** engram-bench `./target/release/engram-bench locomo` (standalone, no cogmembench)
**Substrate:** engram main (with ISS-103 fix: `created_at = wall-clock`, `occurred_at = caller-supplied`)
**Fixture:** LoCoMo conv-26 (real, 152 questions, full split)
**Artifacts:** `engram-bench/.gid/eval-runs/2026-05-05T20-24-01Z_locomo/`

## Headline

| Metric | RUN-0017 (pre-fix) | RUN-0018 (post-fix) | Δ |
|---|---|---|---|
| Overall J-score | 3.6% | **42.1%** | **+38.5pp** |
| n_queries | 152 | 152 | — |

ISS-100 Layer 2 occurred_at driver path is **alive**. The 38.5pp jump confirms the temporal grounding pipeline (ingest → store → retrieve with caller-supplied event time) is working end-to-end. RUN-0017's 3.6% was the symptom of `occurred_at` being silently overwritten by `created_at = Utc::now()` on store — fixed in ISS-103.

## Category breakdown

| Category | n | Mean J | Zero-score count |
|---|---|---|---|
| multi-hop | 37 | 56.8% | 16 |
| temporal | 70 | 50.0% | 35 |
| open-domain | 13 | 38.5% | 8 |
| **single-hop** | **32** | **9.4%** | **29** |

**temporal at 50% is the headline win** — that's exactly the category Layer 2 was meant to lift, and it moved from near-zero to half-passing.

**single-hop at 9.4% is anomalous** and triggered a separate investigation (see ISS-104, opened from this run).

## Latency profile (152 queries)

- avg total: 6.48 s
- avg retrieve: 33 ms ← retrieval is fast, not the bottleneck
- avg generate (LLM compose answer): 3.60 s
- avg judge (LLM verdict): 2.84 s
- p95 total: 8.21 s

## Plan execution

- 48 plan downgrades in stdout (`abstract unavailable` → fallback to associative; affective `no_self_state` → fallback)
- All 152 queries returned `outcome=ok` from `execute_plan` — no panics, no errors
- L5 abstract reasoning + affective plans never fired in this run; everything ran on associative + episodic plans

## Ship-gate decision

```
P0 Ship Gates
─────────────
  [FAIL]  GOAL-5.1   LOCOMO overall J-score 0.421 < 0.685 (target)
  [ERROR] GOAL-5.2   baseline 'Graphiti temporal' unavailable

Decision: BLOCK  (1 blocking gate(s): GOAL-5.2)
```

GOAL-5.1 fails on threshold (0.421 < 0.685) but moved the right direction. GOAL-5.2 errors because Graphiti temporal baseline isn't wired yet — separate work item.

## Observations & follow-ups

1. **single-hop 9.4% anomaly** — 29/32 zeros. Two failure modes:
   - **List-typed gold** (~10 cases): gold is comma-separated ("pottery, camping, painting, swimming"); model answers one item; judge strictly returns No. **Judge / fixture format issue, not retrieval.**
   - **"I don't know" abstain** (~19 cases): retrieval returned candidates (33ms typical), but generator LLM looked at them and abstained. Need to inspect candidate quality vs. gold to decide if this is recall failure or generator over-conservatism.
   - **Tracked in:** `engram-bench/.gid/issues/ISS-001` (opened with this run; engram-bench's first issue)

2. **L5/affective never fires** — 48 fallback events in stdout. Either the planner never selects these on conv-26 questions, or the substrate signals required to trigger them aren't being produced. Worth confirming what the activation criteria are before deciding if this is a bug or expected.

3. **Graphiti baseline gate** — GOAL-5.2 errors on missing baseline. Need to either wire Graphiti or temporarily mark the gate optional until baseline harness lands.

4. **Hot path opportunity** — generate+judge dominate latency (6.4s of 6.5s). Retrieval is already at 33ms. Throughput optimization should target LLM call concurrency, not retrieval.

## Smoke-to-full corroboration

Earlier 5-question smoke after the ISS-103 fix went 40% → 60% (vs RUN-0017's 20% on the same 5). Full 152 ran at 42.1% — consistent. The smoke wasn't a false positive.

## Files

- Per-query JSONL: `engram-bench/.gid/eval-runs/2026-05-05T20-24-01Z_locomo/locomo_per_query.jsonl`
- Summary JSON: `engram-bench/.gid/eval-runs/2026-05-05T20-24-01Z_locomo/locomo_summary.json`
- stdout log: `engram-bench/.gid/eval-runs/RUN-0018.stdout`

## Next

- Close ISS-103 (fix shipped, regression tests pass, e2e confirms pipeline alive)
- Investigate ISS-104 (single-hop anomaly) — likely judge fixture issue + generator tuning
- Decide GOAL-5.2 path: implement Graphiti baseline OR mark gate as work-in-progress
- Then re-run for higher overall J once single-hop is unstuck
