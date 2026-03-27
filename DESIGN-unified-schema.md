# Engram Unified Schema — Design Doc

> Defining the canonical storage format for AI agent cognitive memory.
> Any language, any bot, one shared memory.

**Date**: 2026-03-22
**Status**: Draft
**Author**: potato + Clawd

---

## Problem

Engram has 3 implementations (Python, Rust, TypeScript) that each defined their own SQLite schema independently. A memory written by Python can't be read by Rust due to:

- `created_at`: Python uses REAL (unix float), Rust uses TEXT (ISO 8601)
- Column names differ: `source_file` (Python) vs `source` (Rust)
- Python has extra columns (`summary`, `tokens`) that Rust/TS don't
- FTS5 indexes differ (Python indexes content+summary+tokens, Rust indexes content only)

This means a bot built in Rust can't read memories stored by a Python bot, breaking the core promise of Engram as a universal cognitive memory system.

## Goal

**One canonical schema spec** that all implementations conform to. A memory stored by any Engram client (Python CLI, Rust agent, TS bot, future Go/Java/etc.) is readable and writable by every other client.

## Design Principles

1. **SQLite is the protocol** — the DB file IS the interface, not an API
2. **Lowest common denominator types** — use types every language handles natively
3. **Forward-compatible** — unknown columns are ignored, not rejected
4. **No migration hell** — include a version table and auto-migration logic

---

## Canonical Schema (v1)

### `engram_meta` — Schema version tracking

```sql
CREATE TABLE IF NOT EXISTS engram_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Seed: INSERT INTO engram_meta VALUES ('schema_version', '1');
```

### `memories` — Core memory store

```sql
CREATE TABLE IF NOT EXISTS memories (
    id                  TEXT PRIMARY KEY,        -- UUID v4
    content             TEXT NOT NULL,           -- The memory content
    memory_type         TEXT NOT NULL,           -- factual|episodic|relational|emotional|procedural|opinion
    layer               TEXT NOT NULL,           -- working|core|longterm|archived
    importance          REAL NOT NULL DEFAULT 0.3,
    working_strength    REAL NOT NULL DEFAULT 1.0,
    core_strength       REAL NOT NULL DEFAULT 0.0,
    pinned              INTEGER NOT NULL DEFAULT 0,
    consolidation_count INTEGER NOT NULL DEFAULT 0,
    source              TEXT DEFAULT '',         -- Who/what created this memory
    namespace           TEXT NOT NULL DEFAULT 'default',
    created_at          REAL NOT NULL,           -- Unix timestamp (float, seconds since epoch)
    last_consolidated   REAL,                    -- Unix timestamp
    metadata            TEXT,                    -- JSON blob for extensibility
    -- Reserved for future use, clients MUST ignore unknown columns
    contradicts         TEXT DEFAULT '',
    contradicted_by     TEXT DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
```

### `access_log` — ACT-R activation tracking

```sql
CREATE TABLE IF NOT EXISTS access_log (
    memory_id   TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    accessed_at REAL NOT NULL  -- Unix timestamp
);

CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
```

### `hebbian_links` — Association network

```sql
CREATE TABLE IF NOT EXISTS hebbian_links (
    source_id           TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id           TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    strength            REAL NOT NULL DEFAULT 1.0,
    coactivation_count  INTEGER NOT NULL DEFAULT 0,
    namespace           TEXT NOT NULL DEFAULT 'default',
    created_at          REAL NOT NULL,  -- Unix timestamp
    PRIMARY KEY (source_id, target_id)
);

CREATE INDEX IF NOT EXISTS idx_hebbian_source ON hebbian_links(source_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_target ON hebbian_links(target_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_namespace ON hebbian_links(namespace);
```

### `memory_embeddings` — Vector store (optional)

```sql
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    embedding TEXT NOT NULL,   -- JSON array of float64
    model     TEXT NOT NULL,   -- e.g. "nomic-embed-text", "text-embedding-3-small"
    dimension INTEGER NOT NULL -- e.g. 768, 1536
);
```

### `engram_acl` — Multi-agent access control

```sql
CREATE TABLE IF NOT EXISTS engram_acl (
    agent_id   TEXT NOT NULL,
    namespace  TEXT NOT NULL,
    permission TEXT NOT NULL,  -- read|write|admin
    granted_by TEXT NOT NULL,
    created_at REAL NOT NULL,  -- Unix timestamp
    PRIMARY KEY (agent_id, namespace)
);
```

### `memories_fts` — Full-text search

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content=memories,
    content_rowid=rowid
);

-- Sync triggers
CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
    INSERT INTO memories_fts(rowid, content) VALUES (new.rowid, new.content);
END;
```

---

## Key Decisions

### Timestamps: REAL (Unix float)

**Chosen**: `REAL` (Unix timestamp, seconds since epoch, float for sub-second precision)

**Why not ISO 8601 TEXT?**
- Every language has native float → datetime conversion
- Numeric comparison is faster than string comparison
- Sorting, range queries are natural (`WHERE created_at > 1711108800`)
- No timezone ambiguity (always UTC)
- Python's `time.time()` returns this directly

**Why not INTEGER?**
- Float gives sub-second precision without extra columns

### Column naming: `source` not `source_file`

`source` is more general — memories can come from agents, tools, conversations, not just files.

### `summary` and `tokens` columns: REMOVED

These were Python-specific implementation details for FTS optimization. The unified schema keeps FTS on `content` only. Implementations can store derived data in `metadata` JSON.

### Embeddings: separate table, optional

Not every client has an embedding model. Embeddings are stored in `memory_embeddings` with the model name, so different clients can use different models and know which embeddings are compatible.

### Forward compatibility

Clients MUST:
- Ignore unknown columns (don't fail on SELECT *)
- Ignore unknown tables
- Check `engram_meta.schema_version` and refuse to operate if major version is higher than supported

---

## Migration Plan

### Python (current DB: 889 memories)

```sql
-- 1. Add meta table
CREATE TABLE engram_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO engram_meta VALUES ('schema_version', '1');

-- 2. Rename source_file → source
ALTER TABLE memories RENAME COLUMN source_file TO source;

-- 3. Drop summary/tokens from FTS (rebuild triggers)
-- Keep summary/tokens columns for backward compat, just remove from FTS

-- 4. created_at is already REAL ✅
-- 5. access_log.accessed_at is already REAL ✅
```

### Rust

```rust
// Change all TEXT timestamps to REAL
// Change column reads to expect REAL for created_at, last_consolidated, accessed_at
// Add schema_version check on open
```

### TypeScript

```typescript
// Align with canonical schema
// Add schema_version check
```

---

## Cognitive Memory Architecture

This schema models a biologically-inspired memory system:

```
┌─────────────────────────────────────────────────────┐
│                   Agent (any language)                │
│                                                       │
│  ┌─────────┐   ┌──────────┐   ┌──────────────────┐  │
│  │ Recall  │──→│ ACT-R    │──→│ Working Memory   │  │
│  │ (query) │   │ Scoring  │   │ (7±2 chunks)     │  │
│  └─────────┘   └──────────┘   └──────────────────┘  │
│       ↑              ↑                                │
│       │              │                                │
│  ┌─────────┐   ┌──────────┐   ┌──────────────────┐  │
│  │  FTS5   │   │ Hebbian  │   │ Embeddings       │  │
│  │ Search  │   │ Links    │   │ (optional)       │  │
│  └─────────┘   └──────────┘   └──────────────────┘  │
│       ↑              ↑              ↑                 │
│       └──────────────┴──────────────┘                 │
│                      │                                │
│              ┌───────────────┐                        │
│              │   SQLite DB   │  ← THE universal       │
│              │  (engram.db)  │    interface            │
│              └───────────────┘                        │
└─────────────────────────────────────────────────────┘

Memory Lifecycle:
  Ingest → Working (high activation)
       → Consolidate → Core (medium, reinforced)
            → Consolidate → Long-term (stable, low decay)
                 → Forget → Archived (below threshold)

Scoring: activation = base_level × recency_decay + importance × weight + hebbian_boost
```

### Why SQLite as the protocol?

1. **Zero infrastructure** — no server, no network, just a file
2. **Atomic transactions** — concurrent reads, safe writes (WAL mode)
3. **Universal** — every language has SQLite bindings
4. **Portable** — copy the file to share all memories
5. **Inspectable** — `sqlite3 engram.db "SELECT * FROM memories"` just works
6. **Fast** — microsecond reads, sub-millisecond writes

### Memory Types (cognitive model)

| Type | Maps to | Example |
|------|---------|---------|
| `factual` | Semantic memory | "CLOB API is in London eu-west-2" |
| `episodic` | Episodic memory | "On 2026-03-21, bot hung for 8h due to RPC timeout" |
| `procedural` | Procedural memory | "Deploy with ./scripts/deploy.sh" |
| `relational` | Social memory | "potato prefers direct communication" |
| `emotional` | Emotional memory | "potato said 'I kinda like you'" |
| `opinion` | Belief system | "Latency arbitrage is not viable on Polymarket" |

### Memory Layers (strength model)

| Layer | Activation | Persistence |
|-------|-----------|-------------|
| `working` | High (1.0) | Decays in minutes |
| `core` | Medium (0.3-0.7) | Reinforced by use |
| `longterm` | Stable (0.1-0.3) | Low decay rate |
| `archived` | Near-zero | Below forget threshold |

---

## Entity & Relation Tables (for structured knowledge)

Engram stores flat text memories. But bots with LLM capabilities can extract entities and relationships
and store them in structured tables. This enables queries like "who does potato know?" or "what projects
use Rust?".

**These tables are optional** — bots without entity extraction still work fine with flat memories.

### `entities` — Named things

```sql
CREATE TABLE IF NOT EXISTS entities (
    id          TEXT PRIMARY KEY,        -- UUID v4
    name        TEXT NOT NULL,           -- "potato", "Polymarket", "RustClaw"
    entity_type TEXT NOT NULL,           -- person|project|tool|concept|place|org
    namespace   TEXT NOT NULL DEFAULT 'default',
    metadata    TEXT,                    -- JSON blob (attributes, aliases, etc.)
    created_at  REAL NOT NULL,           -- Unix timestamp
    updated_at  REAL NOT NULL            -- Unix timestamp
);

CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_namespace ON entities(namespace);
```

### `entity_relations` — How things connect

```sql
CREATE TABLE IF NOT EXISTS entity_relations (
    id          TEXT PRIMARY KEY,        -- UUID v4
    source_id   TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_id   TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relation    TEXT NOT NULL,           -- "created_by", "works_on", "married_to", "depends_on"
    confidence  REAL NOT NULL DEFAULT 1.0,
    source      TEXT DEFAULT '',         -- Which memory/conversation this came from
    namespace   TEXT NOT NULL DEFAULT 'default',
    created_at  REAL NOT NULL,           -- Unix timestamp
    metadata    TEXT                     -- JSON blob
);

CREATE INDEX IF NOT EXISTS idx_relations_source ON entity_relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON entity_relations(target_id);
CREATE INDEX IF NOT EXISTS idx_relations_type ON entity_relations(relation);
```

### `memory_entities` — Link memories to entities they mention

```sql
CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    role      TEXT DEFAULT 'mentioned',  -- mentioned|subject|object
    PRIMARY KEY (memory_id, entity_id)
);
```

### Usage Pattern

Bot's LLM extracts entities during conversation:
```
User: "potato's wife joined The Unusual project last week"

Bot LLM extracts:
  entities: [{name: "potato", type: "person"}, {name: "wife", type: "person"}, {name: "The Unusual", type: "project"}]
  relations: [{source: "wife", target: "potato", relation: "married_to"}, {source: "wife", target: "The Unusual", relation: "joined"}]

Bot calls:
  engram add "potato's wife joined The Unusual project" --type factual
  engram entity add "potato" --type person
  engram entity add "The Unusual" --type project
  engram entity link "wife" "The Unusual" --relation "joined"
```

Later query:
```
  engram entity query "The Unusual" --relations
  → wife (joined), potato (created_by)
```

## Engram's Role vs Bot's Role

| Responsibility | Engram | Bot (LLM) |
|---|---|---|
| Store memories | ✅ | |
| Recall with cognitive scoring | ✅ | |
| Consolidate/forget | ✅ | |
| Semantic search (embedding) | ✅ | |
| **Decide what to store** | | ✅ |
| **Extract entities/relations** | | ✅ |
| **Judge importance** | | ✅ |
| **Detect contradictions** | | ✅ |
| Provide APIs for all above | ✅ | |

**Engram is the memory. Bot is the brain. The brain decides what goes into memory.**

---

## Implementation Checklist

- [ ] Finalize schema spec (this doc)
- [ ] Write migration script for Python DB (889 memories)
- [ ] Update Python `store.py` to conform to v1 schema
- [ ] Update Rust `storage.rs` to conform to v1 schema  
- [ ] Update TS `store.ts` to conform to v1 schema
- [ ] Add `engram_meta` version check to all implementations
- [ ] Add schema validation test (create DB in Python, read in Rust, write in TS, verify all)
- [ ] Add embedding table support to Rust crate
- [ ] Document spec in repo README
- [ ] Publish updated packages (PyPI 1.2.0, crates.io, npm)

---

*This spec defines how AI agents remember. Get it right, and any bot in any language can share a cognitive memory. Get it wrong, and we have three incompatible implementations forever.*
