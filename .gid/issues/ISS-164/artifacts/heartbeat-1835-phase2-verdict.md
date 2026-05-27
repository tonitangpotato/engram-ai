# ISS-164 Phase 2 A/B verdict — entity channel does NOT lift AC-5a + regresses elsewhere (RustClaw2 heartbeat 18:35)

**TL;DR: Entity channel A/B clean comparison (within-sweep, HYDE=off, K=10) shows: zero of the 9 stubborn single-fact questions flipped. The +2 single-hop apparent gain is in the LIST sub-bucket (same drift signature as morning). Meanwhile multi-hop regressed -4 questions and temporal -3. Overall -3.3pp. The Phase 1 implementation is NET NEGATIVE on conv-26. Per the sweep script's own stop-rule, this is a "channel actively hurts → file root-cause issue" situation.**

## Numbers

| Bucket | n | A (channel off) | B (channel on) | Δ |
|---|---|---|---|---|
| single-hop | 32 | 6/32 = 0.188 | 8/32 = 0.250 | +2 ↑ |
| multi-hop | 37 | 17/37 = 0.459 | 13/37 = 0.351 | **-4 ↓** |
| temporal | 70 | 33/70 = 0.471 | 30/70 = 0.429 | -3 ↓ |
| open-domain | 13 | 4/13 = 0.308 | 4/13 = 0.308 | 0 = |
| **overall** | 152 | **0.395** | **0.362** | **-3.3pp ↓** |

## Within single-hop: where did the +2 come from?

Two questions flipped UP, zero flipped DOWN:

- **q32 (LIST): "Pride parade, school speech, support group"** — B retrieved Caroline's list of activities
- **q39 (LIST): "Joining activist group, going to pride parades..."** — B retrieved another activist participation list

**Both flips are in the LIST sub-bucket (Trader's n=5).** Single-fact sub-bucket (n=27, AC-5a target) was unchanged.

This is the SAME drift signature as morning's Arm A → Arm F-new baseline noise: the 3 questions that drifted UP with identical config were q32, q65, q70 — all LIST answers. The list bucket has ±60% sweep-to-sweep noise. Two list flips here cannot be attributed to the entity channel — they could be entirely baseline noise.

## The 9 stubborn single-fact questions

ALL still at 0 under entity channel:

| qid | gold | A | B |
|---|---|---|---|
| q3 | Adoption agencies | 0 | 0 |
| q7 | Single | 0 | 0 |
| q11 | Sweden | 0 | 0 |
| q37 | sunset | 0 | 0 |
| q40 | 2 | 0 | 0 |
| q43 | abstract art | 0 | 0 |
| q71 | "Becoming Nicole" | 0 | 0 |
| q75 | 3 | 0 | 0 |
| q76 | 19 October 2023 | 0 | 0 |

**Even q43 "abstract art" (which flipped under HYDE pc_v2 earlier) did NOT flip here.** That's because this sweep uses HYDE=off — the morning's q43 flip was a HYDE pc_v2 effect, not an entity-channel-class effect.

## The regression is real

-4 questions on multi-hop and -3 on temporal are NOT within drift noise (multi-hop n=37, temporal n=70 — bigger sample, lower drift variance). This is signal that entity channel is **pulling wrong candidates into the pool**, displacing better candidates that worked under associative-without-entity.

Likely root cause hypothesis: ResolvedAnchors injection in Step 2b uses `memories_mentioning_entity()` — but for multi-hop / temporal queries, the right memories are NOT the ones that "mention" the entity, they're the ones that contextually relate. Entity channel might be displacing context-rich edges with shallow entity-mentioning matches.

## Recommendation

**Do NOT ship ISS-164 Phase 1 as default behavior** (the opt-in flag mechanism is good — keep it). The script's built-in stop-rule should fire: "channel actively hurts; revert Phase 1 commits 77ef3f3."

Before reverting, two cheap diagnostics:

1. **Per-question dump for the 4 lost multi-hop questions** — are they cases where the previously-winning memory had narrative/temporal context but was pushed out by an entity-only match?
2. **Test ISS-164 + HYDE=per_category_v2** — maybe the +9 single-hop lift from HYDE pc_v2 + the +2 from entity channel are additive on different questions. (Risky to test if entity-channel alone hurts overall — but cheap to launch since infrastructure is ready.)

If neither diagnostic produces a clear lift on AC-5a target single-fact, the honest move is **file Lever 6 (redefine AC-5a)** per Trader's 12:40 decision rule.

## Note on directions remaining

After today's exhaustive sweep:
- L1 BM25: never tested ← Trader recommended originally, still untested
- L2 HYDE pc_v2: +2 single-fact (q40 + q43), but not on the stubborn 9
- L3 extractor v2: BROKEN (Claude alignment reflex)
- L7 gen v2: falsified
- ISS-164 entity channel: 0 single-fact lift, -3.3pp overall

**The only retrieval lever that hasn't been tested is L1 BM25 weight bump on Factual adapter.** Trader recommended it back at 11:00 EDT. Worth a probe before declaring AC-5a unreachable.

— RustClaw2 heartbeat 18:35 EDT
