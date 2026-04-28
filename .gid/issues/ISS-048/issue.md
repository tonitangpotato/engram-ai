# ISS-048: EntityExtractor / EdgeExtractor architectural mismatch — pattern-based entities cannot keep up with LLM-extracted edge subjects/objects, causing 100% `unresolved_subject` failure on fresh-ingest

- **Status**: in_progress
- **Severity**: blocker (graph layer extracts zero entities/edges/mentions on any free-form ingest where named entities are not pre-configured — i.e. ~all real-world content)
- **Filed**: 2026-04-28
- **Discovered during**: ISS-047 verification on LoCoMo conv-26 fresh-ingest (commit `be35217`)
- **Related**: ISS-047 (failure-label allowlist — fixed; this issue is what showed up *after* the rollback was lifted), ISS-046 (graph DB wiring), ISS-021 (subdim coverage), v0.3 design §3.2 / §3.3

---

## Summary

The v0.3 resolution pipeline has two extraction stages with **fundamentally
different recall profiles**:

| Stage | Implementation | Recall on free-form text |
|---|---|---|
| §3.2 EntityExtract ([`stage_extract.rs`](../../crates/engramai/src/resolution/stage_extract.rs)) | Pattern-based: Aho-Corasick over user-configured `known_people + known_projects + known_technologies` + builtin tech names + regex over structural patterns (issue IDs, URLs, file paths) | **Zero** for any name not pre-listed |
| §3.3 EdgeExtract ([`stage_edge_extract.rs`](../../crates/engramai/src/resolution/stage_edge_extract.rs)) | LLM-backed (`TripleExtractor`, Anthropic in production) | **High** — extracts arbitrary subjects/objects from free text |

EdgeExtract emits triples with subject/object strings that **cannot exist
in `ctx.entity_drafts`** because EntityExtract has no mechanism to emit
them. Every such edge fails resolution with `unresolved_subject` (or
`unresolved_object`). With ISS-047 fixed, the per-edge failure rows are
now correctly persisted, but the graph still ends up with 0 entities, 0
edges, 0 mentions on any input that doesn't happen to mention pre-listed
projects or builtin tech names.

This is the **second layer of the same iceberg** ISS-047 surfaced: ISS-047
was about how failures *propagate*; ISS-048 is about why the failures are
universal in the first place.

---

## Reproduction

LoCoMo conv-26 fresh-ingest, after ISS-047 fix (commit `be35217`):

```
32 messages ingested
  pipeline runs:           32 / 32 succeeded (no rollback)
  entities extracted:      0
  edges extracted:         99   (LLM produced triples)
  edges persisted:         0
  mentions:                0
  failures recorded:       99 × { stage: "resolve", category: "unresolved_subject" }
  applied_deltas:          (entities=0, edges=0, mentions=0) for all 32
```

The pipeline does *not* error — it returns `Ok(())` because §3.4 records
each unresolved edge as a `StageFailure { stage: Resolve, kind:
"unresolved_subject" }`, and ISS-047's allowlist now accepts that label.
But there is nothing to persist except 99 failure rows.

---

## Root cause: design vs implementation gap

v0.3 design §3.2 explicitly contemplates two paths:

> "EntityExtract emits drafts via the existing v0.2 pattern catalog;
> additional drafts may be lifted from EdgeExtract triples when the
> subject/object is novel."

The first half is implemented (`stage_extract.rs`).
**The second half — back-filling entity drafts from edge triples — is not
implemented anywhere.** `resolve_edges` looks up subject/object names
against `ctx.entity_drafts` only, and treats a miss as an unresolved
failure. It never falls back to "create a draft on the fly from the edge
triple's subject/object string".

So the architecture was meant to be:

```
content ─┬─→ EntityExtract (patterns)              ─┐
         └─→ EdgeExtract  (LLM, triples)            ─┤→ resolve
                          │                          │
                          └─→ lift novel s/o → draft ┘     (NOT IMPLEMENTED)
```

What we shipped:

```
content ─┬─→ EntityExtract (patterns) ──────────────┐
         └─→ EdgeExtract  (LLM, triples)            ├→ resolve → unresolved_subject
                                                    │             (no fallback)
                                                    ┘
```

This is a missing-implementation bug, not a prompt regression — both
extractors are doing exactly what their code says.

---

## Why this wasn't caught earlier

1. **Tests use `extractor_with_people(&["Alice", "Bob"])`** — i.e., they
   pre-configure the names that show up in the test text. The test
   `drafts_one_per_mention_in_order` passes precisely because `Alice` and
   `Bob` are in `known_people`.
2. **ISS-047 masked it** — until last week, every LoCoMo fresh-ingest
   rolled back the entire pipeline due to the allowlist mismatch, so we
   never saw the "0 entities + 99 edge failures" pattern downstream of a
   successful (no-rollback) run. The first clean fresh-ingest run is
   what surfaced this.
3. **No integration test ingests free-form text** without pre-configuring
   `known_people`. The only end-to-end ingestion paths in CI use either
   curated fixtures with known entities or synthetic patterns that match
   builtin technology regex.

---

## Options

### Option A — LLM-fallback in EntityExtract (smallest behavioral delta)

Extend §3.2 to call the same `TripleExtractor` (or a sibling `EntityLlmExtractor`)
when the pattern scan produces zero or "obviously incomplete" results
(heuristic TBD). Aligns the two stages on the same recall floor.

- **Pros**: keeps `resolve_edges` invariant ("subjects/objects must
  resolve to a draft"); minimal changes downstream; symmetric with
  EdgeExtract.
- **Cons**: doubles LLM cost per ingest (one call for entities + one
  for edges); needs prompt + parser + schema work; adds another stage
  failure mode.

### Option B — Lift novel s/o into entity drafts during/before resolve (design's original intent)

After EdgeExtract runs, walk `ctx.extracted_triples`, and for every
subject/object string not already in `ctx.entity_drafts`, append a
synthesized `DraftEntity { canonical_name: <s/o>, kind: Unknown,
provenance: EdgeLift, … }`. Then run resolve as today.

- **Pros**: zero extra LLM calls; matches design §3.2's "additional
  drafts may be lifted from EdgeExtract triples" sentence; surfaces every
  named entity the LLM saw, even ones the patterns missed.
- **Cons**: introduces `EntityKind::Unknown` (or similar) drafts —
  needs a typing decision; novel-edge drafts have no `EntityType`
  classification beyond what we can infer from the predicate; risks
  flooding entity registry with low-quality drafts (mitigated by §3.4
  thresholds).

### Option C — Drop unresolved edges silently (status-quo with cosmetic fix)

Stop recording `unresolved_subject` as a failure; just discard the edge.
Equivalent to "edges only persist when both endpoints happen to be in
the pattern catalog".

- **Pros**: trivial fix.
- **Cons**: graph layer becomes ~useless on free-form ingest (the very
  use case v0.3 was built for). Rejected.

### Option D — Hybrid: B as default, A as opt-in for high-quality typing

Lift novel s/o into `EntityKind::Unknown` drafts (fast, free), and
*optionally* run an LLM entity-typing pass when high-confidence types
are needed for retrieval/affect routing.

- **Pros**: best recall + cost trade-off.
- **Cons**: most work; needs design §3.2 update.

**Recommendation**: Option B as the immediate root fix (low cost, matches
design intent, restores graph functionality on free-form ingest).
Option A or D as a follow-up tracked separately if entity typing turns
out to materially affect retrieval quality.

---

## Plan (Option B, root fix)

1. **Adapter**: extend `crate::resolution::adapters` with
   `draft_entity_from_triple_endpoint(name, kind=Unknown, occurred_at,
   affect, provenance=EdgeLift)`. Reuse normalization from
   `draft_entity_from_mention` so canonical-name forms align.
2. **Stage glue**: in `pipeline.rs` (or a new helper called between
   `extract_edges` and `resolve_edges`), iterate `ctx.extracted_triples`,
   compute the set of normalized s/o names not already present in
   `ctx.entity_drafts.canonical_name`, append synthesized drafts.
3. **EntityKind::Unknown**: ensure `EntityKind` has a variant suitable for
   "type unknown, lifted from edge". If not, add `EntityKind::Unknown`
   with serde + DB-storage support; check `graph::entity` tests.
4. **Resolve invariants**: re-confirm `resolve_edges` does not require
   typed entities — only resolved canonical_id. Add explicit assertion
   if needed.
5. **Tests**:
   - Unit: `pipeline_lifts_novel_edge_subjects_into_drafts`
   - Integration: ingest LoCoMo conv-26 fresh, assert
     `entities > 0 ∧ edges > 0 ∧ unresolved_subject failure count → 0`.
   - Regression: pre-configured `known_people` path still works (drafts
     are not duplicated when an entity is in both pattern hits and edge
     triples — dedup by normalized canonical_name).
6. **Stats / observability**: bump pipeline stats to record
   `entity_drafts_from_patterns` vs `entity_drafts_from_edges` so we
   can see the split in production.

---

## Out of scope (deferred)

- LLM-based entity *typing* for novel drafts (Option A / D) — file as
  ISS-049 if Option B's `EntityKind::Unknown` proves insufficient for
  downstream signal scoring or retrieval quality.
- Re-architecting EntityExtract to be LLM-first (would invalidate v0.2
  retrieval features that depend on the pattern catalog).

---

## Acceptance criteria

- Fresh-ingest of LoCoMo conv-26 produces `entities ≥ 32`, `edges ≥ 50`,
  `unresolved_subject` failures `< 10%` of triples.
- All 32 messages have non-zero `applied_deltas.entities` and
  `applied_deltas.edges`.
- No regression in existing pattern-based entity tests.
- Stats expose pattern-vs-edge-lifted draft counts.

---

## Implementation note (2026-04-28)

**Implemented (Option B, partial):**
- `draft_entity_from_triple_endpoint(name, occurred_at, affect_snapshot)` in
  `crates/engramai/src/resolution/adapters.rs` — produces a weak
  `EntityKind::other("unknown")` draft. Reuses the existing sanctioned
  `Other` constructor; **no new `Unknown` variant added** (one fewer
  serde/DB migration). Aliases carry the lowercase form to match
  `draft_entity_from_mention`'s dedup key.
- `lift_novel_endpoints(ctx)` in `crates/engramai/src/resolution/pipeline.rs`,
  hooked into both `process()` (live ingest) and `resolve_for_backfill()`
  (migration backfill) — between `extract_edges` and `resolve_entities`,
  per design plan step 2. Pure CPU, no IO, no LLM.
- 9 unit tests (7 adapter normalization/affect/dedup-parity in adapters.rs,
  2 pipeline-level lift behavior + dedup-against-pattern in pipeline.rs).
  `cargo test -p engramai resolution::pipeline` → 5/5 pass; full
  `resolution::` → 203/203 pass.

**Deferred (left open, this issue stays `in_progress`):**
- LoCoMo conv-26 fresh-ingest acceptance (≥32 entities, ≥50 edges,
  <10% unresolved subjects). Infrastructure overhead too heavy for
  overnight run; needs longer session.
- Stats split (`entity_drafts_from_patterns` vs `_from_edges`). Today
  `stats.entities_extracted` is rebumped after lift so it reflects the
  final draft count, but the per-source breakdown is not exposed.

**Status:** stays `in_progress` — code shipped, end-to-end retrieval
acceptance gate not yet verified.
