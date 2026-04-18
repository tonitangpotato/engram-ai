/**
 * Hybrid search: combines vector similarity + FTS5 text search
 */

import Database from 'better-sqlite3';
import { vectorSearch, VectorSearchResult } from './vector_search';

export interface HybridSearchResult {
  id: string;
  score: number;
  vectorScore: number;
  ftsScore: number;
}

/**
 * FTS5 search with BM25 scoring
 */
function ftsSearch(
  db: Database.Database,
  query: string,
  limit: number = 50,
): Map<string, number> {
  const stmt = db.prepare(`
    SELECT m.id, bm25(memories_fts) as score
    FROM memories_fts
    JOIN memories m ON memories_fts.rowid = m.rowid
    WHERE memories_fts MATCH ?
    ORDER BY score
    LIMIT ?
  `);

  const rows = stmt.all(query, limit) as Array<{ id: string; score: number }>;
  const results = new Map<string, number>();

  // Normalize BM25 scores to [0, 1]
  // BM25 scores are negative (lower is better), so we negate and normalize
  const scores = rows.map((r) => -r.score);
  const maxScore = Math.max(...scores, 1);

  for (let i = 0; i < rows.length; i++) {
    results.set(rows[i].id, scores[i] / maxScore);
  }

  return results;
}

/**
 * Hybrid search combining vector similarity and FTS5
 * 
 * @param db SQLite database
 * @param queryVector Query embedding vector (null = FTS5 only)
 * @param queryText Query text for FTS5
 * @param limit Number of results
 * @param vectorWeight Weight for vector similarity (0.0 - 1.0)
 * @param ftsWeight Weight for FTS5 score (0.0 - 1.0)
 * @param minVectorSimilarity Minimum vector similarity threshold
 */
export function hybridSearch(
  db: Database.Database,
  queryVector: number[] | null,
  queryText: string,
  opts: {
    limit?: number;
    vectorWeight?: number;
    ftsWeight?: number;
    minVectorSimilarity?: number;
  } = {},
): HybridSearchResult[] {
  const {
    limit = 10,
    vectorWeight = 0.7,
    ftsWeight = 0.3,
    minVectorSimilarity = 0.0,
  } = opts;

  // Vector search results
  const vectorResults = new Map<string, number>();
  if (queryVector) {
    const results = vectorSearch(db, queryVector, limit * 2, minVectorSimilarity);
    for (const r of results) {
      vectorResults.set(r.id, r.similarity);
    }
  }

  // FTS5 search results
  const ftsResults = ftsSearch(db, queryText, limit * 2);

  // Combine results
  const allIds = new Set([...vectorResults.keys(), ...ftsResults.keys()]);
  const combined: HybridSearchResult[] = [];

  for (const id of allIds) {
    const vectorScore = vectorResults.get(id) || 0;
    const ftsScore = ftsResults.get(id) || 0;

    // Weighted fusion
    const score = vectorScore * vectorWeight + ftsScore * ftsWeight;

    combined.push({
      id,
      score,
      vectorScore,
      ftsScore,
    });
  }

  // Sort by combined score (descending)
  combined.sort((a, b) => b.score - a.score);

  return combined.slice(0, limit);
}

/**
 * Adaptive hybrid search: auto-adjust weights based on result overlap
 * 
 * If vector and FTS5 return similar results → increase weight on vector
 * If results diverge → balance weights more evenly
 */
export function adaptiveHybridSearch(
  db: Database.Database,
  queryVector: number[] | null,
  queryText: string,
  limit: number = 10,
): HybridSearchResult[] {
  if (!queryVector) {
    // No vector available, FTS5 only
    return hybridSearch(db, null, queryText, { limit, vectorWeight: 0, ftsWeight: 1.0 });
  }

  // Get top results from each method
  const vectorResults = vectorSearch(db, queryVector, limit);
  const ftsResults = ftsSearch(db, queryText, limit);

  // Calculate overlap (Jaccard similarity)
  const vectorIds = new Set(vectorResults.map((r) => r.id));
  const ftsIds = new Set(ftsResults.keys());
  const intersection = new Set([...vectorIds].filter((id) => ftsIds.has(id)));
  const union = new Set([...vectorIds, ...ftsIds]);
  const overlap = intersection.size / union.size;

  // Adjust weights based on overlap
  let vectorWeight: number;
  let ftsWeight: number;

  if (overlap > 0.5) {
    // High overlap → trust vector more (it's more precise)
    vectorWeight = 0.8;
    ftsWeight = 0.2;
  } else if (overlap > 0.2) {
    // Medium overlap → balanced
    vectorWeight = 0.6;
    ftsWeight = 0.4;
  } else {
    // Low overlap → query might be keyword-specific, trust FTS5 more
    vectorWeight = 0.4;
    ftsWeight = 0.6;
  }

  return hybridSearch(db, queryVector, queryText, {
    limit,
    vectorWeight,
    ftsWeight,
  });
}
