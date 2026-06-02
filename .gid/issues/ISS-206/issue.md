---
id: ISS-206
title: Resolved event dates are stranded in temporal metadata, absent from memory text — generator cannot answer "when" even when the dated episode is retrieved
status: verify-only
resolution: already-satisfied-by-ISS-190/191-surfacing
priority: P3
severity: retrieval-quality
tags: [retrieval, temporal, extraction, generation, date-stranding, locomo]
created: 2026-06-02
relates_to: [ISS-190, ISS-191, ISS-204, ISS-205, ISS-201, ISS-207]
depends_on: []
---

> **RE-SCOPED (2026-06-02): the date is NOT stranded — Option C is already
> live.** The original premise ("the generator reads memory `content`, not
> the temporal dimension") is contradicted by the running code.
> `engram-bench` `format_context_block` (drivers/locomo.rs:1063, 1135) builds
> each generator line as `[{when}] {content}` where `when =
> derived_temporal_value(record).or(occurred_at)`, and
> `derived_temporal_value` reads `metadata./engram/dimensions/temporal/value`.
> The production retrieval read path (`Storage::get_by_ids` →
> `row_to_record_from_node_impl`, storage.rs:9649) parses the `attributes`
> column into `MemoryRecord.metadata`, so the temporal dimension survives to
> the generator. Surfacing defaults **ON**
> (`temporal_surface_enabled`, locomo.rs:1001).
>
> **Probe proof** (`examples/iss206_date_surface_probe.rs`, forensic DB
> `.tmpK8lZyN`, gold node `a838a102`):
> ```
> content       : "Caroline attended a LGBTQ support group"
> metadata None?: false
> derived_temporal_value: Some("2023-05-07")
> generator line (surfacing ON):
>     [2023-05-07] Caroline attended a LGBTQ support group
> ```
> The gold episode's resolved date (`2023-05-07`, correct — `occurred_at` is
> the off-by-one conversation timestamp `2023-05-08`) **already surfaces** into
> the generator prompt. ISS-206's proposed Option C is implemented.
>
> **Therefore q0's residual blocker is RETRIEVAL DELIVERY, not date legibility:**
> the gold line is answerable *once it reaches the generator's top-K*. Getting
> it there is ISS-205 (reservation) + ISS-207 (hybrid factual sub-plan emits
> candidates in memory_id order, bypassing the reservation's graph-score
> privilege). This issue is downgraded to **verify-only**: no extractor /
> content-mutation work (Option A/B) is needed. Keep open only to (a) confirm
> the surfaced date persists through the *production* (non-bench) generation
> path if/when one exists, and (b) re-run AC-2 after ISS-207 lands to confirm
> q0 flips once delivery is fixed.

# ISS-206: dated episodes carry their date only in temporal metadata, not in the memory text the generator reads

> **One-line:** A memory like *"Caroline attended a LGBTQ support group"* has
> its resolved date (`2023-05-07`) living in structured temporal metadata
> (the `occurred_on` edge / temporal mark), but **not in the content string
> the answer-generation step reads**. So even when retrieval correctly places
> that episode in the top-K context, the generator cannot say *when* — and
> instead answers from a sibling episode that happens to carry its date
> in-text. This is the residual cause of conv-26 q0 after ISS-204 (date is a
> graph edge) and ISS-205 (dated episode survives top-K).

## How this was isolated

ISS-205's q0 fused-pool probe (STAMP `20260602T024240Z`, conv-26, R=5)
dumped the post-fusion candidate pool for `conv-26-q0`
(*"When did Caroline go to the LGBTQ support group?"*, gold `2023-05-07`):

- The gold episode **is in the pool** (re-hashed to `858fc792` this ingest;
  content *"Caroline attended a LGBTQ support group"*), at **rank 26/217**,
  with the pool's **highest `vector_score` (0.90)**.
- ISS-205's scoped reservation privilege lifts it into the top-10 by graph
  axis. So retrieval is fixed.
- But its `content_head` is literally *"Caroline attended a LGBTQ support
  group"* — **no date anywhere in the text**.
- The Arm-B prediction for q0 was: *"Caroline attended an LGBTQ+ counseling
  workshop on 2023-06-23 ... no mention of her attending an LGBTQ support
  group"*. The generator picked the **counseling-workshop** episode — which
  is the wrong event, but **carries `2023-06-23` in its content string** — so
  it could produce a confident dated answer. The gold, lacking an in-text
  date, was unusable for a "when" question even when present.

This is a clean separation of two defects:

1. **Ranking** (ISS-205, fixed): get the dated episode into the top-K.
2. **Date-surfacing** (this issue): make the resolved date *legible to the
   generator* by putting it in the text the generator reads.

ISS-205 alone cannot flip AC-3 (q0) precisely because of (2).

## Root cause

The extractor resolves relative/duration expressions to absolute dates
(ISS-190 reference-date grounding) and ISS-204 pins those dates onto
`occurred_on` graph edges with non-NULL `source_memory_id`. But the
**memory content string** that the generation step assembles into its prompt
is the episode's *semantic* text — it does not interpolate the resolved date.
The date lives in:

- the `occurred_on` edge object literal (graph layer), and/or
- the temporal mark interval (`TemporalMark`, ISS-191 AC-2/3),

neither of which is in the `content` field the answer LLM sees. The generator
reads memory `content`, not the graph edge or the temporal dimension.

This is the same shape as the 2026-05-29 q0 root-cause note: precise resolved
days were stranded in a free-text `note` field
(*"yesterday (2023-05-07) relative to 2023-05-08"*) while the structured
`start`/`end` collapsed to a useless full-year interval. ISS-191 AC-3 fixed
the *interval* (so `temporal_score` ranks correctly), but the **generator
path still reads only `content`**, which has no date.

## Why not "just widen retrieval / rely on temporal_score"

`temporal_score` (ISS-191 AC-3) helps *ranking* — it scores interval overlap
so the dated episode sorts well. It does nothing for *generation*: the LLM
that writes the final answer is handed the memory `content` strings, and if
none of the retrieved-and-relevant strings contain the date, the model
either declines or borrows a date from a wrong-but-dated sibling. The fix has
to make the date present in the material the generator consumes.

## Candidate fixes (to be designed — do NOT pick one yet)

This needs a design pass; sketching the option space so the trade-offs are
explicit.

### Option A — surface the date into memory content at store time

When the extractor resolves an event's date, append/embed a canonical date
token into the stored `content` (e.g. *"Caroline attended a LGBTQ support
group (2023-05-07)"*). The date becomes part of the text every downstream
consumer reads — retrieval BM25/embedding *and* generation.

- Pro: single source of truth; generator needs no change; symmetric with how
  the winning distractor already carries its date in-text.
- Con: mutates content (embedding shifts → re-ingest sensitivity, the
  ~22/152 dedup wobble); must be idempotent and not double-stamp; must not
  corrupt content for non-event memories.

### Option B — assemble date into the generation prompt from the temporal mark

Leave `content` clean; at generation time, for each retrieved memory, look up
its temporal mark / `occurred_on` edge and prepend a resolved-date annotation
to the prompt context (e.g. *"[date: 2023-05-07] Caroline attended ..."*).

- Pro: content stays canonical; no re-ingest sensitivity; date is only added
  where it exists.
- Con: couples the generation step to the graph/temporal layer; needs a
  batched edge/mark lookup for the top-K; the generator prompt format grows.

### Option C — store the date in a structured field the generator template reads

If the generation prompt is template-assembled (not raw content
concatenation), add a `date:` slot fed from the temporal mark, so the
template renders it when present.

- Pro: clean separation; no content mutation.
- Con: only works if the generation path is templated; engram-bench's LoCoMo
  driver controls prompt assembly, so this may be a bench-side change rather
  than an engramai change — need to confirm where the answer prompt is built.

**Decision pending:** which layer owns "make the resolved date legible" —
the extractor (A), the generation assembler (B/C), or both. Likely A is the
root fix (the date *is* part of what happened and belongs in the episode's
text), with B/C as the bench-side complement if the prompt assembler is the
real chokepoint.

## Acceptance criteria

- **AC-1** — A stored event memory whose date was resolved by the extractor
  carries that date in a form the generation step reads (content string or
  generation-prompt annotation, per the chosen design). Unit/integration test
  asserts the date is present in the generator's input for a dated episode.
- **AC-2** — conv-26 q0 flips 0→1 under the locked ISS-190 envelope **with
  ISS-205's reservation on** (the two fixes compose: ISS-205 gets the dated
  episode into top-K, ISS-206 makes its date legible). This is the AC that
  ISS-205's original AC-3 was mis-attributed to.
- **AC-3** — No content corruption: non-event memories and memories without a
  resolved date are byte-identical (no spurious date stamping). Re-ingest of
  the same corpus does not double-stamp dates (idempotent).
- **AC-4** — No regression: conv-26 + conv-44 overall and temporal/single-fact
  categories net non-negative vs the ISS-190 baseline; multi-hop within ±10%
  wobble.
- **AC-5** — Default gating (if Option A mutates content): ships behind a
  serde-defaulted knob (default off) until the A/B clears AC-2..4, matching
  ISS-139 / ISS-205 default-off discipline.

## Notes

- This is the **residual** cause of conv-26 q0 after ISS-204 (date as edge)
  and ISS-205 (dated episode survives top-K). The chain is:
  ISS-190 (resolve relative→absolute) → ISS-191 (structured temporal mark +
  interval `temporal_score`) → ISS-204 (date as first-class graph edge) →
  ISS-205 (dated episode survives top-K ranking) → **ISS-206 (date legible to
  the generator)**. All five are needed for q0 to flip.
- conv-44 q11 is a *different* residual: there both candidate dates surface
  in-text (06-11 and 06-13) and the generator foregrounds the wrong one — a
  disambiguation problem ISS-205's reservation should help by privileging the
  gold-edge episode. q11 does NOT depend on ISS-206. Confirm during ISS-205
  A/B whether q11 flips on ranking alone.
- Confirm where the LoCoMo answer prompt is assembled
  (`engram-bench/src/drivers/locomo.rs` generation step) before choosing B/C
  vs A — the chokepoint location decides whether this is an engramai-side or
  bench-side fix.

## probe5 confirmation (2026-06-02)

Re-confirmed on a fresh ingest (probe5, PID 67705, DB `.tmpK8lZyN`). The
gold episode `a838a102` *"Caroline attended a LGBTQ support group"* reached
the fused pool at **rank 6 of 217** (improved from rank-26 in the earlier
probe — vector strength alone carried it into top-10), `vector_score=0.90`,
`graph_score=0.2`, `score=0.53`. SQL on the final DB confirms the memory
content is literally *"Caroline attended a LGBTQ support group"* with **no
date substring**. The date `2023-05-07` exists only as an `occurred_on`
edge.

This strengthens the ISS-206 thesis: retrieval is **not** the q0 gate — the
gold episode is already at rank 6, inside any reasonable top-K. The gate is
purely date-legibility in the text the generator reads. ISS-205's reserved
privilege (graph_score → 0.7) would only move it from rank 6 to a few slots
higher; it cannot make the date appear in the content. ISS-206 is therefore
the **necessary** fix for conv-26 q0 and all "when did X" queries whose gold
episode carries its date only in structured metadata.
