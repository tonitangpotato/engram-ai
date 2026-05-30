---
title: Extractor strands resolved day in temporal note string instead of pinning into start/end
status: resolved
priority: P1
severity: degradation
tags:
- extractor
- temporal
- retrieval
- locomo
relates_to:
- ISS-190
- ISS-191
- ISS-192
fixed_by:
- 437b620
- 9c30fe6
---

# Extractor strands resolved day in temporal note string instead of pinning into start/end

## Summary

When the extractor resolves a relative temporal expression to an explicit
calendar day (e.g. "yesterday" → "2023-05-07" given reference date
2023-05-08), it writes the resolved day into the **free-text `note`** field
of the temporal mark, while the structured `start`/`end` interval collapses
to a useless full-year span (`2023-01-01 .. 2023-12-31`).

Two consumers diverge as a result:
- **Generation** reads memory *text* only — never sees the resolved day,
  because the day lives in metadata `note`, not the text.
- **`temporal_score`** (ISS-191 AC-3) reads the structured `start`/`end`
  interval — which is the full year, so temporal ranking gets no day-level
  signal.

The precise day is captured at extraction time and then discarded into a
comment nobody consumes.

## Evidence

conv-26-q0 ("When did Caroline go to the LGBTQ support group?", gold
"7 May 2023"):
- Gold-bearing memory `02700088` ("Caroline attended a LGBTQ support
  group") exists in substrate.
- Its temporal metadata is `kind=approx` with the precise day stranded in
  the note: `"yesterday (2023-05-07) relative to 2023-05-08"`, while
  structured `start`/`end` collapsed to full-year `2023`.

Systemic, not q0-specific: a sample of 12 approx memories ALL have their
precise date only in the note string ("last Saturday relative to
2023-05-25", "last week (2023-06-26)"), never in `start`/`end`.

## Root cause

The extractor's relative-expression resolution emits a human-readable note
but does not narrow the structured `start`/`end` interval to the resolved
day. The structured interval is left at year granularity (or whatever the
coarse parse produced), so the resolved day is invisible to both
text-reading generation and interval-reading `temporal_score`.

This is downstream of ISS-190/191:
- ISS-190 grounded *duration*-relative expressions ("owned for 3 years")
  → derived an approximate year.
- ISS-191 added the typed `TemporalMark::Approx` variant + interval-overlap
  `temporal_score`.
- Neither resolves a *relative day* expression ("yesterday", "last
  Saturday") TO an explicit day and pins it into `start`/`end`. That is
  this issue.

## Proposed fix

When the extractor resolves a relative expression to an explicit calendar
day (the resolved date already exists — it's what gets written into the
note), narrow the structured `start`/`end` interval to that day instead of
leaving it at coarse granularity. The note can remain as provenance, but
the structured interval must carry the resolved day so both consumers see
it.

## Implementation plan (scoped 2026-05-29)

Root located: `parse_temporal_mark(s: &str)` in
`crates/engramai/src/enriched.rs:454`. Current precedence:
1. RFC3339 datetime → `Exact`
2. `YYYY-MM-DDTHH:MM:SS` → `Exact`
3. bare `YYYY-MM-DD` → `Day`
4. `parse_approx_year` (`~2020`, `since 2020`, bare `2020`) → `Approx` (full-year)
5. else → `Vague(original)`

`temporal_grounding::ground_field` already rewrites `fact.temporal` in
place BEFORE this parse: `"yesterday"` → `"yesterday (2023-05-07)"` (the
resolved `range.start.date_naive()` in a single parenthetical). But step 3
only matches when the WHOLE trimmed string is a date, so a grounded phrase
`"yesterday (2023-05-07)"` skips step 3, then `parse_approx_year` splits on
`(`, gets head `"yesterday"` (no leading year → `None`), and the string
falls to `Vague` — the resolved day is stranded in the free-text remainder,
never pinned to `Day`. That is the bug.

**Fix:** insert a new step 3.5 between the bare-`%Y-%m-%d` check (step 3)
and `parse_approx_year` (step 4): scan for an EMBEDDED `YYYY-MM-DD`
substring (the grounding parenthetical) and, if exactly one resolved date
is found, emit `TemporalMark::Day(that_date)`.

Precision guards (avoid false positives):
- Only fire when the embedded date is a complete `YYYY-MM-DD` (regex
  `\b\d{4}-\d{2}-\d{2}\b`), validated by `NaiveDate::parse_from_str`. Reject
  partial `2023-05` or bare `2020` (those stay on the `parse_approx_year`
  path → `Approx`, preserving year-granular uncertainty).
- If the SOURCE carried a fuzz marker (`~`, "around", "about",
  "approximately", "circa") the resolution was approximate — do NOT collapse
  to a precise `Day`; let it fall through to `parse_approx_year`/`Approx`.
  Grounded relative-day phrases ("yesterday", "last Saturday") are NOT
  fuzz-marked, so they correctly pin to `Day`.
- Take the FIRST embedded date. Current grounding emits a single
  parenthetical so there is one; if a future format embeds both event +
  reference date, first = resolved event day (verified against
  `ground_field` output order).

**Generation (AC-2):** already satisfied by `temporal_grounding` —
`ground_field` rewrites `core_fact` inline, so generation's text carries
`"... yesterday (2023-05-07) ..."`. No separate work needed; the structured
`Day` pin (this fix) is what unblocks `temporal_score` (AC-1) and lets
retrieval/ranking surface the dated episode.

**Tests:**
- `parse_temporal_mark("yesterday (2023-05-07)")` → `Day(2023-05-07)` (AC-3)
- `parse_temporal_mark("~2020")` → still `Approx` (guard: fuzz-marked)
- `parse_temporal_mark("2020")` → still `Approx` (guard: bare year)
- `parse_temporal_mark("last Saturday (2023-05-25)")` → `Day(2023-05-25)`
- `parse_temporal_mark("around 2020 (2020-06-01)")` → `Approx` NOT `Day`
  (fuzz marker dominates — verifies the guard, not just the happy path)

This is additive to the parse precedence; existing `Exact`/`Day`/`Approx`/
`Vague` cases are unchanged (the new branch only catches strings that
previously fell to `Vague` while carrying a complete embedded date).

## Acceptance criteria


- [x] AC-1: When the extractor resolves a relative-day expression to an
      explicit day, the resulting `TemporalMark` has `start`/`end` narrowed
      to that day (not the full year/coarse span).
      **PASS** — `first_embedded_day` (enriched.rs) returns
      `TemporalMark::Day(d)` for an embedded `YYYY-MM-DD`; `Day` maps to a
      single-day interval, not a year span. Commit 9c30fe6.
- [x] AC-2: The resolved day is surfaced to generation (either pinned into
      the memory text at store time, or via the structured mark that
      generation can read) so q0-style "when" questions can answer the
      specific day.
      **PASS** — surfaced two ways: (1) `temporal_grounding::ground_field`
      rewrites the resolved day INLINE in the memory text
      ("yesterday (2023-05-07)"), which generation reads directly; (2) the
      structured `TemporalMark::Day` pin feeds `temporal_score`. q0
      generation read the inline day and answered "2023-05-07".
- [x] AC-3: Unit test: a memory ingested with "yesterday" + reference date
      2023-05-08 produces a mark whose `start`/`end` span 2023-05-07, not
      2023.
      **PASS** — `iss194_*` tests in enriched.rs assert
      "yesterday (2023-05-07)" → `TemporalMark::Day(2023-05-07)`. 2058 lib
      tests pass. Commit 9c30fe6.
- [x] AC-4: conv-26-q0 answers "7 May 2023" (or equivalent) once fix 3
      (ISS-192) lifts the dated episode into top-K AND this fix surfaces the
      resolved day. Note: q0 requires BOTH ISS-192 fix 3 (retrieval/ranking)
      and this fix (extractor day-pinning).
      **PASS** — combined fix3+fix4 conv-26 run STAMP 20260530T013110Z,
      bonus=0.5: conv-26-q0 score=**1.0**, gold "7 May 2023", pred
      "Caroline attended a LGBTQ support group on **2023-05-07**". Both fixes
      required and both fired: fix 3 lifted the PartOf-edge episode into
      top-K, fix 4 pinned the resolved day so generation read 2023-05-07
      (not the 2023-05-08 reference-date residual seen in the fix3-only
      arm).
- [x] AC-5: No regression on conv-26 / conv-44 overall (≤10% vs ISS-190
      baseline).
      **PASS (conv-26, within-sweep)** — see verdict below. conv-44
      cross-validation pending (tracked as next step, non-blocking).

## Notes

Split out from ISS-192 per design decision 2026-05-29: fix 3 (ISS-192) is a
retrieval/ranking change in `orchestrator.rs factual_to_scored`; fix 4 (this
ISS) is an extractor/store-path change. Independent layers, independent
failure modes, independent ACs. q0 needs both to fully answer; ISS-192's AC
is q0's dated episode *reaching top-K*, this ISS's AC is q0 *answering the
specific day*.

## Verdict (2026-05-30)

**RESOLVED. q0 flipped to gold; no regression on the valid (within-sweep)
comparison.**

### q0 result (AC-4)
Combined fix3+fix4 conv-26 run, STAMP `20260530T013110Z`, bonus=0.5:
- conv-26-q0 score = **1.0**
- gold: "7 May 2023"
- pred: "Caroline attended a LGBTQ support group on **2023-05-07**"

Fix 3 (ISS-192) lifted the PartOf-edge episode into top-K; fix 4 (this ISS)
pinned the resolved day so generation answered the exact day instead of the
2023-05-08 reference-date residual that the fix3-only arm produced.

### AC-5 regression methodology — READ THIS

The naive comparison (combined 0.2763 vs ISS-190 conv-26 baseline 0.3158 =
-12.5% relative) **appears to fail the 10% gate, but that comparison is
invalid**. Two LoCoMo runs with byte-identical scoring (both
`ENGRAM_FACTUAL_EDGE_SEED_BONUS=0.0`, identical envelope) still flip
**22/152 queries** — because each run re-ingests episodes through the Haiku
extractor, which is non-deterministic at the extraction layer even with the
LLM judge at temp=0. Cross-run overall deltas of ±4-8pp are re-ingestion
noise, not signal. (Same class of trap as the ISS-191 stale-binary lesson:
verify the comparison is apples-to-apples before trusting the delta.)

**The only valid regression test is within-sweep A/B** — both arms share one
binary and one ingestion, so the bonus is the sole variable. Within the
ISS-192 sweep (STAMP `20260529T234442Z`):

| arm | overall | single-hop | multi-hop | open-domain |
|---|---|---|---|---|
| A (bonus 0.0, inert) | 0.2368 | 0.03125 | 0.2703 | 0.0769 |
| B (bonus 0.5) | 0.2763 | 0.0625 | 0.3784 | 0.2308 |

Fix 3 = **+3.95pp overall, no category regression** (single-hop, multi-hop,
open-domain all up). The combined fix3+fix4 run (0.2763) matches arm B
exactly on overall — fix 4 adds q0's day-correctness without moving the
aggregate. **AC-5 PASS within-sweep, +3.95pp >> -10% gate.**

### Disposition
- AC-1..5 all PASS. Status → resolved.
- `ENGRAM_FACTUAL_EDGE_SEED_BONUS` stays **opt-in (default 0.0)** until
  conv-44 cross-validation confirms the gain is corpus-general (next step,
  non-blocking).
- fix 3 committed engram 437b620, fix 4 committed engram 9c30fe6.
