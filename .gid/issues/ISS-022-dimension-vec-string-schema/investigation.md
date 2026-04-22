# ISS-022: Dimensional Fields Schema — `Option<String>` → `Vec<String>`

**Status:** open (tech debt)
**Severity:** medium — no user-visible bug, but current schema loses information and blocks future structured retrieval features.
**Feature:** dimensional-extract
**Related:** ISS-019 (dimensional-metadata-write-gap), ISS-021 (subdim-extraction-coverage)
**Blocked by:** nothing — can be picked up independently.
**Blocks:** any future work that wants to query individual participants/relations/etc. as structured entities (e.g., richer dimension-aware retrieval in ISS-020+).

## TL;DR

The `ExtractedFact` dimension fields (`participants`, `temporal`, `causation`, `outcome`, `stance`, `sentiment`, `location`, `context`, `method`, `relations`) are typed as `Option<String>`, but LLMs naturally produce **lists** for most of them (e.g. `["Alice", "Bob"]` for participants). As a temporary fix for ISS-021, we added `deserialize_flexible_string` which **flattens arrays by joining with `", "`** into a single string. This works but is lossy:

- `["Alice", "Bob, Carol"]` → `"Alice, Bob, Carol"` — cannot distinguish 2 participants from 3
- Downstream filters can't match individual entities cleanly
- Prompt has to ask the LLM to "join with commas" which is unnatural and increases variance

The right schema is `Vec<String>`. This ticket tracks the refactor.

## Root Cause & History

Committed fix: `4b02bcc` — `fix(extractor): tolerate LLM dimension output variance (ISS-021)` on branch `wip/dimensional-recall-20260422`.

The reason the schema started as `Option<String>` was historical: early extraction targeted freeform prose descriptions ("how it was done"). As dimensions evolved to represent structured facts (who, when, why), list-shaped data became the norm but the type didn't follow.

ISS-021 smoke pilot revealed this: 10/100 records failed JSON parse because the LLM emitted `[]`/`null`/`["Alice"]` where a `String` was expected. Storage coverage dropped 94% → 74%. The custom deserializer restored it to 84%, but the underlying representation is still wrong.

See `src/extractor.rs:10-80` for the deserializer and `src/extractor.rs:83-127` for the `ExtractedFact` struct with the provisional attributes.

## Scope

### In scope

1. Change `ExtractedFact.{participants, temporal, causation, outcome, stance, sentiment, location, context, method, relations}` from `Option<String>` to `Vec<String>`.
2. Update the extraction prompt to instruct the LLM to emit arrays directly (no more "join with commas" language).
3. Update the write path (`src/memory.rs` around the `merge_enriched_into` / dimension persistence sites) to store `Vec<String>` in `memory.metadata.dimensions.*` as JSON arrays.
4. Update the read path in Knowledge Compiler / recall / scoring to consume arrays instead of splitting strings on `", "`.
5. Write a migration: on read, any legacy scalar-string values in existing databases should be split back into `Vec<String>` on the boundary (or a one-shot backfill writes them as arrays). Decide which at design time.
6. Remove `deserialize_flexible_string` and the associated 7 unit tests. Keep narrower tolerance only if there's a real use case (e.g., accept bare string for backward compat in a 1-line `From<String>` helper, no custom deserializer).
7. Update `ExtractedFact`, `LegacyExtractedFact`, and any intermediate wrapper types to stay in sync.

### Out of scope

- Changing the extractor prompt structure beyond what's needed for array output.
- Touching non-dimension fields (`core_fact`, `importance`, `confidence`, `valence`, `tags` is already `Vec<String>`).
- Retraining / re-ingesting LoCoMo data — this is a schema refactor, not a coverage re-measurement. A re-run pilot is nice-to-have for verification but not a deliverable.
- ISS-021's remaining 16% coverage gap — separate concern, attributed to LLM sampling noise.

## Affected Code

Grep markers to locate work:

- `src/extractor.rs`
  - L10-80: `deserialize_flexible_string` (to remove)
  - L83-127: `ExtractedFact` struct (10 fields to change)
  - L158-176: `LegacyExtractedFact` — check alignment
  - L841 area: "LLM returns list — we join with ', '" comment + associated code
  - Prompt strings: search for "join" / "comma" to find the instructions given to the LLM
- `src/memory.rs` — `merge_enriched_into` and dimension persistence (grep for `.dimensions.` or `participants`)
- Knowledge Compiler (`src/synthesis/`, `src/knowledge/`) — any site that parses dimension strings
- Recall/scoring paths that use `participants` / `causation` etc.

## Acceptance Criteria

- [ ] `ExtractedFact` dimension fields are `Vec<String>` (no `Option<String>` dimensions remain).
- [ ] `deserialize_flexible_string` is removed from `src/extractor.rs` (or explicitly justified as retained with a 1-line scope).
- [ ] Extractor prompt emits arrays directly; no "join with commas" instruction.
- [ ] New or updated tests: happy path (array), empty array → empty Vec, legacy scalar string → single-element Vec (migration path).
- [ ] Write path stores `Vec<String>` as JSON arrays in `memory.metadata.dimensions.*`.
- [ ] Read path (KC + recall + scoring) consumes arrays natively — no `.split(", ")` remaining.
- [ ] Migration strategy decided and implemented: either (a) boundary-read splits legacy strings transparently, or (b) one-shot backfill re-writes existing rows.
- [ ] All lib tests pass (current count: 961, expected to stay ≥ that).
- [ ] Optional verification: re-run ISS-021 smoke pilot, confirm no regression in coverage (≥ 84% storage, ≥ 30% causation).

## Risk & Complexity

**Low-medium risk.** Pure internal schema; no public API breakage (assuming `ExtractedFact` is not exposed through a stable API — verify).

**Touches:** extractor, write path, Knowledge Compiler read path, possibly LSP/scoring. ~300–600 LOC across 4–6 files.

**Migration:** the only non-trivial piece. Existing production databases will have scalar strings in `memory.metadata.dimensions.participants` etc. Choose:
- **(a) Lazy migration** on read — simpler, no downtime, but every read site must handle both shapes until a future cleanup.
- **(b) One-shot backfill** — cleaner end state, requires a versioned schema bump and a backfill job.

Recommend (a) with a clear comment + a follow-up issue to eventually run a one-shot normalization.

## Links

- Commit introducing the provisional fix: `4b02bcc` on `wip/dimensional-recall-20260422`
- `.gid/issues/ISS-019-dimensional-metadata-write-gap/PIVOT-NOTE.md` — context for why dimensions matter
- `.gid/issues/ISS-021-subdim-extraction-coverage/pilot/` — pilot data that exposed the bug
