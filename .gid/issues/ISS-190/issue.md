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

### 2. The grounding path uses two_timer (a rule library) — and it can't parse durations

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

### 3. The deeper root: the extractor LLM is never given the reference date

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

(Note: the mem0 ANSWER_PROMPT evidence is first-hand from its open-source
eval repo. A Zep/Graphiti comparison is NOT yet verified — its paper PDF
didn't extract cleanly; do not cite Zep's approach until its source is read.)

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
3. **Output absolute + raw.** Have the temporal dimension carry the derived
   absolute value alongside the original phrase, so content/anchors are
   self-contained while the original wording is preserved for audit.
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

- [ ] **AC-1** Reference date is threaded from the ingest path into the
      extraction prompt (per-episode `occurred_at`). Unit test: extractor
      receives the reference; prompt contains it.
- [ ] **AC-2** For the q29 topology — input "owned for 3 years" with
      reference 2023-03-27 — the stored temporal dimension (or grounded
      content) carries an absolute year ≈ 2020. Unit/integration test with a
      mocked/temp=0 extractor.
- [ ] **AC-3** Negative guard: input with NO time reference must NOT produce
      a fabricated date — temporal field omitted. Test case included.
- [ ] **AC-4** two_timer demoted to fallback; the dead regex/parser-mismatch
      branch in `temporal_grounding.rs` is either fixed or removed (no silent
      regex match that two_timer then rejects).
- [ ] **AC-5** A/B on conv-26 (locked envelope: K=10 temp=0 HyDE=off MMR=off
      entity_channel=off pipeline_pool=1). Target: duration/relative-temporal
      single-fact questions flip 0→1, regression rate ≤10%.
- [ ] **AC-6** Cross-validate on conv-44: q29 flips 0→1, overall ≥ baseline
      0.2439 (run `ISS189-fix-conv44-20260529T131853Z`).

## Risk / scope

- Touches the extraction prompt + `Extractor::extract` signature + ingest
  wiring + temporal-grounding fallback demotion. Blast radius is the
  extraction path; retrieval/schema untouched.
- Changing the extraction prompt re-touches the same surface as ISS-161 L3
  (extractor prompt experiments) — coordinate so the two don't fight.
- The temporal-dimension shape change (absolute + raw) may need a metadata
  version bump; check `metadata.engram.version` handling.

## Evidence artifacts

- `examples/probe_twotimer.rs` — two_timer all-Err probe (this session)
- substrate row `597d12e7` (`.tmpOaiaHe/substrate.db`) — split-time evidence
- `extractor.rs:561` / `:731` — prompt lacks reference
- `locomo.rs:1053` — per-episode `occurred_at` exists at ingest
- mem0 eval `ANSWER_PROMPT` (answer-time conversion, first-hand)
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
