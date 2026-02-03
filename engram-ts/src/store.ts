/**
 * SQLite-backed memory store with FTS5 search.
 */

import Database, { type Database as DatabaseType } from 'better-sqlite3';
import { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE } from './core';

const _SCHEMA = `
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    summary TEXT DEFAULT '',
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

CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_mid ON graph_links(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_nid ON graph_links(node_id);
`;

const _FTS_SCHEMA = `
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, summary, content=memories, content_rowid=rowid
);
`;

const _FTS_TRIGGERS = `
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content, summary)
    VALUES (new.rowid, new.content, new.summary);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary)
    VALUES ('delete', old.rowid, old.content, old.summary);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content, summary)
    VALUES ('delete', old.rowid, old.content, old.summary);
    INSERT INTO memories_fts(rowid, content, summary)
    VALUES (new.rowid, new.content, new.summary);
END;
`;

interface MemoryRow {
  id: string;
  content: string;
  summary: string | null;
  memory_type: string;
  layer: string;
  created_at: number;
  working_strength: number;
  core_strength: number;
  importance: number;
  pinned: number;
  consolidation_count: number;
  last_consolidated: number | null;
  source_file: string | null;
  contradicts: string | null;
  contradicted_by: string | null;
}

function rowToEntry(row: MemoryRow, accessTimes?: number[]): MemoryEntry {
  return new MemoryEntry({
    id: row.id,
    content: row.content,
    summary: row.summary ?? '',
    memoryType: row.memory_type as MemoryType,
    layer: row.layer as MemoryLayer,
    createdAt: row.created_at,
    accessTimes: accessTimes ?? [],
    workingStrength: row.working_strength,
    coreStrength: row.core_strength,
    importance: row.importance,
    pinned: Boolean(row.pinned),
    consolidationCount: row.consolidation_count,
    lastConsolidated: row.last_consolidated,
    sourceFile: row.source_file ?? '',
    contradicts: row.contradicts ?? '',
    contradictedBy: row.contradicted_by ?? '',
  });
}

export class SQLiteStore {
  dbPath: string;
  private db: DatabaseType;

  constructor(dbPath: string = ':memory:') {
    this.dbPath = dbPath;
    this.db = new Database(dbPath);
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('foreign_keys = ON');
    this.db.exec(_SCHEMA);
    this._migrateContradictionColumns();
    this.db.exec(_FTS_SCHEMA);
    this.db.exec(_FTS_TRIGGERS);
  }

  private _migrateContradictionColumns(): void {
    const cols = this.db.pragma('table_info(memories)') as Array<{ name: string }>;
    const colNames = new Set(cols.map(c => c.name));
    if (!colNames.has('contradicts')) {
      this.db.exec("ALTER TABLE memories ADD COLUMN contradicts TEXT DEFAULT ''");
    }
    if (!colNames.has('contradicted_by')) {
      this.db.exec("ALTER TABLE memories ADD COLUMN contradicted_by TEXT DEFAULT ''");
    }
  }

  add(content: string, memoryType: MemoryType = MemoryType.FACTUAL,
      importance?: number, sourceFile: string = ''): MemoryEntry {
    const entry = new MemoryEntry({
      content,
      memoryType,
      importance: importance ?? DEFAULT_IMPORTANCE[memoryType],
      workingStrength: 1.0,
      coreStrength: 0.0,
      sourceFile,
    });

    this.db.prepare(`
      INSERT INTO memories (id, content, summary, memory_type, layer, created_at,
        working_strength, core_strength, importance, pinned, consolidation_count,
        last_consolidated, source_file, contradicts, contradicted_by)
      VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
    `).run(
      entry.id, entry.content, entry.summary, entry.memoryType,
      entry.layer, entry.createdAt, entry.workingStrength,
      entry.coreStrength, entry.importance, entry.pinned ? 1 : 0,
      entry.consolidationCount, entry.lastConsolidated, entry.sourceFile,
      entry.contradicts, entry.contradictedBy,
    );

    this.db.prepare('INSERT INTO access_log (memory_id, accessed_at) VALUES (?,?)')
      .run(entry.id, entry.createdAt);

    entry.accessTimes = [entry.createdAt];
    return entry;
  }

  get(memoryId: string): MemoryEntry | null {
    const row = this.db.prepare('SELECT * FROM memories WHERE id=?').get(memoryId) as MemoryRow | undefined;
    if (!row) return null;
    this.recordAccess(memoryId);
    const accessTimes = this.getAccessTimes(memoryId);
    return rowToEntry(row, accessTimes);
  }

  all(): MemoryEntry[] {
    const rows = this.db.prepare('SELECT * FROM memories').all() as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  update(entry: MemoryEntry): void {
    this.db.prepare(`
      UPDATE memories SET content=?, summary=?, memory_type=?, layer=?,
        working_strength=?, core_strength=?, importance=?, pinned=?,
        consolidation_count=?, last_consolidated=?, source_file=?,
        contradicts=?, contradicted_by=?
      WHERE id=?
    `).run(
      entry.content, entry.summary, entry.memoryType, entry.layer,
      entry.workingStrength, entry.coreStrength, entry.importance,
      entry.pinned ? 1 : 0, entry.consolidationCount, entry.lastConsolidated,
      entry.sourceFile, entry.contradicts, entry.contradictedBy, entry.id,
    );
  }

  searchFts(query: string, limit: number = 20): MemoryEntry[] {
    const rows = this.db.prepare(`
      SELECT m.* FROM memories m
      JOIN memories_fts f ON m.rowid = f.rowid
      WHERE memories_fts MATCH ?
      ORDER BY rank LIMIT ?
    `).all(query, limit) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  searchByType(memoryType: MemoryType): MemoryEntry[] {
    const rows = this.db.prepare('SELECT * FROM memories WHERE memory_type=?')
      .all(memoryType) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  searchByLayer(layer: MemoryLayer): MemoryEntry[] {
    const rows = this.db.prepare('SELECT * FROM memories WHERE layer=?')
      .all(layer) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  getAccessTimes(memoryId: string): number[] {
    const rows = this.db.prepare(
      'SELECT accessed_at FROM access_log WHERE memory_id=? ORDER BY accessed_at'
    ).all(memoryId) as Array<{ accessed_at: number }>;
    return rows.map(r => r.accessed_at);
  }

  recordAccess(memoryId: string): void {
    this.db.prepare('INSERT INTO access_log (memory_id, accessed_at) VALUES (?,?)')
      .run(memoryId, Date.now() / 1000);
  }

  delete(memoryId: string): void {
    this.db.prepare('DELETE FROM memories WHERE id=?').run(memoryId);
  }

  export(path: string): void {
    this.db.backup(path);
  }

  stats(): { total_memories: number; by_type: Record<string, number>; by_layer: Record<string, number>; total_accesses: number } {
    const total = (this.db.prepare('SELECT COUNT(*) as c FROM memories').get() as any).c;
    const byType: Record<string, number> = {};
    for (const row of this.db.prepare('SELECT memory_type, COUNT(*) as c FROM memories GROUP BY memory_type').all() as any[]) {
      byType[row.memory_type] = row.c;
    }
    const byLayer: Record<string, number> = {};
    for (const row of this.db.prepare('SELECT layer, COUNT(*) as c FROM memories GROUP BY layer').all() as any[]) {
      byLayer[row.layer] = row.c;
    }
    const accessCount = (this.db.prepare('SELECT COUNT(*) as c FROM access_log').get() as any).c;
    return { total_memories: total, by_type: byType, by_layer: byLayer, total_accesses: accessCount };
  }

  // Graph link methods

  addGraphLink(memoryId: string, entity: string, relation: string = ''): void {
    this.db.prepare('INSERT INTO graph_links (memory_id, node_id, relation) VALUES (?,?,?)')
      .run(memoryId, entity, relation);
  }

  removeGraphLinks(memoryId: string): void {
    this.db.prepare('DELETE FROM graph_links WHERE memory_id=?').run(memoryId);
  }

  searchByEntity(entity: string): MemoryEntry[] {
    const rows = this.db.prepare(`
      SELECT m.* FROM memories m
      JOIN graph_links g ON m.id = g.memory_id
      WHERE g.node_id = ?
    `).all(entity) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  getEntities(memoryId: string): Array<[string, string]> {
    const rows = this.db.prepare('SELECT node_id, relation FROM graph_links WHERE memory_id=?')
      .all(memoryId) as Array<{ node_id: string; relation: string }>;
    return rows.map(r => [r.node_id, r.relation]);
  }

  getAllEntities(): string[] {
    const rows = this.db.prepare('SELECT DISTINCT node_id FROM graph_links')
      .all() as Array<{ node_id: string }>;
    return rows.map(r => r.node_id);
  }

  getRelatedEntities(entity: string, hops: number = 2): string[] {
    const visited = new Set<string>([entity]);
    let frontier = new Set<string>([entity]);

    for (let i = 0; i < hops; i++) {
      if (frontier.size === 0) break;

      const placeholders = Array.from(frontier).map(() => '?').join(',');
      const memRows = this.db.prepare(
        `SELECT DISTINCT memory_id FROM graph_links WHERE node_id IN (${placeholders})`
      ).all(...frontier) as Array<{ memory_id: string }>;
      const memIds = memRows.map(r => r.memory_id);

      if (memIds.length === 0) break;

      const placeholders2 = memIds.map(() => '?').join(',');
      const entRows = this.db.prepare(
        `SELECT DISTINCT node_id FROM graph_links WHERE memory_id IN (${placeholders2})`
      ).all(...memIds) as Array<{ node_id: string }>;

      const newEntities = new Set<string>();
      for (const r of entRows) {
        if (!visited.has(r.node_id)) {
          newEntities.add(r.node_id);
          visited.add(r.node_id);
        }
      }
      frontier = newEntities;
    }

    visited.delete(entity);
    return Array.from(visited);
  }

  close(): void {
    this.db.close();
  }
}
