# Design: Engram v0.3 — Graph Layer

> **Feature:** v03-graph-layer
> **GOAL namespace:** GOAL-1.X
> **Master design:** `docs/DESIGN-v0.3.md` (§3 Data Model)
> **Requirements:** `.gid/features/v03-graph-layer/requirements.md`
> **Status:** Draft for review

## 1. Scope & Non-Scope

**In scope.** The at-rest shape of v0.3's semantic graph (L4): the `Entity` node type, the `Edge` relationship type, the hybrid predicate schema registry, the SQLite tables that persist them, the memory↔entity provenance join (`graph_memory_entity_mentions`), the L5 knowledge-topics table and its bridge to `EntityKind::Topic`, the pipeline-run / resolution-trace audit tables, the `GraphStore` CRUD trait, and body-signal telemetry hooks emitted when graph state mutates. Minimal extensions to `MemoryRecord` (provenance links per GOAL-1.11), visible-failure recording for graph extraction stages (GOAL-1.12), the layer-classification derivation rule (GOAL-1.14), and the structural hook for L5 knowledge topics (GOAL-1.15).

**Out of scope.** The write pipeline (Stages 3–5) → **v03-resolution**. Query classification, dual-level retrieval, mood-congruent recall → **v03-retrieval**. v0.2 → v0.3 migration SQL, backfill, rollback → **v03-migration** (this doc declares the target schema, not migration ordering). Benchmark corpora and LOCOMO harness → **v03-benchmarks**. Automatic predicate promotion is deferred to v0.4 per master §3.5; only the drift-monitoring query surface (GOAL-1.10) is in scope here.

## 2. Architecture Overview

```
                        Memory (public facade)
                               │
       ┌───────────────────────┼──────────────────────────┐
       │                       │                          │
   store_raw / recall    new v0.3 API                graph-layer
   (v0.2 surface, §5)    (add_episode, ...)          internals
                                                          │
                                                          ▼
                                               ┌──────────────────┐
                                               │  graph module    │
                                               │  (this feature)  │
                                               └─────────┬────────┘
                                                         │
     ┌────────────────────┬──────────────────┬───────────┴────────────┐
     │                    │                  │                        │
   entity.rs           edge.rs        schema_registry.rs         store.rs
   (Entity,           (Edge,          (PredicateCatalog,        (GraphStore
    EntityKind)        EdgeEnd,        Canonical /               trait, SQLite
                       Predicate)      Proposed rules)           impl)
     │                    │                  │                        │
     └────────────────────┴──────────────────┴────────────────────────┘
                                   │
                                   ▼
                  ┌────────────────────────────────────┐
                  │   Storage (existing, extended)     │
                  │   rusqlite, WAL, FK=ON             │
                  │   + tables: entities, edges,       │
                  │     entity_aliases, predicates,    │
                  │     extraction_failures            │
                  └──────────────┬─────────────────────┘
                                 │ emits
                                 ▼
                  Telemetry / body-signal bus (master §3.7)
                  OperationalLoad • ResourcePressure
```

The graph module sits alongside — not on top of — the existing episodic `storage.rs` tables (`memories`, `hebbian_links`, `entities`, `entity_relations`). The legacy v0.2 `entities` / `entity_relations` tables are untouched; v0.3 introduces a parallel `graph_entities` / `graph_edges` namespace so resolution, traversal, and bi-temporal invalidation can evolve without destabilizing v0.2 consumers (GOAL-1.13, NG5). Migration between the two namespaces is owned by v03-migration.

`GraphStore` is a trait so alternative backends (in-memory, test fixtures) can implement it. The production impl owns a `rusqlite::Connection` borrowed from the existing `Storage` struct via a new `Storage::graph()` accessor, so both legacy and graph writes share one transaction boundary (§4.3).

Body-signal emission (§6) is a pure outbound channel: the graph module never reads Telemetry or Affect — it only publishes `OperationalLoad` per graph write and `ResourcePressure` hints when row counts cross thresholds. The dependency arrow stays one-way, preserving the boundary rules from master §3.7.


## 3. Data Model

### 3.1 Entity (L3')

A v0.3 `Entity` is a canonical, long-lived node in the semantic graph. It is distinct from the v0.2 `ExtractedEntity` (in `crates/engramai/src/entities.rs`), which remains in place as a lightweight mention-level match and is **not** touched by this feature. The new type lives in a new module `crates/engramai/src/graph/entity.rs` so the v0.2 type and its tests stay source-compatible (GOAL-1.13).

```rust
// crates/engramai/src/graph/entity.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::affect::SomaticFingerprint; // 8-dim, locked (master §3.7)

/// Canonical entity kind. Superset of v0.2 `EntityType`, but a new enum so
/// v0.2's serde representation is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Person, Organization, Place, Concept, Event, Artifact,
    Topic,             // L5 bridge — see §3.4 and master §3.6
    /// Escape hatch for kinds outside the canonical set. The inner string is
    /// **normalized on insert**: lowercased, trimmed of surrounding whitespace,
    /// and NFKC-folded. Two `Other` values compare exactly on the normalized
    /// form (so `Other("Robot")` and `Other(" robot ")` collapse to one kind).
    /// Callers are discouraged from using `Other` for kinds that could
    /// plausibly be added as a canonical variant — file an issue and add the
    /// variant instead.
    Other(String),
}

/// A canonical graph-layer entity (L4 node).
///
/// Invariants:
///  - `id` is stable for the entity's lifetime; never reassigned on merge
///    (the loser's id is preserved in `entity_aliases`, §3.4; GOAL-1.6).
///  - `canonical_name` may be updated by later, more confident observations,
///    but the change is logged in the typed `history` field (audit, not
///    overwrite). `history` is a first-class column on `graph_entities`, not
///    an `attributes` JSON key — see design-r1 #51-52 for the promotion
///    rationale (caller writes to `attributes` cannot clobber audit state).
///  - `first_seen <= last_seen`. Both are real-world (episode) times.
///  - `somatic_fingerprint`, when present, is an aggregate over episode-level
///    fingerprints (GOAL-1.13). Recomputed by v03-resolution; stored here.
///  - `activation`, `importance`, `identity_confidence` are in [0.0, 1.0].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: Uuid,
    pub canonical_name: String,
    pub kind: EntityKind,

    /// Rolling summary. Empty until the first resolution pass completes.
    pub summary: String,

    /// Typed properties. Free-form JSON for caller-authored keys only.
    /// **Reserved keys are promoted to first-class fields** (`history`, `merged_into`
    /// — see below) and MUST NOT appear inside this map. `insert_entity` /
    /// `update_entity_*` reject any write whose `attributes` object contains
    /// a reserved key; the rejection is `GraphError::Invariant("reserved attribute key")`.
    pub attributes: serde_json::Value,

    /// Append-only audit of canonical_name changes (promoted from `attributes.history`
    /// so merge invariants can't be clobbered by a caller). Mutated only by
    /// `merge_entities` and by `update_entity_canonical_name` (not shown).
    #[serde(default)] pub history: Vec<HistoryEntry>,

    /// Set on merge loser: the winner's id. Promoted from `attributes.merged_into`
    /// so the redirect signal (§8 Reader semantics during merge) cannot be
    /// overwritten accidentally through `update_entity_cognitive`.
    /// None on winners and on entities that have never been merged.
    #[serde(default)] pub merged_into: Option<Uuid>,

    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub created_at: DateTime<Utc>,  // ingest time of first asserting episode
    pub updated_at: DateTime<Utc>,  // wall-clock of last mutation

    // Provenance (GOAL-1.3). Stored in join tables; materialized on read.
    #[serde(default)] pub episode_mentions: Vec<Uuid>,
    #[serde(default)] pub memory_mentions:  Vec<String>,

    // Cognitive state (GOAL-1.2)
    pub activation: f64,
    pub agent_affect: Option<serde_json::Value>,  // 11-dim affect tag as JSON
    pub arousal: f64,
    pub importance: f64,
    pub identity_confidence: f64,

    /// Entity-level somatic fingerprint (GOAL-1.13). None until first pass.
    pub somatic_fingerprint: Option<SomaticFingerprint>,
}
```

**Lifecycle.** Created by v03-resolution when Stage 4 decides an incoming mention cannot fuse with any existing canonical id. Updated on every subsequent mention (last_seen, activation, fingerprint recompute). **Never deleted** — a merge loser is retained as an alias pointing to the winner (§3.4, GOAL-1.6). Consolidation (out of scope) decays `activation` and `importance`.

**Why a new type, not an extension of `ExtractedEntity`.** `ExtractedEntity` is a surface match from Aho-Corasick / regex scans — one-write lifespan, no identity beyond `(normalized, entity_type)`. Forcing UUID, bi-temporal state, affect, and fingerprint into it would break every v0.2 consumer. The v0.3 `Entity` subsumes it conceptually but coexists; v03-resolution adapts between them.

### 3.2 Edge (L4)

```rust
// crates/engramai/src/graph/edge.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::graph::entity::Entity;
use crate::graph::schema::Predicate;

/// The "object" side of an edge. v0.3 edges connect a subject entity to
/// either another entity (structural) or a literal value (attribute-like).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EdgeEnd {
    Entity { id: Uuid },
    Literal { value: serde_json::Value },
}

/// How an edge's resolution decision was reached. Drives audit queries
/// (GOAL-1.7) and is one input to `identity_confidence` on the subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMethod {
    /// Resolved by cheap signals alone (string match, embedding, graph context).
    Automatic,
    /// Cheap signals were ambiguous; an LLM tie-breaker was consulted.
    LlmTieBreaker,
    /// Explicitly asserted or corrected by an agent tool call (Letta-style).
    AgentCurated,
    /// Imported from v0.2 data via v03-migration. See `Edge.confidence_source`
    /// for whether the stored `confidence` is recovered, defaulted, or inferred.
    Migrated,
}

/// Provenance of the numeric `confidence` on an edge. Retrieval (v03-retrieval §5)
/// consults this to decide how much to trust the value. On non-`Migrated` edges
/// the default `Recovered` applies — the confidence was produced by the resolver
/// at write time from real signals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceSource {
    /// Confidence was produced by the resolver (or carried from v0.2 unambiguously).
    Recovered,
    /// Confidence could not be recovered; the stored value is a sentinel default
    /// (conventionally 0.5). Retrieval should weight this below `Recovered` evidence.
    Defaulted,
    /// Confidence was inferred from secondary signals (edge age, source kind).
    Inferred,
}

/// A typed, bi-temporal relationship edge (L4).
///
/// Invariants:
///  - `id` is never reassigned or reused after invalidation (GOAL-1.6).
///  - `confidence` in [0.0, 1.0]. v0.3 stores claim-confidence on the edge,
///    not on the MemoryRecord (master §3.2).
///  - `valid_from <= valid_to` when both are present.
///  - `recorded_at` is monotonic per `(subject_id, predicate, object)` triple:
///    a later `recorded_at` is always a newer observation.
///  - Invalidation is non-destructive: setting `invalidated_at` requires
///    `invalidated_by` to point to the successor edge's id, and the successor
///    must carry `supersedes = Some(this.id)` (GOAL-1.6 + GUARD-3).
///  - No field on an invalidated edge is mutated thereafter except for
///    late-arriving `invalidated_by` on chains (see §3.4 chain rules).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,

    pub subject_id: Uuid,
    pub predicate: Predicate,
    pub object: EdgeEnd,

    /// Natural-language rendering, for display and for LLM re-prompting
    /// during Stage 5 resolution. Authored at write time, never re-written.
    pub summary: String,

    // ---- Bi-temporal validity (GOAL-1.5) ----
    /// When the fact became true in the real world.
    pub valid_from: Option<DateTime<Utc>>,
    /// When the fact stopped being true in the real world (None = still true
    /// as far as we know).
    pub valid_to: Option<DateTime<Utc>>,
    /// When the system learned the fact (ingest time of the asserting episode).
    pub recorded_at: DateTime<Utc>,
    /// When the system learned the fact stopped being true.
    pub invalidated_at: Option<DateTime<Utc>>,

    // ---- Invalidation chain (GOAL-1.6) ----
    /// If this edge has been superseded, the successor's id.
    pub invalidated_by: Option<Uuid>,
    /// If this edge supersedes a previous one, the predecessor's id.
    pub supersedes: Option<Uuid>,

    // ---- Provenance (GOAL-1.7) ----
    pub episode_id: Option<Uuid>,
    pub memory_id: Option<String>,
    pub resolution_method: ResolutionMethod,

    // ---- Cognitive state (GOAL-1.4) ----
    pub activation: f64,
    pub confidence: f64,
    /// Provenance of `confidence`. Defaults to `Recovered`; set to `Defaulted`
    /// on `ResolutionMethod::Migrated` edges where the v0.2 source did not
    /// carry a confidence value.
    #[serde(default = "default_confidence_source")]
    pub confidence_source: ConfidenceSource,
    pub agent_affect: Option<serde_json::Value>,

    pub created_at: DateTime<Utc>,
}
```

**Predicate semantics.** A `Predicate` is either canonical or proposed (§3.3). Canonical predicates carry optional inverse/symmetric hints which `GraphStore::traverse` consults; proposed are opaque and never participate in structural traversal (GOAL-1.9).

**Confidence handling.** `Edge.confidence` is write-once at edge creation, sourced from Stage 5 resolution. Later contradicting evidence produces a **new** edge with its own confidence plus sets `invalidated_by` on the old one — it does **not** rewrite the old value (GUARD-3, GOAL-1.6). Read-time "how much do I trust this claim?" is computed downstream (v03-retrieval) from `confidence`, `activation`, and the subject's `identity_confidence`.

**Relation to v0.2 `Triple`.** The v0.2 `Triple` / `Predicate` types in `triple.rs` remain compiled and exported for backward compat (GOAL-1.13). A thin `impl From<&Edge> for Triple` adapter (lossy — drops bi-temporal and audit data) lives in `graph/compat.rs`. A `#[deprecated(note = "use engramai::graph::Edge")]` attribute is applied to `triple::Predicate` without removing it.


### 3.3 Schema Registry

Master §3.5 mandates the hybrid approach: a **canonical** set of predicates with known structural semantics (inverse, symmetric, cardinality hints) plus an **open** space of proposed predicates that preserve LLM-authored labels verbatim. This feature owns the types and the persistence; v03-resolution owns the decision of which bucket a given extraction lands in.

```rust
// crates/engramai/src/graph/schema.rs
use serde::{Deserialize, Serialize};

/// A typed predicate. Either canonical (has structural semantics) or
/// proposed (opaque label, preserved verbatim).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    Canonical(CanonicalPredicate),
    /// LLM- or agent-proposed predicate. Stored as lowercase, whitespace-
    /// normalized string; never participates in inverse/symmetric traversal
    /// (GOAL-1.9). The exact string is preserved (GOAL-1.8 — no info loss).
    Proposed(String),
}

/// Canonical predicates seeded with known semantics. This list is the
/// v0.3 baseline; extending it is a schema change owned by a minor release.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalPredicate {
    // identity / type
    IsA,
    PartOf,
    // social / organizational
    WorksAt,
    MemberOf,
    MarriedTo,
    ParentOf,
    // technical
    DependsOn,
    Uses,
    Implements,
    // causal / temporal
    CausedBy,
    LeadsTo,
    PrecededBy,
    // authored / sourced
    CreatedBy,
    MentionedIn,
    // dialectical
    Contradicts,
    Supports,
    // generic fallback (avoid when possible)
    RelatedTo,
}

/// Structural hint used by GraphStore::traverse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Directionality {
    /// `A p B` implies `B inverse(p) A` — walker can traverse both ways.
    Directed { inverse: Option<CanonicalPredicate> },
    /// `A p B` implies `B p A` — same predicate both directions.
    Symmetric,
}

/// Cardinality hint — purely advisory; not enforced at write time. Used by
/// consolidation/audit (out of scope for this feature) to flag suspicious
/// fan-out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToMany,
}

pub struct PredicateCatalog {
    // Static table built at startup. Exposed as &'static via OnceLock.
}

impl PredicateCatalog {
    pub fn directionality(&self, p: &CanonicalPredicate) -> Directionality { /* ... */ }
    pub fn cardinality(&self, p: &CanonicalPredicate) -> Cardinality { /* ... */ }

    /// Parse an LLM-emitted string into `Predicate`. If it matches a
    /// canonical label (case-insensitive, with snake_case / dash / space
    /// normalized), returns Canonical; otherwise returns Proposed with the
    /// normalized (but preserved) string (GOAL-1.8).
    pub fn classify(&self, raw: &str) -> Predicate { /* ... */ }
}
```

**Evolution rules.**

1. **Canonical is append-only.** Variants may be added in a minor release, never removed or renamed in-place; renames require a new variant + migration (v03-migration).
2. **Proposed is free-form.** Two proposed strings differing only in whitespace/case collapse to one after `classify` normalization; raw form is preserved in `graph_predicates.raw_first_seen` (§4.1) for audit.
3. **Promotion is not automatic.** A proposed predicate is never silently re-classified as canonical. Promotion is an explicit schema operation deferred to v0.4 (ISS-031). This feature only exposes the query surface for drift (GOAL-1.10).

### 3.4 Alias & identity

Every canonical `Entity.id` is a v4 UUID minted at creation time. Mapping from surface strings ("Mel", "Melanie Smith") to the canonical id lives in a dedicated `entity_aliases` table rather than on the entity row, for two reasons: (1) aliases are append-only and benefit from an independent index, and (2) a merge operation (§3.4 below) needs to re-point many aliases atomically without rewriting `Entity` rows.

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityAlias {
    /// Surface form as observed. Case preserved.
    pub alias: String,
    /// Normalized form used for lookup (lowercase, trimmed, NFKC).
    pub normalized: String,
    /// The canonical entity this alias resolves to.
    pub canonical_id: Uuid,
    /// If this alias came from a merge (loser → winner), the loser's original id.
    /// None for aliases that were just additional surface forms.
    pub former_canonical_id: Option<Uuid>,
    pub first_seen: DateTime<Utc>,
    pub source_episode: Option<Uuid>,
}
```

**Resolution lookup order** (enforced by `GraphStore::resolve_alias`):

1. Exact match on `normalized` → return `canonical_id`.
2. Not found → None (the caller — v03-resolution — decides whether to create a new entity or run signal fusion).

**Merge semantics (GOAL-1.6 non-destructive).** When v03-resolution determines entities A and B are the same:

1. Pick a winner (heuristic owned by v03-resolution — typically older or higher `identity_confidence`).
2. Insert an `EntityAlias` row for the loser with `former_canonical_id = Some(loser.id)` and `canonical_id = winner.id`.
3. All the loser's existing aliases are re-pointed: `canonical_id` updated to winner; rows remain.
4. All edges with `subject_id = loser.id` get new edges minted with `subject_id = winner.id` and `supersedes = Some(old)`. Original edges get `invalidated_by = Some(new)`. Audit chain preserved (GUARD-3).
5. The loser `Entity` row is **not** deleted; its first-class `merged_into = Some(winner.id)` field is set (see §3.1 — this is a typed field, not an `attributes` key) and the row is filtered from default traversals. Any caller-authored data in the loser's `attributes` JSON map is preserved untouched; the merge path does not read or write the `attributes` map.

Step 4 can fan out; it runs inside a single SQLite transaction (§4.3). For large merges (>1000 edges), `GraphStore::merge_entities(winner, loser, batch_size)` paginates with resumable semantics.

**Identity confidence.** After merge, winner's `identity_confidence` is recomputed (formula owned by v03-resolution). This feature guarantees only that the pre-merge value is preserved in the winner's typed `history` field (a first-class column on `graph_entities`, not an `attributes` JSON key — see §3.1 and design-r1 #51-52).


## 4. Storage Layer

### 4.1 SQLite schema

Tables are added alongside (not in place of) the existing v0.2 schema in `storage.rs`. All new tables are prefixed `graph_` to avoid collision with the v0.2 `entities` / `entity_relations` tables, which stay untouched for backward compat (NG5).

```sql
-- Canonical graph entities (§3.1).
CREATE TABLE IF NOT EXISTS graph_entities (
    id                  BLOB PRIMARY KEY,                      -- 16-byte UUID
    canonical_name      TEXT NOT NULL,
    kind                TEXT NOT NULL,                         -- serde tag of EntityKind
    summary             TEXT NOT NULL DEFAULT '',
    attributes          TEXT NOT NULL DEFAULT '{}',            -- JSON
    first_seen          REAL NOT NULL,                         -- unix seconds
    last_seen           REAL NOT NULL,
    created_at          REAL NOT NULL,
    updated_at          REAL NOT NULL,
    activation          REAL NOT NULL DEFAULT 0.0,
    agent_affect        TEXT,                                  -- JSON or NULL
    arousal             REAL NOT NULL DEFAULT 0.0,
    importance          REAL NOT NULL DEFAULT 0.3,
    identity_confidence REAL NOT NULL DEFAULT 0.5,
    somatic_fingerprint BLOB,                                  -- 8 * f32 little-endian, or NULL; see blob format note below
    namespace           TEXT NOT NULL DEFAULT 'default',
    history             TEXT NOT NULL DEFAULT '[]',            -- JSON: Vec<HistoryEntry>; typed first-class field (§3.1, design-r1 #51-52)
    merged_into         BLOB REFERENCES graph_entities(id) ON DELETE RESTRICT, -- merge-loser redirect; typed first-class field (§3.1)
    CHECK (activation          BETWEEN 0.0 AND 1.0),
    CHECK (arousal             BETWEEN 0.0 AND 1.0),
    CHECK (importance          BETWEEN 0.0 AND 1.0),
    CHECK (identity_confidence BETWEEN 0.0 AND 1.0),
    CHECK (first_seen <= last_seen),
    CHECK (somatic_fingerprint IS NULL OR length(somatic_fingerprint) = 32)
);
CREATE INDEX IF NOT EXISTS idx_graph_entities_namespace   ON graph_entities(namespace);
CREATE INDEX IF NOT EXISTS idx_graph_entities_kind        ON graph_entities(kind);
CREATE INDEX IF NOT EXISTS idx_graph_entities_last_seen   ON graph_entities(last_seen);

**Blob format note — `somatic_fingerprint`.** The blob is `8 * f32` little-endian (32 bytes exactly). The CHECK constraint above rejects any other length at write time, so a corrupted / truncated blob fails SQLite's constraint before the application sees it. The fixed length is cross-locked with master §3.7 `SomaticFingerprint` (GUARD-7 dimensional lock). If the fingerprint dimension ever changes (e.g., 8 → 10), this is a coordinated change: (a) GUARD-7 is relaxed in master, (b) the CHECK constant is updated here, (c) v03-migration adds a forward pass that re-derives every existing blob under the new dim — old blobs are **not** parseable under a new dim, by design (there is no dimension header; the schema row count + table-level constant is the dimensional oracle). Reader code validates `blob.len() == N * 4` before `from_le_bytes` decoding and returns `GraphError::Invariant("somatic fingerprint dim mismatch")` on mismatch.

-- Aliases (§3.4). Composite PK allows many aliases per entity.
CREATE TABLE IF NOT EXISTS graph_entity_aliases (
    normalized          TEXT NOT NULL,
    canonical_id        BLOB NOT NULL REFERENCES graph_entities(id) ON DELETE CASCADE,
    alias               TEXT NOT NULL,
    former_canonical_id BLOB,                                  -- set on merge
    first_seen          REAL NOT NULL,
    source_episode      BLOB,
    namespace           TEXT NOT NULL DEFAULT 'default',
    PRIMARY KEY (namespace, normalized, canonical_id)
);
CREATE INDEX IF NOT EXISTS idx_graph_aliases_canonical ON graph_entity_aliases(canonical_id);

-- Bi-temporal typed edges (§3.2).
CREATE TABLE IF NOT EXISTS graph_edges (
    id                  BLOB PRIMARY KEY,
    subject_id          BLOB NOT NULL REFERENCES graph_entities(id) ON DELETE RESTRICT,
    predicate_kind      TEXT NOT NULL,                         -- 'canonical' | 'proposed'
    predicate_label     TEXT NOT NULL,                         -- canonical variant name or proposed raw string
    object_kind         TEXT NOT NULL,                         -- 'entity' | 'literal'
    object_entity_id    BLOB    REFERENCES graph_entities(id) ON DELETE RESTRICT,
    object_literal      TEXT,                                  -- JSON; NULL iff object_kind='entity'
    summary             TEXT NOT NULL DEFAULT '',
    valid_from          REAL,
    valid_to            REAL,
    recorded_at         REAL NOT NULL,
    invalidated_at      REAL,
    invalidated_by      BLOB REFERENCES graph_edges(id),
    supersedes          BLOB REFERENCES graph_edges(id),
    episode_id          BLOB,
    memory_id           TEXT REFERENCES memories(id) ON DELETE RESTRICT,
    resolution_method   TEXT NOT NULL,
    activation          REAL NOT NULL DEFAULT 0.0,
    confidence          REAL NOT NULL DEFAULT 0.5,
    agent_affect        TEXT,
    created_at          REAL NOT NULL,
    namespace           TEXT NOT NULL DEFAULT 'default',
    CHECK (activation BETWEEN 0.0 AND 1.0),
    CHECK (confidence BETWEEN 0.0 AND 1.0),
    CHECK (
        (object_kind = 'entity'  AND object_entity_id IS NOT NULL AND object_literal IS NULL) OR
        (object_kind = 'literal' AND object_literal   IS NOT NULL AND object_entity_id IS NULL)
    ),
    CHECK (valid_from IS NULL OR valid_to IS NULL OR valid_from <= valid_to),
    CHECK (predicate_kind IN ('canonical', 'proposed'))
);
CREATE INDEX IF NOT EXISTS idx_graph_edges_subject        ON graph_edges(subject_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_object_entity  ON graph_edges(object_entity_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_predicate      ON graph_edges(predicate_label);
CREATE INDEX IF NOT EXISTS idx_graph_edges_namespace      ON graph_edges(namespace);
CREATE INDEX IF NOT EXISTS idx_graph_edges_recorded_at    ON graph_edges(recorded_at);
CREATE INDEX IF NOT EXISTS idx_graph_edges_invalidated_at ON graph_edges(invalidated_at);
CREATE INDEX IF NOT EXISTS idx_graph_edges_live
    ON graph_edges(subject_id, predicate_label) WHERE invalidated_at IS NULL;
-- Supports `edges_as_of` (§4.2, GOAL-1.5): for each (subject, predicate_label, object)
-- window, pick the latest row with recorded_at <= t. Covers the as-of scan without
-- excluding historical rows (unlike idx_graph_edges_live).
CREATE INDEX IF NOT EXISTS idx_graph_edges_subject_pred_recorded
    ON graph_edges(subject_id, predicate_label, recorded_at DESC);

-- Predicate registry (§3.3). One row per distinct (kind, label) ever seen.
-- Canonical rows are pre-seeded at migration time; proposed rows accrete.
CREATE TABLE IF NOT EXISTS graph_predicates (
    kind            TEXT NOT NULL,                             -- 'canonical' | 'proposed'
    label           TEXT NOT NULL,
    raw_first_seen  TEXT NOT NULL,                             -- original un-normalized form (audit)
    usage_count     INTEGER NOT NULL DEFAULT 0,
    first_seen      REAL NOT NULL,
    last_seen       REAL NOT NULL,
    PRIMARY KEY (kind, label)
);
CREATE INDEX IF NOT EXISTS idx_graph_predicates_usage ON graph_predicates(usage_count DESC);

-- Visible-failure surface for graph extraction (GOAL-1.12 / GUARD-1 / GUARD-2).
CREATE TABLE IF NOT EXISTS graph_extraction_failures (
    id              BLOB PRIMARY KEY,
    episode_id      BLOB NOT NULL,
    stage           TEXT NOT NULL,                             -- 'extraction' | 'entity_resolution' | 'edge_resolution'
    error_category  TEXT NOT NULL,                             -- 'timeout' | 'rate_limit' | 'provider_error' | 'parse_error' | 'other'
    error_detail    TEXT,
    occurred_at     REAL NOT NULL,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    resolved_at     REAL,                                      -- set when a successful re-extraction clears it
    namespace       TEXT NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_extraction_failures_episode    ON graph_extraction_failures(episode_id);
CREATE INDEX IF NOT EXISTS idx_extraction_failures_unresolved
    ON graph_extraction_failures(occurred_at) WHERE resolved_at IS NULL;

-- Minimal MemoryRecord extensions (GOAL-1.11). Implemented as additive ALTERs
-- on the existing `memories` table in v03-migration; the target state is:
ALTER TABLE memories ADD COLUMN episode_id BLOB;      -- FK-style, not enforced
ALTER TABLE memories ADD COLUMN entity_ids TEXT;      -- JSON array of UUIDs
ALTER TABLE memories ADD COLUMN edge_ids   TEXT;      -- JSON array of UUIDs

-- Memory ↔ Entity provenance join (GOAL-1.3, GOAL-1.7).
-- Back-linking is the *authoritative* source for "which entities mention this memory"
-- and "which memories mention this entity". The `memories.entity_ids` JSON column above
-- is a denormalized cache; the join table below is the source of truth (faster indexed
-- range scans, and a true cross-ref usable by retrieval plans).
CREATE TABLE IF NOT EXISTS graph_memory_entity_mentions (
    memory_id       TEXT   NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id       BLOB   NOT NULL REFERENCES graph_entities(id) ON DELETE RESTRICT,
    mention_span    TEXT,                                   -- optional: "[start,end)" in source text
    confidence      REAL   NOT NULL DEFAULT 1.0 CHECK (confidence BETWEEN 0.0 AND 1.0),
    recorded_at     REAL   NOT NULL,
    namespace       TEXT   NOT NULL DEFAULT 'default',
    PRIMARY KEY (memory_id, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_by_memory ON graph_memory_entity_mentions(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_by_entity ON graph_memory_entity_mentions(entity_id);
CREATE INDEX IF NOT EXISTS idx_graph_mem_ent_ns       ON graph_memory_entity_mentions(namespace);

-- L5 Knowledge Topics (GOAL-1.15 structural hook).
-- A Topic is a synthesized L5 view: a cluster of memories + entities summarized by
-- the Knowledge Compiler (v03-resolution §5bis). Topics have their own embedding +
-- BM25-indexed summary so retrieval (v03-retrieval §4.4) can score them directly
-- without re-scanning their source memories on the read path.
--
-- Every topic row is *mirrored* by an `EntityKind::Topic` entity row (§3.1) sharing
-- the same UUID (`topic_id = entity_id`). This keeps topics first-class graph
-- participants (they can be subjects/objects of edges) while also carrying the
-- structured retrieval fields (source set, cluster weights, embedding) that don't
-- belong on `graph_entities`.
CREATE TABLE IF NOT EXISTS knowledge_topics (
    topic_id                BLOB   PRIMARY KEY REFERENCES graph_entities(id) ON DELETE RESTRICT,
    title                   TEXT   NOT NULL,
    summary                 TEXT   NOT NULL,                -- BM25-indexable text
    embedding               BLOB,                            -- f32 array, same dim as memory embeddings; see blob format note
    source_memories         TEXT   NOT NULL DEFAULT '[]',   -- JSON array<MemoryId>
    contributing_entities   TEXT   NOT NULL DEFAULT '[]',   -- JSON array<EntityId>
    cluster_weights         TEXT,                            -- JSON object (affect-weighting record, GOAL-3.7 input)
    synthesis_run_id        BLOB,                            -- FK-style to v03-resolution's compiler run (§5bis)
    synthesized_at          REAL   NOT NULL,
    superseded_by           BLOB   REFERENCES knowledge_topics(topic_id),
    superseded_at           REAL,
    namespace               TEXT   NOT NULL DEFAULT 'default'
);
-- Dimension check for `embedding` is enforced application-side at write time\n-- (not via CHECK, because SQLite CHECK cannot reference a runtime configuration\n-- value; a compile-time literal would fossilize the dim). The writer validates\n-- `blob.len() == current_embedding_dim * 4` before INSERT; reader validates on\n-- decode and returns `GraphError::Invariant(\"knowledge topic embedding dim mismatch\")`\n-- if a stale blob survives a dim change. If/when the memory embedding dim is\n-- ever changed, v03-migration runs a forward pass rewriting every topic blob\n-- under the new dim \u2014 same approach as the `somatic_fingerprint` blob (see\n-- blob format note earlier in this section).
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_ns        ON knowledge_topics(namespace);
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_live
    ON knowledge_topics(namespace, synthesized_at DESC) WHERE superseded_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_knowledge_topics_run       ON knowledge_topics(synthesis_run_id);
-- FTS on summary is created by v03-migration alongside the existing memory FTS index.

-- Pipeline-run ledger: every resolution/synthesis run gets a row for auditing.
-- Owned schema-wise by this feature (graph-write transactions touch it); populated
-- by v03-resolution workers (§5, §5bis).
CREATE TABLE IF NOT EXISTS graph_pipeline_runs (
    run_id          BLOB PRIMARY KEY,
    kind            TEXT NOT NULL,          -- 'resolution' | 'reextract' | 'knowledge_compile'
    started_at      REAL NOT NULL,
    finished_at     REAL,
    status          TEXT NOT NULL,          -- 'running' | 'succeeded' | 'failed' | 'cancelled'
    input_summary   TEXT,                   -- JSON: {episode_ids: [...], memory_count: N, ...}
    output_summary  TEXT,                   -- JSON: {entities_written: N, edges_written: M, topics_written: K, ...}
    error_detail    TEXT,
    namespace       TEXT NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_graph_pipeline_runs_kind   ON graph_pipeline_runs(kind, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_graph_pipeline_runs_status ON graph_pipeline_runs(status) WHERE status != 'succeeded';

-- Per-edge resolution trace (GOAL-1.7 provenance at decision granularity, owned by v03-resolution §7.1).
-- Schema sits here because the write transaction in v03-resolution §3.5 commits it
-- alongside entities/edges; keeping all graph-write schema in one place simplifies migrations.
CREATE TABLE IF NOT EXISTS graph_resolution_traces (
    trace_id        BLOB PRIMARY KEY,
    run_id          BLOB NOT NULL REFERENCES graph_pipeline_runs(run_id) ON DELETE CASCADE,
    edge_id         BLOB          REFERENCES graph_edges(id) ON DELETE CASCADE,
    entity_id       BLOB          REFERENCES graph_entities(id) ON DELETE CASCADE,
    stage           TEXT NOT NULL,          -- 'entity_extract' | 'edge_extract' | 'dedup' | 'persist'
    decision        TEXT NOT NULL,          -- 'new' | 'matched_existing' | 'superseded' | 'merged' | 'rejected'
    reason          TEXT,                   -- human-readable "why this decision"
    candidates      TEXT,                   -- JSON: ranked candidates considered (id, score)
    recorded_at     REAL NOT NULL,
    CHECK (edge_id IS NOT NULL OR entity_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_run   ON graph_resolution_traces(run_id);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_edge  ON graph_resolution_traces(edge_id);
CREATE INDEX IF NOT EXISTS idx_graph_res_traces_ent   ON graph_resolution_traces(entity_id);
```

Foreign keys use `ON DELETE RESTRICT` for entity→edge references because an entity with live edges cannot be hard-deleted under GUARD-3. The merge path (§3.4) routes around this via superseding edges — never by deleting rows.

**`graph_edges.memory_id` FK — note on GUARD-3.** The `memory_id` column on `graph_edges` uses `ON DELETE RESTRICT` to match the entity FK: edges hold provenance links that are part of the audit chain, and dropping a memory would silently erase that provenance (a GUARD-3 violation even if the memory row is nominally "hard-deletable"). In v0.3 memory deletion is not a supported operation — memories are archived, pinned, or demoted, never removed. An edge additionally carries `episode_id` as a weaker back-pointer that survives even if a memory row is ever manually removed out-of-band; retrieval treats `episode_id` as the audit anchor of last resort.

**Namespace lifecycle.** Namespaces appear on every graph_ table and default to `'default'`. They are created **implicitly** on first write (no explicit `create_namespace` call; any insert with a new `namespace` value establishes it). A new trait method `GraphStore::list_namespaces() -> Result<Vec<String>, GraphError>` enumerates them. **Deletion is not supported in v0.3** — operators who need tenant isolation should use a dedicated per-namespace database file (one `Storage` instance per namespace) rather than sharing. **Cross-namespace edges are disallowed**: every `graph_edges` row's `namespace` must match the `namespace` of both its subject and object entities; this is enforced at the trait boundary (write-time validation in `insert_edge` / `apply_graph_delta`, returning `GraphError::Invariant("cross-namespace edge")`). A future release may add a CHECK constraint via trigger once namespaces are guaranteed populated.

**Trace retention.** The `graph_resolution_traces` table grows faster than the graph itself (typically ~3 rows per entity/edge write). For a 50k-memory ingest producing ~100k entities + ~200k edges, first-ingest traces land at ~900k rows and the table grows monotonically thereafter. Retention policy: (a) trace recording is gated by `EngramConfig.graph.record_traces: bool` (default `true` in v0.3 so ingest audit works; operators can disable for high-volume deployments); (b) when enabled, a periodic pruning job (owned by v03-resolution's worker wiring, same cadence as the consistency job above) deletes trace rows whose `run_id` references a `graph_pipeline_runs` row with `status='succeeded'` and `finished_at < now() - retention_days` (default 30 days). Failed-run traces are retained indefinitely for post-mortem. A future "cold-storage archive" hook is out of scope for v0.3; pruning is the baseline mechanism.

**Note on `graph_memory_entity_mentions` vs `memories.entity_ids`.** Both exist intentionally. The join table is the source of truth (indexed range scans in both directions, composable with `graph_edges` queries). The JSON column on `memories` is a denormalized cache for fast single-row reads and v0.2 back-compat. v03-resolution §3.5 commits both in the same transaction; divergence is a GUARD-1/GUARD-2 bug.

**Divergence detection (not just reconciliation).** Reconciliation is owned by v03-migration §3, but detection happens continuously at three layers here: (a) **write-path assertion** — in debug builds, `apply_graph_delta` re-reads both sources after commit and emits `ResourcePressure(subsystem="graph_mention_divergence", utilization=1.0)` if row counts disagree for any touched `memory_id`; (b) **periodic consistency job** — a background task (wired by v03-resolution) samples 1% of recently-written memories per hour, compares the join table against the JSON cache, and emits `ResourcePressure` with `utilization = diverged_count / sampled` if any divergence is found; (c) **read-path opt-in check** — `entities_linked_to_memory` accepts a debug flag `verify_cache: bool` that cross-checks and returns `GraphError::Invariant("mention cache divergence")` on mismatch (used by contract tests). Without at least (a) and (b) running in production, the "source of truth" claim is aspirational. The reconciliation script in v03-migration §3 remains the repair tool; these detectors are what trigger it. (v03-migration §3 owns a reconciliation script).

**Note on `knowledge_topics.topic_id = entity_id`.** Topics are first-class entities (so they can participate in edges, satisfy `list_entities_by_kind(Topic)`, and surface in graph traversal) *and* rows in a structured topic table (so retrieval can index embeddings/summaries directly and read `source_memories` / `cluster_weights` without joins). The bridge is a shared UUID — the `graph_entities` row is the "identity" facet, the `knowledge_topics` row is the "synthesis" facet. Deleting a topic (GUARD-3 never allows true delete; only `superseded_by`) requires both rows; the foreign key enforces the identity direction.

Migration ordering and legacy-data mirror-in are owned by **v03-migration**.

### 4.2 CRUD operations

```rust
// crates/engramai/src/graph/store.rs
use rusqlite::Transaction;
use uuid::Uuid;

use crate::graph::entity::{Entity, EntityKind};
use crate::graph::edge::{Edge, ResolutionMethod};
use crate::graph::schema::Predicate;

/// All graph-layer persistence sits behind this trait so tests can stub it.
pub trait GraphStore {
    // Entity
    fn insert_entity(&mut self, e: &Entity) -> Result<(), GraphError>;
    fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, GraphError>;
    fn update_entity_cognitive(
        &mut self, id: Uuid, activation: f64, importance: f64,
        identity_confidence: f64, agent_affect: Option<serde_json::Value>,
    ) -> Result<(), GraphError>;
    fn touch_entity_last_seen(&mut self, id: Uuid, ts: f64) -> Result<(), GraphError>;
    fn list_entities_by_kind(&self, kind: &EntityKind, limit: usize) -> Result<Vec<Entity>, GraphError>;

    // Alias / identity (§3.4)
    fn upsert_alias(
        &mut self, normalized: &str, alias_raw: &str,
        canonical_id: Uuid, source_episode: Option<Uuid>,
    ) -> Result<(), GraphError>;
    fn resolve_alias(&self, normalized: &str) -> Result<Option<Uuid>, GraphError>;
    fn merge_entities(&mut self, winner: Uuid, loser: Uuid, batch_size: usize)
        -> Result<MergeReport, GraphError>;

    // Edge
    fn insert_edge(&mut self, edge: &Edge) -> Result<(), GraphError>;
    fn get_edge(&self, id: Uuid) -> Result<Option<Edge>, GraphError>;
    /// Invalidate `old` pointing at `successor`; mark `successor.supersedes = old`.
    /// Single transaction; atomic rollback if any invariant breaks.
    fn supersede_edge(&mut self, old: Uuid, successor: &Edge, at: f64) -> Result<(), GraphError>;
    fn edges_of(&self, subject: Uuid, predicate: Option<&Predicate>, include_invalidated: bool)
        -> Result<Vec<Edge>, GraphError>;
    /// As-of query (GOAL-1.5). Edges "believed true" for `subject` at real-world time `at`.
    ///
    /// **Algorithm.** For each `(subject_id, predicate_label, object)` window,
    /// select the row with the largest `recorded_at <= at` such that
    /// `valid_from IS NULL OR valid_from <= at`, `valid_to IS NULL OR valid_to > at`,
    /// and `invalidated_at IS NULL OR invalidated_at > at`. Backed by the compound
    /// index `idx_graph_edges_subject_pred_recorded (subject_id, predicate_label,
    /// recorded_at DESC)` — the scan range is bounded by `subject_id` and the
    /// per-triple pick is an index walk, not a table scan.
    ///
    /// **Performance guidance.** This is a warm-path query, not a hot-retrieval
    /// primitive. Cost is `O(edges_of_subject_at_or_before(at))`. For entities
    /// with deep history (>10k historical edges), callers should cache results at
    /// the query layer — v03-retrieval §4.3 does this per request.
    fn edges_as_of(&self, subject: Uuid, at: f64) -> Result<Vec<Edge>, GraphError>;
    /// BFS over *canonical* predicates only (GOAL-1.9).
    ///
    /// **Contract.**
    /// - **Bounded output:** `max_results` is required; the walker stops as soon
    ///   as that many `(entity, edge)` pairs have been emitted, even if depth
    ///   is not yet exhausted. Recommended default for retrieval: 256.
    /// - **Cycle handling:** the walker maintains a `HashSet<Uuid>` of visited
    ///   entity ids; an entity is visited at most once even when reached via
    ///   multiple edges or symmetric predicates (e.g. `MarriedTo`).
    /// - **Ordering:** BFS by depth; within a depth level, edges are yielded in
    ///   descending `activation` order (ties broken by descending `recorded_at`).
    ///   Gives retrieval a stable, "most salient first" ordering without a sort.
    /// - **Complexity:** `O(max_results)` with the visited-set; never worse than
    ///   `O(sum_of_fanout_up_to_max_depth)` if `max_results` saturates early.
    /// - **Proposed predicates are excluded** (GOAL-1.9); `predicate_filter`
    ///   accepts only canonical variants — a `Predicate::Proposed(_)` in the
    ///   filter returns `GraphError::MalformedPredicate`.
    fn traverse(
        &self,
        start: Uuid,
        max_depth: usize,
        max_results: usize,
        predicate_filter: &[Predicate],
    ) -> Result<Vec<(Uuid, Edge)>, GraphError>;

    // Provenance (GOAL-1.3, GOAL-1.7)
    fn entities_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError>;
    fn edges_in_episode(&self, episode: Uuid) -> Result<Vec<Uuid>, GraphError>;
    fn mentions_of_entity(&self, entity: Uuid) -> Result<EntityMentions, GraphError>;

    // Memory ↔ Entity provenance (GOAL-1.3, GOAL-1.7).
    // These back the `graph_memory_entity_mentions` join table (§4.1) and are the
    // sole write/read path for the memory-level provenance consumed by
    // v03-retrieval §4.3 (Associative plan seed→entity lookup).
    fn link_memory_to_entities(
        &mut self,
        memory_id: &str,
        entity_ids: &[(Uuid, f64, Option<String>)],   // (entity, confidence, mention_span)
        at: f64,
    ) -> Result<(), GraphError>;
    fn entities_linked_to_memory(&self, memory_id: &str) -> Result<Vec<Uuid>, GraphError>;
    fn memories_mentioning_entity(&self, entity: Uuid, limit: usize) -> Result<Vec<String>, GraphError>;
    fn edges_sourced_from_memory(&self, memory_id: &str) -> Result<Vec<Edge>, GraphError>;

    // L5 Knowledge Topics (GOAL-1.15). CRUD matches the `knowledge_topics` table (§4.1).
    // Synthesis logic (choosing what to cluster, computing summaries/embeddings) lives in
    // v03-resolution §5bis; this trait only exposes persistence.
    fn upsert_topic(&mut self, t: &KnowledgeTopic) -> Result<(), GraphError>;
    fn get_topic(&self, id: Uuid) -> Result<Option<KnowledgeTopic>, GraphError>;
    fn list_topics(&self, namespace: &str, include_superseded: bool, limit: usize)
        -> Result<Vec<KnowledgeTopic>, GraphError>;
    fn supersede_topic(&mut self, old: Uuid, successor: Uuid, at: f64) -> Result<(), GraphError>;

    // Pipeline-run ledger (schema in §4.1; owned by v03-resolution workers).
    fn begin_pipeline_run(&mut self, kind: PipelineKind, input_summary: serde_json::Value)
        -> Result<Uuid, GraphError>;
    fn finish_pipeline_run(
        &mut self,
        run_id: Uuid,
        status: RunStatus,
        output_summary: Option<serde_json::Value>,
        error_detail: Option<&str>,
    ) -> Result<(), GraphError>;
    fn record_resolution_trace(&mut self, t: &ResolutionTrace) -> Result<(), GraphError>;

    // Schema registry (GOAL-1.10)
    /// Register a use of a predicate. In production this is typically invoked
    /// inside `apply_graph_delta` via a per-transaction **batched** counter:
    /// the graph writer accumulates predicate-use counts in a `HashMap`
    /// during the transaction and issues one `UPDATE graph_predicates SET
    /// usage_count = usage_count + :n, last_seen = :ts WHERE ...` per
    /// distinct predicate at commit time. This avoids turning the tiny
    /// canonical-predicate set (~20 rows) into a write hotspot under
    /// high-throughput ingest, while keeping `usage_count` exactly accurate.
    /// The single-call variant below is provided for tests and for callers
    /// that do not go through `apply_graph_delta`.
    fn record_predicate_use(&mut self, p: &Predicate, raw: &str, at: f64) -> Result<(), GraphError>;
    fn list_proposed_predicates(&self, min_usage: u64) -> Result<Vec<ProposedPredicateStats>, GraphError>;

    // Visible failures (GOAL-1.12)
    fn record_extraction_failure(&mut self, f: &ExtractionFailure) -> Result<(), GraphError>;
    fn list_failed_episodes(&self, unresolved_only: bool) -> Result<Vec<Uuid>, GraphError>;
    fn mark_failure_resolved(&mut self, failure_id: Uuid, at: f64) -> Result<(), GraphError>;

    // Transaction escape hatch
    /// **Warning:** this API exposes a raw `&Transaction<'_>` and permits
    /// arbitrary SQL — including writes against any table in the database,
    /// not just graph tables. It is **not audited against GUARDs**: a caller
    /// can trivially violate GUARD-3 (e.g. `DELETE FROM graph_edges WHERE ...`)
    /// through this hatch. Prefer the typed methods above.
    ///
    /// Intended only for (a) advanced test fixtures that need to set up
    /// pathological states, and (b) one-off migration scripts owned by
    /// v03-migration. Production code paths must not use `with_transaction`.
    /// Consider pairing uses with a telemetry `Invariant` signal.
    fn with_transaction<F, R>(&mut self, f: F) -> Result<R, GraphError>
    where F: FnOnce(&Transaction<'_>) -> Result<R, GraphError>;
}

#[derive(Debug, Clone)]
pub struct MergeReport { pub edges_superseded: u64, pub aliases_repointed: u64 }

#[derive(Debug, Clone)]
pub struct EntityMentions { pub episode_ids: Vec<Uuid>, pub memory_ids: Vec<String> }

#[derive(Debug, Clone)]
pub struct ProposedPredicateStats { pub label: String, pub usage_count: u64 }

#[derive(Debug, Clone)]
pub struct ExtractionFailure {
    pub id: Uuid,
    pub episode_id: Uuid,
    pub stage: &'static str,
    pub error_category: &'static str,
    pub error_detail: Option<String>,
    pub occurred_at: f64,
}

/// L5 Knowledge Topic — structured synthesis row (§4.1 `knowledge_topics`).
/// Mirrors a `graph_entities` row of kind `Topic` via shared UUID; see §4.1 note.
#[derive(Debug, Clone)]
pub struct KnowledgeTopic {
    pub topic_id: Uuid,                          // == entity_id of the mirrored Topic entity
    pub title: String,
    pub summary: String,
    pub embedding: Option<Vec<f32>>,
    pub source_memories: Vec<String>,            // MemoryIds the topic synthesizes over
    pub contributing_entities: Vec<Uuid>,        // entities that appear across source_memories
    pub cluster_weights: Option<serde_json::Value>, // affect-weighting record (GOAL-3.7 input)
    pub synthesis_run_id: Option<Uuid>,          // back-link to graph_pipeline_runs
    pub synthesized_at: f64,
    pub superseded_by: Option<Uuid>,
    pub superseded_at: Option<f64>,
    pub namespace: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PipelineKind { Resolution, Reextract, KnowledgeCompile }

#[derive(Debug, Clone, Copy)]
pub enum RunStatus { Running, Succeeded, Failed, Cancelled }

/// Per-decision record written during a resolution or synthesis run.
/// One row per {entity, edge} touched by the run. Indexed by `run_id` (audit a run)
/// and by `edge_id` / `entity_id` (trace back from a single graph object).
#[derive(Debug, Clone)]
pub struct ResolutionTrace {
    pub trace_id: Uuid,
    pub run_id: Uuid,
    pub edge_id: Option<Uuid>,
    pub entity_id: Option<Uuid>,
    pub stage: &'static str,                     // 'entity_extract' | 'edge_extract' | 'dedup' | 'persist'
    pub decision: &'static str,                  // 'new' | 'matched_existing' | 'superseded' | 'merged' | 'rejected'
    pub reason: Option<String>,
    pub candidates: Option<serde_json::Value>,   // ranked candidates considered
    pub recorded_at: f64,
}
```

The production impl is `SqliteGraphStore`, wrapping `&mut rusqlite::Connection`. Trait is object-safe (`dyn GraphStore`) for test injection.

### 4.3 Transaction boundaries

Three atomicity rules:

**Rule A — single-episode write is one transaction.** An episode's full graph output (N entity upserts + M edge inserts + alias upserts + supersede calls) executes inside one `with_transaction` block driven by v03-resolution. If any step fails, the whole transaction rolls back *and* an `ExtractionFailure` row is committed in a **separate** follow-up transaction so the failure is durable (GUARD-1/GUARD-2 — visibility cannot itself be lost). The L1/L2 write (master INV2) has **already** committed in an earlier transaction owned by the episode-admit path, so a graph failure never takes the episodic trace down with it (GOAL-1.11/1.12).

**Rule B — merges are chunked transactions.** `merge_entities` takes a `batch_size` and commits each batch. Intermediate states are audit-consistent: a superseded edge always has a valid successor. Callers resume by calling `merge_entities` again with the same `(winner, loser)` — idempotent.

**Rule C — read paths do not open write transactions.** `get_entity`, `edges_of`, `traverse`, etc. use read-only deferred transactions (reader-concurrent with writers under WAL — see §8).

**Cross-cutting with L2/L3.** A graph write and a `MemoryRecord.entity_ids`/`edge_ids` update share the same rusqlite `Connection`, so v03-resolution can bundle them into one transaction. The MemoryRecord row is created by the episodic admit path *before* graph extraction runs, so the cross-reference update is always UPDATE, never INSERT (avoids ordering deadlocks).


## 5. Public API Surface

Backward compat is the non-negotiable design constraint here (GOAL-1.13). The v0.2 surface is preserved verbatim:

```rust
// `Memory` impl — v0.2 surface (UNCHANGED, still works). (The public holder type in
// `engramai/src/memory.rs` is `Memory`; earlier drafts said `Engram` — pre-canonical.
// See v03-retrieval §6.1 GUARD-11 note for the canonical naming table.)
impl Memory {
    pub fn store_raw(&mut self, content: &str, kind: MemoryType) -> Result<String, EngramError>;
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<MemoryRecord>, EngramError>;
    pub fn recall_recent(&self, limit: usize) -> Result<Vec<MemoryRecord>, EngramError>;
    pub fn recall_associated(&self, memory_id: &str, limit: usize) -> Result<Vec<MemoryRecord>, EngramError>;
    // ... etc.
}
```

New v0.3 additions are **added methods on the existing `Memory` struct** plus a `graph()` accessor that hands out a borrowed `&dyn GraphStore` for advanced callers:

```rust
impl Memory {
    // ---- Episode anchor (underpins graph provenance) ----
    pub fn add_episode(&mut self, ep: Episode) -> Result<Uuid, EngramError>;

    // ---- Direct graph access (advanced callers / tools / agents) ----
    pub fn graph(&self) -> &dyn GraphStore;
    pub fn graph_mut(&mut self) -> &mut dyn GraphStore;

    // ---- Convenience: high-level graph queries ----
    pub fn get_entity(&self, id: Uuid) -> Result<Option<Entity>, EngramError>;
    pub fn find_entity(&self, name: &str) -> Result<Option<Uuid>, EngramError>;
    pub fn neighbors(
        &self,
        entity: Uuid,
        max_depth: usize,
    ) -> Result<Vec<(Uuid, Edge)>, EngramError>;
    pub fn edges_as_of(&self, entity: Uuid, at: DateTime<Utc>) -> Result<Vec<Edge>, EngramError>;

    // ---- Failure surface (GOAL-1.12) ----
    pub fn list_failed_episodes(&self) -> Result<Vec<Uuid>, EngramError>;
    /// Signal to v03-resolution: re-attempt extraction on the given episodes.
    /// This method is a thin shim; the actual retry logic lives in v03-resolution.
    pub fn reextract_episodes(&mut self, eps: &[Uuid]) -> Result<ReextractReport, EngramError>;
}
```

**Compat guarantees.**

1. `ExtractedEntity`, `EntityType`, `EntityExtractor`, `EntityConfig` in `entities.rs` — unchanged, unmoved, unrenamed. Still re-exported from the crate root.
2. `Triple`, `Predicate` (v0.2), `TripleSource` in `triple.rs` — unchanged. A `#[deprecated]` attribute is applied to `triple::Predicate` only (not the `Triple` struct, because v0.2 consumers may hold `Vec<Triple>` directly). The v0.3 `graph::schema::Predicate` is a distinct type at a distinct path.
3. `Storage::entities(...)` and related v0.2 methods — untouched. The new graph tables live behind `Storage::graph(&mut self) -> SqliteGraphStore<'_>`.

No v0.2 type is re-exported from `graph::`; no v0.3 type shadows a v0.2 name. A downstream crate can `use engramai::*` before and after the upgrade and get the same symbols.

### 5bis. Cross-Feature Handoff Types (r3)

This section defines types that **this feature owns** and exposes to v03-resolution / v03-migration. Without these, those features cannot close their contracts.

#### `GraphDelta` — Batched, Atomic Graph Write Unit

**Motivation (v03-migration §5.2 + v03-resolution §6.5).** Both the hot-write path and migration need to stage a complete set of graph mutations for one `MemoryRecord` and commit them atomically. `GraphDelta` is the staging container.

```rust
/// The complete graph-state change produced by resolving one memory.
/// Constructed by v03-resolution (`resolve_for_backfill` returns one, the
/// normal pipeline constructs one internally). Consumed by
/// `GraphStore::apply_graph_delta` — the only write path that accepts it.
///
/// A `GraphDelta` is **self-contained**: applying it to an empty or partially
/// populated graph produces the same end state regardless of prior content,
/// as long as referenced entity ids resolve. This property is what makes
/// backfill idempotent (v03-migration §5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphDelta {
    /// The memory this delta was produced for (back-reference; the delta
    /// itself does not mutate the `memories` row, but `apply_graph_delta`
    /// updates `memories.entity_ids` / `memories.edge_ids` atomically).
    pub memory_id: String,

    /// Entities to upsert. `method` field on each entity indicates how it
    /// was produced (New | Merged | Migrated | ...). Keyed by canonical
    /// name + kind for dedup — re-running migration produces the same
    /// entity ids for the same inputs (idempotence).
    pub entities: Vec<Entity>,

    /// Entity merges to perform as part of this delta (winner, loser).
    /// Applied before `entities` upserts so loser ids in `edges` below
    /// are remapped to winner ids.
    pub merges: Vec<EntityMerge>,

    /// Edges to insert. Edge endpoints must reference entities either in
    /// `self.entities` or already present in the store. Bi-temporal fields
    /// (valid_from / valid_to / recorded_at) are pre-populated.
    pub edges: Vec<Edge>,

    /// Edges to invalidate (set `valid_to = now()`). Used by
    /// §3.4.4 retro-evolution rules. Preserves the old edge row per
    /// GUARD-3 (no erasure).
    pub edges_to_invalidate: Vec<EdgeInvalidation>,

    /// Memory-to-entity mention rows to insert into
    /// `graph_memory_entity_mentions`. Populated by v03-resolution
    /// §3.5 persist stage.
    pub mentions: Vec<MemoryEntityMention>,

    /// Predicate registrations to make against the schema registry
    /// (§3.3.2 proposed-predicate path). Empty if all predicates used
    /// are already canonical.
    pub proposed_predicates: Vec<ProposedPredicate>,

    /// If resolution recorded any stage failures (GOAL-1.12), they are
    /// carried in the delta so persistence lands them in
    /// `graph_extraction_failures` as part of the same transaction.
    pub stage_failures: Vec<StageFailureRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMerge {
    pub winner: Uuid,
    pub loser: Uuid,
    pub reason: MergeReason,  // defined in §3.4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeInvalidation {
    pub edge_id: Uuid,
    pub invalidated_at: f64,            // unix seconds
    pub superseded_by: Option<Uuid>,    // optional pointer to replacement
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntityMention {
    pub memory_id: String,
    pub entity_id: Uuid,
    pub mention_text: String,
    pub span_start: Option<u32>,
    pub span_end: Option<u32>,
    pub confidence: f64,
}
```

**Invariants (owned by this feature; callers may rely on them):**

1. **Referential integrity.** Every `entity_id` appearing in `edges`, `mentions`, or `merges` either (a) appears in `self.entities`, or (b) already exists in the store when `apply_graph_delta` runs. Violation = `GraphError::DanglingReference`, whole delta rolls back.
2. **No duplicate merges.** Each `loser` appears in `merges` at most once.
3. **Bi-temporal consistency.** For every edge in `edges_to_invalidate`, the target edge exists and is currently live (`valid_to IS NULL`). Re-invalidating an already-invalid edge is an error (migration resume uses idempotent semantics through `apply_graph_delta`, not through double-invalidation).
4. **Determinism.** Serializing a `GraphDelta` to JSON and back produces an equal delta (`PartialEq`). This is why `HashMap` is not used in the struct — all collections are `Vec` with stable order.

#### `GraphStore::apply_graph_delta` — Atomic Persistence

```rust
pub trait GraphStore {
    // ... existing methods unchanged ...

    /// Apply a `GraphDelta` atomically. Either every mutation in the delta
    /// lands (entities upserted, merges applied, edges inserted, invalidations
    /// set, mentions inserted, predicates registered, failures recorded, and
    /// `memories.entity_ids` / `memories.edge_ids` updated), or none do.
    ///
    /// Wraps all operations in a single SQLite transaction using the same
    /// `Storage::with_graph_tx` boundary described in §4.3.
    ///
    /// Idempotence (v03-migration §5.2 requirement): applying the same delta
    /// twice is a no-op on the second call — returns `Ok(ApplyReport {
    /// already_applied: true, .. })` without re-executing any mutation.
    /// Idempotence key: `(memory_id, delta_content_hash, schema_version)`
    /// persisted in the `graph_applied_deltas` table (new; schema addition
    /// noted in §4.1 follow-up).
    ///
    /// **Crash-recovery guarantee.** The `graph_applied_deltas` row is inserted
    /// **inside the same SQLite transaction** as the entity/edge/mention
    /// writes. There is no window where graph state is advanced but the
    /// idempotence row is missing (or vice versa). If the process crashes
    /// mid-apply, the transaction rolls back per §4.3 Rule A and the next
    /// call replays cleanly. If the process crashes after commit but before
    /// any external acknowledgement, the next call observes `already_applied=true`
    /// and no row is duplicated. Contract test in v03-migration §5.2 kills the
    /// process between final commit and acknowledgement to verify this.
    ///
    /// Performance: `apply_graph_delta` is the preferred write API for any
    /// caller that has a fully-staged delta. Stitching individual
    /// `insert_entity` / `insert_edge` / ... calls from outside the trait is
    /// supported but opens multiple transactions.
    fn apply_graph_delta(
        &mut self,
        delta: &GraphDelta,
    ) -> Result<ApplyReport, GraphError>;
}

#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub already_applied: bool,    // true iff idempotence short-circuit fired
    pub entities_upserted: u32,
    pub entities_merged: u32,
    pub edges_inserted: u32,
    pub edges_invalidated: u32,
    pub mentions_inserted: u32,
    pub predicates_registered: u32,
    pub failures_recorded: u32,
    pub tx_duration_us: u64,
}
```

**Contract with v03-migration (§5.2 handoff).**
- `apply_graph_delta` is the sole write entry point migration uses per-record. Migration never calls individual CRUD methods during backfill.
- Idempotence means checkpoint resume after a crash is a replay — migration reads its last-committed `(memory_id, delta_hash)` checkpoint and retries from there; any already-applied delta is skipped cheaply.
- `ApplyReport.already_applied` lets migration distinguish "new work done" from "replay no-op" in its progress telemetry.

**Contract with v03-resolution (§3.5, §6.5 handoff).**
- The normal pipeline's persist stage (§3.5) stages a `GraphDelta` and calls `apply_graph_delta`. This unifies the hot-write and backfill paths on a single atomic boundary — migration and new-ingest cannot diverge in transactional semantics.
- Callers of `resolve_for_backfill` (v03-resolution §6.5) receive the delta and are expected to call `apply_graph_delta` themselves; resolution does not implicitly persist in backfill mode.

**Schema addition note (§4.1 follow-up).** The idempotence table:

```sql
CREATE TABLE graph_applied_deltas (
    memory_id       TEXT NOT NULL,
    delta_hash      BLOB NOT NULL,      -- BLAKE3 of canonical JSON serialization (see below)
    schema_version  INTEGER NOT NULL,   -- GraphDelta schema version at apply time
    applied_at      REAL NOT NULL,
    report          TEXT NOT NULL,      -- JSON of ApplyReport
    PRIMARY KEY (memory_id, delta_hash, schema_version)
);
CREATE INDEX idx_applied_deltas_memory ON graph_applied_deltas(memory_id);
```

This is additive (no migration of existing tables). First call to `apply_graph_delta` on an upgraded DB lazily creates the table if missing.

**Crash-recovery & idempotence invariant.** The `graph_applied_deltas` row is written **inside the same SQLite transaction** as all entity, edge, mention, predicate-registration, and failure writes for the delta. There is no separate "finalize" transaction. Consequence: a crash at any point prior to commit leaves zero rows persisted (per §4.3 Rule A rollback), so a replay sees `already_applied = false` and re-runs from clean state; a crash after commit leaves the idempotence row *plus* all the corresponding data rows, so a replay short-circuits with `already_applied = true`. There is no intermediate state in which the data rows exist but the idempotence row does not, or vice versa — this is what makes `apply_graph_delta` safe to call after unplanned termination (migration resume, hot-write retry). A contract test (test-plan bullet): kill the process between the final `COMMIT` and any external acknowledgement; restart; re-call `apply_graph_delta` with the identical delta; assert `already_applied = true` and zero duplicate rows in any `graph_*` table touched by the delta.

**Canonical serialization of `GraphDelta` for `delta_hash`.** To make the idempotence key stable across processes and versions, the hash input is **not** raw `serde_json::to_vec(delta)`. It is a rigorously defined canonical form:

1. **Keys sorted lexicographically** (byte order) at every object level.
2. **No whitespace** between tokens; no trailing newline.
3. **Floats** serialized in shortest-roundtrip form (Rust's `{}` formatter on `f64::to_string` equivalent), with `-0.0` normalized to `0.0` and `NaN`/`±Inf` rejected (error — they should never appear in a delta).
4. **Integers** serialized as integers (no `1.0` for an integer-valued `f64`; the struct uses integer types where applicable to avoid this ambiguity).
5. **UUIDs** serialized as their canonical hyphenated lowercase string form.
6. **Hash is taken over a frozen subset** of fields listed in the table below — *not* the full `GraphDelta`. New fields added in future versions are excluded from the hash until explicitly promoted, preventing silent hash drift.

| Field included in hash                | Rationale                                         |
|---------------------------------------|---------------------------------------------------|
| `memory_id`                           | Identity of the write target                      |
| `entities[].id, canonical_name, kind` | Identity of each upserted entity                  |
| `merges[].winner, loser`              | Identity of each merge                            |
| `edges[].id, subject_id, predicate, object, valid_from, valid_to, recorded_at` | Identity + bi-temporal stamp of each edge |
| `edges_to_invalidate[].edge_id, invalidated_at` | Identity of each invalidation              |
| `mentions[].memory_id, entity_id`     | Identity of each mention                          |
| `proposed_predicates[].label`         | Identity of each registered predicate             |

All other fields (cognitive state, summaries, confidences, affect) are **not** in the hash: they are derived/refinable and may legitimately change between runs without changing delta identity.

**Hash stability rules:**

- `#[serde(deny_unknown_fields)]` is applied to `GraphDelta`, `EntityMerge`, `EdgeInvalidation`, and `MemoryEntityMention`. Any new field addition is a breaking change that requires bumping `SCHEMA_VERSION` and writing a migration of `graph_applied_deltas`.
- A compile-time constant `pub const GRAPH_DELTA_SCHEMA_VERSION: u32 = 1;` lives alongside the struct. It is written to the `schema_version` column on every apply. The idempotence short-circuit in `apply_graph_delta` matches on *all three* PK columns — a row written under v1 will **not** short-circuit a v2 replay; migration of the table is required when bumping the version.
- Cross-version replays surface explicitly: if `apply_graph_delta` finds a row with the same `(memory_id, delta_hash)` but a different `schema_version`, it returns `GraphError::Invariant("delta schema version mismatch")` rather than silently re-running or no-op'ing. The caller (migration) decides whether to re-hash under the new schema and retry.

## 6. Telemetry Integration

Master §3.7 pins graph operations to the **Telemetry** layer (the agent's body signals), specifically two signal families:

| Graph op                              | Signal emitted      | Semantics                                                    |
|---------------------------------------|---------------------|--------------------------------------------------------------|
| `insert_entity`, `insert_edge`        | `OperationalLoad`   | +1 unit of write work; aggregator decays per master §3.7     |
| `merge_entities` (per batch)          | `OperationalLoad`   | +N units where N = batch size; flagged as "consolidation"    |
| `traverse` with `max_depth > k`       | `OperationalLoad`   | +1 unit; marks deep traversal for potential budget throttle  |
| Table row count crosses a watermark   | `ResourcePressure`  | hint: "entities" or "edges" count is approaching a soft cap  |
| `record_extraction_failure`           | `OperationalLoad`   | +1 unit; a failure is still work — visibility is the point   |

**What is *not* emitted.** The graph module never emits Affect signals (`ValenceShift`, `ArousalSpike`, etc.) — those are Affect-owned per master §3.7 boundary rule #1. The graph module never consumes any signal — it is a pure publisher. This asymmetry is what lets the dependency graph stay acyclic: `graph` → `telemetry_bus`, never the reverse.

**Emission mechanism.** A thin `TelemetrySink` trait is injected into `SqliteGraphStore` at construction; in production it forwards to the global telemetry bus, in tests it is a `Vec` collector. No direct global state is read from inside the graph module.

```rust
pub trait TelemetrySink: Send + Sync {
    fn emit_operational_load(&self, op: &'static str, units: u32);
    fn emit_resource_pressure(&self, subsystem: &'static str, utilization: f64);
}
```

Watermark thresholds (when to emit `ResourcePressure`) are advisory; initial values are 100k entities / 500k edges per namespace and are configurable via `EngramConfig.graph.pressure_thresholds`. Cross-referenced from master §6 (consolidation): these signals are inputs that consolidation listens to when deciding how aggressively to decay and compact.

## 7. Error Model

```rust
#[derive(thiserror::Error, Debug)]
pub enum GraphError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("entity not found: {0}")]
    EntityNotFound(Uuid),

    #[error("edge not found: {0}")]
    EdgeNotFound(Uuid),

    #[error("invariant violation: {0}")]
    Invariant(&'static str),

    /// Attempted to mutate an invalidated edge (GUARD-3 / GOAL-1.6).
    #[error("edge {0} is invalidated and cannot be modified")]
    EdgeFrozen(Uuid),

    /// Attempted to delete an entity with live edges.
    #[error("entity {0} has live edges; merge or supersede instead")]
    EntityHasLiveEdges(Uuid),

    /// Predicate classify failed (should be infallible, but kept explicit).
    #[error("malformed predicate: {0}")]
    MalformedPredicate(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("transaction was rolled back: {0}")]
    Rolledback(String),
}
```

**Recovery strategies.**

- `Sqlite(SQLITE_BUSY)` — retried by the caller with the existing 5000 ms `busy_timeout` PRAGMA in `storage.rs`. After retry exhaustion, surfaces to v03-resolution, which records an `ExtractionFailure` and returns control.
- `EdgeFrozen` / `EntityHasLiveEdges` — programming errors, not runtime conditions; they should never occur in production code paths and indicate a bug in v03-resolution. Logged at `error` level.
- `Invariant` — indicates data corruption or a concurrency bug. Emits an `OperationalLoad` burst with op = `"graph_invariant_violation"` so the telemetry layer can flag it. The call is aborted; no partial write is committed (transaction rollback).

**GUARD compliance.**
- **GUARD-1 (episodic completeness):** graph errors never roll back the L1/L2 write because that commit happened in an earlier transaction (§4.3 Rule A).
- **GUARD-2 (never silent degrade):** every failure either (a) is returned up the stack or (b) lands in `graph_extraction_failures` — never both suppressed.
- **GUARD-3 (bi-temporal never erases):** `EdgeFrozen` enforces this at the trait boundary; no code path exists that can set `confidence` or `valid_to` on an edge whose `invalidated_at IS NOT NULL`.

## 8. Concurrency & Consistency

**Writer model: single-writer, multi-reader.** SQLite in WAL mode (already enabled by `storage.rs` via `PRAGMA journal_mode=WAL`) gives lock-free reader concurrency against a single writer. The graph module inherits the connection discipline of the enclosing `Storage`; no new concurrency primitives. Cross-process access serializes on SQLite's file lock with the existing 5000 ms `busy_timeout`.

**Reader semantics during ingest.** A reader sees pre-commit state for the duration of a writer transaction (§4.3 Rule A). Since the write is atomic over entity + edges, a reader never observes an edge whose subject or object doesn't exist. `edges_as_of(t)` is evaluated against currently-committed state; in-flight async resolution is not yet visible (correct semantics — "what do I believe right now as of time t?", not "what will I believe once resolution finishes?").

**Activation updates are eventually consistent.** `update_entity_cognitive` (driven by consolidation, out of scope) races with reads at row level; rusqlite's row-level isolation is sufficient because activation is a scalar that converges — no cross-row invariant depends on it.

**Merge atomicity.** `merge_entities` is the only operation that violates the single-transaction rule (§4.3 Rule B). Between batches, a reader may see a partially-merged state. See **Reader semantics during merge** below for the precise contract.

**Reader semantics during merge.** For large merges that span multiple batch transactions, the single-transaction illusion does not hold — but the following contract does:

1. **Redirect signal lands first.** The loser's typed `merged_into = Some(winner.id)` field (a first-class column on `graph_entities`, not an `attributes` JSON key — see §3.1 and design-r1 #51-52) is set in the merge's **first** batch transaction. Readers doing entity-level lookups on the loser id MUST check `merged_into` and transparently follow the redirect; `get_entity` in this trait does this automatically. The loser row itself is filtered from `list_entities_by_kind` and from default traversals.
2. **Edges are transiently double-visible by design.** Between batches, un-superseded loser edges remain live (they have not yet been rewritten), *and* the already-rewritten batches appear under the winner. A naïve `edges_of(loser, include_invalidated=false) ∪ edges_of(winner, include_invalidated=false)` can therefore return the same *fact* twice during a merge — once keyed by loser, once by winner.
3. **De-duplication is the reader's responsibility.** Any consumer that aggregates across subject ids during a merge window MUST de-duplicate by `(predicate, object)` after redirecting loser → winner via rule 1. v03-retrieval's Associative plan applies this de-dup unconditionally; ranking contract tests treat any `(predicate, object)` collision as a single piece of evidence.
4. **No missing-edge window.** The design guarantees no edge is *invisible* at any intermediate point: a loser edge is only marked `invalidated_at` in the same batch transaction that inserts its winner-side successor, so one or the other is always live and queryable.
5. **Consistency mode for callers that need snapshot semantics.** Callers that cannot tolerate transient double-visibility (offline analytics, audit exports) must either (a) check `list_running_merges()` and defer, or (b) call `edges_of` with `include_invalidated=true` and reconstruct the as-of state via `edges_as_of(now)` — which filters by `invalidated_at` and gives a consistent post-merge view.

Option (a) — a DB-level read lock for the merge duration — was considered and rejected because it forfeits the WAL concurrency win for a consistency property (de-dup by `(predicate, object)`) that consumers already need for *non-merge* reasons (the same fact can be extracted from two episodes).

**FK enforcement.** `PRAGMA foreign_keys=ON` already set in `Storage::new`. Graph FKs declared `ON DELETE RESTRICT` on both entity and memory references (see §4.1 note on `graph_edges.memory_id`: audit provenance is part of the GUARD-3 chain and cannot be silently dropped by deleting a memory). No `ON DELETE CASCADE` on edges — cascading deletes would silently erase audit history (GUARD-3).


## 9. Requirements Traceability

| GOAL       | Priority | Satisfied by section(s)                          | Notes                                                                                   |
|------------|----------|--------------------------------------------------|-----------------------------------------------------------------------------------------|
| GOAL-1.1   | P0       | §3.1 (Entity struct, invariants)                 | `id`, `canonical_name`, aliases (§3.4), `kind`, `first_seen`/`last_seen`, `summary`     |
| GOAL-1.2   | P0       | §3.1 (cognitive-state fields); §4.1 CHECKs       | `activation`, `agent_affect`, `arousal`, `importance`, `identity_confidence` all present |
| GOAL-1.3   | P0       | §3.1 (`episode_mentions`/`memory_mentions`); §4.1 `graph_memory_entity_mentions`; §4.2 `entities_in_episode`, `mentions_of_entity`, `link_memory_to_entities`, `entities_linked_to_memory`, `memories_mentioning_entity` | Bidirectional queries exposed on `GraphStore`; memory↔entity join is the source of truth for retrieval's Associative plan (v03-retrieval §4.3) |
| GOAL-1.4   | P0       | §3.2 (Edge struct, EdgeEnd, cognitive fields)    | Literal-or-entity object, typed predicate, activation/confidence/affect                 |
| GOAL-1.5   | P0       | §3.2 (bi-temporal fields); §4.2 `edges_as_of`    | `valid_from`, `valid_to`, `recorded_at`, `invalidated_at`                                |
| GOAL-1.6   | P0       | §3.2 invariants; §3.4 merge; §4.2 `supersede_edge`; §4.1 (no `ON DELETE CASCADE` on edges) | Invalidation non-destructive; chain traversable via `invalidated_by` / `supersedes`     |
| GOAL-1.7   | P1       | §3.2 (`episode_id`, `memory_id`, `resolution_method`); §4.1 `graph_memory_entity_mentions`, `graph_resolution_traces`; §4.2 `edges_in_episode`, `edges_sourced_from_memory`, `record_resolution_trace` | Provenance queryable both directions at episode, memory, and decision granularity |
| GOAL-1.8   | P0       | §3.3 (Predicate enum with Canonical vs Proposed); §4.1 `graph_predicates` | Verbatim preservation via `raw_first_seen`; distinguishable in all queries via `kind`   |
| GOAL-1.9   | P1       | §3.3 evolution rule #1; §4.2 `traverse` docstring | `traverse` accepts canonical predicates only; proposed never enter structural logic     |
| GOAL-1.10  | P2       | §4.2 `list_proposed_predicates`; §4.1 `graph_predicates.usage_count`            | Operators can monitor drift; promotion itself deferred to v0.4                         |
| GOAL-1.11  | P0       | §4.1 `ALTER TABLE memories ADD episode_id/entity_ids/edge_ids`; §5 compat clause | Additive; v0.2 consumers unaffected                                                    |
| GOAL-1.12  | P0       | §4.1 `graph_extraction_failures`; §4.2 `record_extraction_failure` / `list_failed_episodes` / `mark_failure_resolved`; §5 `reextract_episodes`; §4.3 Rule A separate-transaction commit of failures | Visible, queryable, re-targetable                                                      |
| GOAL-1.13  | P0       | §3.1 (two-fingerprint rationale); §3.2; §4.1 `somatic_fingerprint BLOB` on `graph_entities`; master §3.7 for schema lock | Episode-level immutability owned by Episode (master §3.1); entity-level aggregate stored here |
| GOAL-1.14  | P1       | (derivation rule, see note below)                | Not a stored column. `MemoryLayer` computed from `working_strength` / `core_strength` / `pinned` via a pure fn `classify_layer(&MemoryRecord) -> MemoryLayer`. Same inputs → same output. |
| GOAL-1.15  | P1       | §3.1 `EntityKind::Topic`; §4.1 `knowledge_topics` table; §4.2 `upsert_topic` / `get_topic` / `list_topics` / `supersede_topic`; `KnowledgeTopic` struct | Topics are first-class **in both facets**: identity (entity row, edges-capable) and synthesis (topic row, embedding + source set + cluster weights). Synthesis itself (what to cluster) lives in v03-resolution §5bis (Knowledge Compiler). Retrieval consumes the topic row directly (v03-retrieval §4.4). |

**Note on GOAL-1.14.** The current v0.2 `memories.layer` column is retained in the schema (for back-compat reads) but is demoted to a derived-cache role in v0.3: writes to `MemoryRecord` no longer set it as a source-of-truth field — instead, the pure function `fn classify_layer(r: &MemoryRecord) -> MemoryLayer` computes it from `working_strength`, `core_strength`, and `pinned` at read time, and the column is kept in sync as an index-friendly denormalization. The final derivation formula is owned by the consolidation feature (out of scope for v0.3); this document provides a **provisional formula** so GOAL-1.14 is testable in v0.3 without blocking on consolidation:

```text
fn classify_layer(r: &MemoryRecord) -> MemoryLayer {
    if r.pinned { return MemoryLayer::Core; }
    if r.core_strength    >= 0.7 { return MemoryLayer::Core; }
    if r.working_strength >= 0.6 { return MemoryLayer::Working; }
    MemoryLayer::Archived
}
```

This provisional formula is subject to refinement by the consolidation feature and is **not** part of the v0.3 public API contract — it lives in a crate-internal module and can be swapped without a semver bump. Its guarantees in v0.3 are: deterministic (same inputs → same output), pure (no side effects, no I/O), and monotonic under strength increase (raising `core_strength` never demotes the layer).

**GUARDs (reference, per §1 rules — not restated):**
- GUARD-1 → GOAL-1.12 enforcement in §4.3 Rule A (separate-transaction failure commit).
- GUARD-2 → §6 (every failure emits telemetry) + §4.1 `graph_extraction_failures` (every failure persists).
- GUARD-3 → §3.2 invariants + §4.1 no-cascade + §7 `EdgeFrozen` error + §8 merge atomicity.
- GUARD-7 → §3.1 `somatic_fingerprint: Option<SomaticFingerprint>` (type imported, not redefined).
- GUARD-8 → Episode-level immutability is owned by Episode (master §3.1); this feature only stores the entity-level *aggregate*, which is mutable by design (recomputed on new mentions).

All 15 GOAL-1.X and all 5 directly-relevant GUARDs are accounted for.

## 10. Cross-Feature References

This design owns the persistent graph shape + write trait + atomic apply boundary. It is the **producer** for types that resolution, migration, and retrieval all consume.

- **v03-resolution/design.md** (consumer on hot-write path)
  - Constructs a `GraphDelta` (§5bis) during the persist stage (§3.5) and commits via `GraphStore::apply_graph_delta` (§5bis).
  - Reads `Entity` / `Edge` / `EntityKind` / `Predicate` / `EdgeEnd` / `ResolutionMethod` (§3) — all types owned here.
  - Writes through `GraphStore` CRUD methods (§4.2) during the resolve stage (§3.4) for candidate search + alias lookup.
  - **Status:** Resolution already acknowledges this boundary in its §10. No further action needed from this side beyond keeping §5bis type signatures stable across r3.

- **v03-migration/design.md** (consumer on backfill path)
  - Per-record backfill calls `ResolutionPipeline::resolve_for_backfill` (v03-resolution §6.5) → receives `GraphDelta` → calls `GraphStore::apply_graph_delta` (§5bis) for atomic persistence.
  - Relies on **idempotence** of `apply_graph_delta` (§5bis contract) for crash-recovery / resume semantics: migration's checkpoint key matches the `(memory_id, delta_hash)` primary key of `graph_applied_deltas`.
  - Relies on `ApplyReport.already_applied` to distinguish new work from replay no-op in progress telemetry.
  - **Status:** Migration design references both `GraphDelta` and `apply_graph_delta`; this section is the formal acknowledgement from the graph-layer side. Any change to the idempotence key or delta serialization (→ `delta_hash`) requires coordinated migration update.

- **v03-retrieval/design.md** (read-only consumer)
  - Reads `Entity` / `Edge` / `EntityKind` / `Predicate` (§3) for query planning (v03-retrieval §4).
  - Reads `GraphStore::traverse`, `neighbors`, `edges_of`, `edges_as_of` (§4.2) for Factual / Associative plans (v03-retrieval §4.1, §4.3).
  - Reads `graph_memory_entity_mentions` and `knowledge_topics` tables (§4.1) for the Associative and Abstract plans respectively.
  - Does **not** write through `GraphStore` — all graph writes happen on the resolution / migration path. This asymmetry keeps the dependency DAG acyclic: retrieval → graph-layer (read), resolution/migration → graph-layer (write); neither read side nor graph-layer depends on retrieval.
  - **Status:** Retrieval's §10 already enumerates the graph types it reads. No cross-change required for r3 — this section is the mirror acknowledgement.

- **v03-benchmarks/design.md** (indirect consumer through retrieval + resolution)
  - Does not read graph types directly. Drives `Memory::ingest_with_stats` (v03-resolution §6.4) and `Memory::graph_query` (v03-retrieval §6.2); the `GraphDelta` / `apply_graph_delta` boundary is invisible to it.
  - Does rely on the `graph_pipeline_runs` / `graph_resolution_traces` tables (§4.1) being populated for its cost gate, but only via the public `ResolutionStats` surface from v03-resolution §6.4.
  - **Status:** No direct handoff from graph-layer to benchmarks. Keeping the §4.1 pipeline-audit tables schema-stable across r3 is the implicit contract.

- **Master DESIGN-v0.3.md**
  - §3 four-layer stack — §2 Architecture Overview above is the direct realization of L3'–L4.
  - §3.1 Episode anchor — consumed, not produced here (episodes stay owned by the master episodic layer per GUARD-1).
  - §3.7 Cognition / Telemetry / Affect boundary rules — §6 above pins the graph module strictly to the Telemetry publisher role.
  - §6 Consolidation — consumes the `ResourcePressure` / `OperationalLoad` signals emitted by §6 above; no shared code.

- **Master requirements.md GUARDs**
  - GUARD-1 (episodic completeness) → graph mutations never delete `MemoryRecord` rows; all `memories` writes are additive or column-update via §4.3 rule B.
  - GUARD-2 (never silent degrade) → `graph_extraction_failures` (§4.1) + `StageFailureRow` in `GraphDelta` (§5bis).
  - GUARD-3 (no erasure on invalidation) → `EdgeInvalidation` sets `valid_to` only; the old edge row is preserved (§5bis invariant).
  - GUARD-11 (v0.2 API compat) → §5 Public API Surface preserves v0.2 surface verbatim; the canonical holder type is `Memory` (see v03-retrieval §6.1 GUARD-11 naming note).

## 11. Open Questions

Carried forward from master DESIGN-v0.3.md §10, filtered to items that affect graph-layer shape:

1. **Entity-level somatic fingerprint aggregation function.** Master §3.7 says "aggregate over episode fingerprints" but does not pin a formula (mean? time-decayed mean? importance-weighted?). This feature stores the result; the computation is owned by v03-resolution. **Open**: confirm formula at v03-resolution design time.

2. **Watermark thresholds for `ResourcePressure`.** Initial values (100k entities / 500k edges per namespace) are guesses. Correct values depend on SQLite performance on target hardware and on the ingest rate of a typical deployment. **Open**: calibrate after v03-benchmarks produces numbers; values are config-overridable in the meantime.

3. **Canonical predicate list (§3.3).** The 17-variant seed list in `CanonicalPredicate` is drawn from master §3.5 examples plus the v0.2 `triple::Predicate` set. Deliberately conservative — omits `LocatedIn`, `HasPart`, `OwnedBy`, etc. **Open**: operator/reviewer pass to confirm nothing glaringly common is missing before first ingest at scale.

4. **Merge `batch_size` default.** §3.4 requires pagination but does not specify a default. SQLite can comfortably handle ~5000 edge supersedes in one transaction at current dataset sizes, but this is untested under WAL contention. **Open**: pick a default after v03-benchmarks; expose as config.

5. **Proposed-predicate normalization rules.** `classify` in §3.3 normalizes whitespace and case. Unclear whether stemming ("works_for" vs "working_for") should also collapse. Current answer: no — preserve verbatim (GOAL-1.8) and let a future schema-inducer (v0.4) decide. **Open**: confirm at v0.4 design time; no action now.

6. **FK type for `episode_id` across tables.** `Episode.id` is `Uuid` (master §3.1) but is not yet a real SQLite table in this feature's scope (owned by L1 persistence, which is split across v03-migration and v03-resolution). The `episode_id BLOB` columns in §4.1 are declared **without** a FK constraint until the `episodes` table lands. **Open**: v03-migration will add the FK in a follow-up ALTER once `episodes` exists.

7. **Alias normalization for non-ASCII / CJK names.** §3.4 specifies NFKC + lowercase + trim. `storage.rs` already uses jieba for FTS tokenization on CJK; aliases do not currently run through jieba. **Open**: decide whether alias lookup should use jieba-segmented forms (likely yes for CJK entity names, but out of scope for this design pass).

Nothing in this document depends on resolving these to ship the graph layer's types and storage; each is scoped to a downstream feature or a post-MVP calibration pass.

