"""
Supabase store backend for neuromemory-ai.

Requires: pip install supabase

Usage:
    from engram import Memory
    from engram.stores.supabase import SupabaseStore
    
    store = SupabaseStore(
        url="https://xxx.supabase.co",
        key="your-anon-key",
        user_id="user-123"
    )
    mem = Memory(store=store)
"""

from __future__ import annotations

import uuid
from datetime import datetime
from typing import Iterator, Optional, List, Any

try:
    from supabase import create_client, Client
    HAS_SUPABASE = True
except ImportError:
    HAS_SUPABASE = False
    Client = Any  # type: ignore

from engram.core import MemoryEntry, MemoryType, MemoryLayer


class SupabaseStore:
    """
    Supabase-backed store for neuromemory-ai.
    
    Implements the same interface as SQLiteStore but uses Supabase PostgreSQL.
    Supports multi-user isolation via user_id.
    """
    
    def __init__(
        self,
        url: str,
        key: str,
        user_id: str,
        table_prefix: str = "neuromemory_"
    ):
        """
        Initialize Supabase store.
        
        Args:
            url: Supabase project URL
            key: Supabase anon key (or service role key)
            user_id: User ID for row-level security
            table_prefix: Prefix for table names (default: "neuromemory_")
        """
        if not HAS_SUPABASE:
            raise ImportError(
                "Supabase client not installed. "
                "Install with: pip install supabase"
            )
        
        self.client: Client = create_client(url, key)
        self.user_id = user_id
        self.prefix = table_prefix
        
        # Table names
        self.memories_table = f"{table_prefix}memories"
        self.access_log_table = f"{table_prefix}access_log"
        self.graph_links_table = f"{table_prefix}graph_links"
        self.hebbian_links_table = f"{table_prefix}hebbian_links"
    
    def _row_to_entry(self, row: dict) -> MemoryEntry:
        """Convert Supabase row to MemoryEntry."""
        return MemoryEntry(
            id=row["id"],
            content=row["content"],
            summary=row.get("summary"),
            type=MemoryType(row["type"]),
            importance=row["importance"],
            source=row.get("source"),
            working_strength=row["working_strength"],
            core_strength=row["core_strength"],
            layer=MemoryLayer(row["layer"]),
            access_count=row["access_count"],
            last_access=row.get("last_access"),
            created_at=row["created_at"],
            pinned=row.get("pinned", False),
            tags=row.get("tags", []),
            contradicts=row.get("contradicts"),
            contradicted_by=row.get("contradicted_by"),
        )
    
    def _entry_to_row(self, entry: MemoryEntry) -> dict:
        """Convert MemoryEntry to Supabase row."""
        return {
            "id": entry.id,
            "user_id": self.user_id,
            "content": entry.content,
            "summary": entry.summary,
            "type": entry.type.value,
            "importance": entry.importance,
            "source": entry.source,
            "working_strength": entry.working_strength,
            "core_strength": entry.core_strength,
            "layer": entry.layer.value,
            "access_count": entry.access_count,
            "last_access": entry.last_access,
            "created_at": entry.created_at,
            "pinned": entry.pinned,
            "tags": entry.tags,
            "contradicts": entry.contradicts,
            "contradicted_by": entry.contradicted_by,
        }
    
    def add(self, entry: MemoryEntry) -> str:
        """Add a memory entry."""
        row = self._entry_to_row(entry)
        self.client.table(self.memories_table).insert(row).execute()
        return entry.id
    
    def get(self, memory_id: str) -> Optional[MemoryEntry]:
        """Get a memory by ID."""
        result = (
            self.client.table(self.memories_table)
            .select("*")
            .eq("id", memory_id)
            .eq("user_id", self.user_id)
            .single()
            .execute()
        )
        if result.data:
            return self._row_to_entry(result.data)
        return None
    
    def update(self, memory_id: str, **fields) -> None:
        """Update memory fields."""
        # Convert enum values to strings
        update_data = {}
        for key, value in fields.items():
            if isinstance(value, (MemoryType, MemoryLayer)):
                update_data[key] = value.value
            else:
                update_data[key] = value
        
        (
            self.client.table(self.memories_table)
            .update(update_data)
            .eq("id", memory_id)
            .eq("user_id", self.user_id)
            .execute()
        )
    
    def delete(self, memory_id: str) -> None:
        """Delete a memory."""
        (
            self.client.table(self.memories_table)
            .delete()
            .eq("id", memory_id)
            .eq("user_id", self.user_id)
            .execute()
        )
    
    def all(self) -> Iterator[MemoryEntry]:
        """Iterate over all memories for this user."""
        result = (
            self.client.table(self.memories_table)
            .select("*")
            .eq("user_id", self.user_id)
            .execute()
        )
        for row in result.data:
            yield self._row_to_entry(row)
    
    def search(self, query: str, limit: int = 10) -> List[MemoryEntry]:
        """Full-text search using PostgreSQL tsvector."""
        # Use Supabase's textSearch capability
        result = (
            self.client.table(self.memories_table)
            .select("*")
            .eq("user_id", self.user_id)
            .text_search("fts", query)
            .limit(limit)
            .execute()
        )
        return [self._row_to_entry(row) for row in result.data]
    
    def log_access(self, memory_id: str) -> None:
        """Log a memory access for ACT-R activation calculation."""
        self.client.table(self.access_log_table).insert({
            "memory_id": memory_id,
            "accessed_at": datetime.utcnow().isoformat()
        }).execute()
    
    def get_access_times(self, memory_id: str) -> List[float]:
        """Get all access times for a memory (as Unix timestamps)."""
        result = (
            self.client.table(self.access_log_table)
            .select("accessed_at")
            .eq("memory_id", memory_id)
            .execute()
        )
        times = []
        for row in result.data:
            dt = datetime.fromisoformat(row["accessed_at"].replace("Z", "+00:00"))
            times.append(dt.timestamp())
        return times
    
    def add_graph_link(self, memory_id: str, node_id: str, relation: str = "") -> None:
        """Add a graph link (entity connection)."""
        self.client.table(self.graph_links_table).insert({
            "memory_id": memory_id,
            "node_id": node_id,
            "relation": relation
        }).execute()
    
    def get_graph_links(self, memory_id: str) -> List[tuple]:
        """Get all graph links for a memory."""
        result = (
            self.client.table(self.graph_links_table)
            .select("node_id, relation")
            .eq("memory_id", memory_id)
            .execute()
        )
        return [(row["node_id"], row["relation"]) for row in result.data]
    
    def get_memories_by_entity(self, node_id: str) -> List[str]:
        """Get all memory IDs linked to an entity."""
        result = (
            self.client.table(self.graph_links_table)
            .select("memory_id")
            .eq("node_id", node_id)
            .execute()
        )
        return [row["memory_id"] for row in result.data]
    
    # Hebbian methods
    def get_hebbian_link(
        self, source_id: str, target_id: str
    ) -> Optional[dict]:
        """Get Hebbian link between two memories."""
        result = (
            self.client.table(self.hebbian_links_table)
            .select("*")
            .eq("source_id", source_id)
            .eq("target_id", target_id)
            .single()
            .execute()
        )
        if result.data:
            return {
                "strength": result.data["strength"],
                "coactivation_count": result.data["coactivation_count"],
                "created_at": result.data["created_at"]
            }
        return None
    
    def upsert_hebbian_link(
        self,
        source_id: str,
        target_id: str,
        strength: float,
        coactivation_count: int
    ) -> None:
        """Insert or update a Hebbian link."""
        self.client.table(self.hebbian_links_table).upsert({
            "source_id": source_id,
            "target_id": target_id,
            "strength": strength,
            "coactivation_count": coactivation_count,
            "created_at": datetime.utcnow().isoformat()
        }).execute()
    
    def get_hebbian_neighbors(self, memory_id: str) -> List[str]:
        """Get all memories linked via Hebbian learning."""
        result = (
            self.client.table(self.hebbian_links_table)
            .select("target_id")
            .eq("source_id", memory_id)
            .gt("strength", 0.5)  # Only strong links
            .execute()
        )
        return [row["target_id"] for row in result.data]
    
    def decay_hebbian_links(self, factor: float) -> int:
        """Decay all Hebbian link strengths."""
        # PostgreSQL: UPDATE ... SET strength = strength * factor
        # Supabase doesn't support this directly, need RPC
        # For now, fetch all and update
        result = (
            self.client.table(self.hebbian_links_table)
            .select("source_id, target_id, strength")
            .execute()
        )
        count = 0
        for row in result.data:
            new_strength = row["strength"] * factor
            if new_strength < 0.01:
                # Delete weak links
                (
                    self.client.table(self.hebbian_links_table)
                    .delete()
                    .eq("source_id", row["source_id"])
                    .eq("target_id", row["target_id"])
                    .execute()
                )
            else:
                (
                    self.client.table(self.hebbian_links_table)
                    .update({"strength": new_strength})
                    .eq("source_id", row["source_id"])
                    .eq("target_id", row["target_id"])
                    .execute()
                )
            count += 1
        return count
    
    def close(self) -> None:
        """Close connection (no-op for Supabase)."""
        pass


# SQL Migration for Supabase
SUPABASE_SCHEMA = """
-- neuromemory-ai Supabase Schema
-- Run this in Supabase SQL Editor

-- Memory types enum
CREATE TYPE neuromemory_type AS ENUM (
    'factual', 'episodic', 'relational', 'emotional', 'procedural', 'opinion'
);

-- Memory layers enum  
CREATE TYPE neuromemory_layer AS ENUM (
    'working', 'core', 'archive'
);

-- Main memories table
CREATE TABLE neuromemory_memories (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES auth.users NOT NULL,
    content TEXT NOT NULL,
    summary TEXT,
    type neuromemory_type DEFAULT 'factual',
    importance REAL DEFAULT 0.5,
    source TEXT,
    working_strength REAL DEFAULT 1.0,
    core_strength REAL DEFAULT 0.0,
    layer neuromemory_layer DEFAULT 'working',
    access_count INTEGER DEFAULT 0,
    last_access TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    pinned BOOLEAN DEFAULT FALSE,
    tags TEXT[] DEFAULT '{}',
    contradicts UUID REFERENCES neuromemory_memories(id),
    contradicted_by UUID REFERENCES neuromemory_memories(id)
);

-- Full-text search
ALTER TABLE neuromemory_memories ADD COLUMN fts tsvector 
    GENERATED ALWAYS AS (
        to_tsvector('english', content || ' ' || COALESCE(summary, ''))
    ) STORED;
CREATE INDEX neuromemory_memories_fts_idx ON neuromemory_memories USING GIN(fts);

-- Row Level Security
ALTER TABLE neuromemory_memories ENABLE ROW LEVEL SECURITY;
CREATE POLICY "Users access own memories" ON neuromemory_memories
    FOR ALL USING (auth.uid() = user_id);

-- Access log for ACT-R
CREATE TABLE neuromemory_access_log (
    id BIGSERIAL PRIMARY KEY,
    memory_id UUID REFERENCES neuromemory_memories(id) ON DELETE CASCADE,
    accessed_at TIMESTAMPTZ DEFAULT NOW()
);

-- Graph links (entity connections)
CREATE TABLE neuromemory_graph_links (
    memory_id UUID REFERENCES neuromemory_memories(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    relation TEXT DEFAULT ''
);

-- Hebbian links (co-activation connections)
CREATE TABLE neuromemory_hebbian_links (
    source_id UUID REFERENCES neuromemory_memories(id) ON DELETE CASCADE,
    target_id UUID REFERENCES neuromemory_memories(id) ON DELETE CASCADE,
    strength REAL DEFAULT 1.0,
    coactivation_count INTEGER DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (source_id, target_id)
);

-- Indexes
CREATE INDEX idx_neuromemory_memories_user ON neuromemory_memories(user_id);
CREATE INDEX idx_neuromemory_memories_layer ON neuromemory_memories(user_id, layer);
CREATE INDEX idx_neuromemory_access_log_mid ON neuromemory_access_log(memory_id);
CREATE INDEX idx_neuromemory_graph_links_mid ON neuromemory_graph_links(memory_id);
CREATE INDEX idx_neuromemory_graph_links_nid ON neuromemory_graph_links(node_id);
CREATE INDEX idx_neuromemory_hebbian_source ON neuromemory_hebbian_links(source_id);
CREATE INDEX idx_neuromemory_hebbian_target ON neuromemory_hebbian_links(target_id);
"""
