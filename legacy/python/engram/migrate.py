"""
Engram Schema Migration — v0 (pre-unified) → v1 (canonical)

Migrates an existing Engram SQLite database to conform to the unified schema
defined in DESIGN-unified-schema.md.

Changes in v1:
  - Add engram_meta table with schema_version = '1'
  - Rename column source_file → source
  - Rebuild FTS triggers to index only `content` (not summary/tokens)
  - Create entity tables (entities, entity_relations, memory_entities)
  - Add namespace column to hebbian_links
  - Add missing indexes

Backward compatibility:
  - summary and tokens columns are KEPT (not dropped)
  - graph_links table is KEPT (not dropped)
  - Existing data is preserved as-is
"""

import sqlite3
import sys


SCHEMA_VERSION = "1"


def get_schema_version(conn: sqlite3.Connection) -> str | None:
    """Check if engram_meta exists and return schema_version, or None if pre-v1."""
    try:
        row = conn.execute(
            "SELECT value FROM engram_meta WHERE key = 'schema_version'"
        ).fetchone()
        return row[0] if row else None
    except sqlite3.OperationalError:
        # Table doesn't exist → pre-v1
        return None


def migrate_to_v1(db_path: str, dry_run: bool = False) -> dict:
    """
    Migrate a pre-v1 Engram database to v1 schema.

    Args:
        db_path: Path to the SQLite database file
        dry_run: If True, print SQL but don't execute

    Returns:
        dict with migration results
    """
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA foreign_keys=OFF")  # Disable during migration

    version = get_schema_version(conn)

    if version == SCHEMA_VERSION:
        print(f"✓ Database already at schema version {SCHEMA_VERSION}. Nothing to do.")
        conn.close()
        return {"status": "already_migrated", "version": SCHEMA_VERSION}

    if version is not None and version > SCHEMA_VERSION:
        print(f"✗ Database is at version {version}, which is newer than {SCHEMA_VERSION}.")
        print("  Cannot downgrade. Aborting.")
        conn.close()
        return {"status": "version_too_new", "version": version}

    # Count memories for reporting
    mem_count = conn.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
    print(f"Migrating database with {mem_count} memories to schema v{SCHEMA_VERSION}...")

    if dry_run:
        print("\n[DRY RUN — no changes will be made]\n")

    steps = []

    # ── Step 1: Create engram_meta table ──────────────────────
    sql_meta = """
CREATE TABLE IF NOT EXISTS engram_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"""
    sql_meta_insert = f"INSERT OR REPLACE INTO engram_meta VALUES ('schema_version', '{SCHEMA_VERSION}');"
    steps.append(("Create engram_meta table", sql_meta + sql_meta_insert))

    # ── Step 2: Rename source_file → source ───────────────────
    # Check if column exists first
    cols = {row[1] for row in conn.execute("PRAGMA table_info(memories)").fetchall()}
    if "source_file" in cols and "source" not in cols:
        sql_rename = "ALTER TABLE memories RENAME COLUMN source_file TO source;"
        steps.append(("Rename source_file → source", sql_rename))
    elif "source" in cols:
        steps.append(("Rename source_file → source", "-- Already has 'source' column, skipping"))
    else:
        # Neither exists — add source column
        sql_add = "ALTER TABLE memories ADD COLUMN source TEXT DEFAULT '';"
        steps.append(("Add source column", sql_add))

    # ── Step 3: Rebuild FTS triggers (content only) ───────────
    # Drop old FTS table and triggers, recreate with content-only indexing
    sql_fts = """
-- Drop old triggers
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;
DROP TRIGGER IF EXISTS memories_fts_ai;
DROP TRIGGER IF EXISTS memories_fts_ad;
DROP TRIGGER IF EXISTS memories_fts_au;

-- Drop and recreate FTS table (content-only)
DROP TABLE IF EXISTS memories_fts;

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content,
    content=memories,
    content_rowid=rowid
);

-- Repopulate FTS index from existing data
INSERT INTO memories_fts(rowid, content)
    SELECT rowid, content FROM memories;

-- Create new triggers (content-only)
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
"""
    steps.append(("Rebuild FTS triggers (content-only)", sql_fts))

    # ── Step 4: Add namespace to hebbian_links if missing ─────
    hebb_cols = {row[1] for row in conn.execute("PRAGMA table_info(hebbian_links)").fetchall()}
    if "namespace" not in hebb_cols:
        sql_hebb_ns = "ALTER TABLE hebbian_links ADD COLUMN namespace TEXT NOT NULL DEFAULT 'default';"
        steps.append(("Add namespace to hebbian_links", sql_hebb_ns))
    else:
        steps.append(("Add namespace to hebbian_links", "-- Already exists, skipping"))

    # ── Step 5: Create entity tables ──────────────────────────
    sql_entities = """
CREATE TABLE IF NOT EXISTS entities (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    namespace   TEXT NOT NULL DEFAULT 'default',
    metadata    TEXT,
    created_at  REAL NOT NULL,
    updated_at  REAL NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_namespace ON entities(namespace);
"""
    steps.append(("Create entities table", sql_entities))

    sql_relations = """
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

CREATE INDEX IF NOT EXISTS idx_relations_source ON entity_relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON entity_relations(target_id);
CREATE INDEX IF NOT EXISTS idx_relations_type ON entity_relations(relation);
"""
    steps.append(("Create entity_relations table", sql_relations))

    sql_mem_entities = """
CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    role      TEXT DEFAULT 'mentioned',
    PRIMARY KEY (memory_id, entity_id)
);
"""
    steps.append(("Create memory_entities table", sql_mem_entities))

    # ── Step 6: Create optional tables from canonical schema ──
    sql_embeddings = """
CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT PRIMARY KEY REFERENCES memories(id) ON DELETE CASCADE,
    embedding TEXT NOT NULL,
    model     TEXT NOT NULL,
    dimension INTEGER NOT NULL
);
"""
    steps.append(("Create memory_embeddings table (if missing)", sql_embeddings))

    sql_acl = """
CREATE TABLE IF NOT EXISTS engram_acl (
    agent_id   TEXT NOT NULL,
    namespace  TEXT NOT NULL,
    permission TEXT NOT NULL,
    granted_by TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY (agent_id, namespace)
);
"""
    steps.append(("Create engram_acl table (if missing)", sql_acl))

    # ── Step 7: Add missing indexes ───────────────────────────
    sql_indexes = """
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at);
CREATE INDEX IF NOT EXISTS idx_hebbian_namespace ON hebbian_links(namespace);
"""
    steps.append(("Add missing indexes", sql_indexes))

    # ── Execute all steps ─────────────────────────────────────
    for step_name, sql in steps:
        print(f"  → {step_name}")
        if dry_run:
            for line in sql.strip().split("\n"):
                if line.strip() and not line.strip().startswith("--"):
                    print(f"    {line.strip()}")
        else:
            if not sql.strip().startswith("--"):
                conn.executescript(sql)

    if not dry_run:
        conn.execute("PRAGMA foreign_keys=ON")
        conn.commit()

    # Verify
    new_version = get_schema_version(conn)
    conn.close()

    if dry_run:
        print(f"\n[DRY RUN complete — no changes made]")
        return {"status": "dry_run", "steps": len(steps)}
    else:
        print(f"\n✓ Migration complete! Schema version: {new_version}")
        print(f"  {mem_count} memories preserved.")
        return {"status": "migrated", "version": new_version, "memories": mem_count}


def main():
    """CLI entry point for migration."""
    import argparse

    parser = argparse.ArgumentParser(description="Migrate Engram database to v1 schema")
    parser.add_argument("db_path", help="Path to the Engram SQLite database")
    parser.add_argument("--dry-run", action="store_true", help="Print SQL without executing")
    args = parser.parse_args()

    migrate_to_v1(args.db_path, dry_run=args.dry_run)


if __name__ == "__main__":
    main()
