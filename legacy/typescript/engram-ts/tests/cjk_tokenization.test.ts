/**
 * Tests for CJK tokenization support.
 *
 * Verifies that Chinese/Japanese/Korean text is properly tokenized
 * for FTS5 search, matching the Rust implementation behavior.
 */

import { Memory } from '../src/memory';
import { MemoryConfig } from '../src/config';
import {
  isCjkChar,
  containsCjk,
  insertCjkBoundaries,
  tokenizeCjkCharacters,
  tokenizeForFts,
  getTokenizerStatus,
} from '../src/tokenizers';

describe('CJK Tokenization', () => {
  describe('isCjkChar', () => {
    test('returns false for ASCII characters', () => {
      expect(isCjkChar('a')).toBe(false);
      expect(isCjkChar('Z')).toBe(false);
      expect(isCjkChar('0')).toBe(false);
      expect(isCjkChar(' ')).toBe(false);
    });

    test('returns true for Chinese characters', () => {
      expect(isCjkChar('是')).toBe(true);
      expect(isCjkChar('一')).toBe(true);
      expect(isCjkChar('个')).toBe(true);
    });

    test('returns true for Japanese characters', () => {
      expect(isCjkChar('あ')).toBe(true); // Hiragana
      expect(isCjkChar('ア')).toBe(true); // Katakana
    });

    test('returns true for Korean characters', () => {
      expect(isCjkChar('한')).toBe(true); // Hangul
      expect(isCjkChar('글')).toBe(true);
    });
  });

  describe('containsCjk', () => {
    test('returns false for pure ASCII text', () => {
      expect(containsCjk('Hello world')).toBe(false);
      expect(containsCjk('RustClaw is an AI agent framework')).toBe(false);
    });

    test('returns true for mixed CJK/ASCII text', () => {
      expect(containsCjk('RustClaw是一个')).toBe(true);
      expect(containsCjk('Hello 世界')).toBe(true);
    });

    test('returns true for pure CJK text', () => {
      expect(containsCjk('这是纯中文')).toBe(true);
      expect(containsCjk('こんにちは')).toBe(true);
    });
  });

  describe('insertCjkBoundaries', () => {
    test('returns unmodified text for ASCII only', () => {
      expect(insertCjkBoundaries('Hello world')).toBe('Hello world');
    });

    test('inserts spaces at CJK/ASCII boundaries', () => {
      const result = insertCjkBoundaries('RustClaw是一个');
      expect(result).toContain(' ');
      expect(result.startsWith('RustClaw')).toBe(true);
    });

    test('handles mixed content', () => {
      const result = insertCjkBoundaries('用Rust写agent');
      expect(result).toContain(' Rust ');
      expect(result).toContain(' agent');
    });
  });

  describe('tokenizeForFts', () => {
    test('returns ASCII text unchanged', () => {
      expect(tokenizeForFts('Hello world')).toBe('Hello world');
    });

    test('tokenizes Chinese text into characters', () => {
      const result = tokenizeForFts('是一个');
      expect(result).toContain('是');
      expect(result).toContain('一');
      expect(result).toContain('个');
    });

    test('handles mixed CJK/ASCII', () => {
      const result = tokenizeForFts('RustClaw是一个AI agent');
      // Should have separate tokens for ASCII and CJK
      expect(result).toContain('RustClaw');
      expect(result).toContain('AI');
      expect(result).toContain('agent');
      expect(result).toContain('是');
    });
  });

  describe('getTokenizerStatus', () => {
    test('returns status object', () => {
      const status = getTokenizerStatus();
      expect(status.fallback).toBe('character_unigrams');
      expect(typeof status.jieba).toBe('boolean');
      expect(typeof status.sudachi).toBe('boolean');
    });
  });
});

describe('Search Weights Configuration', () => {
  test('default config has correct search weights', () => {
    const config = MemoryConfig.default();
    expect(config.ftsWeight).toBe(0.15);
    expect(config.embeddingWeight).toBe(0.60);
    expect(config.actrWeight).toBe(0.25);
    // Verify they sum to 1.0
    expect(config.ftsWeight + config.embeddingWeight + config.actrWeight).toBe(1.0);
  });

  test('presets have search weights', () => {
    const chatbot = MemoryConfig.chatbot();
    expect(chatbot.ftsWeight).toBe(0.15);
    expect(chatbot.embeddingWeight).toBe(0.60);
    expect(chatbot.actrWeight).toBe(0.25);
  });

  test('custom weights can be set', () => {
    const config = new MemoryConfig({
      ftsWeight: 0.3,
      embeddingWeight: 0.5,
      actrWeight: 0.2,
    });
    expect(config.ftsWeight).toBe(0.3);
    expect(config.embeddingWeight).toBe(0.5);
    expect(config.actrWeight).toBe(0.2);
  });
});

describe('Memory with CJK Content', () => {
  let mem: Memory;

  beforeEach(() => {
    mem = new Memory(':memory:');
  });

  afterEach(() => {
    mem.close();
  });

  test('can add and recall CJK content', () => {
    const id = mem.add('RustClaw是一个AI agent框架', {
      type: 'factual',
      importance: 0.8,
    });
    expect(id).toBeTruthy();

    const results = mem.recall('AI框架', { limit: 3 });
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].content).toContain('RustClaw');
  });

  test('can search with pure Chinese query', () => {
    mem.add('今天天气很好，适合出门散步', {
      type: 'factual',
      importance: 0.5,
    });

    const results = mem.recall('天气', { limit: 3 });
    expect(results.length).toBeGreaterThan(0);
    expect(results[0].content).toContain('天气');
  });

  test('can search mixed language memories', () => {
    mem.add('Claude是Anthropic开发的AI助手', { type: 'factual' });
    mem.add('OpenAI developed GPT models', { type: 'factual' });

    const results = mem.recall('AI助手', { limit: 3 });
    expect(results.length).toBeGreaterThan(0);
    expect(results.some(r => r.content.includes('Claude'))).toBe(true);
  });
});
