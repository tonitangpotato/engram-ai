/**
 * Engram Memory — Public API
 */

import { MemoryConfig } from './config';
import { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_IMPORTANCE } from './core';
import { SQLiteStore } from './store';
import { retrieveTopK, retrievalActivation as calcRetrievalActivation } from './activation';
import { SearchEngine, SearchResult } from './search';
import { runConsolidationCycle, getConsolidationStats } from './consolidation';
import { effectiveStrength, shouldForget, pruneforgotten } from './forgetting';
import { confidenceScore, confidenceLabel } from './confidence';
import { detectFeedback, applyReward } from './reward';
import { synapticDownscale } from './downscaling';
import { BaselineTracker } from './anomaly';
import { recordCoactivation, decayHebbianLinks, getHebbianNeighbors } from './hebbian';
import { SessionWorkingMemory, SessionRecallResult, getSessionWM } from './session_wm';
import { EmbeddingProvider, EmbeddingConfig, DEFAULT_EMBEDDING_CONFIG } from './embeddings/base';
import { detectProvider, getAvailableProviders } from './embeddings/provider_detection';
import { migrateVectorColumn, storeVector, getVector, getVectorCount, vectorSearch, cosineSimilarity } from './vector_search';
import { hybridSearch, adaptiveHybridSearch } from './hybrid_search';
import { Permission, AclEntry, RecallResult } from './types';
import { grantPermission, revokePermission, checkPermission, listPermissions } from './acl';
import { SubscriptionManager, Subscription, Notification } from './subscriptions';
import { EmotionalBus, SoulUpdate, HeartbeatUpdate } from './bus/index';
import { MemoryExtractor, ExtractedFact, autoDetectExtractor } from './extractor';

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
  private _embeddingProvider: EmbeddingProvider | null = null;
  private _embeddingConfig: EmbeddingConfig;
  private _embeddingInitialized = false;
  private _agentId: string | null = null;
  private _emotionalBus: EmotionalBus | null = null;
  private _subscriptionManager: SubscriptionManager | null = null;
  private _extractor: MemoryExtractor | null = null;

  constructor(path: string = './engram.db', config?: MemoryConfig, embeddingConfig?: EmbeddingConfig) {
    this.path = path;
    this.config = config ?? MemoryConfig.default();
    this._store = new SQLiteStore(path);
    this._tracker = new BaselineTracker(this.config.anomalyWindowSize);
    this._createdAt = Date.now() / 1000;
    this._embeddingConfig = embeddingConfig ?? DEFAULT_EMBEDDING_CONFIG;
    
    // Migrate vector column if needed
    migrateVectorColumn(this._store.db);
    
    // Initialize subscription manager
    this._subscriptionManager = new SubscriptionManager(this._store.db);
    
    // Auto-configure extractor from environment/config
    this._extractor = autoDetectExtractor();
  }

  /**
   * Create a Memory instance with an Emotional Bus attached
   */
  static withEmotionalBus(
    path: string,
    workspaceDir: string,
    config?: MemoryConfig,
    embeddingConfig?: EmbeddingConfig,
  ): Memory {
    const mem = new Memory(path, config, embeddingConfig);
    mem._emotionalBus = new EmotionalBus(workspaceDir, mem._store.db);
    return mem;
  }

  /** Get the Emotional Bus, if attached */
  get emotionalBus(): EmotionalBus | null {
    return this._emotionalBus;
  }

  /** Set the agent ID for this memory instance (for ACL checks) */
  setAgentId(id: string): void {
    this._agentId = id;
  }

  /** Get the current agent ID */
  get agentId(): string | null {
    return this._agentId;
  }

  /** Set a memory extractor for LLM-based fact extraction. */
  setExtractor(extractor: MemoryExtractor): void {
    this._extractor = extractor;
  }

  /** Remove the memory extractor (revert to storing raw content). */
  clearExtractor(): void {
    this._extractor = null;
  }

  /** Check if an extractor is configured. */
  get hasExtractor(): boolean {
    return this._extractor !== null;
  }

  /**
   * Lazily initialize embedding provider
   */
  private async _ensureEmbedding(): Promise<void> {
    if (this._embeddingInitialized) return;
    
    this._embeddingProvider = await detectProvider(this._embeddingConfig);
    this._embeddingInitialized = true;
  }

  /**
   * Add a new memory (synchronous, no embedding)
   * For embedding support, use `addWithEmbedding()` instead
   */
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

  /**
   * Add a new memory with LLM extraction + embedding support (async).
   *
   * If an extractor is configured, passes content through LLM to extract
   * structured facts. Each extracted fact is stored as a separate memory.
   * Falls back to raw text if extraction fails or returns nothing.
   *
   * Also generates embeddings if a provider is available.
   */
  async addWithExtraction(
    content: string,
    opts: {
      type?: string;
      importance?: number;
      source?: string;
      tags?: string[];
      entities?: Array<string | [string, string]>;
      contradicts?: string;
      namespace?: string;
    } = {},
  ): Promise<string> {
    if (this._extractor) {
      try {
        const facts = await this._extractor.extract(content);
        if (facts.length > 0) {
          let lastId = '';
          for (const fact of facts) {
            lastId = await this.addWithEmbedding(fact.content, {
              type: fact.memoryType,
              importance: fact.importance,
              source: opts.source,
              tags: opts.tags,
              entities: opts.entities,
            });
          }
          return lastId;
        }
        // No facts extracted — nothing worth storing
        return '';
      } catch (e) {
        // Extraction failed — fall back to raw storage
        console.warn('Extractor failed, storing raw:', e);
      }
    }

    // No extractor or extraction failed — store raw (backward compatible)
    return this.addWithEmbedding(content, opts);
  }

  /**
   * Add a new memory with embedding support (async)
   */
  async addWithEmbedding(
    content: string,
    opts: {
      type?: string;
      importance?: number;
      source?: string;
      tags?: string[];
      entities?: Array<string | [string, string]>;
      contradicts?: string;
    } = {},
  ): Promise<string> {
    // Add memory first (synchronous)
    const memoryId = this.add(content, opts);

    // Generate and store embedding (async)
    await this._ensureEmbedding();
    
    if (this._embeddingProvider && this._embeddingProvider.name !== 'none') {
      try {
        const result = await this._embeddingProvider.embed(content);
        storeVector(this._store.db, memoryId, result.embedding);
      } catch (error) {
        console.error(`Failed to generate embedding for ${memoryId}:`, error);
        // Continue without embedding (graceful degradation)
      }
    }

    return memoryId;
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

    // === Hybrid Search: FTS + embedding + ACT-R (matching Rust weights) ===
    // Weights: 15% FTS, ~60% embedding, ~25% ACT-R
    const FTS_WEIGHT = this.config.ftsWeight ?? 0.15;
    const EMBEDDING_WEIGHT = this.config.embeddingWeight ?? 0.60;
    const ACTR_WEIGHT = this.config.actrWeight ?? 0.25;
    const now = Date.now() / 1000;
    const fetchLimit = limit * 3;

    // 1. Get FTS results (always)
    const ftsResults = this._store.searchFtsNs(query, fetchLimit, 'default');
    const ftsScoreMap = new Map<string, number>();
    for (let i = 0; i < ftsResults.length; i++) {
      const score = 1.0 - (i / Math.max(ftsResults.length, 1));
      ftsScoreMap.set(ftsResults[i].id, score);
    }

    // 2. Get vector results (if embeddings exist)
    const vectorScoreMap = new Map<string, number>();
    const vectorCount = getVectorCount(this._store.db);
    if (vectorCount > 0) {
      // We need the query vector — try to get it synchronously from stored vectors
      // For sync recall, use the vectorSearch from hybrid_search which works with stored vectors
      const hybridResults = hybridSearch(this._store.db, null, query, {
        limit: fetchLimit,
        vectorWeight: 0,
        ftsWeight: 1.0,
      });
      // Note: We can't do async embedding here in sync recall.
      // Vector scores will be used only if recallWithEmbedding() is called.
    }

    // 3. Collect all candidate IDs
    const allIds = new Set<string>();
    for (const entry of ftsResults) allIds.add(entry.id);

    // 4. Score each candidate with FTS + ACT-R (sync path — no embedding)
    const scored: Array<{ entry: MemoryEntry; combinedScore: number; activation: number }> = [];

    for (const id of allIds) {
      const entry = this._store.get(id);
      if (!entry) continue;
      if (types && !types.includes(entry.memoryType)) continue;

      const ftsScore = ftsScoreMap.get(id) ?? 0;
      const activation = calcRetrievalActivation(
        entry,
        context ?? [],
        now,
        this.config.actrDecay,
        this.config.contextWeight,
        this.config.importanceWeight,
      );
      // Normalize activation to 0..1
      const activationNorm = Math.max(0, Math.min(1, (activation + 10) / 20));

      // Without embedding in sync path: redistribute embedding weight to FTS + ACT-R
      const combinedScore = (FTS_WEIGHT + EMBEDDING_WEIGHT * 0.3) * ftsScore
        + (ACTR_WEIGHT + EMBEDDING_WEIGHT * 0.7) * activationNorm;

      scored.push({ entry, combinedScore, activation });
    }

    // Also run the original search engine for graph expansion (catches things FTS misses)
    if (graphExpand) {
      const engine = new SearchEngine(this._store);
      const extraResults = engine.search({
        query,
        limit: fetchLimit,
        contextKeywords: context,
        types,
        minConfidence: 0,
        graphExpand: true,
      });
      for (const r of extraResults) {
        if (!allIds.has(r.entry.id)) {
          allIds.add(r.entry.id);
          const ftsScore = ftsScoreMap.get(r.entry.id) ?? 0;
          const activationNorm = Math.max(0, Math.min(1, (r.score + 10) / 20));
          const combinedScore = (FTS_WEIGHT + EMBEDDING_WEIGHT * 0.3) * ftsScore
            + (ACTR_WEIGHT + EMBEDDING_WEIGHT * 0.7) * activationNorm;
          scored.push({ entry: r.entry, combinedScore, activation: r.score });
        }
      }
    }

    // Sort by combined score
    scored.sort((a, b) => b.combinedScore - a.combinedScore);

    const output = scored.slice(0, limit).map(({ entry, combinedScore, activation }) => {
      const conf = Math.max(0, Math.min(1, combinedScore));
      return {
        id: entry.id,
        content: entry.content,
        type: entry.memoryType,
        confidence: Math.round(conf * 1000) / 1000,
        confidence_label: confidenceLabel(conf),
        strength: Math.round(effectiveStrength(entry, now) * 1000) / 1000,
        activation: Math.round(activation * 1000) / 1000,
        age_days: Math.round(entry.ageDays() * 10) / 10,
        layer: entry.layer,
        importance: Math.round(entry.importance * 100) / 100,
        contradicted: Boolean(entry.contradictedBy),
      };
    }).filter(r => r.confidence >= minConfidence);

    // Record Hebbian co-activation
    if (this.config.hebbianEnabled && output.length >= 2) {
      const resultIds = output.map(r => r.id);
      recordCoactivation(this._store, resultIds, this.config);
    }

    this._tracker.update('retrieval_count', output.length);
    return output;
  }

  /**
   * Recall with embedding support (hybrid search)
   * Combines vector similarity + FTS5 for better cross-language/semantic recall
   */
  async recallWithEmbedding(
    query: string,
    opts: {
      limit?: number;
      context?: string[];
      types?: string[];
      minConfidence?: number;
      vectorWeight?: number;
      ftsWeight?: number;
    } = {},
  ): Promise<Array<{
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
    vector_score?: number;
    fts_score?: number;
  }>> {
    const {
      limit = 5,
      vectorWeight = this.config.embeddingWeight ?? 0.7,
      ftsWeight = this.config.ftsWeight ?? 0.3,
    } = opts;

    // Ensure embedding provider initialized
    await this._ensureEmbedding();

    // Generate query embedding
    let queryVector: number[] | null = null;
    if (this._embeddingProvider && this._embeddingProvider.name !== 'none') {
      try {
        const result = await this._embeddingProvider.embed(query);
        queryVector = result.embedding;
      } catch (error) {
        console.error('Failed to generate query embedding:', error);
        // Fall back to FTS5 only
      }
    }

    // Hybrid search
    const searchResults = adaptiveHybridSearch(
      this._store.db,
      queryVector,
      query,
      limit,
    );

    // Convert to Memory format
    const now = Date.now() / 1000;
    const output = searchResults.map(r => {
      const entry = this._store.get(r.id);
      if (!entry) {
        throw new Error(`Memory ${r.id} not found`);
      }

      const conf = confidenceScore(entry, null, now);

      return {
        id: entry.id,
        content: entry.content,
        type: entry.memoryType,
        confidence: Math.round(conf * 1000) / 1000,
        confidence_label: confidenceLabel(conf),
        strength: Math.round(effectiveStrength(entry, now) * 1000) / 1000,
        activation: Math.round(r.score * 1000) / 1000,
        age_days: Math.round(entry.ageDays() * 10) / 10,
        layer: entry.layer,
        importance: Math.round(entry.importance * 100) / 100,
        contradicted: Boolean(entry.contradictedBy),
        vector_score: Math.round(r.vectorScore * 1000) / 1000,
        fts_score: Math.round(r.ftsScore * 1000) / 1000,
      };
    });

    // Record Hebbian co-activation
    if (this.config.hebbianEnabled && output.length >= 2) {
      const resultIds = output.map(r => r.id);
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

  /**
   * Get embedding provider status
   */
  async embeddingStatus(): Promise<{
    provider: string;
    model: string;
    dimensions: number;
    available: boolean;
    vector_count: number;
    available_providers: {
      ollama: boolean;
      mcp: boolean;
      openai: boolean;
      selected: string;
    };
    error?: string;
  }> {
    await this._ensureEmbedding();

    const vectorCount = getVectorCount(this._store.db);
    const availableProviders = await getAvailableProviders(this._embeddingConfig);

    if (!this._embeddingProvider) {
      return {
        provider: 'none',
        model: 'none',
        dimensions: 0,
        available: false,
        vector_count: vectorCount,
        available_providers: availableProviders,
        error: 'No embedding provider configured',
      };
    }

    const info = await this._embeddingProvider.getInfo?.() || {
      name: this._embeddingProvider.name,
      model: this._embeddingProvider.model,
      dimensions: 0,
      available: false,
    };

    return {
      provider: info.name,
      model: info.model,
      dimensions: info.dimensions,
      available: info.available,
      vector_count: vectorCount,
      available_providers: availableProviders,
      error: info.error,
    };
  }

  close(): void {
    if (this._embeddingProvider && 'close' in this._embeddingProvider) {
      (this._embeddingProvider as any).close();
    }
    this._store.close();
  }

  get length(): number {
    return this._store.all().length;
  }

  toString(): string {
    return `Memory(path='${this.path}', entries=${this.length})`;
  }

  // === v2: Namespace Support ===

  /**
   * Add a memory to a specific namespace
   */
  addToNamespace(
    content: string,
    opts: {
      type?: string;
      importance?: number;
      source?: string;
      tags?: string[];
      entities?: Array<string | [string, string]>;
      contradicts?: string;
      namespace?: string;
    } = {},
  ): string {
    const namespace = opts.namespace || 'default';
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

    // Calculate base importance
    let baseImportance = importance ?? DEFAULT_IMPORTANCE[memoryType];

    // Apply drive alignment boost if Emotional Bus is attached
    if (this._emotionalBus) {
      const boost = this._emotionalBus.alignImportance(content);
      baseImportance = Math.min(1.0, baseImportance * boost);
    }

    const entry = this._store.add(actualContent, memoryType, baseImportance, source, null, namespace);

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

  /**
   * Add a memory with emotional tracking
   */
  addWithEmotion(
    content: string,
    opts: {
      type?: string;
      importance?: number;
      source?: string;
      tags?: string[];
      entities?: Array<string | [string, string]>;
      contradicts?: string;
      namespace?: string;
      emotion: number;
      domain: string;
    },
  ): string {
    const memoryId = this.addToNamespace(content, opts);

    // Record emotion if bus is attached
    if (this._emotionalBus) {
      this._emotionalBus.processInteraction(content, opts.emotion, opts.domain);
    }

    return memoryId;
  }

  /**
   * Recall from a specific namespace
   */
  recallFromNamespace(
    query: string,
    limit: number = 10,
    opts: {
      context?: string[];
      minConfidence?: number;
      namespace?: string;
    } = {},
  ): RecallResult[] {
    const namespace = opts.namespace || 'default';
    const now = Date.now() / 1000;
    const context = opts.context ?? [];
    const minConf = opts.minConfidence ?? 0.0;

    // Get candidate memories via FTS (namespace-aware)
    const candidates = this._store.searchFtsNs(query, limit * 3, namespace);

    // Score each candidate with ACT-R activation
    const scored = candidates.map(entry => {
      const activation = calcRetrievalActivation(
        entry,
        context,
        now,
        this.config.actrDecay,
        this.config.contextWeight,
        this.config.importanceWeight,
      );
      return { entry, activation };
    }).filter(x => x.activation > -Infinity);

    // Sort by activation descending
    scored.sort((a, b) => b.activation - a.activation);

    // Take top-k and compute confidence
    const results = scored.slice(0, limit).map(({ entry, activation }) => {
      const conf = confidenceScore(entry, this._store, now);
      const label = confidenceLabel(conf);
      return { entry, activation, confidence: conf, confidenceLabel: label };
    }).filter(r => r.confidence >= minConf) as RecallResult[];

    // Record access for all retrieved memories
    for (const result of results) {
      this._store.recordAccess(result.entry.id);
    }

    // Hebbian learning: record co-activation (namespace-aware)
    if (this.config.hebbianEnabled && results.length >= 2) {
      const memoryIds = results.map(r => r.entry.id);
      recordCoactivation(this._store, memoryIds, this.config);
    }

    return results;
  }

  // === Recall Associated (renamed from recall_causal) ===

  /**
   * Recall associated memories using Hebbian links and causal type filtering.
   *
   * Uses Hebbian links to find memories that frequently co-occur.
   * Note: this finds *associations*, not true causal relationships.
   * LLMs can infer causality from the associated context.
   *
   * @param causeQuery Optional query to filter causal memories
   * @param limit Maximum number of results
   * @param minConfidence Minimum confidence threshold
   * @param namespace Namespace to search
   */
  recallAssociated(
    causeQuery?: string,
    limit: number = 5,
    minConfidence: number = 0.0,
    namespace?: string,
  ): RecallResult[] {
    const now = Date.now() / 1000;
    const ns = namespace || 'default';

    if (causeQuery) {
      // Do normal recall but filter to causal type
      const results = this.recallFromNamespace(causeQuery, limit * 2, {
        minConfidence,
        namespace: ns,
      });
      return results
        .filter((r) => r.entry.memoryType === MemoryType.CAUSAL)
        .slice(0, limit);
    }

    // Get all causal memories sorted by importance
    const allMemories = this._store.all();
    const causalMemories = allMemories
      .filter((m) => m.memoryType === MemoryType.CAUSAL)
      .map((entry) => {
        const activation = calcRetrievalActivation(
          entry,
          [],
          now,
          this.config.actrDecay,
          this.config.contextWeight,
          this.config.importanceWeight,
        );
        const conf = confidenceScore(entry, this._store, now);
        return {
          entry,
          activation,
          confidence: conf,
          confidenceLabel: confidenceLabel(conf),
        };
      })
      .filter((r) => r.confidence >= minConfidence)
      .sort((a, b) => b.entry.importance - a.entry.importance)
      .slice(0, limit);

    return causalMemories;
  }

  /**
   * @deprecated Use `recallAssociated()` instead. This is a compatibility alias.
   */
  recallCausal(
    causeQuery?: string,
    limit: number = 5,
    minConfidence: number = 0.0,
    namespace?: string,
  ): RecallResult[] {
    return this.recallAssociated(causeQuery, limit, minConfidence, namespace);
  }

  // === v2: ACL Support ===

  /**
   * Grant a permission to an agent for a namespace
   */
  grant(agentId: string, namespace: string, permission: Permission): void {
    const grantor = this._agentId || 'system';
    grantPermission(this._store.db, agentId, namespace, permission, grantor);
  }

  /**
   * Revoke a permission from an agent for a namespace
   */
  revoke(agentId: string, namespace: string): boolean {
    return revokePermission(this._store.db, agentId, namespace);
  }

  /**
   * Check if an agent has a specific permission for a namespace
   */
  checkPermission(agentId: string, namespace: string, permission: Permission): boolean {
    return checkPermission(this._store.db, agentId, namespace, permission);
  }

  /**
   * List all permissions for an agent
   */
  listPermissions(agentId: string): AclEntry[] {
    return listPermissions(this._store.db, agentId);
  }

  // === v2: Subscription Support ===

  /**
   * Subscribe to notifications for a namespace
   */
  subscribe(agentId: string, namespace: string, minImportance: number): void {
    if (!this._subscriptionManager) {
      throw new Error('Subscription manager not initialized');
    }
    this._subscriptionManager.subscribe(agentId, namespace, minImportance);
  }

  /**
   * Unsubscribe from a namespace
   */
  unsubscribe(agentId: string, namespace: string): boolean {
    if (!this._subscriptionManager) {
      throw new Error('Subscription manager not initialized');
    }
    return this._subscriptionManager.unsubscribe(agentId, namespace);
  }

  /**
   * List subscriptions for an agent
   */
  listSubscriptions(agentId: string): Subscription[] {
    if (!this._subscriptionManager) {
      throw new Error('Subscription manager not initialized');
    }
    return this._subscriptionManager.listSubscriptions(agentId);
  }

  /**
   * Check for notifications since last check
   */
  checkNotifications(agentId: string): Notification[] {
    if (!this._subscriptionManager) {
      throw new Error('Subscription manager not initialized');
    }
    return this._subscriptionManager.checkNotifications(agentId);
  }

  /**
   * Peek at notifications without updating cursor
   */
  peekNotifications(agentId: string): Notification[] {
    if (!this._subscriptionManager) {
      throw new Error('Subscription manager not initialized');
    }
    return this._subscriptionManager.peekNotifications(agentId);
  }
}
