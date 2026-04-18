"""
Porter Stemmer - Lightweight implementation for FTS5 tokenization
Based on the original Porter algorithm (1980)
Zero external dependencies
"""

class PorterStemmer:
    def __init__(self):
        self.vowels = frozenset('aeiou')
    
    def _is_consonant(self, word: str, i: int) -> bool:
        if word[i] in self.vowels:
            return False
        if word[i] == 'y':
            return i == 0 or not self._is_consonant(word, i - 1)
        return True
    
    def _measure(self, stem: str) -> int:
        """Count VC sequences"""
        m = 0
        i = 0
        n = len(stem)
        while i < n and self._is_consonant(stem, i):
            i += 1
        while i < n:
            while i < n and not self._is_consonant(stem, i):
                i += 1
            if i >= n:
                break
            m += 1
            while i < n and self._is_consonant(stem, i):
                i += 1
        return m
    
    def _has_vowel(self, stem: str) -> bool:
        return any(not self._is_consonant(stem, i) for i in range(len(stem)))
    
    def _ends_double_consonant(self, word: str) -> bool:
        return len(word) >= 2 and word[-1] == word[-2] and self._is_consonant(word, len(word) - 1)
    
    def _ends_cvc(self, word: str) -> bool:
        if len(word) < 3:
            return False
        return (self._is_consonant(word, -3) and 
                not self._is_consonant(word, -2) and 
                self._is_consonant(word, -1) and
                word[-1] not in 'wxy')
    
    def _replace(self, word: str, suffix: str, replacement: str, m_threshold: int = 0) -> str:
        if word.endswith(suffix):
            stem = word[:-len(suffix)] if suffix else word
            if self._measure(stem) > m_threshold:
                return stem + replacement
        return word
    
    def stem(self, word: str) -> str:
        word = word.lower()
        if len(word) <= 2:
            return word
        
        # Step 1a
        if word.endswith('sses'):
            word = word[:-2]
        elif word.endswith('ies'):
            word = word[:-2]
        elif word.endswith('ss'):
            pass
        elif word.endswith('s'):
            word = word[:-1]
        
        # Step 1b
        if word.endswith('eed'):
            if self._measure(word[:-3]) > 0:
                word = word[:-1]
        elif word.endswith('ed'):
            stem = word[:-2]
            if self._has_vowel(stem):
                word = stem
                if word.endswith(('at', 'bl', 'iz')):
                    word += 'e'
                elif self._ends_double_consonant(word) and word[-1] not in 'lsz':
                    word = word[:-1]
                elif self._measure(word) == 1 and self._ends_cvc(word):
                    word += 'e'
        elif word.endswith('ing'):
            stem = word[:-3]
            if self._has_vowel(stem):
                word = stem
                if word.endswith(('at', 'bl', 'iz')):
                    word += 'e'
                elif self._ends_double_consonant(word) and word[-1] not in 'lsz':
                    word = word[:-1]
                elif self._measure(word) == 1 and self._ends_cvc(word):
                    word += 'e'
        
        # Step 1c
        if word.endswith('y') and self._has_vowel(word[:-1]):
            word = word[:-1] + 'i'
        
        # Step 2
        step2_pairs = [
            ('ational', 'ate'), ('tional', 'tion'), ('enci', 'ence'),
            ('anci', 'ance'), ('izer', 'ize'), ('abli', 'able'),
            ('alli', 'al'), ('entli', 'ent'), ('eli', 'e'),
            ('ousli', 'ous'), ('ization', 'ize'), ('ation', 'ate'),
            ('ator', 'ate'), ('alism', 'al'), ('iveness', 'ive'),
            ('fulness', 'ful'), ('ousness', 'ous'), ('aliti', 'al'),
            ('iviti', 'ive'), ('biliti', 'ble'),
        ]
        for suffix, replacement in step2_pairs:
            if word.endswith(suffix):
                stem = word[:-len(suffix)]
                if self._measure(stem) > 0:
                    word = stem + replacement
                break
        
        # Step 3
        step3_pairs = [
            ('icate', 'ic'), ('ative', ''), ('alize', 'al'),
            ('iciti', 'ic'), ('ical', 'ic'), ('ful', ''), ('ness', ''),
        ]
        for suffix, replacement in step3_pairs:
            if word.endswith(suffix):
                stem = word[:-len(suffix)]
                if self._measure(stem) > 0:
                    word = stem + replacement
                break
        
        # Step 4
        step4_suffixes = [
            'al', 'ance', 'ence', 'er', 'ic', 'able', 'ible', 'ant',
            'ement', 'ment', 'ent', 'ion', 'ou', 'ism', 'ate', 'iti',
            'ous', 'ive', 'ize',
        ]
        for suffix in step4_suffixes:
            if word.endswith(suffix):
                stem = word[:-len(suffix)]
                if self._measure(stem) > 1:
                    if suffix == 'ion' and stem and stem[-1] in 'st':
                        word = stem
                    elif suffix != 'ion':
                        word = stem
                break
        
        # Step 5a
        if word.endswith('e'):
            stem = word[:-1]
            m = self._measure(stem)
            if m > 1 or (m == 1 and not self._ends_cvc(stem)):
                word = stem
        
        # Step 5b
        if self._ends_double_consonant(word) and word[-1] == 'l' and self._measure(word) > 1:
            word = word[:-1]
        
        return word


# Quick test
if __name__ == "__main__":
    stemmer = PorterStemmer()
    tests = [
        ("researching", "research"),
        ("research", "research"),
        ("playing", "plai"),  # Porter produces "plai" for "playing"
        ("hobbies", "hobbi"),
        ("pottery", "potteri"),
        ("classes", "class"),
        ("learning", "learn"),
        ("remembered", "rememb"),
    ]
    for word, expected in tests:
        result = stemmer.stem(word)
        status = "✓" if result == expected else f"✗ (got {result})"
        print(f"{word} -> {result} {status}")
