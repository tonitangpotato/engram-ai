/**
 * LLM-based memory extraction.
 *
 * Converts raw text into structured facts using LLMs. Optional feature
 * that preserves backward compatibility — if no extractor is set,
 * memories are stored as-is.
 *
 * Ported from Rust: engram-ai-rust/src/extractor.rs
 */

/**
 * A single extracted fact from a conversation.
 */
export interface ExtractedFact {
  /** The extracted fact content (self-contained, understandable without context) */
  content: string;
  /** Memory type classification: "factual", "episodic", "relational", "procedural", "emotional", "opinion", "causal" */
  memoryType: string;
  /** Importance score (0.0 - 1.0) */
  importance: number;
  /** Confidence level: "confident", "likely", "uncertain" */
  confidenceLabel: string;
}

/**
 * Interface for memory extraction — converts raw text into structured facts.
 *
 * Implement this interface to use different LLM backends for extraction.
 */
export interface MemoryExtractor {
  /**
   * Extract key facts from raw conversation text.
   *
   * Returns empty array if nothing worth remembering.
   * Throws if the extraction fails (network, parsing, etc.).
   */
  extract(text: string): Promise<ExtractedFact[]>;
}

/** The extraction prompt template. */
const EXTRACTION_PROMPT = `You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should be self-contained (understandable without context)
- Skip greetings, filler, acknowledgments
- Classify each fact: factual, episodic, relational, procedural, emotional, opinion, causal
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement, clear fact), "likely" (reasonable inference), "uncertain" (vague mention, speculation)
- If nothing worth remembering, return empty array
- Respond in the SAME LANGUAGE as the input

Respond with ONLY a JSON array (no markdown, no explanation):
[{"content": "...", "memory_type": "...", "importance": 0.X, "confidence": "confident|likely|uncertain"}]

Conversation:
`;

/**
 * Configuration for Anthropic-based extraction.
 */
export interface AnthropicExtractorConfig {
  /** Model to use (default: "claude-haiku-4-5-20251001") */
  model?: string;
  /** API base URL (default: "https://api.anthropic.com") */
  apiUrl?: string;
  /** Maximum tokens for response (default: 1024) */
  maxTokens?: number;
  /** Request timeout in ms (default: 30000) */
  timeoutMs?: number;
}

const ANTHROPIC_DEFAULTS: Required<AnthropicExtractorConfig> = {
  model: 'claude-haiku-4-5-20251001',
  apiUrl: 'https://api.anthropic.com',
  maxTokens: 1024,
  timeoutMs: 30000,
};

/**
 * Extracts facts using Anthropic Claude API.
 *
 * Supports both OAuth tokens (Claude Max) and API keys.
 * Haiku is recommended for cost/speed balance.
 */
export class AnthropicExtractor implements MemoryExtractor {
  private config: Required<AnthropicExtractorConfig>;
  private authToken: string;
  private isOAuth: boolean;

  /**
   * Create a new AnthropicExtractor.
   *
   * @param authToken API key or OAuth token
   * @param isOAuth True if using OAuth token (Claude Max), false for API key
   * @param config Optional configuration overrides
   */
  constructor(authToken: string, isOAuth: boolean, config?: AnthropicExtractorConfig) {
    this.authToken = authToken;
    this.isOAuth = isOAuth;
    this.config = { ...ANTHROPIC_DEFAULTS, ...config };
  }

  /**
   * Build request headers based on auth type.
   */
  private buildHeaders(): Record<string, string> {
    const headers: Record<string, string> = {
      'anthropic-version': '2023-06-01',
      'content-type': 'application/json',
    };

    if (this.isOAuth) {
      // OAuth mode — mimic Claude Code stealth headers
      headers['anthropic-beta'] = 'claude-code-20250219,oauth-2025-04-20';
      headers['authorization'] = `Bearer ${this.authToken}`;
      headers['user-agent'] = 'claude-cli/2.1.39';
      headers['x-app'] = 'cli';
      headers['anthropic-dangerous-direct-browser-access'] = 'true';
    } else {
      // API key mode
      headers['x-api-key'] = this.authToken;
    }

    return headers;
  }

  async extract(text: string): Promise<ExtractedFact[]> {
    const prompt = EXTRACTION_PROMPT + text;

    const body = {
      model: this.config.model,
      max_tokens: this.config.maxTokens,
      messages: [{ role: 'user', content: prompt }],
    };

    const url = `${this.config.apiUrl}/v1/messages`;

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.config.timeoutMs);

    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: this.buildHeaders(),
        body: JSON.stringify(body),
        signal: controller.signal,
      });

      if (!response.ok) {
        const errorBody = await response.text().catch(() => '');
        throw new Error(`Anthropic API error ${response.status}: ${errorBody}`);
      }

      const responseJson = await response.json() as any;

      const contentText = responseJson?.content?.[0]?.text;
      if (typeof contentText !== 'string') {
        throw new Error('Invalid response structure from Anthropic API');
      }

      return parseExtractionResponse(contentText);
    } finally {
      clearTimeout(timeout);
    }
  }
}

/**
 * Configuration for Ollama-based extraction.
 */
export interface OllamaExtractorConfig {
  /** Ollama host URL (default: "http://localhost:11434") */
  host?: string;
  /** Model to use (default: "llama3.2:3b") */
  model?: string;
  /** Request timeout in ms (default: 60000) */
  timeoutMs?: number;
}

const OLLAMA_DEFAULTS: Required<OllamaExtractorConfig> = {
  host: 'http://localhost:11434',
  model: 'llama3.2:3b',
  timeoutMs: 60000,
};

/**
 * Extracts facts using a local Ollama chat model.
 *
 * Useful for local/private extraction without API costs.
 */
export class OllamaExtractor implements MemoryExtractor {
  private config: Required<OllamaExtractorConfig>;

  /**
   * Create a new OllamaExtractor.
   *
   * @param config Optional configuration overrides
   */
  constructor(config?: OllamaExtractorConfig) {
    this.config = { ...OLLAMA_DEFAULTS, ...config };
  }

  /**
   * Create with just model name.
   */
  static withModel(model: string): OllamaExtractor {
    return new OllamaExtractor({ model });
  }

  /**
   * Create with model and host.
   */
  static withHost(model: string, host: string): OllamaExtractor {
    return new OllamaExtractor({ model, host });
  }

  async extract(text: string): Promise<ExtractedFact[]> {
    const prompt = EXTRACTION_PROMPT + text;

    const body = {
      model: this.config.model,
      messages: [{ role: 'user', content: prompt }],
      stream: false,
    };

    const url = `${this.config.host}/api/chat`;

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.config.timeoutMs);

    try {
      const response = await fetch(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
        signal: controller.signal,
      });

      if (!response.ok) {
        const errorBody = await response.text().catch(() => '');
        throw new Error(`Ollama API error ${response.status}: ${errorBody}`);
      }

      const responseJson = await response.json() as any;

      const contentText = responseJson?.message?.content;
      if (typeof contentText !== 'string') {
        throw new Error('Invalid response structure from Ollama API');
      }

      return parseExtractionResponse(contentText);
    } finally {
      clearTimeout(timeout);
    }
  }
}

/**
 * Parse LLM extraction response into ExtractedFacts.
 *
 * Handles common LLM quirks:
 * - Markdown-wrapped JSON (```json ... ```)
 * - Extra whitespace
 * - Invalid JSON (returns empty array with warning)
 */
export function parseExtractionResponse(content: string): ExtractedFact[] {
  let jsonStr = content.trim();

  // Strip markdown code blocks if present
  if (jsonStr.startsWith('```json')) {
    jsonStr = jsonStr.slice(7);
  } else if (jsonStr.startsWith('```')) {
    jsonStr = jsonStr.slice(3);
  }
  if (jsonStr.endsWith('```')) {
    jsonStr = jsonStr.slice(0, -3);
  }
  jsonStr = jsonStr.trim();

  // Handle empty array case
  if (jsonStr === '[]') {
    return [];
  }

  // Try to find JSON array in the response
  const jsonStart = jsonStr.indexOf('[');
  const jsonEnd = jsonStr.lastIndexOf(']');

  if (jsonStart === -1 || jsonEnd === -1 || jsonStart >= jsonEnd) {
    console.warn('No JSON array found in extraction response:', jsonStr.slice(0, 100));
    return [];
  }

  const jsonToParse = jsonStr.slice(jsonStart, jsonEnd + 1);

  try {
    const raw = JSON.parse(jsonToParse) as any[];

    return raw
      .map((item: any) => ({
        content: String(item.content ?? ''),
        memoryType: String(item.memory_type ?? 'factual').toLowerCase(),
        importance: Math.max(0, Math.min(1, Number(item.importance ?? 0.5))),
        confidenceLabel: String(item.confidence ?? 'likely'),
      }))
      .filter((f) => f.content.length > 0);
  } catch (e) {
    console.warn('Failed to parse extraction JSON:', e, '- content:', jsonToParse.slice(0, 100));
    return [];
  }
}

/**
 * Auto-detect and create an extractor from environment variables and config file.
 *
 * Detection order (high → low priority):
 * 1. ANTHROPIC_AUTH_TOKEN env var → AnthropicExtractor with OAuth
 * 2. ANTHROPIC_API_KEY env var → AnthropicExtractor with API key
 * 3. ~/.config/engram/config.json extractor section
 * 4. null → no extraction (backward compatible)
 *
 * Model can be overridden via ENGRAM_EXTRACTOR_MODEL env var.
 */
export function autoDetectExtractor(): MemoryExtractor | null {
  const model = process.env.ENGRAM_EXTRACTOR_MODEL || 'claude-haiku-4-5-20251001';

  // Check ANTHROPIC_AUTH_TOKEN first (OAuth mode)
  const oauthToken = process.env.ANTHROPIC_AUTH_TOKEN;
  if (oauthToken) {
    return new AnthropicExtractor(oauthToken, true, { model });
  }

  // Check ANTHROPIC_API_KEY (API key mode)
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (apiKey) {
    return new AnthropicExtractor(apiKey, false, { model });
  }

  // Check config file
  return loadExtractorFromConfig();
}

/**
 * Load extractor configuration from ~/.config/engram/config.json.
 *
 * The config file stores non-sensitive settings (provider, model, host).
 * Auth tokens MUST come from environment variables or code.
 */
function loadExtractorFromConfig(): MemoryExtractor | null {
  try {
    const os = require('os');
    const fs = require('fs');
    const path = require('path');

    const configPath = path.join(os.homedir(), '.config', 'engram', 'config.json');
    if (!fs.existsSync(configPath)) return null;

    const content = fs.readFileSync(configPath, 'utf-8');
    const config = JSON.parse(content);

    const extractorConfig = config?.extractor;
    if (!extractorConfig?.provider) return null;

    switch (extractorConfig.provider) {
      case 'anthropic': {
        // Still need env var for auth — config file NEVER stores tokens
        const token = process.env.ANTHROPIC_AUTH_TOKEN || process.env.ANTHROPIC_API_KEY;
        if (!token) return null;
        const isOAuth = Boolean(process.env.ANTHROPIC_AUTH_TOKEN);
        const model = extractorConfig.model || 'claude-haiku-4-5-20251001';
        return new AnthropicExtractor(token, isOAuth, { model });
      }
      case 'ollama': {
        const model = extractorConfig.model || 'llama3.2:3b';
        const host = extractorConfig.host || 'http://localhost:11434';
        return new OllamaExtractor({ model, host });
      }
      default:
        console.warn('Unknown extractor provider in config:', extractorConfig.provider);
        return null;
    }
  } catch {
    return null;
  }
}
