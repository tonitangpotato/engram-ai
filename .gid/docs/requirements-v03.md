# Requirements: Engram v0.3

> **Master requirements document** — project overview, feature index, and cross-cutting GUARDs.
> Per-feature GOALs live in `.gid/features/v03-*/requirements.md`.
> Source: `docs/DESIGN-v0.3.md` (1029-line design doc, 2026-04-23).

## Overview

Engram v0.3 is a major version upgrade of the `engramai` memory crate (currently v0.2.2) that adds a bi-temporal semantic graph layer (L4) and a knowledge-topic synthesis layer (L5) **on top of** the existing three-layer memory model (L1 Episode / L2 Working / L3 Core). The thesis is not "engram + graph bolted together" — it is **cognition writes on the structural substrate**: affect modulates entity merging, mood drives recall, consolidation re-evolves edges through later emotional context, and topics compile from what the system *cared about*. v0.3 preserves v0.2's cognitive signature (decay, ACT-R activation, Hebbian bonds, affect, consolidation, interoception, metacognition) while making each of those mechanisms interact with the new structural layers rather than run parallel to them.

v0.3 ships as an embedded single-node SQLite crate (no new required dependencies), provides a migration path from v0.2 databases, and targets per-episode LLM cost of **2–3 calls average** via multi-signal fusion (vs Graphiti's reported 5–10) without quality regression.

## Priority Levels

- **P0**: Core — required for v0.3 to function at all (no ship without these)
- **P1**: Important — needed for production-quality operation (may slip to v0.3.1)
- **P2**: Enhancement — improves efficiency, UX, or observability

## Guard Severity

- **hard**: Violation = system is broken, execution must stop / migration must abort / write must fail
- **soft**: Violation = degraded quality, should warn + surface via telemetry, can continue

## Feature Index

v0.3 is split into five features. Each has its own requirements doc with 8–15 GOALs.

- **v03-graph-layer** → `.gid/features/v03-graph-layer/requirements.md`
  Core data model: Entity, Edge, Predicate, MemoryRecord extensions, SomaticFingerprint, layer/source semantics, bi-temporal invalidation, extraction failure surfacing. Covers DESIGN §3 and §4.5.

- **v03-resolution** → `.gid/features/v03-resolution/requirements.md`
  The write-path pipeline: episode → candidate retrieval → multi-signal fusion (s1–s8) → entity/edge resolution → bi-temporal supersession. Covers DESIGN §4.1–§4.4.

- **v03-retrieval** → `.gid/features/v03-retrieval/requirements.md`
  Dual-level query API, automatic routing (entity query → graph, abstract query → L5 topics), mood-congruent recall, hot/warm/cold tier formalization, Knowledge Compiler L5 synthesis interactions. Covers DESIGN §5 and §6 (L5 + retro-evolution touchpoints).

- **v03-migration** → `.gid/features/v03-migration/requirements.md`
  v0.2 → v0.3 migration: pre-migration backup, schema additions, backfill of Entity/Edge from existing MemoryRecord content, rollout phasing, Knowledge Compiler topic reconciliation, rollback, v0.2 public API backward compatibility. Covers DESIGN §7.3, §8, and §9.

- **v03-benchmarks** → `.gid/features/v03-benchmarks/requirements.md`
  Ship-gate measurement harness: LOCOMO / LongMemEval recall gates, LLM cost gate, v0.2 test preservation gate, cognitive-feature integration regression, migration data-integrity gate. Covers DESIGN §11.

**Cross-feature note:** DESIGN §6 (consolidation) touches all four features — decay/Hebbian affect graph-layer state, retro-evolution is a resolution concern, L5 synthesis is a retrieval concern, schema touches migration. Each feature doc references the relevant §6 sub-bullets; there is no standalone "consolidation" feature.

## Guards

Cross-cutting invariants. These apply to ALL features — no feature may violate them, and every feature must respect them.

### System-wide invariants (from DESIGN §1)

- **GUARD-1** [hard]: Every ingested interaction completes L1 (Episode) and L2 (Working admit) writes, even when downstream L4/L5 stages fail. Losing the episodic trace is never an acceptable failure mode. *(ref: DESIGN §1/INV2 Episodic completeness)*
- **GUARD-2** [hard]: No pipeline stage degrades silently. When extraction, resolution, LLM call, or consolidation cannot complete, the failure is surfaced as visible data (log + metric + typed status on the affected record). No silent fallback to cheaper models, no silent result truncation, no "best effort" without explicit signal. *(ref: DESIGN §1/INV1 Never silent degrade)*
- **GUARD-3** [hard]: Bi-temporal invalidation never erases. Superseding an edge or fact marks the old version invalid with provenance + timestamp; it never overwrites or deletes without audit trail. Consolidation and retro-evolution produce new versions, not in-place rewrites. *(ref: DESIGN §1/INV3 No retroactive silent rewrites)*

### Module-boundary rules (from DESIGN §3.7)

- **GUARD-4** [hard]: Affect may read from Telemetry, but Telemetry never reads from Affect. The dependency direction telemetry → affect → interoceptive is one-directional; reverse edges are forbidden and enforced by `cargo-deny` workspace rules. *(ref: DESIGN §3.7 boundary rule #1 + #2)*
- **GUARD-5** [hard]: EmpathyState (perception of the other party) never flows into AffectState (agent's self-state) in v0.3. Emotional contagion is a v0.4+ question (Q8); v0.3 enforces full isolation. *(ref: DESIGN §3.7 boundary rule #3)*
- **GUARD-6** [hard]: Cognitive state (affect, telemetry, empathy, metacognition) never gates writes. Episodes and memory records are admitted regardless of current mood, load, or anomaly score. Cognitive state only *annotates* writes (via `Episode.affect_snapshot`) and *modulates* reads (via mood-congruent recall); it never blocks them. *(ref: DESIGN §3.7 boundary rule #4)*

### Somatic fingerprint stability

- **GUARD-7** [hard]: The 8-dimension `SomaticFingerprint` schema is stable. Index semantics (0=valence, 1=arousal, 2=confidence, 3=alignment, 4=operational_load, 5=cognitive_flow, 6=anomaly_arousal, 7=feedback_recent) MUST NOT be reordered or reassigned after v0.3.0 ships. Any change requires a breaking version bump and migration. *(ref: DESIGN §3.7 "Somatic fingerprint — locked semantics")*
- **GUARD-8** [hard]: `Episode.affect_snapshot` is captured at write time and is immutable thereafter. Consumers must not recompute it from current AffectState after the episode is written. Entity aggregates over episode snapshots may be recomputed on new mentions, but historical per-episode snapshots are append-only. *(ref: DESIGN §3.7 "Update cadence" — immutability rule)*

### Deployment & compatibility

- **GUARD-9** [hard]: No new required external dependency. v0.3 remains a single-node embedded SQLite crate. Optional dependencies are allowed but must degrade cleanly to a baseline mode when absent. *(ref: DESIGN §1/G6)*
- **GUARD-10** [hard]: A v0.2 database survives migration to v0.3 without data loss. A pre-migration backup is written before any schema change, and migration aborts if the backup cannot be written. Rollback to v0.2 is possible from the backup. *(ref: DESIGN §1/G7 + §8 migration)*
- **GUARD-11** [soft]: v0.3 public API is a superset of v0.2 where possible. When a v0.2 API signature changes (e.g., due to `MemoryRecord` extensions), the crate provides a deprecation shim for at least one minor version. *(ref: DESIGN §1/NG5 "MemoryRecord is extended, not replaced" + §8 migration phasing)*

### Cost & performance

- **GUARD-12** [soft]: Per-episode LLM call count is measured and exposed via `write_stats.rs`. Average over a rolling window of N≥100 episodes is expected to be 2–3; violation (sustained >4) triggers a telemetry warning. *(ref: DESIGN §1/G3)*

## Out of Scope

- Distributed / multi-node deployment (NG1)
- General-purpose graph database features — the graph exists to serve memory only (NG2)
- LLM-free operation — ambiguity *requires* LLM judgment; v0.3 minimizes calls, does not eliminate them (NG3)
- Agent-managed-only curation (Letta model) — v0.3 defaults to automatic, agent tools optional (NG4)
- Replacing v0.2 `MemoryRecord` — extended with provenance + reliability fields, not replaced (NG5)
- Multi-tenant isolation beyond v0.2's existing ACL (NG6)
- Emotional contagion (EmpathyState → AffectState flow) — deferred to v0.4+ (Q8)

## Dependencies

- **engramai v0.2.2** — v0.3 is a major version upgrade of this crate. v0.2 schema + feature set is the starting point.
- **SQLite (rusqlite)** — existing dependency, gains new tables (`entity`, `edge`, `affect_mood_history`, etc.) and columns.
- **LLM provider (existing)** — Claude / OpenAI via existing `engramai::llm` abstraction. Used for: (a) ambiguity resolution in entity/edge extraction (Stage 5), (b) topic synthesis at L5, (c) reconciliation prompts (§4.4). No new provider required.
- **Embedding model (existing)** — used for candidate retrieval in Stage 1 and mood-congruent recall in §5.3.

## Open Questions (deferred — tracked in DESIGN §10)

v0.3 ships with 10 acknowledged open questions. Q7 has been resolved in requirements (see GOAL-4.6 — preserve-plus-resynthesize). The remainder are listed here with one-line summaries so reviewers can triage impact without opening DESIGN-v0.3.md.

- **Q1** Decay schedule for entity activation vs memory activation. Affects: v03-graph-layer (GOAL-1.2), v03-retrieval ranking.
- **Q2** Per-stage LLM model selection (cheap model vs strong model per stage). Affects: v03-resolution cost structure, v03-benchmarks GOAL-5.4 headroom.
- **Q3** Candidate retrieval breadth (top-K default + limits). Affects: v03-resolution (GOAL-2.5).
- **Q4** Somatic fingerprint aggregation function (mean vs median vs EWMA) at entity level. Affects: v03-graph-layer (GOAL-1.13), v03-resolution (GOAL-2.8).
- **Q5** Retro-evolution trigger rules (when does a later episode re-illuminate an edge?). Affects: v03-resolution (GOAL-2.12); design leans toward shipping without, adding in v0.3.1.
- **Q6** Multi-user / namespace scoping for entities. Out of scope for v0.3 per NG6, but flagged here for v0.4.
- **Q7** ✅ **Resolved**: v0.2 topics are carried forward with `legacy=true` + provenance; re-synthesis runs alongside (GOAL-4.6).
- **Q8** Emotional contagion (EmpathyState → AffectState flow). Out of scope per GUARD-5; deferred to v0.4+.
- **Q9** Knowledge Compiler clustering strategy (co-occurrence vs affect-weighted). Affects: v03-retrieval (GOAL-3.7); design leans toward (b) affect-weighted.
- **Q10** L5 topic cost budget — how many LLM calls can topic synthesis consume before it's gating on cost. Affects: v03-retrieval (GOAL-3.13), v03-benchmarks.

See DESIGN-v0.3.md §10 for full discussion. If a resolution changes requirements, the affected GOAL gets amended with a revision note.

## Summary

**12 GUARDs** (10 hard / 2 soft), 5 features, GOALs live in per-feature docs.

Feature GOAL counts (post-review-r1):
- v03-graph-layer: 15 GOALs (10 P0 / 4 P1 / 1 P2)
- v03-resolution: 14 GOALs (7 P0 / 6 P1 / 1 P2)
- v03-retrieval: 14 GOALs (6 P0 / 6 P1 / 2 P2)
- v03-migration: 9 GOALs (4 P0 / 5 P1)
- v03-benchmarks: 8 GOALs (5 P0 / 2 P1 / 1 P2)

**Total: 60 GOALs** across five features, far above the 15-GOAL single-doc threshold — confirming the split decision.
