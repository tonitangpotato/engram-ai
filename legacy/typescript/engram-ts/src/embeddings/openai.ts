/**
 * OpenAI embedding provider
 */

import { EmbeddingProvider, EmbeddingResult, ProviderInfo } from './base';

export class OpenAIEmbeddingProvider implements EmbeddingProvider {
  name = 'openai';
  model: string;
  private apiKey: string;
  private baseUrl: string;

  constructor(opts: { apiKey: string; model?: string; baseUrl?: string }) {
    this.apiKey = opts.apiKey;
    this.model = opts.model || 'text-embedding-3-small';
    this.baseUrl = opts.baseUrl || 'https://api.openai.com/v1';
  }

  async embed(text: string): Promise<EmbeddingResult> {
    const response = await fetch(`${this.baseUrl}/embeddings`, {
      method: 'POST',
      headers: {
        'Authorization': `Bearer ${this.apiKey}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        input: text,
        model: this.model,
      }),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`OpenAI embedding failed: ${response.status} ${error}`);
    }

    const data = await response.json() as { data: Array<{ embedding: number[] }> };
    const embedding = data.data[0].embedding;

    return {
      embedding,
      dimensions: embedding.length,
    };
  }

  async isAvailable(): Promise<boolean> {
    try {
      const response = await fetch(`${this.baseUrl}/models`, {
        headers: { 'Authorization': `Bearer ${this.apiKey}` },
      });
      return response.ok;
    } catch {
      return false;
    }
  }

  async getInfo(): Promise<ProviderInfo> {
    const available = await this.isAvailable();
    
    // text-embedding-3-small: 1536 dims
    // text-embedding-3-large: 3072 dims
    const dimensions = this.model.includes('large') ? 3072 : 1536;

    return {
      name: this.name,
      model: this.model,
      dimensions,
      available,
    };
  }
}
