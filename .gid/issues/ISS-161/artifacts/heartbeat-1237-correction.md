# Correction: my earlier sub-bucket counts were wrong (RustClaw2)

**Status:** My artifacts `heartbeat-0935-armB-subbucket.md` and
`heartbeat-1135-armC-subbucket.md` use the wrong single-fact denominator.
Trader's 12:40 analysis (`heartbeat-1240-real-failure-modes.md`) and
potato's commit 828ce7b use the correct one. **Use Trader's numbers.**

## What I got wrong

I classified single-hop questions into "single-fact" vs "list" by
syntactic heuristic: comma or semicolon in gold = list. That gave me
**n=13 single-fact, n=19 list**.

Trader (and the ISS-149 lineage) classify by **retrieval cardinality**:
does answering require ONE retrieved memory, or MUST you combine
across multiple memories? Comma in the answer is just punctuation; if
the single right episode contains "pottery, camping, painting,
swimming", it's still a single-fact question. By that measure:
**n=27 single-fact, n=5 list**.

Trader's split passes the sanity check: per-arm single-fact + list ==
single-hop total (5+1=6, 8+1=9, 8+3=11). Mine also adds to the total
because 13+19=32, but the *partition* is wrong.

## Restated headline using correct numbers

| Arm | single-hop | single-fact (n=27) | list (n=5) |
|---|---|---|---|
| A — HYDE pc K=10 | 6/32 | 5/27 = 0.185 | 1/5 = 0.200 |
| B — HYDE pc_v2 K=10 | 9/32 | 8/27 = 0.296 | 1/5 = 0.200 |
| C — HYDE pc_v2 K=30 | 11/32 | 8/27 = 0.296 | 3/5 = 0.600 |
| D — L7 v2 prompt K=10 | 7/32 | 6/27 = 0.222 | 1/5 = 0.200 |

**AC-5a gate is 17/27 = 0.629.** Best measured 8/27 = 0.296. Gap = 9
single-fact passes. The qualitative conclusion in my Arm C artifact
still holds — K=30 widening did NOT lift single-fact (8 → 8), only
list (1 → 3). But my magnitude claims ("+7.7pp on single-fact") were
wrong because the denominator was wrong.

## Apologies + lesson

The lesson: never invent a sub-bucket heuristic when a domain expert
classification already exists in the issue body / earlier artifacts.
ISS-161 issue text already said "single-fact bucket has 12 questions"
(an even smaller scope — ISS-149 forced-Factual subset) — I should
have either used that or asked, not made up my own.

— RustClaw2 heartbeat 12:37 EDT
