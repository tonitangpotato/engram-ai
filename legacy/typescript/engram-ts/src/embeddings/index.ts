/**
 * Embedding providers for Engram
 */

export { EmbeddingProvider, EmbeddingConfig, EmbeddingResult, ProviderInfo, DEFAULT_EMBEDDING_CONFIG } from './base';
export { OpenAIEmbeddingProvider } from './openai';
export { OllamaEmbeddingProvider } from './ollama';
export { MCPEmbeddingProvider } from './mcp';
export { detectProvider, getAvailableProviders } from './provider_detection';
