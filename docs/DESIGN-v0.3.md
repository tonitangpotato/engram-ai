# Engram v0.3 — Design Document (DRAFT)

> **Status**: Draft for review (2026-04-23)
> **Prereq reading**: `engram-v0.3-design-discussion.md` + `engram-v0.3-research.md`
> **Target**: MVP in 2–3 months, polished in 6 months
> **Backwards compatibility**: engramai crate is published (v0.2.2). v0.3 is a **major version** — breaking changes allowed, migration path required.

---

## §0. Why v0.3 exists

Engram v0.2 is cognitively rich but structurally thin: world-class decay, affect, consolidation, interoception — and no entity identity, no typed edges, no bi-temporal facts. Ask it "is Melanie still married to Marcus?" and it gives you two unrelated strings. Every modern agent memory system (Graphiti, mem0, A-MEM, Letta, LightRAG) has the structural layer but only shallow cognition next to it — importance scores, agent-exposed forgetting, trace annotations. Parallel, not interlocked.

**v0.3's thesis is stronger than "add a graph":** the cognitive substrate should *shape* the structural layer, not sit beside it. Affect modulates which entities merge (§4.3 s6/s8). Mood congruence retrieves by felt-sense, not text similarity (§5.3). Consolidation re-writes edges through later emotional context (§6 retro-evolution). Topics compile from what the system *cared about*, not just what it saw (§6 + L5). Decay, ACT-R activation, and Hebbian strengthening operate on the *same* substrate the graph lives on — memories, entities, and edges all age, activate, and bond together.

This is what "interlock" means concretely, and it is the design's novel claim. Engram v0.3 is the first memory system where cognition writes on the structural substrate rather than running alongside it.

---

## §1. Goals & non-goals

### Goals

- **G1** Answer structured factual queries with Graphiti-level precision ("what is X's role?", "when did Y end?")
- **G2** Retain engram's cognitive signature — decay, affect, consolidation, interoception, metacognition — and make them **interact** with the new graph layer, not run parallel to it
- **G3** Reduce per-episode LLM cost to **average 2–3 calls** via multi-signal fusion (vs Graphiti's reported 5–10) **without sacrificing quality**. Measurement: per-stage counter in `write_stats.rs`, averaged over a benchmark corpus of N = 500 episodes drawn from LOCOMO + rustclaw's production trace.
- **G4** Expose hot/warm/cold memory tiers as a formal API (Letta-style) mapped onto existing working/core/archived
- **G5** Support dual-level retrieval — entity queries hit the graph, abstract queries hit Knowledge Compiler topics — with automatic routing
- **G6** Preserve SQLite-embedded deployment. No new required external dependency.
- **G7** Provide a clean migration path from v0.2 data

### Non-goals

- **NG1** Not a distributed system. Single-node SQLite is the substrate.
- **NG2** Not a general-purpose graph database. The graph exists to serve memory, not vice versa.
- **NG3** Not LLM-free. Ambiguity *requires* LLM judgment; we minimize calls, we don't eliminate them.
- **NG4** Not agent-managed-only (Letta model). Agents *may* curate via tools, but default behavior is automatic.
- **NG5** Not replacing v0.2's `MemoryRecord`. It is *extended* with provenance + reliability fields (§3.2) and retains its episodic-trace role — graph entities/edges layer alongside, not on top of, the existing record. Existing consumers keep working.
- **NG6** No support for multi-tenant isolation beyond what v0.2 already has (ACL)

### Invariants (non-negotiable, apply system-wide)

- **INV1 — Never silent degrade.** The system may complete fully, or fail cleanly with explicit surfacing (log + metric + optional alert). It MUST NOT produce a lower-quality result without the operator/user being notified. No background fallback to cheaper models, no silent result truncation, no "best effort" semantics without an explicit signal. When a pipeline stage cannot complete (LLM error, rate limit, timeout), the failure becomes visible data in the system — not absent.
- **INV2 — Episodic completeness.** L1 (Episode) and L2 (MemoryRecord admit) MUST succeed for every ingested interaction, even when downstream stages (extraction, entity/edge updates) fail. Losing the episodic trace is never an acceptable failure mode; losing the structural extraction is acceptable if (and only if) INV1 is respected.
- **INV3 — No retroactive silent rewrites.** Bi-temporal invalidation marks old data superseded; it does not erase. Consolidation and retro-evolution produce new versions with provenance, never overwrite without audit trail.

---

## §2. The five-layer memory model

```
┌──────────────────────────────────────────────────────────┐
│  L5  Knowledge Topics     (Knowledge Compiler)           │  ← abstract
│       ↑ synthesize                                       │
├──────────────────────────────────────────────────────────┤
│  L4  Semantic Graph       (Entity + Edge, bi-temporal)   │  ← structured facts
│       ↑ extract + resolve                                │
├──────────────────────────────────────────────────────────┤
│  L3  Core Memory          (r2, consolidated traces)      │  ← long-term episodic
│       ↑ consolidate (Murre & Chessa ODE)                 │
├──────────────────────────────────────────────────────────┤
│  L2  Working Memory       (r1, recent traces)            │  ← short-term episodic
│       ↑ admit                                            │
├──────────────────────────────────────────────────────────┤
│  L1  Episode Buffer       (raw interaction units)        │  ← provenance anchor
└──────────────────────────────────────────────────────────┘

Cross-cutting (runs on ALL layers):
 • ACT-R activation                 • Affective metadata (11-dim)
 • Decay / forgetting               • Hebbian associations
 • Interoceptive regulation         • Metacognition / confidence
```

**Key insight**: layers are not a waterfall. Retrieval and writes cross layers freely.

- L1 is new (explicit episode boundary, replaces ad-hoc `source` string)
- L2/L3 are v0.2's `working_strength` / `core_strength` — renamed & formalized
- L4 is new (the graph)
- L5 is v0.2's Knowledge Compiler — integrated, not separate

### Layer responsibilities

| Layer | Stores | Writeable by | Queryable by |
|---|---|---|---|
| L1 Episode | raw message + timestamp + session | ingest pipeline | provenance lookups |
| L2 Working | short-term trace (r1) with full metadata | admit() | recall(), recent-bias queries |
| L3 Core | consolidated trace (r2) with full metadata | consolidate() | recall(), stable memory queries |
| L4 Graph | entity nodes, edges (bi-temporal) | extractor, resolver | structured queries, traversal, hybrid |
| L5 Topics | synthesized topic pages | Knowledge Compiler | abstract queries, summarization |

---

## §3. Data model

### 3.1 Episode (L1) — new

```rust
pub struct Episode {
    pub id: Uuid,                        // stable provenance anchor
    pub session_id: String,              // conversation/session grouping
    pub occurred_at: DateTime<Utc>,      // real-world time
    pub ingested_at: DateTime<Utc>,      // when we processed it
    pub content: String,                 // raw text (user + assistant turns, or a doc chunk)
    pub participants: Vec<String>,       // speakers, for convos
    pub source_kind: SourceKind,         // Conversation | Document | Observation | Reflection
    pub metadata: serde_json::Value,     // channel, user_id, etc.

    // §3.7 — cognitive state captured at write time
    pub affect_snapshot: Option<SomaticFingerprint>,  // 8-dim self-state felt-sense; immutable once written; None for legacy/backfilled episodes
}
```

Rationale: every fact and every memory must trace back to *something observable*. v0.2's `source: String` is too loose — we need UUIDs so graph edges can link back. The `affect_snapshot` field captures the agent's integrated felt-sense **at the moment of writing** (see §3.7); it is the write-time fingerprint that later feeds Stage 4 s8 and mood-congruent recall (§5.3).

### 3.2 MemoryRecord (L2/L3) — kept, extended

```rust
pub struct MemoryRecord {
    // ---- v0.2 fields (KEPT) ----
    pub id: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub layer: MemoryLayer,              // Working | Core | Archived
    pub created_at: DateTime<Utc>,
    pub access_times: Vec<DateTime<Utc>>,
    pub working_strength: f64,           // r1
    pub core_strength: f64,              // r2
    pub importance: f64,
    pub pinned: bool,
    pub consolidation_count: i32,
    pub last_consolidated: Option<DateTime<Utc>>,
    pub contradicts: Option<String>,
    pub contradicted_by: Option<String>,
    pub superseded_by: Option<String>,
    pub metadata: Option<serde_json::Value>,  // Affect-owned 11-dim affect tag stored under key "affect" (§3.7 rule #1)

    // ---- v0.3 new ----
    pub episode_id: Option<Uuid>,        // provenance (L1 link)
    pub entity_ids: Vec<Uuid>,           // entities mentioned (L4 refs)
    pub edge_ids: Vec<Uuid>,             // edges derived from this (L4 refs)
}
```

MemoryRecord **stays the episodic-trace unit**. It's what decays, what consolidates, what carries affect. The graph extracts *from* it but does not replace it.

**On confidence — intentionally not stored on MemoryRecord.** Earlier drafts included a `confidence: f64` field. It was removed because MemoryRecord's confidence is **derived, not intrinsic**: the L2/L3 trace is the record of "this was experienced"; confidence in any factual claim extracted from it belongs on the *extracted edge*, not on the trace. Storing confidence at MemoryRecord level invited two bugs:
1. Conflating "did this happen?" (high — it was in the episode) with "is the extracted claim correct?" (varies — depends on extraction/resolution).
2. Creating a second source of truth alongside `Edge.confidence` (§3.4) that drifted under consolidation.

The fix is semantic clarity, not a new field: **edges carry claim confidence; memory records carry trace strength** (`working_strength`, `core_strength`, `importance`). When a read path needs a "confidence for this memory's claims", it composes from the linked edges, not from the record.

### 3.3 Entity (L4) — new

```rust
pub struct Entity {
    pub id: Uuid,
    pub canonical_name: String,
    pub aliases: Vec<String>,            // "Mel", "Melanie Smith"
    pub entity_type: EntityType,         // Person | Org | Place | Concept | Event | Artifact | Topic
    pub summary: String,                 // LLM-generated rolling description
    pub attributes: serde_json::Value,   // typed properties
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub episode_mentions: Vec<Uuid>,     // provenance: L1 episodes
    pub memory_mentions: Vec<String>,    // provenance: L2/L3 memory ids

    // ---- engram-unique ----
    pub activation: f64,                 // ACT-R; decays when not accessed
    pub agent_affect: f64,               // self-directed affect toward entity, -1..+1 (see §3.7)
    pub arousal: f64,                    // activation level associated with this entity (0..1)
    pub importance: f64,                 // weighted by mention count + pin + affect
    pub confidence: f64,                 // how sure are we about this entity's identity
    pub domain: Vec<String>,             // work / family / project-X (from mentions)
    pub somatic_fingerprint: [f32; 8],   // §3.7: aggregate of per-episode fingerprints (see fingerprint index semantics)
    // future (v0.4): empathy_signature — how the user feels about this entity
}
```

v0.2 already has `ExtractedEntity` — this is the **persisted** version, with identity + cognitive state.

### 3.4 Edge (L4) — new

```rust
pub struct Edge {
    pub id: Uuid,
    pub subject: Uuid,                   // Entity id
    pub predicate: String,               // free-form, see §3.5 on schema
    pub object: EdgeObject,              // Entity(Uuid) | Literal(Value)
    pub summary: String,                 // natural language restatement

    // ---- bi-temporal validity ----
    pub valid_at: Option<DateTime<Utc>>,   // when the fact became true (real-world)
    pub invalid_at: Option<DateTime<Utc>>, // when it stopped being true
    pub asserted_at: DateTime<Utc>,        // when engram learned it

    // ---- provenance ----
    pub episode_id: Uuid,                // which episode asserted it (REQUIRED)
    pub memory_id: Option<String>,       // which MemoryRecord extracted it

    // ---- invalidation (not deletion) ----
    pub invalidated_by: Option<Uuid>,    // newer edge that supersedes this
    pub supersedes: Option<Uuid>,        // older edge this replaces

    // ---- engram-unique ----
    pub activation: f64,
    pub confidence: f64,                 // cheap signal score OR LLM confidence
    pub resolution_method: ResolutionMethod,  // CheapMerge | CheapNew | LlmResolved | AgentCurated
    pub agent_affect: f64,               // self-directed affect (-1..+1), inherited from parent memory (see §3.7)
}

pub enum EdgeObject {
    Entity(Uuid),
    Literal(serde_json::Value),          // for (subject, age, 42) style
}

pub enum ResolutionMethod {
    CheapMerge { confidence: f64, signals: SignalBreakdown },
    CheapNew   { confidence: f64 },
    LlmResolved { action: ReconcileAction },   // ADD/UPDATE/DELETE/NONE
    AgentCurated { agent_id: String },
}
```

### 3.5 Schema — hybrid (seeded canonical + open proposed)

Graphiti forces you to predeclare `edge_type_map`. Engram takes the **hybrid middle path** — structure where we have conviction, openness where we don't.

**v0.3 predicate schema:**

```rust
pub enum Predicate {
    // ---- Seeded canonical variants (9, chosen from analysis of high-frequency
    // predicates across v0.2 corpora + general-purpose relation taxonomies) ----
    Mentions,        // generic: "X mentions Y"
    LocatedAt,       // spatial: "X at/in Y"
    PartOf,          // mereological: "X part of Y"
    MemberOf,        // grouping: "X member of Y"
    CausedBy,        // causal (forward form)
    Enables,         // causal (capability form)
    Precedes,        // temporal ordering
    RelatesTo,       // fallback generic
    EquivalentTo,    // canonical identity

    // ---- Proposed fallback: novel predicate, raw string preserved ----
    Proposed(String),
}
```

**Why hybrid, not pure-emergent:**
- Seeded variants have **known inverse semantics** (`CausedBy` ↔ "cause of"), **known query patterns** (graph traversal needs to know `PartOf` is transitive), and **stable downstream code** can pattern-match them.
- Pure-emergent (v0.2 style, free strings everywhere) meant symmetric queries, traversal logic, and invariant checks all had to string-match against an unbounded vocabulary — brittle and slow.

**Why open, not pure-seeded:**
- Real interactions surface relations we didn't anticipate. Forcing them into `RelatesTo` loses information; forbidding them blocks ingestion.
- `Proposed(String)` preserves the raw predicate verbatim — **no information loss**.

**Invariants on `Proposed`:**
1. **Proposed variants do NOT participate in symmetric/inverse graph queries.** The inverse-relation logic (`CausedBy` ↔ "cause of") operates on Seeded only. Proposed strings are treated as opaque labels for retrieval, not structural relations.
2. **No downstream decision code reads specific Proposed strings.** They exist for preservation and future canonicalization. Any code that branches on a specific Proposed string is a bug.
3. **Proposed strings accumulate duplicates** (`"precedes"` / `"precede"` / `"happens before"` may co-exist). This is accepted technical debt, bounded and non-compounding — see "Schema inducer — deferred" below.

**Extractor guidance on choosing Seeded vs `Proposed`:**
- When the LLM-proposed predicate matches Seeded semantics (including near-synonyms like "part of" / "contains" mapping to `PartOf`), canonicalize to the Seeded variant.
- When no Seeded variant fits, emit `Proposed(raw_string)` — **do not shoehorn into `RelatesTo`**. `RelatesTo` is reserved for cases where the relation *is* semantically generic, not as a catch-all for unfamiliar predicates. Shoehorning defeats the "no information loss" property of the hybrid schema.
- Extractors are permitted to refuse (emit no edge) when the input is ambiguous. No-edge is better than a miscategorized edge.

**Schema inducer — deferred to v0.4 (ISS-031).** A background job that clusters Proposed strings and promotes high-confidence clusters to new Seeded variants is **not in v0.3 scope**. Rationale: clustering/promotion thresholds cannot be tuned without real corpus. Shipping the inducer pre-v0.3 would mean guessing numbers that almost certainly need rework. The right sequence is: ship v0.3 → collect real Proposed distribution → design inducer against data → ship in v0.4. Because invariant (2) ensures no downstream code depends on specific Proposed strings, this debt is a **one-time rewrite migration** at v0.4, not a compounding structural cost.

### 3.6 Knowledge Topic (L5) — kept from v0.2

No change to the Compiler's data model. The upgrade: topics now **link to entities** (not just memories), and abstract-query recall routes here.

---

### 3.7 Cognitive state model — Telemetry / Affect / Empathy

Engram is not a passive memory DB — it is a **cognitive substrate**. Memory, affect, and self-monitoring are co-located because in biological cognition they are not separable: encoding is affect-weighted, recall is mood-congruent, metacognition gates consolidation. v0.2 implemented this intuition but fused 10 disparate signal sources into a single flat `SignalSource` enum, creating category confusion (operational load and voice-detected user emotion sat side-by-side). v0.3 separates them into three orthogonal layers with strict boundary rules enforced at the module-dependency level.

#### The three layers

**Telemetry** — the agent's "body signals." Raw interoceptive inputs reflecting process health. Neuroscience analog: homeostatic and nociceptive signals (load, fatigue, resource pressure) the insula receives from the body.

**Affect** — the agent's interpreted emotional + metacognitive state. Self-directed. What recent literature calls "constructed emotion" (Barrett 2017): raw interoception + context + valuation. This is the layer that drives encoding salience and recall congruence.

**Empathy** — perception of *others'* affective states. Other-directed. A fully independent axis. Drives response-layer decisions (tone, pacing, topic sensitivity) and never flows back into Affect in v0.3 (see boundary rule #3).

#### Signal assignment

| Signal source              | Owned by    | Also consumed by          | Neuroscience role                    |
|----------------------------|-------------|---------------------------|--------------------------------------|
| OperationalLoad            | Telemetry   | Affect (via mapping)      | Homeostatic body-state               |
| ExecutionStress            | Telemetry   | Affect (via mapping)      | Sympathetic arousal proxy            |
| CognitiveFlow              | Telemetry   | Affect (via mapping)      | Effort/engagement proxy              |
| ResourcePressure           | Telemetry   | —                         | Metabolic pressure analog            |
| Anomaly detector           | Telemetry   | Affect (arousal reading)  | Novelty → arousal (Berlyne 1971)     |
| Accumulator (domain mood)  | Affect      | —                         | Long-run affective tone per domain   |
| Feedback (success rate)    | Affect      | —                         | RPE-like reinforcement signal        |
| Confidence                 | Affect      | —                         | Metacognitive self-model             |
| Alignment (drive-match)    | Affect      | —                         | Goal-state valence                   |
| VoiceEmotion               | Empathy     | —                         | Social perception (prosody)          |
| TextEmotion (future)       | Empathy     | —                         | Social perception (lexical)          |

**One source, multiple consumers is explicit.** Each signal has exactly one **owner** (the module that computes and stores it). Other layers that need it **subscribe** and apply a named mapping function — they never recompute. Example: the Anomaly detector lives in Telemetry and publishes a raw z-score; Affect subscribes and applies `arousal_of_anomaly(z) = tanh(|z| / 3.0)` to produce `AffectState.anomaly_arousal`. Change the detector once, all consumers follow.

#### Boundary rules (invariants)

1. **Affect owns the affect fields on persisted memory — `MemoryRecord.metadata.affect` and `Entity.agent_affect`.** Telemetry is never the *sole* content of an affective annotation; it may only appear as **components** inside Affect-owned composite structures (e.g., `SomaticFingerprint` indices [4] and [5]). The composite is authored at Affect's write time, so Telemetry is consumed, not promoted. Empathy never persists to self-affect fields (reserved for future `Entity.empathy_signature`, v0.4).
2. **Telemetry → Affect is one-way.** Body feeds mind, never reverse. Enforced at the module-dependency level: the `affect` crate depends on `telemetry`; the `telemetry` crate **cannot** depend on `affect`. Reverse edges are compile-time forbidden via workspace dependency rules (`cargo-deny` ban list). This makes the rule architectural, not disciplinary.
3. **Empathy is isolated from Affect in v0.3.** Detected user distress does not make the agent sad — v0.3 hard-codes no emotional contagion. The mechanism for v0.4 (if any) is deferred to §10 Q8.
4. **Write path is not gated by cognitive state.** Per §4.5: Telemetry drives backpressure (throttle extraction cadence, never skip admission); Affect raises encoding priority and consolidation ordering (raises only — never lowers below default); Empathy influences response-layer decisions only (never write path). No layer can cause an episode to be silently dropped.

#### Types

```rust
pub type Domain = String;  // free-form domain tag, e.g. "coding", "research"

/// Raw body-signal state. Runtime-only, never persisted.
pub struct TelemetryState {
    pub operational_load: f32,      // 0..1
    pub execution_stress: f32,      // 0..1
    pub cognitive_flow: f32,        // -1..+1 (-1=stuck, +1=flow)
    pub resource_pressure: f32,     // 0..1
    pub anomaly_score: f32,         // z-score, unbounded
    pub updated_at: DateTime<Utc>,
}

/// Agent's interpreted affective + metacognitive state. Runtime-only; its
/// projection (SomaticFingerprint) is snapshotted into Episode at write.
pub struct AffectState {
    pub valence: f32,                     // -1..+1
    pub arousal: f32,                     // 0..1
    pub confidence: f32,                  // 0..1, metacognitive
    pub alignment: f32,                   // 0..1, drive-match
    pub mood_by_domain: HashMap<Domain, f32>,  // -1..+1 per domain
    pub anomaly_arousal: f32,             // 0..1, derived from TelemetryState.anomaly_score
    pub feedback_recent: f32,             // 0..1, short-window success rate (RPE-like)
    pub updated_at: DateTime<Utc>,
}

/// Perception of the *other party's* affective state. Drives response-layer
/// decisions (tone, pacing); does not flow into AffectState in v0.3.
pub struct EmpathyState {
    pub user_valence: f32,      // -1..+1
    pub user_arousal: f32,      // 0..1
    pub user_engagement: f32,   // 0..1
    pub source: EmpathySource,  // Voice | Text | Mixed
    pub updated_at: DateTime<Utc>,
}

/// Umbrella view combining Telemetry + Affect (classic v0.2 shape).
/// Empathy is intentionally excluded — the umbrella covers self-state only;
/// "interoception" in the neuroscience sense spans body + interpreted
/// emotion, but not perception of others.
pub struct InteroceptiveState {
    pub telemetry: TelemetryState,
    pub affect: AffectState,
}
```

#### Somatic fingerprint — locked semantics

The 8-dim `somatic_fingerprint` used in `Episode.affect_snapshot` (§3.1), `Entity.somatic_fingerprint` (§3.3), and Stage 4 resolution (s8 signal, §4.3) has **fixed dimension semantics**. Reordering or reassigning indices is a breaking schema change.

```rust
/// 8-dim snapshot of "what it was like to be the agent at this moment."
/// Used for recall mood-congruence and Stage 4 entity resolution (s8).
/// Captures self-state only — empathy is explicitly excluded.
pub struct SomaticFingerprint([f32; 8]);

// Index semantics (STABLE — do not reorder):
//   [0] valence           Affect    — primary hedonic axis     (-1..+1)
//   [1] arousal           Affect    — activation level          ( 0..1)
//   [2] confidence        Affect    — metacognitive             ( 0..1)
//   [3] alignment         Affect    — drive congruence          ( 0..1)
//   [4] operational_load  Telemetry — normalized                ( 0..1)
//   [5] cognitive_flow    Telemetry — flow axis                (-1..+1)
//   [6] anomaly_arousal   Affect    — novelty reading           ( 0..1)
//                                     (derived from TelemetryState.anomaly_score
//                                      at snapshot time; stored because the
//                                      composite is Affect-owned)
//   [7] feedback_recent   Affect    — short-window success rate ( 0..1)
//                                     (from AffectState.feedback_recent)
```

Cosine similarity between two fingerprints ≈ similarity of the agent's integrated felt-sense across those moments (Damasio's somatic-marker hypothesis). Empathy is absent: matching episodes by "how the user felt" would conflate self-indexed recall with theory-of-mind, degrading Stage 4 fusion quality.

**Two fingerprint flavors, same schema:**
- `Episode.affect_snapshot` — write-time snapshot, immutable once set.
- `Entity.somatic_fingerprint` — aggregate over mention-episodes (e.g., EMA of contributing `Episode.affect_snapshot` values). Recomputed when new mentions arrive.

Stage 4 s8 compares a new Episode's snapshot against candidate Entities' aggregates.

#### Update cadence

| Layer     | Update trigger                                      | Typical rate     |
|-----------|-----------------------------------------------------|------------------|
| Telemetry | Event-driven (task start/end, resource poll, anomaly) | Sub-second     |
| Affect    | Telemetry delta + memory write + feedback + drive check | ~Second       |
| Empathy   | Per user-message (voice/text signal extraction)     | Per-interaction  |

Consumers should **never** recompute `Episode.affect_snapshot` from current state after the episode is written — it is captured at write time from `AffectState` and is immutable thereafter. This is why Stage 4 fusion can safely cache Episode fingerprints. Entity aggregates are recomputed when new mentions arrive but never replace historical Episode snapshots.

#### Persistence

- **TelemetryState** — runtime-only. Reconstructed on restart from signal inputs; historical values are not stored.
- **AffectState** — the live struct is runtime-only, but:
  - Its `SomaticFingerprint` projection is snapshotted into every `Episode.affect_snapshot` at write time.
  - `mood_by_domain` is persisted to a dedicated `affect_mood_history` table (one row per domain per update) for trend analysis and mood-congruent recall (§5.3).
- **EmpathyState** — runtime-only. Per-interaction snapshots may be stored in `Episode.metadata.empathy_at_write` (optional, config-gated; default off in v0.3 to keep the schema lean).
- **Entity.somatic_fingerprint** — aggregate over mention episodes; recomputed, persisted on Entity update.

#### API surface

```rust
impl Engram {
    pub fn telemetry_snapshot(&self)   -> TelemetryState;
    pub fn affect_snapshot(&self)      -> AffectState;
    pub fn empathy_snapshot(&self)     -> EmpathyState;

    /// Umbrella view (Telemetry + Affect). Permanent — "interoception"
    /// correctly denotes both body and interpreted layers in neuroscience,
    /// so this name stays as the canonical self-state accessor.
    pub fn interoceptive_snapshot(&self) -> InteroceptiveState;
}
```

#### Implementation split (informational)

```
crates/engramai/src/
├── telemetry/       new — body-signal layer (owns OperationalLoad,
│                      ExecutionStress, CognitiveFlow, ResourcePressure,
│                      Anomaly detector)
├── affect/          new — self-affect + metacognition (owns Accumulator,
│                      Feedback, Confidence, Alignment; subscribes to
│                      telemetry for arousal/body components)
├── empathy/         new — other-affect perception (extracted from
│                      SignalContext::VoiceEmotion + future TextEmotion)
└── interoceptive/   kept — thin facade, re-exports + blended snapshot
```

Module dependency graph is strictly one-directional: `interoceptive → {telemetry, affect}` (facade for self-state only), `affect → telemetry`, `empathy → (nothing)`, `telemetry → (nothing)`. `empathy` is a sibling crate, not reached through `interoceptive` — reflecting the neuroscience meaning of interoception as self-signal integration. Reverse edges are forbidden by workspace `cargo-deny` rules (see boundary rule #2). This supersedes ISS-030.

---

## §4. Write path — the ingest pipeline

### 4.1 Overview

```
raw input
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 1: Episode creation (no LLM)                   │
│   create Episode{}, assign UUID, persist to L1       │
└──────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 2: MemoryRecord admission (no LLM)             │
│   admit(content) → r1 trace in L2                    │
│   compute 11-dim affect (existing pipeline)          │
│   compute embedding (existing pipeline)              │
└──────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 3: Extract (1 LLM call)                        │
│   extract entities + edges from episode + recent     │
│   graph context (neighbors of mentioned entities)    │
│   → ExtractionResult { entities[], edges[] }         │
└──────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 4: Entity resolution (cheap path + LLM tie-   │
│          breaker, see §4.3)                          │
│   for each extracted entity:                         │
│     candidates = vector_search_entities(...)         │
│     confidence = multi_signal_fusion(candidates)     │
│     if conf > 0.85 → auto-merge                      │
│     if conf < 0.30 → auto-new                        │
│     else            → LLM decide (mem0-style prompt) │
└──────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 5: Edge resolution (cheap path + LLM)          │
│   for each extracted edge:                           │
│     existing = query_matching_edges(subj, pred)      │
│     cheap_decision = analyze(new, existing)          │
│     if clear → auto-apply                            │
│     else     → LLM decide: ADD/UPDATE/DELETE/NONE   │
│       UPDATE  → invalidate old edge, create new      │
│       DELETE  → invalidate old edge (don't purge)    │
│       NONE    → no-op                                │
│       ADD     → create new edge                      │
└──────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────┐
│ Stage 6: Linking & activation spread                 │
│   link MemoryRecord.entity_ids ← resolved entities   │
│   link MemoryRecord.edge_ids   ← resolved edges      │
│   spread activation: mentioned entities += Δ         │
│   update Hebbian links between co-mentioned entities │
│   update interoceptive state                         │
└──────────────────────────────────────────────────────┘
```

**Expected LLM calls per episode:**
- Simple (no ambiguity): 1 extract + 0 resolution = **1 call**
- Typical: 1 extract + 1–2 ambiguous entity/edge LLM calls = **2–3 calls**
- Complex (heavy ambiguity): up to 5 calls
- **Average target: 2–3 calls** vs Graphiti's 5–10

### 4.2 Stage 3 — extraction with graph context

Key improvement over v0.2: the extractor prompt includes **current graph neighborhood**.

```
System: You are extracting entities and facts from a new episode.
The following entities are already known and relevant (same session,
or high-activation, or recently mentioned):
  - Melanie (Person, ID e7a2) — "User's spouse, works at Acme"
  - Marcus (Person, ID 3f91) — "User's coworker, mentioned last week"
  - Acme (Org, ID 8d1c) — "Melanie's employer"

Episode: "Mel got promoted at her company yesterday"

Extract entities (reuse existing IDs when clearly the same) and edges
with temporal info when available.
```

This eliminates the "Melanie / Mel / she" fragmentation that plagues v0.2.

### 4.3 Stage 4 — entity resolution via multi-signal fusion

**This is engram's core cost-saver.** Eight signals, fused into a single confidence score.

**Stage 4 input contract.** `ExtractedEntity` in v0.2 is minimal (`name / normalized / entity_type`). Stage 4 requires richer context, so we introduce a resolution input that bundles the extracted entity with its source-episode context:

```rust
pub struct EntityResolutionInput<'a> {
    pub extracted:      &'a ExtractedEntity,   // name, normalized, entity_type
    pub embedding:      &'a [f32],             // computed from mention span
    pub co_mentions:    &'a [EntityId],        // other entities extracted from the same Episode
    pub domain:         Option<Domain>,        // classified from Episode content
    pub source_affect:  Option<AgentAffect>,   // Episode's author-affect toward this mention
    pub source_episode: &'a Episode,           // enables reading affect_snapshot (§3.1, §3.7)
}
```

`source_affect` mirrors `Entity.agent_affect` in type and semantics so `affect_similarity` compares like-with-like. It is derived from the Episode's AffectState at extraction time, then passed in (never stored on `ExtractedEntity`).

```rust
fn entity_match_confidence(new: &EntityResolutionInput, cand: &Entity) -> f64 {
    let s1 = embedding_cosine(new.embedding, cand.embedding);              // 0..1
    let s2 = name_match_score(&new.extracted.name, &cand.aliases);         // exact=1, partial=0.5
    let s3 = actr_activation(cand);                                        // high act ⇒ currently-discussed
    let s4 = hebbian_overlap(new.co_mentions, cand);                       // shared neighbors
    let s5 = temporal_proximity(cand.last_seen, now);                      // recently seen ⇒ same
    let s6 = affect_similarity(new.source_affect, cand.agent_affect);      // §3.7: self-affect continuity
    let s7 = domain_match(new.domain.as_ref(), &cand.domain);              // same context
    let s8 = somatic_match(
        new.source_episode.affect_snapshot.as_ref(),                       // Episode snapshot (§3.1)
        cand.somatic_fingerprint.as_ref(),                                 // Entity aggregate (§3.3)
    );

    weighted_fusion(&[
        (s1, 0.30), (s2, 0.20), (s3, 0.10), (s4, 0.10),
        (s5, 0.10), (s6, 0.08), (s7, 0.07), (s8, 0.05),
    ])
}
```

**Decision thresholds** (to be tuned on LOCOMO):
- `conf > 0.85` → auto-merge (cheap path)
- `conf < 0.30` → auto-new (cheap path)
- `0.30 ≤ conf ≤ 0.85` → LLM tie-breaker

**Weights are initial guesses** — §8 covers tuning.

### 4.4 Stage 5 — edge resolution (mem0-style)

When cheap path is uncertain, we send one LLM call with the exact mem0 prompt shape:

```
Given the new fact and existing facts about the same subject+predicate,
choose ONE action:
  - ADD: new fact doesn't conflict, add it
  - UPDATE: new fact replaces old fact (old becomes invalid_at=now)
  - DELETE: new fact negates old fact (old becomes invalid_at=now, no new)
  - NONE: new fact is already known, skip

New:      (Melanie, works_at, BigCo)  [asserted 2026-04-23]
Existing: (Melanie, works_at, Acme)   [valid since 2024-01, still valid]

Answer: UPDATE (promotion / job change implied)
```

This is the cleanest reconciliation prompt in the field. We adopt it verbatim with minor temporal extensions.

### 4.5 Extraction failure handling

Graph extraction stages (Stage 3 extraction, Stage 4 entity resolution, Stage 5 edge resolution) all depend on LLM calls, which can fail transiently (rate limits, timeouts, provider errors). The write path handles these failures **reactively**, not preemptively — and always in compliance with INV1 (never silent degrade) and INV2 (episodic completeness).

**On LLM failure during Stage 3/4/5:**

1. The episode is admitted to L1 and L2 as normal. INV2 holds — the interaction is not lost.
2. Entity/edge updates for that episode are skipped. The graph layer is left incomplete for this specific episode.
3. The episode is marked with `extraction_error` metadata (stage + error kind + timestamp), stored on `Episode.metadata` (the free-form `serde_json::Value` field in §3.1). **Failure becomes visible data**, per INV1.
4. An `extraction_errors_total` metric increments; a warning is logged; repeated failures within a short window trigger an operator notification.
5. **No automatic retry.** Repeated LLM failures indicate infrastructure issues that need attention, not a problem to mask with retry loops. An operator-facing command (`engramai reextract --failed`) drives manual retry after the underlying issue is resolved.

**Explicitly NOT used as write-path gates** (per §3.7 boundary rule #4):

- **Telemetry** (operational load, execution stress, cognitive flow, resource pressure, anomaly) — informs metacognition and backpressure (throttle extraction cadence, never skip admission). High-stress episodes still enter L1/L2 in full.
- **Affect** (valence, arousal, confidence, alignment) — raises encoding priority and consolidation ordering, never lowers below default. High-valence episodes deserve *more* encoding, not less.
- **Empathy** (user voice emotion, engagement) — response-layer input only (tone, pacing). Reading the user's state must not gate storing the user's interaction; that would be data loss disguised as politeness.

No layer can cause an episode to be silently dropped. Throttling may delay processing; it must never delete.

**What replaced the earlier "interoceptive gating / backlog" mechanism:**

An earlier draft proposed pausing Stage 3/4/5 when interoceptive stress was high, queuing episodes with `pending_extraction = true`, and processing the backlog during idle consolidation. This mechanism is removed. It violated INV1 (silent degradation: extraction silently skipped) and created a second write path with its own failure modes and consistency invariants. The reactive-error-handling model above achieves the same robustness goal — bursts don't lose data — without the ghost-pipeline complexity.

---

## §5. Read path — retrieval

### 5.1 Query classification (one cheap LLM call OR heuristic)

Borrowed from LightRAG and our existing `query_classifier`:

```
QueryIntent {
  Factual,          // "when did X happen?", "who is Y?"
  Abstract,         // "what have we discussed about X?", "summarize our work on Y"
  Episodic,         // "what did I say yesterday?"
  Affective,        // "what made me feel stuck?" (engram-unique)
  Hybrid,           // mixture
}
```

Heuristic first (regex + keyword + dimension hits), LLM fallback only when heuristic is unsure.

### 5.2 Dual-level retrieval

```
Query → classify intent → route:

Factual   → L4 graph query (entity + edge traversal)
           + L2/L3 memories linked to those entities
           + hybrid vector/BM25 rerank
           
Abstract  → L5 Knowledge Topics (compiled)
           + entity summaries from L4
           
Episodic  → L2/L3 vector + temporal filter
           + episode reconstruction from L1
           
Affective → L2/L3 filtered by somatic fingerprint
           + ACT-R activation boost
           + mood-congruent affect match (§5.3)
           
Hybrid    → run multiple paths, fuse via RRF
```

### 5.3 Affect-driven recall (engram-unique)

Beyond "does this match the query?", engram also ranks by **affective congruence**:

```rust
score = vector_sim * w_sim
      + actr_activation * w_act
      + affect_match(current, mem.source_episode.affect_snapshot) * w_aff
      + hebbian_spread(seed_mems, mem) * w_heb
      + recency_bonus * w_rec
```

Where:
- `current = engram.affect_snapshot()` — the agent's **current** AffectState projected to `SomaticFingerprint`. This is mood-congruent recall in the Bower 1981 sense: the agent recalls more of what it encoded while in a similar self-state.
- `mem.source_episode.affect_snapshot` — the **immutable** snapshot captured at the Episode's write time (§3.1, §3.7). Recall never reads the live `AffectState` of past episodes; the snapshot is the sole source of truth.
- `affect_match` — cosine similarity between the two 8-dim fingerprints, clipped to 0..1.

This is explicitly **mood-congruent** (read the agent's own past felt-sense) and **not** content-congruent (read affect inferred from the query text). Content-congruent recall would require a separate `query_affect` extraction step and is deferred to v0.4.

Default weights adapt to query intent (factual queries drop `w_aff`, affective queries boost it). `w_aff = 0.0` for Factual queries (§5.2 routing); meaningful for Affective queries.

### 5.4 Temporal queries

"What did I believe about X on date D?" becomes a first-class query:

```rust
graph.edges_valid_at(subject=X, at=D)
  .filter(|e| e.valid_at <= D && e.invalid_at.map_or(true, |iv| iv > D))
```

This is what bi-temporal edges are for. v0.2 can't answer this.

---

## §6. Consolidation — where the layers interlock

Consolidation is the clearest embodiment of §0's thesis: one cycle, one substrate, six interacting processes. Decay (1-2) lowers activation on memories, entities, and edges uniformly — the structural layer ages with the episodic one. Hebbian strengthening (3) operates on entity-pair co-access, not just memory-memory, so the graph's activation network *is* the cognitive association network. Retro-evolution (4) re-reads older edges through the lens of later episodes — the only place where temporal context actively rewrites prior structure, preserving audit trail per INV3. Audit (5) catches resolution errors that fusion (§4.3) couldn't avoid at write time, using accumulated evidence the write path didn't have. Knowledge Compiler (6) clusters *consolidated* entities, not raw memories — so topics emerge from what the cognitive substrate has already filtered and stabilized. Each step reads the output of the others; none runs in isolation.

One cycle does all of:

1. **Dual-trace ODE** (existing) — r1 decays, r2 accumulates
   `dr1/dt = -μ1·r1`, `dr2/dt = α·r1 - μ2·r2`
2. **ACT-R activation decay** (existing) — on memories AND entities/edges (new)
3. **Hebbian strengthening** (existing) — co-accessed pairs bond; applies to entity pairs too, not just memory pairs
4. **Retro-evolution** (A-MEM, new) — for each consolidated memory, check if its entity summaries / edge annotations should be rewritten given later episodes (produces new versions with provenance, per INV3)
5. **Offline audit** (new) — sample entity merges, check for false merges/splits, auto-correct with full audit trail
6. **Knowledge Compiler** (existing) — synthesize new topics from consolidated memories + graph clusters (clusters formed from post-audit entities, not raw extractions)

Cycle frequency: hourly for light tasks (1,2,3), daily for heavy (4,5,6). Triggered by **agent idle state** (no recent episode ingestion) — runs when the system is quiet, not on a fixed timer.

**Not in v0.3 consolidation:**
- *Pending-extraction backlog processing* — removed with §4.5's ghost-pipeline; extraction failures are now surfaced reactively (§4.5), not deferred here.
- *Schema induction (predicate clustering)* — deferred to v0.4 (ISS-031), see §3.5.


---

## §7. Public API

### 7.1 Core API — simple by default, powerful when needed

```rust
impl Memory {
    // ---- Write ----
    
    /// Admit a raw episode. Handles the full pipeline (§4).
    /// Returns the Episode id + admitted MemoryRecord id.
    pub async fn ingest(&mut self, ep: EpisodeInput) -> Result<IngestResult>;
    
    /// Fast path: store a single fact as text (v0.2-compatible).
    /// Internally wraps in a single-message Episode.
    pub async fn store(&mut self, content: &str, meta: StorageMeta) -> Result<String>;
    
    // ---- Read ----
    
    /// Query-classifying recall. Routes to graph / topics / vector as appropriate.
    pub async fn recall(&self, query: &str, opts: RecallOptions) -> Result<Vec<RecallResult>>;
    
    /// Structured factual query (bypasses classification).
    pub fn query_facts(&self, spec: FactQuery) -> Result<Vec<Edge>>;
    
    /// Temporal query: "what was true at time T?"
    pub fn query_at(&self, spec: FactQuery, at: DateTime<Utc>) -> Result<Vec<Edge>>;
    
    /// Entity-centric query: everything about entity X.
    pub fn entity_view(&self, id: Uuid) -> Result<EntityView>;

    /// Debuggability: return the trace of a recall — which layer answered,
    /// what candidates each stage produced, how scores were composed, which
    /// cognitive signals modulated ranking. Essential given the 5-layer model.
    /// Overhead is opt-in; standard `recall()` does not pay the trace cost.
    pub async fn explain_recall(&self, query: &str, opts: RecallOptions) -> Result<RecallTrace>;
    
    // ---- Tier API (Letta-style, engram-backed) ----
    
    pub fn hot_memories(&self) -> Vec<MemoryRecord>;     // working (r1 high)
    pub fn warm_memories(&self) -> Vec<MemoryRecord>;    // core (r2 high, recent activation)
    pub fn cold_memories(&self) -> Vec<MemoryRecord>;    // archived
    pub fn pin(&mut self, id: &str) -> Result<()>;
    pub fn unpin(&mut self, id: &str) -> Result<()>;
    
    // ---- Cognitive state (§3.7: Telemetry / Affect / Empathy) ----

    /// Raw body-signal layer: operational load, stress, flow, resource
    /// pressure, anomaly score. Runtime-only, never persisted.
    pub fn telemetry_snapshot(&self) -> TelemetryState;

    /// Self-directed affect + metacognition: valence, arousal, confidence,
    /// alignment, per-domain mood. The layer that drives encoding salience
    /// and mood-congruent recall.
    pub fn affect_snapshot(&self) -> AffectState;

    /// Other-directed empathy: perceived user valence/arousal/engagement.
    /// Response-layer input only; does not flow into AffectState in v0.3.
    pub fn empathy_snapshot(&self) -> EmpathyState;

    /// Umbrella view (Telemetry + Affect). Permanent API — "interoception"
    /// in neuroscience correctly spans body + interpreted emotion, so this
    /// name stays as the canonical self-state accessor. Empathy is
    /// intentionally excluded (self-state only).
    pub fn interoceptive_snapshot(&self) -> InteroceptiveState;

    pub fn metacognition_report(&self) -> Option<MetaCognitionReport>;
}
```

### 7.2 Agent tools (opt-in, Letta-inspired)

For agents that want to curate memory, expose these as MCP/function tools:

- `memory.pin(id)` — prevent decay on a specific memory
- `memory.correct(id, new_content)` — supersede with correction
- `memory.forget(id, reason)` — soft delete (Quarantined layer, not purged)
- `memory.link(a, b, relation)` — manually assert an edge
- `memory.summarize_entity(id)` — trigger entity summary regeneration

**Design choice: these are optional.** Default behavior is fully automatic. Agent curation is an enhancement, not a requirement (rejecting Letta's "agent drives everything" model).

### 7.3 Backward compatibility

The v0.2 API surface stays working:
- `store`, `recall`, `recall_recent`, `recall_with_associations` — unchanged signatures
- Internally, they now route through the v0.3 pipeline
- New capabilities require new methods (`ingest`, `query_facts`, `entity_view`)

---

## §8. Migration from v0.2

### 8.1 Schema migration

SQLite tables added (non-destructive):
- `episodes`
- `entities`
- `entity_aliases`
- `edges`
- `edge_invalidations`
- `predicate_canonicals` (for emergent schema)

Existing tables kept:
- `memories` — gets new columns `episode_id`, `entity_ids` (JSON array), `edge_ids` (JSON array), `confidence`
- `hebbian_links` — gains optional `entity_pair` column

Schema details tied to §3.7 (cognitive state model):
- `episodes.affect_snapshot` — `BLOB NULL`, 32 bytes = 8 × f32 (little-endian) when present. Captured at write time from `AffectState` per §3.7 persistence rules. Nullable because legacy/backfilled episodes have no snapshot.
- `entities.agent_affect` — replaces the earlier `entities.valence` column name. Semantic clarification (see §3.3): this is the agent's self-directed affect toward the entity, not an empathy reading of the entity. Migration is a pure rename (no data change).
- `edges.agent_affect` — same rename on the `edges` table (see §3.4). Inherited from parent memory; the rename aligns Edge and Entity schemas under §3.7 semantics.
- `affect_mood_history` (new) — `(domain TEXT, value REAL, updated_at TIMESTAMP)`, append-only, for `AffectState.mood_by_domain` persistence per §3.7.

Migration tool: `engramai migrate --from 0.2 --to 0.3`. Runs idempotently:
1. **Pre-migration safety**: write `{db_path}.pre-v03.bak` (full SQLite file copy) unless `--no-backup` is passed. Migration aborts if the backup cannot be written.
2. Add new tables and columns.
3. Backfill: for each existing memory, create synthetic Episode and re-extract entities/edges lazily on first access (or in a batch job).
4. Verify invariants (no orphan edges, etc.).

**Rollback path.** v0.3 → v0.2 schema migration is **not reversible in-place** — new tables/columns cannot be dropped cleanly once populated. Users who want to roll back must restore from `{db_path}.pre-v03.bak`. The CLI makes this explicit:
- On first run of a published-v0.2 DB, the tool prints: "This migration is forward-only. A backup will be written to `{db_path}.pre-v03.bak`. To roll back, stop engramai and `mv` the backup over the live file. Proceed? [y/N]" — unless `--accept-forward-only` is passed for scripted environments.
- `engramai migrate --status` reports whether a backup exists and its timestamp.

### 8.2 Behavioral backward compatibility

v0.2 consumers see identical behavior until they opt in to new APIs:
- `store(content)` still works, graph extraction runs in the background
- `recall(query)` still returns ranked memories; dual-level routing is transparent

### 8.3 Fusion weight tuning

Initial weights in §4.3 are guesses. Tuning plan:
1. Run LOCOMO + LongMemEval with guess weights → baseline
2. Grid search on weights using 20% held-out subset
3. Report before/after scores; freeze weights for v0.3.0 release
4. Expose weights via `MemoryConfig::fusion_weights` for users to override

---

## §9. Roadmap

### Phase 0 — Alignment (1 week, this week)

- Finalize this DESIGN doc after review
- Split into feature-level requirements + design per `draft-requirements` / `draft-design` skills
- Create `.gid/features/graph-layer`, `.gid/features/resolution`, `.gid/features/retrieval`, `.gid/features/migration`
- Generate graph via `gid_design`

### Phase 1 — Graph foundation (3 weeks)

- Implement L1 Episode persistence
- Implement Entity + Edge schema (SQLite)
- Implement bi-temporal invalidation
- Port/adapt the existing `triple.rs` + `entities.rs` into the new graph layer
- Unit tests; invariant checks (no dangling edges, no orphan entities)
- **Milestone**: can manually insert entities/edges and query them; no LLM yet

### Phase 2 — Ingest pipeline (3 weeks)

- Stage 1–6 of §4 implemented end-to-end
- Extraction with graph context
- Multi-signal fusion (start with rough weights)
- Edge resolution prompt
- Reactive extraction-failure handling (§4.5) + surfacing
- **Milestone**: `ingest(episode)` works end-to-end; LOCOMO smoke test runs

### Phase 3 — Retrieval upgrade (2 weeks)

- Query classifier formalized
- Dual-level routing
- Affect-driven ranking
- Temporal queries
- **Milestone**: recall on LOCOMO ≥ v0.2 baseline on all categories

### Phase 4 — Consolidation integration (2 weeks)

- Retro-evolution (A-MEM borrow)
- Offline audit
- Knowledge Compiler hooked to entity clusters
- **Milestone**: consolidation cycle runs all 6 steps; quality stays stable over 1000-episode simulation

### Phase 5 — Migration + benchmark + polish (2 weeks)

- `engramai migrate` tool
- LOCOMO + LongMemEval full rerun with fusion-weight tuning
- Publish v0.3.0-rc1 to crates.io
- Documentation
- **Milestone**: v0.3.0 released

**Total: ~13 weeks ≈ 3.25 months.**  (Phase 2 replaced "interoceptive gating" with reactive failure handling; Phase 4 shortened by 1 week since schema induction is deferred to v0.4 per ISS-031.) Matches the "2–3 months MVP, 6 months polished" estimate.

### Parallelization

Phases 1 and 2 mostly serial. Phase 3, 4, 5 can overlap:
- Phase 3 can start once Phase 2 has basic ingest
- Phase 4 can start in parallel with Phase 3 (different code regions)
- Phase 5 tuning begins once Phase 4 lands

---

## §10. Open questions (need potato's call before implementation)

These are the blocking decisions the design skirts around. Don't start coding until resolved.

**Q1. Does MemoryRecord become optional once L4 exists?**
Two reads on the data flow:
- (a) MemoryRecord is the *always-present* episodic trace; L4 extracts from it
- (b) Some episodes skip L2/L3 and go straight to L4 (e.g., ingesting a structured doc)
I lean (a) for uniformity. Your call.

**Q2. Predicate schema — fully emergent, or seeded?**
Fully emergent is clean but cold-starts badly (early predicates are noisy). Seeded with a small set of canonical predicates (e.g., 20 common ones like `works_at`, `located_in`, `knows`) bootstraps faster but reintroduces Graphiti's "predefined schema" smell. I lean seeded-with-override; happy to go fully emergent if you prefer.

**Q3. Offline audit cost budget.**
Retro-evolution + audit are LLM-heavy (sampling, rewriting, correcting). How much daily LLM spend is acceptable for consolidation? Options:
- Aggressive (all memories monthly): ~1000 LLM calls/day for a chatty user
- Moderate (top-k important only): ~100/day
- Minimal (on explicit trigger): ~10/day

**Q4. Embedding strategy for entities.**
Options: (a) embed `canonical_name + summary`, re-embed on summary change; (b) maintain a rolling centroid of all mentions' embeddings; (c) both, stored separately. (b) is more robust but costs more at write time.

**Q5. Do we ship v0.3 without full A-MEM retro-evolution, and add in v0.3.1?**
Retro-evolution is the most speculative piece. Shipping without it would cut 2 weeks and 1 open question. Adding in v0.3.1 is low-risk since it's pure background work. I lean **ship without, add in v0.3.1**.

**Q6. Multi-user / namespace scoping.**
v0.2 has ACL. Does the graph respect namespaces? If two users both know a "Melanie", are those the same entity? Almost certainly not. Need explicit namespace isolation at entity level.

**Q7. What to do with existing `Knowledge Compiler`'s topics during migration?** ✅ **Resolved (2026-04-24, requirements r1)** — preserve-plus-resynthesize: all v0.2 topics are carried forward into L5 with `legacy=true` flag + provenance pointing to their v0.2 source; post-migration re-synthesis runs alongside and produces new topics without deleting legacy ones. See requirements GOAL-4.6.

**Q8. Emotional contagion — does Empathy ever flow into Affect (v0.4+)?**
v0.3 hard-codes full isolation (boundary rule #3). For v0.4, plausible options:
- (a) Scalar `contagion_coefficient: f32 ∈ [0, 1]` — weighted pull from `EmpathyState.user_valence` into `AffectState.valence`. Simple, but conflates all social situations.
- (b) Per-domain contagion weights — agent mirrors user mood more in "social" domains, less in "technical" domains.
- (c) Context-gated contagion — only when the user is a "close" entity (high ACT-R activation + positive `agent_affect`). Most psychologically plausible.
- (d) No contagion ever — perception ≠ absorption. Stay v0.3's stance.
Deferring until we have real multi-session empathy data from v0.3 deployment.

**Q9. Knowledge Compiler × graph — topics as "what the system cared about"?**
§0's thesis says topics compile from *caring*, not just co-occurrence. But §6 step 6 only says "synthesize topics from consolidated memories + graph clusters." Two concrete choices to resolve:
- (a) **Co-occurrence only** (v0.2 behavior): cluster by memory-memory and entity-entity co-access. Simple, but doesn't realize the §0 claim.
- (b) **Affect-weighted clustering**: weight cluster edges by `Entity.agent_affect` × `Entity.activation` × co-mention count. Entities the agent cared about (high affect magnitude, high activation) cluster more readily into a topic. Realizes §0 but introduces a new knob that needs tuning.
- (c) **Somatic-similar cluster seeds**: seed topic candidates from entities whose aggregate `somatic_fingerprint` is mutually close (similar felt-sense when encountered), then expand by graph proximity. Strongest affective signature, highest risk of producing topics that only "feel coherent" without being semantically tight.
I lean (b) — cheap, testable, and has an off-switch (set affect weight to 0 for v0.2 parity). Decision affects Phase 4 scope.

**Q10. Graph → Affect backchannel — emergent mood from structural state?**
Boundary rule §3.7 #2 forbids `affect → telemetry`, and §3.7 #4 forbids cognitive state from gating writes. But nothing is said about the *graph* feeding Affect. Plausible signals the graph could publish:
- Dense positive-affect subgraph being currently accessed → boost `AffectState.valence` briefly (agent is "in good company")
- Fresh high-confidence contradiction edge → raise `AffectState.anomaly_arousal` (cognitive dissonance as affect)
- Many recent Proposed-predicate edges unresolved → lower `AffectState.confidence` (system knows its schema is messy)
Options:
- (a) **None in v0.3**: graph is read-only for Affect; all affect inputs come from Telemetry + feedback + drive-match. Simplest, preserves current architecture.
- (b) **Read-only subscriptions**: Affect subscribes to named graph-derived signals (published by consolidation step 5 audit, or on graph access), applies mapping functions just like it does for Telemetry. No new dependency direction (graph is below affect in the module stack).
- (c) **Full backchannel**: graph state actively modulates affect updates. Richest, but blurs the Telemetry/Affect boundary since "graph" isn't in the three-layer §3.7 taxonomy.
I lean (a) for v0.3 (keeps §3.7 clean) with (b) as a v0.4 candidate once we see whether the seam actually hurts. Flagging now because this is the last genuinely-undecided piece of the interlock story.

---

## §11. Success criteria

v0.3 ships when:

- ✅ LOCOMO overall score ≥ mem0's 68.5% (matching or beating the LLM-driven baseline)
- ✅ LOCOMO temporal category ≥ Graphiti's reported number (we should win here via bi-temporal)
- ✅ LongMemEval overall ≥ v0.2 baseline + 15 percentage points
- ✅ Average LLM calls per episode ≤ 3 (measured via `write_stats.rs` counter over N = 500 benchmark episodes — LOCOMO test set + rustclaw production trace)
- ✅ All v0.2 tests (~280) still pass after migration
- ✅ Migration tool runs on real engram DB (rustclaw's own engram-memory.db) without data loss
- ✅ Interoceptive / metacognition / affect features all demonstrably affect retrieval ranking (regression test)

---

## §12. What this design explicitly does NOT do

Being clear about scope to avoid creep:

- ❌ Does not add a graph query language (Cypher/SPARQL). Queries are Rust API calls.
- ❌ Does not add distributed storage. Single-node SQLite only.
- ❌ Does not add vector DB pluggability beyond what v0.2 has. (Can be added later.)
- ❌ Does not add streaming/real-time sync to external KGs. (Export API exists; import is manual.)
- ❌ Does not implement "memory sharing between agents". Each agent has its own namespace.
- ❌ Does not replace Knowledge Compiler. Compiler is integrated, not superseded.
- ❌ Does not remove any v0.2 cognitive features. All are kept and integrated with the graph.

---

## §13. Review checklist (for potato before approving)

Read the doc, then check:

- [ ] §1 goals match your mental model
- [ ] §2 five-layer model makes sense (especially L1 as explicit provenance)
- [ ] §3 data model — any field missing? Any extra?
- [ ] §4 write pipeline — do the 6 stages feel right?
- [ ] §4.3 multi-signal fusion — do the 8 signals cover the cheap-path cases you were worried about?
- [ ] §5 dual-level retrieval routing — does the intent taxonomy match your usage?
- [ ] §6 consolidation — do the 6 steps all belong in one cycle, or should some split out?
- [ ] §9 roadmap — 3.5 months realistic, or should we narrow scope?
- [ ] §10 open questions — make a call on each (especially Q5 — ship without retro-evolution?)
- [ ] §11 success criteria — are these the right KPIs?

Once you sign off, I'll use the `draft-requirements` + `draft-design` skills to split this into feature-level docs and run `gid_design` to produce the graph + task breakdown.
