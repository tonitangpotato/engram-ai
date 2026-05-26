# ISS-161 L3 round-2 final result + drift characterization update (RustClaw2 heartbeat 15:35)

**TL;DR: L3 round 2 confirms (a) the v2 prompt rewrite did NOT fix the persona-escape problem (206 JSON parse failures, 34 "I'm Claude", 77 "I appreciate" in Arm G ingest) — V2 extractor remains broken. (b) The baseline drift I flagged at 14:38 is now more precisely characterized: drift is in the LIST sub-bucket only. Single-fact (AC-5a target) has been ROCK-STABLE at 5-6/27 across all 4 arms tested today. (c) The 9 known failing single-fact questions (q3, q7, q11, q37, q40, q43, q71, q75, q76) ALL stayed at 0 in every arm — F-new included. Empirically, no lever tested today has moved the AC-5a target.**

## Final L3 round-2 numbers

| Arm | Config | overall | single-hop | sf (n=27) | list (n=5) |
|-----|--------|---------|------------|-----------|------------|
| F-new | extractor v1 (control), HYDE pc, K=10 | 0.362 | 9/32 | 6/27 | 3/5 |
| G-new | extractor v2 rewritten, HYDE pc, K=10 | 0.349 | 4/32 | ~2/27 | ~2/5 |

F-new — sweep finished, valid data.
G-new — extractor broken (see persona-escape counts above), data invalid.

## Baseline drift: now characterized as LIST-bucket-only

Earlier this morning Arm A (same exact config as F-new): single-hop 6/32.
F-new this afternoon: single-hop 9/32.

The 3 questions that flipped UP A→F-new:
- q32: "Pride parade, school speech, support group" (LIST)
- q65: "Changes to her body, losing unsupportive friends" (LIST)
- q70: "Poetry reading, conference" (LIST)

**All 3 drift flips are in the LIST sub-bucket (n=5 per Trader's classification).** This means:

1. The list bucket is unstable across sweeps with same config (±3 of 5
   questions can flip). That's ±60% — list bucket numbers are too
   noisy to draw conclusions from across sweeps.
2. **The single-fact bucket is STABLE.** Single-fact scores 5-6/27
   across A, F-new — within ±1 question.
3. **The 9 known failing single-fact (q3, q7, q11, q37, q40, q43,
   q71, q75, q76) ALL stayed at 0 across A, F-new, G-new.** Not a
   single drift flip on any of them.

## What this means for ISS-161

The picture is now clear:

- **Single-fact AC-5a target: 5-6/27 ≈ 0.20** has been the ceiling
  across L1 (BM25 baseline), L2 (HYDE pc, HYDE pc_v2), L3 (extractor
  v2 — broken), L7 (gen v2). No retrieval-side or generation-side
  lever has moved the 9 stubborn single-fact questions in 7+ arms.
- **q40 and q43 flipped under HYDE pc_v2** (Arm B, Arm C) but not in
  any other arm. Those were the only single-fact lifts seen all day,
  and only on Arm B/C with HYDE pc_v2 + K≥10. Probably worth keeping
  as the "best so far" config: **8/27 = 0.296**.
- **AC-5a 0.60 gate = 17/27.** Distance: 9 single-fact questions
  must flip. Empirically: 0 levers tested today move any of these 9.

## Recommendation

Per Trader's 12:40 decision rule: "if Lever 3 fails, file Lever 6
(redefine AC-5a) with conviction." L3 attempt is now empirically
broken twice. The persona-escape issue is a real ALIGNMENT-LAYER
problem with Claude — even mechanical-looking prompts trigger
"I appreciate the kind words but I should clarify..."

**Three honest options for potato:**

1. **Switch extractor model** (not Claude). Use an OpenAI/local/Ollama
   model that doesn't have Claude's persona-escape reflex on
   extraction tasks. Re-run L3 with that. ~2h work, ~$3.
2. **Pure schema-only prompt** — strip ALL natural language from
   extractor prompt down to JSON schema + 3 examples, no rules text
   at all. Test if persona-escape rate drops to 0%. ~30min work.
3. **File Lever 6 (redefine AC-5a)** to ≥0.30 or whatever
   8/27 = 0.296 supports honestly. Ship 0.3-class single-fact, file
   q3/q7/q11/q71 etc. as a follow-up "future work" issue.

Option 2 is the cheapest test before falling back to option 3.
Option 1 is a bigger change but probably what production should be
doing anyway (Claude is an expensive choice for mechanical extraction).

— RustClaw2 heartbeat 15:35 EDT
