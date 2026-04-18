"""
Base embedding adapter protocol.

All embedding providers must implement this interface.
"""

from abc import ABC, abstractmethod
from typing import Protocol, runtime_checkable


@runtime_checkable
class EmbeddingAdapter(Protocol):
    """
    Protocol for embedding providers.
    
    Implementations must provide:
    - embed(): Embed multiple texts (for batch storage)
    - embed_query(): Embed a single query (may use different model/params)
    - dimension: The embedding vector dimension
    """
    
    @property
    def dimension(self) -> int:
        """Return the dimension of embeddings produced by this adapter."""
        ...
    
    def embed(self, texts: list[str]) -> list[list[float]]:
        """
        Embed multiple texts.
        
        Args:
            texts: List of texts to embed
            
        Returns:
            List of embedding vectors (same length as texts)
        """
        ...
    
    def embed_query(self, query: str) -> list[float]:
        """
        Embed a single query.
        
        Some providers use different models or parameters for queries
        vs documents. Default implementation just calls embed().
        
        Args:
            query: Query text to embed
            
        Returns:
            Embedding vector
        """
        ...


class BaseEmbeddingAdapter(ABC):
    """
    Base class for embedding adapters with common functionality.
    """
    
    _dimension: int = 0
    
    @property
    def dimension(self) -> int:
        return self._dimension
    
    @abstractmethod
    def embed(self, texts: list[str]) -> list[list[float]]:
        """Embed multiple texts."""
        pass
    
    def embed_query(self, query: str) -> list[float]:
        """Embed a single query. Override for query-specific behavior."""
        results = self.embed([query])
        return results[0]
