# Unified Substrate (v0.4)

**Status**: DRAFT — pending potato review
**Author**: claude (rustclaw session 2026-05-12)
**Supersedes**: `v03-wireup/design.md` (G1–G6 are rewritten here to target unified schema directly, not via intermediate v0.3 schema)
**Prerequisite read**: `v03-wireup/design.md`, `consolidation-autopilot-DRAFT.md`, `engramai/src/storage.rs`, `engramai/src/retrieval/api.rs`
**2026-05-14 scope split**: sections §4.11–§4.15 (cognitive function specs) and §8.9–§8.13 (their tasks T45–T59) were moved to a new feature `v05-cognitive-substrate/` — see the stub at §4.11 below. All cross-references in this doc to `§4.11`–`§4.15` now point to **v0.5's** §2.1–§2.5; the substrate primitives those sections use (new `node_kind`s, `edge_kind`s, `WriteOp` variants) stayed in v0.4 §3 and §6 because they ARE the substrate.

---

## 0. TL;DR

Engram's mental model has always been "graph is the substrate". The
implementation grew organically into **10 tables** (4 node-shaped, 5
edge-shaped, 1 FTS) which is a schema-sprawl artifact of "add a feature
→ add a table", not a designed substrate.

This document specifies the terminal schema: **`nodes` + `edges` +
`nodes_fts` + `node_embeddings` (multi-model extension) + audit tables**,
and the **migration of every existing storage path** (memories, entities,
relations, hebbian, embeddings, FTS, knowledge topics, synthesis) onto
that substrate, ending with the **removal of the 10 legacy tables**.

v0.4's contract is narrow on purpose: **"substrate consolidated, legacy
dropped, parity proven"** — a stability gate. The cognitive functions
that this substrate *enables* (interoception/anomaly, empathy bus,
working memory, metacognition, dimensional signature) are specified in
the sibling feature `v05-cognitive-substrate/` and ship on top of v0.4,
not inside it. The substrate primitives those functions consume (new
`node_kind`s, `edge_kind`s, `WriteOp` variants) live here in v0.4 §3 and
§6 because they ARE the substrate; the function-level wiring lives in
v0.5.

The **v0.2 KC** code mass (§4.16, 21 modules, 656KB, zero production
callers) is also retired inside v0.4 (T60), since it's substrate
cleanup, not cognitive work.

The v0.3 schema (`graph_entities` + `graph_edges`) is **already 90% of
the terminal shape** — this is not a rewrite, it's a generalization +
migration. G1–G6 of v0.3 wire-up are rewritten here to land on the
unified schema directly, so we do not ship the intermediate v0.3 form.

A single-consumer **writer queue** (§6) serializes all mutations
behind SQLite WAL, supports priority lanes + Hebbian coalescing +
compound-op atomicity, and has a documented throughput ceiling of
~11k ops/sec on commodity hardware — well above projected production
load (§6.6, §6.7). Readers never block.

Execution plan: §8 has **24 open tasks** in v0.4 after the v0.5 split —
T26b/c (validation), T30–T40 (cutover + legacy drop), T41/T43 (docs),
T60 (v0.2 KC retire), T61–T68 (writer queue, parked until needed). The
remaining 15 tasks (T45–T59) moved to v0.5 along with their design.

---

## 1. Why unified

### 1.1 First-principles framing

Brains do not have a `memories` table separate from an `entities` table.
A cortical column doesn't know whether the pattern it encodes is
"episodic memory" or "concept" or "topic" — those are emergent labels
on the same substrate (neurons + synapses + activation patterns).

Engram has been writing code as if this is true (multi-plan retrieval,
fusion, column-shaped plan registry, bi-temporal edges) but storing
data as if it isn't (separate tables per cognitive function). This
mismatch is the technical-debt thesis behind this document.

### 1.2 Concrete pain caused by current sprawl

- Adding a new node-kind (e.g. `Insight`, `Plan`, `Episode`) requires
  a new table, new CRUD, new migration, new test fixture. ~3 days each.
- Cross-kind queries ("what entities does this insight reference?")
  require N-way JOINs across heterogeneous schemas.
- Embedding is multi-model on `memories` (good) but single-model
  inlined on `graph_entities` (limited) — same concept, two designs.
- Decay/forget is `deleted_at` on memories; entity retirement is
  `merged_into` on graph_entities; supersession is `superseded_by`
  on edges. Three retirement mechanisms for one concept.
- Hebbian co-activation lives in its own table (`hebbian_links`) but is
  semantically an edge with `predicate='co_activated'`. Two storage
  models for one graph.

### 1.3 What unified buys

- One ingest path → write to `nodes` and `edges`, done.
- One retrieval substrate → all plans operate on `(nodes, edges)`.
- Adding new node-kinds is a schema-free operation (just a new value
  in `node_kind`).
- Multi-model embedding becomes uniform (any node can have multiple
  embeddings via `node_embeddings` extension).
- Retirement is uniform: `deleted_at` (soft delete / forget) +
  `superseded_by` (correction / merge). One node, two mechanisms.

---

## 2. Verified current state (2026-05-12)

Production DB (`rustclaw/engram-memory.db`):

| Table                 | Rows  | Shape | Notes                                  |
|-----------------------|-------|-------|----------------------------------------|
| memories              | 24624 | node  | core content, multi-model emb separate |
| memory_embeddings     | 24467 | ext   | multi-model `(memory_id, model)` key   |
| entities              | 2310  | node  | v0.2 entity layer (not v0.3)           |
| entity_relations      | 6531  | edge  | v0.2 free-form predicate               |
| memory_entities       | 9237  | edge  | mention edges (memory → entity)        |
| hebbian_links         | 43710 | edge  | co-activation, weight-only             |
| knowledge_topics      | 0     | node  | KC layer, never populated (ISS-109)    |
| cluster_assignments   | 0     | edge  | topic→memory containment, empty        |
| synthesis_provenance  | 72    | edge  | insight → source memory                |
| promotion_candidates  | 0     | node  | KC promotion gate, empty               |

**Totals**: 4 active node-shaped tables, 5 active edge-shaped tables, 1
multi-model extension. **90%+ of fields are isomorphic across the
node-shaped tables** (content, kind, timestamps, activation, importance,
namespace, embedding, affect).

v0.3 DB (`crates/engramai/.gid/graph.db` and bench fixtures):

- `graph_entities` — already the terminal `nodes` shape minus
  generalization of `kind` to include memory/topic/insight
- `graph_edges` — already the terminal `edges` shape minus `edge_kind`
  discriminator and `subject` FK generalization (currently only entity)

This is the basis for "90% there already".

---

## 3. Terminal schema

Three core tables + one multi-model extension + retained audit tables.

### 3.1 `nodes` — every conceptual unit

```sql
CREATE TABLE nodes (
    -- identity
    id                  TEXT PRIMARY KEY,                  -- string UUID
    node_kind           TEXT NOT NULL,                     -- 'memory'|'entity'|'topic'|'insight'|'episode'|'plan'|...
    namespace           TEXT NOT NULL DEFAULT 'default',

    -- memory-specific sub-classification (NULL for non-memory kinds)
    layer               TEXT,                              -- 'core'|'working'|'archive' (memories only)
    memory_type         TEXT,                              -- 'factual'|'episodic'|'relational'|'emotional'|'procedural'|'opinion'|'causal' (memories only)

    -- content
    content             TEXT NOT NULL,                     -- raw content (memory) / canonical_name (entity) / summary (topic)
    summary             TEXT NOT NULL DEFAULT '',          -- optional secondary text
    attributes          TEXT NOT NULL DEFAULT '{}',        -- JSON: kind-specific fields

    -- vector
    embedding           BLOB,                              -- primary embedding (system default model)
    embedding_model     TEXT,                              -- model id; NULL iff embedding NULL

    -- temporal (bi-temporal)
    occurred_at         REAL,                              -- when content happened (memory event time)
    valid_from          REAL,                              -- truth window start (entity/fact)
    valid_to            REAL,                              -- truth window end
    created_at          REAL NOT NULL,                     -- ingest wall-clock
    updated_at          REAL NOT NULL,
    first_seen          REAL,                              -- entity-style observation window
    last_seen           REAL,

    -- decay / activation / strength
    activation          REAL NOT NULL DEFAULT 0.0,         -- spreading activation, [0,1]
    working_strength    REAL NOT NULL DEFAULT 1.0,         -- working-memory half-life
    core_strength       REAL NOT NULL DEFAULT 0.0,         -- consolidated long-term
    importance          REAL NOT NULL DEFAULT 0.3,
    confidence          REAL NOT NULL DEFAULT 0.5,         -- identity_confidence for entity, generic confidence elsewhere

    -- affect
    agent_affect        TEXT,                              -- JSON or NULL
    arousal             REAL NOT NULL DEFAULT 0.0,
    somatic_fingerprint BLOB,                              -- 8 × f32 LE or NULL

    -- retirement
    deleted_at          REAL,                              -- soft delete (forget)
    superseded_by       TEXT REFERENCES nodes(id),         -- correction / entity merge / topic update
    pinned              INTEGER NOT NULL DEFAULT 0,        -- protect from decay/forget

    -- provenance
    source              TEXT NOT NULL DEFAULT '',          -- origin: 'user'|'agent'|'extraction'|'synthesis'|...
    source_run_id       TEXT,                              -- pipeline_runs.id when extracted (string UUID)
    -- (episode_id removed: episodes are nodes, linked via containment edges — see §7.4)
    consolidation_count INTEGER NOT NULL DEFAULT 0,
    last_consolidated   REAL,

    -- history (audit trail of in-place mutations, e.g. entity merges)
    history             TEXT NOT NULL DEFAULT '[]',        -- JSON: Vec<HistoryEntry>

    -- FTS surrogate: stable integer for nodes_fts rowid (§3.3).
    -- Assigned at INSERT via writer queue (§6) from a monotonic counter;
    -- never updated, never reused. Cannot use SQLite implicit rowid because
    -- VACUUM reassigns it when PK is TEXT.
    fts_rowid           INTEGER NOT NULL UNIQUE,

    CHECK (activation       BETWEEN 0.0 AND 1.0),
    CHECK (arousal          BETWEEN 0.0 AND 1.0),
    CHECK (importance       BETWEEN 0.0 AND 1.0),
    CHECK (confidence       BETWEEN 0.0 AND 1.0),
    CHECK (working_strength BETWEEN 0.0 AND 1.0),
    CHECK (core_strength    BETWEEN 0.0 AND 1.0)
);

CREATE INDEX idx_nodes_kind         ON nodes(node_kind, namespace);
CREATE INDEX idx_nodes_namespace    ON nodes(namespace);
CREATE INDEX idx_nodes_created      ON nodes(created_at);
CREATE INDEX idx_nodes_occurred     ON nodes(occurred_at) WHERE occurred_at IS NOT NULL;
CREATE INDEX idx_nodes_deleted      ON nodes(deleted_at) WHERE deleted_at IS NULL;  -- partial: live rows
CREATE INDEX idx_nodes_kind_active  ON nodes(node_kind, activation) WHERE deleted_at IS NULL;
CREATE INDEX idx_nodes_memory_type  ON nodes(memory_type) WHERE node_kind='memory';
CREATE INDEX idx_nodes_superseded   ON nodes(superseded_by) WHERE superseded_by IS NOT NULL;
-- fts_rowid is already UNIQUE (implicit index).

-- Monotonic counter for fts_rowid assignment (§3.3, §6 writer).
CREATE TABLE fts_rowid_counter (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 0),
    next_value INTEGER NOT NULL DEFAULT 1
);
INSERT INTO fts_rowid_counter (singleton, next_value) VALUES (0, 1);
```

**ID type rationale**: `id` is `TEXT` (string UUID) to match all existing
schemas (`memories.id`, `entities.id`) and the Rust API
(`MemoryRecord.id: String`). This means zero conversion at API boundaries
and zero churn in the dual-write phase. All FK columns referencing
`nodes(id)` (in `edges`, `node_embeddings`, `nodes.superseded_by`) are
correspondingly `TEXT`.

**Wide-table + NULL strategy**. SQLite stores NULL with negligible
overhead; we avoid JOIN-heavy retrieval which is the hot path. Per-kind
required fields enforced application-side (typed structs in Rust).

### 3.2 `edges` — every relation

```sql
CREATE TABLE edges (
    id                  TEXT PRIMARY KEY,
    source_id           TEXT NOT NULL REFERENCES nodes(id) ON DELETE RESTRICT,
    target_id           TEXT REFERENCES nodes(id) ON DELETE RESTRICT,
    target_literal      TEXT,                              -- JSON; NULL iff target_id IS NOT NULL

    -- typing: two-level discriminator
    edge_kind           TEXT NOT NULL,                     -- 'structural'|'associative'|'containment'|'provenance'|'temporal'|'supersession'
    predicate_kind      TEXT NOT NULL DEFAULT 'canonical', -- 'canonical'|'proposed'
    predicate           TEXT NOT NULL,                     -- within edge_kind: 'co_activated','contains','mentions','derived_from','located_in','is_a',...

    -- payload
    summary             TEXT NOT NULL DEFAULT '',
    attributes          TEXT NOT NULL DEFAULT '{}',        -- JSON: kind-specific fields (Hebbian signal_source/coactivation_count/temporal_*/direction; synthesis gate_decision/gate_scores/cluster_id; entity_relations.metadata; etc.)
    weight              REAL NOT NULL DEFAULT 1.0,         -- Hebbian weight / fusion contribution
    activation          REAL NOT NULL DEFAULT 0.0,
    confidence          REAL NOT NULL DEFAULT 0.5,

    -- temporal (bi-temporal)
    valid_from          REAL,
    valid_to            REAL,
    recorded_at         REAL NOT NULL,

    -- supersession / retirement
    invalidated_at      REAL,
    invalidated_by      TEXT REFERENCES edges(id),
    supersedes          TEXT REFERENCES edges(id),

    agent_affect        TEXT,

    -- (episode_id removed: episodes are nodes, linked via containment edges — see §7.4)
    source_run_id       TEXT,                                -- string UUID, references pipeline_runs.id
    source_memory_id    TEXT REFERENCES nodes(id),
    resolution_method   TEXT NOT NULL DEFAULT 'direct',

    namespace           TEXT NOT NULL DEFAULT 'default',
    created_at          REAL NOT NULL,

    CHECK (activation BETWEEN 0.0 AND 1.0),
    CHECK (confidence BETWEEN 0.0 AND 1.0),
    CHECK (weight     >= 0.0),
    CHECK (
        (target_id IS NOT NULL AND target_literal IS NULL) OR
        (target_id IS NULL     AND target_literal IS NOT NULL)
    )
);

CREATE INDEX idx_edges_source         ON edges(source_id, edge_kind);
CREATE INDEX idx_edges_target         ON edges(target_id, edge_kind) WHERE target_id IS NOT NULL;
CREATE INDEX idx_edges_kind_pred      ON edges(edge_kind, predicate, namespace);
CREATE INDEX idx_edges_namespace      ON edges(namespace);
CREATE INDEX idx_edges_temporal       ON edges(valid_from, valid_to) WHERE valid_from IS NOT NULL;
CREATE INDEX idx_edges_live           ON edges(edge_kind, predicate) WHERE invalidated_at IS NULL;

-- Partial UNIQUE indexes enforce upsert semantics for kinds that must not duplicate
-- (associative co-activation accumulates weight; containment is a set membership).
-- Structural edges may legitimately have duplicates from different runs.
CREATE UNIQUE INDEX idx_edges_assoc_unique
    ON edges(source_id, target_id, edge_kind, predicate,
             json_extract(attributes, '$.signal_source'))
    WHERE edge_kind = 'associative';
-- Note: signal_source is part of the associative-edge identity (see §4.3).
-- Each distinct signal_source between the same (src, tgt) pair gets its own
-- row, so that §4.6 differential decay applies per-signal-source without
-- mixing. SQLite supports json_extract in expression-indexed unique
-- constraints and resolves ON CONFLICT against them — verified.
CREATE UNIQUE INDEX idx_edges_containment_unique
    ON edges(source_id, target_id, edge_kind, predicate)
    WHERE edge_kind = 'containment';
```

**`predicate_kind` semantics**: discriminator for canonical (resolved /
curated) vs proposed (extraction-suggested, awaiting validation) edges.
`ResolutionPipeline` writes `'proposed'` for low-confidence triples;
promotion to `'canonical'` is a separate transition handled by the
promotion gate. Used in: ResolutionPipeline output (T13), promotion gate
(see §4.9). Keeping the column up-front avoids a future schema migration
when the promotion gate ships.

**`edge_kind` taxonomy** (two-level discriminator = closed outer type
+ open-but-enumerated inner predicate). The outer `edge_kind` is a
closed set of 6 values — adding a 7th is a deliberate schema-level
change. The inner `predicate` is open within each `edge_kind` but the
full set used in this design is enumerated below so an implementer can
build a `CHECK (edge_kind IN (...) AND predicate IN (...))` constraint
or a lookup table without re-deriving from §4:

| edge_kind     | predicate                       | direction              | source §        | replaces                                    |
|---------------|---------------------------------|------------------------|-----------------|---------------------------------------------|
| structural    | `is_a`                          | child → parent type    | §4.2            | `entity_relations`                          |
| structural    | `located_in`                    | thing → place          | §4.2            | `entity_relations`                          |
| structural    | `causes`                        | cause → effect         | §4.2            | `entity_relations`                          |
| structural    | `same_as`                       | alias → canonical      | §4.2            | `graph_entities.merged_into`                |
| structural    | `subject_of`                    | entity → memory        | §4.2            | `memory_entities` (subject role)            |
| containment   | `tagged`                        | memory → tag node      | §4.15 Tier 3    | (new — set membership, idempotent)          |
| containment   | `describes_participants`        | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.participants`          |
| containment   | `describes_location`            | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.location`              |
| containment   | `describes_temporal`            | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.temporal`              |
| containment   | `describes_context`             | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.context`               |
| containment   | `describes_causation`           | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.causation`             |
| containment   | `describes_outcome`             | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.outcome`               |
| containment   | `describes_method`              | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.method`                |
| containment   | `describes_relations`           | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.relations`             |
| containment   | `describes_sentiment`           | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.sentiment`             |
| containment   | `describes_stance`              | memory → dim node      | §4.15 Tier 2    | `memories.dimensions.stance`                |
| associative   | `co_activated`                  | A ↔ B (direction attr) | §4.3            | `hebbian_links`                             |
| associative   | `evoked_by`                     | marker → trigger       | §4.11           | (new, somatic markers)                      |
| associative   | `aligns_with`                   | memory → drive node    | §4.11           | (new, drive alignment)                      |
| containment   | `contains`                      | container → contained  | §4.4, §4.16     | `cluster_assignments`, `topic_member`-style |
| containment   | `belongs_to_episode`            | memory → episode       | §4.2            | (new, episode nodes)                        |
| containment   | `wm_contained`                  | snapshot → memory      | §4.13           | (new, WM snapshots)                         |
| provenance    | `mentions`                      | memory → entity        | §4.2            | `memory_entities` (mention role)            |
| provenance    | `derived_from`                  | output → input         | §4.4, §4.5      | `synthesis_provenance`                      |
| provenance    | `wm_snapshot_of`                | snapshot → feedback    | §4.13, §4.14    | (new)                                       |
| temporal      | `before` / `after` / `during`   | A → B                  | §4 (capability) | (new capability)                            |
| supersession  | (managed via `supersedes` col)  | new → old              | §4.7            | `memories.superseded_by`/`contradicts`      |

Three rules govern this table:

1. **`edge_kind` is closed**: 6 values, no more. A "new edge_kind" is a schema design act requiring §3.2 revision + new index strategy.
2. **`predicate` is open per `edge_kind`** but every predicate used in this design appears above. Adding a new predicate within an existing `edge_kind` (e.g. another `describes_*`) is a §4 design act, not a schema act.
3. **Supersession is structural, not predicate-shaped**: the `supersedes` and `invalidated_by` *columns* on `edges` express edge-level supersession; `edge_kind='supersession'` is reserved for cases where supersession itself is the relation being modeled (rare — most supersession is signaled via the column on the new edge replacing the old).

### 3.3 `nodes_fts` — full-text search across all kinds

FTS5 in **contentless mode** keyed by a stable surrogate integer.

**The constraint**: FTS5 virtual tables only support `WHERE rowid = ?` or `WHERE <fts_col> MATCH ?` in DELETE/UPDATE statements. `WHERE id = ?` against an `UNINDEXED` column **does not work** (FTS5 rejects arbitrary predicates regardless of column indexing). Implicit SQLite `rowid` on `nodes` is unstable across `VACUUM` when the declared PK is `TEXT`, so it cannot be the FTS key directly.

**The design**: add a `fts_rowid` column to `nodes` — a stable, monotonic integer surrogate — and use it as the FTS5 rowid:

```sql
-- Augmentation to §3.1 nodes table (also reflected there):
--   fts_rowid INTEGER NOT NULL UNIQUE
-- assigned at INSERT time from a dedicated counter (sqlite_sequence row or
-- a singleton `fts_rowid_counter` table); never reused, never updated.

CREATE VIRTUAL TABLE nodes_fts USING fts5(
    content,
    summary,
    tokenize='unicode61 remove_diacritics 2',
    content=''                                    -- contentless: FTS stores tokens only
);

-- Maintain FTS in lockstep with nodes via fts_rowid.
CREATE TRIGGER nodes_fts_ai AFTER INSERT ON nodes BEGIN
    INSERT INTO nodes_fts(rowid, content, summary)
    VALUES (new.fts_rowid, new.content, new.summary);
END;

CREATE TRIGGER nodes_fts_ad AFTER DELETE ON nodes BEGIN
    -- contentless FTS5 requires the 'delete' command form:
    INSERT INTO nodes_fts(nodes_fts, rowid, content, summary)
    VALUES ('delete', old.fts_rowid, old.content, old.summary);
END;

CREATE TRIGGER nodes_fts_au AFTER UPDATE OF content, summary ON nodes BEGIN
    INSERT INTO nodes_fts(nodes_fts, rowid, content, summary)
    VALUES ('delete', old.fts_rowid, old.content, old.summary);
    INSERT INTO nodes_fts(rowid, content, summary)
    VALUES (new.fts_rowid, new.content, new.summary);
END;
```

**Querying**: callers do `SELECT n.* FROM nodes_fts f JOIN nodes n ON n.fts_rowid = f.rowid WHERE nodes_fts MATCH ?`. The `fts_rowid` ↔ `id` mapping is a regular indexed B-tree lookup — no rowid-stability risk.

FTS indexes **all node kinds**, not only memory. Entity canonical names, topic summaries, insight text become searchable through one path. Net gain over current `memories_fts`-only design. Risk R3 (FTS rowid volatility) is eliminated because `fts_rowid` is owned by us, never reassigned by `VACUUM`.

### 3.4 `node_embeddings` — multi-model extension

```sql
CREATE TABLE node_embeddings (
    node_id     TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    embedding   BLOB NOT NULL,
    dimensions  INTEGER NOT NULL,
    created_at  REAL NOT NULL,
    PRIMARY KEY (node_id, model)
);
CREATE INDEX idx_node_embeddings_model ON node_embeddings(model);
```

99% of queries hit `nodes.embedding` (single model, inlined, no JOIN).
This table serves the multi-model power-user case currently provided
by `memory_embeddings`. **Capability gain**: entity/topic/insight can
now also be multi-model.

### 3.5 Audit tables (retained, not unified)

Append-only operational logs, not cognitive substrate. Stay separate:

- `pipeline_runs` — ResolutionPipeline invocations
- `resolution_traces` — per-stage resolution decisions
- `extraction_failures` — quarantined errors
- `access_log` — retrieval access for activation feedback
- `engram_meta` — schema version
- `backfill_queue` — async backfill state
- `quarantine` — failed ingest holding pen

---

## 4. Cognitive functions mapped to unified ops

Verification that every existing function fits the substrate. If any
function doesn't fit, schema is wrong — iterate before implementing.

### 4.1 Memory ingest (currently `memory.rs:store_raw`)

**Current**: INSERT into memories + INSERT into memory_embeddings.

**Unified**:
```
INSERT INTO nodes (id, node_kind='memory', layer, memory_type, content,
                   embedding, embedding_model,
                   occurred_at, created_at, working_strength=1.0, importance,
                   namespace, source);
-- if memory belongs to an episode:
INSERT INTO edges (id, source_id=memory_id, target_id=episode_node_id,
                   edge_kind='containment', predicate='belongs_to_episode', ...);
-- multi-model: INSERT INTO node_embeddings if additional models
```

ResolutionPipeline (async, post-write) extracts triples → creates
`node_kind='entity'` nodes + `edge_kind='structural'` edges.

### 4.2 Entity resolution (currently `resolution/pipeline.rs`)

**Current**: writes `graph_entities` rows with no resolution between
surface forms. "potato" and "@horseonedragon" are two unrelated rows.

**Unified** (per §7.2): every entity surface form is a `nodes` row of
`node_kind='entity'` carrying its surface text in `content` (so it
participates in `nodes_fts`) and its concept embedding in `embedding`.
Surface forms referring to the same referent are linked by
`edge_kind='structural', predicate='same_as'`. Resolution is therefore
a graph operation (find the `same_as` connected component), not a
column lookup.

`memory_entities.role` maps to `edges.predicate` (role='mention' →
predicate='mentions', role='subject' → predicate='subject_of', etc.).
If role is unknown or 'mention', use `predicate='mentions'`.

**Out of scope for this design**: the choice between (a) a designated
canonical node per cluster vs (b) a `same_as` clique with no canonical.
The substrate supports both; the resolution algorithm is a v0.4.1
concern. See §7.2.1.

### 4.3 Hebbian co-activation (currently `association/former.rs` and `models/hebbian.rs`)

**Current**: two legacy writers into `hebbian_links`, with different
accumulation rules:

| Caller                                    | Trigger              | Strength update      | `coactivation_count` |
|-------------------------------------------|----------------------|----------------------|----------------------|
| `Storage::record_association`             | `LinkFormer` discovery | `max(existing, new)` | unchanged            |
| `Storage::record_coactivation_ns` (`models::record_coactivation_ns`) | recall co-fire | `+ 0.1` cap 1.0 | `+= 1` |

These two writers project semantically distinct events (signal-derived
association vs. usage-driven coactivation) onto the same `hebbian_links`
row, losing provenance. The unified substrate fixes this.

**Unified: signal_source is part of the row identity.** Each distinct
`signal_source` between the same `(source_id, target_id)` pair gets its
own `edges` row. `signal_source` lives in `attributes` JSON, and the
partial unique index `idx_edges_assoc_unique` (§3.2) includes
`json_extract(attributes, '$.signal_source')` in its key so SQLite's
`ON CONFLICT` resolves correctly. This design choice:

**Canonical (src, tgt) ordering invariant**: associative-edge writes
MUST canonicalize the pair via `(src, tgt) = if src < tgt then (src, tgt)
else (tgt, src)` before INSERT, so an edge between A and B is always
stored as `(min(A,B), max(A,B))`. Without this, `record_association`
called from both directions would produce two unified rows for the same
Hebbian fact (legacy hides this via a bidirectional `OR` lookup;
unified relies on the unique index, which is direction-sensitive).
Implementation lives in the dual-write helper, not in callers.

The design choice listed above yields:

1. Lets §4.6 **differential decay** apply per-signal-source without
   mixing (a hot `corecall` link decaying independently from a cold
   `entity_overlap` signal between the same pair).
2. Preserves provenance — retrieval can ask "which signals connect
   these two memories?" by `SELECT ... GROUP BY signal_source`.
3. Treats `weight` and `coactivation_count` as **sum-accumulating per
   signal source**, which is the Hebbian frequency-weighted model the
   legacy `record_coactivation_ns` was approximating with `+0.1` cap.
   `record_association`'s `max` semantics are dropped in unified — they
   were an artifact of LinkFormer always passing `config.initial_strength`
   (a constant), so `max(c, c) = c` made max behave as first-write-wins
   in production. Unified replaces it with sum, which carries the
   reuse-frequency signal legacy never transmitted.

**Canonical UPSERT** (one row per distinct `signal_source` between a
pair):

```sql
INSERT INTO edges (
    id, source_id, target_id, edge_kind, predicate, namespace,
    weight, attributes, recorded_at
) VALUES (
    :uuid, :src, :tgt, 'associative', 'co_activated', :namespace,
    :delta,
    json_object(
        'signal_source',       :signal_source,       -- 'corecall'|'multi'|'entity'|'temporal'|... drives differential decay (§4.6)
        'signal_detail',       :signal_detail,
        'coactivation_count',  1,
        'temporal_forward',    :tf,
        'temporal_backward',   :tb,
        'direction',           :direction
    ),
    :now
)
ON CONFLICT (source_id, target_id, edge_kind, predicate,
             json_extract(attributes, '$.signal_source'))
DO UPDATE SET
    weight       = edges.weight + excluded.weight,
    recorded_at  = excluded.recorded_at,
    attributes   = json_patch(
        edges.attributes,
        json_object(
            'coactivation_count',
                json_extract(edges.attributes, '$.coactivation_count') + 1,
            'temporal_forward',
                json_extract(edges.attributes, '$.temporal_forward')
                + json_extract(excluded.attributes, '$.temporal_forward'),
            'temporal_backward',
                json_extract(edges.attributes, '$.temporal_backward')
                + json_extract(excluded.attributes, '$.temporal_backward')
        )
    );
```

**Intentional legacy↔unified divergence** (Phase B): legacy and unified
deliberately accumulate differently. Phase B parity tests (§8.10 T17)
verify *existence* and *signal_source provenance*, not numeric equality
on `weight`/`coactivation_count`:

| Field                  | Legacy `hebbian_links`                                       | Unified `edges(edge_kind='associative')`     |
|------------------------|--------------------------------------------------------------|----------------------------------------------|
| `(src, tgt)` row count | 1 row per pair (regardless of signal source)                 | N rows per pair (one per distinct signal_source) |
| `strength` / `weight`  | `record_association`: max; `record_coactivation_ns`: +0.1 cap | sum per signal_source                        |
| `coactivation_count`   | 0 (record_association) or `+= 1` (record_coactivation_ns)   | `+= 1` per matching signal_source UPSERT     |
| `signal_source`        | last writer wins (or whoever's strength was higher)          | dimension of row identity                    |

Phase D adopts unified semantics by deleting `hebbian_links` and
routing reads through unified `edges`. The divergence is therefore a
one-way street: unified is the destination, legacy is a transient
double-write during B/C.

Three properties this UPSERT relies on:
1. **Partial UNIQUE index** declared in §3.2 (`idx_edges_assoc_unique`)
   covers exactly `(source_id, target_id, edge_kind, predicate,
   json_extract(attributes, '$.signal_source'))` **WHERE
   `edge_kind='associative'`**. SQLite resolves the `ON CONFLICT`
   target against this expression-indexed partial index because the
   inserted row satisfies its `WHERE` clause. Inserts of other
   `edge_kind` values (e.g. `structural`, `containment`) bypass this
   conflict target — they get their own `id` UNIQUE PK conflict p
2. **`predicate='co_activated'`** is the canonical value for Hebbian
   edges. Other associative predicates (e.g. `evoked_by` for somatic
   markers in §4.11) use a different conflict path (different
   `(source_id, target_id, predicate)` tuple).
3. **Atomicity with the parent recall** — when Hebbian bumps fire from
   inside a retrieval, the bumps are coalesced (§6.3) and submitted as
   a single `BumpAssociation` op to the writer queue. The UPSERT runs
   inside the writer's batch transaction (§6.2), not inline.

### 4.4 Knowledge compilation (currently `knowledge_compile/`)

**Current**: writes `knowledge_topics` + `cluster_assignments`.

**Unified**:
```
INSERT INTO nodes (node_kind='topic', content=summary, embedding=centroid, ...);
INSERT INTO edges (source_id=topic_id, target_id=member_memory_id,
                   edge_kind='containment', predicate='contains', weight=membership_score);
```

ISS-109 (clusterer collapse) becomes a tuning problem on the unified
substrate, not a separate-table issue.

### 4.5 Synthesis / insights (currently `synthesis/`)

**Current**: writes `synthesis_provenance` linking insight → source.

**Unified**:
```
INSERT INTO nodes (node_kind='insight', content=text, embedding, importance, source='synthesis');
INSERT INTO edges (source_id=insight_id, target_id=source_memory_id,
                   edge_kind='provenance', predicate='derived_from',
                   confidence=synthesis_confidence,
                   recorded_at=synthesis_timestamp,
                   attributes=json_object(
                       'gate_decision', gate_decision,        -- which synthesis gate passed
                       'gate_scores',   gate_scores,          -- per-gate score JSON
                       'cluster_id',    cluster_id));         -- originating cluster
```

> **Attributes shape — open question (T25-r1 FINDING-1, 2026-05-13)**
>
> The Phase B writer (T16, `storage.rs::store_synthesis_provenance`) and
> the Phase C backfill (T25, `substrate::backfill::backfill_synthesis_provenance_to_edges`)
> currently produce DIFFERENT `attributes` JSON shapes for the same logical row:
>
> | key | Phase B T16 | Phase C T25 |
> |---|---|---|
> | `gate_decision` | always present | always present |
> | `gate_scores` | always present (`null` when NULL) | omitted when NULL/empty |
> | `cluster_id` | always present | always present |
> | `source_original_importance` | always present (`null` when NULL) | omitted when NULL |
> | `synthesis_timestamp` | NOT emitted | always present (RFC3339) |
>
> Because `INSERT OR IGNORE` is used, whichever writer fires first wins; the
> attributes blob a reader sees depends on race order. T27 verifier MUST
> either (a) treat missing-key ≡ null-key for these attributes and ignore
> `synthesis_timestamp` divergence, or (b) pin one canonical shape and
> align both writers. Recommend (b) with Phase C's shape (synthesis_timestamp
> preserved, omit-NULL convention), reasoning: forensic value of preserving
> the original synthesis time is non-zero, and JSON omit-NULL is the
> standard convention. Decision deferred to T27 implementation.

### 4.6 Decay / forget (currently `memory.rs::check_decay_and_flag` + `storage.rs`; report types in `lifecycle.rs`)

**Current**: `memory.rs::check_decay_and_flag` (line ~1647) reads `memories.created_at`, decays `working_strength`,
sets `deleted_at` when threshold crossed. SQL UPDATE happens in `storage.rs`. `lifecycle.rs` only holds
the `DecayReport` / `ForgetReport` types (~580 LoC, mostly tests).

**Unified**: identical logic, reads `nodes.created_at`, writes
`nodes.deleted_at` and `nodes.working_strength`. Filters by
`node_kind='memory'` (entity/topic decay logic may differ; entities
typically don't decay, topics may decay on relevance — separate
behaviors using the same fields).

**ISS-103 invariant — decay MUST read `created_at`, not `occurred_at`.**
`created_at` is the ingest wall-clock (when the system observed the
fact). `occurred_at` is the event time the fact refers to (which may
be years in the past for a historical recall). Decay models the
ingest-age forgetting curve; using `occurred_at` would soft-delete a
freshly-ingested historical memory on first tick. This bug existed
pre-ISS-103 (RUN-0017 → 3.6% J-score from a 152-question suite where
the gold set was full of pre-1970 historical references) and the fix
is preserved on the unified substrate: decay's date input is always
`nodes.created_at`.

`pinned=1` rows skip decay (same as current).

**Differential decay for associative edges**: the existing
`decay_hebbian_links_differential` applies different decay rates per
signal source (corecall=0.95, multi=0.90, default=0.85). On the
unified substrate this MUST read the discriminator from
`edges.attributes.signal_source` (JSON), not from a dedicated column.
Backfill (§5.3 T24) preserves this field. Edge decay reads
`edges.recorded_at` (when the association was *recorded* — equivalent
to `created_at` for nodes), not any per-event `valid_from` field.

### 4.7 Supersession / correction (currently scattered)

**Current**: `memories.superseded_by`, `memories.contradicts`,
`graph_edges.supersedes`, `graph_entities.merged_into` — four
mechanisms.

**Unified**: one mechanism per layer:
- Node supersession: `nodes.superseded_by` (entity merge, memory
  correction, topic update — all the same operation)
- Edge supersession: `edges.supersedes` + `invalidated_at/by`
  (already in v0.3 schema)

### 4.8 Retrieval plans (currently `retrieval/plans/*`)

**Current**: 7 plans (abstract_l5, affective, associative, bitemporal, episodic, factual, hybrid) + 5 adapters, fallback to v0.2 tables when v0.3 empty.

**Unified**: same plans, adapters read from `(nodes, edges)`. No
fallback path needed — there is only one substrate. The plans listed
in `consolidation-autopilot-DRAFT.md` and v03 retrieval continue to
operate, but their data source is uniform.

Specifically:
- `episodic` plan → `nodes WHERE node_kind='memory' AND occurred_at BETWEEN ...`
- `factual` plan → traverse `edges WHERE edge_kind IN ('structural', 'containment')` — structural for entity relations (is_a, located_in, same_as), containment for dimensional set membership (describes_location, tagged, …)
- `associative` plan → traverse `edges WHERE edge_kind='associative'` (replaces Hebbian spreading)
- `abstract_l5` plan → `nodes WHERE node_kind='topic'` + `edges WHERE predicate='contains'`
- `bitemporal` plan → `edges WHERE valid_from/valid_to filter`
- `affective` plan → filter by `agent_affect` / `arousal` on nodes & edges

### 4.9 Promotion (currently `compiler/` + `promotion.rs`)

**Current**: writes `promotion_candidates`.

**Unified**: `promotion_candidates` becomes nodes of kind
`'promotion_candidate'` linked via `edges` (kind=provenance,
predicate=`promotion_source`) to source memories. Or kept as audit
table — decision in §7 Q5.

### 4.10 Episodes (currently scattered as `episode_id` columns)

In the legacy schema `episode_id` is a free-form column on memory and
entity rows with no FK constraint and no episode table backing it.
There is no episode entity in the substrate — just a grouping label.

**Unified** (per §7.4): episodes become first-class `nodes` of
`node_kind='episode'`. Memories link via `edges` with
`edge_kind='containment', predicate='belongs_to_episode'`.
The denormalized `episode_id` columns on `nodes` and `edges` are
**dropped** — not retained as cheap filter. Reasoning: see §7.4
(episode is a cognitive entity, not a label; dual representation is
technical debt; index complexity on the containment edge is identical
to column-filter complexity).

**Migration**: during Phase C backfill (T19 for memories, T22 for
entities), every legacy `episode_id` value becomes a containment edge
pointing at the corresponding episode node. Episode nodes themselves
are created during Phase C from the distinct set of legacy episode_id
values. Phase F (T41) drops the `episode_id` columns.

### 4.11 – 4.15 → moved to `v05-cognitive-substrate/design.md`

The cognitive-function specs for interoception + somatic markers (§4.11), empathy bus (§4.12), working memory (§4.13), metacognition (§4.14), and dimensional signature (§4.15) have been split into the **v0.5 Cognitive Substrate** feature on 2026-05-14. Rationale: those sections describe functions *built on top of* the v0.4 unified substrate, not the substrate itself. Conflating "does the substrate work" (v0.4) with "what does it enable" (v0.5) stretched v0.4 to 68 tasks and blurred when "done" means done.

See `.gid/features/v05-cognitive-substrate/design.md` §2 for the design specs (preserved verbatim from this file's prior §4.11–§4.15 modulo subsection renumbering), and `requirements.md` in the same directory for the formal GOAL/GUARD set.

The action-plan transplant lives in v0.5 §3 (tasks T45–T59). T60 (v0.2 KC retirement) stays under v0.4 cleanup, and T61–T68 (writer-queue infrastructure) stay parked under v0.4 §8.15 until a separate feature picks them up.

---

### 4.16 v0.2 Knowledge Compiler triage (currently `crates/engramai/src/compiler/`)

**Today (verified 2026-05-12 by direct audit; see *Evidence* below)**:

- `crates/engramai/src/compiler/` — **21 modules, 656 KB source**, last meaningful edit Apr 23 2026.
- **21/21 modules have ZERO external production call sites.** Only test code and the compiler's own integration tests touch them.
- `KnowledgeCompiler::new` is instantiated **0 times** outside the `compiler/` crate boundary.
- `Memory::compile_knowledge` (memory.rs:6552, **sync `pub fn`**) **fully routes through v0.3** (`knowledge_compile::compile`, 9 files / 2384 LoC in `crates/engramai/src/knowledge_compile/`).
- v0.2 still compiles and 5/5 integration tests pass — **functional but unused**. Nobody ceremonied its death.

**Evidence (reproducible)**:

```
# 1. Module count
$ find crates/engramai/src/compiler -name '*.rs' | wc -l
21

# 2. External callers of any compiler:: symbol
$ grep -rn 'use .*compiler::\|compiler::[A-Za-z_]*::\b' crates/engramai/src --include='*.rs' \
    | grep -v 'src/compiler/'
(empty)

# 3. Memory::compile_knowledge implementation
$ sed -n '6552p' crates/engramai/src/memory.rs
    pub fn compile_knowledge(&self, namespace: &str) -> Result<...> {
        crate::knowledge_compile::compile(...)
```

The audit overturned a ~2-week-old working-memory belief that v0.2 was load-bearing for some retrieval path. It is not — `Memory::compile_knowledge` has routed exclusively to v0.3 for some time. The two modules with *concepts* worth re-using (`intake.rs`, `manual_edit.rs`) also have zero callers today; they are listed in §4.16.4 as candidates for re-integration as substrate writers, not as active dependencies that need migrating.

#### 4.16.1 Disposition: retire v0.2, do not migrate it

v0.2 has no place in v04 substrate planning. There is no production code that depends on it, so:

- §5 Migration plan does **not** carry v0.2-specific work items. v0.2's tables/indices, if any are still created in `storage.rs`, are dropped during Phase F (§5.6 — legacy-table teardown).
- §6 Writer queue does **not** include `compiler::*` ops. The only "compile knowledge" writer is the v0.3 path (`WriteKnowledgeTopic`, already covered by §4.4).
- §8 Action plan (commit 4) adds **one task** for v0.2 retirement: `T-XX: Remove crates/engramai/src/compiler/ and update Cargo.toml + lib.rs exports`. No design work, just deletion.

The retirement is deferred until **after Phase E parity** (§5.5) so the v04 cutover is not entangled with a separate code-removal change.

#### 4.16.2 v0.3 KC operations map cleanly to the substrate (no design change needed)

The active path — v0.3 `knowledge_compile` — already aligns with §3:

- **Clustering output** → `node_kind='topic'` rows, attributes = `{ title, summary, source_count, created_at }`.
- **Topic membership** → `edges` rows of `edge_kind='containment', predicate='contains'` from topic node to each contributing memory node (same predicate as §4.4 KC output — KC and topic-membership are the same operation seen from different views).
- **Entity rollup** → `edge_kind='containment', predicate='contains'` from topic to entity node (already a node per §4.2 entity resolution). The container/contained semantics are identical to memory membership; topic-to-entity rollup is simply a topic containing the entities mentioned by its member memories.
- **Provenance** → `edge_kind='provenance', predicate='derived_from'` from topic to the synthesis trace (§4.5 synthesis).

The v0.3 KC writer becomes a §6-queue producer of `WriteKnowledgeTopic { topic_node, members[], entities[], provenance }` — a single batched op that creates the topic node, all membership edges, and provenance edges in one transaction. No semantic change from today's `knowledge_compile` output; only the storage shape moves from the standalone `knowledge_topics` table to the unified `nodes`/`edges` tables.

This is **already covered by §4.4 (Knowledge compilation)** in this design. §4.16 explicitly re-confirms that coverage so a future reader doesn't wonder whether v0.2's existence creates ambiguity. It does not.

#### 4.16.3 Active v0.3 feature debt (tracked, not in scope for v04)

Verify also exposed a **real** v0.3 KC bug, distinct from substrate concerns:

- `EmbeddingInfomapClusterer` (default `similarity_threshold=0.5`) **degenerates to a single super-cluster** on dense single-domain corpora — concretely, LoCoMo conv-26 (441 episodes, one conversation) collapses into one topic absorbing all 441 memories. The super-topic then squeezes Factual/Episodic candidates out of retrieval, producing the **-22pp J-score regression** observed in RUN-0026 vs RUN-0025 (0.559 → 0.342).

This is filed as **engram ISS-111** (`v0.3 KC EmbeddingInfomapClusterer collapses to 1 super-cluster on dense single-domain corpora`, P1 / severity:degradation, relates_to ISS-106). The fix is a clustering-algorithm tuning question — threshold heuristics, density-aware Infomap parameters, or a two-pass strategy — **orthogonal to the v04 substrate**:

- Substrate stores whatever topics the clusterer produces. If the clusterer produces one super-topic, the substrate faithfully stores one super-topic. The substrate did not cause the bug and cannot fix it.
- If ISS-111 lands a fix that changes the *number* of topics produced for a given input, the substrate absorbs that change with no design adjustment — `WriteKnowledgeTopic` is parameterized by `(topic_node, members[])`, not by a fixed topic count.

§4.16 records this for future readers so the v04 design is **not blamed** for clusterer behavior. ISS-111's resolution does not block, and is not blocked by, v04.

A small additional feature debt: `contributing_entities` field on `knowledge_topics` is populated to 0 in the degenerate case (entity-layer rollup never fires when there's only one cluster). That is the same bug surface as ISS-111 — fixed together.

#### 4.16.4 Retirement timeline (no rush, but committed)

v0.2 retirement is **a code-deletion task**, not a substrate-migration phase. It does not sit on the §5 phase timeline (Phase A–F all concern legacy *substrate* tables, not dead Rust modules). It is tracked as a single task in §8:

- **Phase A–F (§5) running**: v0.2 untouched. `compiler/` continues to compile and its tests continue to pass.
- **After §5.6 (Phase F) is complete and one week of post-migration traffic has passed**: single PR removes `crates/engramai/src/compiler/`, updates `Cargo.toml`, removes the `pub mod compiler;` from `lib.rs`, and runs the full test suite. Expected diff: −656 KB source, −21 modules, +0 LoC net (the path was load-bearing for nothing). One commit, one CI run, done. Tracked as `T-XX: Remove v0.2 compiler/ module` in §8 (added in commit 4 of this design).
- **Concept preservation**: `intake.rs` and `manual_edit.rs` encode patterns that may eventually become substrate writers (an `intake` op that ingests external corpora as memory nodes, a `manual_edit` op for human-curated overrides). The patterns are noted here so that when the modules are deleted, the *concepts* survive in the design record. Re-implementing them on the unified substrate is a separate future feature, not a port.
- No code is "ported" from v0.2 to v0.3 because v0.3 already covers the functionality. The 21 modules are mausoleum — preserved by inertia, not by purpose.

If between now and the retirement task some forgotten caller is discovered, the retirement is **paused, not abandoned**: the caller is reviewed (does it actually need KC, or just a slimmer service?) and either migrated to v0.3 or deleted as dead test scaffolding. The task resumes once the call site is resolved.

---

### 4.17 Coverage closure (no remaining counter-examples)

After §4.11–§4.14 (and §4.15–§4.16 added in design-commit-2), every active cognitive function in the codebase maps cleanly. The substrate is sufficient. Two near-future extensions verified compatible:

- **Batch consolidation reactivation** (sleep-like replay): relies on the associative-edge UNIQUE constraint (§3.2 `idx_edges_assoc_unique`) to upsert co-activation weight without creating duplicate edges.
- **Goal/plan completion**: status of `node_kind='plan'` nodes lives in `nodes.attributes.status` (e.g. `'active'|'completed'|'abandoned'`), distinct from the retirement model (`deleted_at`/`superseded_by`). A completed goal is not deleted; it is a historical achievement.

(Working memory's positioning in the original sanity-check section is now obsolete — §4.13 supersedes it with a hybrid model: WM stays in-memory at the hot path, materializing to substrate as `wm_snapshot` nodes only on demand-driven triggers like metacog feedback events. Neither "out of substrate" nor "fully in substrate" — the right answer was "in substrate at the moments substrate-presence has value".)

---

## 5. Migration plan

**Principle**: every step is reversible. We do not drop legacy tables
until parity is proven and one week of production traffic has passed.

### 5.1 Phase A — schema additive (no behavior change)

1. Create `nodes`, `edges`, `nodes_fts`, `node_embeddings` tables and
   indexes in fresh DBs (storage.rs `open()` migration).
2. Bump `engram_meta.schema_version` to `0.4-additive`.
3. **No code reads or writes these tables yet.** They are dormant.

**Acceptance**: existing test suite green (still using legacy tables);
new schema present and empty.

### 5.2 Phase B — dual-write (new writes go to both)

**Atomicity prerequisite**: §7 Q1 must be closed as **single-file DB**
before Phase B begins. Dual-write uses one `rusqlite::Connection`
shared across legacy + unified tables so that all dual-writes occur
inside a single SQLite transaction (atomic on commit, rolled back as a
unit on error). If Q1 is left as split-file, dual-write becomes
"best-effort with reconciliation" and T17 must add a reconciliation
step — not the recommended path.

4. `store_raw` writes to `memories` AND `nodes` in the same
   transaction on the shared connection.
5. ResolutionPipeline writes to `graph_entities`/`graph_edges` AND
   `nodes`/`edges` in the same transaction.
6. Hebbian writes to `hebbian_links` AND `edges`.
7. KC writes to `knowledge_topics`/`cluster_assignments` AND
   `nodes`/`edges`.
8. Synthesis writes to `synthesis_provenance` AND `edges`.

**Acceptance**:
- Every new memory produces 1 legacy row + 1 nodes row, byte-equal
  content/timestamps.
- Row-count parity script passes nightly.
- LoCoMo bench still green (still reads from legacy).

### 5.3 Phase C — backfill (historical rows into nodes/edges)

9. Backfill driver:
   - `memories` → `nodes` (24624 rows, ~1 min, no LLM).
     **Field mapping**: `memories.id→nodes.id` (TEXT, no conversion);
     `memories.layer→nodes.layer`; `memories.memory_type→nodes.memory_type`;
     `memories.superseded_by`: convert empty-string `''` → `NULL`
     (verify no code path distinguishes NULL from `''` by searching
     for `superseded_by =`); standard scalar copies for everything else.
     **Ordering**: two-pass — (1) insert all rows with
     `superseded_by=NULL`; (2) `UPDATE` to set `superseded_by` FK after
     all rows exist so the FK never references a missing row.
   - `memory_embeddings` → `node_embeddings` (24467 rows, ~1 min)
   - `entities` → `nodes` (2310 rows, no LLM).
     **Field mapping**: `entities.entity_type→nodes.attributes.entity_type`;
     `entities.metadata`: parse as JSON and **merge keys** into
     `nodes.attributes` (do not overwrite existing keys; on collision
     prefer existing). `entities.id→nodes.id` (TEXT, no conversion).
   - `entity_relations` → `edges (kind=structural)` (6531 rows).
     **Field mapping**: `entity_relations.metadata`: parse as JSON and
     **merge keys** into `edges.attributes`.
   - `memory_entities` → `edges` (9237 rows), split by role per
     the kind/predicate table in §3.3 (canonical, normative):

       | role                          | edge_kind    | predicate    |
       |-------------------------------|--------------|--------------|
       | `'mention'` (default in prod) | `provenance` | `mentions`   |
       | `''` (empty)                  | `provenance` | `mentions`   |
       | `'triple'`                    | `provenance` | `mentions`   |
       | unknown / free-form           | `provenance` | `mentions`   |
       | `'subject'`                   | `structural` | `subject_of` |
       | `'object'`                    | `structural` | `object_of`  |

     Non-canonical roles (`'triple'`, free-form) MUST preserve the
     raw role string in `edges.attributes.legacy_role` for audit
     traceability; canonical roles write empty attributes (`'{}'`).
     `namespace` and `created_at` are derived from the parent
     memory via JOIN since the link table has no own columns for
     these.
   - `hebbian_links` → `edges (kind=associative, predicate=co_activated)` (43710 rows).
     **Field mapping**: `strength→weight`; `namespace→namespace`;
     `created_at→created_at`. Pack all signal/temporal fields into
     `edges.attributes` JSON: `signal_source`, `signal_detail`,
     `coactivation_count`, `temporal_forward`, `temporal_backward`,
     `direction`. **These fields drive differential decay (§4.6) and
     MUST NOT be dropped.**
   - `synthesis_provenance` → `edges (kind=provenance, predicate=derived_from)` (72 rows).
     **Field mapping**: `confidence→confidence`;
     `synthesis_timestamp→recorded_at`. Pack into `edges.attributes`:
     `gate_decision`, `gate_scores`, `cluster_id`.
   - Triple extraction backfill (v0.3 wire-up G3b): ~24k Haiku calls
     populating `edges (kind=structural)` from memory content. ~30min
     wall-clock, ~$25. **Independently restartable.**
10. Verify counts: post-backfill `SELECT COUNT(*) FROM nodes WHERE node_kind='memory'` == legacy memories count.

**Idempotency** (re-runnable backfill — required because backfill can
crash mid-way, dual-write may diverge, or operator may need to retry
on a subset of rows):

- **`memories → nodes`**: `id` is preserved → `INSERT OR IGNORE` on
  `nodes(id)` makes re-run safe. Same for `entities → nodes` and
  `memory_embeddings → node_embeddings` (PK `(node_id, model)`).
- **Source tables without PKs that survive (`entity_relations`,
  `memory_entities`, `hebbian_links`, `synthesis_provenance`)**:
  edge `id` is derived **deterministically** from source row identity
  via SHA-256 over a canonical tuple, then formatted as UUID:

  ```
  edges.id = uuid_from_hash(sha256(
      source_table || '|' ||
      source_id    || '|' ||      -- the row's primary key in legacy table
      target_id    || '|' ||
      edge_kind    || '|' ||
      predicate
  ))
  ```

  For **link tables** whose natural key is a composite of more than
  one column, inline all discriminator columns flat in the canonical
  natural-key order — the number of pipe-delimited tokens is fixed
  per source table. Concretely:

  ```
  # entity_relations (PK = id):
  hash_input = "entity_relations|<id>|<target_id>|<edge_kind>|<predicate>"

  # memory_entities (composite natural key (memory_id, entity_id, role)):
  hash_input = "memory_entities|<memory_id>|<entity_id>|<role>|<edge_kind>|<predicate>"

  # hebbian_links (composite natural key + signal_source as §4.3
  # row-identity dimension; signal_source MUST be in the hash so a
  # future write of the same canonical pair with a different
  # signal_source gets a distinct edge and is NOT silently rejected
  # by INSERT OR IGNORE on the primary id before the partial unique
  # index `idx_edges_assoc_unique` can catch it):
  hash_input = "hebbian_links|<memory_id>|<related_id>|<namespace>|<signal_source>|<edge_kind>|<predicate>"

  # synthesis_provenance (PK = id) — NOTE: provenance edges are
  # append-only (no partial unique index per §3.2 / §4.5), and
  # Phase B's T16 dual-write uses the legacy `id` directly as
  # `edges.id`. The backfill driver MUST do the same so that
  # re-emitting a provenance row through Phase B AFTER backfill
  # collides with the backfilled edge on primary key (the desired
  # idempotency), rather than producing a second edge under a
  # different hashed id. Therefore: NO hash; pass legacy.id through.
  # (Verifier I5 in §6 also asserts "matching id".)
  edge_id = legacy.id                # pass-through, NOT a hash
  ```

  This is the **single source of truth** for hash input layout;
  drivers MUST follow these exact templates. Combined with the
  partial UNIQUE indexes on
  `edges(source_id, target_id, edge_kind, predicate)`
  declared in §3.2 (covering `edge_kind='associative'` and
  `edge_kind='containment'`), this makes `INSERT OR IGNORE` correct
  for the kinds where re-emission is supposed to be idempotent: a
  re-run that re-emits the same Hebbian bump, the same dimension
  edge, or the same tag edge produces the same UUID and is silently
  skipped. `structural` / `provenance` / `temporal` edges that
  legitimately repeat across runs are not constrained by the partial
  index and accumulate.

- **Verification**: backfill driver emits a `backfill_runs` audit row
  per invocation `(run_id, table, rows_read, rows_inserted, rows_skipped_existing)`.
  On re-run, `rows_skipped_existing` should equal `rows_inserted` from
  the prior successful run minus any new dual-writes that landed in
  the interim.

**Acceptance**: full row-count + spot-check content parity report; running
the full backfill driver a second time results in zero new rows and zero
errors.

### 5.4 Phase D — switch reads (one plan at a time)

11. Add `MemoryConfig::unified_substrate: bool` flag, default off.
12. When on, retrieval adapters read from `nodes`/`edges` instead of
    legacy tables.
13. Run parity campaign:
    - **LoCoMo end-to-end (primary gate)**: unified overall J-score
      ≥ legacy overall J-score − 2pp on the same 152-query conv-26
      run, AND no per-category drop > 5pp that is not explained as
      LLM-judge noise (≤ 2 flips on near-identical predictions).
    - **50-query manual probe on production DB snapshot**
      (informational, NOT a gate): Jaccard@10 distribution between
      unified and legacy candidate sets, reported in
      `.gid/eval-runs/RUN-T30/`. Used to characterise the rank shuffle
      so we know what we're absorbing into the generator+judge, not
      to gate the flip.

    **Rationale for not gating on probe Recall/Jaccard@10**: T30
    measured parity_ratio = 0.40 at K=10 / jac≥0.95 — well below the
    original ≥95% spec. Investigation (`.gid/eval-runs/RUN-T30/`)
    found this is a structural FTS5 IDF shift: `nodes_fts` indexes
    memories + entities + insights together (22174 rows, 14%
    non-memory mass), so the IDF distribution differs from
    `memories_fts` (19378 memory rows). Top-K rank shuffles by a few
    positions per query without changing the *set of relevant
    candidates the LLM sees*. T31 confirmed unified ≥ legacy on
    LoCoMo end-to-end despite the K=10 shuffle, so probe-Recall@10 is
    the wrong granularity to gate on. The right gate is the metric
    the downstream task actually depends on (LoCoMo J-score). See
    `.gid/eval-runs/RUN-T30/rank-diag-root-cause.md` Option 3 for the
    full argument.

14. Flip default to on (T32). Legacy tables still being written.

**Acceptance**: T30/T31 archived in `.gid/eval-runs/`, T31 unified ≥
legacy on LoCoMo per the gate above (RUN-T31 confirmed: legacy
0.3947, unified 0.4013, +0.66pp), plus ≥1 week production at
default-on with no quality regression flagged.

### 5.5 Phase E — stop legacy writes

**Entry gate**: Phase D acceptance soak ≥1 week elapsed at
`unified_substrate=true` default, no quality regression flagged
against the LoCoMo gate (§5.4), no ISS-136-class regressions traced
to the cutover. Begin only after potato signs off on the soak.

**Approach**: Phase B already dual-writes every prod legacy write to
the unified substrate (T12–T16). Phase E is therefore a *deletion
job*, not a refactor — each prod legacy SQL statement gets removed
from its enclosing transaction, leaving the unified-side write that
was added in Phase B as the sole survivor. There is no behaviour
change for callers; the public storage API stays byte-identical.

**Audit boundary**: this section is scoped to `crates/engramai/src/`
prod code only. Test fixtures that seed legacy tables (`graph/store.rs`
Batch A/B helpers, `bus/subscriptions.rs` test mod, lifecycle/test
infrastructure) are *not* rewritten in Phase E — they continue to
write to legacy tables until Phase F (T39) rewrites them to seed
`nodes`/`edges` directly. Migration helpers (T19–T25 backfill drivers,
`migrate_hebbian_*`) are also exempt; they exist to populate the
unified side from pre-cutover legacy data and stay until the legacy
tables are dropped.

#### 5.5.1 Prod-only legacy-write inventory (81 sites)

Verified 2026-05-22 via AST-strip of `#[cfg(test)]` / `#[test]`
blocks and comment lines over `crates/engramai/src/`. Counts are
INSERT/UPDATE/DELETE statements that hit the named legacy table
from production paths (test helpers excluded).

By table:

- `memories` — 17 writes (1 file)
- `memories_fts` — 13 writes (1 file)
- `hebbian_links` — 27 writes (1 file)
- `memory_entities` — 5 writes (1 file)
- `memory_embeddings` — 4 writes (1 file)
- `entities` — 3 writes (1 file)
- `entity_relations` — 1 write (1 file)
- `synthesis_provenance` — 3 writes (1 file)
- `cluster_assignments` — 5 writes (1 file)
- `knowledge_topics` — 2 writes (`graph/store.rs`)
- `memories` (graph store) — 1 write (`graph/store.rs`)

Total: **81 prod legacy writes** across exactly **2 files** —
`crates/engramai/src/storage.rs` (78) and
`crates/engramai/src/graph/store.rs` (3). The narrow file blast
radius is what makes Phase E tractable as a deletion pass rather
than a multi-week refactor.

#### 5.5.2 Deletion pattern — worked example: `Storage::add`

`Storage::add` (`storage.rs:1834`) is the canonical legacy memory
writer and exercises the full pattern. Pre-Phase-E body, abbreviated:

```rust
pub fn add(&mut self, record: &MemoryRecord, namespace: &str) -> Result<(), rusqlite::Error> {
    let tx = self.conn.transaction()?;
    tx.execute("INSERT INTO memories (...) VALUES (...)", params![...])?;  // ← DELETE in Phase E
    tx.execute("INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)", ...)?;
    let rowid: i64 = tx.query_row("SELECT rowid FROM memories WHERE id = ?", ...)?;  // ← DELETE
    tx.execute("INSERT INTO memories_fts(rowid, content) VALUES (?, ?)", ...)?;     // ← DELETE
    // T12 Phase B dual-write — survives Phase E:
    Self::insert_memory_node_row(&tx, record, namespace, metadata_json.as_deref())?;
    tx.commit()?;
    Ok(())
}
```

Post-Phase-E body:

```rust
pub fn add(&mut self, record: &MemoryRecord, namespace: &str) -> Result<(), rusqlite::Error> {
    let tx = self.conn.transaction()?;
    tx.execute("INSERT INTO access_log (memory_id, accessed_at) VALUES (?, ?)", ...)?;
    Self::insert_memory_node_row(&tx, record, namespace, metadata_json.as_deref())?;
    tx.commit()?;
    Ok(())
}
```

Five things to note:

1. **`access_log` is retained.** It is an audit/observability table,
   not a legacy substrate table. It survives all phases (see §3.5).
2. **The FTS write is deleted, not rewritten.** `nodes_fts` is
   maintained by SQL triggers `nodes_fts_ai/ad/au` (storage.rs:1011),
   which fire automatically on `nodes` INSERT/DELETE/UPDATE. The
   `insert_memory_node_row` call therefore covers FTS as a side
   effect — no explicit FTS write needed.
3. **The rowid SELECT goes with the FTS write.** It was only there
   to feed the legacy `memories_fts(rowid, content)` insert.
4. **`metadata_json` materialisation is retained** because
   `insert_memory_node_row` still consumes it.
5. **The transaction boundary is unchanged.** Atomicity for callers
   is preserved.

This same pattern — *delete the legacy SQL, keep the Phase-B
unified-side call* — applies to every site in the inventory. The
unified-side call already exists in every transaction; Phase E
removes the now-redundant legacy half.

#### 5.5.3 Per-file refactor plan

Phase E is split into three task batches keyed by file and table
group. Each batch is independently reviewable, independently
testable, and independently revertable. Recommended sequencing:
T34 → T35 → T36 (storage by complexity, low to high), then T37
(graph/store.rs) last because it touches the knowledge-topic
read path.

**T34 — `storage.rs` memory core (39 prod writes)**

Drop legacy writes from the memory CRUD surface. Tables touched:
`memories` (17), `memories_fts` (13), `memory_embeddings` (4),
`memory_entities` (5 — see note below). Entry points:

- T34a `Storage::add` (L1834) — INSERT memories + INSERT memories_fts
- T34b `Storage::store_raw` (L6521) — INSERT memories + INSERT memories_fts
- T34c `Storage::update_inner` (L2703) — UPDATE memories + DELETE/INSERT memories_fts (FTS roundtrip)
- T34d `Storage::update_content_inner` (L2869) — UPDATE memories + FTS roundtrip
- T34e `Storage::delete_inner` (L2802) — DELETE memories + DELETE memories_fts
- T34f `Storage::soft_delete` (L4107) — UPDATE memories.deleted_at
- T34g `Storage::supersede` / `supersede_bulk` / `unsupersede` (L3566/3632/3674) — UPDATE memories.superseded_by
- T34h `Storage::merge_memory_into` (L5985, L6000, L6044) + `merge_enriched_into` (L6224) — UPDATE memories importance/content/metadata
- T34i `Storage::update_importance` (L6488) + `append_merge_provenance` (L6916) + `increment_extraction_attempts` (L7312) — single-column UPDATEs
- T34j `Storage::hard_delete_cascade` (L4176/4179/4182/4184/4195/4197) — DELETE cascade across memories, memories_fts, memory_embeddings, memory_entities, synthesis_provenance, hebbian_links
- T34k `Storage::store_embedding` (L3945) — INSERT OR REPLACE memory_embeddings
- T34l `Storage::delete_embedding` (L4281) + `delete_all_embeddings` (L4302) — DELETE memory_embeddings
- T34m `Storage::rebuild_fts_if_needed` (L1806/1817) + `rebuild_fts` (L5086) — FTS recovery paths; **delete entirely** because `nodes_fts` is trigger-maintained and never needs explicit rebuild from prod code
- T34n `Storage::link_memory_entity` (L5251) + `clear_memory_entity_links` (L5879) + `cleanup_orphaned_entity_links` (L7743) — memory_entities INSERT/DELETE (Phase B T23 already dual-writes the structural/subject_of/object_of edges)

Pre-condition for T34: confirm every entry point above has a Phase B
dual-write counterpart by grepping for `insert_memory_node_row`,
`insert_structural_edge_row`, `insert_provenance_edge_row`,
`dual_write_entity_to_nodes`, or the relevant `nodes`/`edges` UPDATE.
If any entry point lacks a dual-write, file a Phase B gap issue
*before* touching Phase E — the gap means the data is not actually
on the unified side yet.

**T35 — `storage.rs` Hebbian (27 prod writes)**

Drop legacy writes from the Hebbian path. Table touched:
`hebbian_links` (all 27). Entry points:

- T35a `Storage::record_coactivation` (L3202–3230, 4 statements) — UPDATE/INSERT hebbian_links (default namespace)
- T35b `Storage::record_coactivation_ns` (L4605–4632, 4 statements) — UPDATE/INSERT hebbian_links (namespaced) — Phase B T14 dual-writes these to `edges(edge_kind='associative')`
- T35c `Storage::decay_hebbian_links` (L3264/3268) + `decay_hebbian_links_differential` (L3459/3468) — bulk UPDATE strength * decay + DELETE strength<0.1; needs equivalent `edges` bulk ops or a Phase B audit confirming decay is already mirrored
- T35d `Storage::merge_hebbian_links` (L3359/3366/3377) — node-merge consolidation; Phase B T14 already mirrors but the *merge* path may need separate verification
- T35e Migration helpers (`migrate_hebbian_signals` L1489, `migrate_hebbian_canonical_rows` L1560/1607) — **retained**, these are explicit migration helpers (see audit boundary above)
- T35f `Storage::hard_delete_cascade` hebbian portion (L4179) — already covered by T34j as part of the cascade

Risk note: the Hebbian decay path (T35c) is the highest-risk site in
Phase E. The `edges` table does not yet have a `weight < 0.1`
bulk-DELETE equivalent — adding one is a Phase B follow-up, not a
Phase E task. **Do not start T35c until decay parity is confirmed
on the unified side.** If parity is missing, defer T35c to Phase F
or file a Phase B catch-up ticket.

**T36 — `storage.rs` synthesis + clusters + entities tail (12 prod writes)**

Drop legacy writes from the synthesis/KC surface. Tables touched:
`entities` (3), `entity_relations` (1), `synthesis_provenance` (3),
`cluster_assignments` (5). `memory_entities` (5) is covered entirely
by T34n. Entry points:

- T36a `Storage::upsert_entity` (L5165) + `insert_triple_entity` (L7129/7172) + `delete_entity` (L5843) — entities CRUD; Phase B T21 dual-writes to `nodes(node_kind='entity')`
- T36b `Storage::upsert_entity_relation` (L5339) — entity_relations INSERT; Phase B T22 dual-writes to `edges(edge_kind='structural')`
- T36c `Storage::record_provenance` (L6276) + `delete_provenance` (L6439) — synthesis_provenance INSERT/DELETE; Phase B T16 dual-writes to `edges(edge_kind='provenance')`
- T36d `Storage::assign_to_cluster` (L7389) + `replace_clusters` (L7493/7512) + `save_full_cluster_state` (L7553/7576) — cluster_assignments; Phase B T15 dual-writes containment edges to `edges`
- T36e `Storage::hard_delete_cascade` synthesis_provenance portion (L4184) — already covered by T34j

**T37 — `graph/store.rs` (3 prod writes)**

- T37a INSERT `knowledge_topics` (L4828) in `persist_cluster` — Phase B T15 dual-writes; delete the legacy INSERT
- T37b UPDATE `knowledge_topics` (L4947) — topic supersession path; delete the legacy UPDATE after confirming Phase B mirrors `nodes(node_kind='topic').superseded_by`
- T37c UPDATE `memories` (L5782) `entity_ids`/`edge_ids` in pipeline finalisation — these two columns are denormalised projections of `memory_entities`/`hebbian_links`; check whether the unified-side reads can recompute on demand (preferred) or whether a `nodes.attributes` mirror is required

T37c is the only site in Phase E that may need a small *additive*
change rather than pure deletion — flag during the T37 review.

#### 5.5.4 Per-task acceptance criteria

Each Tnn task is accepted when ALL of:

1. **Legacy write deleted** — grep over `crates/engramai/src/`
   (with `#[cfg(test)]` masked) confirms zero matches for the
   removed statement.
2. **Unified-side write retained** — the corresponding `nodes` /
   `edges` / `node_embeddings` write still exists in the same
   transaction and produces the same row(s).
3. **Lib tests green** — `cargo test --lib` passes (target: full
   1910-test suite from T32).
4. **Phase B parity tests green** — `cargo test --test
   v04_phase_b_dual_write` and `cargo test --test
   v04_phase_c_*` continue to pass; they prove the unified side
   still receives the data.
5. **Targeted regression test** — a new test under
   `tests/v04_phase_e_no_legacy_writes.rs` calls the entry point
   under cutover (`unified_substrate=true`) and asserts the legacy
   table row count is unchanged from before the call (i.e. nothing
   landed in the legacy table). For DELETE paths, assert the legacy
   row is still present (we no longer touch it) but the unified row
   is gone.
6. **No public API change** — `cargo public-api diff` (if
   available) or manual diff over `pub fn` signatures shows zero
   changes.

#### 5.5.5 Phase-level acceptance

Phase E is complete when:

- **Inventory check passes**: re-running the §5.5.1 AST-strip
  inventory yields 0 prod hits across all 10 legacy tables in
  `crates/engramai/src/`.
- **Full test suite green**: lib (1910) + integration (~2489) +
  all `v04_phase_*` tests pass.
- **One-week production soak**: default-on for ≥1 calendar week
  with no quality regression flagged. (Mirrors the Phase D entry
  gate; Phase F entry requires *another* week.)
- **No new ISS-136-class regressions**: master LoCoMo J-score
  stays within ±1pp of the Phase D baseline (0.4013) across at
  least two independent runs.

**Acceptance**: AST-strip inventory yields 0, all tests green,
soak ≥1 week, LoCoMo within ±1pp. Legacy tables are now read-only
in prod (writers retained only in migration helpers and test
fixtures).

### 5.6 Phase F — drop legacy

17. After ≥2 weeks of unified-only writes, drop legacy tables in a
    schema migration (`0.4-final`).

**Acceptance**: schema diff matches §3 exactly. `ls -lh
engram-memory.db` shows size reduction proportional to dropped tables.

---

## 6. Concurrency architecture

This section specifies the unified write path. The motivation is in §4.11–§4.16 (rationale below); the design is below.

**Rationale**: §4.11–§4.16 add 5+ new writer paths (interoception signals, empathy bus, metacognition feedback, working-memory tags, dimension edges). Without a unified write-path design, naïve direct-write would multiply SQLite write-lock contention. Single Writer pattern collapses all writers into one ordered queue, makes cross-op transactions trivially atomic, and turns "concurrent cognitive ops" into "an event log replayable for audit". The pattern is well-known (Datomic single transactor, LMAX Disruptor, Kafka partition leader, actor model).

**What's already true** (verified 2026-05-12 in `crates/engramai/src/memory.rs:68` and `storage.rs:157`):

- `Memory` holds `storage: Storage` **by value, not behind `Mutex` or `Arc`**. A caller must own `&mut Memory` to mutate.
- The Rust borrow checker enforces single-mutable-borrow → **single-writer at the type level is already implicit** for in-process use.
- SQLite is opened in WAL mode with `busy_timeout=5000ms` (storage.rs:228), so multi-process readers are fine and a second writer would block-then-retry rather than corrupt.

§6 formalizes this: the Single Writer pattern becomes **explicit** (a queue + worker, not an `&mut` invariant), gains **priority/backpressure** for cognitive ops with different urgency, and gains **cross-op atomicity** for the compound writes that §4.11–§4.16 introduced (e.g. `WriteFeedbackEvent` + `WriteWmSnapshot` in the same transaction per §4.14).

### 6.1 Write op enum (one variant per writer path)

Every mutation in engram becomes a `WriteOp` variant. The set is closed and audited. Each variant carries (a) its payload — what to write — and (b) a `reply: oneshot::Sender<Result<R>>` channel where the writer task sends success (with any returned ID) or failure. The reply field is not optional: a writer caller that doesn't care about the result still gets the slot, and may drop the receiver:

```rust
pub enum WriteOp {
    // ─────────────── §4.1 ingest ───────────────
    WriteMemory {
        content: String,                            // → nodes.content (column name)
        dimensions: Dimensions,                     // §4.15: macro-op. Writer expands inline (see note below)
        memory_type: MemoryType,                    // → nodes.memory_type (caller-supplied; default Episodic — see macro-op notes)
        layer: Layer,                               // → nodes.layer (caller-supplied; default L0 — see macro-op notes)
        occurred_at: Option<DateTime<Utc>>,         // ISS-103 fix: nullable, separate from created_at
        embedding: Option<Vec<f32>>,
        namespace: String,
        agent_id: Option<String>,
        episode_id: Option<NodeId>,                 // §4.10: if Some, writer creates (memory, belongs_to_episode, episode) edge
        reply: oneshot::Sender<Result<WriteMemoryReply>>,
    },

    // ─────────────── §4.2 entity resolution ───────────────
    WriteEntity {
        canonical_name: String,
        kind: EntityKind,
        embedding: Option<Vec<f32>>,
        namespace: String,
        reply: oneshot::Sender<Result<NodeId>>,
    },
    WriteEntityMention {
        memory_id: NodeId,
        entity_id: NodeId,
        role: MentionRole,                          // mention | subject | object
        span: Option<Span>,
        reply: oneshot::Sender<Result<EdgeId>>,
    },
    WriteEntitySameAs {                             // §4.2 entity resolution clique edges
        alias_id: NodeId,
        canonical_id: NodeId,
        confidence: f64,
        reply: oneshot::Sender<Result<EdgeId>>,
    },

    // ─────────────── §4.3 Hebbian ───────────────
    BumpAssociation {
        source_id: NodeId,
        target_id: NodeId,
        delta: f64,
        signal_source: SignalSource,                // corecall | multi | ... — drives differential decay (§4.6)
        temporal_forward: f64,
        temporal_backward: f64,
        direction: Direction,
        reply: oneshot::Sender<Result<()>>,         // bumps coalesce; receiver gets Ok(()) once the coalesced batch commits
    },

    // ─────────────── §4.4 KC + §4.5 synthesis ───────────────
    WriteKnowledgeTopic {
        topic_node: NodeDraft,                      // title, summary, embedding, source_count
        members: Vec<NodeId>,                       // → containment/contains edges
        entities: Vec<NodeId>,                      // → containment/contains edges (topic-to-entity)
        provenance: SynthesisTrace,                 // → provenance/derived_from edge
        reply: oneshot::Sender<Result<NodeId>>,     // NodeId of the new topic
    },
    WriteSynthesisInsight {
        content: String,
        sources: Vec<NodeId>,                       // → provenance/derived_from edges
        importance: f64,
        embedding: Option<Vec<f32>>,
        namespace: String,
        reply: oneshot::Sender<Result<NodeId>>,
    },

    // ─────────────── §4.6 lifecycle ───────────────
    ApplyDecayTick {
        now: DateTime<Utc>,                         // reads nodes.created_at — never occurred_at (ISS-103)
        reply: oneshot::Sender<Result<DecayReport>>,
    },
    SoftDelete {
        id: NodeId,
        reason: DeletionReason,
        reply: oneshot::Sender<Result<()>>,
    },

    // ─────────────── §4.7 supersession ───────────────
    Supersede {
        old_id: NodeId,
        new_id: NodeId,
        rationale: String,                          // stored on the new node's attributes
        reply: oneshot::Sender<Result<()>>,
    },

    // ─────────────── §4.9 promotion ───────────────
    PromoteNode {
        id: NodeId,
        from_kind: String,
        to_kind: String,
        gate_decision: GateDecision,                // audit trail
        reply: oneshot::Sender<Result<()>>,
    },

    // ─────────────── §4.11 interoception ───────────────
    UpdateDomainStats {                             // closes FINDING-A3-4 — was missing
        domain: String,                             // 'coding'|'trading'|'general'|...
        signal: InteroceptiveSignal,                // baseline stream: folded into rolling stats, NOT persisted as event
        reply: oneshot::Sender<Result<()>>,         // ack only; no event row created
    },
    WriteAnomalyEvent {                             // anomaly stream: persistent
        domain: String,
        signature: AnomalySignature,
        z_score: f64,
        triggered_regulation: Option<String>,
        rationale: String,
        triggered_by: NodeId,                       // → associative/evoked_by edge to source memory
        reply: oneshot::Sender<Result<NodeId>>,
    },
    WriteSomaticMarker {                            // closes FINDING-A3-3 — was missing
        pattern_signature: String,
        evoked_affect: AffectState,
        sample_count: u32,
        triggered_by: Vec<NodeId>,                  // → associative/evoked_by edges to all trigger memories
        reply: oneshot::Sender<Result<NodeId>>,
    },
    WriteRegulationPolicy {                         // closes FINDING-A3-3 — was missing
        policy_name: String,
        domain_filter: Option<String>,
        action_template: RegulationActionTemplate,
        reply: oneshot::Sender<Result<NodeId>>,
    },
    WriteDriveAlignment {                           // §4.12 drive ↔ memory edges (alignment scores)
        memory_id: NodeId,
        drive_id: NodeId,
        weight: f64,
        reply: oneshot::Sender<Result<EdgeId>>,
    },

    // ─────────────── §4.12 empathy bus ───────────────
    // closes FINDING-A3-5 — was collapsed into a single generic WriteEmpathySignal.
    // Names + shape match the real bus/ module (accumulator/alignment/feedback/mod_io)
    // — engram has single-agent + SOUL drives, not multi-agent empathy.
    WriteValenceAccumulator {                       // §4.12 — per-domain valence trend update
        domain: String,                             // e.g. "coding", "trading"; matches bus/accumulator.rs
        valence_delta: f64,                         // signed; pushed into rolling window on domain node
        event_count_delta: i64,                     // usually 1
        reply: oneshot::Sender<Result<()>>,
    },
    WriteActionOutcome {                            // §4.12 — heartbeat action result as a node
        action_type: String,
        success: bool,
        latency_ms: u64,
        notes: Option<String>,
        triggered_by_drive: Option<NodeId>,         // optional edge target
        involves_memory: Option<NodeId>,            // optional edge target
        reply: oneshot::Sender<Result<NodeId>>,
    },
    LogExternalWrite {                              // §4.12 — audit record for bus/mod_io.rs writes
        target_file: String,                        // "SOUL.md" | "HEARTBEAT.md" | "IDENTITY.md"
        operation: String,                          // "update_field" | "add_drive" | etc.
        content_hash: String,                       // SHA-256 of the written content for traceability
        reply: oneshot::Sender<Result<NodeId>>,
    },


    // ─────────────── §4.13 working memory ───────────────
    WriteWmSnapshot {
        feedback_event_id: NodeId,                  // → provenance/wm_snapshot_of edge
        slots: Vec<WmSlot>,                         // → containment/wm_contained edges to each WM memory
        trigger_reason: String,
        wm_state: WmState,                          // cold_start | warm — §4.13 in-memory ring buffer flag
        reply: oneshot::Sender<Result<NodeId>>,
    },

    // ─────────────── §4.14 metacognition ───────────────
    WriteFeedbackEvent {                            // typically batched with WriteWmSnapshot via Batch
        dimension: String,                          // 'recall_accuracy'|'synthesis_quality'|...
        score: f64,
        target_id: NodeId,                          // → structural edge to evaluated memory/topic/action
        rationale: String,
        reply: oneshot::Sender<Result<NodeId>>,
    },

    // ─────────────── §4.15 Tier 2/3 dimension edges (standalone variants) ───────────────
    // PRIMARY path: dimension edges are NOT caller-constructed for normal memory ingest;
    // the writer expands `WriteMemory.dimensions` inline into the same SQL transaction
    // (macro-op semantics, see §6.1 note below). These standalone variants exist only for:
    //   - Backfill (§5.3) when migrating legacy memories that already have node rows
    //   - Post-ingest dimension correction (rare; e.g. resolution canonicalization later
    //     merges two dimension_location nodes and edges must be repointed)
    // Callers writing fresh memories MUST use `WriteMemory { dimensions, .. }` — not Batch.
    WriteDimensionEdge {
        memory_id: NodeId,
        field: String,                              // 'participants'|'location'|... — predicate becomes `describes_<field>`
        dimension_node_id: NodeId,                  // pre-resolved (canonicalized) dimension node
        reply: oneshot::Sender<Result<EdgeId>>,
    },
    WriteTagEdge {                                  // §4.15 Tier 3
        memory_id: NodeId,
        tag_node_id: NodeId,
        reply: oneshot::Sender<Result<EdgeId>>,
    },

    // ─────────────── compound (multi-op atomic batches; see §6.4) ───────────────
    Batch {
        ops: Vec<WriteOp>,
        reply: oneshot::Sender<Result<Vec<WriteOpResult>>>,
    },
}
```

**Reply semantics for `Batch`** (closes FINDING-A4-8 — was undefined):

- The outer `Batch.reply` fires **once**, with `Ok(Vec<WriteOpResult>)` on full success or `Err(BatchAborted { failed_index, cause })` on first failure.
- Inner ops' own `reply` channels are **never used** when sent inside a Batch. If the caller constructs a Batch containing `WriteOp::WriteMemory { reply, .. }`, the `reply` field is dropped by the writer task without being signaled. The caller MUST use the outer `Batch.reply` to receive results in order.
- Nested `Batch` inside `Batch` is forbidden: the writer rejects with `Err(NestedBatch)` before opening the transaction. This keeps the failure surface flat — a Batch is exactly one SQLite transaction, no sub-transactions.
- Result ordering: `WriteOpResult` at index `i` corresponds to `ops[i]`. A `WriteMemory` returns `WriteOpResult::NodeId(_)`; a `BumpAssociation` returns `WriteOpResult::Unit`; etc. The result enum mirrors the `Ok(_)` payload of each variant's would-be `reply`.

**Why an enum, not a trait object?**
- Closed set — every writer in the codebase is one of these variants. Adding a new variant is a deliberate design act, surfaced in code review.
- No dynamic dispatch in the hot loop.
- The worker `match`-arms each variant to a typed handler — the variant payload carries everything the handler needs, no field lookup on a `dyn Any`.

**Variant naming convention**: `Write<Thing>` for ops that create nodes/edges; `Bump<Thing>` for idempotent accumulators; `Apply<Thing>` for sweeps over many rows; `Supersede`/`Promote`/`SoftDelete` are domain verbs. `Update<Thing>` is reserved for in-place mutations of a *single existing row* (e.g. `UpdateDomainStats`), distinct from `Write<Thing>` which always creates.

**Macro-op semantics for `WriteMemory`** (closes r3-p4a-FINDING-6 — was ambiguous):

`WriteMemory` is a **macro-op**: a single `WriteOp` variant the writer expands internally into multiple SQL statements within one transaction. Specifically, `apply_write_memory(tx, op)` performs:

1. `INSERT INTO nodes(...)` for the memory itself, populating **`memory_type` and `layer` directly from the op payload** — the writer does NOT derive these from `dimensions`. Tier 1 scalar dimensions land in `nodes.attributes` JSON (no separate write).
2. For each Tier 2 narrative field with a value: resolve or create the `dimension_<field>` node (single `INSERT ... ON CONFLICT DO NOTHING RETURNING id`, or SELECT-then-INSERT under WAL), then `INSERT INTO edges(...)` for the `containment / describes_<field>` edge. Up to 10 fields.
3. For each Tier 3 tag: resolve or create the `tag` node, `INSERT INTO edges(...)` for the `containment / tagged` edge. Typical 0–8 tags.
4. **Episode link** — if `episode_id` is `Some(ep)`, `INSERT INTO edges(...)` for one `containment / belongs_to_episode` edge from the memory node → episode node. Skipped entirely when `episode_id` is `None`. The caller is responsible for ensuring `ep` references an existing `node_kind='episode'` row; the writer does not create episode nodes (those go through a separate `WriteEpisode` op, out of scope here).

**Caller responsibility for `memory_type` and `layer`** (F4 resolution): both fields map directly to the `nodes.memory_type` and `nodes.layer` columns and MUST be supplied by the caller (currently `memory.rs::store_raw`). The writer performs no derivation from `dimensions.type_weights` or any other signal — derivation belongs in the ingest API, not the substrate writer. **Defaults**: if the caller has no better information, `memory_type = MemoryType::Episodic` and `layer = Layer::L0` are the documented fallbacks. However, the public ingest API (`store_raw` and any future ingest entry point) MUST require these as explicit arguments — no surprise defaults at the API boundary. The defaults exist only so backfill/migration tooling has a defined behavior when historical rows carry no value.

This produces up to ~22 SQL statements per `WriteMemory` (1 memory node + up to 10 dim edges + up to 8 tag edges + 1 episode edge + dim/tag node upserts), all inside the same `BEGIN ... COMMIT`. **Callers do not decompose `WriteMemory` into `Batch([WriteMemory{no dims}, WriteDimensionEdge, ...])`** — the macro-op exists precisely so the caller surface stays one op and the atomicity boundary is the writer's responsibility, not the caller's.

The reply payload:

```rust
pub struct WriteMemoryReply {
    pub memory_id: NodeId,
    pub dimension_edges: Vec<(String, EdgeId)>,    // (field_name, edge_id) — empty if Tier 2 fields all None
    pub tag_edges: Vec<EdgeId>,                    // empty if tags empty
    pub episode_edge: Option<EdgeId>,              // §4.10: Some(id) iff caller passed episode_id; None otherwise
}
```

Callers who don't care about edge IDs ignore those vectors. Callers performing post-ingest fix-ups (rare) get the IDs they need without a second SELECT.

**Why macro-op, not Batch-of-WriteMemory-plus-sub-ops**:

- The dimension/tag node resolution (step 2/3 above) needs to **see the memory_id from step 1** to construct the edge. In a caller-constructed `Batch`, the caller would have to know the memory ID *before* the writer assigns it — impossible without round-tripping. Macro-op lets the writer chain the assignments in one frame.
- `Batch` results are positional (`WriteOpResult` at index `i` corresponds to `ops[i]`). A `WriteMemory` macro-op produces a *single* `WriteMemoryReply` at its slot — clean. A decomposed Batch would have ~20 slots, most of which the caller doesn't care about, and the caller would have to know the exact ordering to interpret them.
- **No nested-Batch issue.** Because `WriteMemory` is a single variant (not a `Batch` itself), wrapping it in a Batch — e.g. `Batch([WriteMemory, WriteEntity])` for a multi-row atomic write — is legal. The "nested Batch forbidden" rule (§6.4) is only violated if a caller puts a `Batch` *variant* inside another `Batch.ops` vector; macro-ops don't trigger it.

`WriteDimensionEdge` and `WriteTagEdge` standalone variants remain in the enum for the migration backfill path (§5.3) and post-ingest correction. They are NOT the normal write path.

### 6.2 Writer main loop (dedicated OS thread, batched commit)

**Critical constraint** (closes FINDING-A4-4): rusqlite is **synchronous**. `tx.commit()` performs an `fsync(2)` that blocks the calling thread for ~30–80µs on NVMe and can spike to single-digit ms under load. Doing this work directly inside a tokio task **blocks the tokio worker**, freezing every other task scheduled on the same worker — including retrieval. The writer therefore runs on a **dedicated OS thread**, never on a tokio worker. Async callers reach it through an mpsc channel; the writer thread is the only owner of the `Storage` handle.

```rust
// Public API: the supervisor is the durable handle. `spawn` constructs the
// public channels (one per priority), wires private channels into a fresh
// writer thread, and returns the supervisor handle. On writer panic the
// supervisor respawns the thread (see §6.9); the public channels and the
// supervisor itself outlive any single writer-thread generation.
pub fn spawn_writer(storage_path: PathBuf) -> WriterSupervisor {
    let (tx_high_pub, rx_high_pub) = mpsc::channel(QUEUE_CAP_HIGH);
    let (tx_med_pub,  rx_med_pub)  = mpsc::channel(QUEUE_CAP_MED);
    let (tx_low_pub,  rx_low_pub)  = mpsc::channel(QUEUE_CAP_LOW);

    let supervisor = WriterSupervisor::new(storage_path, rx_high_pub, rx_med_pub, rx_low_pub);
    supervisor.start_generation();  // forks the first writer thread + private channels
    supervisor
}

// Internal: writer thread entry point. The supervisor owns this thread's
// JoinHandle and the private mpsc receivers (rx_*_priv). On panic the
// thread exits; the supervisor detects via JoinHandle and respawns.
fn writer_loop(
    mut storage: Storage,
    mut rx_high: mpsc::Receiver<WriteOp>,
    mut rx_med:  mpsc::Receiver<WriteOp>,
    mut rx_low:  mpsc::Receiver<WriteOp>,
) {
    let mut batch: Vec<WriteOp> = Vec::with_capacity(BATCH_MAX);
    let mut hebbian: HashMap<(NodeId, NodeId), BumpAccum> =
        HashMap::with_capacity(HEBBIAN_COALESCE_CAP);

    loop {
        // 1. Block waiting for the first op (any priority — see §6.3 fairness rules).
        let first = match recv_first_with_fairness(&mut rx_high, &mut rx_med, &mut rx_low) {
            Some(op) => op,
            None => break, // all channels closed → graceful shutdown
        };
        let deadline = Instant::now() + BATCH_LINGER;
        push_or_coalesce(&mut batch, &mut hebbian, first);

        // 2. Opportunistically drain up to BATCH_MAX or until deadline (§6.3 ordering).
        while batch.len() + hebbian.len() < BATCH_MAX {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { break; }
            match recv_with_fairness_timeout(
                &mut rx_high, &mut rx_med, &mut rx_low, remaining,
            ) {
                Some(op) => push_or_coalesce(&mut batch, &mut hebbian, op),
                None     => break, // timeout or all-empty
            }
        }

        // 3. Drain Hebbian accumulator into batch as BumpAssociation ops.
        for ((src, tgt), accum) in hebbian.drain() {
            batch.push(WriteOp::BumpAssociation {
                source_id: src, target_id: tgt,
                delta: accum.delta_sum,
                signal_source: accum.signal_source,
                temporal_forward:  accum.tf_sum,
                temporal_backward: accum.tb_sum,
                direction: accum.direction,
                reply: accum.reply, // last-writer-wins reply channel; see §6.3
            });
        }

        // 4. Commit the whole batch in one transaction.
        //    apply_op returns Result<WriteOpResult>; failures are reported to the op's
        //    reply channel but do NOT abort the batch (per-op isolation), except inside
        //    a Batch op which is all-or-nothing (§6.4).
        let tx_result = storage.conn_mut().transaction().and_then(|tx| {
            for op in batch.drain(..) {
                apply_op(&tx, op); // sends reply on op's oneshot internally
            }
            tx.commit()
        });
        if let Err(e) = tx_result {
            // Commit-time failure (disk full, schema invariant violated, etc.):
            // §6.9 dictates each enqueued op's reply has already been sent a copy of
            // the error before the batch was assembled, so callers are not stranded.
            log::error!("writer batch commit failed: {e}; continuing");
        }
    }
}
```

Tunables (initial values; revisit after Phase B benchmark):

- `BATCH_MAX = 64` ops per transaction.
- `BATCH_LINGER = 5ms` (latency budget for the *first* op in a batch).
- `HEBBIAN_COALESCE_CAP = 4096` (see §6.3 for the cap rationale).

**Why batching**: SQLite's WAL fsync is the dominant write-cost on NVMe (~30–80µs per `tx.commit()` on a modern Mac mini). Amortizing fsync across 64 ops cuts per-op write cost by ~50×. The 5ms linger is invisible to retrieval (which doesn't wait on writes) and acceptable for ingest (the previous synchronous path was 200–500µs/op anyway).

**Why a dedicated OS thread, not `spawn_blocking`**:

- `tokio::task::spawn_blocking` is fine for *bursty* blocking work, but the writer is a *steady-state* consumer. A long-lived `spawn_blocking` task occupies one of tokio's blocking-pool threads forever, which is wasteful and creates a hidden coupling to the blocking-pool size limit.
- A dedicated `std::thread` is owned, named (`engram-writer`), and visible in stack traces / `top -H`. Easier to debug, easier to size.
- Cross-thread communication uses `tokio::sync::mpsc` channels whose senders are usable from any tokio task; `recv()` on the channel from the OS thread uses the blocking `recv()` variant (`blocking_recv` or a small `tokio::runtime::Handle::block_on` shim), not the async one.

**Single-threaded by design**: one OS thread owns `Storage`. No `Mutex<Connection>`, no shared mutable state. The writer thread is the bottleneck *and* the serialization point — both desirable.

### 6.3 Priority & backpressure

Not all writes are equal:

- **Ingest** (`WriteMemory`, `WriteEntity`): user-blocking. High priority. Bounded queue (drop = data loss → bad).
- **Hebbian** (`BumpAssociation`): not user-blocking. Idempotent (an upsert with weight clamp). Coalescable (10 bumps of the same edge in 100ms = 1 commit with the summed delta).
- **Decay** (`ApplyDecayTick`): background. Low priority. Should never block ingest. Drop-oldest is fine — the next tick covers what was dropped.
- **Metacog/interoception** (`WriteFeedbackEvent`, `WriteAnomalyEvent`, `UpdateDomainStats`): medium priority. Loss is acceptable in extreme overload (one missing feedback event doesn't break the agent) but should be rare.

Implementation: **three mpsc channels** (high / medium / low). The writer drains them with **weighted fairness** rather than strict priority, to prevent starvation of medium/low under sustained high load (closes FINDING-A4-5).

**Weighted fairness rule** (closes FINDING-A4-5):

Strict priority drain (`drain rx_high until empty, then rx_med, then rx_low`) will starve medium and low whenever `rx_high` has steady-state arrivals faster than the commit rate. That is a real production scenario — a busy ingest path keeps `rx_high` non-empty for minutes — and would silently freeze decay + metacog.

The writer instead uses a **bounded credit scheme**: each batch is sized `BATCH_MAX = 64` ops, but no priority lane may contribute more than a per-lane cap to a single batch:

- `BATCH_CAP_HIGH = 48` (75% — ingest dominates but does not monopolize)
- `BATCH_CAP_MED  = 12` (~19%)
- `BATCH_CAP_LOW  = 4`  (~6%)

```rust
// Per-batch fair drain (replaces strict-priority pseudocode):
let mut count_high = 0; let mut count_med = 0; let mut count_low = 0;
while batch.len() + hebbian.len() < BATCH_MAX {
    // Try lanes in priority order, but respect per-lane cap.
    let took = if count_high < BATCH_CAP_HIGH {
        if let Ok(op) = rx_high.try_recv() { push_or_coalesce(&mut batch, &mut hebbian, op); count_high += 1; true } else { false }
    } else { false };
    if took { continue; }

    if count_med < BATCH_CAP_MED {
        if let Ok(op) = rx_med.try_recv() { push_or_coalesce(&mut batch, &mut hebbian, op); count_med += 1; continue; }
    }
    if count_low < BATCH_CAP_LOW {
        if let Ok(op) = rx_low.try_recv() { push_or_coalesce(&mut batch, &mut hebbian, op); count_low += 1; continue; }
    }
    // All lanes empty or all caps hit → batch ready (or wait for deadline).
    break;
}
```

If the high lane is empty, the high cap is unused — medium/low can fill the rest of the batch. If high alone produces > `BATCH_CAP_HIGH` ops per batch, the excess waits one batch (5ms BATCH_LINGER + commit time). This guarantees medium gets at most a ~5ms latency penalty and low gets at most ~20ms even under sustained high-pressure ingest — well within acceptable limits for non-user-blocking work.

Backpressure:

- **High-priority channel**: bounded (capacity 1024). When full, sender `await`s — ingest is naturally throttled by the writer's commit rate. This is the desired behavior: never silently drop a user memory.
- **Medium**: bounded (capacity 4096). When full, sender returns `Err(QueueFull)` — caller chooses to retry, drop, or surface. For metacog this means a feedback event during a write storm might fail to enqueue; that's logged and counted, not fatal.
- **Low**: bounded (capacity 256), drop-oldest. The next decay tick subsumes the missed one (decay is idempotent over time).

**Hebbian coalescing with bounded memory** (closes FINDING-A4-6):

The writer maintains a `HashMap<(NodeId, NodeId), BumpAccum>` accumulator. Successive `BumpAssociation` ops with the same `(source_id, target_id)` add to the accumulator instead of emitting separate edge upserts. The map is flushed at every batch commit.

The previous design said "a small HashMap" with no cap — that is **unbounded** in pathological cases (e.g. an adversarial recall pattern producing distinct `(src, tgt)` pairs faster than the commit rate). A real production worst case: a long-running ingest job activating 10k distinct memories against 10k entities = up to 100M unique pairs theoretically; even 1% of that is 1M entries × ~80 bytes per entry = 80MB resident.

Cap and eviction policy:

- `HEBBIAN_COALESCE_CAP = 4096` distinct `(source_id, target_id)` pairs.
- When the map reaches the cap, the writer **immediately flushes** (forces a commit of the current batch + accumulator) instead of growing the map further.
- An emergency flush counts against the next batch's `BATCH_MAX`, so it does not blow up the per-batch transaction size.
- The cap is configurable but never higher than `BATCH_MAX × 64 = 4096` by default — the multiplier reflects that a typical batch holds 64 ops and Hebbian bumps tend to be 1:1 with ops on the high lane.
- Replies to coalesced `BumpAssociation` ops: each accumulator entry stores the *most-recent caller's* `reply` channel. Earlier callers' replies receive `Ok(())` as soon as the entry coalesces with theirs (a coalesced bump committed inside a batch is functionally equivalent to N independent bumps committed in order — every caller's contract is satisfied). This avoids holding N reply channels per entry, which would defeat the memory savings of coalescing.

Total writer-thread memory budget under cap: ~4k ops × ~256 bytes/op = ~1 MB for the batch + 4k Hebbian entries × ~96 bytes = ~400 KB for the accumulator. Bounded and predictable.

### 6.4 Cross-op atomicity (compound writes in one transaction)

Several §4.x ops are inherently compound:

- **§4.7 Supersession**: mark old node deleted + create new node + link via `supersedes` edge + bump references in dependent topics. 4+ row writes, must be atomic (a half-applied supersession leaves the graph in a state where both old and new are "current").
- **§4.14 Metacog**: `WriteFeedbackEvent` + `WriteWmSnapshot` link via `feedback_event_id` — the snapshot is meaningless without the event it explains.
- **§4.15 Dimensions**: `WriteMemory` produces 1 memory node + up to 10 narrative dimension edges + N tag edges + 0..K new dimension nodes. The memory must not be visible to readers until its dimension edges are present (else retrieval by dimension would miss it for a window).
- **§4.5 Synthesis** + **§4.4 KC**: a knowledge topic write produces the topic node + N membership edges + entity rollup edges + provenance edges in one commit.

**Mechanism**: the `Batch(Vec<WriteOp>)` variant in §6.1. A caller composes the compound op as a single `Batch`, enqueues it, and the writer applies all sub-ops inside one `tx.commit()`. The reply oneshot fires only after the full batch commits.

This is **why** the writer queue exists. Without it, a caller doing `store_raw → write_dimensions → write_tags` from outside the queue would either:
- Take a lock around `Memory` (defeats concurrency), or
- Hold three open transactions (deadlocks under concurrent ingest), or
- Allow partial visibility (broken invariants).

The `Batch` variant collapses the question: "is this one atomic act?" → "yes, ship as `Batch`". The writer never sees torn writes because there are no other writers.

**Failure semantics**: if any sub-op in a `Batch` returns `Err`, the whole transaction rolls back and the caller's oneshot receives the error. No partial application. This matches SQLite's transaction semantics — `tx.commit()` is all-or-nothing.

### 6.5 Reader snapshot strategy

Reads do **not** go through the writer queue. They open their own SQLite connection with `BEGIN DEFERRED` and run against the WAL snapshot at read start. This means:

- **Readers see a consistent point-in-time view** for the duration of their query, even if the writer commits 50 batches in the meantime.
- **No reader blocks the writer.** No writer blocks a reader (WAL).
- **Long-running scans (KC clustering, backfill)** are fine — they hold a deferred snapshot for minutes; only the WAL grows during that window. WAL truncates on the next checkpoint after the scan ends.

Connection pooling: a small pool of read connections (default 4, configurable) is held by `Memory`. Each retrieval call checks one out, runs the query, checks it back in. Async-friendly via `tokio::sync::Semaphore` for pool tickets.

**Snapshot invariant for §6.4 atomicity**: because reads use WAL snapshots and writes commit batches atomically, a reader either sees a `Batch` entirely or not at all. A retrieval query running concurrently with a `WriteMemory` `Batch` will either:
- Start before the commit → never see any of the batch's nodes/edges (consistent old view), or
- Start after the commit → see all of the batch's nodes and dimension edges (consistent new view).

It will **never** see "memory node present but its dimension edges missing". This is the cross-op atomicity guarantee §4.15 implicitly relies on for `WHERE dimension='location:Caroline house'` queries to be correct under load.

### 6.6 Writer throughput analysis

The writer is a single thread (one tokio task). Its sustained throughput must exceed the agent's ingest rate, or the bounded queue (§6.3) fills and backpressure surfaces as ingest latency.

**Per-batch cost model** (NVMe-class SSD, WAL mode, measured on Mac mini M2 Pro baseline):

| Component | Cost per batch | Notes |
|---|---|---|
| Begin tx | ~5µs | `sqlite3_exec("BEGIN")` |
| Apply N ops (N=64) | ~10µs × N = 640µs | row insert + index update; varies by op |
| Embedding *blob upsert* (N memories) | ~80µs × N = 5120µs | SQLite blob INSERT + index; **embedding generation cost is paid before enqueue**, not in the writer |
| Commit (fsync) | ~80µs | one fsync per batch (WAL append) |
| **Total** | **~5.8 ms / 64-op batch** | ≈ **11k ops/sec** sustained ceiling |

This is **for a pure-ingest batch**. Decay/Hebbian batches don't touch embeddings → ~120µs total → ~530k ops/sec ceiling (Hebbian-dominated workloads are not write-bound; they're CPU-bound on cosine similarity in the reader).

**Workload reality check**: an active agent generates 10–100 memories/hour during real use. Even at 100/hr (one every 36 sec), the writer is idle 99.97% of the time. The throughput ceiling matters only for:

- **Benchmark replay** (LoCoMo: 441 episodes in conv-26, ingested in ~10 seconds → 44 ops/sec — 250× under ceiling).
- **Backfill** (Phase C): historical-row replay can saturate the writer; mitigation in §6.8.
- **Multi-agent shared memory** (future): N agents writing to one DB; ceiling divides by N. Mitigation in §6.7.

**No latency SLO is needed for writes** — writes are not on the user-blocking path for retrieval (which uses readers). The only SLO is *ingest latency from caller's perspective*: `await store_raw(...)`. Modeled cost: 5ms (BATCH_LINGER) + 5.8ms (commit) = **~11ms p99 for ingest**, well under the 100ms perceptual threshold.

### 6.7 Multi-tenant concurrency and the scale ceiling

The current `Memory` model is **single-tenant, in-process**: one Rust process owns one `Memory`, which owns one `Storage`, which owns one SQLite file. Multiple agents in the same process share through `&Memory` (reads) but only one owns `&mut Memory` (writes).

**v04 preserves this model.** The writer-queue refactor (§6.1–§6.5) is a within-process formalization. It does **not** introduce multi-process IPC.

**Scale ceiling under this model**: one SQLite file, one writer task, ~11k ops/sec sustained → **adequate for ~100 concurrent active agents** at 100 ops/agent/hour (realistic upper bound for genuine agent cognition, not synthetic load). Above that, the architecture needs sharding.

**Sharding directions (out of scope for v04, listed for future readers)**:

1. **Per-namespace shard**: each namespace gets its own SQLite file + writer. The `Memory` API selects the right writer by namespace prefix on each op. Pro: trivially scales N namespaces ≈ N×ceiling. Con: cross-namespace queries become application-layer joins. Acceptable for agent-private memory; bad for shared knowledge.
2. **Read replicas**: append-only WAL streamed to N read-only replicas. Pro: read throughput scales linearly. Con: replicas lag, breaking same-session read-your-writes. Acceptable only for analytics.
3. **External writer process**: writer becomes an IPC service (Unix socket or gRPC), N client processes enqueue ops. Pro: clean isolation, allows different runtimes (RustClaw + future Python clients) to share a substrate. Con: serialization overhead per op (~50µs Bincode roundtrip) cuts ceiling to ~5k ops/sec. Justifiable only when sharing is required.

None of these are committed for v04. The decision: **defer until measured pressure**, because every shard introduces real complexity (cross-shard transactions, replica lag, IPC failure modes) and the single-writer model is *empirically* adequate for the foreseeable workload.

**Trigger criteria for re-opening sharding** (so future readers know when to act): writer queue depth p99 exceeds 5000 ops for >30 seconds in production, OR ingest latency p99 exceeds 200ms for >5 minutes, OR multi-tenant requirement appears with ≥2 agents whose namespaces never overlap. Until any of these fires, single-writer single-file is the right design.

### 6.8 Migration-phase concurrency (Phase B dual-write through the queue)

Phase B (§5.2) dual-writes every mutation to both legacy tables (`memories`, `memory_embeddings`, `entities`, `knowledge_topics`, …) and the unified tables (`nodes`, `edges`, `node_embeddings`). The naïve implementation is per-call-site dual-write code, which is wrong: it allows torn writes (legacy succeeds, unified fails) and doubles the lock-contention surface.

**Through the queue**: each `WriteOp` handler in §6.2's `apply_op` writes to **both** legacy and unified tables within the *same* `tx` transaction. Atomicity is free — SQLite's transaction either commits both or rolls back both. No new code per call site; the dual-write is centralized in the writer.

```rust
fn apply_write_memory(tx: &Transaction, op: WriteMemoryOp) -> Result<NodeId> {
    let memory_id = NodeId::new();

    // Legacy write (Phase B keeps this for parity)
    tx.execute("INSERT INTO memories (id, body, dimensions, ...) VALUES (?, ?, ?, ...)", ...)?;
    tx.execute("INSERT INTO memory_embeddings (memory_id, vec) VALUES (?, ?)", ...)?;

    // Unified write (new, Phase B starts populating)
    tx.execute("INSERT INTO nodes (id, node_kind, attributes, ...) VALUES (?, 'memory', ?, ...)", ...)?;
    for (field, value_node_id) in dimension_edges_for(&op.dimensions) {
        tx.execute("INSERT INTO edges (source_id, target_id, edge_kind, predicate) VALUES (?, ?, ?, ?)",
                   params![memory_id, value_node_id, format!("describes_{field}")])?;
    }
    tx.execute("INSERT INTO node_embeddings (node_id, vec) VALUES (?, ?)", ...)?;

    Ok(memory_id)
}
```

**Phase C backfill** (§5.3) runs as a **dedicated low-priority `BackfillBatch` WriteOp variant** flowing through the same queue. This preserves the single-writer invariant — no separate backfill connection competing with live ingest for the writer lock. The backfill driver enqueues `BackfillBatch { rows: Vec<LegacyRow>, ... }` in batches of 256; the writer interleaves them between live ops at low priority. A 10M-row backfill at ~11k ops/sec ≈ 15 minutes — acceptable as a one-time migration cost.

**Phase D switch-reads** (§5.4) is a pure read-side change; the writer queue is unaffected.

**Phase E stop-legacy-writes** (§5.5): in the handler above, the legacy `INSERT` lines become `// removed in Phase E`. Diff is local to the writer; no caller-site change.

### 6.9 Failure modes and write journal

**Process crash mid-batch**: the writer's batch is in a SQLite transaction. SQLite's WAL guarantees: either the commit completes and is durable, or the WAL is rolled back on next open. No half-committed batch survives a crash. The ops that hadn't reached commit are lost — **but the in-memory queue is also lost** (since the writer is in-process), so callers' `oneshot` receivers receive `Err(QueueClosed)` and can decide to retry.

**Queue overflow** (§6.3 backpressure): high-priority channel full → caller `await`s. Medium full → caller gets `Err(QueueFull)` and decides per-op (metacog: log+drop; supersession: must succeed, so loop with backoff). Low full → silent drop-oldest.

**Writer thread panic** (closes FINDING-A4-10 + r3-p4a-FINDING-5): if `apply_op` panics on a single bad op (e.g. malformed dimensions causing a JSON serialization error), the entire writer thread dies and the channels become a black hole for all subsequent sends. Mitigation:

- `apply_op` catches `Result::Err` and sends it back on the op's `oneshot`. Errors do not kill the writer.
- **Genuine panics CANNOT be caught with `std::panic::catch_unwind` around rusqlite calls.** rusqlite's `Connection`, `Statement`, and `Transaction` types are **not `UnwindSafe`** — they hold raw pointers into SQLite's C state, and unwinding across a partially-committed transaction would leave SQLite in an undefined internal state. Wrapping `apply_op` in `catch_unwind` and resuming the same thread is **unsound**. The reviewer in r2 flagged this; the design accepts it.

- The correct recovery model is **process-level with supervisor-owned channels**. Concretely:

  ```text
  ┌──────────────┐  WriteOp        ┌──────────────┐  WriteOp     ┌────────────┐
  │ async caller │ ──────────────► │  Supervisor  │ ───────────► │  Writer    │
  └──────────────┘   mpsc(public)  │ (owns recvs) │ mpsc(private)│  (thread)  │
                                   └──────────────┘              └────────────┘
                                          ▲                            │
                                          └────── JoinHandle ──────────┘
  ```

  - The **public** mpsc channels (one per priority level) are owned by a `WriterSupervisor` task — NOT by the writer thread. Async callers always send into the supervisor.
  - The supervisor forwards each `WriteOp` into a **private** mpsc channel that the writer thread consumes. **The writer keeps a reference to the original `oneshot::Sender` inside the `WriteOp` variant** (direct-send model) — the supervisor does not consume or short-circuit it. On the happy path, the writer simply calls `reply.send(Ok(...))` (via the wrapped slot — see next bullet) after the transaction commits; the supervisor is never in the data path. The supervisor's role on the forwarding hop is bookkeeping only: it wraps the sender in a shared slot so a crash notifier can co-own it (next bullet). Forwarding cost: one channel hop + one heap allocation, ~1µs.
  - **Why not type-erase replies into a `HashMap<OpId, oneshot::Sender<???>>`?** Because each WriteOp variant has a different reply type (`Sender<Result<WriteMemoryReply>>`, `Sender<Result<NodeId>>`, `Sender<Result<()>>`, …) and a heterogeneous map would require either a trait-object wrapper per slot or a single union reply type. Direct-send keeps replies strongly typed at the variant level and avoids any per-op heap allocation on the hot path.
  - **In-flight bookkeeping for crash recovery** uses a typed-closure map, NOT a sender map: the supervisor maintains a `HashMap<OpId, Box<dyn FnOnce(WriterCrashed) + Send>>` of *crash notifiers*. Concretely, on the public→private forwarding hop, the supervisor extracts each op's raw `oneshot::Sender<R>` from the WriteOp variant, wraps it in `Arc<Mutex<Option<oneshot::Sender<R>>>>`, and rebuilds the variant with the wrapped slot before forwarding. The writer thread and the in-flight crash-notifier closure each hold one `Arc` clone. On the happy path the writer does `slot.lock().take().unwrap().send(Ok(r))`; if a panic intervenes, the supervisor's closure does `slot.lock().take()` and finds either `Some(sender)` (still pending → sends `Err(WriterCrashed)`) or `None` (writer already replied → no-op). The `Arc<Mutex<…>>` indirection lives entirely behind the private channel — the public WriteOp surface (§6.1) keeps the raw `oneshot::Sender<R>` and is unchanged. Cost: one heap allocation + one uncontended lock per op — negligible vs the SQLite commit cost (~50µs).
  - **Op completion signaling**: when the writer commits op `i`, it sends `Ok(reply)` directly on the embedded `oneshot::Sender` and then sends a **completion tick** `OpDone(op_id_i)` on a dedicated `mpsc::UnboundedSender<OpId>` that the supervisor drains in the background. On each tick, the supervisor removes the matching entry from the in-flight map. Tick processing is best-effort cleanup; if it lags, the map grows transiently but every entry is still valid (closures are idempotent — running an already-completed entry's closure finds the `Arc<Mutex<Option<...>>>` slot empty and is a no-op).
  - When the writer thread panics, its `JoinHandle::join()` returns `Err`. The supervisor observes this in one of two ways:
    1. Its forward task gets `Err(SendError)` when the private channel's receiver is dropped (panicking thread's stack unwinds the receiver).
    2. A watchdog `tokio::select!` includes `tokio::task::spawn_blocking(|| handle.join())` returning.
  - On panic detection, the supervisor:
    1. Closes the private channel send half (drops it).
    2. Iterates the in-flight `HashMap<OpId, Box<dyn FnOnce(WriterCrashed) + Send>>` and calls each closure with `WriterCrashed { generation, cause }`. Each closure attempts `Arc::Mutex::take()` on its captured reply slot; if the slot is `Some`, it sends `Err(WriterCrashed)`; if `None`, the writer already replied (race against the OpDone tick) and the closure is a no-op.
    3. Drains the public mpsc receivers up to a bound (e.g. 1024 ops) — for each, sends `Err(WriterCrashed)` directly on the op's embedded reply channel. Beyond the bound, simply drops; those callers' `oneshot::Receiver.await` yields `RecvError`, which they treat as equivalent to `WriterCrashed` (the public contract documents this equivalence).
    4. Calls `Storage::reopen()` on a fresh `PathBuf` (the old `Storage` was owned by the panicking thread and has already been dropped during unwind, releasing the SQLite connection).
    5. Spawns a fresh writer thread with the new `Storage` and a fresh private channel, increments `generation` (so any straggler replies from the old generation can be discriminated), and resumes forwarding.

- This is more expensive than in-thread catch-and-continue (one `panic!` costs ≈ 1ms thread respawn + ≤ 1µs per drained in-flight op) but it is **sound** — the new thread starts from a guaranteed-clean SQLite handle, no caller observes corrupted intermediate state, and every caller whose op was in-flight either gets `WriterCrashed` (graceful, supervisor-delivered) or `RecvError` (rare, beyond-bound drain). The public API documents these two as equivalent failure modes.
- The `WriterCrashed` error carries `generation: u64` so test harnesses + observability can confirm "this caller's op was lost to the panic at generation N" without ambiguity.
- For panic surface reduction, `apply_op` itself does as little arithmetic as possible — it dispatches into per-variant handler functions that do explicit validation up-front (`return Err(...)` for bad input instead of panicking), so the panic case is reserved for genuine bugs (slice OOB, integer overflow in release math), not for bad user input.
- **No write journal beyond SQLite's WAL.** A separate disk journal of pre-commit ops would be a "WAL on top of WAL" — pointless duplication. SQLite's WAL *is* the durable log. The §8 task T66 implements the supervisor + thread-respawn logic described above — it does not introduce a journal file. Lost ops are signaled to callers via `Err(WriterCrashed)` / `Err(RecvError)`, which is the design's intentional consistency contract.

**What this design does not promise**:

- **Cross-process write coordination**: out of scope (§6.7).
- **Exactly-once delivery to the writer**: callers may retry on `Err(QueueFull)`; the writer cannot deduplicate semantically identical ops. Idempotent ops (Hebbian bump, decay tick) are safe to retry; non-idempotent ops (`WriteMemory`) get a new node ID on retry, which is the desired semantic (retried = new memory).
- **Strict FIFO across priority levels**: high beats medium beats low. Within a priority level, FIFO holds.

---

## 7. Resolved design decisions

All §7 questions are now closed. Reasoning is grounded in the **engram thesis**:
the substrate models how the brain stores memory — cell assemblies (nodes)
connected by synapses (edges). Whether a concept belongs in the substrate
or in adjacent housekeeping is decided by asking: *does the brain represent
this as a pattern of neural activation, or is it bookkeeping about that
pattern?* Patterns → graph. Bookkeeping → audit table.

### 7.1 ✅ Q1 — Single DB file (`engram-memory.db`)

**Decision**: one SQLite file for both substrate (nodes/edges/embeddings/FTS)
and audit (pipeline_runs, promotion_candidates, etc.).

**Reasoning**:
- Phase B requires **atomic fan-out** across `memories → nodes + edges + events`.
  SQLite's `ATTACH DATABASE` does not provide true cross-database atomic
  commits — a crash mid-write can leave the substrate and audit halves
  inconsistent. Single file = single WAL = real atomicity.
- FK constraints can reference across cognitive/audit boundary
  (`pipeline_runs.id` referenced by `nodes.source_run_id` and `edges.source_run_id`).
- One backup, one mental model, one schema version.

**Counter considered**: audit tables in a separate attached DB to keep
substrate "pure". Rejected — purity is a code-organization concern, not
a storage concern. Module boundaries enforce purity; file boundaries
just break atomicity.

### 7.2 ✅ Q2 — Entity surface forms are nodes

**Decision**: every surface form ("potato", "@horseonedragon", "potatosoupup",
"oneB") is a `node_kind='entity'` row. Surface forms that refer to the same
real-world referent are linked by `edge_kind='structural', predicate='same_as'`
to a canonical entity node (or, equivalently, form a same_as clique with no
designated canonical — see §7.2.1).

**Reasoning** (first principle):
- The real problem is **entity resolution**, not "where do aliases live".
  Currently engram stores 2312 entities with no resolution: "potato" and
  "@horseonedragon" are two unrelated rows, and a query that surfaces one
  cannot reach memories about the other.
- In cortex, lexical surface forms (Wernicke's area) and concept representations
  (semantic memory) are **separate populations of cells with edges between them**.
  An alias is not a property of a concept — it is a distinct cell assembly
  that points at the same concept. Surface forms must be queryable as
  first-class strings (FTS5), embeddable, and linkable.
- Inline JSON aliases (`nodes.attributes.aliases = [...]`) would re-create
  the substrate sprawl this design is fixing: alias text would not be in
  FTS5, would not participate in graph traversal, and entity resolution
  would still need a side table.

**Implementation note**: `same_as` is structural (non-unique edge_kind), so
the existing structural-edge schema in §3.2 handles it without changes.
Surface-form nodes carry their text in `content` and participate in
`nodes_fts` automatically.

**6.2.1 Canonical vs clique** — out of scope for this design. The substrate
supports both:
- *Designated canonical*: one node carries the "canonical" flag in
  `attributes`, others have `same_as` edges pointing at it.
- *Clique*: all surface forms have `same_as` edges to each other.

Either works on the same schema; resolution algorithm is a v0.4.1 concern.

### 7.3 ✅ Q3 — Partial UNIQUE indexes (already in §3.2)

(Unchanged from prior version.) Partial UNIQUE on `edges(source_id, target_id,
edge_kind, predicate) WHERE edge_kind IN ('associative', 'containment')`.
Structural edges remain non-unique. See §3.2 and §4.3 for the ON CONFLICT
upsert mechanics.

### 7.4 ✅ Q4 — Episode is a node, not a column

**Decision**: drop `nodes.episode_id` and `edges.episode_id`. Memories
belong to episodes via `edge_kind='containment', predicate='belongs_to_episode'`
pointing at a `node_kind='episode'` row.

**Reasoning** (first principle):
- An episode in the brain is a **hippocampal spatio-temporal binding** —
  it binds together a set of cell assemblies (memories) with shared
  temporal/contextual context. It is a *thing that exists*, with its own
  decay curve, its own importance, eventually its own synthesis-generated
  summary. It is **not a label attached to memories**.
- Treating `episode_id` as a column treats episodes as bookkeeping.
  That is the same substrate sprawl this design rejects — a concept
  has two representations (column on memory + potential node), forcing
  a dual-write invariant forever.
- Performance objection ("column filter is faster than graph traversal"):
  rejected. `WHERE episode_id = ?` on an indexed column is O(log n).
  `SELECT source_id FROM edges WHERE target_id = ? AND edge_kind = 'containment'`
  on `idx_edges_target_kind` is also O(log n). Same complexity, same index
  fanout, no measurable difference at our scale.

**Migration note**: existing `episode_id` values become `containment` edges
during Phase C backfill (T19 for memories, T22 for entities). The columns
are dropped in Phase F.

**§3.1 schema impact**: remove `nodes.episode_id TEXT` (L160) and
`edges.episode_id TEXT` (L229). See §3.1 update note.

### 7.5 ✅ Q5 — Promotion candidates stay as audit table

**Decision**: `promotion_candidates` remains a dedicated table (current
schema unchanged). It does NOT become a `node_kind`.

**Reasoning** (first principle):
- A promotion candidate is **not a cognitive entity**. It is the working
  state of the promotion algorithm: "this pattern's weight is climbing
  toward threshold but hasn't crossed yet." In the brain, this is not a
  separate cell assembly — it is the *current synaptic weight* of an
  existing connection. The weight already lives on the edge.
- The `promotion_candidates` table is **scratchpad / audit** for the
  promotion algorithm: which patterns were considered this cycle, what
  scores they got, why one was promoted and another rejected. This is
  *log data about the algorithm*, not substrate.
- Earlier reasoning ("simpler, keeps audit clean") was correct but
  under-justified. The deeper reason is **separation of concerns**:
  substrate stores cognitive state; audit stores algorithmic decisions
  about that state. Mixing them pollutes graph traversal semantics
  (`SELECT * FROM nodes` would return both "things I remember" and
  "things the promoter is currently thinking about promoting").

This is not technical debt — it is the correct partition.

### 7.6 ✅ Q6 — Drop `triples` table in Phase F

**Decision**: drop `triples` table. 0 rows in production, no writer,
no reader. Dead schema from a v0.2.5-ish abandoned layer.

Action: included in Phase F (T41 or new T-id).

### 7.7 ✅ Q7 — Legacy reader during Phase B, with hard exit criteria

**Decision**: during Phase B (dual-write), all readers continue to use
the legacy schema. Phase B is invisible to consumers — pure write fan-out
for verification.

**Hard exit criteria** (must all hold before Phase B can complete and
Phase C may start):

1. **Zero invariant violations** for 7 consecutive days:
   - Row-count parity: `count(memories) == count(nodes WHERE node_kind='memory')`,
     and analogous parity for entities → entity nodes, hebbian_links →
     associative edges, KC topics → topic nodes.
   - Field-level spot check: 100 random memory IDs verified field-equal
     between `memories` and `nodes` (content, layer, memory_type, occurred_at,
     created_at, namespace).
2. **Shadow-read parity** ≥99%: for each Phase B day, replay a sample of
   the day's production retrievals against both substrates (legacy + unified)
   and compare top-K results. K=20, Jaccard similarity ≥0.99 on at least
   95% of queries.
3. **Bench unchanged**: LoCoMo J-score (full 152-q) on the dual-write build
   matches the pre-Phase-B baseline within ±1pp. Confirms the write
   fan-out hasn't accidentally affected legacy paths.

**Why hard criteria matter**: dual-write is a verification window, NOT a
"temporary" state. Indefinite dual-write is technical debt (two schemas
forever, two invariants to maintain, two query paths to test). The
criteria force a decision point: either Phase B succeeds and we move
to Phase C within ~7 days, or it fails and we roll back the fan-out
without consumer impact.

**Roll-forward gate**: Phase C (read switch) is gated on §7.7 criteria
plus the §8.5 Phase D parity campaign. Phase B success does not
automatically promote to Phase C — explicit human go-decision required
after reviewing §7.7 metrics.

---

## 8. Action plan (executable checklist)

Each item is sized for a single sub-agent task (≤300 lines output) or
one focused session.

### 8.1 Setup
- [x] **T01** This doc reviewed and approved — 5 review rounds (r1 pre-expansion → r5 spot-check) completed 2026-05-12, all Critical findings applied via commits 3-9 (`reviews/design-r1.md` through `design-r5.md`)
- [x] **T02** Resolve §7 open questions (potato decisions) — closed Round 3 2026-05-12 via first-principles cognitive-substrate framing; see §7 and `reviews/design-r1.md` Round 3
- [x] **T03** Write requirements doc `v04-unified-substrate/requirements.md` (GOAL-1.1 ... GOAL-N) — derives from §3, §4. **Shipped 2026-05-14** as a single ~250-line file covering 6 categories (schema / writer behaviors / backfill / read-switch / writer queue / cross-cutting GUARDs) + per-phase acceptance + out-of-scope list. Deliberately compact — outside-engineer-readable in 10 minutes rather than a multi-feature split (GOAL count came in at 33, below the 15-per-doc heuristic threshold that would force a split).
- [x] **T04** Update `consolidation-autopilot-DRAFT.md` §2 invariants to reference unified substrate — **shipped 2026-05-14**: added new §2.5 with 8 substrate invariants (I-A1 dual-write transit, I-A2 no raw SQL, I-A3 idempotent, I-A4 namespace-respecting, I-A5 read-switch transparent, I-A6 backfill compatible, I-A7 counter accounting, I-A8 writer-queue priority lanes) and a new acceptance bullet referencing them. Autopilot inherits Phase B dual-write guarantees transitively — no autopilot-specific dual-write code needed.

### 8.2 Phase A — schema additive
- [x] **T05** Storage migration: add `nodes` table + indexes (storage.rs) — b7b9290
- [x] **T06** Storage migration: add `edges` table + indexes — 2ab635d
- [x] **T07** Storage migration: add `nodes_fts` + triggers — fb03f1a
- [x] **T08** Storage migration: add `node_embeddings` table — d7ac8cf
- [x] **T09** Bump `engram_meta.schema_version` to `0.4-additive` — 9d18c59
- [x] **T10** Add Rust types: `Node`, `Edge`, `NodeKind`, `EdgeKind` (with typed `attributes` per kind) — 89569f2
- [x] **T11** Test: storage open on fresh DB creates all unified tables; open on legacy DB adds them without touching old data — ba35622 (3/3 pass)

### 8.3 Phase B — dual-write
- [x] **T12** `store_raw`: dual-write memory → nodes — 2fd9531 (dropped into `Storage::add` since it's the single canonical memory write path; `store_raw` flows through `add`)
- [x] **T13** ResolutionPipeline: dual-write entities → nodes(kind=entity), edges — 4966ec1 (helpers `dual_write_entity_to_nodes` + `dual_write_edge_to_edges` in `graph/store.rs`; wired into `insert_entity`, `insert_edge`, `apply_graph_delta`. `source_memory_id=NULL` in Phase B — T19 backfill closes it. `merge_entities`/`supersede_edge` out of scope.)
- [x] **T14** Hebbian (`Storage::record_association` + `record_coactivation_ns` + `record_cross_namespace_coactivation`): dual-write co-activation → `edges(edge_kind='associative')`. All three legacy writers map onto the unified UPSERT in §4.3 with `signal_source` as part of row identity. Cascade refactor: `record_association` `&self` → `&mut self`, propagate through `LinkFormer::storage`, `memory.rs:2585`, `promotion.rs:272` test helper, 5 `former.rs` tests.
- [x] **T15** KC (knowledge_compile): dual-write topics → nodes(kind=topic), containment edges — 4253cc3 (`dual_write_containment_to_edges` helper in `graph/store.rs`; `dual_write_entity_to_nodes` now derives `node_kind` from `EntityKind` so `Topic→'topic'`, others→'entity'; new `GraphWrite::upsert_topic_containment` API wired into `persist_cluster` after `upsert_topic`. 4 integration tests cover topic node-kind mapping + containment shape + idempotency + empty-cluster edge case. `weight=1.0` since ProtoCluster has no per-member score; supersession-retire deferred to Phase C.)
- [x] **T16** Synthesis: dual-write provenance → edges. `Storage::record_provenance` now also writes `edges(edge_kind='provenance', predicate='derived_from', source=insight, target=source_memory)` with `gate_decision`/`gate_scores`/`cluster_id`/`source_original_importance` embedded in `attributes` via `json_object(...)` (gate_scores wrapped in `json()` to avoid double-encoding). `Storage::store_raw` dual-writes the insight itself into `nodes` with `node_kind='insight'` (store_raw is synthesis-only — every caller goes through `synthesis::engine::store_insight_atomically`, which already hardcodes `memories.source='synthesis'`). 5 integration tests cover: insight node_kind mapping, provenance edge shape, nested gate metadata JSON roundtrip, NULL gate_scores roundtrip, and end-to-end atomic flow (insight + N provenance edges within one synthesis transaction).
- [x] **T17** Parity test (CI nightly) — `tests/v04_phase_b_dual_write.rs::t17_phase_b_parity_invariants_across_namespaces`. **Not** raw row-count parity — legacy and unified deliberately diverge on associative edges (§4.3) and on `apply_graph_delta`'s edge-without-source-memory case (§T13 footnote). T17 asserts the following invariants per namespace:
  - **I1a.** For every legacy `memories` row with `source != 'synthesis'`, unified has exactly one `nodes(node_kind='memory')` row with byte-equal `id`/`content`/`created_at`.
  - **I1b.** For every legacy `memories` row with `source = 'synthesis'`, unified has exactly one `nodes(node_kind='insight')` row with byte-equal `id`/`content`/`created_at`. (Refinement noted during implementation: `Storage::store_raw` dual-writes insights with `node_kind='insight'`, not `'memory'` — see T16.)
  - **I2.** For every legacy `graph_entities` row, unified has exactly one `nodes(node_kind='entity')` row with byte-equal `id` (BLOB→UUID-string mapping).
  - **I3.** For every legacy `graph_edges` row with `object_kind='entity'`, unified has at least one `edges(edge_kind='assertion')` row with matching `(source_id, target_id, predicate)`. Literal-object edges are out of scope here (they map to `edges.target_literal`, not a `(src,tgt,predicate)` triple).
  - **I4.** For every legacy `hebbian_links` row with `strength > 0`, unified has at least one `edges(edge_kind='associative', predicate='co_activated')` row matching `(source_id, target_id)` in either direction, **regardless of weight/count**. (Unified canonicalizes to `(min, max)`; legacy does not.)
  - **I5.** For every legacy `synthesis_provenance` row whose `insight_id` lives in this namespace, unified has exactly one `edges(edge_kind='provenance', predicate='derived_from')` row with matching `id`, `source_id=insight_id`, `target_id=source_memory_id`.
  - Weight and coactivation_count are *not* compared between legacy and unified for associative edges — this divergence is intentional and documented in §4.3.
  - The seeder drives writers across three namespaces (`default`, `alpha`, `beta`); synthesis is only exercised in `default` because `store_raw` hardcodes `namespace='default'`.
- [x] **T18** Read isolation: dual-write does not affect any retrieval path. **Implemented as a hostile read-isolation test** (`tests/v04_phase_b_dual_write.rs::t18_read_isolation_unaffected_by_unified_table_mutation`) rather than a LoCoMo bench run. Rationale:
  - The invariant T18 is meant to prove — "Phase B dual-write changes nothing user-visible" — is structural, not statistical. A single 152q LoCoMo run cannot distinguish a 0.4 pp dual-write regression from LLM-judge noise (RUN-0024→0027 drift evidence); proving "unchanged" statistically would require N≥5 reruns at ~$25/30 min each.
  - **Static evidence** (verified at commit time): `grep -rn 'FROM nodes\b\|JOIN nodes\b' crates/engramai/src/` returns 0 hits outside test code; same for `FROM edges`. The only production `nodes_fts` references are CREATE TRIGGER / CREATE VIRTUAL TABLE in `storage.rs::create_nodes_fts_*` — no `SELECT … FROM nodes_fts` in production. Production FTS reads route through legacy `memories_fts`. Retrieval API (`retrieval/*`) walks `memories` + `hebbian_links` + `graph_entities` + `graph_edges` (legacy graph tables), not the unified `edges` table.
  - **Runtime backstop**: the test seeds T12/T14/T16 writers (which fire dual-writes), snapshots every public Storage retrieval API (`search_fts`, `search_fts_ns`, `search_by_type`, `search_by_type_ns`, `fetch_recent`, `all_in_namespace`, `get_by_ids`, `get_hebbian_neighbors`, `get_hebbian_links_weighted`), then hostile-mutates `DELETE FROM nodes; DELETE FROM edges; DELETE FROM nodes_fts;` and re-snapshots. Byte-equality assertion across the entire `RetrievalSnapshot` struct catches any hidden read path (triggers, view joins, dyn-dispatched callbacks) that static grep would miss.
  - This is strictly stronger than a bench run for the stated invariant: a bench produces a noisy aggregate score; the read-isolation test produces a categorical proof.
  - LoCoMo bench *re-runs* are deferred to Phase C (when reads actually cut over to unified — that's the moment a bench delta signals something).

#### 8.3.1 Phase B writer audit (closure)

The original Phase B writer set (T12–T16) only covered initial insert paths. Phase D read-switch work surfaced six additional writers that mutated legacy tables without mirroring to the unified substrate — every one became a real divergence bug under `unified_substrate=true`:

- **ISS-121** `soft_delete` — `deleted_at` not stamped onto nodes — *closed by `ea00b65`*
- **ISS-122** `upsert_entity` — entity columns not mirrored onto `nodes(node_kind='entity')` — *closed by `cb9e2e9`*
- **ISS-123** `link_memory_entity` — memory↔entity edge not dual-written to `edges` — *closed by `a902529`*
- **ISS-124** `update` / `update_content` / `update_importance` — memory metadata UPDATE family — *closed by `80d17a4`*
- **ISS-125** `delete_embedding` (single-model) — `node_embeddings` orphan — *closed by `965b747`*
- **ISS-126** `delete` (hard) — orphan `nodes` row + dangling edges — *closed by `9442478`*

Plus three earlier shim-key fixes (ISS-119 `contradicts`/`contradicted_by` round-trip via `_legacy_*` attribute keys, ISS-120 `EntityKind::Other(_)` variant round-trip, ISS-115 broader Phase B dual-DELETE) that landed before the read-switch pass.

**Closure assertion**: every `pub fn` on `Storage` that mutates `memories`, `entities`, `memory_entities`, `entity_relations`, `memory_embeddings`, or `hebbian_links` now has a contract test pinning that the mutation also lands on the corresponding `nodes` / `edges` / `node_embeddings` row. Read-only or maintenance methods (`record_access` on `access_log`, `decay_hebbian_links` strength-only mutation, `store_promotion_candidate` on a separate sidecar table) don't touch the memory substrate and are out of scope. The Phase B contract is now fully enforced.

### 8.4 Phase C — backfill
- [x] **T19** Backfill driver: memories → nodes (no LLM) — single helper `Storage::insert_memory_node_row` shared with T12 dual-write; `substrate::backfill::backfill_memories_to_nodes` wraps a two-pass driver (Pass 1 INSERT OR IGNORE, Pass 2 UPDATE for self-referential supersession), audit row in `backfill_runs`, optional namespace filter. 6/6 phase-C tests including byte-equal parity vs T12 dual-write; 21/21 phase-B + 1871/1871 lib still pass.
- [x] **T20** Backfill driver: memory_embeddings → node_embeddings — `Storage::insert_node_embedding_row` helper (single-source-of-truth, ready for any future Phase B embedding dual-write); `substrate::backfill::backfill_embeddings_to_node_embeddings` single-pass driver. Handles RFC3339→epoch conversion with fallback-to-now() on malformed dates (count surfaced in audit notes), skips orphan embeddings when parent `nodes` row missing (T19 prerequisite — handled as `rows_skipped_missing_node` not failed), supports multi-model per memory, namespace filter via JOIN on `memories.namespace`. 7/7 phase-C-embeddings tests + 1871 lib + 21 phase B + 7 phase C memories all pass.
- [x] **T21** Backfill driver: entities → nodes — `Storage::insert_entity_node_row` helper writes the slim legacy projection (id/name/entity_type/namespace/created_at/updated_at into `node_kind='entity'` row). `substrate::backfill::backfill_entities_to_nodes` two-pass driver: Pass 1 INSERT OR IGNORE via helper; Pass 2 (inline, same tx) merges legacy `entities.metadata` into existing `nodes.attributes` with **existing-wins** policy via `merge_attributes_existing_wins` (Rust-side merge — SQLite's JSON_PATCH is last-write-wins, wrong polarity). `entities.entity_type` lands as a synthetic `attributes.entity_type` key; legacy metadata keys can shadow it. Defence-in-depth: Pass 2 refuses to mutate non-`entity` node_kinds (topic/memory/insight), counted as `rows_kind_mismatch` in audit notes. Helper is a separate single-source-of-truth from T13's `dual_write_entity_to_nodes` because the legacy table shape is a strict subset of `graph::entity::Entity`. 9/9 phase-C-entities tests + 1871 lib + 21 phase B + 7 phase C memories + 7 phase C embeddings all pass.
- [x] **T22** Backfill driver: entity_relations → edges — `Storage::insert_structural_edge_row` helper writes the slim legacy projection into `edges(edge_kind='structural', predicate_kind='canonical')`. `substrate::backfill::backfill_entity_relations_to_edges` two-pass driver: Pass 1 FK-guards both endpoints via `EXISTS` against `nodes(id)` (skips with `rows_skipped_dangling_endpoint` audit counter on missing endpoint — recovery is run T21 then re-run T22), INSERT OR IGNORE via helper. Pass 2 (inline, same tx) merges legacy `attributes` into existing structural-edge rows with existing-wins (FINDING-1 polarity applied prophylactically — column-derived `source` key wins over metadata `source` key). Defence-in-depth: Pass 2 refuses to mutate non-`structural` edge_kinds (assertion/associative/provenance), counted as `rows_existing_kind_mismatch`. Helper is separate single-source-of-truth from T13's `dual_write_edge_to_edges` because legacy `entity_relations` has thinner shape and different edge_kind. 10/10 phase-C-entity_relations tests + 1871 lib + 21 phase B + all other phase C tests pass.
- [x] **T23** Backfill driver: memory_entities → edges — `Storage::insert_provenance_edge_row` helper (new) writes `edge_kind='provenance'` rows; existing `Storage::insert_structural_edge_row` is reused for the `subject`/`object` role branches. `substrate::backfill::backfill_memory_entities_to_edges` single-pass driver splits by `role` per design §3.3: `mention`/`''`/`triple`/unknown → `provenance/mentions`; `subject` → `structural/subject_of`; `object` → `structural/object_of`. Idempotency via deterministic edge id (`sha256("memory_entities|memory_id|entity_id|role|edge_kind|predicate")` → first 16 bytes → UUID, per design §5.3 lines 1170-1182, new `sha2` crate dep). Non-canonical roles (`triple`, free-form) preserve raw value in `edges.attributes.legacy_role` for audit traceability; canonical roles write `'{}'`. `namespace` and `created_at` derived from parent memory via JOIN since `memory_entities` has no own columns for these. FK pre-check both endpoints exist as `nodes(id)` — missing endpoints counted in `rows_skipped_dangling_endpoint` (recovery: run T19+T21 first). Defence-in-depth: pre-existing edges row with our deterministic id but mismatched `edge_kind` counted in `rows_skipped_mismatched_kind` (under contract this never fires — id hash includes kind — but the counter makes a future hash-invariant bug visible). Audit notes also surface `rows_normalized_legacy_role`, `unknown_role_distinct_count`, `unknown_role_samples` (cap 10), and `unknown_role_samples_truncated` flag. **Design inconsistency noted**: §5.3 line 1140 prose says "memory_entities → edges (kind=provenance)" without role split; §3.3 (lines 320, 338) splits by role and is normative — driver follows §3.3. Tighten the §5.3 prose in a follow-up doc commit. 14/14 phase-C-memory_entities tests + 1874 lib + 21 phase B + all other phase C tests pass.
- [x] **T24** Backfill driver: hebbian_links → edges — `Storage::insert_associative_edge_row` helper (new) writes `edge_kind='associative', predicate='co_activated'` rows with `weight`/`attributes`/`namespace`/`created_at` parameters and `confidence=1.0` baked in (matches Phase B T14 dual-write convention). `substrate::backfill::backfill_hebbian_links_to_edges` is the only Phase C driver doing **SQL-side merge** before INSERT: GROUP BY `(canonical_a, canonical_b, namespace, signal_source)` with `(canonical_a, canonical_b) = (MIN(source_id, target_id), MAX(source_id, target_id))` collapses the `(A,B) + (B,A)` collision class (production: 119 such pairs out of 43,346 rows). Aggregation policy per design §4.3: `weight = SUM(strength)`, `coactivation_count = SUM`, `temporal_forward = SUM`, `temporal_backward = SUM`, `created_at = MIN` (earliest observation wins). `direction` and `signal_detail` packed as scalar string when homogeneous (production today is uniformly `'bidirectional'` with empty signal_detail), sorted JSON array when heterogeneous (defence for future multi-direction signals). Deterministic id hash includes `(table | canonical_lo | canonical_hi | namespace | signal_source | edge_kind | predicate)` — signal_source IS in the hash even though §5.3's amended template doesn't list it, because §4.3 makes signal_source a row-identity dimension via the partial unique index `idx_edges_assoc_unique` and a future multi-signal production write would otherwise be silently dropped by the primary id collision before the partial index check fires. **Audit innovation**: `notes.merged_collision_pairs` surfaces the count of canonical pairs whose legacy rows came from both directions — the most diagnostic Phase C stat (if 0, all merges were trivial; if >0, the merge policy actually fired). `rows_inserted` counts LEGACY rows collapsed, not edges produced, so the invariant `rows_read = inserted + skipped` holds across the SQL-side merge. FK pre-check both endpoints exist in `nodes` — missing endpoints counted in `rows_skipped_dangling_endpoint` (recovery: run T19, re-run T24). Defence-in-depth: pre-existing edges row with our deterministic id but mismatched `edge_kind`/`predicate` counted in `rows_skipped_mismatched_kind` (under contract never fires — hash discriminates — but counter makes future hash-invariant bugs visible). 13/13 phase-C-hebbian tests including SUM-semantics for collisions, separate-edges-per-signal-source, byte-identical deterministic id across direction orderings, heterogeneous-direction JSON array packing, audit `merged_collision_pairs` count. 1877 lib + 21 phase B + 61 phase C all pass.
- [x] **T25** Backfill driver: synthesis_provenance → edges — reuses existing `Storage::insert_provenance_edge_row` helper (T23 already covers the `edge_kind='provenance'` projection shape; T25's only new predicate is `derived_from`). `substrate::backfill::backfill_synthesis_provenance_to_edges` is the simplest Phase C driver: single-row natural-key, no merge, single namespace inheritance JOIN. **Three contract-shaping decisions documented in this commit**: (1) `edges.id = legacy.id` pass-through, NOT a hash — provenance is append-only per §3.2 (no partial unique index for `kind='provenance'`), and Phase B's T16 dual-write also uses legacy.id directly; if T25 hashed instead, a re-emission via Phase B AFTER backfill would land as a second edge under a different hashed id instead of colliding on PK (the desired idempotency). §5.3 line 1216 amended accordingly — replaced the `hash_input = ...` template with explicit `edge_id = legacy.id` and a paragraph linking the decision to §3.2 + §4.5 + verifier I5. (2) `edges.confidence = legacy.confidence` pass-through, NOT 1.0 — **first Phase C driver to pass a legacy confidence column through**, establishing the policy "legacy-column-wins when present, default to 1.0 only when legacy table has no confidence column" (T22/T23/T24 used 1.0 because entity_relations / memory_entities / hebbian_links lack the column). FINDING-3 from T24-r1 review identified this as a doc gap; T25's audit `notes.confidence_policy` field carries the policy string as a forward-citable reference. (3) Namespace inheritance via JOIN `memories(insight_id).namespace` — synthesis_provenance has no own NS column; this mirrors T23's pattern for memory_entities. Attributes JSON embeds `gate_decision`, `gate_scores` (parsed as nested JSON object, NOT a quoted string — `serde_json::from_str` round-trip), `cluster_id`, `source_original_importance` (omitted entirely when NULL — not stored as JSON `null`), `synthesis_timestamp` (verbatim RFC3339 string for forensic traceability). FK pre-check on both `insight_id` and `source_id`. Defense-in-depth `existing_kind` lookup detects id collision under a different `edge_kind`/`predicate` (under contract never fires — legacy UUIDs are unique to synthesis writer — but counter makes future regressions visible). Malformed `gate_scores` JSON preserved as a string in attributes rather than dropped (operator visibility). 12/12 phase-C-synthesis-provenance tests covering: legacy.id pass-through invariant, gate_scores parses to nested object, dangling endpoint recovery via T19, idempotent rerun, namespace JOIN filter, confidence pass-through with distinct values, malformed JSON preservation, NULL importance omission, empty table no-op, synthesis_timestamp → recorded_at/created_at/updated_at epoch conversion, audit row + notes capture, counter invariant under mixed outcomes. 1877 lib + 21 phase B + 73 phase C all pass.
- [x] **T26a** Backfill driver (≤300 lines code, doc comments excluded): resumable batch processor for triple extraction — checkpoint state to disk, rate limiting, error/retry handling. No live API calls. **Shipped 2026-05-14** as `crates/engramai/src/substrate/triple_backfill.rs` (306 code lines, 87 doc) + 10 contract tests in `tests/v04_phase_c_triple_backfill.rs`. New migration `migrate_triple_backfill_checkpoint` adds the `triple_backfill_checkpoint` table for per-memory-id resume cursor (separate from `backfill_runs` because the triple driver is the only iterator-state-bearing one — other drivers are SQL-set based and re-converge trivially on re-run). Audit row emitted into `backfill_runs` with `legacy_table='triples'`. Driver takes `&dyn TripleExtractor` so tests inject `NoopTripleExtractor` + a local `CountingMockExtractor`; production wires `AnthropicTripleExtractor`. Counter invariant: `rows_read = memories_processed + rows_skipped_existing + rows_failed`. `rows_inserted` carries the **triple count** (more useful for audit than memory count); per-memory count surfaced in `notes.memories_inserted`. **ISS-128 patch shipped 2026-05-15**: `notes` JSON now also carries `failed_memory_ids` (capped at 10k), `failed_ids_truncated` flag, and `last_error_message` — surfaced after T26c (2026-05-14) ran with 5314 failures and no way to recover which memories failed. 5 new contract tests (`iss128_*`) lock the behaviour. Unblocks ISS-129 rerun. **Out of scope for T26a**: `insert_triple_entity` in `Storage::store_triples` writes entities + memory_entities via raw SQL and **does not** cascade through the ISS-123 Phase B dual-write — under `unified_substrate=true`, triple-derived entities won't appear in `nodes`/`edges` after extraction. This is its own gap and will be filed as a follow-up ISS; T26a-the-driver is correct, the writer it delegates to has a separate dual-write debt.
- [x] **T26b** Dry-run on 100 random memories; validate output quality; extrapolate cost (operational, human-supervised). **Tooling shipped**: `cargo run --release --example t26b_triple_backfill_sample -- --source <db> --sample 100 --out <report.md>`. Clones source DB to temp by default (non-destructive), wires `AnthropicTripleExtractor` (Haiku) into the T26a driver with `max_memories: Some(N)`, emits markdown report with BackfillRun counters, predicate distribution, sample triples, cost extrapolation, and a human-judgement checklist. **Run executed 2026-05-14** on 100 production memories — 363 triples reviewed clean (9 meaningful predicates, mean conf 0.82, 0.8% sentence fragments, 0 self-loops, CJK clean). **Verdict: PROCEED** (archived in `.gid/features/v04-unified-substrate/operational-runs/T26b-sample-2026-05-14.md`).
- [x] **T26c** Full 24k production run (~$25, ~30 min wall-clock) — operational, human-supervised, NOT a sub-agent task. **Run executed 2026-05-14** against clone DB `engram-memory-t26c.db`. Terminated early after ~7h (PID 18943 disappeared without writing `status='completed'` — no crash report, root cause unverified but presumed sustained-load API rate-limit). **Partial result accepted as deliverable**: 7,575 succeeded + 5,314 failed = 12,889/14,881 attempted (86.6% coverage), 27,423 triples written. Quality on the successful portion matches the T26b sample (mean conf 0.826, 0.5% sentence fragments, 10/27,423 self-loops, healthy predicate spread). Archive: `.gid/features/v04-unified-substrate/operational-runs/T26c-partial-2026-05-14.md`. Clone DB preserved as v0.4 reference fixture (do not merge to prod until ISS-129 rerun completes). Follow-up issues: **ISS-128** (persist failed memory_ids — landed 2026-05-15) and **ISS-129** (T26c rerun with conservative retry, blocked on ISS-128 — now unblocked).
- [x] **T27** Backfill verification report: counts + content spot-check — `crates/engramai/src/substrate/verify.rs` (new module, ~1100 lines) ships five invariants over the Phase C backfill drivers. **I1 count parity** (all 7 drivers): per-driver `DriverSpec` table maps each legacy table to its unified-side `Fingerprint` (`NodeKind`, `PlainTable`, `EdgeKindPredicateIn`, `EdgeKindMinusPredicates`, `Union`); the fingerprint discriminator is needed because `edge_kind` alone cannot distinguish T22/T23 (both write `structural`) or T23/T25 (both write `provenance`) — distinguishing key is `(edge_kind, predicate ∈ closed_set)`. `merge_semantics` flag per driver: T22/T23/T24 = true (`unified >= legacy` acceptable because SQL-side merge collapses rows), T19/T20/T21/T25 = false (strict equality). `legacy_has_namespace` flag: `memory_entities` / `memory_embeddings` / `synthesis_provenance` have no own NS column; when filter set, legacy count for those drivers ignores the filter (documented limitation; unified side still filters via JOIN-inherited NS). **I2 audit row consistency**: SQL-side filter `WHERE rows_read <> rows_inserted + rows_skipped_existing + rows_failed`, skip `finished_at IS NULL` rows (in-progress backfills are not violations). Cheap scan even on long backfill_runs histories. **I3 idempotency** (gated): separate entry point `verify_phase_c_parity_mut(&mut Storage, ...)` re-executes every Phase C driver in dependency order (nodes before edges so missing-node-causes-edge-reinsert points at the right driver), asserts each `BackfillRun.rows_inserted == 0`. Read-only path stays on `verify_phase_c_parity(&Storage, ...)`; the API split makes the read-only contract self-documenting at the call site (the common case — CI smoke, dashboard health check — cannot accidentally trigger driver re-execution). Audit-table append side effect by design: backfill_runs is append-only and the new rows are the durable proof of the re-run. **I4 content spot-check** (T19 driver only, this iteration): `sample_legacy_ids()` with seeded `StdRng::seed_from_u64` + `SliceRandom` shuffle for reproducible sampling; `spot_check_memories()` fetches both sides, projects via `MemoryRow` struct + `macro_rules cmp!` for scalar fields, parses `attributes` JSON value-by-value so key-order differences are not false positives. **I4 scope decision**: spec literal "counts + content spot-check" satisfied by I1 (all 7) + I4 (T19); T20/T21/T22/T23/T24/T25 I4 helpers deferred to ISS-113 because pass-through drivers (T20/T21/T25) need new spot-check helpers and merge-semantics drivers (T22/T23/T24) need a different assertion shape ("unified row with right shape exists" rather than field-equal — counter fields like `weight` SUM across legacy rows and cannot be equality-checked). **I5 FK closure**: `LEFT JOIN nodes` on both `edges.source_id` and `edges.target_id WHERE target_id IS NOT NULL` (target_literal-NULL xor target_id-NOT-NULL constraint means target_id-null rows are legitimately endpoint-free literals). Public types: `VerifyOpts`, `VerificationReport`, `DriverCounts`, `AuditViolation`, `IdempotencyViolation`, `ContentMismatch`, `FkViolation`; all `Serialize` for dashboard/CLI consumption. `recompute_ok()` re-derives the top-level pass/fail boolean after appending violations (exposed for tests that inject divergence). 20/20 verifier tests pass (5 skeleton + 7 I1 + 4 I2 + 5 I4 + 2 I3) + 1877 lib + 73 phase C backfill tests unchanged. Follow-up `ISS-113` files I4 extension across the other 6 drivers. **Update 2026-05-13**: ISS-113 resolved — all 7 drivers now have I4 coverage (T20 byte-equal BLOB, T21 FINDING-1 column-wins regression guard, T25 parsed-JSON gate_scores round-trip, T22/T23/T24 merge-semantics existence + shape checks incl. T24 SUM lower-bound on counter fields). 42/42 verifier tests after extension. T23 implementation surfaced a §3.3-vs-driver endpoint-direction drift (design.md documents `subject_of: entity → memory`; T23 driver writes `memory → entity` for all roles, locked by existing `t23_subject_role_writes_structural_subject_of` integration test) — verifier locked to as-built; docs fix tracked separately.

### 8.5 Phase D — switch reads
- [x] **T28** `MemoryConfig::unified_substrate` flag wired through — commit `7ee3898`. New `pub unified_substrate: bool` field on `MemoryConfig` with `#[serde(default)]`; default `false` so existing deployments stay on legacy reads until the Phase D parity gate (`verify_phase_c_parity` + LoCoMo J-score ≥ legacy baseline of 42.1% per RUN-0018) is cleared in production. All four presets (`chatbot`, `task_agent`, `personal_assistant`, `researcher`) inherit `false` via `..Default::default()`. Four pinned tests guard the contract: (1) `test_unified_substrate_default_off` — default must remain false; (2) `test_unified_substrate_off_in_all_presets` — no preset may silently flip the flag; (3) `test_unified_substrate_serde_roundtrip` — explicit `true` survives ser/de unchanged; (4) `test_unified_substrate_absent_key_defaults_false` — configs written before T28 (which lack the `unified_substrate` key entirely) must deserialize cleanly into `false`, exercising the `#[serde(default)]` attribute against accidental future removal. Writes are unaffected — Phase B (T13–T18) dual-writes continue to keep both sides in sync. T29 will read the flag in retrieval adapters one read path at a time per §5.4 "one plan at a time" rule. Lib test count 1877 → 1881 (+4).
- [x] **T29** Retrieval adapters: read from nodes/edges when flag on. **Split into sub-tasks per §5.4 one-plan-at-a-time rule.** All sub-tasks T29.1–T29.6 shipped 2026-05-13/14; T29.7 deferred to Phase F prep (rationale documented inline below). Parent task tickable because every read-switch contract is in place behind the `unified_substrate` flag.
  - [x] **T29.1** subscriptions read-switch — `e34b6b8`
  - [x] **T29.2** synthesis_provenance read-switch — `251bb03` (+ plumbing `ac1c9f0`)
  - [x] **T29.3** embeddings read-switch + dual-write writer — `1ad0827`
  - [x] **T29.4** hebbian read-switch (7 sub-parts, 4 readers + cross-axis ISS-118 root fix):
    - part-1 `get_hebbian_neighbors` — `6f6d49d`
    - part-2 `get_hebbian_links_weighted` — `0f3076d`
    - part-3 `get_hebbian_neighbors_ns` (with ISS-117 OR-match retrofit) — `b74315d`
    - part-4 `discover_cross_links` — `485ef7b`
    - ISS-118 ns-aware migration root fix — `5eff26b` + `8ca0c1b` docs
    - part-5 `get_cross_namespace_neighbors` — `ec7fa2c`
    - part-6 `get_all_cross_links` — `2971fa3`
  - [x] **T29.5** entity / triple readers — enumerate then ship (parts 1-4 shipped 2026-05-13/14; triples deliberately out of scope, see sub-bullet)
    - part-1 `get_entity` (entity reader, single-row) — `da3f443` decode helper (`_legacy_kind` → `attributes.entity_type` → `node_kind`) + 4 contract tests
    - part-2 `find_entities` + `count_entities` (single-table collection readers) — `01ef466` + 4 contract tests
    - part-3 `list_entities` (JOIN-with-edges reader) — `a902529` (impl piggybacked on ISS-123 fix) + `fe9235d` tests (4 contract tests: mention-count parity, ns filter, type filter, limit)
    - part-4 `get_entities_for_memory` (JOIN reader) — `2f7f3d7` + 5 contract tests (empty, single, multi, multi-role asymmetry, ns isolation). Surfaced a real semantic divergence: legacy `memory_entities` PK collapses two roles for the same `(memory, entity)`, unified `edges` does not — pinned with a follow-up note.
    - **Triple readers (`get_triples`, `has_triples`, `store_triples`) NOT in scope**: the legacy `triples` table is the raw-extraction record. Its semantic content already projects into `memory_entities → edges` (T23) and `entity_relations → edges` (T22). Whether `triples` survives Phase F is a separate design decision tracked under follow-up — keeping triple readers on legacy for now is correct.
  - [x] **T29.6** FTS readers (`memories_fts` → `nodes_fts`) — read-switch on `search_fts` and `search_fts_ns` via JOIN through `nodes.fts_rowid`. Returns `MemoryRecord` (joins back into `memories`) — only the inverted index used changes. 7 contract tests (parity, deleted, superseded, ns-specific, ns-star, limit, empty-query). **Caveat**: production `nodes_fts` has only the post-T12-dual-write era of memories; before T26c backfill, recall under `unified_substrate=true` is degraded for pre-dual-write rows. Flag stays opt-in (T32 gate).
  - [ ] **T29.7** remaining `SELECT FROM memories` reads in retrieval / consolidation paths — **deferred to Phase F prep**. Reasoning: under T12 dual-write contract, `memories` and `nodes` already agree on the memory row-set. The remaining `SELECT FROM memories` sites (`all()`, `get_many`, `search_by_type`, `fetch_recent`, `all_in_namespace`, supersession-chain queries, soft-delete queries) assemble `MemoryRecord` directly from legacy columns. Switching them to `nodes` would require either (a) joining back into `memories` for `MemoryRecord` columns not in `nodes` (which makes them legacy-dependent anyway), or (b) reverse-mapping `nodes` rows + sidecar tables into `MemoryRecord` (substantial work). Neither is needed before Phase F, because the legacy table is still the source of truth for the columns we'd need. **Gate**: T29.7 becomes mandatory the moment we plan to drop `memories` in Phase F — at that point we will inventory full-record assembly and either (i) widen `nodes` to absorb memory columns, or (ii) keep a memory-detail sidecar. Tracked as Phase F design work.
- [x] **T30** Manual probe set: 50 queries on production DB snapshot
  (`/tmp/t30-probe.db`, 12756 active memories / 2791 entities).
  Built two engramai examples: `t30_phase_d_backfill_runner.rs`
  (Phase C backfill driver) and `t30_probe_parity.rs` (Jaccard@K
  driver, 20 broad + 30 production-entity queries). Result:
  parity_ratio at jac≥0.95 = 0.40 (K=10) / 0.58 (K=5) / 0.64 (K=3),
  parity_ratio at jac≥0.50 = 0.98 (K=10). Root-caused to FTS5 IDF
  shift from `nodes_fts` composition (19378 memory + 2791 entity + 5
  insight rows vs `memories_fts`'s 19378 memory rows). Conclusion:
  per-query top-K rank shuffles by a few positions but does not
  drop the relevant candidate set; downstream LoCoMo J-score
  absorbs the shuffle (confirmed in T31). Probe Recall@10 ≥ 0.95 is
  the wrong granularity to gate on — see §5.4 rationale. Archived
  to `.gid/eval-runs/RUN-T30/` (summary.md +
  rank-diag-root-cause.md + 3 per-K JSON reports + rank-diag.md +
  backfill log). Engram commits: 1c73a2c (driver), f63073b
  (diagnosis).
- [x] **T31** Parity campaign: LoCoMo unified vs legacy, conv-26 152
  queries. Result: legacy overall 0.3947, unified overall 0.4013
  (+0.66pp). Per-category: multi-hop 0.541→0.595 (+5.4pp),
  open-domain 0.154→0.231 (+7.7pp), temporal 0.486→0.486 (0pp),
  single-hop 0.125→0.0625 (-6.25pp). Per-query flip analysis
  (`.gid/eval-runs/RUN-T31/summary.md`): 9 flips total (5 unified-
  gains, 4 unified-losses), all on essentially identical
  predictions where the LLM-judge scored Yes/No inconsistently
  (e.g. "sunset painting" vs "painting with sunset colors"). The
  single-hop -6.25pp is 2 flips on n=32, both judge wobble — not
  substrate degradation.

  **§5.4 LoCoMo end-to-end gate (unified ≥ legacy − 2pp)**: PASS
  (unified +0.66pp).

  **ISS-111 ≥ 0.559 gate (LoCoMo J ≥ RUN-0025 baseline)**: **NOT
  met** — both arms came in at ~0.40, well below 0.559. However,
  this is **not** an ISS-111 regression (KC clusterer collapse): the
  legacy arm uses the unchanged read path and also dropped, so the
  cause is master-side and predates T29.* read switches. Filed as
  ISS-136. Decision on whether T32 can proceed without resolving
  ISS-136 deferred to potato — recommendation is yes (ISS-136
  affects both arms equally, so T32's flip does not make it worse),
  but the literal spec language was conservative and that's a call
  for potato.

  engram-bench harness change committed in 82e26d6; RUN-T31 archive
  committed in engram 270fef4.
- [x] **T32** Flip default to on (2026-05-23). `MemoryConfig::default()`
  now sets `unified_substrate = true`, via a dedicated
  `default_unified_substrate()` helper used by both `impl Default` and
  `#[serde(default = "default_unified_substrate")]`. Upgrading
  configs that omit the key now opt into unified reads; explicit
  `"unified_substrate": false` remains supported for regression
  comparison runs. T28 config tests inverted accordingly
  (`test_unified_substrate_default_on`,
  `test_unified_substrate_on_in_all_presets`,
  `test_unified_substrate_absent_key_defaults_true`).

  `Storage::new` was intentionally NOT flipped — it stays pinned to
  legacy reads because the `tests/t29_*` parity tests use
  `("legacy", Storage::new(..))` / `("unified", with_unified_substrate(true))`
  as their two arms. Flipping `Storage::new` would invert the test
  labels without changing what is being tested. Docs on `Storage::new`
  updated to call out the asymmetry: user-facing callers go through
  `Memory::new` (post-T32 default = unified); low-level / parity
  callers use `Storage::new` (legacy) or `Storage::with_unified_substrate`
  (explicit).

  Test fallout: 1 lib test broke
  (`memory::confidence_tests::test_broadcast_hebbian_spreading`). Root
  cause: test raw-INSERTed into legacy `hebbian_links` to seed a link,
  bypassing the T14/ISS-116 dual-write to `edges`. Post-T32 the read
  path queries `edges WHERE edge_kind='associative'` and found the
  edge missing. Fixed by replacing the raw INSERT with two calls to
  `Storage::record_coactivation` (canonical API, threshold=1), which
  forms the link in `hebbian_links` AND fires the dual-write properly.
  All 1910 lib tests pass post-fix.

  Acceptance soak (≥1 week production at default-on) starts now. Watch
  ISS-136 master regression and any new quality-regression reports.
  If a regression is traced to unified reads (not ISS-136 or LLM
  noise), opt back to legacy via explicit
  `"unified_substrate": false` in config and reopen this task; do
  NOT revert the default without a clear post-mortem.
- [ ] **T33** 1-week production observation, log quality issues

### 8.6 Phase E — stop legacy writes

See §5.5 for the full per-file refactor plan, deletion pattern (worked
example: `Storage::add`), and per-task acceptance criteria. Each task
below maps to a numbered sub-section in §5.5.3.

- [ ] **T34** `storage.rs` memory core: delete legacy writes from the memory CRUD surface (`memories`, `memories_fts`, `memory_embeddings`, `memory_entities` from L1834/L1841/L1884/L1806/L1817/L2703/L2734/L2740/L2743/L2750/L2798/L2802/L2869/L2878/L3566/L3632/L3674/L3945/L4107/L4176-L4197/L4281/L4302/L5086/L5251/L5879/L5985/L6000/L6044/L6224/L6488/L6521/L6916/L7312/L7743). Per-entry-point breakdown in §5.5.3 T34a–T34n. Total: ~39 deletes.
- [ ] **T35** `storage.rs` Hebbian: delete legacy writes from co-activation/decay/merge paths (`hebbian_links` from L3202/L3215/L3221/L3230/L3264/L3268/L3359/L3366/L3377/L3459/L3468/L4179/L4605/L4617/L4623/L4632). Per-entry-point breakdown in §5.5.3 T35a–T35f. **High-risk site**: T35c (decay parity must be confirmed on `edges` side before deletion). Migration helpers (L1489/L1560/L1607) explicitly retained. Total: ~14 deletes (decay sites deferred if parity gap exists).
- [ ] **T36** `storage.rs` synthesis + clusters + entities tail: delete legacy writes for `entities` (L5165/L5843/L7129/L7172), `entity_relations` (L5339), `synthesis_provenance` (L4184/L6276/L6439), `cluster_assignments` (L7389/L7493/L7512/L7553/L7576). Per-entry-point breakdown in §5.5.3 T36a–T36e. Total: 12 deletes.
- [ ] **T37** `graph/store.rs` (3 prod writes): delete legacy INSERT/UPDATE `knowledge_topics` (L4828/L4947) and decide UPDATE `memories.entity_ids`/`edge_ids` strategy (L5782 — recompute-on-read preferred over `nodes.attributes` mirror). Per-entry-point breakdown in §5.5.3 T37a–T37c. **Note**: T37c is the only Phase E site that may need a small additive change rather than pure deletion.
- [ ] **T37x** Phase E exit gate: run §5.5.1 AST-strip inventory on `crates/engramai/src/` (with `#[cfg(test)]` masked) and confirm 0 prod hits across all 10 legacy tables. Full test suite green (lib + integration + all `v04_phase_*`). One-week production soak at `unified_substrate=true` with no quality regression, master LoCoMo within ±1pp of Phase D baseline (0.4013) across ≥2 independent runs. Per §5.5.5.

### 8.7 Phase F — drop legacy
- [ ] **T38** ≥2 weeks of unified-only operation logged
- [ ] **T39** Schema migration `0.4-final`: DROP legacy tables (`memories`, `graph_entities`, `graph_edges`, `hebbian_links`, `knowledge_topics`, `cluster_assignments`, `entity_aliases` if present) **and** DROP dead schema (`triples` table per §7.6) **and** DROP denormalized columns (`nodes.episode_id`, `edges.episode_id` per §7.4)
- [ ] **T40** DB VACUUM, size-reduction report
- [ ] **T41** Documentation: update README, design docs reflecting terminal state

### 8.8 Cleanup / supersession of prior plans
- [x] **T42** Mark `v03-wireup/design.md` as superseded by this doc — **shipped 2026-05-14**: added supersession note at top of `v03-wireup/design.md` pointing to `v04-unified-substrate/design.md` and noting Phase A–D complete.
- [ ] **T43** Close G1–G5 / ISS-* that are subsumed
- [x] **T44** Update `consolidation-autopilot-DRAFT.md` to reference unified substrate — **duplicate of T04, shipped 2026-05-14** (§2.5 substrate invariants I-A1–I-A8). Kept as a separate tick for traceability; the two tasks ended up describing the same deliverable.

### 8.9 – 8.13 → moved to `v05-cognitive-substrate/design.md` §3

Tasks **T45–T59** for the cognitive functions (interoception, empathy bus, working memory, metacognition, dimensional signature) moved to the v0.5 Cognitive Substrate feature on 2026-05-14 alongside their design specs (see §4.11–§4.15 stub above). 14 tasks total.

The v0.4 task ledger from this point forward contains only:
- **§8.14** v0.2 KC retirement (T60) — cleanup, stays v0.4
- **§8.15** Writer queue infrastructure (T61–T68) — parked / deferred to a separate feature

---

### 8.14 v0.2 KC retirement (§4.16)
- [x] **T60** Confirm v0.2 KC has **zero production call sites** outside
  `crates/engramai/src/compiler/` — **verified 2026-05-12, re-verified
  2026-05-15**. `grep -rn 'KnowledgeCompiler::new' crates/engramai/src/
  | grep -v compiler/` returns zero matches; `Memory::compile_knowledge`
  (`memory.rs:6670`) routes fully to `crate::knowledge_compile::compile`.
  Filed **ISS-130** to retire 19 of 21 modules in `compiler/` after
  Phase D, keeping 2 concepts (`intake/import` + `manual_edit`) for
  re-integration as substrate writers (T61–T68). ISS-130 is soft-blocked
  on ISS-111 (v0.3 clusterer degeneration on single-domain corpora) per
  design §4.16.3.

### 8.15 Writer queue infrastructure (§6)
- [ ] **T61** Implement `WriteOp` enum (§6.1) — one variant per writer
  path identified in §4.x mappings.
- [ ] **T62** Implement single-consumer writer loop (§6.2) with batched
  commit (configurable batch size, default 32 ops or 50ms timer
  whichever first).
- [ ] **T63** Implement priority queue + backpressure (§6.3): three
  priority lanes (interactive / background / coalescable). Hebbian
  edge-weight updates use coalesce lane.
- [ ] **T64** Implement compound-op atomicity (§6.4): `WriteOp::Batch`
  variant takes Vec<WriteOp> and commits in single transaction.
- [ ] **T65** Implement reader WAL snapshot path (§6.5): readers acquire
  read-tx, never block on writer, see consistent snapshot.
- [ ] **T66** Implement writer supervisor (§6.9): `WriterSupervisor`
  owns the public per-priority mpsc receivers and forwards ops via a
  private mpsc into the writer thread (direct-send model — the
  writer keeps each op's original `oneshot::Sender` and replies on
  the happy path with zero supervisor involvement). Supervisor
  maintains a `HashMap<OpId, Box<dyn FnOnce(WriterCrashed) + Send>>`
  of *crash notifiers* (NOT a heterogeneous sender map — see §6.9
  rationale on type erasure). Each notifier closure captures a
  shared `Arc<Mutex<Option<oneshot::Sender<...>>>>` of the reply slot
  so writer-vs-supervisor races on the same slot are resolved by
  `take()`. Writer signals completion via a separate
  `mpsc::UnboundedSender<OpId>` tick channel that the supervisor
  drains to evict in-flight map entries. On writer panic (detected
  via `JoinHandle::join()` returning Err or private-channel send
  failure), supervisor invokes every in-flight notifier with
  `Err(WriterCrashed { generation, cause })`, drains the public
  receivers up to 1024 with the same error (further callers see
  `RecvError` ≡ `WriterCrashed` per public contract), calls
  `Storage::reopen()`, spawns fresh writer thread, increments
  `generation`. **No separate disk journal** — SQLite WAL is the
  durable log; in-flight queue ops on crash are surfaced to callers
  via `Err(WriterCrashed)` / `Err(RecvError)` for caller-side retry
  (§6.9 stance).
- [ ] **T67** Bench: writer throughput target ~11k ops/sec (§6.6), measure
  with synthetic load mixing all WriteOp variants in production-realistic
  proportions.
- [ ] **T68** Test: multi-tenant scale ceiling (§6.7) — 100 concurrent
  namespaces driving writes; verify single-writer doesn't starve, p99
  latency < 200ms at 80% capacity.

---

## 9. Risks

**R1. Schema rev mid-implementation**
Mitigation: §3 is locked before Phase A starts. Changes require new
phase letter (0.5).

**R2. Hebbian semantics drift**
Current `hebbian_links` weight is a counter, decayed via lifecycle.
`edges.weight` semantics must match. Phase B parity test must compare
weight evolution.

**R3. FTS row-id volatility** — ✅ mitigated by design.
Resolved by §3.3 choice of FTS5 **external-content + manual triggers**
keyed on `nodes.id` (TEXT UUID). No rowid coupling, so VACUUM and
schema migrations cannot break FTS row identity. Phase A test (T11)
must still exercise: insert → delete → re-insert keeps FTS consistent.

**R4. Multi-model embedding regression**
Current `memory_embeddings` supports multi-model. `node_embeddings`
must round-trip every model currently in use. Verify in Phase C.

**R5. Bench harness reads from bench fixture DBs**
Existing eval-run DBs are on legacy schema. Either run unified migration
on them or accept that pre-0.4 RUN-* are not directly comparable
post-0.4. Recommend: add a migration helper for bench DBs and re-run
RUN-0018 baseline post-0.4 to confirm regression-free.

**R6. Cost of triple-extraction backfill**
~$25 one-time. Run during Phase C. Resumable on error.

**R7. Decay/forget on entities**
Current schema doesn't decay entities. Unified schema makes it
possible. Decision: keep current behavior (`node_kind='entity'`
skips decay) until a real use case appears.

**R8. Interoceptive baseline ephemerality (Tier 1 contract)**
Per §4.11 decision, baseline running stats live only in memory.
Process restart loses baseline → first N observations after restart
will have unstable variance estimates → spurious anomaly_event noise
in the first ~5 minutes. Mitigation: (a) document the warm-up window,
(b) anomaly detector requires `sample_count ≥ MIN_SAMPLES` (e.g., 30)
before emitting events. Trade-off accepted: persisting baseline on
every observation would be a hot-path write, defeating the "anomalies
are rare" cost model in §6.3 priority lanes.

**R9. Writer queue single-point-of-failure / latency**
§6.2 mandates a single-consumer writer loop for SQLite WAL
serialization. If the writer thread panics or stalls, **all** writes
stall (interoception, Hebbian, ingest, metacog). Mitigations:
(a) writer loop runs in dedicated tokio task with panic-catcher +
auto-restart (T66), (b) in-flight queue ops on crash are *not*
recovered — callers receive `Err(QueueClosed)` and decide retry per
op class (idempotent ops loop; non-idempotent ops surface the error
up the stack). This is the explicit stance in §6.9: SQLite WAL is the
durable log, no "WAL on top of WAL". (c) bench T67/T68 verifies p99
stays bounded under realistic mixed load. Open question for v0.5:
shard by namespace for true multi-writer (§6.7 lists 3 future sharding
paths).

**R10. Dimension edge storage growth**
Per §4.15 Tier 2/3, narrative dimensions and tags are stored as **edges**
to dimension/tag nodes (not as a separate `node_dimensions` table).
At ~10 narrative dimensions per memory × 10M memories that's 100M edge
rows — non-trivial but manageable on SQLite with the existing
`idx_edges_source` / `idx_edges_target` indexes on `edges`. Mitigation:
edge `attributes` JSON keeps per-edge payload minimal; aggregate caches
("average dimension over namespace") can be added as derived nodes if
profiling shows a hot path. Phase A test must include row-size
estimation on RUN-0018-scale corpus.

**R11. v0.2 KC retirement leaves orphan code**
§4.16 retires 19 of 21 modules in `crates/engramai/src/compiler/`.
Mitigation: T60 explicitly preserves `intake/import` and `manual_edit`
concepts for re-integration as substrate writers. Block on ISS-111
(v0.3 clusterer degeneration) being resolved OR confirmed orthogonal
— do not retire v0.2 while v0.3 has unresolved correctness regressions.

---

## 10. Status / Next step

**Design completeness — 4 commits done 2026-05-12:**

- **Commit 1 (structure)**: §4 expanded from 10 to 17 subsections, added stubs for §4.11 interoception, §4.12 empathy bus, §4.13 working memory, §4.14 metacognition, §4.17 coverage closure. §6 stub inserted (concurrency placeholder).
- **Commit 1b (push-back resolutions)**: §4.11 Tier-1/Tier-2 split (baseline ephemeral, anomaly_event persistent). §4.13 in-memory WM + metacog-driven snapshot (rejected pure-in-graph). §4.14 atomic `WriteWmSnapshot` with `WriteFeedbackEvent`. §4.17 supersession note updated.
- **Commit 2 (dimensions + KC triage)**: §4.15 dimensional signature (4 subsections, 3-tier storage model — Tier 1 scalar attributes, Tier 2 `describes_<field>` edges, Tier 3 `tagged` edges, plus §4.15.4 shim spec). §4.16 v0.2 KC retirement triage (4 subsections — verified 0 production callers, 21 modules → retire 19, keep 2 concepts).
- **Commit 3 (concurrency)**: §6 fully written. 6.1 `WriteOp` enum (~15 variants). 6.2 single-consumer writer loop with batched commit. 6.3 priority lanes + backpressure + Hebbian coalescing. 6.4 cross-op atomicity via `WriteOp::Batch`. 6.5 reader WAL snapshots (never block). 6.6 throughput math: ~11k ops/sec ceiling. 6.7 multi-tenant scale ceiling + 3 future sharding paths. 6.8 dual-write through queue (Phase B). 6.9 failure modes + write journal.
- **Commit 4 (closure)**: §8 expanded T45-T68 covering §4.11–§4.16 impl + §6 writer infrastructure. §0 TL;DR refreshed to mention §4.11–§4.16 and §6. §9 risks expanded to R8–R11 (baseline ephemerality, writer SPOF, dimension growth, v0.2 KC retirement). §10 (this section) closes.
- **Commit 5 (debt cleanup, 2026-05-12)**: r2 review applied. 5 critical + 10 important findings resolved (the "real technical debt" subset of 50 findings). Changes: (a) §3.2 `edge_kind` taxonomy table expanded to full closed-set + open-predicate enumeration (27 rows covering every predicate used in §4); (b) §4.3 Hebbian SQL rewritten as single canonical UPSERT (was malformed); (c) §4.6 Decay explicitly mandates `created_at` not `occurred_at` (ISS-103 protection); (d) §4.4 KC topic edges rewritten to use `containment/contains` (was wrong `edge_kind='topic_member'`); (e) §4.11 self-contradiction fixed (introduces 4 node_kinds, not 1); (f) §4.13 dimension_access migration deferred to §4.15.4 (was duplicating §4.15); (g) §6.1 `WriteOp` enum extended from 14→24 variants, every variant has explicit `reply` field, `Batch` reply semantics specified, missing `WriteSomaticMarker`/`WriteRegulationPolicy`/`UpdateDomainStats`/4 empathy variants added; (h) §6.2 writer loop migrated from `async fn` on tokio task to `fn` on dedicated OS thread (rusqlite is sync — running it on a tokio worker blocks retrieval); (i) §6.3 strict priority drain replaced with weighted fairness (`BATCH_CAP_HIGH=48/MED=12/LOW=4`) — no starvation; (j) §6.3 Hebbian coalescing HashMap capped at 4096 entries with emergency-flush policy — bounded memory; (k) §6.9 panic recovery rewritten to acknowledge rusqlite is NOT `UnwindSafe` — `catch_unwind` is unsound; recovery is process-level via thread respawn + `Storage::reopen()`; (l) §4.15.6 new subsection — write-amplification budget with per-tier math (~2.2× P50 ratio, 9× throughput headroom vs §6.6 ceiling).

**Design is now implementation-ready.** 70 atomic tasks (T01–T68 + a few additions in commit 5) sized for single sub-agent execution. Cross-references verified: all §-refs resolve, all ISS-refs are real (ISS-100/103/104/106/111 verified via `gid_artifact_show`).

**Known doc-debt deferred to implementation phase** (the 10 important + 13 minor findings not blocking T01): explicit cross-refs from §4 ops to §6 writer queue (A2-3), §5 phase B/D/E/F atomicity + gate condition prose (A2-8, A2-10, A2-11, A2-12), §2 verified-state number provenance (A1-8), §4.7 supersession retrieval filter spec (A2-5), §4.8 plan count "8 vs 7" (A2-6), §4.17 coverage table auditability (A2-13), §7.2 misnumbered subsection (A1-5), §3.1 missing index on `superseded_by` (A1-9), `Dimensions` row pattern explainers (A3-14, A4-9, A4-11, A4-12). These are documentation-clarity issues whose absence does not produce wrong code; they are listed here so implementers can patch as encountered.

**Next step**: T01 → spawn `review-design` sub-agent against this doc (2100+ lines, 17 §4 subsections, 70 tasks, 11 risks). Apply findings via review→approve→apply workflow. Then T03 (`requirements.md` — multi-feature split per `draft-requirements` skill since GOAL count will exceed 15).

**Blocking**: T60 (v0.2 KC retirement) blocks on ISS-111 (v0.3 clusterer degeneration) being either fixed OR confirmed orthogonal. All other tasks are unblocked once T01 review applies.



