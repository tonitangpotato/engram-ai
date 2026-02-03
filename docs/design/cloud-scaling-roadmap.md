# Cloud Scaling Roadmap

> Migration path from local SQLite → Supabase → Cloudflare Edge

## Current State: Local SQLite

```
┌─────────────────┐
│  engramai       │
│  (Python/TS)    │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  SQLite + FTS5  │
│  (local file)   │
└─────────────────┘
```

**Pros:** Zero config, zero deps, works offline  
**Cons:** Single device, no sync, no multi-user

---

## Phase 1: Supabase Backend

**Target:** Multi-device sync, multi-user support  
**Scale:** Up to ~100K users

```
┌─────────────────┐
│  engramai       │
│  (Python/TS)    │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  SupabaseStore  │  ← New pluggable backend
└────────┬────────┘
         │
         ▼
┌─────────────────────────────────────────┐
│  Supabase                               │
│  ├── PostgreSQL (memories, links)       │
│  ├── tsvector (full-text search)        │
│  ├── pgvector (optional embeddings)     │
│  ├── Auth (user management)             │
│  ├── RLS (row-level security)           │
│  └── Realtime (sync subscriptions)      │
└─────────────────────────────────────────┘
         │
         ▼
┌─────────────────┐
│  Multi-device   │
│  sync           │
└─────────────────┘
```

### Schema (PostgreSQL)

```sql
-- Core memories table
CREATE TABLE memories (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id UUID REFERENCES auth.users NOT NULL,
  content TEXT NOT NULL,
  summary TEXT,
  type memory_type DEFAULT 'factual',
  importance REAL DEFAULT 0.5,
  working_strength REAL DEFAULT 1.0,
  core_strength REAL DEFAULT 0.0,
  layer memory_layer DEFAULT 'working',
  access_count INTEGER DEFAULT 0,
  pinned BOOLEAN DEFAULT FALSE,
  contradicts UUID REFERENCES memories(id),
  contradicted_by UUID REFERENCES memories(id),
  created_at TIMESTAMPTZ DEFAULT NOW(),
  updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Full-text search
ALTER TABLE memories ADD COLUMN fts tsvector 
  GENERATED ALWAYS AS (to_tsvector('english', content || ' ' || COALESCE(summary, ''))) STORED;
CREATE INDEX memories_fts_idx ON memories USING GIN(fts);

-- Row-level security (multi-tenant)
ALTER TABLE memories ENABLE ROW LEVEL SECURITY;
CREATE POLICY "Users can only access own memories"
  ON memories FOR ALL USING (auth.uid() = user_id);

-- Access log for ACT-R
CREATE TABLE access_log (
  id BIGSERIAL PRIMARY KEY,
  memory_id UUID REFERENCES memories(id) ON DELETE CASCADE,
  accessed_at TIMESTAMPTZ DEFAULT NOW()
);

-- Entity graph links
CREATE TABLE graph_links (
  memory_id UUID REFERENCES memories(id) ON DELETE CASCADE,
  node_id TEXT NOT NULL,
  relation TEXT DEFAULT ''
);

-- Hebbian links
CREATE TABLE hebbian_links (
  source_id UUID REFERENCES memories(id) ON DELETE CASCADE,
  target_id UUID REFERENCES memories(id) ON DELETE CASCADE,
  strength REAL DEFAULT 1.0,
  coactivation_count INTEGER DEFAULT 0,
  created_at TIMESTAMPTZ DEFAULT NOW(),
  PRIMARY KEY (source_id, target_id)
);

-- Indexes
CREATE INDEX idx_memories_user ON memories(user_id);
CREATE INDEX idx_memories_layer ON memories(user_id, layer);
CREATE INDEX idx_access_log_mid ON access_log(memory_id);
CREATE INDEX idx_graph_links_mid ON graph_links(memory_id);
CREATE INDEX idx_hebbian_source ON hebbian_links(source_id);
CREATE INDEX idx_hebbian_target ON hebbian_links(target_id);
```

### Why Supabase First

| Aspect | Benefit |
|--------|---------|
| PostgreSQL | More powerful than D1 (SQLite) |
| Auth | Built-in, no need to implement |
| RLS | Multi-tenant security out of the box |
| Realtime | Sync subscriptions built-in |
| Ecosystem | SaltyHall already uses Supabase |
| Dev speed | Faster to implement |
| Scale | Handles ~100K users easily |

---

## Phase 2: Cloudflare Edge Cache (Optional)

**Target:** Global low-latency reads  
**Scale:** 100K+ users, global distribution

```
┌─────────────────┐
│  engramai       │
└────────┬────────┘
         │
         ▼
┌─────────────────────────────────────────────────────┐
│  Cloudflare Edge (300+ cities)                      │
│  ├── Workers (API routing)                          │
│  ├── KV (hot memory cache)                          │
│  └── Read replica (cache Supabase queries)          │
└────────────────────────┬────────────────────────────┘
                         │ cache miss
                         ▼
┌─────────────────────────────────────────┐
│  Supabase (source of truth)             │
│  └── PostgreSQL (writes + cold reads)   │
└─────────────────────────────────────────┘
```

### When to Add Cloudflare

- Global users complaining about latency (>200ms)
- Read-heavy workload (>90% reads)
- Scale beyond Supabase's single-region limits
- Cost optimization at high volume

### Architecture

1. **Write path:** Client → Supabase (direct)
2. **Read path:** Client → Cloudflare Edge → Cache hit? Return : Fetch from Supabase → Cache → Return
3. **Invalidation:** Supabase webhook → Cloudflare purge

---

## Phase 3: Full Cloudflare (If Needed)

**Target:** Maximum scale, minimum latency  
**Scale:** Millions of users

```
┌─────────────────┐
│  engramai       │
└────────┬────────┘
         │
         ▼
┌─────────────────────────────────────────────────────┐
│  Cloudflare Full Stack                              │
│  ├── Workers (API)                                  │
│  ├── D1 (SQLite at edge)                            │
│  ├── Vectorize (vector search)                      │
│  ├── Workers AI (optional: embeddings)              │
│  └── Durable Objects (real-time sync)               │
└─────────────────────────────────────────────────────┘
```

### When to Consider Full Migration

- Supabase costs become prohibitive
- Need <50ms latency globally
- Millions of QPS
- Edge-first use cases (IoT, robots)

### Trade-offs

| Gain | Lose |
|------|------|
| Global <50ms latency | PostgreSQL power (D1 is SQLite) |
| Edge-native | Supabase Realtime ease |
| Cost efficiency at scale | Development simplicity |

---

## Comparison: Cloudflare vs Supabase

| Aspect | Cloudflare | Supabase |
|--------|------------|----------|
| Architecture | Edge (300+ cities) | Single region (or enterprise multi) |
| Latency | <50ms global | Higher for distant users |
| Database | D1 (SQLite) | PostgreSQL |
| Vector search | Vectorize | pgvector |
| Real-time sync | DIY (Durable Objects) | ✅ Built-in Realtime |
| Auth | DIY | ✅ Built-in Auth |
| Pricing | Per-request (cheap at scale) | Storage + compute |
| Best for | Read-heavy, global, massive scale | Complex queries, dev speed, <100K users |

---

## Recommended Path

```
NOW           3 MONTHS        1 YEAR           SCALE
 │               │               │               │
 ▼               ▼               ▼               ▼
SQLite    →   Supabase    →   Supabase     →   Cloudflare
(local)       (cloud)         + CF cache       (full)
                              (if needed)      (if needed)
```

**Decision points:**
1. **Need multi-device?** → Add Supabase
2. **Global latency issues?** → Add Cloudflare cache
3. **Supabase too expensive/slow?** → Migrate to full Cloudflare

---

## Implementation Notes

### Store Interface (Already Designed)

```python
class Store(Protocol):
    def add(self, entry: MemoryEntry) -> str: ...
    def get(self, memory_id: str) -> MemoryEntry | None: ...
    def update(self, memory_id: str, **fields) -> None: ...
    def delete(self, memory_id: str) -> None: ...
    def search(self, query: str, limit: int) -> list[MemoryEntry]: ...
    def all(self) -> Iterator[MemoryEntry]: ...
```

All backends implement this interface. Switching is one line:

```python
# Local
mem = Memory("./local.db")

# Supabase
mem = Memory(store=SupabaseStore(url, key, user_id))

# Cloudflare
mem = Memory(store=CloudflareStore(worker_url, api_key))
```

### Files to Create

1. `engram/stores/supabase.py` — SupabaseStore implementation
2. `engram/stores/cloudflare.py` — CloudflareStore implementation (later)
3. `migrations/supabase/` — SQL migrations for Supabase

---

*Document created: 2026-02-03*  
*Reference: shodh-cloudflare architecture*
