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

- [ ] **AC-1** Add a typed accessor on `MemoryRecord` (or a thin helper
      in the dimensions module) that returns the derived temporal mark
      without callers touching raw `metadata` JSON paths.
- [ ] **AC-2** Extend `TemporalMark`/`TimeRange` to represent an
      uncertainty-preserving year-granular / ongoing value (the "~2020,
      ongoing" case) as structured data, with a metadata version bump and
      round-trip serde tests.
- [ ] **AC-3** `temporal_score` interval support uses the structured form
      (don't regress existing exact/range scoring).
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
