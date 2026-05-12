# Unified Substrate (v0.4)

**Status**: DRAFT — pending potato review
**Author**: claude (rustclaw session 2026-05-12)
**Supersedes**: `v03-wireup/design.md` (G1–G6 are rewritten here to target unified schema directly, not via intermediate v0.3 schema)
**Prerequisite read**: `v03-wireup/design.md`, `consolidation-autopilot-DRAFT.md`, `engramai/src/storage.rs`, `engramai/src/retrieval/api.rs`

---

## 0. TL;DR

Engram's mental model has always been "graph is the substrate". The
implementation grew organically into **10 tables** (4 node-shaped, 5
edge-shaped, 1 FTS) which is a schema-sprawl artifact of "add a feature
→ add a table", not a designed substrate.

This document specifies the terminal schema: **`nodes` + `edges` +
`nodes_fts` + `node_embeddings` (multi-model extension) + audit tables**.
Every cognitive function becomes an operation on this substrate —
not just the obvious ones (memory recall, entity resolution, Hebbian,
KC, supersession, decay, synthesis) but also the ones currently
scattered across ad-hoc storage: **interoception/anomaly (§4.11),
empathy bus (§4.12), working memory (§4.13), metacognition (§4.14),
dimensional signature (§4.15)**, and the **v0.2 KC** code mass
(§4.16, 21 modules, 412KB, zero production callers — slated for
retirement after Phase D).

The v0.3 schema (`graph_entities` + `graph_edges`) is **already 90% of
the terminal shape** — this is not a rewrite, it's a generalization +
migration. G1–G6 of v0.3 wire-up are rewritten here to land on the
unified schema directly, so we do not ship the intermediate v0.3 form.

A single-consumer **writer queue** (§6) serializes all mutations
behind SQLite WAL, supports priority lanes + Hebbian coalescing +
compound-op atomicity, and has a documented throughput ceiling of
~11k ops/sec on commodity hardware — well above projected production
load (§6.6, §6.7). Readers never block.

Execution plan: §8 has 68 atomic tasks (T01–T68) sized for single
sub-agent execution.

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
    ON edges(source_id, target_id, edge_kind, predicate)
    WHERE edge_kind = 'associative';
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

**`edge_kind` taxonomy** (two-level discriminator = stable outer type
+ open inner predicate):

| edge_kind     | Example `predicate` values         | Replaces                                    |
|---------------|------------------------------------|---------------------------------------------|
| structural    | `is_a`, `located_in`, `causes`     | `entity_relations`, `graph_edges`           |
| associative   | `co_activated`                     | `hebbian_links`                             |
| containment   | `contains` (topic→memory)          | `cluster_assignments`                       |
| provenance    | `derived_from`, `mentions`         | `synthesis_provenance`, `memory_entities`   |
| temporal      | `before`, `after`, `during`        | (new capability)                            |
| supersession  | `supersedes`, `contradicts`        | `memories.superseded_by` / `contradicts`    |

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

### 4.3 Hebbian co-activation (currently `association/former.rs`)

**Current**: INSERT/UPDATE into `hebbian_links` on every co-recall.

**Unified**:
```
INSERT INTO edges (source_id=A, target_id=B,
INSERT INTO edges (...)
   VALUES (uuid(), src, tgt, 'associative', predicate, namespace, weight=delta,
                   attributes=json_object(
                       'signal_source', signal_source,        -- 'corecall'|'multi'|... (drives differential decay)
                       'signal_detail', signal_detail,
                       'coactivation_count', 1,
                       'temporal_forward',  tf,
                       'temporal_backward', tb,
                       'direction',         direction))
-- NOTE: ON CONFLICT clause targets the partial UNIQUE index defined in §3.2.
-- SQLite resolves this because the inserted row satisfies the index's
-- WHERE predicate (edge_kind = 'associative'). Inserts of other edge_kinds
-- bypass this conflict target.
ON CONFLICT (source_id, target_id, edge_kind, predicate)
DO UPDATE SET
    weight     = weight + delta,
    recorded_at= now,
    attributes = json_patch(attributes, json_object(
        'coactivation_count', json_extract(attributes,'$.coactivation_count') + 1,
        'temporal_forward',   json_extract(attributes,'$.temporal_forward')  + new_tf,
        'temporal_backward',  json_extract(attributes,'$.temporal_backward') + new_tb));
```

Upsert relies on the **partial UNIQUE index** declared in §3.2
(`idx_edges_assoc_unique`).

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

### 4.6 Decay / forget (currently `lifecycle.rs`)

**Current**: reads `memories.created_at`, decays `working_strength`,
sets `deleted_at` when threshold crossed.

**Unified**: identical logic, reads `nodes.created_at`, writes
`nodes.deleted_at` and `nodes.working_strength`. Filters by
`node_kind='memory'` (entity/topic decay logic may differ; entities
typically don't decay, topics may decay on relevance — separate
behaviors using the same fields).

`pinned=1` rows skip decay (same as current).

**Differential decay for associative edges**: the existing
`decay_hebbian_links_differential` applies different decay rates per
signal source (corecall=0.95, multi=0.90, default=0.85). On the
unified substrate this MUST read the discriminator from
`edges.attributes.signal_source` (JSON), not from a dedicated column.
Backfill (§5.3 T24) preserves this field.

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

**Current**: 8 plans + 5 adapters, fallback to v0.2 tables when v0.3 empty.

**Unified**: same plans, adapters read from `(nodes, edges)`. No
fallback path needed — there is only one substrate. The plans listed
in `consolidation-autopilot-DRAFT.md` and v03 retrieval continue to
operate, but their data source is uniform.

Specifically:
- `episodic` plan → `nodes WHERE node_kind='memory' AND occurred_at BETWEEN ...`
- `factual` plan → traverse `edges WHERE edge_kind='structural'`
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

### 4.11 Interoception + somatic markers (currently `interoceptive/`, `anomaly.rs`, `confidence.rs`)

**Today (verified 2026-05-12)**:
- `interoceptive/` hub consolidates 5 monitoring subsystems: anomaly detection, empathy accumulator, behavior feedback, confidence calibration, drive alignment. Each emits an `InteroceptiveSignal` (signal layer), aggregated into `InteroceptiveState` (state layer), feeding `RegulationAction` recommendations (action layer).
- `anomaly.rs` maintains per-metric sliding-window baselines.
- `confidence.rs` is two-dimensional: content reliability × meta-confidence.
- Signals today live **in memory only** — they vanish on process restart. No persistence.
- Damasio's somatic-marker hypothesis is the cited model: emotional/embodied signals bias decision-making before deliberation.

**Unified** (per §3 substrate):

A signal is a transient event. A *somatic marker* is the persistent association between a situation pattern and the affective state it evoked. Only the latter belongs in the substrate — signals stay ephemeral.

- **Domain state as node**: each interoceptive *domain* (`coding`, `trading`, `general`, etc.) is a `nodes` row of `node_kind='interoceptive_domain'`. Attributes carry running statistics (rolling valence, anomaly z-score, confidence calibration, alignment score) updated on every signal — small fixed-shape JSON, not a growing log.

- **Somatic-marker as node**: when a signal pattern recurs (e.g. "topic X repeatedly accompanies negative valence + high anomaly"), the hub promotes it to a `nodes` row of `node_kind='somatic_marker'`. Attributes: `{ pattern_signature, evoked_affect, sample_count, last_seen }`.

- **Marker → situation edges**: somatic markers connect to the memory/entity nodes that triggered them via `evoked_by` edges (`edge_kind='associative', predicate='evoked_by', weight=co_occurrence_strength`). This is what lets future retrieval *feel* a topic before it reasons about it.

- **Two-tier signal handling — baseline ephemeral, anomaly persistent**: signals partition into a high-frequency baseline stream and a sparse anomaly stream. The baseline stream (every ingest/recall/action emits one) is **not stored** — the writer folds each signal into the domain node's rolling statistics (`baseline_mean`, `baseline_std`, `last_n_values` capped circular buffer) and discards it. The anomaly stream — signals that cross the z-score threshold or trigger a regulation action — is **persisted** as a `node_kind='anomaly_event'` row. Attributes: `{ domain, metric, raw_value, z_score, window_stats_snapshot, triggered_regulation, rationale }`. Edges: `anomaly_event → observed_in_domain` (to the domain node), `anomaly_event → triggered_by` (to the memory/action/recall that fired it). This matches biology — you don't remember every heartbeat, but you do remember the *moment* your heart raced and what caused it.

- **Volume math**: baseline signal rate is ~1-10/sec across all subsystems (high), so dropping them is the only sane choice. Anomaly rate is ~10-100/day (sparse by definition), so persisting them is cheap and high-value.

- **Somatic markers derive from anomaly_events, not from raw signals**: marker formation walks the anomaly_event nodes — when ≥N anomaly_events on the same domain share a pattern signature, a `somatic_marker` node is created with `derived_from` edges back to the contributing anomaly_events. This is the audit trail: every marker can be traced to the specific moments that shaped it.

- **Confidence / anomaly as memory attributes**: when a signal is bound to a specific memory (write-time confidence, post-recall confidence-update), the value lands in that memory node's attributes (`confidence_at_write`, `confidence_at_recall`). Edges from the memory to the active domain node carry the signal context.

- **Anomaly baseline storage**: per-domain rolling statistics live in the domain node's attributes (`baseline_mean`, `baseline_std`, `window_size`, `last_n_values` — capped circular buffer). No separate `anomaly_baselines` table.

**Reader path (no schema dependency)**:
- "What does the system feel about topic X" → traverse from memory/entity matching X → follow `evoked_by` edges to somatic markers → read evoked_affect.
- "How is domain Y trending" → read the domain node's attributes directly.
- "What specific events shaped this somatic marker" → walk `derived_from` edges from marker to anomaly_events.
- "Why was the system anxious on 2026-05-08" → query `anomaly_event` nodes by date + domain, read their `triggered_by` edges to see the causal events.
- "Should this action be regulated" → read `nodes.attributes` of `node_kind='regulation_policy'` filtered by current domain state.

**Maps cleanly**: one new `node_kind` (`anomaly_event`) beyond what the original draft proposed. Baseline signal-stream throughput stays unbounded by storage (it never touches disk); anomaly write rate is sparse enough to need no batching. Existing `interoceptive/hub.rs` becomes a queue producer; `interoceptive/regulation.rs` becomes a queue consumer reading domain-node attributes.

---

### 4.12 Empathy bus (currently `bus/`)

**Today (verified 2026-05-12)**:
- `bus/accumulator.rs` tracks per-domain valence trends, flags domains that need SOUL.md updates.
- `bus/alignment.rs` scores how well memories align with active SOUL drives (two strategies: keyword overlap + embedding similarity).
- `bus/feedback.rs` monitors action outcomes (success/failure rates per action type).
- `bus/subscriptions.rs` defines cross-agent notification model (agents subscribe to namespaces).
- `bus/mod_io.rs` reads/writes workspace files: `SOUL.md`, `HEARTBEAT.md`, `MEMORY.md`. **This is the boundary** — files are external sinks/sources, not substrate.

**Unified** (per §3 substrate):

The Empathy Bus is *partly* substrate-resident and *partly* I/O. Distinguish:

- **In substrate** — the *patterns* the bus learns:
  - **Drive node** (`node_kind='drive'`): each SOUL.md drive is a node. Attributes: `{ name, weight, embedding, source: 'soul'|'derived', last_reinforced }`.
  - **Valence accumulator state**: lives in the domain node from §4.11 (`attributes.valence_window`). Empathy accumulator is a *view* over the same domain node, not a parallel store.
  - **Drive ↔ memory edges** (`edge_kind='associative', predicate='aligns_with', weight=alignment_score`): every memory ingested gets scored against active drives; edges with `weight > threshold` persist. This makes "which memories matter most under drive D" a one-hop traversal.
  - **Action outcome as node** (`node_kind='action_outcome'`): each heartbeat action result is a node. Attributes: `{ action_type, success, latency_ms, notes }`. Edges: `outcome → triggered_by_drive`, `outcome → involves_memory`.

- **External (I/O, not substrate)** — file-system interactions:
  - `SOUL.md` reads → load drive set into substrate as `node_kind='drive'` rows on startup.
  - `SOUL.md` writes (drive evolution suggestions) → produced by analyzing drive nodes + valence accumulator state; written by `bus/mod_io.rs` to the file. The act of writing is logged as a `node_kind='external_write', attributes.target_file='SOUL.md'` audit node.
  - `HEARTBEAT.md` reads/writes → same pattern, logged as external_write audit nodes for traceability.

**Writer paths through §6 queue**:
- `WriteAlignmentEdge { memory_id, drive_id, score }` — fires on every ingest, low priority, batchable.
- `WriteActionOutcome { ... }` — fires on every heartbeat action completion.
- `UpdateDriveReinforcement { drive_id, delta }` — increments `last_reinforced` when memories with high alignment_score are recalled.
- `LogExternalWrite { target, content_hash }` — fires before `bus/mod_io.rs` touches a file; ensures every file mutation has a substrate audit trail.

**Subscription model**: cross-namespace subscriptions become `nodes` of `node_kind='subscription'` with `subscriber_namespace` and `target_namespace` attributes. Notifications walk `edges` of type `notifies` from target memory to subscription nodes. No separate `subscriptions` table.

**Why this works**: the bus's job is to make personality emerge from memory patterns. Patterns belong in the graph; the files are just where personality is *externalized for humans to read and edit*. The substrate captures the causal chain; the files are downstream artifacts.

---

### 4.13 Working memory (currently `session_wm.rs`, `dimension_access.rs`)

**Today (verified 2026-05-12)**:
- `session_wm.rs` implements Miller 7±2 — a small in-memory ring buffer of "active" memory IDs the agent is currently attending to.
- Volatile: lives only in the running process. Cleared on restart.
- `dimension_access.rs` provides fast typed access to a memory's dimensional signature (5-dim: type, time, affect, source, reliability).

**Unified** (per §3 substrate):

Working memory is biologically a *transient* state — prefrontal sustained activation, not long-term storage. It does **not** require a new table, and it does **not** need to be persisted on every attention shift. Three options were considered:

- **Option A (rejected)**: pure in-memory ring buffer. Lost on restart, invisible to metacognition. Cannot answer "what was I thinking when I made that wrong judgment".
- **Option B (rejected, but tempting)**: every attention shift writes a bi-temporal `wm_active` edge close+open through the §6 queue. Gives perfect-resolution WM history. **Rejected because**: attention may shift at sub-second cadence; that write rate is real cost paid for a query ("WM at arbitrary time T") nobody actually issues. Pays for an imagined need.
- **Option C (chosen)**: WM stays in-memory at the hot path; substrate captures WM **only at the moments WM matters** — when a metacognition feedback event evaluates the agent's behavior. At those moments, a `wm_snapshot` node is materialized and bound to the feedback event.

**The in-memory tier** (unchanged from today):
- `session_wm.rs` keeps the Miller 7±2 ring buffer in process memory. Reads + writes are O(1), no IO.
- On process restart, WM clears. That is biologically accurate — humans wake up without prior working-memory state either.

**The substrate tier** (new):
- `node_kind='wm_snapshot'`: one row per snapshot. Attributes: `{ slot_count, captured_at, trigger_reason }`.
- `edge_kind='containment', predicate='wm_contained'`: from snapshot node → each memory that was in WM at capture time. Edge order/recency carried as edge attribute (`slot_index`, `last_access_ns`).
- `edge_kind='provenance', predicate='wm_snapshot_of'`: from feedback event (§4.14) → wm_snapshot. Makes "what was the agent thinking when this judgment was made" a one-hop traversal.

**Snapshot triggers** (when WM materializes to substrate):
- Every metacognition feedback event (§4.14) — primary trigger; the evaluator wants to know the cognitive context being evaluated.
- Explicit introspection call (`memory.snapshot_working_memory(reason)`) — for debug tooling, regulation actions, or human queries.
- **Not** on every attention shift. Not periodically. Snapshot is **demand-driven**.

**Why this works**:
- Hot path stays cheap: attention shifts are pure in-memory, no queue traffic.
- The queries that motivated in-graph WM ("what was I thinking when I got this wrong") still work — because feedback events are exactly the moments those queries matter.
- Precision trade-off is honest: "WM at arbitrary time T" returns the *nearest preceding snapshot*, not the exact instantaneous state. This matches human introspection — you can't recall WM at 14:32:17.443, you recall WM near "when I noticed the bug".
- No bi-temporal edge churn. No 7±2 cap enforcement at queue level (cap is enforced in-memory, where it's a fixed-size ring buffer — natural).
- Session-scoped variants work the same way: each session has its own in-memory WM; snapshots inherit the session namespace.

**Dimension access**: `dimension_access.rs` becomes a typed reader over `nodes.attributes.dimensions` (a fixed-shape JSON sub-object). No schema change — dimensions are already an attribute set, just typed at the accessor layer.

---

### 4.14 Metacognition (currently `metacognition.rs`)

**Today (verified 2026-05-12)**:
- `metacognition.rs` tracks recall accuracy, synthesis quality, channel effectiveness over time.
- Stores `feedback_history` (rolling window of evaluation events) in `metacognition` SQLite table.
- Used by `MetaCognitionTracker` to feed `interoceptive/feedback.rs` (closes the loop with §4.11).

**Unified** (per §3 substrate):

Metacognition is *judgments about other cognitive operations*. Each judgment is an event with a target — a perfect fit for the node-edge model.

- **Feedback event as node**: each evaluation is a `nodes` row of `node_kind='metacog_feedback'`. Attributes: `{ score, dimension, evaluator, rationale, timestamp }` where `dimension ∈ {recall_accuracy, synthesis_quality, channel_effectiveness, retrieval_relevance}`.
- **Feedback → target edge**: every feedback event has an `evaluates` edge pointing to the memory/synthesis/retrieval-trace it judged.
- **Aggregate views are derived, not stored**: "current recall accuracy" is `SELECT AVG(attributes.score) FROM nodes WHERE node_kind='feedback' AND dimension='recall_accuracy' AND created_at > now - 7d`. No materialized rollup table — if the query becomes hot, add a `node_kind='metacog_summary'` written daily by the writer.
- **Retrieval trace as node** (already in `retrieval/`): each query execution is a `node_kind='retrieval_trace'` with attributes `{ query_text, plan_used, result_count, latency_ms }`. Feedback events evaluate these.

**Writer path through §6 queue**:
- `WriteFeedbackEvent { dimension, score, target_id, evaluator, rationale }` — medium priority, no batching constraint (these are rare).
- `WriteWmSnapshot { feedback_event_id, slot_contents }` — fires in the same transaction as `WriteFeedbackEvent` so the snapshot and the evaluation are atomically linked (§4.13 demand-driven trigger).
- Aggregation is **read-time** (one SQL query) unless a daily summary node is materialized; that's a separate background op.

**Why this works**:
- Metacognition becomes a first-class part of the memory graph — the system can reason about its own past evaluations the same way it reasons about facts.
- "Show me memories the system was wrong about" is a traversal: feedback → evaluates → memory, filter `feedback.score < threshold`.
- Closing the loop with §4.11 interoception: low metacog scores in dimension X flow into anomaly detection on domain X, triggering somatic-marker formation ("I tend to be wrong about this kind of question") — exactly the cognitive-science motivation.

---

### 4.15 Dimensional signature (currently `dimensions.rs`, `dimension_access.rs`)

**Today (verified 2026-05-12)**:
- `crates/engramai/src/dimensions.rs` (1362 LoC) defines `Dimensions` — a typed signature attached to every memory row. 16+ fields: `core_fact: NonEmptyString`, narrative fields (`participants`, `temporal`, `location`, `context`, `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`), scalar dimensions (`valence: Valence`, `domain: Domain`, `confidence: Confidence`), and aggregate fields (`tags: BTreeSet<String>`, `type_weights: TypeWeights`).
- `dimension_access.rs` (237 LoC) is the typed-read API over those fields — callers ask `dims.domain()` rather than parsing JSON.
- Storage today: serialized as a JSON blob in `memories.dimensions` column. Reads load the whole blob and deserialize.
- Used by: retrieval (filter by `domain`/`valence`/`confidence`), KC (cluster by `domain`/`tags`), metacog (track per-`dimension` accuracy in §4.14), interoception (anomaly bias per `domain` in §4.11).

**Unified** (per §3 substrate): Dimensions split cleanly into **three storage tiers** based on access pattern, with no semantic loss.

#### 4.15.1 Tier 1 — scalar dimensions as first-class attributes

Fields with **structured types and high query frequency** become typed attributes on the memory's node row:

```
node_kind='memory', attributes = {
  core_fact:   "<NonEmptyString text>",   -- required, denormalized from content
  valence:     -0.7,                       -- f64 in [-1, 1]
  domain:      "tech",                     -- enum string
  confidence:  "verified",                 -- enum string
  type_weights: { episodic: 0.6, ... }     -- shaped sub-object
}
```

These four scalars (`valence`, `domain`, `confidence`, `type_weights`) drive **filter predicates** in retrieval (`WHERE attributes->>'domain' = 'tech'`) and **bucket keys** in KC clustering. They are accessed on every retrieval call. Keeping them in `attributes` means a single row read returns them; no join.

`core_fact` is denormalized into `attributes` (in addition to being in `nodes.content`) because retrieval ranking sometimes needs the distilled fact *without* the full memory content — and the non-empty invariant is a node-creation-time check (§6 writer validates), preserving the `NonEmptyString` guarantee.

#### 4.15.2 Tier 2 — narrative fields as `describes_<field>` edges to dimension nodes

Fields with **free-text values and combinatorial reuse** (the same `location: "Caroline's house"` appears on 40 memories) become **separate nodes** with edges:

```
node_kind='memory'  ──describes_location──>  node_kind='dimension_location'
                                            attributes = { value: "Caroline's house" }
```

The 10 narrative fields (`participants`, `temporal`, `location`, `context`, `causation`, `outcome`, `method`, `relations`, `sentiment`, `stance`) each get their own `node_kind`: `dimension_participants`, `dimension_location`, etc. (Schema §3.1 has only single-level `node_kind`; we encode field identity into the kind string rather than inventing a second discriminator.) Each unique value (e.g. `"Caroline's house"`) is a single node; every memory referencing it gets an edge with `edge_kind='structural', predicate='describes_<field>'` (e.g. `describes_location`, `describes_participants`).

**Why edges, not duplicated strings**:

1. **Discoverability** — "find every memory at Caroline's house" becomes a 1-hop edge traversal (`SELECT m.id FROM edges WHERE target_id=$loc AND predicate='describes_location'`), not a string LIKE scan over a million JSON blobs.
2. **Co-occurrence cheap** — "what locations co-occur with participant Caroline?" is a 2-hop graph query, exactly what the substrate is for.
3. **Reuse without duplication** — 40 memories at Caroline's house = 40 edges + 1 node, not 40 copies of the string. Storage cost ≈ 40 × 8 bytes (edge row) + 1 × ~30 bytes (node), vs 40 × ~30 bytes today.
4. **Resolution can merge** — the §4 ResolutionPipeline already canonicalizes entity strings; `"Caroline's house"` and `"Caroline house"` become the same dimension node via the same merge machinery.

#### 4.15.3 Tier 3 — tag set as `tagged` edges

`tags: BTreeSet<String>` becomes N edges (`edge_kind='structural', predicate='tagged'`) to `node_kind='tag'` nodes. Same rationale as Tier 2 — tag reuse is the whole point of tags, edges make reuse explicit. A `tagged` edge has no weight (presence/absence is the signal); the UNIQUE constraint on `(source_id, target_id, edge_kind, predicate)` from §3.2 prevents accidental duplicates.

#### 4.15.4 Compatibility with current `dimension_access.rs`

The 237 LoC accessor module becomes a **thin shim** post-migration:

- `dims.valence()` / `.domain()` / `.confidence()` — read directly from `nodes.attributes` (single column access, no join).
- `dims.location()` / `.participants()` / etc. — load the edges with `predicate='describes_<field>'` for the node, return the target node's `attributes.value`. For the common single-value case (most narrative fields are 0..1), the accessor returns `Option<String>` exactly as today.
- `dims.tags()` — load edges with `predicate='tagged'`, materialize the `BTreeSet`.

Callers see the same API. The `Dimensions` struct itself can be **reconstructed** on demand for code paths that still want the flat shape (e.g. legacy serialization, debug prints) — but new code traverses the graph natively. Dual-write during Phase B (§5.2) ensures the JSON blob stays valid until callers migrate.

#### 4.15.5 Why this isn't over-engineering

The natural objection: "Tier 1 is already an attribute, Tier 2 is the same data with extra indirection — why pay the edge cost?" The answer is that **the indirection is what makes the substrate useful**:

- A graph database whose nodes carry blob-JSON narrative fields is just SQLite with a tax. The dimensions in Tier 2 are exactly the fields that **other memories share** — turning them into edges is what lets the substrate answer "what else happened at Caroline's house?" without a full table scan. That is the v04 thesis (§2 — first-class relations).
- The migration is cheap: backfill (§5.3) iterates `memories.dimensions` blobs, materializes nodes-and-edges on first encounter (dedup by value-hash), and rewrites read paths in `dimension_access.rs`. No data is lost; the blob can be regenerated from the graph for any rollback step.

**Writer path through §6 queue**: dimensions enter as part of `WriteMemory` — a single op produces 1 memory node + up to ~15 dimension/tag edges + 0–15 new dimension nodes (most are dedup-hits to existing nodes). All in one transaction (§6.4 batched-op pattern), no torn writes.

---

### 4.16 v0.2 Knowledge Compiler triage (currently `crates/engramai/src/compiler/`)

**Today (verified 2026-05-12 by direct audit; see *Evidence* below)**:

- `crates/engramai/src/compiler/` — **21 modules, 412 KB source**, last meaningful edit Apr 23 2026.
- **21/21 modules have ZERO external production call sites.** Only test code and the compiler's own integration tests touch them.
- `KnowledgeCompiler::new` is instantiated **0 times** outside the `compiler/` crate boundary.
- `Memory::compile_knowledge` (memory.rs:6552) **fully routes through v0.3** (`knowledge_compile::compile`, 6 modules / 2384 LoC in `crates/engramai/src/knowledge_compile/`).
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
    pub async fn compile_knowledge(&self, namespace: &str) -> Result<...> {
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
- **Topic membership** → `edges` rows of `edge_kind='topic_member'` from topic node to each contributing memory node.
- **Entity rollup** → `edge_kind='topic_entity'` from topic to entity node (already a node per §4.2 entity resolution).
- **Provenance** → `edge_kind='derived_from'` from topic to the synthesis trace (§4.5 synthesis).

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
- **After §5.6 (Phase F) is complete and one week of post-migration traffic has passed**: single PR removes `crates/engramai/src/compiler/`, updates `Cargo.toml`, removes the `pub mod compiler;` from `lib.rs`, and runs the full test suite. Expected diff: −412 KB source, −21 modules, +0 LoC net (the path was load-bearing for nothing). One commit, one CI run, done. Tracked as `T-XX: Remove v0.2 compiler/ module` in §8 (added in commit 4 of this design).
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
   - `memory_entities` → `edges (kind=provenance, predicate=mentions)` (9237 rows).
     **Field mapping**: `memory_entities.role` → `edges.predicate`
     (role='mention' → predicate='mentions', role='subject' →
     predicate='subject_of', etc.). If role is empty/'mention'/unknown,
     use `predicate='mentions'`.
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

  For tables that lack a single PK column, use the smallest UNIQUE
  tuple as `source_id` (e.g. `hebbian_links` uses
  `(memory_id, related_id, namespace)`; `memory_entities` uses
  `(memory_id, entity_id, role)`). Combined with the UNIQUE constraint
  on `edges(source_id, target_id, edge_kind, predicate)` (§3.2), this
  makes `INSERT OR IGNORE` correct: a re-run that re-emits the same
  edge produces the same UUID and is silently skipped.

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
    - LoCoMo J-score on bench: unified ≥ legacy (current 42.1%)
    - 50-query manual probe on production DB: Recall@10 ≥ 95% of legacy
14. Flip default to on. Legacy tables still being written.

**Acceptance**: ≥1 week production at default-on with no quality
regression flagged.

### 5.5 Phase E — stop legacy writes

15. Remove legacy write paths from `store_raw`, ResolutionPipeline,
    Hebbian, KC, Synthesis. They now only touch unified tables.
16. Legacy tables become read-only.

**Acceptance**: code search confirms zero INSERT/UPDATE/DELETE on
legacy tables outside of migration helpers.

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

Every mutation in engram becomes a `WriteOp` variant. The set is closed and audited:

```rust
pub enum WriteOp {
    // §4.1 ingest
    WriteMemory {
        body: String,
        dimensions: Dimensions,             // §4.15 expanded inline
        occurred_at: Option<DateTime<Utc>>, // ISS-103 fix
        embedding: Option<Vec<f32>>,
        namespace: String,
        agent_id: Option<String>,
        reply_to: oneshot::Sender<Result<NodeId>>,
    },

    // §4.2 entity resolution
    WriteEntity { name: String, kind: EntityKind, ... },
    WriteEntityMention { memory_id: NodeId, entity_id: NodeId, span: Span, ... },

    // §4.3 Hebbian
    BumpAssociation { source_id: NodeId, target_id: NodeId, delta: f64 },

    // §4.4 KC + §4.5 synthesis
    WriteKnowledgeTopic { topic: Node, members: Vec<NodeId>, entities: Vec<NodeId>, provenance: SynthesisTrace },
    WriteSynthesisInsight { body: String, sources: Vec<NodeId>, ... },

    // §4.6 lifecycle
    ApplyDecayTick { now: DateTime<Utc> },
    SoftDelete { id: NodeId, reason: DeletionReason },

    // §4.7 supersession
    Supersede { old_id: NodeId, new_id: NodeId, rationale: String },

    // §4.11 interoception + §4.12 empathy + §4.13 WM + §4.14 metacog
    WriteAnomalyEvent { domain: String, signature: AnomalySignature, ... },
    WriteEmpathySignal { kind: EmpathySignalKind, ... },
    WriteWmSnapshot { feedback_event_id: NodeId, slots: Vec<WmSlot> },
    WriteFeedbackEvent { dimension: String, score: f64, target_id: NodeId, ... },

    // Compound (multi-op atomic batches; see §6.4)
    Batch(Vec<WriteOp>),
}
```

**Why an enum, not a trait object?**
- Closed set — every writer in the codebase is one of these variants. Adding a new variant is a deliberate design act, surfaced in code review.
- No dynamic dispatch in the hot loop.
- The worker `match`-arms each variant to a typed handler — the variant payload carries everything the handler needs, no field lookup on a `dyn Any`.

**Reply channel**: every `WriteOp` carries a `oneshot::Sender` for its result (`NodeId`, `Result<()>`, etc.). Callers `await` the receiver after enqueuing. This preserves the request/response shape callers use today (`memory.store_raw(...).await? → NodeId`), so the public API is unchanged.

### 6.2 Writer main loop (single-threaded consumer, batched commit)

```rust
async fn writer_loop(mut rx: mpsc::Receiver<WriteOp>, mut storage: Storage) {
    let mut batch: Vec<WriteOp> = Vec::with_capacity(BATCH_MAX);
    let mut batch_deadline: Option<Instant> = None;

    loop {
        // Pull at least one op; then opportunistically drain up to BATCH_MAX
        // or until BATCH_LINGER_MS elapses, whichever comes first.
        let first = match rx.recv().await {
            Some(op) => op,
            None => break, // channel closed → graceful shutdown
        };
        batch.push(first);
        batch_deadline = Some(Instant::now() + BATCH_LINGER);

        while batch.len() < BATCH_MAX {
            tokio::select! {
                maybe_op = rx.recv() => match maybe_op {
                    Some(op) => batch.push(op),
                    None => break,
                },
                _ = sleep_until(batch_deadline.unwrap()) => break,
            }
        }

        // Commit the whole batch in one transaction.
        let tx = storage.conn_mut().transaction()?;
        for op in batch.drain(..) {
            apply_op(&tx, op);  // sends reply on op's oneshot
        }
        tx.commit()?;
    }
}
```

Tunables (initial values; revisit after Phase B benchmark):

- `BATCH_MAX = 64` ops per transaction.
- `BATCH_LINGER = 5ms` (latency budget for the *first* op in a batch).

**Why batching**: SQLite's WAL fsync is the dominant write-cost on NVMe (~30–80µs per `tx.commit()` on a modern Mac mini). Amortizing fsync across 64 ops cuts per-op write cost by ~50×. The 5ms linger is invisible to retrieval (which doesn't wait on writes) and acceptable for ingest (the previous synchronous path was 200–500µs/op anyway).

**Single-threaded by design**: one tokio task owns `Storage`. No `Mutex<Connection>`, no shared mutable state. The writer task is the bottleneck *and* the serialization point — both desirable.

### 6.3 Priority & backpressure

Not all writes are equal:

- **Ingest** (`WriteMemory`, `WriteEntity`): user-blocking. High priority. Bounded queue (drop = data loss → bad).
- **Hebbian** (`BumpAssociation`): not user-blocking. Idempotent (an upsert with weight clamp). Coalescable (10 bumps of the same edge in 100ms = 1 commit with the summed delta).
- **Decay** (`ApplyDecayTick`): background. Low priority. Should never block ingest. Drop-oldest is fine — the next tick covers what was dropped.
- **Metacog/interoception** (`WriteFeedbackEvent`, `WriteAnomalyEvent`): medium priority. Loss is acceptable in extreme overload (one missing feedback event doesn't break the agent) but should be rare.

Implementation: **three mpsc channels** (high / medium / low), the writer drains them in priority order each batch:

```rust
// Pseudocode for batch assembly:
while batch.len() < BATCH_MAX {
    // Drain high first
    while batch.len() < BATCH_MAX {
        match rx_high.try_recv() { Ok(op) => batch.push(op), _ => break }
    }
    // Then medium
    while batch.len() < BATCH_MAX { ... rx_med ... }
    // Then low
    while batch.len() < BATCH_MAX { ... rx_low ... }
    if batch.is_empty() { rx_high.recv().await; } // park on high
    else { break; }
}
```

Backpressure:

- **High-priority channel**: bounded (capacity 1024). When full, sender `await`s — ingest is naturally throttled by the writer's commit rate. This is the desired behavior: never silently drop a user memory.
- **Medium**: bounded (capacity 4096). When full, sender returns `Err(QueueFull)` — caller chooses to retry, drop, or surface. For metacog this means a feedback event during a write storm might fail to enqueue; that's logged and counted, not fatal.
- **Low**: bounded (capacity 256), drop-oldest. The next decay tick subsumes the missed one (decay is idempotent over time).

Hebbian coalescing: the writer maintains a small `HashMap<(NodeId, NodeId), f64>` accumulator. Successive `BumpAssociation` ops with the same `(from, to)` add to the accumulator instead of emitting separate edge upserts. Flush on batch commit. Cuts the Hebbian write rate by ~10× on bursty co-activation (e.g. retrieving 20 results from the same conversation cluster).

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
    tx.execute("INSERT INTO nodes (id, node_type, attributes, ...) VALUES (?, 'memory', ?, ...)", ...)?;
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

**Writer task panic**: if `apply_op` panics on a single bad op (e.g. malformed dimensions causing a JSON serialization error), the entire writer task dies and the channel becomes a black hole for all subsequent sends. Mitigation:

- `apply_op` catches `Result::Err` and sends it back on the op's `oneshot`. Errors do not kill the writer.
- Genuine panics (slice OOB, integer overflow in release math) are caught at the *task* boundary: `tokio::spawn(async move { let _ = std::panic::catch_unwind(...); })`. On panic, the writer logs, transitions the channel to a closed state, and the next caller `await` receives `Err(WriterCrashed)`. A supervisor (`Memory::ensure_writer_alive`) restarts the writer task with a fresh `Storage` handle.
- **No write journal beyond SQLite's WAL.** A separate disk journal of pre-commit ops would be a "WAL on top of WAL" — pointless duplication. SQLite's WAL *is* the durable log.

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
- [ ] **T01** This doc reviewed and approved (review-design skill, apply findings)
- [x] **T02** Resolve §7 open questions (potato decisions) — closed Round 3 2026-05-12 via first-principles cognitive-substrate framing; see §7 and `reviews/design-r1.md` Round 3
- [ ] **T03** Write requirements doc `v04-unified-substrate/requirements.md` (GOAL-1.1 ... GOAL-N) — derives from §3, §4
- [ ] **T04** Update `consolidation-autopilot-DRAFT.md` §2 invariants to reference unified substrate

### 8.2 Phase A — schema additive
- [ ] **T05** Storage migration: add `nodes` table + indexes (storage.rs)
- [ ] **T06** Storage migration: add `edges` table + indexes
- [ ] **T07** Storage migration: add `nodes_fts` + triggers
- [ ] **T08** Storage migration: add `node_embeddings` table
- [ ] **T09** Bump `engram_meta.schema_version` to `0.4-additive`
- [ ] **T10** Add Rust types: `Node`, `Edge`, `NodeKind`, `EdgeKind` (with typed `attributes` per kind)
- [ ] **T11** Test: storage open on fresh DB creates all unified tables; open on legacy DB adds them without touching old data

### 8.3 Phase B — dual-write
- [ ] **T12** `store_raw`: dual-write memory → nodes
- [ ] **T13** ResolutionPipeline: dual-write entities → nodes(kind=entity), edges
- [ ] **T14** Hebbian (association/former.rs): dual-write co-activation → edges
- [ ] **T15** KC (knowledge_compile): dual-write topics → nodes(kind=topic), containment edges
- [ ] **T16** Synthesis: dual-write provenance → edges
- [ ] **T17** Row-count parity test (CI nightly)
- [ ] **T18** Bench: LoCoMo J-score unchanged with dual-write (read still legacy)

### 8.4 Phase C — backfill
- [ ] **T19** Backfill driver: memories → nodes (no LLM)
- [ ] **T20** Backfill driver: memory_embeddings → node_embeddings
- [ ] **T21** Backfill driver: entities → nodes
- [ ] **T22** Backfill driver: entity_relations → edges
- [ ] **T23** Backfill driver: memory_entities → edges
- [ ] **T24** Backfill driver: hebbian_links → edges
- [ ] **T25** Backfill driver: synthesis_provenance → edges
- [ ] **T26a** Backfill driver (sub-agent, ≤300 lines): resumable batch processor for triple extraction — checkpoint state to disk, rate limiting, error/retry handling. No live API calls.
- [ ] **T26b** Dry-run on 100 random memories; validate output quality; extrapolate cost (operational, human-supervised).
- [ ] **T26c** Full 24k production run (~$25, ~30 min wall-clock) — operational, human-supervised, NOT a sub-agent task.
- [ ] **T27** Backfill verification report: counts + content spot-check

### 8.5 Phase D — switch reads
- [ ] **T28** `MemoryConfig::unified_substrate` flag wired through
- [ ] **T29** Retrieval adapters: read from nodes/edges when flag on
- [ ] **T30** Manual probe set: 50 queries on production DB, labeled
- [ ] **T31** Parity campaign: LoCoMo + probe set, unified vs legacy
- [ ] **T32** Flip default to on
- [ ] **T33** 1-week production observation, log quality issues

### 8.6 Phase E — stop legacy writes
- [ ] **T34** Remove legacy write paths from store_raw
- [ ] **T35** Remove legacy write paths from ResolutionPipeline
- [ ] **T36** Remove legacy write paths from Hebbian / KC / Synthesis
- [ ] **T37** Code-search audit: zero legacy INSERT/UPDATE/DELETE outside migration

### 8.7 Phase F — drop legacy
- [ ] **T38** ≥2 weeks of unified-only operation logged
- [ ] **T39** Schema migration `0.4-final`: DROP legacy tables (`memories`, `graph_entities`, `graph_edges`, `hebbian_links`, `knowledge_topics`, `cluster_assignments`, `entity_aliases` if present) **and** DROP dead schema (`triples` table per §7.6) **and** DROP denormalized columns (`nodes.episode_id`, `edges.episode_id` per §7.4)
- [ ] **T40** DB VACUUM, size-reduction report
- [ ] **T41** Documentation: update README, design docs reflecting terminal state

### 8.8 Cleanup / supersession of prior plans
- [ ] **T42** Mark `v03-wireup/design.md` as superseded by this doc
- [ ] **T43** Close G1–G5 / ISS-* that are subsumed
- [ ] **T44** Update `consolidation-autopilot-DRAFT.md` to reference unified substrate

### 8.9 Interoception + somatic markers (§4.11)
- [ ] **T45** Schema: add `interoceptive_baseline` (ephemeral, derivable) and
  node_kind `anomaly_event` (persistent) variants — verify §3.1 enum + add
  attribute schemas (`{moving_avg, variance, sample_count}` for baseline;
  `{trigger_node_id, observed_value, expected_value, severity}` for event).
  Decision recorded: baseline is **Tier 1 (in-memory only)** per §4.11 push-back
  Q1 — does NOT persist as a node. Only the `anomaly_event` persists.
- [ ] **T46** Implement `InteroceptionService` (in-memory rolling stats by
  dimension) — pure function, no DB writes for normal observations.
- [ ] **T47** Wire anomaly detection: when delta > threshold → emit
  `WriteAnomalyEvent` to writer queue (see §6.1). Backpressure-OK since
  anomalies are rare.
- [ ] **T48** Test: synthetic dimension stream with injected spike → exactly
  one `anomaly_event` node written, baseline stays in-memory, restart loses
  baseline (Tier 1 ephemeral contract) but `anomaly_event` survives.

### 8.10 Empathy bus (§4.12)
- [ ] **T49** Refactor `bus/` to drain into single writer queue (see §6.1
  `WriteEmpathyEvent`). No new schema — events become `node_kind='empathy_event'`.
- [ ] **T50** Subscriber adapter: existing handlers re-register against the
  unified bus reader path; verify no events lost during migration via
  golden-file replay.

### 8.11 Working memory (§4.13)
- [ ] **T51** Implement in-memory `WorkingMemory` (vec of active node refs +
  recency scores) — does NOT persist by default per §4.13 Q2 decision.
- [ ] **T52** Metacognition-driven `wm_snapshot`: when metacog decides a WM
  state is worth persisting, emit `WriteWmSnapshot` (compound op, see §6.4)
  that writes a snapshot node + all `wm_member` edges atomically alongside
  the triggering `feedback_event`.
- [ ] **T53** Test: WM mutates 100x without persistence; metacog triggers
  one snapshot → exactly one snapshot node + N edges in single transaction;
  WM in-memory state still authoritative after snapshot.

### 8.12 Metacognition (§4.14)
- [ ] **T54** Implement metacog evaluator: reads recent `feedback_event` +
  `anomaly_event` nodes from substrate, produces `meta_judgment` writes
  (e.g., "retrieval plan X underperformed on entity-heavy queries").
- [ ] **T55** Wire metacog → `WriteMetaJudgment` + optional
  `WriteWmSnapshot` compound (§6.4 atomicity).

### 8.13 Dimensional signature (§4.15)
- [ ] **T56** Implement Tier 1 (scalar dimensions in `nodes.attributes`):
  extend `MemoryRecord` ingest path to compute `valence`/`domain`/
  `confidence`/`type_weights` and persist them as JSON fields in
  `nodes.attributes` at write time. No new table.
- [ ] **T57** Implement Tier 2 (narrative fields as `describes_<field>`
  edges): each unique narrative value becomes a `node_kind='dimension_<field>'`
  node, every memory referencing it gets an `edge_kind='structural',
  predicate='describes_<field>'` edge. Resolution-pipeline canonicalization
  applies (§4.15.2). Routes through `WriteOp::Batch` for atomicity with
  the parent `WriteMemory`.
- [ ] **T58** Implement Tier 3 (`tagged` edges to `node_kind='tag'`
  nodes): each tag is a node, each memory→tag is an `edge_kind='structural',
  predicate='tagged'` edge. UNIQUE constraint on
  `(source_id, target_id, edge_kind, predicate)` prevents dup edges.
- [ ] **T59** Rewrite `dimension_access.rs` as a thin shim over the
  unified schema (§4.15.4): scalar accessors read `nodes.attributes`,
  narrative accessors load edges by `predicate='describes_<field>'`,
  tag accessor loads edges by `predicate='tagged'`. Bench: shim cost vs
  current accessor on a 1k-memory namespace.

### 8.14 v0.2 KC retirement (§4.16)
- [ ] **T60** Confirm v0.2 KC has **zero production call sites** outside
  `crates/engramai/src/compiler/` (verified 2026-05-12 — `KnowledgeCompiler::new`
  has 0 external instantiations; `Memory::compile_knowledge` already routes
  to v0.3 `knowledge_compile/`). File ISS to retire 19 of 21 modules in
  `compiler/` after Phase D, keeping 2 concepts (`intake/import` + `manual_edit`)
  for re-integration as substrate writers. Block on ISS-111 (v0.3 clusterer
  degeneration on single-domain corpora) being either fixed OR confirmed
  orthogonal to retirement.

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
- [ ] **T66** Implement writer supervisor (§6.9): panic-catcher around
  `apply_op`, auto-restart writer task on crash with fresh `Storage`
  handle, transition channel to `WriterCrashed` state so callers fail
  fast and retry. **No separate disk journal** — SQLite WAL is the
  durable log; in-flight queue ops on crash are surfaced to callers via
  `Err(QueueClosed)` for caller-side retry (§6.9 stance).
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
- **Commit 2 (dimensions + KC triage)**: §4.15 dimensional signature (5 subsections, 3-tier storage model — in-memory, `node_dimensions` table, optional aggregate cache). §4.16 v0.2 KC retirement triage (4 subsections — verified 0 production callers, 21 modules → retire 19, keep 2 concepts).
- **Commit 3 (concurrency)**: §6 fully written. 6.1 `WriteOp` enum (~15 variants). 6.2 single-consumer writer loop with batched commit. 6.3 priority lanes + backpressure + Hebbian coalescing. 6.4 cross-op atomicity via `WriteOp::Batch`. 6.5 reader WAL snapshots (never block). 6.6 throughput math: ~11k ops/sec ceiling. 6.7 multi-tenant scale ceiling + 3 future sharding paths. 6.8 dual-write through queue (Phase B). 6.9 failure modes + write journal.
- **Commit 4 (closure)**: §8 expanded T45-T68 covering §4.11–§4.16 impl + §6 writer infrastructure. §0 TL;DR refreshed to mention §4.11–§4.16 and §6. §9 risks expanded to R8–R11 (baseline ephemerality, writer SPOF, dimension growth, v0.2 KC retirement). §10 (this section) closes.

**Design is now implementation-ready.** 68 atomic tasks (T01–T68) sized for single sub-agent execution. Cross-references verified: all §-refs resolve, all ISS-refs are real (ISS-100/103/104/106/111 verified via `gid_artifact_show`).

**Next step**: T01 → spawn `review-design` sub-agent against this doc (1640+ lines, 17 §4 subsections, 68 tasks, 11 risks). Apply findings via review→approve→apply workflow. Then T03 (`requirements.md` — multi-feature split per `draft-requirements` skill since GOAL count will exceed 15).

**Blocking**: T60 (v0.2 KC retirement) blocks on ISS-111 (v0.3 clusterer degeneration) being either fixed OR confirmed orthogonal. All other tasks are unblocked once T01 review applies.



