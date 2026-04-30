# LoCoMo Evaluation Protocol v2 — Plan

> **⚠️ SUPERSEDED 2026-04-30** by [`../docs/locomo-protocol.md`](../docs/locomo-protocol.md).
> This doc is preserved for historical context (it captures the reasoning
> that led to the v2 protocol). Do **not** edit. For current protocol, see the link above.

**Date:** 2026-04-29
**Author:** rustclaw (decided with potato)
**Status:** Step 1 closed (2026-04-29) — see Verdict at bottom; full protocol superseded 2026-04-30.
**Context:** RUN-0001..0005 showed hit@5 too noisy on n=25 to drive engram v0.3 retrieval decisions. This doc records the chosen fix BEFORE doing it, so we don't forget mid-execution.

---

## Why noisy (root causes — from RUN-0003/0004/0005 data)

Five compounding causes, NOT one:

1. **n=25 too small** — single-query flip = 4pp swing, dominated by sampling noise.
2. **Class imbalance** — 17 Factual / 4 Abstract / 2 Affective / 2 Hybrid. Affective & Hybrid at n=2 each: one query flip = 50pp. Untestable.
3. **Ranking churn from candidate-pool changes** (RUN-0005 evidence) — ISS-068 fix added 19 new admitted memories, hit@5 dropped 56% → 48% because new neighbors pushed real golds out of top-5. Means we're measuring local top-5 stability, not retrieval capability.
4. **Gold annotation noise** (RUN-0004 q2 evidence) — LoCoMo flagged D1:12 as evidence but content unrelated to question; real answer in D1:14. dia_id exact-match scoring inherits LoCoMo annotation bugs. No one has audited the 25 gold rows.
5. **Structural-blocked queries** — 8 of 25 (32%) judged miss for non-retrieval reasons:
   - Abstract (4): all downgrade because L5 stub not wired.
   - Affective (2): all return `no_cognitive_state`.
   - Hybrid (2): all return empty.
   These contribute zero signal about retrieval quality.

**True effective n = 17 Factual queries.** Everything else is structural noise or scaffold-state indicator.

---

## Strategy

Tackle in order of (low cost × high signal gain). NOT all at once.

### Step 1 (this doc): Lower noise without changing v0.3 implementation

Three actions:

#### 1.1 Gold audit — 25 LoCoMo gold annotations

- For each of the 25 conv-26 sessions-1-3 queries, manually check:
  - question text
  - LoCoMo `evidence` dia_ids
  - actual content at those dia_ids in the substrate
  - verdict: `match` | `partial` | `noisy`
- Output: `engram/.gid/eval-runs/GOLD-AUDIT-conv26.md`
- DO NOT delete noisy gold from input — keep both columns (`hit_strict`, `hit_clean`) in the report.
- Time budget: ~40min.

#### 1.2 Driver — split headline + add diagnostics

Modify `crates/engramai/examples/locomo_conv26_retrieval.rs`:

- Headline change:
  - Before: `Hits @ 5: 14/25 (56.0%)`
  - After:
    ```
    Factual:           N/17 (X%)   ← real signal
    Abstract:          N/4         (downgrade rate Y%)
    Affective:         N/2         (no_cognitive_state Z/2)
    Hybrid:            N/2         (empty W/2)
    Structural-blocked: K/25 (queries judged miss for reasons unrelated to retrieval quality)
    ```
- Add metrics (only meaningful for Factual subset):
  - `recall@10` — does gold appear in top-10? (less sensitive to top-5 churn)
  - `MRR` — 1/rank of first hit, 0 if miss. (smooth — rank shifts visible)
- Add `hit_strict` vs `hit_clean` columns once GOLD-AUDIT is done. Until then, only `hit_strict` is reported.
- Time budget: ~30min.

#### 1.3 Run RUN-0006 with new protocol

- Same engram commit as RUN-0005 (925800b) — NO substrate change.
- Same query set.
- Compare:
  - hit@5 strict (should match RUN-0005 for sanity)
  - new diagnostics: per-plan headline, recall@10, MRR
  - hit@5 clean (after gold audit excludes noisy ones)
- Goal: establish v2-protocol baseline. NOT to chase a higher number.
- Time budget: ~20min.

### Step 2 (decided AFTER Step 1 lands)

Look at Factual-clean@17 with new metrics:

- If still noisy (e.g., recall@10 swings >10% across no-op runs) → need to expand dataset (n=25 too small even at clean).
- If stable → Step 1 was sufficient. Move on to fixing the structurally-blocked plans (separate ISS chain — Abstract L5 wiring, Affective cognitive-state, Hybrid fallback).

Do NOT decide Step 2 now. Look at Step 1 data first.

---

## Files this plan touches

- **Plan/decision (this file):** `engram/.gid/eval-runs/PROTOCOL-V2-PLAN.md`
- **Gold audit output:** `engram/.gid/eval-runs/GOLD-AUDIT-conv26.md` *(to be created in 1.1)*
- **Driver modified:** `engram/crates/engramai/examples/locomo_conv26_retrieval.rs` *(in 1.2)*
- **RUN-0006 result:** `engram/.gid/eval-runs/RUN-0006.md` *(in 1.3)*

## Files this plan does NOT touch

- Engram retrieval implementation (`crates/engram-retrieval/`, `crates/engram-resolution/`, etc.) — protocol-only changes, NOT chasing hit@5 with code patches.
- Substrate / ingestion — RUN-0006 reuses RUN-0005's substrate verbatim for fair comparison.
- LoCoMo dataset — gold audit is observation-only, no edits to `locomo10.json`.

## Out of scope for this plan

- Building "LoCoMo loop" tooling (run.sh, autopilot, etc.) — explicitly deferred. After RUN-0006 we may decide to script things, or not.
- Expanding to conv-25/27 or other LoCoMo sessions — Step 2 decision.
- Any v0.3 GOAL implementation — separate work.

---

## Execution checklist

- [ ] 1.1 Gold audit → `GOLD-AUDIT-conv26.md` *(deferred — see verdict)*
- [x] 1.2 Driver — headline split *(per-LoCoMo-category breakdown landed; recall@10 + MRR deferred — see verdict)*
- [x] 1.3 RUN-0006 with v2 protocol → `RUN-0006.md` *(2026-04-29 14:25 -04:00; bit-identical sanity-check vs RUN-0005)*
- [x] Update this file's status when each step lands
- [x] After 1.3: write a short verdict at the bottom

---

## Verdict (filled after Step 1.3 completes — 2026-04-29)

**v2 protocol decomposition succeeded; the decomposition itself revealed structural bugs that dwarf the gold-noise issue Step 1.1 was meant to address.**

### What landed

- **1.2 (partial)**: per-LoCoMo-category counters added to `locomo_conv26_retrieval.rs`. New output buckets cat=1..5 and prints a "cat 1-4 only" cleaner headline. recall@10 / MRR were NOT added — see "why deferred" below.
- **1.3**: RUN-0006 reproduced RUN-0005's 12/25 = 48% bit-identically (substrate verbatim, deterministic retrieval), then exposed the per-category shape:

  ```
  cat=1 Multi-hop   0/3   ( 0.0%)    structurally broken
  cat=2 Temporal    6/7   (85.7%)    strongest cohort
  cat=3 Open-ended  1/1   (100%)     n too small
  cat=4 Single-hop  3/9   (33.3%)    weakest answerable
  cat=5 Adversarial 2/5   (40.0%)    meaningless — gold is "unanswerable"
  ```

### New north-star (adopted)

**`hit@5 (cat 1-4) = 10/20 = 50.0%`** — primary headline going forward. Cat=5 reported separately ("abstain-correctness") because retrieval-hit ≠ correctness when the gold answer is "unanswerable".

### Why 1.1 (gold audit) and 1.2's recall@10/MRR are deferred

Step 1.1 was designed to disambiguate `hit_strict` vs `hit_clean` — i.e., to salvage a noisy headline number on n=25. RUN-0006 surfaced that **the headline noise was masking ≥2 structural bugs**, not gold-label noise:

- **Multi-hop plan dispatcher has zero graph traversal** — falls through to single-shot Factual/Abstract. → ISS-070 (P0).
- **Affective plan short-circuits to `no_cognitive_state` no-op** — orchestrator never threads `self_state`. → ISS-071 (P1).
- (Plus ISS-069 from RUN-0005 — ranking instability under candidate-pool growth.)

Fixing those three structural bugs will move the headline more than any audit refinement. Gold-audit + recall@10/MRR remain valid future work but are not the marginal next action.

### Next move

1. Triage ISS-070 / ISS-071 / ISS-069 — these are now the bottlenecks, not protocol noise.
2. After at least one structural fix lands, re-run the same v2 protocol against the same substrate to measure delta.
3. If headline cat-1-4 hit@5 stops moving despite fixes (i.e., we're chasing gold-label noise), revive 1.1 (gold audit) and 1.2 (recall@10 + MRR) at that point.

**Status: Step 1 closed. Step 2 = ISS triage, not dataset expansion.**
