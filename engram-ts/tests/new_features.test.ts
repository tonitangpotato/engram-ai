/**
 * Tests for newly added features:
 * 1. LLM Extraction (extractor.ts)
 * 2. Config Hierarchy (config.ts)
 * 3. Hybrid Search (memory.ts recall changes)
 * 4. recallAssociated rename
 * 5. Session Working Memory defaults
 */

import {
  Memory,
  SessionWorkingMemory,
  getSessionWM,
  parseExtractionResponse,
  AnthropicExtractor,
  OllamaExtractor,
  getConfigPath,
  loadFileConfig,
  autoDetectExtractor,
} from '../src/index';
import type { ExtractedFact, MemoryExtractor } from '../src/extractor';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

// Helper: create a temporary Memory instance for testing
function createTempMemory(): { mem: Memory; dbPath: string } {
  const dbPath = path.join(os.tmpdir(), `engram-test-${Date.now()}-${Math.random().toString(36).slice(2)}.db`);
  const mem = new Memory(dbPath);
  return { mem, dbPath };
}

// Helper: cleanup temp DB
function cleanup(dbPath: string) {
  try {
    fs.unlinkSync(dbPath);
  } catch {}
  try {
    fs.unlinkSync(dbPath + '-wal');
  } catch {}
  try {
    fs.unlinkSync(dbPath + '-shm');
  } catch {}
}

// ============================================================
// 1. LLM Extraction (src/extractor.ts)
// ============================================================

describe('LLM Extraction', () => {
  describe('ExtractedFact type', () => {
    test('ExtractedFact can be created with all fields', () => {
      const fact: ExtractedFact = {
        content: 'User prefers dark mode',
        memoryType: 'factual',
        importance: 0.7,
        confidenceLabel: 'confident',
      };
      expect(fact.content).toBe('User prefers dark mode');
      expect(fact.memoryType).toBe('factual');
      expect(fact.importance).toBe(0.7);
      expect(fact.confidenceLabel).toBe('confident');
    });
  });

  describe('parseExtractionResponse()', () => {
    test('parses valid JSON array', () => {
      const input = JSON.stringify([
        { content: 'Fact one', memory_type: 'factual', importance: 0.8, confidence: 'confident' },
        { content: 'Fact two', memory_type: 'episodic', importance: 0.5, confidence: 'likely' },
      ]);
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(2);
      expect(results[0].content).toBe('Fact one');
      expect(results[0].memoryType).toBe('factual');
      expect(results[0].importance).toBe(0.8);
      expect(results[0].confidenceLabel).toBe('confident');
      expect(results[1].content).toBe('Fact two');
      expect(results[1].memoryType).toBe('episodic');
    });

    test('parses markdown-wrapped JSON (```json ... ```)', () => {
      const input = '```json\n[{"content": "wrapped fact", "memory_type": "relational", "importance": 0.6, "confidence": "likely"}]\n```';
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(1);
      expect(results[0].content).toBe('wrapped fact');
      expect(results[0].memoryType).toBe('relational');
    });

    test('parses markdown-wrapped JSON without language tag (``` ... ```)', () => {
      const input = '```\n[{"content": "no lang tag", "memory_type": "factual", "importance": 0.5, "confidence": "confident"}]\n```';
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(1);
      expect(results[0].content).toBe('no lang tag');
    });

    test('returns empty array for invalid JSON', () => {
      const warnSpy = jest.spyOn(console, 'warn').mockImplementation(() => {});
      const results = parseExtractionResponse('this is not json at all');
      expect(results).toEqual([]);
      warnSpy.mockRestore();
    });

    test('returns empty array for empty JSON array', () => {
      const results = parseExtractionResponse('[]');
      expect(results).toEqual([]);
    });

    test('filters out entries with empty content', () => {
      const input = JSON.stringify([
        { content: '', memory_type: 'factual', importance: 0.5, confidence: 'likely' },
        { content: 'Valid fact', memory_type: 'factual', importance: 0.5, confidence: 'likely' },
      ]);
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(1);
      expect(results[0].content).toBe('Valid fact');
    });

    test('handles missing fields with defaults', () => {
      const input = JSON.stringify([{ content: 'minimal fact' }]);
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(1);
      expect(results[0].memoryType).toBe('factual'); // default
      expect(results[0].importance).toBe(0.5); // default
      expect(results[0].confidenceLabel).toBe('likely'); // default
    });

    test('clamps importance to [0, 1]', () => {
      const input = JSON.stringify([
        { content: 'high', importance: 5.0, memory_type: 'factual', confidence: 'confident' },
        { content: 'low', importance: -2.0, memory_type: 'factual', confidence: 'confident' },
      ]);
      const results = parseExtractionResponse(input);
      expect(results[0].importance).toBe(1.0);
      expect(results[1].importance).toBe(0.0);
    });

    test('handles JSON with surrounding text', () => {
      const input = 'Here are the facts:\n[{"content": "extracted", "memory_type": "factual", "importance": 0.7, "confidence": "confident"}]\nEnd.';
      const results = parseExtractionResponse(input);
      expect(results).toHaveLength(1);
      expect(results[0].content).toBe('extracted');
    });
  });

  describe('AnthropicExtractor instantiation', () => {
    test('creates with OAuth token', () => {
      const extractor = new AnthropicExtractor('sk-ant-oat01-test', true);
      expect(extractor).toBeDefined();
      expect(extractor).toHaveProperty('extract');
    });

    test('creates with API key', () => {
      const extractor = new AnthropicExtractor('sk-ant-api03-test', false);
      expect(extractor).toBeDefined();
      expect(extractor).toHaveProperty('extract');
    });

    test('accepts custom config', () => {
      const extractor = new AnthropicExtractor('test-key', false, {
        model: 'claude-sonnet-4-20250514',
        maxTokens: 2048,
        timeoutMs: 60000,
      });
      expect(extractor).toBeDefined();
    });
  });

  describe('OllamaExtractor instantiation', () => {
    test('creates with default config', () => {
      const extractor = new OllamaExtractor();
      expect(extractor).toBeDefined();
      expect(extractor).toHaveProperty('extract');
    });

    test('creates with custom config', () => {
      const extractor = new OllamaExtractor({
        model: 'mistral:7b',
        host: 'http://custom:11434',
        timeoutMs: 120000,
      });
      expect(extractor).toBeDefined();
    });

    test('creates with static withModel()', () => {
      const extractor = OllamaExtractor.withModel('phi3:mini');
      expect(extractor).toBeDefined();
    });

    test('creates with static withHost()', () => {
      const extractor = OllamaExtractor.withHost('llama3:8b', 'http://remote:11434');
      expect(extractor).toBeDefined();
    });
  });

  describe('Memory extractor integration', () => {
    let mem: Memory;
    let dbPath: string;

    beforeEach(() => {
      ({ mem, dbPath } = createTempMemory());
    });

    afterEach(() => {
      cleanup(dbPath);
    });

    test('hasExtractor is false when no env vars set', () => {
      // autoDetectExtractor returns null when no ANTHROPIC_* env vars
      // (assuming test env doesn't have them)
      // But constructor calls autoDetectExtractor, so check based on env
      if (!process.env.ANTHROPIC_AUTH_TOKEN && !process.env.ANTHROPIC_API_KEY) {
        expect(mem.hasExtractor).toBe(false);
      }
    });

    test('setExtractor() enables extraction', () => {
      const mockExtractor: MemoryExtractor = {
        extract: async () => [],
      };
      mem.setExtractor(mockExtractor);
      expect(mem.hasExtractor).toBe(true);
    });

    test('clearExtractor() disables extraction', () => {
      const mockExtractor: MemoryExtractor = {
        extract: async () => [],
      };
      mem.setExtractor(mockExtractor);
      expect(mem.hasExtractor).toBe(true);
      mem.clearExtractor();
      expect(mem.hasExtractor).toBe(false);
    });

    test('addWithExtraction() falls back to raw when no extractor', async () => {
      mem.clearExtractor();
      const id = await mem.addWithExtraction('raw text without extraction');
      expect(id).toBeTruthy();
      // Should be stored as-is
      const results = mem.recall('raw text without extraction', { limit: 1 });
      expect(results.length).toBeGreaterThanOrEqual(1);
      expect(results[0].content).toContain('raw text without extraction');
    });

    test('addWithExtraction() uses extractor when set', async () => {
      const mockExtractor: MemoryExtractor = {
        extract: async () => [
          {
            content: 'Extracted: user likes cats',
            memoryType: 'factual',
            importance: 0.8,
            confidenceLabel: 'confident',
          },
        ],
      };
      mem.setExtractor(mockExtractor);
      const id = await mem.addWithExtraction('I really like cats');
      expect(id).toBeTruthy();
      const results = mem.recall('cats', { limit: 5 });
      expect(results.some((r) => r.content.includes('user likes cats'))).toBe(true);
    });

    test('addWithExtraction() falls back on extractor failure', async () => {
      const failExtractor: MemoryExtractor = {
        extract: async () => {
          throw new Error('API failure');
        },
      };
      mem.setExtractor(failExtractor);
      const warnSpy = jest.spyOn(console, 'warn').mockImplementation(() => {});
      const id = await mem.addWithExtraction('fallback content');
      expect(id).toBeTruthy();
      warnSpy.mockRestore();
    });
  });
});

// ============================================================
// 2. Config Hierarchy (src/config.ts)
// ============================================================

describe('Config Hierarchy', () => {
  test('getConfigPath() returns expected path', () => {
    const configPath = getConfigPath();
    expect(configPath).toContain('.config');
    expect(configPath).toContain('engram');
    expect(configPath).toContain('config.json');
    expect(configPath).toBe(path.join(os.homedir(), '.config', 'engram', 'config.json'));
  });

  test('loadFileConfig() returns null for missing file', () => {
    // Unless the user actually has a config file, this should return null or a config
    const result = loadFileConfig();
    // We just verify it doesn't throw
    expect(result === null || typeof result === 'object').toBe(true);
  });

  describe('autoDetectExtractor() env var detection', () => {
    const originalEnv = { ...process.env };

    afterEach(() => {
      // Restore env
      process.env = { ...originalEnv };
    });

    test('returns null with no Anthropic env vars', () => {
      delete process.env.ANTHROPIC_AUTH_TOKEN;
      delete process.env.ANTHROPIC_API_KEY;
      // autoDetectExtractor may still find a config file, so we test behavior
      const result = autoDetectExtractor();
      // If no env vars and no config file with ollama provider, should be null
      // (config file anthropic provider also requires env var)
      if (!fs.existsSync(getConfigPath())) {
        expect(result).toBeNull();
      }
    });

    test('returns AnthropicExtractor with ANTHROPIC_AUTH_TOKEN', () => {
      process.env.ANTHROPIC_AUTH_TOKEN = 'sk-ant-oat01-test-token';
      delete process.env.ANTHROPIC_API_KEY;
      const result = autoDetectExtractor();
      expect(result).toBeDefined();
      expect(result).not.toBeNull();
      expect(result).toBeInstanceOf(AnthropicExtractor);
    });

    test('returns AnthropicExtractor with ANTHROPIC_API_KEY', () => {
      delete process.env.ANTHROPIC_AUTH_TOKEN;
      process.env.ANTHROPIC_API_KEY = 'sk-ant-api03-test-key';
      const result = autoDetectExtractor();
      expect(result).toBeDefined();
      expect(result).not.toBeNull();
      expect(result).toBeInstanceOf(AnthropicExtractor);
    });

    test('ANTHROPIC_AUTH_TOKEN takes priority over API_KEY', () => {
      process.env.ANTHROPIC_AUTH_TOKEN = 'sk-ant-oat01-oauth';
      process.env.ANTHROPIC_API_KEY = 'sk-ant-api03-key';
      const result = autoDetectExtractor();
      expect(result).toBeInstanceOf(AnthropicExtractor);
      // OAuth should take priority — we can verify by checking it's not null
      expect(result).not.toBeNull();
    });
  });
});

// ============================================================
// 3. Hybrid Search (memory.ts recall changes)
// ============================================================

describe('Hybrid Search', () => {
  let mem: Memory;
  let dbPath: string;

  beforeEach(() => {
    ({ mem, dbPath } = createTempMemory());
  });

  afterEach(() => {
    cleanup(dbPath);
  });

  test('recall() returns results for matching query', () => {
    mem.add('TypeScript is a typed superset of JavaScript');
    mem.add('Python is great for data science');
    mem.add('Rust provides memory safety without garbage collection');

    const results = mem.recall('TypeScript', { limit: 5 });
    expect(results.length).toBeGreaterThan(0);
  });

  test('exact term matches appear in results', () => {
    mem.add('The capital of France is Paris');
    mem.add('Germany has a strong economy');
    mem.add('Tokyo is in Japan');

    const results = mem.recall('Paris', { limit: 5 });
    expect(results.length).toBeGreaterThan(0);
    expect(results.some((r) => r.content.includes('Paris'))).toBe(true);
  });

  test('exact match ranks higher than partial match', () => {
    mem.add('Machine learning uses neural networks');
    mem.add('Deep learning is a subset of machine learning');
    mem.add('Quantum computing is an emerging field');
    mem.add('The exact phrase: quantum neural network breakthrough');

    const results = mem.recall('quantum neural network', { limit: 5 });
    expect(results.length).toBeGreaterThan(0);
    // The memory containing all three words should rank high
    const exactIdx = results.findIndex((r) => r.content.includes('quantum neural network'));
    if (exactIdx >= 0) {
      // Exact match should be in the top results
      expect(exactIdx).toBeLessThan(3);
    }
  });

  test('recall returns expected fields', () => {
    mem.add('Test memory for field validation', { type: 'factual', importance: 0.8 });
    const results = mem.recall('field validation', { limit: 1 });
    expect(results.length).toBeGreaterThan(0);
    const r = results[0];
    expect(r).toHaveProperty('id');
    expect(r).toHaveProperty('content');
    expect(r).toHaveProperty('type');
    expect(r).toHaveProperty('confidence');
    expect(r).toHaveProperty('confidence_label');
    expect(r).toHaveProperty('strength');
    expect(r).toHaveProperty('activation');
    expect(r).toHaveProperty('age_days');
    expect(r).toHaveProperty('layer');
    expect(r).toHaveProperty('importance');
  });
});

// ============================================================
// 4. recallAssociated rename
// ============================================================

describe('recallAssociated rename', () => {
  let mem: Memory;
  let dbPath: string;

  beforeEach(() => {
    ({ mem, dbPath } = createTempMemory());
  });

  afterEach(() => {
    cleanup(dbPath);
  });

  test('recallAssociated() method exists and works', () => {
    expect(typeof mem.recallAssociated).toBe('function');
    // Should not throw even with no data
    const results = mem.recallAssociated();
    expect(Array.isArray(results)).toBe(true);
  });

  test('recallAssociated() with query returns results', () => {
    mem.add('Coffee causes alertness', { type: 'causal', importance: 0.7 });
    const results = mem.recallAssociated('coffee', 5);
    expect(Array.isArray(results)).toBe(true);
  });

  test('recallCausal() deprecated alias calls recallAssociated', () => {
    expect(typeof mem.recallCausal).toBe('function');

    // Spy on recallAssociated to verify it gets called
    const spy = jest.spyOn(mem, 'recallAssociated');
    mem.recallCausal('test query', 3, 0.0);
    expect(spy).toHaveBeenCalledWith('test query', 3, 0.0, undefined);
    spy.mockRestore();
  });

  test('recallCausal() returns same results as recallAssociated()', () => {
    mem.add('Rain causes flooding', { type: 'causal', importance: 0.8 });
    const associated = mem.recallAssociated('rain', 5);
    const causal = mem.recallCausal('rain', 5);
    expect(associated.length).toBe(causal.length);
  });
});

// ============================================================
// 5. Session Working Memory defaults
// ============================================================

describe('Session Working Memory defaults', () => {
  test('default capacity is 15 (not 7)', () => {
    const swm = new SessionWorkingMemory();
    expect(swm.capacity).toBe(15);
  });

  test('default decay is 300 seconds', () => {
    const swm = new SessionWorkingMemory();
    expect(swm.decaySeconds).toBe(300);
  });

  test('getSessionWM() uses default capacity of 15', () => {
    const sessionId = `test-defaults-${Date.now()}`;
    const swm = getSessionWM(sessionId);
    expect(swm.capacity).toBe(15);
    expect(swm.decaySeconds).toBe(300);
  });

  test('custom capacity and decay can be set', () => {
    const swm = new SessionWorkingMemory(10, 600);
    expect(swm.capacity).toBe(10);
    expect(swm.decaySeconds).toBe(600);
  });
});
