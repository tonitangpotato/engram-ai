---
id: ISS-226
title: 'Sub-direction A: pin RELATIVE-date golds (event-relative→day) into start/end so they produce occurred_on edges — closes the 53pp EXACT-vs-RELATIVE temporal gap'
status: open
priority: P1
severity: degradation
labels: [retrieval, temporal, extraction, date-grounding, locomo]
relates_to: [engram:ISS-225, engram:ISS-190, engram:ISS-191, engram:ISS-204, engram:ISS-205, engram:ISS-206, engram:ISS-201]
depends_on: [engram:ISS-225]
filed: 2026-06-13
filed_by: rustclaw
---

## TL;DR

This is sub-direction (A) of the ISS-225 recall-ceiling umbrella, and it
is **narrower than the umbrella's first framing**. The date chain
(ISS-190 → 191 → 204 → 205 → 206) is already landed: relative/duration
expressions resolve to absolute dates at extraction, day-precision dates
become traversable `occurred_on` graph edges, dated episodes survive
top-K, and the resolved date surfaces to the generator. q0 flips 0→1.

The single genuinely-unbuilt piece is the **RELATIVE-date bucket**. On
ISS-204's AC-4 conv-26 trace (run `ISS204-AC4-conv26-20260601T143034Z`,
DB-verified), date-bearing golds split into:

- **EXACT** (gold is an absolute date): 10/17 = **0.588**
- **RELATIVE** (gold is a relative-date phrase, e.g. "the week before
  9 June 2023"): 1/18 = **0.056**
- **GAP = 0.533**

RELATIVE flatlines at the noise floor **by design**: those memories have
no day-precision `temporal` mark, so the projection pipeline emits no
`occurred_on` edge (it requires a day-precision mark — see ISS-204
Component 1 + the `low_precision_mark_emits_no_occurred_on_edge` test).
The resolved day, when one exists, is stranded in the `approx` mark's
free-text `note` field while structured `start`/`end` collapse to a
full-year interval. Closing this gap is the highest-EV temporal lever
remaining.

## What is already landed (do NOT rebuild — verified 2026-06-13)

- **ISS-190** (resolved): extractor `reference_preamble()` injects
  per-episode `occurred_at`; Haiku resolves relative/duration → absolute
  date. `parse_temporal_mark("~2020")` → `Vague("~2020")` (year string
  preserved, not structured).
- **ISS-191** (resolved): structured `TemporalMark` (Exact/Range/Day/
  Approx/Vague) + interval `temporal_score`. `Approx{start,end:Option,...}`
  variant exists; `parse_approx_year` produces it for `~2020`/`since`/
  `ongoing`/bare-year.
- **ISS-204** (resolved, P0): event-time is a first-class graph edge.
  `Predicate::OccurredOn` + literal-object `DraftEdge` emitted at
  projection **only when a day-precision mark exists AND a triple anchor
  exists**. 60 edges on conv-26, 100% non-NULL provenance.
- **ISS-205** (resolved): dated episode survives top-K via scoped
  reservation.
- **ISS-206** (verify-only, already-satisfied): the resolved date
  surfaces to the generator — `format_context_block` renders
  `[{when}] {content}` where `when = derived_temporal_value()`. The date
  is NOT stranded from the generator; surfacing defaults ON.

So the chain works for **day-precision** golds. The gap is purely:
**relative-date golds never reach day-precision, so they never get an
edge, so they degrade to vector recall.**

## Root cause (the precise gap)

For a RELATIVE gold like "the week before 9 June 2023":

1. The extractor (ISS-190) may resolve the day, but for `approx`/relative
   phrasings the resolved day lands in the `TemporalMark` `note` field
   ("yesterday (2023-05-07) relative to 2023-05-08"), while structured
   `start`/`end` collapse to the **full-year interval** 2023-01-01..
   2023-12-31 (the 2026-05-29 q0 root-cause note + ISS-203 q35 camping
   finding both confirm this shape).
2. ISS-204's projection requires a **day-precision** mark to emit an
   `occurred_on` edge. A full-year `Approx` interval is not day-precision,
   so **no edge is emitted**.
3. With no `occurred_on` edge, the temporal query degrades to vector
   recall over all memories mentioning the salient entity (ISS-204's
   "needle in haystack"), and the RELATIVE bucket flatlines.

**The fix is upstream of the edge:** when the extractor has resolved a
precise day from a relative expression (the day is sitting in `note`),
pin it into the structured `start`/`end` so the mark becomes day-precision
— then ISS-204's existing producer emits the edge with zero new plumbing,
and ISS-205/206 carry it the rest of the way. This is exactly the
"resolve-day-into-interval" prerequisite ISS-204 explicitly deferred to
the ISS-190/191 track ("for `approx` golds the day must first be pinned
into start/end so the graph edge gets a usable interval, not a full-year
smear").

## Target query set (achievable-recall denominator)

- **In scope:** the 18 RELATIVE-bucket conv-26 golds from ISS-204 AC-4
  (1/18 today), plus the `approx`-stranded temporal failures called out in
  ISS-203 (q35 camping "two weekends ago from 2023-07-17", q33 mixed) and
  the date-stranded subset of the ISS-201 AC-4 16-query temporal family.
- **Explicitly EXCLUDED from the target metric (ungroundable / out of
  mechanism):**
  - **Cross-year coverage gap** — q1/q26/q49 (gold="2022"): off-handed
    prior-year facts with no day-precision event memory at all. ISS-204
    AC-4 confirmed 0/60 edges fall in 2022. These cannot be pinned (there
    is no resolvable day to pin). Out of scope.
  - **Crowding** — q0-style (Caroline carries 31 `occurred_on` siblings):
    a ranking/disambiguation problem owned by ISS-203, not date-pinning.
  - **Pure recall miss** — q58/q63/q76: gold never enters the pool;
    upstream retrieval, tracked under ISS-225 sub-directions (B)/(C).
  - **Ungroundable gold** — anything whose resolved day cannot be derived
    from the episode text + reference date (no `note`-resolved day to
    pin).

The achievable denominator is the RELATIVE golds for which the extractor
actually resolved a day into `note` but failed to promote it to
`start`/`end`. The first experiment must measure that denominator (how
many of the 18 have a `note`-resolved day) before claiming a target.

## First experiment (cheap, no re-ingest)

Before any code change or full bench: a **forensic DB probe** on an
existing conv-26 substrate, mirroring the ISS-204 AC-4 / ISS-206 probe
pattern:

1. For each of the 18 RELATIVE-bucket golds, locate the gold memory and
   read its `TemporalMark`: does `note` contain a resolved day while
   `start`/`end` are a full-year interval? (This sizes the achievable
   denominator — the "pinnable" subset.)
2. For the pinnable subset, confirm no `occurred_on` edge currently
   exists (SQL: `occurred_on` edges anchored on that memory's entity in
   the gold's year-month).

Decision gate:
- **≥ ~8/18 are pinnable** (note has a day, start/end is full-year) →
  the lever is real; proceed to implement the `note`→`start/end`
  promotion + re-run ISS-204's producer.
- **< ~4/18 are pinnable** → the resolved day isn't even in `note` for
  most; the gap is extraction (ISS-190 didn't resolve them), not
  promotion — re-scope toward extractor resolution, not interval pinning.

## Proposed fix (only if the probe confirms pinnable ≥ gate)

At the extractor / temporal-mark construction site (where `parse_approx_*`
builds the `Approx`/`Vague` mark): when the resolution carries a concrete
resolved day (present in `note`), set structured `start = end = that day`
(day-precision interval) instead of collapsing to the full year. Keep the
`note` for provenance. This promotes the mark to day-precision, which is
the exact precondition ISS-204's producer already checks — no change to
the edge producer, ISS-205 reservation, or ISS-206 surfacing.

Guardrails (mirror ISS-206/ISS-204 discipline):
- Idempotent: re-ingest does not double-pin or corrupt the interval.
- Only pin when the resolved day is unambiguous; genuinely-vague marks
  ("sometime in 2023") stay full-year — do NOT fabricate a day.
- Default-off serde knob until A/B clears the regression gate, matching
  ISS-139/ISS-205/ISS-206 default-off discipline.

## Acceptance criteria

- **AC-1** — Forensic probe sizes the pinnable denominator (how many of
  the 18 RELATIVE golds have a `note`-resolved day with full-year
  `start`/`end`). Written into this issue. (Gate decision happens here.)
- **AC-2** — If the lever is confirmed: temporal-mark construction
  promotes a `note`-resolved day into day-precision `start`/`end`; unit
  test asserts a relative expression with a resolved day yields a
  day-precision mark (not full-year), and a genuinely-vague expression
  stays full-year.
- **AC-3** — DB-verify (fresh conv-26 ingest) that the pinnable RELATIVE
  golds now emit `occurred_on` edges with non-NULL provenance and
  query-resolvable anchors (same trace shape as ISS-204 AC-4).
- **AC-4** — RELATIVE bucket lift on conv-26 under the locked ISS-190
  envelope, measured against the 0.056 baseline; EXACT bucket and
  non-temporal categories net non-negative; multi-hop within ±10% wobble.
  conv-44 cross-validation required (a conv-26-only lift is corpus-overfit
  per the ISS-201 K=30 lesson).
- **AC-5** — No content/interval corruption: day-precision and
  genuinely-vague marks unaffected; re-ingest idempotent.
- **AC-6** — `engram_recall` confirms no prior falsified attempt at
  `note`→`start/end` promotion before any bench run (process rule from
  ISS-225).

## Artifacts / evidence

- ISS-204 AC-4 verdict (EXACT 0.588 / RELATIVE 0.056 / GAP 0.533):
  run `ISS204-AC4-conv26-20260601T143034Z`, live DB `.tmpcYbhzb/substrate.db`.
- 2026-05-29 q0 root-cause note (resolved day stranded in `note`,
  start/end full-year) + ISS-203 q35 camping `approx` finding.
- ISS-206 surfacing proof (date legible to generator once day-precision):
  `examples/iss206_date_surface_probe.rs`.
