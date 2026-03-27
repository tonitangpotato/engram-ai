export { MemoryEntry, MemoryType, MemoryLayer, DEFAULT_DECAY_RATES, DEFAULT_IMPORTANCE } from './core';
export { MemoryConfig } from './config';
export { SQLiteStore } from './store';
export { Memory } from './memory';
export { baseLevelActivation, spreadingActivation, retrievalActivation, retrieveTopK } from './activation';
export { retrievability, computeStability, effectiveStrength, shouldForget, pruneforgotten, retrievalInducedForgetting } from './forgetting';
export { applyDecay, consolidateSingle, runConsolidationCycle, getConsolidationStats } from './consolidation';
export { contentReliability, retrievalSalience, confidenceScore, confidenceLabel, confidenceDetail } from './confidence';
export { detectFeedback, applyReward } from './reward';
export { synapticDownscale } from './downscaling';
export { BaselineTracker } from './anomaly';
export { SearchEngine, SearchResult } from './search';
export { recordCoactivation, maybeCreateLink, getHebbianNeighbors, decayHebbianLinks, strengthenLink, getAllHebbianLinks } from './hebbian';
export { SessionWorkingMemory, SessionRecallResult, getSessionWM, clearSession, listSessions } from './session_wm';

// v2: Namespace, ACL, Emotional Bus, Subscriptions
export { Permission, AclEntry, CrossLink, RecallResult, RecallWithAssociationsResult } from './types';
export { initAclTables, grantPermission, revokePermission, checkPermission, listPermissions } from './acl';
export { Subscription, Notification, SubscriptionManager } from './subscriptions';
export { EmotionalBus, SoulUpdate, HeartbeatUpdate } from './bus/index';
export { Drive, HeartbeatTask, Identity, extractKeywords, parseSoul, parseHeartbeat, parseIdentity, readSoul, readHeartbeat, readIdentity, updateSoulField, addSoulDrive, updateHeartbeatTask, addHeartbeatTask } from './bus/mod_io';
export { EmotionalAccumulator, EmotionalTrend, NEGATIVE_THRESHOLD, MIN_EVENTS_FOR_SUGGESTION, needsSoulUpdate, describeTrend } from './bus/accumulator';
export { scoreAlignment, calculateImportanceBoost, isStronglyAligned, findAlignedDrives, ALIGNMENT_BOOST } from './bus/alignment';
export { BehaviorFeedback, BehaviorLog, ActionStats, DEFAULT_SCORE_WINDOW, LOW_SCORE_THRESHOLD, MIN_ATTEMPTS_FOR_SUGGESTION, shouldDeprioritize, describeStats } from './bus/feedback';

// Embedding exports (v1.0.0)
export { EmbeddingProvider, EmbeddingConfig, EmbeddingResult, ProviderInfo, DEFAULT_EMBEDDING_CONFIG } from './embeddings/base';
export { OpenAIEmbeddingProvider } from './embeddings/openai';
export { OllamaEmbeddingProvider } from './embeddings/ollama';
export { MCPEmbeddingProvider } from './embeddings/mcp';
export { detectProvider, getAvailableProviders } from './embeddings/provider_detection';
export { cosineSimilarity, vectorSearch, VectorSearchResult, migrateVectorColumn, storeVector, getVector, getVectorCount } from './vector_search';
export { hybridSearch, adaptiveHybridSearch, HybridSearchResult } from './hybrid_search';

// Extractor (LLM-based fact extraction)
export { MemoryExtractor, ExtractedFact, AnthropicExtractor, AnthropicExtractorConfig, OllamaExtractor, OllamaExtractorConfig, parseExtractionResponse, autoDetectExtractor } from './extractor';

// Config file hierarchy
export { EngramFileConfig, getConfigPath, loadFileConfig, saveFileConfig, interactiveConfigSetup } from './config';

// CJK Tokenization
export { isCjkChar, containsCjk, insertCjkBoundaries, tokenizeCjkCharacters, tokenizeForFts, getTokenizerStatus } from './tokenizers';
