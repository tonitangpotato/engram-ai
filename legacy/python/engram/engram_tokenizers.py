"""
Tokenizers for different languages.

FTS5 default tokenizer (unicode61) works well for space-delimited languages
but fails for CJK (Chinese, Japanese, Korean) which don't use spaces.

This module provides:
1. Language detection
2. Language-specific tokenization
3. Fallback to character n-grams when no tokenizer is available
"""

import re
from typing import Optional

# CJK Unicode ranges
CJK_RANGES = [
    (0x4E00, 0x9FFF),    # CJK Unified Ideographs
    (0x3400, 0x4DBF),    # CJK Unified Ideographs Extension A
    (0x3000, 0x303F),    # CJK Symbols and Punctuation
    (0x3040, 0x309F),    # Hiragana
    (0x30A0, 0x30FF),    # Katakana
    (0xAC00, 0xD7AF),    # Hangul Syllables
]

# Check for optional tokenizer packages
_jieba_available = False
_sudachi_available = False

try:
    import jieba
    _jieba_available = True
except ImportError:
    pass

try:
    from sudachipy import tokenizer as sudachi_tokenizer
    from sudachipy import dictionary as sudachi_dict
    _sudachi_available = True
except ImportError:
    pass


def is_cjk_char(char: str) -> bool:
    """Check if a character is CJK."""
    code = ord(char)
    return any(start <= code <= end for start, end in CJK_RANGES)


def contains_cjk(text: str) -> bool:
    """Check if text contains CJK characters."""
    return any(is_cjk_char(c) for c in text)


def detect_language(text: str) -> str:
    """
    Simple language detection based on character ranges.
    
    Returns: 'zh' (Chinese), 'ja' (Japanese), 'ko' (Korean), 'en' (default)
    """
    cjk_count = sum(1 for c in text if is_cjk_char(c))
    
    if cjk_count == 0:
        return "en"
    
    # Check for Japanese-specific characters (hiragana/katakana)
    hiragana = sum(1 for c in text if 0x3040 <= ord(c) <= 0x309F)
    katakana = sum(1 for c in text if 0x30A0 <= ord(c) <= 0x30FF)
    
    if hiragana + katakana > cjk_count * 0.1:
        return "ja"
    
    # Check for Korean-specific characters (Hangul)
    hangul = sum(1 for c in text if 0xAC00 <= ord(c) <= 0xD7AF)
    
    if hangul > cjk_count * 0.3:
        return "ko"
    
    return "zh"


def tokenize_chinese(text: str) -> list[str]:
    """
    Tokenize Chinese text using jieba.
    Falls back to character-level if jieba not available.
    """
    if _jieba_available:
        return list(jieba.cut(text, cut_all=False))
    else:
        return tokenize_cjk_fallback(text)


def tokenize_japanese(text: str) -> list[str]:
    """
    Tokenize Japanese text using sudachi.
    Falls back to character-level if sudachi not available.
    """
    if _sudachi_available:
        tokenizer_obj = sudachi_dict.Dictionary().create()
        mode = sudachi_tokenizer.Tokenizer.SplitMode.C
        return [m.surface() for m in tokenizer_obj.tokenize(text, mode)]
    else:
        return tokenize_cjk_fallback(text)


def tokenize_cjk_fallback(text: str) -> list[str]:
    """
    Fallback CJK tokenization using character unigrams and bigrams.
    Works for any CJK language without dependencies.
    """
    tokens = []
    cjk_buffer = []
    
    for char in text:
        if is_cjk_char(char):
            cjk_buffer.append(char)
        else:
            if cjk_buffer:
                # Emit unigrams and bigrams for CJK sequence
                tokens.extend(cjk_buffer)  # Unigrams
                for i in range(len(cjk_buffer) - 1):
                    tokens.append(cjk_buffer[i] + cjk_buffer[i + 1])  # Bigrams
                cjk_buffer = []
            # Keep non-CJK as single token if it's a word char
            if char.isalnum():
                tokens.append(char)
    
    # Flush remaining CJK
    if cjk_buffer:
        tokens.extend(cjk_buffer)
        for i in range(len(cjk_buffer) - 1):
            tokens.append(cjk_buffer[i] + cjk_buffer[i + 1])
    
    return tokens


def tokenize(text: str, lang: Optional[str] = None) -> list[str]:
    """
    Tokenize text for FTS indexing.
    
    Auto-detects language if not specified.
    Returns space-joined tokens for FTS5 indexing.
    
    Args:
        text: Text to tokenize
        lang: Language code ('zh', 'ja', 'ko', 'en') or None for auto-detect
        
    Returns:
        List of tokens
    """
    if lang is None:
        lang = detect_language(text)
    
    if lang == "zh":
        return tokenize_chinese(text)
    elif lang == "ja":
        return tokenize_japanese(text)
    elif lang == "ko":
        # Korean: use character fallback (mecab-ko would be better)
        return tokenize_cjk_fallback(text)
    else:
        # English/European: split on whitespace and punctuation
        return re.findall(r'\w+', text.lower())


def tokenize_for_fts(text: str, lang: Optional[str] = None) -> str:
    """
    Tokenize and return as space-separated string for FTS5.
    
    This is what gets indexed in the FTS table.
    """
    tokens = tokenize(text, lang)
    return " ".join(tokens)


def get_tokenizer_status() -> dict:
    """Return status of available tokenizers."""
    return {
        "jieba": _jieba_available,
        "sudachi": _sudachi_available,
        "fallback": "character_ngrams",
    }
