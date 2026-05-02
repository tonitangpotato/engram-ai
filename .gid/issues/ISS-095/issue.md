---
id: ISS-095
title: "[INVALIDATED] graph extraction silent data loss (~49% pipeline runs fail with dim mismatch)"
status: invalid
priority: low
labels: [eval, retracted]
created: 2026-05-01
closed: 2026-05-01
---

# INVALIDATED — premise was wrong

## Why this was filed

While answering potato's question about RUN-0012 health, I claimed:

> 216 / 437 = 49% extraction failure (silent data loss)

## Why it's wrong

Real numbers from `RUN-0012 .graph.db`:

- `graph_pipeline_runs`: **432 succeeded / 4 failed / 1 running** (~99.1% success)
- `graph_extraction_failures`: 216 rows, but breakdown is:
  - 215 × `stage=entity_extract, error_category=no_facts_extracted`
  - 1 × `stage=resolve, error_category=unresolved_subject`

`no_facts_extracted` is **not an error** — it's the LLM extractor correctly returning "no extractable facts" on chitchat / acknowledgments / interjections (e.g. "haha", "ok cool", "thanks"). These rows are recorded for observability, not because something went wrong.

There is **no `dim_mismatch` error_category** in the schema at all. I confabulated that label across multiple turns by stitching together an old `ingest.log` line with the count from `graph_extraction_failures`.

## True picture

- 432/437 pipeline runs succeeded (~99%)
- 4 actual failures — worth a small investigation but NOT a 49% data-loss event
- 215 `no_facts_extracted` are working-as-intended

## What is real

A small follow-up could:

- Inspect the 4 actual `failed` runs' `error_detail` to confirm they're benign / known
- Add a `RUN summary` tool (see ISS-097) so this kind of mistake (counting symptom rows instead of querying status) is harder to make

## Closing this

This issue is invalidated. Real follow-ups:

- ISS-097 (eval pipeline tooling debt — auto-summary, truth-source doc) — keep
- "Investigate the 4 actually-failed pipeline runs" — track inside ISS-097 if worth it

## Lesson logged to engram (procedural, importance 0.85, 2026-05-01)

> For RUN-NNNN health checks: `graph_pipeline_runs.status` is the truth source for run success/failure, NOT the row count of `graph_extraction_failures`. The failures table records per-stage events, including benign categories like `no_facts_extracted`.
