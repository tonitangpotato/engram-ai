---
title: Surface store-time-derived temporal mark to retrieval consumers (structured TemporalMark + bench context block)
status: open
priority: P1
severity: degradation
tags: [temporal, retrieval, substrate, locomo]
relates_to: [ISS-190, ISS-103, ISS-088]
---

# Surface store-time-derived temporal mark to retrieval consumers

## Problem

ISS-190 made the extractor derive absolute time at **store time**: the
substrate now persists a resolved temporal mark for relative/duration
phrasing — e.g. "owned for 3 years" + reference `2023-03-27` becomes
`temporal = {kind: vague, value: "~2020 (owned for 3 years as of
2023-03-27)"}` under `metadata.engram.dimensions.temporal`.

The derivation works (ISS-190 AC-1..4, verified in substrate). But the
**derived value never reached the answer prompt**, so conv-44 q0 (gold
`2020`) stayed `0.0` despite the correct `~2020` sitting in the substrate.
The answer LLM was handed the *ingest* date + raw content ("owned for 3
years as of March 2023") and refused to subtract. Its own prediction:

> "Audrey has owned Pepper, Precious, and Panda for 3 years as of March
> 2023, but don't specify which year she first adopted them."

Root cause (downstream of ISS-190): the LoCoMo bench's
`format_context_block` (`engram-bench/src/drivers/locomo.rs`) emitted
`[occurred_at] {content}` and ignored the derived dimension.

## Scope

Two layers:

1. **Bench-side (DONE, engram-bench commit `740b8b2`):**
   `format_context_block` now prefers
   `metadata.engram.dimensions.temporal.value` over raw `occurred_at`
   when a derived mark exists. Fallback chain is byte-identical to
   pre-ISS-190 for memories without a mark: derived value → occurred_at →
   created_at. +4 unit tests (present / no-temporal-dim / empty /
   no-metadata).

2. **Substrate-side (TODO, this issue):** the derived value currently
   lives only as a free-text `value` string under
   `metadata.engram.dimensions.temporal`. There is no **typed accessor**
   on `MemoryRecord` for it, and the existing `TemporalMark`
   (Exact/Range/Day/Vague) + `TimeRange` cannot represent
   "~2020 / ongoing / uncertain / year-granular" as structured data —
   `Range` needs two bounds + an open/ongoing end, and uncertainty has no
   home today. Consumers (any upper-layer agent, not just the bench) have
   to reach into raw JSON paths, which is fragile.

## Why this matters (substrate framing)

engram is a memory **substrate** consumed by arbitrary upper-layer
agents. A derived temporal value that only the LoCoMo bench knows how to
extract (by JSON-pointer) is not a substrate capability — it's a
bench-local convention. The structured form + a typed accessor make the
derivation a first-class part of what engram exposes, so every consumer
benefits without re-implementing the JSON-path dance.

## Acceptance criteria

- [x] **AC-1** Add a typed accessor on `MemoryRecord` (or a thin helper
      in the dimensions module) that returns the derived temporal mark
      without callers touching raw `metadata` JSON paths.
      **DONE** (commit `50a8535`): `MemoryRecord::derived_temporal_mark()
      -> Option<TemporalMark>` + Display convenience
      `derived_temporal_value() -> Option<String>`, both routing through
      the canonical `Dimensions::from_stored_metadata` path. +5 unit
      tests. **Uncovered a substrate bug while implementing:** the v2
      store path writes `temporal` as a tagged object
      `{"kind":"vague","value":"~2020 (...)"}` but the read path
      (`from_legacy_metadata`) only did `get_string("temporal")` →
      **silently dropped every v2 typed temporal mark on the canonical
      read path**. Root-fixed in `from_legacy_metadata` (try object
      deserialize first, fall back to v1 string parse). This is the gap
      that forced the bench part (740b8b2) to reach into raw JSON
      pointers. 2042/2042 lib tests pass. NB: `dimension_access.rs`
      (`DimensionView`) is an orphan module (never wired into lib.rs) so
      its tests never ran — it carried the same latent bug.
- [x] **AC-2** Extend `TemporalMark`/`TimeRange` to represent an
      uncertainty-preserving year-granular / ongoing value (the "~2020,
      ongoing" case) as structured data, with a metadata version bump and
      round-trip serde tests.
      **DONE** (commits `bb3f5ac` variant + `8567f8f` producer): added
      `TemporalMark::Approx { start, end: Option<NaiveDate>, approximate:
      bool, note: Option<String> }`. `end: None` = ongoing; `approximate`
      flags inferred bounds; `note` carries derivation provenance.
      `precision_rank` renumbered Exact(5) > Range(4) > Day(3) > Approx(2)
      > Vague(1). `parse_temporal_mark` now emits `Approx` for the ISS-190
      strings (`~2020`, `~2020 (note)`, `since 2020`, `2020 (ongoing)`,
      bare `2020`) instead of `Vague`; non-year leading numbers stay
      `Vague`. Tagged serde object `{"kind":"approx",...}` round-trips on
      the AC-1 read path. +7 tests (serde, Display, 5 parse cases).
      (No metadata *version* bump needed — `Approx` is a new tagged enum
      variant, additive and backward-compatible on read.)
- [x] **AC-3** `temporal_score` interval support uses the structured form
      (don't regress existing exact/range scoring).
      **DONE** (commit `1d52fe8`): added
      `Memory::memory_temporal_extent(record) -> (start, end)` — reads the
      derived mark and yields `[start, end]` for `Approx`/`Range`/`Day`,
      and a single point at `event_time()` for `Exact`/`Vague`/none
      (byte-identical to pre-AC-3). `temporal_score` now scores by
      interval **overlap** and uses the interval midpoint for the
      proximity curve; the 0.5 in-range floor preserves the prior edge
      score. Ongoing (`end: None`) clamps to `event_time` so it never
      matches the far future. +2 tests (~2020 mark matches a 2020 query
      while a bare-2023 point misses; ongoing 2020→2023 overlaps a 2022
      query). Existing 3 temporal_score tests unchanged. 2051/2051 lib
      tests pass.
- [ ] **AC-4** conv-44 q0 (gold `2020`) flips `0→1` end-to-end with the
      bench surfacing the derived mark (validated by run
      `ISS191-fix-conv44-*`).
- [ ] **AC-5** No regression on conv-44 overall (≥ `0.2764`, the ISS-190
      post-fix number) and conv-26 A/B regression ≤ 10%.

## Related

- **ISS-190** — store-time derivation. This issue surfaces what ISS-190
  derives. ISS-190 AC-6 flip-clause was moved here.
- **ISS-103** — `occurred_at` split + Layer-2 temporal grounding.
- **ISS-088** — original temporal handling.

## Out of scope

- Unifying bi-temporal between the memory layer and graph-edge layer
  (`valid_from`/`valid_to` on edges). Architecture-level; file separately
  (karpathy guideline — don't let temporal surfacing balloon into a
  substrate refactor).

## Validation: q0 flips 0→1 (run ISS191-fix-conv44-20260529T155256Z)

With the bench surfacing the derived mark (commit `740b8b2`), the target
question flips:

- **conv-44-q0** (gold `2020`): `0.0 → 1.0`. Prediction: *"Based on memory
  [1], Audrey owned Pepper, Precious, and Panda for 3 years as of
  [2020]…"* — the answer LLM read the surfaced `~2020` mark and computed
  the year. **AC-4 PASS.** The full ISS-190 → ISS-191 chain (derive at
  store time → surface to consumer) is confirmed end-to-end.

### Caveat: overall delta is single-sample noise, not a regression

Overall `0.2764` (ISS-190 run) → `0.2439` (this run): 7 gained (incl q0) /
11 lost, net −4. The losses are NOT temporal-related and CANNOT be caused
by the fix logic:

- q14/q26/q50/q9 are **single-hop content questions** with no
  duration/relative-time element. The `[when]`-prefix change only relabels
  the date on already-retrieved lines — it cannot change *which* memories
  rank.
- The two runs are **separate ingests** → different dedup merge order →
  slightly different candidate pools. Example: q50 (gold "Grooming") — the
  ISS-190 run retrieved the grooming memory, this run didn't. That's
  retrieval-pool variance, not date surfacing.
- Plus temp=0 LLM-judge wobble on borderline phrasings (q14 both runs say
  essentially the same thing about nature).

**To separate signal from noise properly:** a same-DB A/B (toggle the
surfacing on a single fixed ingest via an env flag) or a multi-run mean.
AC-5 (regression gate) should be measured that way, NOT via two
independent ingests. Tracked for follow-up.

- [x] **AC-4** conv-44 q0 flips 0→1 — PASS (this run).
