/**
 * Vector search utilities for SQLite
 */

import Database from 'better-sqlite3';

export interface VectorSearchResult {
  id: string;
  similarity: number;
}

/**
 * Cosine similarity between two vectors
 */
export function cosineSimilarity(a: number[], b: number[]): number {
  if (a.length !== b.length) {
    throw new Error('Vector dimensions must match');
  }

  let dotProduct = 0;
  let normA = 0;
  let normB = 0;

  for (let i = 0; i < a.length; i++) {
    dotProduct += a[i] * b[i];
    normA += a[i] * a[i];
    normB += b[i] * b[i];
  }

  if (normA === 0 || normB === 0) {
    return 0;
  }

  return dotProduct / (Math.sqrt(normA) * Math.sqrt(normB));
}

/**
 * Add vector column to memories table (migration)
 */
export function migrateVectorColumn(db: Database.Database): void {
  const cols = db.pragma('table_info(memories)') as Array<{ name: string }>;
  const colNames = new Set(cols.map((c) => c.name));

  if (!colNames.has('vector')) {
    db.exec('ALTER TABLE memories ADD COLUMN vector TEXT DEFAULT NULL');
    db.exec('CREATE INDEX IF NOT EXISTS idx_vector ON memories(vector) WHERE vector IS NOT NULL');
  }
}

/**
 * Store vector embedding for a memory
 */
export function storeVector(
  db: Database.Database,
  memoryId: string,
  vector: number[],
): void {
  const vectorJson = JSON.stringify(vector);
  const stmt = db.prepare('UPDATE memories SET vector = ? WHERE id = ?');
  stmt.run(vectorJson, memoryId);
}

/**
 * Get vector for a memory
 */
export function getVector(
  db: Database.Database,
  memoryId: string,
): number[] | null {
  const stmt = db.prepare('SELECT vector FROM memories WHERE id = ?');
  const row = stmt.get(memoryId) as { vector: string | null } | undefined;
  
  if (!row || !row.vector) {
    return null;
  }

  return JSON.parse(row.vector);
}

/**
 * Vector similarity search
 * 
 * Note: This is a naive implementation that loads all vectors into memory.
 * For large datasets, consider using a specialized vector database.
 */
export function vectorSearch(
  db: Database.Database,
  queryVector: number[],
  limit: number = 10,
  minSimilarity: number = 0.0,
): VectorSearchResult[] {
  const stmt = db.prepare('SELECT id, vector FROM memories WHERE vector IS NOT NULL');
  const rows = stmt.all() as Array<{ id: string; vector: string }>;

  const results: VectorSearchResult[] = [];

  for (const row of rows) {
    try {
      const vector = JSON.parse(row.vector);
      const similarity = cosineSimilarity(queryVector, vector);

      if (similarity >= minSimilarity) {
        results.push({ id: row.id, similarity });
      }
    } catch (err) {
      console.error(`Failed to parse vector for ${row.id}:`, err);
    }
  }

  // Sort by similarity (descending)
  results.sort((a, b) => b.similarity - a.similarity);

  return results.slice(0, limit);
}

/**
 * Get count of memories with vectors
 */
export function getVectorCount(db: Database.Database): number {
  const stmt = db.prepare('SELECT COUNT(*) as count FROM memories WHERE vector IS NOT NULL');
  const row = stmt.get() as { count: number };
  return row.count;
}

/**
 * Clear all vectors (for testing/migration)
 */
export function clearVectors(db: Database.Database): void {
  db.exec('UPDATE memories SET vector = NULL');
}
