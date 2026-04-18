/**
 * Auto-detect and create embedding provider
 */

import { EmbeddingProvider, EmbeddingConfig } from './base';
import { OpenAIEmbeddingProvider } from './openai';
import { OllamaEmbeddingProvider } from './ollama';
import { MCPEmbeddingProvider } from './mcp';

/**
 * No-op provider (FTS5 only, no embedding)
 */
class NoneProvider implements EmbeddingProvider {
  name = 'none';
  model = 'none';

  async embed(_text: string): Promise<{ embedding: number[]; dimensions: number }> {
    return { embedding: [], dimensions: 0 };
  }

  async isAvailable(): Promise<boolean> {
    return true; // Always available
  }

  async getInfo() {
    return {
      name: 'none',
      model: 'none',
      dimensions: 0,
      available: true,
    };
  }
}

/**
 * Detect available embedding provider
 */
export async function detectProvider(
  config: EmbeddingConfig,
): Promise<EmbeddingProvider> {
  const { provider } = config;

  // Explicit provider requested
  if (provider !== 'auto') {
    switch (provider) {
      case 'openai':
        if (!config.openaiApiKey) {
          throw new Error('OpenAI API key required for openai provider');
        }
        return new OpenAIEmbeddingProvider({
          apiKey: config.openaiApiKey,
          model: config.model,
        });

      case 'ollama':
        return new OllamaEmbeddingProvider({
          model: config.model,
          host: config.ollamaHost,
        });

      case 'mcp':
        return new MCPEmbeddingProvider({
          command: config.mcpConfig?.command,
          args: config.mcpConfig?.args,
          env: config.mcpConfig?.env,
          model: config.model,
        });

      case 'none':
        return new NoneProvider();

      default:
        throw new Error(`Unknown provider: ${provider}`);
    }
  }

  // Auto-detect: try Ollama → MCP → OpenAI → None
  console.log('[Engram] Auto-detecting embedding provider...');

  // Try Ollama
  const ollama = new OllamaEmbeddingProvider({
    model: config.model || 'nomic-embed-text',
    host: config.ollamaHost,
  });
  if (await ollama.isAvailable()) {
    console.log('[Engram] ✅ Using Ollama embedding provider');
    return ollama;
  }

  // Try MCP
  const mcp = new MCPEmbeddingProvider({
    command: config.mcpConfig?.command,
    args: config.mcpConfig?.args,
    env: config.mcpConfig?.env,
  });
  if (await mcp.isAvailable()) {
    console.log('[Engram] ✅ Using MCP embedding provider (Python)');
    return mcp;
  }

  // Try OpenAI
  if (config.openaiApiKey) {
    const openai = new OpenAIEmbeddingProvider({
      apiKey: config.openaiApiKey,
      model: config.model || 'text-embedding-3-small',
    });
    if (await openai.isAvailable()) {
      console.log('[Engram] ✅ Using OpenAI embedding provider');
      return openai;
    }
  }

  // Fallback to None (FTS5 only)
  console.log('[Engram] ⚠️  No embedding provider available, using FTS5 only');
  return new NoneProvider();
}

/**
 * Get info about all available providers
 */
export async function getAvailableProviders(config: EmbeddingConfig): Promise<{
  ollama: boolean;
  mcp: boolean;
  openai: boolean;
  selected: string;
}> {
  const ollama = new OllamaEmbeddingProvider({
    model: config.model,
    host: config.ollamaHost,
  });
  const ollamaAvailable = await ollama.isAvailable();

  const mcp = new MCPEmbeddingProvider({
    command: config.mcpConfig?.command,
    args: config.mcpConfig?.args,
    env: config.mcpConfig?.env,
  });
  const mcpAvailable = await mcp.isAvailable();

  const openaiAvailable = !!config.openaiApiKey;

  let selected = 'none';
  if (config.provider !== 'auto') {
    selected = config.provider;
  } else if (ollamaAvailable) {
    selected = 'ollama';
  } else if (mcpAvailable) {
    selected = 'mcp';
  } else if (openaiAvailable) {
    selected = 'openai';
  }

  return {
    ollama: ollamaAvailable,
    mcp: mcpAvailable,
    openai: openaiAvailable,
    selected,
  };
}
