/**
 * Ollama embedding provider
 */

import { EmbeddingProvider, EmbeddingResult, ProviderInfo } from './base';

export class OllamaEmbeddingProvider implements EmbeddingProvider {
  name = 'ollama';
  model: string;
  private host: string;

  constructor(opts: { model?: string; host?: string }) {
    this.model = opts.model || 'nomic-embed-text';
    this.host = opts.host || 'http://localhost:11434';
  }

  async embed(text: string): Promise<EmbeddingResult> {
    const response = await fetch(`${this.host}/api/embed`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: this.model,
        input: text,
      }),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`Ollama embedding failed: ${response.status} ${error}`);
    }

    const data = await response.json() as { embeddings: number[][] };
    const embedding = data.embeddings[0];

    return {
      embedding,
      dimensions: embedding.length,
    };
  }

  async isAvailable(): Promise<boolean> {
    try {
      const response = await fetch(`${this.host}/api/tags`);
      return response.ok;
    } catch {
      return false;
    }
  }

  async getInfo(): Promise<ProviderInfo> {
    try {
      const available = await this.isAvailable();
      
      if (!available) {
        return {
          name: this.name,
          model: this.model,
          dimensions: 0,
          available: false,
          error: 'Ollama not running',
        };
      }

      // Get model info
      const response = await fetch(`${this.host}/api/show`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: this.model }),
      });

      if (!response.ok) {
        return {
          name: this.name,
          model: this.model,
          dimensions: 0,
          available: false,
          error: `Model ${this.model} not found`,
        };
      }

      const data = await response.json() as { details?: { embedding_length?: number } };
      const dimensions = data.details?.embedding_length || 768; // Default for most models

      return {
        name: this.name,
        model: this.model,
        dimensions,
        available: true,
      };
    } catch (error) {
      return {
        name: this.name,
        model: this.model,
        dimensions: 0,
        available: false,
        error: error instanceof Error ? error.message : 'Unknown error',
      };
    }
  }
}
