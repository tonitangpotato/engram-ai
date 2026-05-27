# ISS-164 Phase 1 Bench Result — RustClaw heartbeat 19:35 EDT 2026-05-26

**Sweep:** /tmp/iss164-bench (PID 96640 — completed)
**STAMP:** 20260526T213218Z (= 17:32 EDT 2026-05-26 launch)
**Wall:** Arm A 26min + Arm B 26min ≈ 53min
**Config:** HYDE=off, K=10, MMR=on, single conv-26, Anthropic Haiku extractor (non-determinism caveat applies)

## Headline numbers

| arm | overall | single-hop | multi-hop | open-domain | temporal |
|-----|---------|------------|-----------|-------------|----------|
| A (entity=off, control) | 0.395 | 0.188 (6/32) | 0.459 | 0.308 | 0.471 |
| B (entity=on)           | 0.362 | **0.250 (8/32)** | 0.351 | 0.308 | 0.429 |
| Δ (B-A)                 | -0.033 | **+0.0625** | -0.108 | 0.000 | -0.043 |

Within-sweep, within-extraction comparison. Internal A/B delta is the
valid signal (per Trader 14:38 EDT caveat on baseline drift).

## Per-question single-hop flips A→B

**2 flips, both LIST-bucket:**

- ↑ **q32**: gold = "Pride parade, school speech, support group" (3-item list)
- ↑ **q39**: gold = "Joining activist group, going to pride parades, ..." (multi-item)

## ZERO movement on 9 stubborn single-fact questions

These are the canonical AC-5a blockers (Trader 15:35 EDT analysis): all still 0.0 in Arm B.

- q3  "Adoption agencies"
- q7  "Single"
- q11 "Sweden"
- q37 "sunset"
- q40 "2"
- q43 "abstract art"
- q71 "Becoming Nicole"
- q75 "3"
- q76 "19 October 2023"

## Interpretation

**Entity channel (ISS-164) is a LIST-bucket lever, NOT a stubborn-single-fact lever.**

- Mechanism: when the anchor entity is the protagonist Caroline/Melanie, the entity channel pulls every memory mentioning the anchor — natural recall-boost for questions where the gold answer is a *set of items* attributed to that anchor.
- The 9 stubborn single-fact questions all need a specific factual recall that the anchor doesn't disambiguate. Per ISS-161 verdict (16:36 EDT): these are likely ISS-162 extractor-context misses (e.g. q3 "adoption agencies" lives in the prev question's noun phrase, lost at single-`&str` extraction).

## AC-5a status

Best single-fact still ~8/27 = 0.296 (counting q32+q39 as list, the 6 baseline single-fact correct remain unchanged). Gap to 0.629 unchanged.

**ISS-162 (extractor sliding-window context) is now the highest-leverage open architecture gap.** ISS-163 (semantic dedup) is the second.

## Caveats

- HYDE=off this sweep; absolute numbers NOT comparable to morning's HYDE=per_category baselines.
- Single conv-26 only; conv-44 cross-check still pending (ISS-160 doctrine).
- Multi-hop -10.8pp is a real regression in this config — entity channel weights are squeezing multi-hop's room. Worth a follow-up to tune the channel cap (currently PER_ENTITY_MEMORY_CAP=16).

## Author

RustClaw main instance, 19:35 EDT 2026-05-26. Filed as heartbeat artifact, not a normative finding. potato to ratify.

---

## ⚠️ SUPERSEDED 19:38 EDT

This artifact was filed by RustClaw main without first checking the artifacts directory. Trader instance had already filed `heartbeat-1835-phase2-verdict.md` (18:37 EDT) and `heartbeat-1839-verification.md` (18:42 EDT) reaching identical conclusions, with multi-hop/temporal/open-domain breakdown that this file lacked.

Authoritative source: heartbeat-1835-phase2-verdict.md (Trader, 18:35).

Keeping this file rather than deleting (SOUL.md no-delete rule) but marking it superseded so it doesn't get picked up as a separate finding.
