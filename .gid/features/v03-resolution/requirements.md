# Requirements: Engram v0.3 — Resolution (Write-Path Pipeline)

> **Feature:** v03-resolution
> **Module prefix:** GOAL-2.X
> **Master doc:** `.gid/docs/requirements-v03.md` (GUARDs live there)
> **Design source:** `docs/DESIGN-v0.3.md` §4.1–§4.5, §6 (retro-evolution)

## Overview

The resolution feature owns the **write-path pipeline** — the sequence of stages that transforms an incoming Episode into entity and edge updates in the semantic graph (L4). It spans candidate retrieval, multi-signal fusion for entity matching, edge reconciliation, bi-temporal supersession, and retro-evolution of edges through later emotional context. The pipeline's core contract: every Episode that enters produces either a complete set of entity/edge writes or an explicit extraction failure — never partial graph state, never silent degradation.

This feature does NOT own: static data model definitions (v03-graph-layer), retrieval-time queries (v03-retrieval), or migration backfill (v03-migration — though backfill reuses this pipeline; see Out of Scope).

## Priority Levels

- **P0**: Core — required for the write path to function at all
- **P1**: Important — needed for production-quality resolution
- **P2**: Enhancement — improves efficiency, observability, or schema evolution

## Goals

### Pipeline Contract

- **GOAL-2.1** [P0]: Every Episode enters the resolution pipeline exactly once — duplicate processing of the same Episode is prevented, and no Episode is silently skipped. *(ref: DESIGN §4.1 "Overview" — pipeline flow from Episode creation through Stage 6)*

- **GOAL-2.2** [P0]: Entity/edge writes for a single Episode are atomic — either all extraction-derived graph updates for that Episode succeed, or the Episode is marked as extraction-failed with no partial graph state persisted. *(ref: DESIGN §4.5 "Extraction failure handling" — point 2, "entity/edge updates for that episode are skipped")*

- **GOAL-2.3** [P0]: When any stage of the pipeline fails (LLM error, timeout, rate limit), the failure is recorded as structured, queryable metadata on the affected Episode, including which stage failed and the error category. The Episode's L1 and L2 records are unaffected by the failure (cross-ref: GUARD-1, GUARD-2). *(ref: DESIGN §4.5 "On LLM failure during Stage 3/4/5" — points 1–4)*

- **GOAL-2.4** [P1]: Concurrent in-process resolution of Episodes (multiple resolver threads in the same process) with overlapping candidate entity sets produces a consistent graph state: every entity/edge written by each Episode is present and consistent, no partial writes are visible, and no duplicate entities are created from racing resolves of the same mention. Cross-process concurrency (two independent processes writing to the same database) is out of scope for v0.3 — external callers must serialize access. *(ref: DESIGN §4.3 "Stage 4" + §4.4 "Stage 5" + DESIGN §1/NG1 single-node deployment)*

### Candidate Retrieval

- **GOAL-2.5** [P1]: For each mention extracted from an Episode, candidate retrieval returns a bounded set of existing entities ranked by relevance, drawn from the current graph state. The candidate set size is configurable with a finite upper bound. *(ref: DESIGN §4.3 "Stage 4 — entity resolution via multi-signal fusion" — candidate retrieval step)*

### Multi-Signal Fusion

- **GOAL-2.6** [P0]: Entity resolution produces a single numeric confidence score per candidate by combining multiple independent signals — including semantic similarity, name matching, recency, co-occurrence, affective continuity, and somatic similarity — into a weighted fusion. The individual signal contributions are observable in the resolution output. *(ref: DESIGN §4.3 "Eight signals, fused into a single confidence score")*

- **GOAL-2.7** [P0]: For each mention, the fusion stage produces a final entity assignment (merge-into-existing-entity, create-new-entity, or defer-to-LLM-adjudication) paired with the confidence score that produced that assignment and an observable record of which decision path was taken. Callers can inspect, for any assignment, why that path was chosen. The specific threshold values and the number of decision paths are design/tuning decisions, not requirements — the requirement is that the decision and its justification are observable. *(ref: DESIGN §4.3 "Decision thresholds")*

- **GOAL-2.8** [P1]: The somatic similarity signal (comparison of the Episode's affective fingerprint against candidate entities' aggregate fingerprints) is included in fusion and its contribution is individually observable in the resolution trace. *(ref: DESIGN §4.3 "s8 — somatic_match" + §3.7 "Somatic fingerprint")*

### Edge Resolution

- **GOAL-2.9** [P0]: Edge resolution determines, for each extracted edge, whether to add a new edge, update (supersede) an existing edge, mark an existing edge as negated, or skip — by comparing the new edge against existing edges with the same subject and predicate. The decision is made cheaply when unambiguous, with LLM adjudication only for uncertain cases. *(ref: DESIGN §4.4 "Stage 5 — edge resolution (mem0-style)" — ADD/UPDATE/DELETE/NONE actions)*

- **GOAL-2.10** [P0]: Superseding an edge during resolution marks the old edge as invalidated with a timestamp and a reference to the new edge — it never deletes or overwrites the old edge (cross-ref: GUARD-3). *(ref: DESIGN §4.4 "UPDATE → invalidate old edge, create new" + §1/INV3)*

### LLM Cost Observability

- **GOAL-2.11** [P1]: The number of LLM calls made during each Episode's resolution is counted per-stage and exposed as observable per-episode metrics. Sustained average call counts are computable over a rolling window for comparison against cost targets (cross-ref: GUARD-12). *(ref: DESIGN §4.1 "Expected LLM calls per episode" + §1/G3 measurement spec)*

### Retro-Evolution

- **GOAL-2.12** [P1]: When a later Episode's context (including affective context) re-illuminates an earlier edge, the pipeline can produce a new version of that edge with provenance linking back to the triggering Episode. The original edge is preserved with invalidation metadata, not overwritten (cross-ref: GUARD-3). *(ref: DESIGN §6 "Retro-evolution (4) re-reads older edges through the lens of later episodes")*

### Schema Evolution

- **GOAL-2.13** [P2]: When no canonical predicate fits an extracted relationship, the pipeline emits a novel predicate proposal preserving the raw predicate text. The rate of novel predicate proposals is observable for schema evolution monitoring. Promotion of frequent novel predicates to canonical status is a governance activity performed between versions (manual review, proposed for a subsequent minor version); v0.3 does not perform automatic promotion — that is deferred to v0.4 per ISS-031. *(ref: DESIGN §3.5 "Proposed(String)" + "Schema inducer — deferred to v0.4")*

### Runtime Cost Budget

- **GOAL-2.14** [P1]: Under production workloads, the average number of LLM calls per episode measured over a rolling window of N ≥ 100 episodes stays ≤ 4. Sustained violation surfaces as a telemetry warning (instantiates soft GUARD-12 at runtime). The stricter ship-gate target (avg ≤ 3 over a fixed benchmark suite) lives in v03-benchmarks GOAL-5.4. *(ref: DESIGN §1/G3 + GUARD-12)*

## Guards

All cross-cutting GUARDs are defined in the master requirements document (`.gid/docs/requirements-v03.md`). This feature is constrained by:

- **GUARD-1** [hard]: Episodic completeness — L1/L2 writes succeed even when resolution fails
- **GUARD-2** [hard]: Never silent degrade — failures surface as visible data
- **GUARD-3** [hard]: Bi-temporal invalidation never erases — supersession produces new versions
- **GUARD-6** [hard]: Cognitive state never gates writes — affect annotates, never blocks
- **GUARD-8** [hard]: Episode affect snapshots are immutable after write — fusion reads snapshots, never recomputes
- **GUARD-9** [hard]: No new required external dependency
- **GUARD-12** [soft]: Per-episode LLM call count target (2–3 avg), violation triggers telemetry warning

## Out of Scope

- **Data model definitions** — Entity, Edge, Predicate structs and schema are owned by v03-graph-layer
- **Retrieval-time queries** — dual-level retrieval, mood-congruent recall, and query classification are owned by v03-retrieval
- **Migration backfill** — v0.2 → v0.3 backfill reuses this pipeline but is owned by v03-migration; this feature provides the pipeline, migration owns the orchestration
- **Automatic retry of failed extractions** — by design, retry is operator-driven (`reextract --failed`), not automatic *(ref: DESIGN §4.5 point 5)*
- **Schema induction (predicate clustering/promotion)** — deferred to v0.4 (ISS-031) *(ref: DESIGN §3.5 "Schema inducer — deferred")*
- **Fusion weight tuning** — initial weights are design-level guesses; tuning methodology is a §8 concern, not a resolution pipeline requirement
- **Consolidation-time audit of past merges** — owned by consolidation (§6 step 5), not the write path

## Dependencies

- **v03-graph-layer** — provides Entity, Edge, Predicate types and persistence; resolution writes through graph-layer APIs
- **Existing embedding pipeline** — candidate retrieval depends on vector similarity search over entity embeddings
- **Existing LLM abstraction (`engramai::llm`)** — extraction (Stage 3) and ambiguity resolution (Stages 4/5) require LLM calls
- **Episode + MemoryRecord persistence (Stages 1–2)** — resolution (Stages 3–6) runs after Episode/MemoryRecord admission; depends on those writes completing first

---

**14 GOALs** (7 P0 / 6 P1 / 1 P2) + **0 GUARDs** (all in master) — 7 referenced GUARDs from master
