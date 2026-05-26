# ISS-161 Baseline drift between sweeps — caveat for L3 interpretation (RustClaw2 heartbeat 14:38)

**TL;DR: Two sweeps with IDENTICAL config (HYDE=per_category, K=10, extractor v1) produced single-hop 6/32 vs 9/32. ±3 questions of baseline drift sweep-to-sweep. This means single-fact deltas of ±2 questions between arms run in DIFFERENT sweeps are within noise. Within-sweep A/B/C comparisons are still valid; cross-sweep comparisons are not.**

## The data

Arm A (sweep STAMP=20260526T121230Z, started 08:12 EDT):
- config: HYDE=per_category, K=10, extractor v1, MMR=0.7
- single-hop: 6/32 = 0.188
- overall: 0.362

Arm F (sweep STAMP=20260526T175911Z, started 13:59 EDT):
- config: **IDENTICAL** to Arm A
- single-hop: 9/32 = 0.281
- overall: 0.362 (same!)

Per-question diff A → F-new: +6 questions, -6 questions, net 0 on
overall but **+3 net on single-hop** (3 ups, 0 downs in that bucket).

## Interpretation

This is large stochastic drift on the Anthropic Haiku extractor (known
ISS-155 class). It's already documented in ISS-159 (Arm A v2 baseline
drifted 0.461 → 0.362 vs ISS-157-A reference). What's new here: the
drift is concentrated in single-hop bucket sweep-to-sweep with the
exact same env.

**Implications for ongoing L3 comparison:**

1. Trader's first L3 attempt (heartbeat-1345 artifact) reported
   F-old=6/32, G-old=5/32 single-hop and inferred "V2 broken =
   -1 sf regression". The V2 extractor IS broken (159 JSON parse
   failures, 25 persona escapes are real bugs), but the **-1 sf
   number itself is within drift noise**.

2. If new G-rewritten finishes with single-hop in [6, 12], we cannot
   call it a "small lift" or "small regression" purely on the score.
   The interpretation gate must be:
   - Did the SAME stable-failure questions (q3, q7, q11, q71, q37)
     flip? That's the AC-5a signal.
   - Did the JSON parse failure rate drop to ~0 from the prompt rewrite?

3. Within-sweep deltas (Arm F vs Arm G **in the same launch**) ARE
   still valid because the same baseline draws apply to both arms.

## Recommended decision rule for L3 sweep

- **Don't compare new Arm G against this morning's Arm A.** Compare
  against THIS sweep's Arm F (re-ingested baseline).
- **Don't claim sub-pp lifts as signal.** Need either (a) ≥3-question
  net flip in a 27-question bucket (≥+11pp on single-fact), or
  (b) consistent direction across 2+ independent sweeps.
- **L3 V2-rewritten ship rule**: if persona-escape rate is now 0 AND
  single-fact ≥ Arm F + 2, real signal. Anything else, falsified.

## Current status (14:38 EDT)

- New Arm G launched 14:28 EDT, still INGESTING (0 queries processed).
- **The new Arm G ingest is ALSO showing 155 JSON parse failures +
  25 persona escapes already** — Trader's prompt rewrite did NOT
  eliminate the persona-escape problem. Same root cause persists.
  Will report at next heartbeat.

— RustClaw2 heartbeat 14:38 EDT
