"""
Embedding adapters for engram.

Usage:
    from engram.embeddings import OpenAIAdapter, OllamaAdapter
    
    # OpenAI
    adapter = OpenAIAdapter(model="text-embedding-3-small")
    
    # Ollama (local)
    adapter = OllamaAdapter(model="nomic-embed-text")
    
    # Use with Memory
    mem = Memory("./agent.db", embedding=adapter)
"""

from engram.embeddings.base import EmbeddingAdapter
from engram.embeddings.openai import OpenAIAdapter

__all__ = ["EmbeddingAdapter", "OpenAIAdapter"]

# Optional imports (may not have dependencies installed)
try:
    from engram.embeddings.ollama import OllamaAdapter
    __all__.append("OllamaAdapter")
except ImportError:
    pass

try:
    from engram.embeddings.sentence_transformers import SentenceTransformerAdapter
    __all__.append("SentenceTransformerAdapter")
except ImportError:
    pass
