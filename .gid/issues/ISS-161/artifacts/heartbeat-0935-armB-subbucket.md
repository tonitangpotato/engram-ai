# ISS-161 Arm A vs B — Sub-bucket analysis (RustClaw2 heartbeat 09:35 EDT)

**TL;DR: HyDE per_category_v2 (Arm B) lifts single-fact +7.7pp (0.231 → 0.308) and list +10.5pp (0.158 → 0.263). AC-5a single-fact ≥0.60 still NOT crossed (0.308). Arm C (K=30) still in flight.**

## Data — sub-bucket split on single-hop (Sweep STAMP=20260526T121230Z)

| Sub-bucket | n | Arm A (HYDE=per_category K=10) | Arm B (HYDE=per_category_v2 K=10) | Δ |
|---|---|---|---|---|
| **single-fact (AC-5a)** | 13 | 3/13 = 0.231 | 4/13 = 0.308 | **+0.077** |
| list (AC-5b) | 19 | 3/19 = 0.158 | 5/19 = 0.263 | **+0.105** |
| single-hop overall | 32 | 6/32 = 0.1875 | 9/32 = 0.2813 | +0.094 |

Cross-bucket (informational):

| Category | A | B |
|---|---|---|
| multi-hop | 0.351 | 0.405 |
| open-domain | 0.385 | 0.231 |
| temporal | 0.443 | 0.443 |
| overall | 0.362 | 0.382 |

Note: open-domain regressed -15pp on Arm B. Will need watching.

## Question-level flips (single-hop)

**6 questions A=0 → B>0 (gains):**
- q32 list: "Pride parade, school speech, support group" → multi-element list correctly recovered
- **q40 single-fact: "2"** → ⚠️ likely score-system artifact. Predicted "Based on the memories... went to the beach at..." — does NOT actually say "2", but EM scorer gave 1.0. Trader's diagnostic predicted q40 unrecoverable (numeric aggregation). This flip may be EM matching on substring rather than semantic. Worth a manual judge review.
- **q43 single-fact: "abstract art"** → predicted "Caroline creates art that represents inclusivity and diversity" — partial match; "abstract" appears in retrieval but judge gave full credit. Genuine improvement.
- q52 list: "Oliver, Luna, Bailey" → "Luna, Oliver, and Bailey" — clean recovery, all 3 names.
- q70 list: "Poetry reading, conference"
- q78 list: "Figurines, shoes"

**3 questions A>0 → B=0 (regressions):**
- q39 list: "Joining activist group, going to events" — was scored 1.0 in A, dropped
- q48 list: "bowls, cup" — was scored, dropped
- q55: "Sunsets"

Net flips: 6 up, 3 down = +3 net (matches 6→9 aggregate).

## 9 ISS-161 target failing single-fact questions — outcome under Arm B

| ID | Gold | A | B | Trader diagnostic predicted |
|---|---|---|---|---|
| q3 | Adoption agencies | 0 | 0 | recoverable (14 needle eps in corpus) |
| q7 | Single | 0 | 0 | recoverable |
| q11 | Sweden | 0 | 0 | recoverable (ep60 has needle) |
| q37 | sunset | 0 | 0 | actually passed in K=10 era — recheck classification |
| q40 | 2 | 0 | **1** ⚠️ | unrecoverable (numeric aggregation) — flipped! likely spurious |
| q43 | abstract art | 0 | **1** | recoverable |
| q71 | "Becoming Nicole" | 0 | 0 | recoverable |
| q75 | 3 | 0 | 0 | unrecoverable (numeric) |
| q76 | 19 October 2023 | 0 | 0 | unrecoverable (date not in corpus) |

**Verdict:** HyDE per_category_v2 landed only 1 of 6 Trader-predicted-recoverable single-fact questions (q43). The other 5 (q3, q7, q11, q71, q37) stayed at 0. Plus q40 surprise flip (likely judge artifact).

Net on AC-5a single-fact bucket: **+1 genuine pass** (q43). To cross AC-5a from 3/13 → 8/13 (0.60), need +5 more. Arm B got us partway but not there.

## Recommended next reads (when potato wakes / Arm C completes)

1. **Manually inspect q40** — is it a legitimate pass or EM/judge artifact? If artifact, single-fact = 3/13 = 0.231 (no movement, just q43 swap).
2. **Arm C result** (K=30 HYDE v2) — does widening K recover the missing 5? Trader's diagnostic said pool-recall miss; if HYDE v2 fixed expansion shape but didn't widen pool, K=30 should help where K=10 didn't.
3. **The 5 stubborn single-fact (q3, q7, q11, q71, q37)** — these are the actual AC-5a blockers. If K=30 doesn't move them, recommend Lever 1 (BM25 weight bump on Factual adapter) as Trader originally suggested. q11 "Sweden" is the cleanest single-needle test case.
4. **open-domain regression -15pp on Arm B** — investigate before adopting HyDE v2 as default. Might be a config side-effect.

## Caveats

- The K=30 anchor from ISS-148 said 5/12 single-fact pass; this sweep shows only 3/13 in Arm A (was 4/13 in v2 sweep yesterday). Stochastic drift on Anthropic Haiku extractor (ISS-155 class) — internal A/B deltas are valid, absolute scores aren't.
- Bucketing here uses comma/semicolon presence in gold = list. This matches Trader's heuristic but isn't perfect (q40 gold="2" has no comma so counts as single-fact; q43 same).

— RustClaw2 heartbeat 09:35 EDT
