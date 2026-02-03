/**
 * Hybrid Search Engine — FTS5 + ACT-R + Structured Filtering
 */

import { MemoryEntry, MemoryType, MemoryLayer } from './core';
import { SQLiteStore } from './store';
import { retrievalActivation } from './activation';
import { effectiveStrength } from './forgetting';
import { confidenceScore, confidenceLabel } from './confidence';

export interface SearchResult {
  entry: MemoryEntry;
  score: number;
  confidence: number;
  confidenceLabel: string;
  relevance: number;
}

export class SearchEngine {
  store: SQLiteStore;

  constructor(store: SQLiteStore) {
    this.store = store;
  }

  search(opts: {
    query?: string;
    limit?: number;
    contextKeywords?: string[];
    types?: string[];
    layers?: string[];
    minConfidence?: number;
    timeRange?: [number, number];
    graphExpand?: boolean;
  } = {}): SearchResult[] {
    const {
      query = '',
      limit = 5,
      contextKeywords,
      types,
      layers,
      minConfidence = 0.0,
      timeRange,
      graphExpand = true,
    } = opts;

    let candidates = this.getCandidates(query, types, layers, timeRange);

    if (graphExpand && candidates.length > 0) {
      candidates = this.expandViaGraph(candidates);

      // Re-apply filters on expanded set
      if (types) {
        const typeSet = new Set(types);
        candidates = candidates.filter(c => typeSet.has(c.memoryType));
      }
      if (layers) {
        const layerSet = new Set(layers);
        candidates = candidates.filter(c => layerSet.has(c.layer));
      }
      if (timeRange) {
        const [tMin, tMax] = timeRange;
        candidates = candidates.filter(c => c.createdAt >= tMin && c.createdAt <= tMax);
      }
    }

    const scored = this.scoreCandidates(candidates, contextKeywords, query.trim().length > 0);
    return this.rankAndFilter(scored, limit, minConfidence);
  }

  private getCandidates(
    query: string,
    types?: string[],
    layers?: string[],
    timeRange?: [number, number],
  ): MemoryEntry[] {
    const q = query.trim();
    let candidates: MemoryEntry[];

    if (q) {
      candidates = this.store.searchFts(q, 100);
      if (candidates.length === 0) {
        candidates = this.store.all();
      }
    } else {
      candidates = this.store.all();
    }

    if (types) {
      const typeSet = new Set(types);
      candidates = candidates.filter(c => typeSet.has(c.memoryType));
    }
    if (layers) {
      const layerSet = new Set(layers);
      candidates = candidates.filter(c => layerSet.has(c.layer));
    }
    if (timeRange) {
      const [tMin, tMax] = timeRange;
      candidates = candidates.filter(c => c.createdAt >= tMin && c.createdAt <= tMax);
    }

    return candidates;
  }

  private expandViaGraph(candidates: MemoryEntry[]): MemoryEntry[] {
    const seenIds = new Set(candidates.map(c => c.id));
    const allEntities = new Set<string>();

    for (const c of candidates) {
      for (const [entity] of this.store.getEntities(c.id)) {
        allEntities.add(entity);
      }
    }

    const expandedEntities = new Set(allEntities);
    for (const entity of allEntities) {
      const related = this.store.getRelatedEntities(entity, 1);
      for (const r of related) expandedEntities.add(r);
    }

    const newCandidates: MemoryEntry[] = [];
    for (const entity of expandedEntities) {
      for (const entry of this.store.searchByEntity(entity)) {
        if (!seenIds.has(entry.id)) {
          seenIds.add(entry.id);
          newCandidates.push(entry);
        }
      }
    }

    return [...candidates, ...newCandidates];
  }

  private scoreCandidates(
    candidates: MemoryEntry[],
    contextKeywords?: string[],
    hasQuery: boolean = false,
  ): SearchResult[] {
    const now = Date.now() / 1000;
    const results: SearchResult[] = [];

    for (const entry of candidates) {
      if (entry.accessTimes.length === 0) {
        entry.accessTimes = this.store.getAccessTimes(entry.id);
      }

      const actScore = retrievalActivation(entry, contextKeywords, now);
      if (actScore === -Infinity) continue;

      const conf = confidenceScore(entry, null, now);
      const label = confidenceLabel(conf);

      const relevance = hasQuery ? 1.0 : 0.0;
      const score = actScore + 0.5 * relevance;

      results.push({
        entry,
        score,
        confidence: conf,
        confidenceLabel: label,
        relevance,
      });
    }

    return results;
  }

  private rankAndFilter(
    scored: SearchResult[],
    limit: number,
    minConfidence: number,
  ): SearchResult[] {
    let results = scored;
    if (minConfidence > 0) {
      results = results.filter(r => r.confidence >= minConfidence);
    }
    results.sort((a, b) => b.score - a.score);
    return results.slice(0, limit);
  }
}
