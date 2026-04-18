/**
 * Base interfaces for embedding providers
 */

export interface EmbeddingVector {
  id: string;
  vector: number[];
  timestamp: number;
}

export interface EmbeddingResult {
  embedding: number[];
  dimensions: number;
}

export interface SearchResult {
  id: string;
  score: number;
  distance: number;
}

/**
 * Base embedding provider interface
 */
export interface EmbeddingProvider {
  /** Provider name (e.g., "openai", "ollama", "mcp") */
  name: string;
  
  /** Model name (e.g., "text-embedding-3-small") */
  model: string;
  
  /** Generate embedding for a single text */
  embed(text: string): Promise<EmbeddingResult>;
  
  /** Optional: check if provider is available */
  isAvailable?(): Promise<boolean>;
  
  /** Optional: get provider info */
  getInfo?(): Promise<ProviderInfo>;
}

export interface ProviderInfo {
  name: string;
  model: string;
  dimensions: number;
  available: boolean;
  error?: string;
}

/**
 * Configuration for embedding
 */
export interface EmbeddingConfig {
  /** Provider type: "auto", "openai", "ollama", "mcp", "none" */
  provider: string;
  
  /** Model name (provider-specific) */
  model?: string;
  
  /** OpenAI API key (if using OpenAI) */
  openaiApiKey?: string;
  
  /** Ollama host (if using Ollama) */
  ollamaHost?: string;
  
  /** MCP server config (if using MCP) */
  mcpConfig?: {
    command: string;
    args: string[];
    env?: Record<string, string>;
  };
}

/**
 * Default embedding configurations
 */
export const DEFAULT_EMBEDDING_CONFIG: EmbeddingConfig = {
  provider: 'auto',
};
