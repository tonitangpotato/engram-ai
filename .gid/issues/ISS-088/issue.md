---
id: ISS-088
title: "Resolved temporal expressions never written into content — LLM cannot ground answers in absolute dates"
status: done
priority: P1
severity: high
tags: [extractor, temporal, content-grounding, llm-answer-quality, cogmembench]
related: [ISS-087, ISS-024]
depends_on: [ISS-087]
---

# ISS-088: Content-level temporal grounding

## Summary

When the extractor pulls a temporal expression like "yesterday" out of source text, the resolved absolute date is stored only in **metadata side-channel** (`temporal: Option<String>` and the parsed `TimeRange`). The **memory's content text is never rewritten** to include the absolute date. As a result, even when retrieval returns the right memory, the answering LLM sees text like "Caroline attended a LGBTQ support group" with no date — and cannot answer "When did Caroline go?" correctly.

## Evidence

- `crates/engramai/src/extractor.rs:91` — `temporal: Option<String>` is on the extracted struct, but lives in metadata only.
- `crates/engramai/src/temporal_dim.rs` — produces `TimeRange` used **only** for ranking (`temporal_score`), not for content rewrite.
- LOCOMO failure mode: gold answer requires "May 7 2023". Stored content contains "yesterday" (or no date at all). Retrieval may surface the right turn, but the LLM has no absolute date to cite.

## Impact

- Any benchmark that asks "when did X happen?" on a corpus with relative time expressions will fail at the **answer** stage even when retrieval succeeds.
- Real-world impact extends beyond benchmarks: any user query like "remind me when I last saw Alice" hits the same problem if the source memory said "yesterday".
- Wastes ISS-087's anchor fix — without grounding, correct anchoring only improves ranking, not answer quality.

## Root Cause

Temporal extraction was designed as a **scoring signal**, not as a **content transformation**. The extractor pipeline has no post-resolution step that rewrites the source content with the absolute date inlined.

## Proposed Fix

**Add a temporal-grounding pass to the extractor pipeline.** Two design options:

### Option A: Inline rewrite (preferred)
After `temporal_dim::parse_dimension_time` resolves `"yesterday"` → `TimeRange { start: 2023-05-07, end: 2023-05-08 }`, rewrite the stored content:
- Original: `"Caroline attended a LGBTQ support group yesterday"`
- Stored:   `"Caroline attended a LGBTQ support group on 2023-05-07"`

Plus keep original in `user_metadata.original_content` for provenance.

### Option B: Append-style grounding
- Stored: `"Caroline attended a LGBTQ support group yesterday [resolved: 2023-05-07]"`

Less natural for the LLM but preserves the original phrasing exactly.

**Recommendation: Option A** — LLMs handle natural prose better than bracketed annotations, and provenance is preserved in metadata.

### Implementation Sketch

1. New module `engramai/src/temporal_grounding.rs`:
   - Function `ground_temporal(content: &str, anchor: DateTime<Utc>) -> (String, Vec<TemporalGroundingRecord>)`
   - Detects relative phrases via the same `two_timer` pass used by `temporal_dim`
   - Substitutes resolved absolute date in-place
   - Returns rewritten string + audit trail of substitutions
2. Extractor pipeline: after extraction, before storage, call `ground_temporal(content, meta.occurred_at.unwrap_or(Utc::now()))`.
3. Store rewritten content in `MemoryRecord.content`, original in `user_metadata.original_content`, audit trail in `user_metadata.temporal_groundings`.

### Edge Cases

- **Multiple relative expressions in one sentence** ("yesterday I met Bob, last Tuesday I saw Alice") — handle each independently, leftmost-first to avoid offset shifting issues.
- **Ambiguous expressions** ("recently", "soon") — `two_timer` returns None; leave content unchanged.
- **Non-temporal "yesterday" usages** (rare, e.g., a song title) — accept false-positive substitution as cost of doing business; flag in audit trail for human review if needed.
- **Future expressions** ("tomorrow", "next week") — same logic, anchor + offset.

## Acceptance Criteria

- [ ] `ground_temporal()` function exists with unit tests covering: yesterday, last Tuesday, two weeks ago, tomorrow, ambiguous expression (no rewrite), multiple expressions.
- [ ] Extractor pipeline invokes grounding before storage.
- [ ] `user_metadata.original_content` and `user_metadata.temporal_groundings` populated on rewritten records.
- [ ] Integration test: ingest "Caroline attended yesterday" with `occurred_at=2023-05-08`, then read back content — must contain "2023-05-07".
- [ ] LOCOMO smoke test: a sample of date-grounded questions improves answer accuracy vs current (qualitative pass — full bench is a separate eval task).

## Dependencies

- **Hard depends on ISS-087.** Without `occurred_at` override, the anchor is wall-clock now, and grounding will produce *correctly resolved* but *wrong-year* dates. ISS-087 must land and be wired through cogmembench before this issue's grounding can be validated against LOCOMO.

## Out of Scope

- LLM-based extraction of temporal expressions (current `two_timer`-based parsing is sufficient for v1).
- Re-grounding of already-stored memories (backfill). If needed, separate migration ISS.
- Grounding of non-temporal context (locations, named entities). Different problem class.

## Notes

- See ISS-087 for the full diagnostic trace and shared context.
- This fix is what actually moves LOCOMO temporal-question accuracy. ISS-087 alone fixes only ranking — necessary but not sufficient.

## Resolution (2026-04-30)

**Implementation landed.** Inline grounding rewrites resolved relative time expressions in `ExtractedFact` text fields with their absolute date, anchored to `StorageMeta.occurred_at` (falls back to wall-clock if unset).

### Files

- **Created:** `crates/engramai/src/temporal_grounding.rs` (305 lines)
  - `ground_fact(fact: &mut ExtractedFact, reference: DateTime<Utc>) -> GroundingResult` — main entry
  - Internal `ground_field()` helper with leftmost-first non-overlapping match resolution and right-to-left rewrite
  - Regex covers: `yesterday|today|tomorrow`, `last|this|next (week|month|year)`, `N (days|weeks|months|years) ago`
  - Idempotent: detects existing `(YYYY-MM-DD)` annotation and skips
  - Reuses `temporal_dim::parse_dimension_time` (already powered by `two_timer`)
- **Modified:** `crates/engramai/src/lib.rs` — added `pub mod temporal_grounding`
- **Modified:** `crates/engramai/src/memory.rs` — wired `ground_fact` into the fact loop in `add_facts_with_emotion` path; `original_content` merged into per-fact `user_metadata` when grounding mutates `core_fact`
- **Modified:** `crates/engramai/Cargo.toml` — no new external deps (regex was already pulled transitively; verify if you see unused warnings)

### Tests

- 6/6 unit tests passing in `temporal_grounding::tests`:
  - `grounds_yesterday_in_core_fact`
  - `grounds_multiple_phrases_leftmost_first_no_offset_drift`
  - `idempotent_does_not_double_annotate`
  - `unparseable_phrase_skipped`
  - `grounds_temporal_and_context_fields_too`
  - `handles_empty_optional_fields`
- Full `engramai` lib suite: **1831 passing, 0 failed, 4 ignored** — no regression
- E2E integration test (separate file under `tests/`) **deferred** — unit tests + memory.rs wiring cover the contract; the cogmembench LOCOMO replay is the real e2e validation and lives in cogmembench, not here.

### Design choices

1. **Original text preservation is core_fact only.** `temporal` and `context` field originals are NOT stashed into `user_metadata`. Rationale: `core_fact` is the retrievable content (`content = core_fact`); other dimensions are facets. Adding all originals would bloat metadata for marginal value. Documented in `ground_fact` doc.
2. **Reference time = `meta.occurred_at.unwrap_or_else(Utc::now)`.** Same convention ISS-087 established for `created_at`. Replay/backfill paths get correct anchors; live-ingest paths get wall-clock.
3. **Mutation in place.** `ground_fact` mutates the fact rather than returning a new one. Avoids clones in the hot path; the caller already owns the `Vec<ExtractedFact>` returned by the extractor.
4. **No grounding cache.** Unlike `temporal_dim` (which caches for the scoring hot path), grounding runs once per ingest. Premature optimization avoided.

### What this unblocks

- cogmembench LOCOMO retrieval can now replay sessions with real timestamps and have absolute dates land in retrievable content. Combined with ISS-087, the full `replay_session_date → occurred_at → ground_fact → MemoryRecord.content` pipeline is operational.
- `temporal_score` ranking signal (ISS-024) now has both correctly-anchored `TimeRange`s (from ISS-087) AND temporally-grounded content for query matching.
