"""
SQLite-backed memory store for Engram.
Replaces the in-memory dict-based MemoryStore with persistent storage.
"""

import sqlite3
import shutil
import time
import uuid
from typing import Optional

# TODO: import from engram.core once package is finalized
import sys, os
from engram.core import MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE


_SCHEMA = """
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    summary TEXT DEFAULT '',
    tokens TEXT DEFAULT '',
    memory_type TEXT NOT NULL,
    layer TEXT NOT NULL,
    created_at REAL NOT NULL,
    working_strength REAL DEFAULT 1.0,
    core_strength REAL DEFAULT 0.0,
    importance REAL DEFAULT 0.3,
    pinned INTEGER DEFAULT 0,
    consolidation_count INTEGER DEFAULT 0,
    last_consolidated REAL,
    source_file TEXT DEFAULT '',
    contradicts TEXT DEFAULT '',
    contradicted_by TEXT DEFAULT ''
);

CREATE TABLE IF NOT EXISTS access_log (
    memory_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    accessed_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS graph_links (
    memory_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    relation TEXT DEFAULT ''
);

CREATE TABLE IF NOT EXISTS hebbian_links (
    source_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    target_id TEXT REFERENCES memories(id) ON DELETE CASCADE,
    strength REAL DEFAULT 1.0,
    coactivation_count INTEGER DEFAULT 0,
    created_at REAL DEFAULT (strftime('%s', 'now')),
    PRIMARY KEY (source_id, target_id)
);

CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_mid ON graph_links(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_nid ON graph_links(node_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_source ON hebbian_links(source_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_target ON hebbian_links(target_id);
"""

_FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, summary, tokens, content=memories, content_rowid=rowid
);
"""

_FTS_TRIGGERS = """
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, summary, tokens)
    VALUES (new.rowid, new.content, new.summary, new.tokens);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, tokens)
    VALUES ('delete', old.rowid, old.content, old.summary, old.tokens);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary, tokens)
    VALUES ('delete', old.rowid, old.content, old.summary, old.tokens);
    INSERT INTO memories_fts(rowid, content, summary, tokens)
    VALUES (new.rowid, new.content, new.summary, new.tokens);
END;
"""


def _row_to_entry(row: sqlite3.Row, access_times: list[float] | None = None) -> MemoryEntry:
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
        source_file=row["source_file"] or "",
        contradicts=row["contradicts"] or "",
        contradicted_by=row["contradicted_by"] or "",
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
        self._migrate_contradiction_columns()
        self._conn.executescript(_FTS_SCHEMA)
        self._conn.executescript(_FTS_TRIGGERS)
        self._conn.commit()

    def _migrate_contradiction_columns(self):
        """Add contradiction columns if they don't exist (migration for older DBs)."""
        cursor = self._conn.execute("PRAGMA table_info(memories)")
        columns = {row[1] for row in cursor.fetchall()}
        if "contradicts" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN contradicts TEXT DEFAULT ''")
        if "contradicted_by" not in columns:
            self._conn.execute("ALTER TABLE memories ADD COLUMN contradicted_by TEXT DEFAULT ''")

    def add(self, content: str, memory_type: MemoryType = MemoryType.FACTUAL,
            importance: Optional[float] = None, source_file: str = "",
            created_at: Optional[float] = None) -> MemoryEntry:
        entry = MemoryEntry(
            content=content,
            memory_type=memory_type,
            importance=importance if importance is not None else DEFAULT_IMPORTANCE[memory_type],
            working_strength=1.0,
            core_strength=0.0,
            source_file=source_file,
        )
        # Override created_at if provided (for temporal simulation)
        if created_at is not None:
            entry.created_at = created_at
        
        # Generate tokens for CJK content
        from engram.tokenizers import contains_cjk, tokenize_for_fts
        tokens = tokenize_for_fts(content) if contains_cjk(content) else ""
        
        self._conn.execute(
            """INSERT INTO memories (id, content, summary, tokens, memory_type, layer, created_at,
               working_strength, core_strength, importance, pinned, consolidation_count,
               last_consolidated, source_file, contradicts, contradicted_by)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (entry.id, entry.content, entry.summary, tokens, entry.memory_type.value,
             entry.layer.value, entry.created_at, entry.working_strength,
             entry.core_strength, entry.importance, int(entry.pinned),
             entry.consolidation_count, entry.last_consolidated, entry.source_file,
             entry.contradicts, entry.contradicted_by),
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
        self._conn.execute(
            """UPDATE memories SET content=?, summary=?, memory_type=?, layer=?,
               working_strength=?, core_strength=?, importance=?, pinned=?,
               consolidation_count=?, last_consolidated=?, source_file=?,
               contradicts=?, contradicted_by=?
               WHERE id=?""",
            (entry.content, entry.summary, entry.memory_type.value, entry.layer.value,
             entry.working_strength, entry.core_strength, entry.importance,
             int(entry.pinned), entry.consolidation_count, entry.last_consolidated,
             entry.source_file, entry.contradicts, entry.contradicted_by, entry.id),
        )
        self._conn.commit()

    def search_fts(self, query: str, limit: int = 20) -> list[MemoryEntry]:
        from engram.tokenizers import contains_cjk, tokenize_for_fts
        
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
