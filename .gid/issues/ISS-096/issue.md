---
id: ISS-096
title: "v0.3 architectural debt: memory layer is row-oriented + graph as augment, not graph-as-substrate"
status: open
priority: medium
labels: [architecture, v0.3, design-debt, discussion]
created: 2026-05-01
relates_to: [ISS-083, ISS-084]
---

# v0.3 architectural debt: graph-as-augment vs graph-as-substrate

## Context

Asked by potato 2026-05-01:

> "graph 是 substrate 唯一基底，我们做 v0.3 不是想做一层新的东西，而是让 memory 在 graph 里。所以我们 v0.2 的 engram 到底是存在什么样的东西里面？"

## Empirical state (RUN-0012 inspection)

The intent stated above is **not** what the implementation does. Today's layout:

- **`<run>.db` (main)**: `memories` table holds all memory rows with `content`, `metadata` (JSON), `entity_ids`, `episode_id`, etc. + `memory_embeddings` table for vectors. **441 rows** in RUN-0012. All retrieval primarily reads from here (FTS, embedding ANN, ACT-R).
- **`<run>.graph.db` (separate file)**: `graph_entities` (623), `graph_edges` (709), `graph_memory_entity_mentions` (1122 rows linking memory.id ↔ entity), `graph_pipeline_runs` (437), etc.

The graph DB is **side-by-side augment**, not substrate:

- Memories exist as independent rows even when no entities are extracted
- Graph entities/edges are **derived** from memories asynchronously by the pipeline
- Retrieval can query either layer; today most channels go through `memories` + embeddings, with graph signals fed in as side info (when present)

## Why this happened (hypothesis, needs validation by reading v0.3 design docs)

Likely chosen during v0.3 migration to:

- Preserve v0.2 ingest path (existing users / benchmarks keep working)
- Make rollback possible (graph layer can be deleted without losing memories)
- Avoid blocking v0.3 release on a full storage rewrite

Cost: the graph signal is structurally a second-class citizen. Retrieval bugs (ISS-083 / ISS-084) keep surfacing because the graph isn't on the primary read path; downstream components (cogmembench adapter, ISS-094) only know how to read flat memory rows.

## What "graph-as-substrate" would look like

- A memory IS a (subject_entity, predicate_edges, object_entities, temporal_dim, spatial_dim, confidence, ...) view assembled from graph nodes/edges, not a separate row
- "Recall" = a graph traversal query, vectors are an index over node embeddings (not a parallel store)
- No `memories` table — or it becomes a thin materialization cache for hot reads

## Decision needed (NOT in this issue — discuss separately)

This is a **multi-week architectural surgery**, not a quick fix. Filing this ISS only to:

1. Capture potato's intent on the record so it's not lost
2. Frame ISS-083 / ISS-084 / ISS-094 as symptoms of this deeper issue
3. Prevent us from "fixing" symptoms in ways that cement the two-layer split further

## Out of scope (here)

- Actual implementation plan
- Migration strategy from current two-layer
- Whether to do this in v0.3.x or wait for v0.4

## Acceptance (for closing)

This ISS closes when one of:

- A v0.4 design doc lands committing to graph-as-substrate
- A decision is made to keep two-layer permanently and the rationale is documented
- ISS-083/084/094 fixes naturally subsume this concern

## Linked

- ISS-083, ISS-084: retrieval bugs that may be symptoms of this
- ISS-094: cogmembench adapter dropping temporal dim — symptom of graph-as-augment treatment
- ISS-097: eval tooling debt that makes the two-layer split painful to reason about
