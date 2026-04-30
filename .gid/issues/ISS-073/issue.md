---
id: ISS-073
title: Explain provenance of 215 entities + 144 edges in v0.3 graph.db (placeholder data audit)
status: resolved
priority: P2
filed: 2026-04-29
filed_by: rustclaw
labels:
- graph
- audit
- investigation
- v0.3
relates_to:
- ISS-072
- .gid/issues/ISS-072/issue.md
resolution:
- subsumed-by-ISS-072 — mystery solved during ISS-072 retraction; the '215 entities' are real LLM extraction output from RUN-0006-substrate
- not placeholders. No fix needed; the underlying bug (kind=other:unknown) is now tracked in ISS-072.
---

# Explain provenance of 215 entities + 144 edges in v0.3 graph.db

## TL;DR

`graph.db` currently contains 215 `graph_entities` rows and 144 `graph_edges` rows that did **not** come from any documented ingestion / extraction pipeline (see ISS-072: `graph_pipeline_runs` is empty, `PipelineKind` has no ingestion variant). We need to identify the code path that actually wrote these rows so we can decide whether to (a) keep them, (b) clean them up, or (c) treat them as a bug to fix.

This is an investigation issue, not a fix.

## Symptoms

- 215 rows in `graph_entities`
  - ~99% have `kind = unknown`
  - 29 duplicates of "Caroline"
  - Pattern inconsistent with LLM-extracted output (which would have typed kinds and dedup)
- 144 rows in `graph_edges`
- 0 rows in `graph_pipeline_runs` — so no documented pipeline produced this data
- 0 rows in v0.3 `graph_entities` table at the application level (per ISS-072 evidence)

The mismatch between "graph.db has 215 entities" and "v0.3 graph_entities table has 0" suggests these rows live in a different table or schema than the v0.3 ingestion path expects, OR they were written by a non-pipeline code path.

## Hypotheses to investigate

1. **Retrieval-time mirror.** A retrieval code path (e.g., resolver, FTS expansion, or a graph-anchor lookup) writes placeholder rows when it can't find a real entity, to keep references consistent.
2. **Schema-init seed.** A schema migration / init script seeds rows for testing or backwards-compat.
3. **v0.2 → v0.3 backfill artifact.** A migration step copied v0.2 entity-like data into v0.3 tables but bypassed the (nonexistent) ingestion pipeline.
4. **Test fixtures bleeding into the dev DB.** Test setup writes to the same DB and rows weren't cleaned.
5. **A debug / scaffold path** left in from earlier development.

The "Caroline" cluster (29 dupes) is a strong fingerprint — it points to a specific dataset (LoCoMo? a fixture?) and tracking that down should narrow this to one of the above.

## Investigation plan

1. `grep` for all writers of `graph_entities` / `graph_edges` in the codebase. Enumerate code paths.
2. For each writer, check whether it goes through `graph_pipeline_runs` instrumentation. The ones that don't are the suspects.
3. Identify which writer is responsible for the `unknown`-kind pattern and the "Caroline" dupes.
4. Determine: is this writer intentional (legitimate non-pipeline use case) or accidental (dev scaffold)?

## Acceptance

- Document identifies the exact code path(s) that wrote the 215 + 144 rows.
- Decision recorded: keep / clean up / fix.
- If "fix": file a follow-up ISS with a concrete patch.

## Why P2

Doesn't block anything. The placeholder data is a curiosity that helps validate ISS-072's diagnosis but doesn't need to be resolved before ISS-072 lands. After ISS-072 implements real ingestion-time extraction, we may want to wipe and rebuild graph.db anyway, which would moot this investigation.

## Related

- ISS-072 (P0): the real architectural gap — no ingestion-time extraction pipeline
- ISS-070 (P0): retrieval symptom, downstream of ISS-072
