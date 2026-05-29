---
id: ISS-190
title: Temporal grounding can't derive absolute dates from duration/relative expressions — occurred_at is never injected into the extraction prompt, two_timer fallback can't parse durations
status: open
priority: P1
severity: degradation
tags:
- temporal-grounding
- extraction
- root-cause
- locomo
- single-fact
created: 2026-05-29
relates_to: [ISS-189, ISS-088, ISS-024, ISS-179, ISS-148]
---

# ISS-190: Relative/duration time expressions never become absolute dates

> Split out of **ISS-189** (which fixed the *recall* defect — the gold
> evidence episode now reaches generation). The residual failure on
> conv-44-q29 was first mislabeled as a "generation refuses arithmetic"
> problem. That framing was **wrong**. Root cause is upstream and internal
> to engram: the temporal-grounding pipeline can't turn
> "owned for 3 years" + a reference date into "since ~2020", so the absolute
> year never enters the stored memory and never reaches retrieval or
> generation. The downstream LLM answers "I don't know" because the year
> genuinely is not in its context — not because it refuses to subtract.

## The canonical failure: conv-44-q29

- **Query:** "Which year did Audrey adopt the first three of her dogs?"
- **Gold:** `2020`
- **Category:** single-hop. **Score:** 0.0 (baseline AND post-ISS-189).
- **Post-fix prediction:** *"I don't know. The memories indicate Audrey has
  owned Pepper, Precious, and Panda for 3 years as of March 2023, but don't
  specify which year she first adopted them."*

The model has the duration and the reference, and still cannot produce 2020 —
because nothing in engram ever computed `2023 − 3` and stored it.

## Evidence chain (every link verified, not inferred)

### 1. The stored memory — time is split across two places, neither absolute

SQL against the persisted substrate (`.tmpOaiaHe/substrate.db`,
memory id `597d12e7`):

```
content     = Audrey has three pets named Pepper, Precious, and Panda
              that she has owned for 3 years
occurred_at = 1679922600.0          # = 2023-03-27 14:30 UTC  (reference date)
metadata.engram.dimensions.temporal = {"kind":"vague","value":"3 years (duration)"}
```

- `occurred_at` correctly carries the reference date (March 2023).
- The LLM extractor correctly identified the temporal dimension as a
  **duration** (`kind: vague`, `value: "3 years (duration)"`).
- But **nothing combined them.** `content` carries no absolute year; the
  duration sits inert in metadata.

### 2. String → TemporalMark is pure format-recognition — it never *derives*

`dimensions.rs:434` turns the extractor's `temporal` string into a typed
`TemporalMark` via `enriched::parse_temporal_mark` (`enriched.rs:454`):

```rust
pub fn parse_temporal_mark(s: &str) -> TemporalMark {
    // RFC3339?  -> Exact
    // YYYY-MM-DDTHH:MM:SS?  -> Exact
    // YYYY-MM-DD?  -> Day
    TemporalMark::Vague(s.to_string())   // everything else
}
```

This function **only recognizes already-absolute date formats.** Any natural
language — "yesterday", "last summer", **"3 years (duration)"** — falls
straight through to `Vague(s)`, stored verbatim with **zero derivation**.
It is not given the reference date and does no arithmetic. So q29's value
becomes `Vague("3 years (duration)")` and stops there.

This is the structurally-missing link: there is no stage in the pipeline that
holds *natural-language time expression* + *reference date* + *derivation
ability* at the same time. `parse_temporal_mark` has the string but is a
format parser (no reference, no derivation); the extractor LLM has language
understanding but no reference date (§3); two_timer has the reference but no
open-set language ability (below). Each stage is missing one of the three.

### 3. The Vague → TimeRange path uses two_timer (a rule library) — and it can't parse durations

`temporal_grounding.rs` (ISS-088) rewrites resolved relative-time phrases
inline so content carries absolute anchors. It detects phrases with a
**regex** (which includes an `N (days|weeks|months|years) ago` pattern), then
re-validates each match with `parse_dimension_time` → `two_timer::parse`.

Empirical probe (`examples/probe_twotimer.rs`, reference = 2023-03-27):

```
"3 years (duration)"   -> Err   (the value actually stored for q29)
"3 years"              -> Err
"3 years ago"          -> Err   (matches the regex, but two_timer rejects it)
"for 3 years"          -> Err
"owned for 3 years"    -> Err
"2 months ago"         -> Err
--- controls that DO parse ---
"yesterday"            -> Ok  2023-03-26
"last year"            -> Ok  2022 (full year)
```

Two findings:
- **two_timer cannot parse ANY duration or `N-units-ago` expression.** Not
  just "3 years (duration)" — the whole family is unsupported.
- The grounding regex matches `N years ago`, but two_timer rejects it on
  re-validation → the regex branch is effectively dead. A
  **regex/parser capability mismatch** internal bug, independent of q29.

### 4. The deeper root: the extractor LLM is never given the reference date

`extractor.rs:561` / `:731`:

```rust
let prompt = format!("{}{}", EXTRACTION_PROMPT, text);
```

The extraction call receives `EXTRACTION_PROMPT + text` **only**. No
`occurred_at`, no "this episode happened on DATE". So even though the
extractor is an LLM (Haiku) — fully capable of resolving "owned for 3 years"
to a year — it **cannot**, because it does not know what "now" is. It can
only copy the relative phrase through as a string, handing the unsolved
arithmetic to a downstream rule library that can't do it either.

Meanwhile the reference date **does exist** at the ingest boundary, per
episode: `locomo.rs:1053` calls
`ingest_with_stats_at(&episode.text, episode.occurred_at)` — one timestamp
per episode. The data is in the pipeline; it just never flows into the
extraction prompt.

## Why the rule-based approach is architecturally wrong (not just incomplete)

Temporal expression is an **open set** — "3 years", "a couple years back",
"since college", "过年那会儿". Matching an open set with enumerated regex /
two_timer patterns is a structural mismatch: every gap ("N years ago",
"a few years back", …) is a new patch. This is the patch-treadmill that
SOUL.md's "root fix, not patch / no technical debt" rule exists to prevent.

The component that *can* handle an open set — an LLM — is **already in the
pipeline** (the extractor). It is simply being denied the one input
(reference date) it needs to do the job.

## Root fix: reference-date-aware LLM temporal derivation at extraction time

### First-principles framing

The real defect is **not** "two_timer is too weak." It is that we used the
wrong time *model*: we compressed structured time (point / interval /
duration) into a single待解析 string, then asked a rule library to parse an
open set. Temporal expression is open-ended ("3 years", "a couple years
back", "since college", "过年那会儿"); enumerated regex/rules can never cover
it — that is the patch treadmill SOUL.md's root-fix rule forbids.

The pipeline splits the capability into three stages, each missing one
ingredient (see evidence §2): the extractor LLM has language understanding
but no reference date; `parse_temporal_mark` has neither (pure format match);
two_timer has the reference but no open-set language ability. **The fix is to
put all three in one place** — the extractor LLM call, which already runs and
is the only component that can handle an open set — by giving it the
reference date it is currently denied.

Resolve relative/duration expressions to absolute dates **where the semantic
understanding and the reference date are both available** — inside the
extractor LLM call, at storage time. This matches engram's existing ISS-088
philosophy ("compute absolute anchors at store time so content is
self-contained"); it only changes the *executor* of the derivation from a
weak rule library to the LLM that is already being invoked.

### Why store-time, not answer-time (the mem0 contrast)

mem0's LoCoMo eval does temporal conversion at **answer time** — its
ANSWER_PROMPT instructs the answering LLM to convert relative refs using
per-memory `timestamp:` prefixes ("convert 'two months ago' to 'March 2023'
based on the memory timestamp"). That works for mem0 because mem0 controls
the answer endpoint.

engram is a memory **substrate** consumed by arbitrary upper-layer agents.
It must not assume the consumer's answer prompt does temporal arithmetic —
otherwise the capability isn't engram's, and a benchmark that patched the
answer prompt would be measuring prompt engineering, not engram. Therefore
the derivation must happen at **store time**, so any agent that recalls the
memory gets the absolute date for free.

### What Zep/Graphiti does (first-hand, confirms this approach)

Verified from Graphiti's `graphiti_core/prompts/extract_edges.py` (Apache-2.0,
read this session):

- Injects `reference_time` (ISO 8601 UTC) into the extraction prompt —
  *"used to resolve relative time mentions"*.
- Instructs the LLM to resolve vague/relative expressions itself —
  *"Use REFERENCE_TIME to resolve vague or relative temporal expressions
  (e.g. 'last week')"*. **No rule library at all.**
- LLM outputs structured absolute time: `valid_at` / `invalid_at` as ISO 8601
  (bi-temporal), with explicit DATETIME RULES ("if only a year is mentioned,
  use January 1st"; ongoing facts → episode timestamp).
- Prefers the per-episode timestamp; `reference_time` is a fallback.
- Anti-hallucination guard: *"Leave both fields null if no explicit or
  resolvable time is stated."*

So two independent first-hand sources (mem0 answer-time, Zep extraction-time)
both use an LLM + reference date for temporal resolution. Our store-time
choice aligns with Zep, which is the system that treats temporality as a core
product concern.

### Where we DIVERGE from Zep (deliberately): keep the uncertainty

Zep forces every resolved time into a precise ISO 8601 timestamp
(`valid_at: 2020-01-01T00:00:00Z`). For an inherently vague input like
"owned for 3 years" that is **lying with a precise type** — dressing "~2020"
up as midnight Jan 1. engram is a *memory* substrate; vagueness is a real
property of memory ("I think I got the dogs about three years ago" *is*
fuzzy), and忠实保留模糊性 is the honest thing to do (SOUL.md: 不要简化问题).

Therefore engram's temporal dimension should carry an **uncertainty-preserving
structured value** — e.g. `{kind: interval, start: "~2020" (year-granular,
uncertain), end: ongoing, reference: 2023-03-27, raw: "3 years"}` — rather
than a fake-precise instant. This is the one place we are *more* correct than
Zep, not just copying it.

(mem0 ANSWER_PROMPT evidence is first-hand from its open-source eval repo;
Zep evidence is first-hand from `extract_edges.py`.)

### Implementation sketch

1. **Thread the reference date into extraction.** Add a `reference:
   Option<DateTime<Utc>>` parameter to `Extractor::extract`; the ingest path
   already holds `occurred_at` per episode and passes it down.
2. **Inject it into the prompt.** Replace `format!("{}{}", PROMPT, text)`
   with a prompt that states the reference and instructs absolute resolution:
   *"This episode occurred on {reference}. Resolve every relative or duration
   time expression to an absolute date/year based on this reference (e.g.
   'owned for 3 years' on 2023-03-27 → started ~2020). If no time reference
   is present, omit the temporal field — do NOT fabricate a date."*
3. **Output an uncertainty-preserving structured value.** Have the temporal
   dimension carry the derived absolute value *with its granularity and
   uncertainty* (e.g. year-granular `~2020`, `ongoing` end) alongside the
   original phrase (`raw`), so content/anchors are self-contained and the
   original wording is preserved for audit. Do NOT coerce vague inputs into
   fake-precise instants (the Zep divergence above).
4. **Demote two_timer to fallback** for the simple deixis it already handles
   ("yesterday"); it is no longer the primary path. Open-set understanding
   moves to the LLM.

### Cost / determinism — the honest tradeoffs

- **Near-zero marginal cost.** Extraction already makes one LLM call; this
  adds only a reference line + temporal instructions to the existing prompt.
  No new LLM call (unlike HyDE). No new latency budget.
- **Loses determinism.** Today grounding is deterministic + cacheable
  (two_timer ~50µs). LLM derivation is not; mitigate with temp=0 and tests.
- **Precision is bounded by the data.** "owned for 3 years" is inherently
  vague (`kind: vague`); the derived year is year-granular and may be ±1y on
  some questions. q29's gold "2020" is correct (3y before March 2023), but
  the fix must NOT claim day-precision. Document this; do not over-promise.

## Acceptance criteria

- [x] **AC-1** Reference date is threaded from the ingest path into the
      extraction prompt (per-episode `occurred_at`). Unit test: extractor
      receives the reference; prompt contains it.
      DONE (24c13ac): `MemoryExtractor::extract(text, reference)` + `reference_preamble()`;
      threaded via `store_raw` (meta.occurred_at) + backfill (record.occurred_at);
      bench `ingest_with_stats_at` already carries it. Test
      `iss190_reference_preamble_resolves_relative_time` asserts the prompt contains
      the date.
- [x] **AC-2** For the q29 topology — input "owned for 3 years" with
      reference 2023-03-27 — the stored temporal dimension (or grounded
      content) carries an absolute year ≈ 2020. Unit/integration test with a
      mocked/temp=0 extractor.
      DONE (68dfd43): `tests/iss190_temporal_grounding_e2e.rs` — mock extractor
      resolves the duration against the threaded reference; resolved year 2020
      survives into the persisted record. NOTE: `parse_temporal_mark("~2020")`
      → `Vague("~2020")` (year string preserved, not a structured calendar
      value). Sufficient for q0; structured form deferred to ISS-191.
- [x] **AC-3** Negative guard: input with NO time reference must NOT produce
      a fabricated date — temporal field omitted. Test case included.
      DONE (24c13ac): preamble instructs OMIT-not-fabricate;
      `iss190_reference_preamble_forbids_fabrication` + `..._absent_is_byte_identical_legacy`
      (None reference → empty preamble → byte-identical legacy prompt).
- [x] **AC-4** two_timer demoted to fallback; the dead regex/parser-mismatch
      branch in `temporal_grounding.rs` is either fixed or removed (no silent
      regex match that two_timer then rejects).
      DONE (949bce1): probe confirmed two_timer resolves days/weeks-ago but
      returns Err for months/years-ago. Removed `months?|years?` from the regex
      arm; LLM now owns multi-month/year derivation. 2 regression tests.
- [ ] **AC-5** A/B on conv-26 (locked envelope: K=10 temp=0 HyDE=off MMR=off
      entity_channel=off pipeline_pool=1). Target: duration/relative-temporal
      single-fact questions flip 0→1, regression rate ≤10%.
- [ ] **AC-6** Cross-validate on conv-44: q29 flips 0→1, overall ≥ baseline
      0.2439 (run `ISS189-fix-conv44-20260529T131853Z`).

## Risk / scope

- Touches the extraction prompt + `Extractor::extract` signature + ingest
  wiring + the String→TemporalMark path + temporal-grounding fallback
  demotion. Blast radius is the extraction/dimension path; retrieval/schema
  largely untouched.
- **Design-phase must-verify (schema reach):** can the existing
  `TemporalMark` (Exact/Range/Day/Vague) + `TimeRange` represent
  "~2020, ongoing, uncertain, year-granular"? Almost certainly NOT — `Range`
  needs two bounds and an open/ongoing end + an uncertainty marker has no
  home today. If the existing types can't carry it, split the
  **uncertainty-preserving structured temporal value** into a follow-up
  **ISS-191** (schema + metadata version bump + `temporal_score` interval
  support), and let ISS-190 land the minimal viable form first (inject
  reference + LLM derives a best-effort absolute year into the existing
  `Day`/`Range`) to capture q29's score without the full schema change.
- Changing the extraction prompt re-touches the same surface as ISS-161 L3
  (extractor prompt experiments) — coordinate so the two don't fight.
- **NOT in scope:** unifying bi-temporal between the memory layer and the
  graph-edge layer (`valid_from`/`valid_to` already exist on edges). That is
  an architecture-level decision; file separately, do not let temporal
  derivation balloon into a substrate refactor (karpathy guideline).

## Evidence artifacts

- `examples/probe_twotimer.rs` — two_timer all-Err probe (this session)
- `enriched.rs:454` (`parse_temporal_mark`) — format-only, no derivation
- substrate row `597d12e7` (`.tmpOaiaHe/substrate.db`) — split-time evidence
- `extractor.rs:561` / `:731` — prompt lacks reference
- `locomo.rs:1053` — per-episode `occurred_at` exists at ingest
- mem0 eval `ANSWER_PROMPT` (answer-time conversion, first-hand)
- Graphiti `extract_edges.py` (extraction-time LLM + reference_time, first-hand)
- Run `ISS189-fix-conv44-20260529T131853Z` — q29 prediction + 0.2439 overall

## Related

- **ISS-189** — fixed the recall half (incoming-edge traversal). Surfaced
  this once recall stopped being the blocker.
- **ISS-088** — original temporal-grounding design (store-time absolute
  anchors). This issue extends its philosophy to durations the rule library
  can't handle.
- **ISS-024** — dimensional read path; `TemporalMark::Vague` → `TimeRange`.
- **ISS-179** — conv-26 single-fact feasibility census.
- **ISS-148** — AC-5a single-fact ship gate.
