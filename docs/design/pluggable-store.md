# Pluggable Store Architecture

## Problem

Engram currently uses SQLite as the sole storage backend. This works for local-first use cases but not for serverless deployments (Vercel, Cloudflare Workers) where there's no persistent filesystem.

## Solution: Store Interface

Define a `Store` interface that all storage backends implement. Core logic (activation, forgetting, consolidation) only depends on the interface, never on a specific backend.

```typescript
interface Store {
  // CRUD
  add(content: string, type: MemoryType, options?: AddOptions): MemoryEntry
  get(id: string): MemoryEntry | null
  update(entry: MemoryEntry): void
  delete(id: string): void
  all(): MemoryEntry[]

  // Search
  searchFts(query: string, limit?: number): MemoryEntry[]
  
  // Access tracking
  recordAccess(id: string): void
  getAccessTimes(id: string): number[]

  // Graph
  addGraphLink(memoryId: string, nodeId: string, relation: string): void
  searchByEntity(entity: string): MemoryEntry[]
  getEntities(memoryId: string): [string, string][]
  getRelatedEntities(entity: string, hops?: number): string[]
  getAllEntities(): string[]

  // Lifecycle
  close(): void
  export(path: string): void
}
```

## Backends

### SQLiteStore (default)
- Local-first, zero dependencies
- FTS5 for full-text search
- Single `.db` file, portable
- Best for: CLI tools, local agents, development

### SupabaseStore (planned)
- For serverless deployments (Vercel, etc.)
- Uses existing Supabase project
- Tables mirror SQLite schema
- Best for: SaltyHall, web apps, cloud agents

### Future possibilities
- TursoStore — SQLite at the edge
- D1Store — Cloudflare Workers
- PostgresStore — generic Postgres
- InMemoryStore — testing/ephemeral

## Schema (shared across backends)

```sql
-- Core memories
CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  memory_type TEXT DEFAULT 'episodic',
  layer TEXT DEFAULT 'working',
  importance REAL DEFAULT 0.5,
  working_strength REAL DEFAULT 1.0,
  core_strength REAL DEFAULT 0.0,
  access_count INTEGER DEFAULT 0,
  consolidation_count INTEGER DEFAULT 0,
  created_at REAL,
  last_accessed REAL,
  last_consolidated REAL,
  source_file TEXT,
  context TEXT, -- JSON array
  pinned INTEGER DEFAULT 0,
  contradicts TEXT,
  contradicted_by TEXT
);

-- Access log for ACT-R activation
CREATE TABLE access_log (
  memory_id TEXT,
  accessed_at REAL
);

-- Entity graph
CREATE TABLE graph_links (
  memory_id TEXT,
  node_id TEXT,
  relation TEXT
);

-- FTS5 index (SQLite-specific, Supabase uses tsvector)
CREATE VIRTUAL TABLE memories_fts USING fts5(content, content=memories, content_rowid=rowid);
```

## Migration Path for SaltyHall

1. Create tables in Supabase matching the schema above
2. Implement SupabaseStore with @supabase/supabase-js
3. Replace agent-memory.ts with engram-ts Memory class using SupabaseStore
4. Each agent gets their memories in the same tables, filtered by agent_id (add agent_id column)
5. Supabase full-text search replaces FTS5
