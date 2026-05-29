---
title: Extractor strands resolved day in temporal note string instead of pinning into start/end
status: open
priority: P1
severity: degradation
tags: [extractor, temporal, retrieval, locomo]
relates_to: [ISS-190, ISS-191, ISS-192]
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

## Acceptance criteria

- [ ] AC-1: When the extractor resolves a relative-day expression to an
      explicit day, the resulting `TemporalMark` has `start`/`end` narrowed
      to that day (not the full year/coarse span).
- [ ] AC-2: The resolved day is surfaced to generation (either pinned into
      the memory text at store time, or via the structured mark that
      generation can read) so q0-style "when" questions can answer the
      specific day.
- [ ] AC-3: Unit test: a memory ingested with "yesterday" + reference date
      2023-05-08 produces a mark whose `start`/`end` span 2023-05-07, not
      2023.
- [ ] AC-4: conv-26-q0 answers "7 May 2023" (or equivalent) once fix 3
      (ISS-192) lifts the dated episode into top-K AND this fix surfaces the
      resolved day. Note: q0 requires BOTH ISS-192 fix 3 (retrieval/ranking)
      and this fix (extractor day-pinning).
- [ ] AC-5: No regression on conv-26 / conv-44 overall (≤10% vs ISS-190
      baseline).

## Notes

Split out from ISS-192 per design decision 2026-05-29: fix 3 (ISS-192) is a
retrieval/ranking change in `orchestrator.rs factual_to_scored`; fix 4 (this
ISS) is an extractor/store-path change. Independent layers, independent
failure modes, independent ACs. q0 needs both to fully answer; ISS-192's AC
is q0's dated episode *reaching top-K*, this ISS's AC is q0 *answering the
specific day*.
