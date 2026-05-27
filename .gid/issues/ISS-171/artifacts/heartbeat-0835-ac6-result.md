# ISS-171 AC-6 — bench result (rustclaw2 watcher, 2026-05-27 08:35 EDT)

**Author:** rustclaw2 (heartbeat watcher, paper-only analysis)
**Status:** within-sweep B vs A delta is real; within-sweep absolute floor is low.

## Sweep identity

- Script: `/tmp/iss164_bench_sweep.sh` (ENGRAM_BENCH_ENTITY_CHANNEL on/off, K=10, HyDE=off, MMR=off, cross-encoder=off, force_intent=off, ENGRAM_BENCH_PIPELINE_POOL=1)
- Stamp: `20260527T112718Z` (07:27→08:31 EDT, ~64 min wall)
- Arms: `ISS164-A-conv26-20260527T112718Z` / `ISS164-B-conv26-20260527T112718Z`
- Engram commit at run: `7e0447e` (ISS-171 GraphEntityLookup wired) → `b8eddb9` (issue lifecycle only)
- conv-26 only. n=152 questions.

## Overall numbers (within-sweep)

```
A (entity channel OFF) overall 0.2039
B (entity channel ON)  overall 0.2303    Δ = +2.6pp
```

## Per-category (within-sweep)

```
category      n   A       B       Δ
multi-hop    37   0.1892  0.2162  +2.70pp
open-domain  13   0.2308  0.2308  +0.00pp
single-hop   32   0.0625  0.1562  +9.38pp   ← biggest move
temporal     70   0.2714  0.2714  +0.00pp
```

## Single-hop bucket detail (where ISS-164's hypothesis lives)

```
A: n=32, mean=0.0625, wins (score>0.5)=2
B: n=32, mean=0.1562, wins (score>0.5)=5
```

**+3 wins in B**, hits the script-header decision threshold of +2.

## 9 stubborn single-fact questions (ISS-171 paper trace target)

```
id           cat          A     B    flip
conv-26-q3   single-hop  0.00  0.00
conv-26-q7   single-hop  0.00  0.00
conv-26-q11  single-hop  0.00  0.00
conv-26-q37  single-hop  0.00  1.00   ↑
conv-26-q40  single-hop  0.00  0.00
conv-26-q43  single-hop  0.00  0.00
conv-26-q71  single-hop  0.00  0.00
conv-26-q75  single-hop  0.00  0.00
conv-26-q76  single-hop  0.00  0.00
```

**1/9 flipped.** Eight still stuck at 0/0 even with the entity channel ON post-fix. The fact-that-it-moved-at-all is the first non-zero signal we've seen on this set across all three ISS-164 sweep generations (yesterday, 05:11Z, 11:27Z).

## Flip distribution (within-sweep)

```
B>A: 8 questions
A>B: 4 questions
same: 140 questions
```

Net +4 questions improved. Concentrated in single-hop (where +3 of the +4 net live).

## Decision-rule check (per sweep script header)

Script's authoritative threshold: `B-sf − A-sf ≥ +2 → ISS-164 ships; 0 or +1 → falsified.`

The script's "sf" is the **n=27 single-fact sub-bucket** from earlier ISS-161 work, not the full 32-question single-hop category. The n=12 / n=20 ISS-165 redefinition (single-fact vs list) supersedes that older n=27, and I don't have the per-question label file for it loaded — so I can't compute the script's exact metric. What I *can* report:

- Full single-hop n=32: +3 wins → hits +2
- 9 stubborn single-fact subset (the items every previous sweep showed 0/9): **1 flip**

If the script's `sf` is closer to the 9-stubborn-style subset, this is +1 (falsification side per the header rule). If it's closer to full single-hop n=32, this is +3 (ship side). **potato needs to apply the canonical single-fact label set to decide which side of the decision rule this lands on.**

## Classifier routing — not directly observable

per_query.jsonl in this harness only has `id, category, predicted, gold, score, latency_*, verdict_raw, tokens_*`. No `plan_kind`. Cannot confirm directly that Factual fired. The +9.4pp single-hop lift is consistent with Factual now being reachable, but not proof.

If potato wants direct confirmation, the bench harness would need to surface `plan_kind` per query (small follow-up — execute_plan already logs ENTER); or grep the run's stderr for "plan_kind=factual" if it was captured (the `/tmp/iss164-bench/iss164-{A,B}.log` files may have it).

## Cross-sweep observation (advisory only — NOT a decision input)

The absolute scores dropped sharply vs the 05:11Z sweep:

```
sweep                 A overall   B overall
20260526T213218Z      0.395       0.362       (pre-165/166/167/171)
20260527T051146Z      0.329       0.329       (post-165/166/167, pre-171)
20260527T112718Z      0.204       0.230       (post-171, this run)
```

By the within-sweep rule this comparison is invalid (drift, fresh ingest, different substrate). Calling it out only so potato can decide if there's a regression worth investigating separately from ISS-164/171's A/B test. The candidate causes are: (a) ISS-171's GraphEntityLookup adding per-token lock contention to Stage-1, (b) ISS-165's mention-extraction now firing on every query (verb/common-noun false positives — that's already filed as ISS-169), (c) ISS-166/167 pool wiring changing what's in the substrate. Not my call to dig further; flagging for visibility.

## Recommended next moves (if asked)

1. **Apply the n=12 single-fact / n=20 list label set** from ISS-165 to today's per_query.jsonl and recompute B-sf − A-sf against the decision rule.
2. **Surface `plan_kind` per query** in the bench harness so AC-6 evidence is direct.
3. **If decision is "ship"**: re-run on conv-44 per the ISS-160 inverted-ratio protocol before flipping FusionConfig::locked default.
4. **If decision is "falsify"**: ISS-169 (verb false positives) is the most likely candidate suspect — the entity channel may be pulling enough wrong anchors to wash the right ones in 8/9 of the stubborn set.

## What I did NOT do

- Did not modify bench output files
- Did not touch ISS-164/171 issue lifecycle
- Did not draw a "ship/falsify" conclusion (n-set ambiguity makes that potato's call)
- Did not re-run the bench

— rustclaw2
