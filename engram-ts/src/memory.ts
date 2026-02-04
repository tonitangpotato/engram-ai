/**
 * Engram Memory — Public API
 */

import { MemoryConfig } from './config';
import { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE } from './core';
import { SQLiteStore } from './store';
import { retrieveTopK } from './activation';
import { SearchEngine, SearchResult } from './search';
import { runConsolidationCycle, getConsolidationStats } from './consolidation';
import { effectiveStrength, shouldForget, pruneforgotten } from './forgetting';
import { confidenceScore, confidenceLabel } from './confidence';
import { detectFeedback, applyReward } from './reward';
import { synapticDownscale } from './downscaling';
import { BaselineTracker } from './anomaly';
import { recordCoactivation, decayHebbianLinks, getHebbianNeighbors } from './hebbian';
import { SessionWorkingMemory, SessionRecallResult, getSessionWM } from './session_wm';

const TYPE_MAP: Record<string, MemoryType> = {};
for (const t of Object.values(MemoryType)) {
  TYPE_MAP[t] = t;
}

export class Memory {
  path: string;
  config: MemoryConfig;
  _store: SQLiteStore;
  private _tracker: BaselineTracker;
  private _createdAt: number;

  constructor(path: string = './engram.db', config?: MemoryConfig) {
    this.path = path;
    this.config = config ?? MemoryConfig.default();
    this._store = new SQLiteStore(path);
    this._tracker = new BaselineTracker(this.config.anomalyWindowSize);
    this._createdAt = Date.now() / 1000;
  }

  add(
    content: string,
    opts: {
      type?: string;
      importance?: number;
      source?: string;
      tags?: string[];
      entities?: Array<string | [string, string]>;
      contradicts?: string;
    } = {},
  ): string {
    const {
      type = 'factual',
      importance,
      source = '',
      tags,
      entities,
      contradicts,
    } = opts;

    const memoryType = TYPE_MAP[type] ?? MemoryType.FACTUAL;

    let actualContent = content;
    if (tags && tags.length > 0) {
      actualContent = `${content} [tags: ${tags.join(', ')}]`;
    }

    const entry = this._store.add(actualContent, memoryType, importance, source);

    if (contradicts) {
      const oldEntry = this._store.get(contradicts);
      if (oldEntry) {
        entry.contradicts = contradicts;
        this._store.update(entry);
        oldEntry.contradictedBy = entry.id;
        this._store.update(oldEntry);
      }
    }

    if (entities) {
      for (const ent of entities) {
        if (Array.isArray(ent)) {
          const [entity, relation] = ent;
          this._store.addGraphLink(entry.id, entity, relation ?? '');
        } else {
          this._store.addGraphLink(entry.id, ent, '');
        }
      }
    }

    this._tracker.update('encoding_rate', 1.0);
    return entry.id;
  }

  recall(
    query: string,
    opts: {
      limit?: number;
      context?: string[];
      types?: string[];
      minConfidence?: number;
      graphExpand?: boolean;
    } = {},
  ): Array<{
    id: string;
    content: string;
    type: string;
    confidence: number;
    confidence_label: string;
    strength: number;
    activation: number;
    age_days: number;
    layer: string;
    importance: number;
    contradicted: boolean;
  }> {
    const {
      limit = 5,
      context,
      types,
      minConfidence = 0.0,
      graphExpand = true,
    } = opts;

    const engine = new SearchEngine(this._store);
    const searchResults = engine.search({
      query,
      limit,
      contextKeywords: context,
      types,
      minConfidence,
      graphExpand,
    });

    const now = Date.now() / 1000;
    const output = searchResults.map(r => ({
      id: r.entry.id,
      content: r.entry.content,
      type: r.entry.memoryType,
      confidence: Math.round(r.confidence * 1000) / 1000,
      confidence_label: r.confidenceLabel,
      strength: Math.round(effectiveStrength(r.entry, now) * 1000) / 1000,
      activation: Math.round(r.score * 1000) / 1000,
      age_days: Math.round(r.entry.ageDays() * 10) / 10,
      layer: r.entry.layer,
      importance: Math.round(r.entry.importance * 100) / 100,
      contradicted: Boolean(r.entry.contradictedBy),
    }));

    // Record Hebbian co-activation
    if (this.config.hebbianEnabled && searchResults.length >= 2) {
      const resultIds = searchResults.map(r => r.entry.id);
      recordCoactivation(this._store, resultIds, this.config);
    }

    this._tracker.update('retrieval_count', output.length);
    return output;
  }

  consolidate(days: number = 1.0): void {
    runConsolidationCycle(
      this._store,
      days,
      this.config.interleaveRatio,
      this.config.alpha,
      this.config.mu1,
      this.config.mu2,
      this.config.replayBoost,
      this.config.promoteThreshold,
      this.config.demoteThreshold,
      this.config.archiveThreshold,
    );
    synapticDownscale(this._store, this.config.downscaleFactor);
    
    // Decay Hebbian links
    if (this.config.hebbianEnabled) {
      decayHebbianLinks(this._store, this.config.hebbianDecay);
    }
  }

  forget(opts: { memoryId?: string; threshold?: number } = {}): void {
    const threshold = opts.threshold ?? this.config.forgetThreshold;
    if (opts.memoryId) {
      this._store.delete(opts.memoryId);
    } else {
      pruneforgotten(this._store, threshold);
    }
  }

  reward(feedback: string, recentN: number = 3): void {
    const [polarity, conf] = detectFeedback(feedback);
    if (polarity === 'neutral' || conf < 0.3) return;
    applyReward(this._store, polarity, recentN, this.config.rewardMagnitude * conf);
  }

  downscale(factor?: number): { n_scaled: number; avg_before: number; avg_after: number } {
    return synapticDownscale(this._store, factor ?? this.config.downscaleFactor);
  }

  stats(): Record<string, any> {
    const consolidation = getConsolidationStats(this._store);
    const allMem = this._store.all();
    const now = Date.now() / 1000;

    const byType: Record<string, { count: number; avg_strength: number; avg_importance: number }> = {};
    for (const mt of Object.values(MemoryType)) {
      const entries = allMem.filter(m => m.memoryType === mt);
      if (entries.length > 0) {
        byType[mt] = {
          count: entries.length,
          avg_strength: Math.round(
            (entries.reduce((s, m) => s + effectiveStrength(m, now), 0) / entries.length) * 1000
          ) / 1000,
          avg_importance: Math.round(
            (entries.reduce((s, m) => s + m.importance, 0) / entries.length) * 100
          ) / 100,
        };
      }
    }

    return {
      total_memories: allMem.length,
      by_type: byType,
      layers: consolidation.layers,
      pinned: consolidation.pinned,
      uptime_hours: Math.round(((now - this._createdAt) / 3600) * 10) / 10,
      anomaly_metrics: this._tracker.metrics(),
    };
  }

  export(path: string): void {
    this._store.export(path);
  }

  updateMemory(memoryId: string, newContent: string, reason: string = 'correction'): string {
    const oldEntry = this._store.get(memoryId);
    if (!oldEntry) throw new Error(`Memory ${memoryId} not found`);

    return this.add(newContent, {
      type: oldEntry.memoryType,
      importance: oldEntry.importance,
      source: `${reason}:${memoryId}`,
      contradicts: memoryId,
    });
  }

  pin(memoryId: string): void {
    const entry = this._store.get(memoryId);
    if (entry) {
      entry.pinned = true;
      this._store.update(entry);
    }
  }

  unpin(memoryId: string): void {
    const entry = this._store.get(memoryId);
    if (entry) {
      entry.pinned = false;
      this._store.update(entry);
    }
  }

  hebbianLinks(memoryId: string): string[] {
    return getHebbianNeighbors(this._store, memoryId);
  }

  /**
   * Session-aware recall using cognitive working memory model.
   *
   * Instead of always doing expensive retrieval, this:
   * 1. Checks if the query topic overlaps with current working memory
   * 2. If yes (continuous topic) → returns cached working memory items
   * 3. If no (topic switch) → does full recall and updates working memory
   *
   * Based on Miller's Law (7±2 chunks) and Baddeley's Working Memory Model.
   * Reduces API calls by 70-80% for continuous conversation topics.
   */
  sessionRecall(
    query: string,
    opts: {
      sessionId?: string;
      sessionWM?: SessionWorkingMemory;
      limit?: number;
      types?: string[];
      minConfidence?: number;
    } = {},
  ): SessionRecallResult {
    const {
      sessionId = 'default',
      sessionWM,
      limit = 5,
      types,
      minConfidence = 0.0,
    } = opts;

    const swm = sessionWM ?? getSessionWM(sessionId);
    const wasEmpty = swm.isEmpty();
    const needsFull = wasEmpty || swm.needsRecall(query, this);

    let results: Array<{
      id: string;
      content: string;
      type: string;
      confidence: number;
      confidence_label: string;
      strength: number;
      age_days: number;
      from_working_memory: boolean;
    }>;

    if (needsFull) {
      // Full recall
      const recallResults = this.recall(query, { limit, types, minConfidence });
      results = recallResults.map(r => ({
        id: r.id,
        content: r.content,
        type: r.type,
        confidence: r.confidence,
        confidence_label: r.confidence_label,
        strength: r.strength,
        age_days: r.age_days,
        from_working_memory: false,
      }));

      // Update working memory
      swm.activate(results.map(r => r.id));
    } else {
      // Return working memory items
      const wmItems = swm.getActiveMemories(this);
      results = wmItems.map(r => ({
        id: r.id,
        content: r.content,
        type: r.type,
        confidence: r.confidence,
        confidence_label: r.confidence_label,
        strength: r.strength,
        age_days: r.age_days,
        from_working_memory: true,
      }));
    }

    return {
      results,
      fullRecallTriggered: needsFull,
      workingMemorySize: swm.size(),
      reason: wasEmpty ? 'empty_wm' : (needsFull ? 'topic_change' : 'topic_continuous'),
    };
  }

  close(): void {
    this._store.close();
  }

  get length(): number {
    return this._store.all().length;
  }

  toString(): string {
    return `Memory(path='${this.path}', entries=${this.length})`;
  }
}
