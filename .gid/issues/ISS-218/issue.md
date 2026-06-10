---
title: Specificity-preserving sliding-window extraction (fix ISS-162 recall-miss losses)
status: open
priority: P1
severity: regression-fix
category: extraction
created: 2026-06-09
relates:
- ISS-162
- ISS-217
- ISS-216
discovered_in: ISS-217 candidate-dump probe verdict 2026-06-09
depends_on:
- ISS-217
- .gid/issues/ISS-217/issue.md
blocks: .gid/issues/ISS-162/issue.md
---

## Summary

ISS-217 proved the conv-26 window-LOSS root cause is **extraction-layer
specificity degradation**, not vector ranking. With sliding-window ingest on
(N=4), the extractor — given preceding turns as context — re-paraphrases the
**current turn's fact less specifically**, dropping discriminating tokens
(proper-noun titles, explicit resolved dates). The lossy variant re-embeds to a
slightly different vector, gets different fusion neighbours, and the
gold-bearing memory **churns out of top-K**.

Evidence (clean same-binary A/B, conv-26, ISS-217):
- **q129** ⭐ smoking gun: window-off extracted `[2023-08-28] song 'Brave' by
  Sara Bareilles` (retrieved A rank-3, A=1.0). Window-on dropped the **title** →
  the gold memory is completely displaced from B's pool. B=0.0.
- **q6 / q20**: window-on lost the **date anchor** on the planning / museum
  memory → the date-bearing variant churned out.
- 6 of 10 real same-binary losses are this recall-miss pattern; the other 4 are
  downstream generation/judge issues unaffected by window policy.

This is the **mirror image** of the temporal WIN mechanism: window context
*helps* under-specified turns (adds a resolved date the bare turn lacked, +17pp
temporal) but *hurts* already-specific turns (paraphrases away tokens the bare
turn already had).

## Why not the obvious blanket fixes

- **N=4 → N=2** (reduce window): would weaken the temporal WINs too (those need
  the preceding-turn date context). Throws away the +17pp temporal gain to claw
  back ~6 questions. Net wrong trade.
- **Window only into temporal_grounding path**: plausible but coarse — the WIN
  set is broader than pure date resolution (some are entity coreference).

## Proposed fix: selective + specificity-preserving window

Two independent levers, ship/measure separately:

**Lever A — selective injection (gate the window):**
Only feed window context to the extractor when the bare turn is
**under-specified**: contains an unresolved reference (pronoun / "it" / "that" /
relative date "next month" / "last week") with no antecedent in the bare turn
itself. Specific turns (already carry a proper noun + explicit date) extract
from the bare turn alone — window can't paraphrase what it never sees.

**Lever B — preservation constraint (guard the extractor):**
When window context IS injected, constrain the extractor to **preserve verbatim
any proper noun or explicit date already present in the current (bare) turn**.
The window may ADD a resolved anchor; it must not REPLACE or paraphrase tokens
the bare turn already states. Implement as an extraction-prompt clause +
post-extract assertion (proper-noun / date tokens in bare turn ⊆ tokens in
emitted memory text, else fall back to bare-turn extraction for that turn).

Lever B is the root fix (preserves specificity regardless of gating); Lever A is
a cheaper coarse gate. Bench both; B alone may suffice.

## Acceptance criteria

- **AC-1**: A/B bench on conv-26 (canonical envelope: FACTUAL_REWEIGHT=on
  HYDE=off MMR=1.0 ENTITY_CHANNEL=off PIPELINE_POOL=1 POPULATE=off TOP_K=10),
  arm A = window-on/baseline-extractor, arm B = window-on/specificity-preserving.
  The 6 ISS-217 recall-miss qids (q6, q20, q82, q91, q129, q141): at least
  **q129 + 2 others** recover (gold re-enters top-K), verified via
  DUMP_CANDIDATES retrieved_candidates.
- **AC-2**: temporal WINs do NOT regress — temporal category Δ ≥ −2pp vs the
  current window-on baseline (ISS-217 Arm B temporal 0.443). The whole point is
  to keep the +17pp temporal gain.
- **AC-3**: overall conv-26 Δ ≥ +2pp vs current window-on baseline (0.2829).
- **AC-4**: cross-validate on conv-44 — recall-miss recovery reproduces and
  open-domain does not collapse further than the existing window-on regression.
- **AC-5**: unit test for Lever B preservation assertion: given a bare turn with
  a proper noun + explicit date and a window of paraphrasing context, the
  emitted memory text retains both tokens.

## Notes

- Generation-miss losses (q63 LLM computed wrong month from "next month"; q100
  judge phrasing-strictness) are OUT OF SCOPE here — they're not window-caused.
  q63 may warrant a separate "explicit-date-in-memory beats relative-reasoning"
  generation-prompt note.
- ISS-216 (heavy SessionState / rolling-summary design) remains the deferred
  alternative if selective+preservation proves insufficient.

---

## RESULTS — Lever B implemented + benched (2026-06-09)

**Implementation (flag-gated, default-off):**
- `extractor::assemble_with_context_preserving` — adds the PRESERVATION clause
  to the ISS-162 windowed framing (forbids paraphrasing away proper nouns /
  titles / explicit dates the final turn states).
- `memory::window_preserve_enabled()` — reads `ENGRAM_WINDOW_PRESERVE`
  (1/true/on/yes); call site at memory.rs selects the variant. Default-off →
  the proven ISS-162 framing stays byte-identical (`iss162_assemble_matches_isolation_framing` still green).
- Unit test `iss218_preserving_framing_keeps_specificity_tokens` (AC-5) ✅.
- 2144 engramai lib tests green.

**A/B bench (both window=4, conv-26, canonical envelope FACTUAL_REWEIGHT=on
HYDE=off MMR=1.0 ENTITY_CHANNEL=off PIPELINE_POOL=1 POPULATE=off TOP_K=10,
DUMP_CANDIDATES=1, same rebuilt binary):**
- Arm A `ENGRAM_WINDOW_PRESERVE=off`: `benchmarks/runs/ISS218-A-conv26-20260609T155222Z`
- Arm B `ENGRAM_WINDOW_PRESERVE=on`:  `benchmarks/runs/ISS218-B-conv26-20260609T155222Z`

| category    | A (off) | B (on)  | Δ        |
|-------------|---------|---------|----------|
| overall     | 0.2895  | 0.3289  | **+3.95pp** |
| multi-hop   | 0.1622  | 0.2432  | **+8.11pp** |
| single-hop  | 0.0938  | 0.1250  | +3.13pp  |
| temporal    | 0.4571  | 0.4857  | +2.86pp  |
| open-domain | 0.2308  | 0.2308  | 0.0      |

Net flips: **+16 gained / −10 lost = +6 net.**

### AC verdict
- **AC-1 ✅** ISS-217 recall-miss qids recovered: q20, q82, q91, **q129 (0.0→1.0,
  the smoking gun)** = 4/6 (needed q129 + ≥2). q6 + q141 still fail (q141 was the
  flagged-weak case; q6's planning-memory churn not resolved by preservation alone).
- **AC-2 ✅** temporal Δ +2.86pp (≥ −2pp) — preservation does NOT cost the
  ISS-162 temporal gain; it slightly raises it.
- **AC-3 ✅** overall Δ +3.95pp (≥ +2pp).
- **AC-5 ✅** unit test passes.
- **AC-4** (conv-44 cross-validation) — PENDING.

### Notes
- The +16/−10 churn includes re-ingestion noise + a few generation-miss cases
  (q13, q100) that were never window-fixable flipping the other way. The targeted
  recall-misses recovered and the net is clearly positive across single/multi/temporal.
- Multi-hop fully recovers the ISS-217 window-on regression (−8.11pp) back to the
  window-off baseline level — preservation removes the independent-fact penalty.

### Recommendation
Lever B (preservation clause) is effective. Next: AC-4 conv-44 cross-validation,
then decide whether to flip `ENGRAM_WINDOW_PRESERVE` to default-on (and unblock
ISS-162). Lever A (selective injection) NOT needed — preservation alone meets ACs.
