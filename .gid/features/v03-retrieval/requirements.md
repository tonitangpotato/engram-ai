# Requirements: Engram v0.3 — Retrieval

> **Feature:** v03-retrieval
> **Parent:** `.gid/docs/requirements-v03.md` (master GUARDs apply)
> **GOAL prefix:** `GOAL-3`
> **Source:** DESIGN-v0.3.md §5 (read path), §6 (Knowledge Compiler L5 synthesis + consolidation tier effects), §2 (L4/L5 layer definitions, cross-ref only)

## Overview

The retrieval feature owns the **read path** — how queries reach the right memory layer and return results. It covers automatic query routing (structured entity queries → L4 graph, abstract/thematic queries → L5 Knowledge Topics), mood-congruent recall biasing, bi-temporal "as-of" queries, hot/warm/cold tier formalization, Knowledge Topic synthesis interactions, and explicit failure modes for all query types. It does NOT own the static data model (v03-graph-layer), the write-path pipeline (v03-resolution), or consolidation's write-side effects on edge versioning (v03-resolution retro-evolution).

## Priority Levels

- **P0**: Core — required for retrieval to function at all
- **P1**: Important — needed for production-quality retrieval
- **P2**: Enhancement — improves retrieval quality, observability, or flexibility

## Goals

### Query Routing & Classification

- **GOAL-3.1** [P0]: The retrieval API automatically classifies incoming queries by intent (factual, abstract, episodic, affective, hybrid) and routes to the appropriate layer(s) without the caller specifying which retrieval path to use. A caller issuing a plain-text query receives results from the correct layer without manual path selection. Routing correctness is validated on a labeled benchmark set of ≥ 50 queries spanning the intent categories; routing accuracy ≥ 90% on that benchmark is required. *(ref: DESIGN §5.1 query classification + §5.2 dual-level retrieval)*

- **GOAL-3.2** [P1]: Query classification is available as both a cheap heuristic path (no LLM call) and an LLM-assisted fallback. The heuristic path is attempted first; the LLM fallback fires only when the heuristic is uncertain. The classification method used is observable in the retrieval trace. *(ref: DESIGN §5.1 "heuristic first, LLM fallback only when heuristic is unsure")*

### Structured Entity Queries

- **GOAL-3.3** [P0]: Structured factual queries ("what is X's role?", "is Y still married to Z?") return answers grounded in the semantic graph, with per-fact provenance (traceable to source episode) and bi-temporal validity (when the fact became true, when it stopped being true, when the system learned it). *(ref: DESIGN §5.2 factual routing + §1/G1)*

- **GOAL-3.4** [P0]: Entity queries respect bi-temporal validity: an "as-of-T" query returns the edge state that was valid at time T (facts valid before T whose invalidation, if any, occurred after T), not the current state — unless the caller explicitly requests current state. *(ref: DESIGN §5.4 temporal queries)*

- **GOAL-3.5** [P1]: Superseded edges (invalidated via bi-temporal supersession per GUARD-3) remain queryable. The historical record is accessible at read time; superseded edges are filtered out of default results but retrievable via an explicit opt-in flag or temporal query. *(ref: DESIGN §5.4 + §1/INV3 no retroactive silent rewrites)*

### Abstract & Thematic Queries

- **GOAL-3.6** [P0]: Abstract and thematic queries ("what has the user been working on?", "summarize our work on Y") return Knowledge Topics from L5 with source-memory traces — each topic result links back to the consolidated memories and graph entities that contributed to it. *(ref: DESIGN §5.2 abstract routing + §3.6 Knowledge Topic L5)*

- **GOAL-3.7** [P1]: L5 Knowledge Topics are synthesized from consolidated memories and graph entity clusters. Affect-weighted clustering is applied such that entities the agent had stronger affective engagement with cluster more readily than affectively neutral entities. The affect-weighting strength is a tunable parameter (the specific tuning knob is a design decision). *(ref: DESIGN §6 step 6 Knowledge Compiler + §10 Q9 leaning option b)*

### Mood-Congruent Recall

- **GOAL-3.8** [P0]: The active cognitive self-state biases retrieval toward memories whose write-time affect snapshot is similar to the agent's current self-state. This is observable as a metric-defined difference: for a fixed query set of size ≥ 20, the top-K rankings produced under two self-states differing by ≥ 0.5 on the valence axis exhibit Kendall-tau correlation < 0.9. The exact metric and threshold may be tuned during implementation, but the requirement is that a quantitative ranking-difference metric is computed and surfaces a non-trivial difference. *(ref: DESIGN §5.3 affect-driven recall, mood-congruent in Bower 1981 sense)*

### Memory Tiers

- **GOAL-3.9** [P0]: Hot, warm, and cold memory tiers are exposed as a formal API surface, mapped onto the existing Working (high short-term trace strength), Core (high long-term trace strength with recent activation), and Archived classifications. Each tier is queryable independently. *(ref: DESIGN §1/G4 + §7.1 tier API)*

### Retrieval Failure & Transparency

- **GOAL-3.10** [P1]: Retrieval failure modes are explicit and distinguishable by the caller: "no entity found" vs "entity found but no edges match the query" vs "query is ambiguous (multiple interpretations)" are returned as distinct, typed outcomes — not collapsed into an empty result set. *(ref: DESIGN §1/INV1 never silent degrade, applied to read path)*

- **GOAL-3.11** [P1]: Retrieval is traceable: an explain/trace mode returns which layer(s) answered, what candidates each stage produced, how scores were composed, and which cognitive signals modulated ranking. This mode is opt-in; standard queries do not pay the trace overhead. *(ref: DESIGN §7.1 explain_recall)*

### Schema Evolution & Cross-Cutting

- **GOAL-3.12** [P1]: Novel predicates (those not in the canonical predicate set) are retrievable through the same query interfaces as canonical predicates, enabling schema-evolution review — callers can discover what novel relations the system has encountered. *(ref: DESIGN §3.5 Proposed(String) predicate variant)*

- **GOAL-3.13** [P2]: L5 topic synthesis cost is observable separately from other LLM usage. The number of LLM calls consumed by Knowledge Compiler L5 synthesis is counted and reported independently from write-path (Stage 3/4/5) resolution calls. *(ref: DESIGN §6 step 6 Knowledge Compiler + §1/G3 LLM cost measurement)*

- **GOAL-3.14** [P1]: Retrieval never blocks results based on cognitive state. Cognitive self-state (affect, telemetry, empathy, metacognition) modulates result ranking but never prevents results from being returned. This instantiates GUARD-6 for the read path. *(ref: DESIGN §3.7 boundary rule #4 + GUARD-6)*

## Guards

All guards are defined in the master document (`.gid/docs/requirements-v03.md`). The following guards are particularly relevant to this feature:

- **GUARD-1** [hard]: Episodic completeness — retrieval depends on L1/L2 always being written.
- **GUARD-2** [hard]: Never silent degrade — retrieval failures must be surfaced, not swallowed.
- **GUARD-3** [hard]: Bi-temporal invalidation never erases — superseded edges remain queryable (GOAL-3.5).
- **GUARD-6** [hard]: Cognitive state never gates — read path modulates ranking, never blocks results (GOAL-3.14).
- **GUARD-8** [hard]: Episode affect snapshots are immutable — mood-congruent recall reads write-time snapshots, never recomputes.

## Out of Scope

- **Static data model** (Entity, Edge, Predicate schema) — owned by v03-graph-layer
- **Write-path pipeline** (extraction, entity/edge resolution) — owned by v03-resolution
- **Consolidation write-side effects** (retro-evolution, edge versioning) — owned by v03-resolution
- **Content-congruent recall** (inferring affect from query text rather than agent self-state) — deferred to v0.4 per DESIGN §5.3
- **Schema induction** (clustering Proposed predicates into canonical variants) — deferred to v0.4 per DESIGN §3.5
- **Query-text affect extraction** — would require a separate extraction step; not in v0.3

## Dependencies

- **v03-graph-layer** — retrieval reads Entity, Edge, and Predicate structures defined there
- **v03-resolution** — retrieval depends on the write path having populated L4/L5 data
- **engramai v0.2 Knowledge Compiler** — L5 topic synthesis extends the existing Compiler; retrieval queries its output
- **engramai v0.2 cognitive state model** — mood-congruent recall reads `AffectState` and `SomaticFingerprint` (§3.7)
- **SQLite (rusqlite)** — existing dependency; retrieval queries new tables (`entities`, `edges`, `episodes`)

---

**14 GOALs** (6 P0 / 6 P1 / 2 P2) — references master doc for **12 GUARDs** (10 hard / 2 soft)
