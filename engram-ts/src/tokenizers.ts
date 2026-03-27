/**
 * Tokenizers for different languages.
 *
 * FTS5 default tokenizer (unicode61) works well for space-delimited languages
 * but fails for CJK (Chinese, Japanese, Korean) which don't use spaces.
 *
 * This module provides:
 * 1. CJK character detection
 * 2. CJK/ASCII boundary insertion
 * 3. Character-level tokenization fallback
 *
 * Note: Unlike the Python/Rust versions, we don't use jieba here.
 * For Node.js, nodejieba exists but adds native dependencies.
 * The boundary-insertion approach works reasonably well for basic CJK search.
 * TODO: Add optional jieba support via nodejieba for better Chinese segmentation.
 */

// CJK Unicode ranges
const CJK_RANGES: Array<[number, number]> = [
  [0x4e00, 0x9fff], // CJK Unified Ideographs
  [0x3400, 0x4dbf], // CJK Unified Ideographs Extension A
  [0xf900, 0xfaff], // CJK Compatibility Ideographs
  [0x3000, 0x303f], // CJK Symbols and Punctuation
  [0x3040, 0x309f], // Hiragana
  [0x30a0, 0x30ff], // Katakana
  [0xac00, 0xd7af], // Hangul Syllables
];

/**
 * Check if a character is CJK (Chinese/Japanese/Korean).
 */
export function isCjkChar(char: string): boolean {
  const code = char.charCodeAt(0);
  return CJK_RANGES.some(([start, end]) => code >= start && code <= end);
}

/**
 * Check if text contains CJK characters.
 */
export function containsCjk(text: string): boolean {
  for (const char of text) {
    if (isCjkChar(char)) return true;
  }
  return false;
}

/**
 * Insert spaces at CJK/ASCII boundaries.
 *
 * This helps FTS5 tokenize mixed CJK/Latin text correctly.
 * e.g. "RustClaw是一个" → "RustClaw 是 一 个"
 * e.g. "用Rust写agent" → "用 Rust 写 agent"
 */
export function insertCjkBoundaries(text: string): string {
  if (!containsCjk(text)) {
    return text; // Fast path: no CJK
  }

  const result: string[] = [];
  let prevWasCjk: boolean | null = null;

  for (const char of text) {
    const isCjk = isCjkChar(char);

    // Insert space at CJK/ASCII boundary
    if (prevWasCjk !== null && prevWasCjk !== isCjk && char !== ' ') {
      result.push(' ');
    }

    result.push(char);
    prevWasCjk = isCjk;
  }

  return result.join('');
}

/**
 * Tokenize CJK text using character-level unigrams.
 *
 * This is a fallback when jieba/sudachi are not available.
 * Works for any CJK language without dependencies.
 */
export function tokenizeCjkCharacters(text: string): string[] {
  const tokens: string[] = [];
  let asciiBuffer = '';

  for (const char of text) {
    if (isCjkChar(char)) {
      // Flush ASCII buffer
      if (asciiBuffer) {
        tokens.push(asciiBuffer);
        asciiBuffer = '';
      }
      // Each CJK character is a token
      tokens.push(char);
    } else if (/\s/.test(char)) {
      // Flush ASCII buffer on whitespace
      if (asciiBuffer) {
        tokens.push(asciiBuffer);
        asciiBuffer = '';
      }
    } else {
      // Accumulate ASCII characters
      asciiBuffer += char;
    }
  }

  // Flush remaining buffer
  if (asciiBuffer) {
    tokens.push(asciiBuffer);
  }

  return tokens;
}

/**
 * Tokenize text for FTS indexing.
 *
 * For CJK text: inserts boundaries and tokenizes characters
 * For non-CJK: returns as-is (FTS5 handles it)
 */
export function tokenizeForFts(text: string): string {
  if (!containsCjk(text)) {
    return text;
  }

  // Insert boundaries, then tokenize
  const withBoundaries = insertCjkBoundaries(text);
  const tokens = tokenizeCjkCharacters(withBoundaries);

  // Filter out empty tokens and single punctuation
  const filtered = tokens.filter(
    (t) => t.length > 0 && !(t.length === 1 && !/[\p{L}\p{N}]/u.test(t))
  );

  return filtered.join(' ');
}

/**
 * Get tokenizer status.
 */
export function getTokenizerStatus(): { jieba: boolean; sudachi: boolean; fallback: string } {
  return {
    jieba: false, // Not implemented in TypeScript yet
    sudachi: false, // Not implemented in TypeScript yet
    fallback: 'character_unigrams',
  };
}
