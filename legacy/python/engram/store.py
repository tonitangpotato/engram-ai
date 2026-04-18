"""
SQLite-backed memory store for Engram.
Replaces the in-memory dict-based MemoryStore with persistent storage.
"""

import json
import sqlite3
import shutil
import time
import uuid
from typing import Optional

# TODO: import from engram.core once package is finalized
import sys, os
from engram.core import MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE


_SCHEMA = """
CREATE TABLE IF NOT EXISTS engram_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    summary TEXT DEFAULT '',
    tokens TEXT DEFAULT '',
    memory_type TEXT NOT NULL,
    layer TEXT NOT NULL,
    created_at REAL NOT NULL,
    working_strength REAL NOT NULL DEFAULT 1.0,
    core_strength REAL NOT NULL DEFAULT 0.0,
    importance REAL NOT NULL DEFAULT 0.3,
    pinned INTEGER NOT NULL DEFAULT 0,
    consolidation_count INTEGER NOT NULL DEFAULT 0,
    last_consolidated REAL,
    source TEXT DEFAULT '',
    contradicts TEXT DEFAULT '',
    contradicted_by TEXT DEFAULT '',
    namespace TEXT NOT NULL DEFAULT 'default',
    metadata TEXT
);

CREATE TABLE IF NOT EXISTS access_log (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    accessed_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS graph_links (
    memory_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    relation TEXT DEFAULT ''
);

CREATE TABLE IF NOT EXISTS hebbian_links (
    source_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    target_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    strength REAL NOT NULL DEFAULT 1.0,
    coactivation_count INTEGER NOT NULL DEFAULT 0,
    namespace TEXT NOT NULL DEFAULT 'default',
    created_at REAL NOT NULL,
    PRIMARY KEY (source_id, target_id)
);

CREATE TABLE IF NOT EXISTS entities (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    namespace   TEXT NOT NULL DEFAULT 'default',
    metadata    TEXT,
    created_at  REAL NOT NULL,
    updated_at  REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS entity_relations (
    id          TEXT PRIMARY KEY,
    source_id   TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_id   TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relation    TEXT NOT NULL,
    confidence  REAL NOT NULL DEFAULT 1.0,
    source      TEXT DEFAULT '',
    namespace   TEXT NOT NULL DEFAULT 'default',
    created_at  REAL NOT NULL,
    metadata    TEXT
);

CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    role      TEXT DEFAULT 'mentioned',
    PRIMARY KEY (memory_id, entity_id)
);

CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    embedding TEXT NOT NULL,
    model     TEXT NOT NULL,
    dimension INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS engram_acl (
    agent_id   TEXT NOT NULL,
    namespace  TEXT NOT NULL,
    permission TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (agent_id, namespace)
);

CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_mid ON graph_links(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_nid ON graph_links(node_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_source ON hebbian_links(source_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_target ON hebbian_links(target_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_namespace ON hebbian_links(namespace);
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_namespace ON entities(namespace);
CREATE INDEX IF NOT EXISTS idx_relations_source ON entity_relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON entity_relations(target_id);
CREATE INDEX IF NOT EXISTS idx_relations_type ON entity_relations(relation);
"""

_FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content=memories,
    content_rowid=rowid
);
"""

# NOTE: FTS triggers removed - we now manually manage FTS insertions
# with tokenized content for CJK support (like Rust implementation).
# This allows us to preprocess content with jieba before indexing.


def _row_to_entry(row: sqlite3.Row, access_times: list[float] | None = None) -> MemoryEntry:
    # Parse metadata JSON if present
    metadata_raw = row["metadata"] if "metadata" in row.keys() else None
    metadata = None
    if metadata_raw:
        try:
            metadata = json.loads(metadata_raw)
        except (json.JSONDecodeError, TypeError):
            metadata = None

    return MemoryEntry(
        id=row["id"],
        content=row["content"],
        summary=row["summary"] or "",
        memory_type=MemoryType(row["memory_type"]),
        layer=MemoryLayer(row["layer"]),
        created_at=row["created_at"],
        access_times=access_times if access_times is not None else [],
        working_strength=row["working_strength"],
        core_strength=row["core_strength"],
        importance=row["importance"],
        pinned=bool(row["pinned"]),
        consolidation_count=row["consolidation_count"],
        last_consolidated=row["last_consolidated"],
        source_file=row["source"] or "",
        contradicts=row["contradicts"] or "",
        contradicted_by=row["contradicted_by"] or "",
        metadata=metadata,
    )


class SQLiteStore:
    """Persistent SQLite-backed memory store with FTS5 search."""

    def __init__(self, db_path: str = ":memory:"):
        self.db_path = db_path
        self._conn = sqlite3.connect(db_path)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._conn.executescript(_SCHEMA)
        self._seed_meta()
        self._migrate_contradiction_columns()
        self._migrate_stdp_columns()
        self._conn.executescript(_FTS_SCHEMA)
        self._drop_fts_triggers()  # Remove old triggers if they exist
        self._rebuild_fts_if_needed()  # Re-tokenize FTS for CJK support
        self._conn.commit()

    def _drop_fts_triggers(self):
        """Drop FTS triggers if they exist (we now manage FTS manually for CJK tokenization)."""
        self._conn.execute("DROP TRIGGER IF EXISTS memories_fts_ai")
        self._conn.execute("DROP TRIGGER IF EXISTS memories_fts_ad")
        self._conn.execute("DROP TRIGGER IF EXISTS memories_fts_au")

    # Current FTS tokenization version — bump when tokenization logic changes
    _FTS_CJK_VERSION = "1"

    def _rebuild_fts_if_needed(self):
        """Rebuild FTS index with CJK tokenization if not already done.
        
        Uses engram_meta 'fts_cjk_version' to track whether the FTS index
        has been built with CJK-aware tokenization. On first run after the
        CJK tokenization feature is added, this rebuilds the entire FTS index
        so old memories get proper tokenization too.
        """
        row = self._conn.execute(
            "SELECT value FROM engram_meta WHERE key = 'fts_cjk_version'"
        ).fetchone()
        current_version = row[0] if row else None

        if current_version == self._FTS_CJK_VERSION:
            return  # Already up to date

        from engram.engram_tokenizers import contains_cjk, tokenize_for_fts

        # Count memories to decide whether to rebuild
        count = self._conn.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
        if count == 0:
            # Empty DB, just mark as done
            self._conn.execute(
                "INSERT OR REPLACE INTO engram_meta VALUES ('fts_cjk_version', ?)",
                (self._FTS_CJK_VERSION,),
            )
            return

        # Rebuild: clear FTS and re-insert all with tokenization
        self._conn.execute("DELETE FROM memories_fts")
        
        rows = self._conn.execute(
            "SELECT rowid, content FROM memories"
        ).fetchall()
        
        for row in rows:
            content = row[1] or ""
            fts_content = tokenize_for_fts(content) if contains_cjk(content) else content
            self._conn.execute(
                "INSERT INTO memories_fts(rowid, content) VALUES (?, ?)",
                (row[0], fts_content),
            )

        # Mark migration complete
        self._conn.execute(
            "INSERT OR REPLACE INTO engram_meta VALUES ('fts_cjk_version', ?)",
            (self._FTS_CJK_VERSION,),
        )
        
        import logging
        logging.getLogger("engram").info(
            f"Rebuilt FTS index with CJK tokenization for {len(rows)} memories"
        )

    def _seed_meta(self):
        """Ensure engram_meta has schema_version set for new databases."""
        row = self._conn.execute(
            "SELECT value FROM engram_meta WHERE key = 'schema_version'"
        ).fetchone()
        if row is None:
            self._conn.execute(
                "INSERT INTO engram_meta VALUES ('schema_version', '1')"
            )

    def _migrate_contradiction_columns(self):
        """Add contradiction columns if they don't exist (migration for older DBs)."""
        cursor = self._conn.execute("PRAGMA table_info(memories)")
        columns = {row[1] for row in cursor.fetchall()}
        if "contradicts" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN contradicts TEXT DEFAULT ''")
        if "contradicted_by" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN contradicted_by TEXT DEFAULT ''")
        # Phase 1: metadata column for structured data (causal memories etc.)
        if "metadata" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN metadata TEXT")
        # Phase 2: namespace column for multi-agent shared memory
        if "namespace" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN namespace TEXT NOT NULL DEFAULT 'default'")
        # Index on memory_type for type-filtered queries (e.g. recall_causal)
        self._conn.execute("CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type)")
        self._conn.execute("CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace)")

    def _migrate_stdp_columns(self):
        """Add STDP temporal tracking columns to hebbian_links (migration for older DBs)."""
        cursor = self._conn.execute("PRAGMA table_info(hebbian_links)")
        columns = {row[1] for row in cursor.fetchall()}
        if "direction" not in columns:
            self._conn.execute(
                "ALTER TABLE hebbian_links ADD COLUMN direction TEXT DEFAULT 'bidirectional'"
            )
        if "temporal_forward" not in columns:
            self._conn.execute(
                "ALTER TABLE hebbian_links ADD COLUMN temporal_forward INTEGER DEFAULT 0"
            )
        if "temporal_backward" not in columns:
            self._conn.execute(
                "ALTER TABLE hebbian_links ADD COLUMN temporal_backward INTEGER DEFAULT 0"
            )

    def add(self, content: str, memory_type: MemoryType = MemoryType.FACTUAL,
            importance: Optional[float] = None, source_file: str = "",
            created_at: Optional[float] = None,
            metadata: Optional[dict] = None,
            namespace: str = "default",
            source: str = None) -> MemoryEntry:
        # Support both source and source_file (backward compat)
        actual_source = source if source is not None else source_file
        entry = MemoryEntry(
            content=content,
            memory_type=memory_type,
            importance=importance if importance is not None else DEFAULT_IMPORTANCE[memory_type],
            working_strength=1.0,
            core_strength=0.0,
            source_file=actual_source,
            metadata=metadata,
        )
        # Override created_at if provided (for temporal simulation)
        if created_at is not None:
            entry.created_at = created_at
        
        # Generate tokens for CJK content
        from engram.engram_tokenizers import contains_cjk, tokenize_for_fts
        tokens = tokenize_for_fts(content) if contains_cjk(content) else ""
        
        # Serialize metadata to JSON
        metadata_json = json.dumps(metadata) if metadata is not None else None
        
        self._conn.execute(
            """INSERT INTO memories (id, content, summary, tokens, memory_type, layer, created_at,
               working_strength, core_strength, importance, pinned, consolidation_count,
               last_consolidated, source, contradicts, contradicted_by, metadata, namespace)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (entry.id, entry.content, entry.summary, tokens, entry.memory_type.value,
             entry.layer.value, entry.created_at, entry.working_strength,
             entry.core_strength, entry.importance, int(entry.pinned),
             entry.consolidation_count, entry.last_consolidated, entry.source_file,
             entry.contradicts, entry.contradicted_by, metadata_json, namespace),
        )
        
        # Get rowid for FTS insertion
        rowid = self._conn.execute("SELECT rowid FROM memories WHERE id=?", (entry.id,)).fetchone()[0]
        
        # Insert into FTS with tokenized content (CJK support)
        # Use tokenized version if CJK present, otherwise use original content
        fts_content = tokenize_for_fts(content) if contains_cjk(content) else content
        self._conn.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?,?)",
            (rowid, fts_content),
        )
        
        # Record initial access
        self._conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?,?)",
            (entry.id, entry.created_at),
        )
        self._conn.commit()
        entry.access_times = [entry.created_at]
        return entry

    def get(self, memory_id: str) -> Optional[MemoryEntry]:
        row = self._conn.execute("SELECT * FROM memories WHERE id=?", (memory_id,)).fetchone()
        if row is None:
            return None
        self.record_access(memory_id)
        access_times = self.get_access_times(memory_id)
        return _row_to_entry(row, access_times)

    def all(self) -> list[MemoryEntry]:
        rows = self._conn.execute("SELECT * FROM memories").fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def update(self, entry: MemoryEntry):
        from engram.engram_tokenizers import contains_cjk, tokenize_for_fts
        
        metadata_json = json.dumps(entry.metadata) if entry.metadata is not None else None
        
        # Get rowid before update
        row = self._conn.execute("SELECT rowid FROM memories WHERE id=?", (entry.id,)).fetchone()
        if row is None:
            return  # Entry not found
        rowid = row[0]
        
        self._conn.execute(
            """UPDATE memories SET content=?, summary=?, memory_type=?, layer=?,
               working_strength=?, core_strength=?, importance=?, pinned=?,
               consolidation_count=?, last_consolidated=?, source=?,
               contradicts=?, contradicted_by=?, metadata=?
               WHERE id=?""",
            (entry.content, entry.summary, entry.memory_type.value, entry.layer.value,
             entry.working_strength, entry.core_strength, entry.importance,
             int(entry.pinned), entry.consolidation_count, entry.last_consolidated,
             entry.source_file, entry.contradicts, entry.contradicted_by,
             metadata_json, entry.id),
        )
        
        # Update FTS index with tokenized content
        fts_content = tokenize_for_fts(entry.content) if contains_cjk(entry.content) else entry.content
        self._conn.execute(
            "INSERT INTO memories_fts(memories_fts, rowid, content) VALUES ('delete', ?, ?)",
            (rowid, fts_content),  # Note: for delete, content doesn't matter but we include it
        )
        self._conn.execute(
            "INSERT INTO memories_fts(rowid, content) VALUES (?,?)",
            (rowid, fts_content),
        )
        
        self._conn.commit()

    def search_fts(self, query: str, limit: int = 20) -> list[MemoryEntry]:
        from engram.engram_tokenizers import contains_cjk, tokenize_for_fts
        
        # Tokenize CJK queries for better matching
        if contains_cjk(query):
            tokens = tokenize_for_fts(query).split()
            # Filter out empty tokens and single-char punctuation
            tokens = [t for t in tokens if len(t) > 0 and not (len(t) == 1 and not t.isalnum())]
            if tokens:
                # Use OR to match ANY token (more intuitive for semantic search)
                # Escape special FTS5 chars and quote tokens
                safe_tokens = [f'"{t}"' for t in tokens]
                query = " OR ".join(safe_tokens)
            else:
                query = query  # fallback to original
        
        rows = self._conn.execute(
            """SELECT m.* FROM memories m
               JOIN memories_fts f ON m.rowid = f.rowid
               WHERE memories_fts MATCH ?
               ORDER BY rank LIMIT ?""",
            (query, limit),
        ).fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def search_by_type(self, memory_type: MemoryType) -> list[MemoryEntry]:
        rows = self._conn.execute(
            "SELECT * FROM memories WHERE memory_type=?", (memory_type.value,)
        ).fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def search_causal(self, limit: int = 20) -> list[MemoryEntry]:
        """Get all causal memories, ordered by importance descending."""
        rows = self._conn.execute(
            "SELECT * FROM memories WHERE memory_type=? ORDER BY importance DESC LIMIT ?",
            (MemoryType.CAUSAL.value, limit),
        ).fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def search_by_layer(self, layer: MemoryLayer) -> list[MemoryEntry]:
        rows = self._conn.execute(
            "SELECT * FROM memories WHERE layer=?", (layer.value,)
        ).fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def get_access_times(self, memory_id: str) -> list[float]:
        rows = self._conn.execute(
            "SELECT accessed_at FROM access_log WHERE memory_id=? ORDER BY accessed_at",
            (memory_id,),
        ).fetchall()
        return [r["accessed_at"] for r in rows]

    def record_access(self, memory_id: str):
        self._conn.execute(
            "INSERT INTO access_log (memory_id, accessed_at) VALUES (?,?)",
            (memory_id, time.time()),
        )
        self._conn.commit()

    def delete(self, memory_id: str):
        # Get rowid and content for FTS deletion before deleting from main table
        row = self._conn.execute(
            "SELECT rowid, content FROM memories WHERE id=?", (memory_id,)
        ).fetchone()
        
        if row is not None:
            rowid, content = row["rowid"], row["content"]
            
            # Delete from FTS index
            from engram.engram_tokenizers import contains_cjk, tokenize_for_fts
            fts_content = tokenize_for_fts(content) if contains_cjk(content) else content
            self._conn.execute(
                "INSERT INTO memories_fts(memories_fts, rowid, content) VALUES ('delete', ?, ?)",
                (rowid, fts_content),
            )
        
        # Delete from main table
        self._conn.execute("DELETE FROM memories WHERE id=?", (memory_id,))
        self._conn.commit()

    def export(self, path: str):
        """Copy database to path. For in-memory DBs, use backup API."""
        if self.db_path == ":memory:":
            dst = sqlite3.connect(path)
            self._conn.backup(dst)
            dst.close()
        else:
            self._conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
            shutil.copy2(self.db_path, path)

    def stats(self) -> dict:
        total = self._conn.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
        by_type = {}
        for row in self._conn.execute("SELECT memory_type, COUNT(*) as c FROM memories GROUP BY memory_type"):
            by_type[row["memory_type"]] = row["c"]
        by_layer = {}
        for row in self._conn.execute("SELECT layer, COUNT(*) as c FROM memories GROUP BY layer"):
            by_layer[row["layer"]] = row["c"]
        access_count = self._conn.execute("SELECT COUNT(*) FROM access_log").fetchone()[0]
        return {
            "total_memories": total,
            "by_type": by_type,
            "by_layer": by_layer,
            "total_accesses": access_count,
        }

    # ── Graph link methods ──────────────────────────────────────

    def add_graph_link(self, memory_id: str, entity: str, relation: str = ""):
        """Link a memory to an entity node."""
        self._conn.execute(
            "INSERT INTO graph_links (memory_id, node_id, relation) VALUES (?,?,?)",
            (memory_id, entity, relation),
        )
        self._conn.commit()

    def remove_graph_links(self, memory_id: str):
        """Remove all graph links for a memory."""
        self._conn.execute("DELETE FROM graph_links WHERE memory_id=?", (memory_id,))
        self._conn.commit()

    def search_by_entity(self, entity: str) -> list[MemoryEntry]:
        """Find all memories linked to an entity."""
        rows = self._conn.execute(
            """SELECT m.* FROM memories m
               JOIN graph_links g ON m.id = g.memory_id
               WHERE g.node_id = ?""",
            (entity,),
        ).fetchall()
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def get_entities(self, memory_id: str) -> list[tuple[str, str]]:
        """Get all (entity, relation) pairs for a memory."""
        rows = self._conn.execute(
            "SELECT node_id, relation FROM graph_links WHERE memory_id=?",
            (memory_id,),
        ).fetchall()
        return [(r["node_id"], r["relation"]) for r in rows]

    def get_all_entities(self) -> list[str]:
        """List all unique entities in the graph."""
        rows = self._conn.execute(
            "SELECT DISTINCT node_id FROM graph_links"
        ).fetchall()
        return [r["node_id"] for r in rows]

    def get_related_entities(self, entity: str, hops: int = 2) -> list[str]:
        """Find entities connected within N hops (via shared memories)."""
        visited = {entity}
        frontier = {entity}
        for _ in range(hops):
            if not frontier:
                break
            # Find all memories linked to frontier entities
            placeholders = ",".join("?" * len(frontier))
            mem_rows = self._conn.execute(
                f"SELECT DISTINCT memory_id FROM graph_links WHERE node_id IN ({placeholders})",
                list(frontier),
            ).fetchall()
            mem_ids = [r["memory_id"] for r in mem_rows]
            if not mem_ids:
                break
            # Find all entities linked to those memories
            placeholders2 = ",".join("?" * len(mem_ids))
            ent_rows = self._conn.execute(
                f"SELECT DISTINCT node_id FROM graph_links WHERE memory_id IN ({placeholders2})",
                mem_ids,
            ).fetchall()
            new_entities = {r["node_id"] for r in ent_rows} - visited
            visited.update(new_entities)
            frontier = new_entities
        visited.discard(entity)
        return list(visited)

    # ── Namespace methods ──────────────────────────────────────

    def search_fts_ns(self, query: str, limit: int = 20, namespace: Optional[str] = None) -> list[MemoryEntry]:
        """
        FTS search with namespace filtering.
        
        Args:
            query: Search query
            limit: Maximum results
            namespace: Namespace filter ("*" = all, None = "default")
        """
        from engram.engram_tokenizers import contains_cjk, tokenize_for_fts
        
        # Tokenize CJK queries for better matching
        if contains_cjk(query):
            tokens = tokenize_for_fts(query).split()
            tokens = [t for t in tokens if len(t) > 0 and not (len(t) == 1 and not t.isalnum())]
            if tokens:
                safe_tokens = [f'"{t}"' for t in tokens]
                query = " OR ".join(safe_tokens)
        
        ns = namespace or "default"
        
        if ns == "*":
            # Search all namespaces
            rows = self._conn.execute(
                """SELECT m.* FROM memories m
                   JOIN memories_fts f ON m.rowid = f.rowid
                   WHERE memories_fts MATCH ?
                   ORDER BY rank LIMIT ?""",
                (query, limit),
            ).fetchall()
        else:
            # Search specific namespace
            rows = self._conn.execute(
                """SELECT m.* FROM memories m
                   JOIN memories_fts f ON m.rowid = f.rowid
                   WHERE memories_fts MATCH ? AND m.namespace = ?
                   ORDER BY rank LIMIT ?""",
                (query, ns, limit),
            ).fetchall()
        
        return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def all_in_namespace(self, namespace: Optional[str] = None) -> list[MemoryEntry]:
        """Get all memories in a namespace."""
        ns = namespace or "default"
        
        if ns == "*":
            return self.all()
        else:
            rows = self._conn.execute(
                "SELECT * FROM memories WHERE namespace = ?",
                (ns,)
            ).fetchall()
            return [_row_to_entry(r, self.get_access_times(r["id"])) for r in rows]

    def get_namespace(self, memory_id: str) -> Optional[str]:
        """Get the namespace for a memory."""
        row = self._conn.execute(
            "SELECT namespace FROM memories WHERE id = ?",
            (memory_id,)
        ).fetchone()
        return row[0] if row else None

    def close(self):
        self._conn.close()


if __name__ == "__main__":
    print("=== SQLiteStore smoke test ===")
    store = SQLiteStore()

    # Add memories
    m1 = store.add("SaltyHall uses Supabase for its backend", MemoryType.FACTUAL)
    m2 = store.add("On Feb 2 we shipped the memory prototype", MemoryType.EPISODIC, importance=0.7)
    m3 = store.add("potato prefers action over discussion", MemoryType.RELATIONAL)
    m4 = store.add("Always use www.moltbook.com not moltbook.com", MemoryType.PROCEDURAL)
    print(f"Added {len(store.all())} memories")

    # Get by ID
    fetched = store.get(m1.id)
    assert fetched is not None
    assert fetched.content == m1.content
    assert len(fetched.access_times) == 2  # creation + get
    print(f"Get OK: {fetched.id} has {len(fetched.access_times)} accesses")

    # FTS search
    results = store.search_fts("Supabase")
    assert len(results) == 1
    assert results[0].id == m1.id
    print(f"FTS 'Supabase': {len(results)} result(s)")

    results = store.search_fts("moltbook")
    assert len(results) == 1
    print(f"FTS 'moltbook': {len(results)} result(s)")

    # Search by type
    facts = store.search_by_type(MemoryType.FACTUAL)
    assert len(facts) == 1
    print(f"By type FACTUAL: {len(facts)}")

    # Search by layer
    working = store.search_by_layer(MemoryLayer.L3_WORKING)
    assert len(working) == 4
    print(f"By layer WORKING: {len(working)}")

    # Update
    m2.layer = MemoryLayer.L2_CORE
    m2.core_strength = 0.8
    store.update(m2)
    updated = store.get(m2.id)
    assert updated.layer == MemoryLayer.L2_CORE
    assert updated.core_strength == 0.8
    print("Update OK")

    # Delete
    store.delete(m3.id)
    assert store.get(m3.id) is None
    assert len(store.all()) == 3
    print("Delete OK")

    # Stats
    s = store.stats()
    print(f"Stats: {s}")
    assert s["total_memories"] == 3

    # Export
    store.export("/tmp/engram_test.db")
    print("Export OK")

    store.close()
    print("=== All tests passed ===")
