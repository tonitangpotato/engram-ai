/**
 * SQLite-backed memory store with FTS5 search.
 */

import Database, { type Database as DatabaseType } from 'better-sqlite3';
import { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE } from './core';
import { initAclTables } from './acl';
import { containsCjk, tokenizeForFts } from './tokenizers';

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
    contradicted_by TEXT DEFAULT '',
    namespace TEXT NOT NULL DEFAULT 'default'
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
    created_at REAL,
    PRIMARY KEY (source_id, target_id)
);

CREATE INDEX IF NOT EXISTS idx_access_log_mid ON access_log(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_mid ON graph_links(memory_id);
CREATE INDEX IF NOT EXISTS idx_graph_links_nid ON graph_links(node_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_source ON hebbian_links(source_id);
CREATE INDEX IF NOT EXISTS idx_hebbian_target ON hebbian_links(target_id);
CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
`;

const _FTS_SCHEMA = `
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    content, summary, content=memories, content_rowid=rowid
);
`;

// NOTE: FTS triggers removed - we now manually manage FTS insertions
// with tokenized content for CJK support (like Rust implementation).
// This allows us to preprocess content with jieba/character tokenization before indexing.

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
  metadata: string | null;
}

function rowToEntry(row: MemoryRow, accessTimes?: number[]): MemoryEntry {
  // Parse metadata JSON if present
  let metadata: Record<string, unknown> | null = null;
  if (row.metadata) {
    try {
      metadata = JSON.parse(row.metadata);
    } catch {
      metadata = null;
    }
  }

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
    metadata,
  });
}

export class SQLiteStore {
  dbPath: string;
  public db: DatabaseType; // Made public for vector_search access

  constructor(dbPath: string = ':memory:') {
    this.dbPath = dbPath;
    this.db = new Database(dbPath);
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('foreign_keys = ON');
    this.db.exec(_SCHEMA);
    this._migrateContradictionColumns();
    this._migrateStdpColumns();
    this.db.exec(_FTS_SCHEMA);
    this._dropFtsTriggers(); // Remove old triggers - we manage FTS manually for CJK support
    this._rebuildFtsIfNeeded(); // Re-tokenize FTS index for CJK support
    // Initialize v2 features
    initAclTables(this.db);
  }

  /** Drop FTS triggers if they exist (we now manage FTS manually for CJK tokenization). */
  private _dropFtsTriggers(): void {
    this.db.exec('DROP TRIGGER IF EXISTS memories_ai');
    this.db.exec('DROP TRIGGER IF EXISTS memories_ad');
    this.db.exec('DROP TRIGGER IF EXISTS memories_au');
  }

  /** Current FTS tokenization version — bump when tokenization logic changes. */
  private static readonly FTS_CJK_VERSION = '1';

  /**
   * Rebuild FTS index with CJK tokenization if not already done.
   * Uses engram_meta 'fts_cjk_version' to track whether the FTS index
   * has been rebuilt with CJK-aware tokenization.
   */
  private _rebuildFtsIfNeeded(): void {
    // Check if engram_meta table exists
    const metaExists = this.db.prepare(
      "SELECT name FROM sqlite_master WHERE type='table' AND name='engram_meta'"
    ).get();
    if (!metaExists) return; // No meta table = very old DB, skip

    const row = this.db.prepare(
      "SELECT value FROM engram_meta WHERE key = 'fts_cjk_version'"
    ).get() as { value: string } | undefined;

    if (row?.value === SQLiteStore.FTS_CJK_VERSION) return; // Already up to date

    const countRow = this.db.prepare('SELECT COUNT(*) as cnt FROM memories').get() as { cnt: number };
    if (countRow.cnt === 0) {
      // Empty DB, just mark as done
      this.db.prepare(
        "INSERT OR REPLACE INTO engram_meta (key, value) VALUES ('fts_cjk_version', ?)"
      ).run(SQLiteStore.FTS_CJK_VERSION);
      return;
    }

    // Rebuild: clear FTS and re-insert all with tokenization
    this.db.exec('DELETE FROM memories_fts');

    const rows = this.db.prepare(
      'SELECT rowid, content, summary FROM memories'
    ).all() as Array<{ rowid: number; content: string; summary: string | null }>;

    const insertStmt = this.db.prepare(
      'INSERT INTO memories_fts(rowid, content, summary) VALUES (?, ?, ?)'
    );

    const rebuildAll = this.db.transaction(() => {
      for (const r of rows) {
        const content = r.content || '';
        const summary = r.summary || '';
        const ftsContent = containsCjk(content) ? tokenizeForFts(content) : content;
        const ftsSummary = summary && containsCjk(summary) ? tokenizeForFts(summary) : summary;
        insertStmt.run(r.rowid, ftsContent, ftsSummary);
      }
      this.db.prepare(
        "INSERT OR REPLACE INTO engram_meta (key, value) VALUES ('fts_cjk_version', ?)"
      ).run(SQLiteStore.FTS_CJK_VERSION);
    });

    rebuildAll();
    console.log(`[engram] Rebuilt FTS index with CJK tokenization for ${rows.length} memories`);
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
    // Phase 1: metadata column for structured data (causal memories etc.)
    if (!colNames.has('metadata')) {
      this.db.exec("ALTER TABLE memories ADD COLUMN metadata TEXT");
    }
    // v2: namespace column for multi-agent memory sharing
    if (!colNames.has('namespace')) {
      this.db.exec("ALTER TABLE memories ADD COLUMN namespace TEXT NOT NULL DEFAULT 'default'");
      this.db.exec("CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace)");
    }
    // Index on memory_type for type-filtered queries
    this.db.exec("CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type)");
  }

  private _migrateStdpColumns(): void {
    const cols = this.db.pragma('table_info(hebbian_links)') as Array<{ name: string }>;
    const colNames = new Set(cols.map(c => c.name));
    if (!colNames.has('direction')) {
      this.db.exec("ALTER TABLE hebbian_links ADD COLUMN direction TEXT DEFAULT 'bidirectional'");
    }
    if (!colNames.has('temporal_forward')) {
      this.db.exec("ALTER TABLE hebbian_links ADD COLUMN temporal_forward INTEGER DEFAULT 0");
    }
    if (!colNames.has('temporal_backward')) {
      this.db.exec("ALTER TABLE hebbian_links ADD COLUMN temporal_backward INTEGER DEFAULT 0");
    }
  }

  add(content: string, memoryType: MemoryType = MemoryType.FACTUAL,
      importance?: number, sourceFile: string = '',
      metadata?: Record<string, unknown> | null, namespace: string = 'default'): MemoryEntry {
    const entry = new MemoryEntry({
      content,
      memoryType,
      importance: importance ?? DEFAULT_IMPORTANCE[memoryType],
      workingStrength: 1.0,
      coreStrength: 0.0,
      sourceFile,
      metadata: metadata ?? null,
    });

    const metadataJson = metadata ? JSON.stringify(metadata) : null;

    this.db.prepare(`
      INSERT INTO memories (id, content, summary, memory_type, layer, created_at,
        working_strength, core_strength, importance, pinned, consolidation_count,
        last_consolidated, source_file, contradicts, contradicted_by, metadata, namespace)
      VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
    `).run(
      entry.id, entry.content, entry.summary, entry.memoryType,
      entry.layer, entry.createdAt, entry.workingStrength,
      entry.coreStrength, entry.importance, entry.pinned ? 1 : 0,
      entry.consolidationCount, entry.lastConsolidated, entry.sourceFile,
      entry.contradicts, entry.contradictedBy, metadataJson, namespace,
    );

    // Get rowid for FTS insertion
    const rowInfo = this.db.prepare('SELECT rowid FROM memories WHERE id = ?').get(entry.id) as { rowid: number };
    const rowid = rowInfo.rowid;

    // Insert into FTS with tokenized content (CJK support)
    const ftsContent = containsCjk(content) ? tokenizeForFts(content) : content;
    const ftsSummary = entry.summary && containsCjk(entry.summary) ? tokenizeForFts(entry.summary) : entry.summary;
    this.db.prepare('INSERT INTO memories_fts(rowid, content, summary) VALUES (?,?,?)')
      .run(rowid, ftsContent, ftsSummary);

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
    // Get rowid before update
    const rowInfo = this.db.prepare('SELECT rowid FROM memories WHERE id = ?').get(entry.id) as { rowid: number } | undefined;
    if (!rowInfo) return; // Entry not found

    const rowid = rowInfo.rowid;
    const metadataJson = entry.metadata ? JSON.stringify(entry.metadata) : null;

    this.db.prepare(`
      UPDATE memories SET content=?, summary=?, memory_type=?, layer=?,
        working_strength=?, core_strength=?, importance=?, pinned=?,
        consolidation_count=?, last_consolidated=?, source_file=?,
        contradicts=?, contradicted_by=?, metadata=?
      WHERE id=?
    `).run(
      entry.content, entry.summary, entry.memoryType, entry.layer,
      entry.workingStrength, entry.coreStrength, entry.importance,
      entry.pinned ? 1 : 0, entry.consolidationCount, entry.lastConsolidated,
      entry.sourceFile, entry.contradicts, entry.contradictedBy,
      metadataJson, entry.id,
    );

    // Update FTS index with tokenized content
    const ftsContent = containsCjk(entry.content) ? tokenizeForFts(entry.content) : entry.content;
    const ftsSummary = entry.summary && containsCjk(entry.summary) ? tokenizeForFts(entry.summary) : entry.summary;
    // Delete old FTS entry
    this.db.prepare("INSERT INTO memories_fts(memories_fts, rowid, content, summary) VALUES ('delete', ?, ?, ?)")
      .run(rowid, ftsContent, ftsSummary);
    // Insert new FTS entry
    this.db.prepare('INSERT INTO memories_fts(rowid, content, summary) VALUES (?,?,?)')
      .run(rowid, ftsContent, ftsSummary);
  }

  searchFts(query: string, limit: number = 20): MemoryEntry[] {
    // Tokenize CJK queries for better matching
    let ftsQuery = query;
    if (containsCjk(query)) {
      const tokens = tokenizeForFts(query).split(' ').filter(t => t.length > 0);
      if (tokens.length > 0) {
        // Use OR to match ANY token (more intuitive for semantic search)
        // Quote tokens to handle special characters
        ftsQuery = tokens.map(t => `"${t}"`).join(' OR ');
      }
    }

    const rows = this.db.prepare(`
      SELECT m.* FROM memories m
      JOIN memories_fts f ON m.rowid = f.rowid
      WHERE memories_fts MATCH ?
      ORDER BY rank LIMIT ?
    `).all(ftsQuery, limit) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  /** Namespace-aware FTS search */
  searchFtsNs(query: string, limit: number = 20, namespace?: string): MemoryEntry[] {
    if (!namespace || namespace === 'default') {
      return this.searchFts(query, limit);
    }

    if (namespace === '*') {
      // Search all namespaces
      return this.searchFts(query, limit);
    }

    // Tokenize CJK queries for better matching
    let ftsQuery = query;
    if (containsCjk(query)) {
      const tokens = tokenizeForFts(query).split(' ').filter(t => t.length > 0);
      if (tokens.length > 0) {
        ftsQuery = tokens.map(t => `"${t}"`).join(' OR ');
      }
    }

    // Specific namespace
    const rows = this.db.prepare(`
      SELECT m.* FROM memories m
      JOIN memories_fts f ON m.rowid = f.rowid
      WHERE memories_fts MATCH ? AND m.namespace = ?
      ORDER BY rank LIMIT ?
    `).all(ftsQuery, namespace, limit) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  /** Get all memories in a namespace */
  allInNamespace(namespace?: string): MemoryEntry[] {
    if (!namespace || namespace === 'default') {
      const rows = this.db.prepare('SELECT * FROM memories WHERE namespace = ?')
        .all('default') as MemoryRow[];
      return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
    }

    if (namespace === '*') {
      return this.all();
    }

    const rows = this.db.prepare('SELECT * FROM memories WHERE namespace = ?')
      .all(namespace) as MemoryRow[];
    return rows.map(r => rowToEntry(r, this.getAccessTimes(r.id)));
  }

  /** Get namespace for a memory */
  getNamespace(memoryId: string): string | null {
    const row = this.db.prepare('SELECT namespace FROM memories WHERE id = ?')
      .get(memoryId) as { namespace: string } | undefined;
    return row?.namespace ?? null;
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
    // Get rowid and content for FTS deletion before deleting from main table
    const row = this.db.prepare('SELECT rowid, content, summary FROM memories WHERE id = ?')
      .get(memoryId) as { rowid: number; content: string; summary: string | null } | undefined;

    if (row) {
      const { rowid, content, summary } = row;
      // Delete from FTS index
      const ftsContent = containsCjk(content) ? tokenizeForFts(content) : content;
      const ftsSummary = summary && containsCjk(summary) ? tokenizeForFts(summary) : summary ?? '';
      this.db.prepare("INSERT INTO memories_fts(memories_fts, rowid, content, summary) VALUES ('delete', ?, ?, ?)")
        .run(rowid, ftsContent, ftsSummary);
    }

    // Delete from main table
    this.db.prepare('DELETE FROM memories WHERE id=?').run(memoryId);
  }

  export(path: string): void {
    // better-sqlite3 serialize() returns a Buffer copy of the entire DB
    const fs = require('fs');
    const buf = this.db.serialize();
    fs.writeFileSync(path, buf);
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

  // Hebbian link methods

  getHebbianLink(sourceId: string, targetId: string): { strength: number; coactivationCount: number; createdAt: number } | null {
    // Try the provided order first
    let row = this.db.prepare(
      'SELECT strength, coactivation_count, created_at FROM hebbian_links WHERE source_id=? AND target_id=?'
    ).get(sourceId, targetId) as { strength: number; coactivation_count: number; created_at: number } | undefined;
    
    // If not found, try the reverse order (links are stored with consistent ordering)
    if (!row) {
      row = this.db.prepare(
        'SELECT strength, coactivation_count, created_at FROM hebbian_links WHERE source_id=? AND target_id=?'
      ).get(targetId, sourceId) as { strength: number; coactivation_count: number; created_at: number } | undefined;
    }
    
    if (!row) return null;
    return {
      strength: row.strength,
      coactivationCount: row.coactivation_count,
      createdAt: row.created_at
    };
  }

  upsertHebbianLink(sourceId: string, targetId: string, strength: number, coactivationCount: number): void {
    const now = Date.now() / 1000;
    const existing = this.getHebbianLink(sourceId, targetId);
    
    if (existing) {
      this.db.prepare(`
        UPDATE hebbian_links
        SET strength = ?, coactivation_count = ?
        WHERE source_id = ? AND target_id = ?
      `).run(strength, coactivationCount, sourceId, targetId);
    } else {
      this.db.prepare(`
        INSERT INTO hebbian_links (source_id, target_id, strength, coactivation_count, created_at)
        VALUES (?, ?, ?, ?, ?)
      `).run(sourceId, targetId, strength, coactivationCount, now);
    }
  }

  getHebbianNeighbors(memoryId: string): string[] {
    // Get neighbors where this memory is the source
    const asSource = this.db.prepare(
      'SELECT target_id as neighbor FROM hebbian_links WHERE source_id = ? AND strength > 0'
    ).all(memoryId) as Array<{ neighbor: string }>;
    
    // Also get neighbors where this memory is the target
    const asTarget = this.db.prepare(
      'SELECT source_id as neighbor FROM hebbian_links WHERE target_id = ? AND strength > 0'
    ).all(memoryId) as Array<{ neighbor: string }>;
    
    // Combine and dedupe
    const neighbors = new Set<string>();
    for (const r of asSource) neighbors.add(r.neighbor);
    for (const r of asTarget) neighbors.add(r.neighbor);
    return Array.from(neighbors);
  }

  getAllHebbianLinks(): Array<{ sourceId: string; targetId: string; strength: number }> {
    const rows = this.db.prepare(
      'SELECT source_id, target_id, strength FROM hebbian_links WHERE strength > 0'
    ).all() as Array<{ source_id: string; target_id: string; strength: number }>;
    return rows.map(r => ({ sourceId: r.source_id, targetId: r.target_id, strength: r.strength }));
  }

  decayHebbianLinks(factor: number): number {
    this.db.prepare(
      'UPDATE hebbian_links SET strength = strength * ? WHERE strength > 0'
    ).run(factor);

    const result = this.db.prepare(
      'DELETE FROM hebbian_links WHERE strength > 0 AND strength < 0.1'
    ).run();

    return result.changes;
  }

  close(): void {
    this.db.close();
  }
}
