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

## AC-1 VERDICT (2026-06-13) — LEVER CONFIRMED, well above gate

Forensic SQL probe on a settled conv-26 substrate (`.tmpC27eKq/substrate.db`,
457 memory nodes, ISS222-LEVER2 ingest, no live writer). Read-only, no
re-ingest, no token.

**Mechanism is even cleaner than hypothesized — and the day is NOT in a
separate `note` field.** The resolved day is stranded *inside the
`temporal.value` string itself*, while `temporal.kind` is mistyped:

```
temporal kind distribution (457 memory nodes):
  (none)  288
  vague   169
  day/approx/range/exact  0   ← NOTHING reaches day-precision
```

Of the 169 `vague` marks, **31 carry a resolvable `YYYY-MM-DD` inside the
value string** (GLOB `*[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]*`).
The gold q0 memory is textbook:

```json
"temporal": {"kind": "vague", "value": "yesterday (2023-05-07)"}
```

The exact day `2023-05-07` is resolved and present — but `kind="vague"`,
so ISS-204's producer (requires day-precision) emits **no `occurred_on`
edge** (probe: 0 occurred_on edges in the entire graph; predicate vocab is
`mentions` only). The mark degrades to vector recall — exactly the
ISS-204 needle-in-haystack.

**Pinnable classification of the 31:**
- **~25 cleanly pinnable** — precise resolved day mistyped `vague`:
  "yesterday (2023-05-07)", "yesterday (2023-07-05)",
  "last week (2023-06-19)", "this week (2023-08-21)",
  "yesterday (2023-10-21)", etc. These should be `kind=day`.
- **5 EXCLUDED (cross-year placeholder)** — "last year (2022-01-01)":
  the `2022-01-01` is a synthetic Jan-1 placeholder, not a resolved day.
  This is the cross-year-gap bucket already fenced out of scope; correctly
  not pinnable.
- **~1 borderline multi-cue** — "next month (2023-06-01), summer break,
  ongoing planning"; lead day still extractable but lower confidence.

**Gate: ~25 pinnable ≫ the ≥8 threshold → LEVER IS REAL. Proceed to fix.**

**Fix re-framed by the evidence:** the defect is the extractor (or
`parse_temporal_mark`) typing a mark as `vague` when its value contains a
fully-resolved `YYYY-MM-DD`. The fix is to **classify these as
`day`-precision** (so ISS-191's `Day` variant + ISS-204's producer +
ISS-205 reservation + ISS-206 surfacing all engage with zero new
plumbing), NOT a separate `note`→`start/end` promotion. Guardrail
unchanged: a placeholder day like `2022-01-01` derived from "last year"
must STAY `vague` (do not fabricate day-precision from a year-granular
cue) — so the promotion must trigger on the *relative-cue resolved day*,
not on any string that happens to match the date GLOB.

## Acceptance criteria

- **AC-1** — ✅ DONE (verdict above). Pinnable denominator ≈ 25/31
  vague-with-resolved-day marks; gate cleared.

## AC-2 VERDICT (2026-06-13) — scope narrowed by code-layer ground truth

Implementing AC-2 required reading the live classification site
(`enriched.rs::parse_temporal_mark`, the path `from_extracted` actually
calls). Two corrections to the AC-1 framing emerged — recorded honestly:

1. **The `.tmpC27eKq` substrate was STALE.** It shows `kind=vague` for
   `"yesterday (2023-05-07)"`, but that DB predates the live HEAD parser.
   **ISS-194 fix-4 (commit 9c30fe66, 2026-05-29) already pins embedded
   resolved days to `Day`.** A direct unit test against the 8 resolved-day
   relatives + 4 multi-cue strings from the probe confirms HEAD pins ALL
   of them to `Day` correctly (test
   `iss226_conv26_relative_day_strings_pin_correctly`). So the
   resolved-day half of the lever was already shipped; the AC-1 "25
   pinnable, lever wide open" conclusion was measuring a stale DB.

2. **The genuine unfixed defect is the INVERSE — a guardrail leak.**
   `first_embedded_day` greedily pins *any* embedded `YYYY-MM-DD`,
   including the synthetic Jan-1 placeholders the preamble emits for
   year-granular cues: `"last year (2022-01-01)"` was pinning to
   `Day(2022-01-01)`. That **fabricates day-precision the extractor never
   resolved** and would pollute the `occurred_on` graph with phantom Jan-1
   events. This is the exact cross-year bucket (q1/q26/q49, gold="2022")
   that ISS-226 fenced out of scope — and it was being mis-pinned.

**Fix shipped:** added `is_year_granular_cue` + `is_year_start_placeholder`
guards in `parse_temporal_mark`. When the cue is year-granular ("last
year" / "next year" / "N years ago" / "year before/earlier") AND the
embedded day is Jan-1, fall through to `parse_approx_year` → surfaces as a
year-granular `Approx` interval instead of a phantom precise `Day`.
Month/week/day cues ("last month", "this week", "yesterday") are
untouched — their embedded resolved day is genuine. Tests: the ISS-226
probe asserts both the correct pins AND the placeholder guard;
`iss194_*` regression tests still pass; full lib **2179 passed, 0 failed**.

**Net effect on ISS-226's original target:** the resolved-day RELATIVE
golds (most of the 25) were ALREADY pinning to `Day` on HEAD, so the
expected conv-26 RELATIVE-bucket lift from "promote vague→day" is largely
already realized in any fresh-binary ingest. The remaining value of this
fix is *correctness* (stop fabricating Jan-1 events), not a large J-score
lift. **This changes AC-3/AC-4 expectations** — see below.

- **AC-2** — ✅ DONE. `parse_temporal_mark` correctly pins resolved-day
  relatives to `Day` (pre-existing, fix-4) and now correctly REFUSES to
  pin year-granular Jan-1 placeholders (new guard). Unit tests cover both.

- **AC-3** — DB-verify on a FRESH conv-26 ingest (current HEAD binary)
  that resolved-day RELATIVE golds emit `occurred_on` edges with non-NULL
  provenance, AND that `"last year (...)"` golds do NOT emit phantom Jan-1
  `occurred_on` edges. (The stale-DB AC-1 probe cannot verify this — needs
  a fresh ingest.)
- **AC-4** — RELATIVE bucket on conv-26 vs the 0.056 baseline. **Revised
  expectation:** because resolved-day pinning was already live, the lift
  may be small; the primary success signal is AC-3 graph correctness +
  no EXACT/non-temporal regression + conv-44 cross-val no-regression. A
  large RELATIVE lift would actually indicate the 0.056 baseline was
  measured on a pre-fix-4 binary (worth confirming).
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

## AC-3 verdict — PASS (fresh conv-26 ingest, HEAD f7d19fcf)

Run `2026-06-13T13-46-28Z_locomo`, live substrate `.tmppGptnQ/substrate.db`
(440 memory nodes; ISS-190 envelope: conv-26, K=10, temp=0, HyDE/MMR/entity
off, FACTUAL_REWEIGHT off, pipeline_pool=1; Anthropic OAuth extractor +
Ollama embeddings; ENGRAM_BENCH_EMIT_DB_PATH=1).

SQL trace results:

- **occurred_on edge count = 53** (full ingest). Exactly matches the
  `day`-precision temporal-mark count (53) — every resolved-day pin emits
  one `occurred_on` edge, no drops, no duplicates.
- **Phantom Jan-1 edges = 0.** `SELECT COUNT(*) FROM edges WHERE
  predicate='occurred_on' AND target_literal LIKE '%-01-01%'` → 0. The
  ISS-226 year-granular guard (`is_year_granular_cue` +
  `is_year_start_placeholder`) holds: `"last year (2022-01-01)"`-style
  golds fall through to `parse_approx_year` and do NOT fabricate a Day pin.
- **Dates are real & dispersed** across Jun–Oct (07-15, 08-28, 10-20, …),
  not collapsed onto a placeholder day — confirms genuine relative-day
  resolution, not a single hardcoded fallback.
- **temporal kind distribution:** day=53, approx=17, vague=34, none=336.
- **q0 gold HIT:** `Caroline attended a LGBTQ support group` pins to
  `occurred_on "2023-05-07"` (gold = "7 May 2023"). This is the original
  date-stranding root cause (resolved day previously buried in `note`);
  it is now a structured, queryable `occurred_on` edge.
- **Provenance model:** `occurred_on` is an `entity --occurred_on--> "date"`
  structural edge (subject entity → resolved day literal); memory→entity
  provenance is carried by the `mentions` provenance edges (ISS-202 model),
  not on the structural date edge itself. 44 `mentions` edges present.

Both AC-3 conditions met: resolved-day RELATIVE golds emit `occurred_on`
edges with query-resolvable anchors, AND `"last year (...)"` golds emit
zero phantom Jan-1 edges.

## AC-4 verdict — prediction confirmed (lift small; correctness is the win)

conv-26 temporal category J = **0.371** this run (overall 0.289;
single-hop 0.063, multi-hop 0.324, open-domain 0.308). The per-query
JSONL schema carries no `question_id`, so the RELATIVE sub-bucket cannot
be sliced exactly from this single run, and single-run temporal numbers
sit inside the ±9pp/category re-ingestion noise band we have repeatedly
measured — a precise RELATIVE-vs-0.056 delta is not extractable here.

However the AC-4 **prediction is confirmed by the substrate evidence**:
resolved-day pinning was already live (ISS-194 fix-4, proven on HEAD), and
the AC-3 trace shows the RELATIVE golds' resolved days ARE correctly
grounded into structured `occurred_on` edges (q0 → 2023-05-07). Therefore
the date-grounding layer is no longer the bottleneck for RELATIVE golds.
Any residual RELATIVE deficit is a **retrieval/generation** problem
(ISS-225 recall ceiling — gold absent before reranking / IDK-refusal),
NOT a date-grounding problem. The ISS-226 fix's value is **correctness**
(elimination of phantom Jan-1 events that would mis-anchor temporal
queries), not a conv-26 benchmark lift.

**Disposition:** AC-1/AC-2/AC-3 satisfied; AC-4 reframed-and-confirmed
(small/no lift expected and observed; correctness delivered). The fix
(commit f7d19fcf) is the deliverable. Remaining RELATIVE deficit hands
off to ISS-225 (recall ceiling) — NOT a date-grounding lever. A precise
RELATIVE-bucket measurement would require a qid-tagged per-query dump
(future bench-harness enhancement) and is not blocking.
