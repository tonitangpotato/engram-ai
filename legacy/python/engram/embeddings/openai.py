"""
OpenAI embedding adapter.

Requires: pip install openai
"""

import os
from typing import Optional

from engram.embeddings.base import BaseEmbeddingAdapter


# Model dimensions
OPENAI_DIMENSIONS = {
    "text-embedding-3-small": 1536,
    "text-embedding-3-large": 3072,
    "text-embedding-ada-002": 1536,
}


class OpenAIAdapter(BaseEmbeddingAdapter):
    """
    OpenAI embedding adapter.
    
    Usage:
        adapter = OpenAIAdapter()  # Uses text-embedding-3-small
        adapter = OpenAIAdapter(model="text-embedding-3-large")
        adapter = OpenAIAdapter(api_key="sk-...")  # Explicit key
    """
    
    def __init__(
        self,
        model: str = "text-embedding-3-small",
        api_key: Optional[str] = None,
        dimensions: Optional[int] = None,
        batch_size: int = 100,
    ):
        """
        Initialize OpenAI adapter.
        
        Args:
            model: OpenAI embedding model name
            api_key: API key (defaults to OPENAI_API_KEY env var)
            dimensions: Override embedding dimensions (for text-embedding-3-*)
            batch_size: Max texts per API call
        """
        try:
            from openai import OpenAI
        except ImportError:
            raise ImportError(
                "OpenAI adapter requires the openai package. "
                "Install with: pip install openai"
            )
        
        self.model = model
        self.batch_size = batch_size
        self._client = OpenAI(api_key=api_key or os.environ.get("OPENAI_API_KEY"))
        
        # Set dimension
        if dimensions:
            self._dimension = dimensions
        elif model in OPENAI_DIMENSIONS:
            self._dimension = OPENAI_DIMENSIONS[model]
        else:
            # Default for unknown models
            self._dimension = 1536
        
        # For text-embedding-3-*, we can request specific dimensions
        self._request_dimensions = None
        if model.startswith("text-embedding-3") and dimensions:
            self._request_dimensions = dimensions
    
    def embed(self, texts: list[str]) -> list[list[float]]:
        """Embed multiple texts using OpenAI API."""
        if not texts:
            return []
        
        all_embeddings = []
        
        # Process in batches
        for i in range(0, len(texts), self.batch_size):
            batch = texts[i:i + self.batch_size]
            
            # Clean texts (OpenAI doesn't like empty strings)
            batch = [t if t.strip() else " " for t in batch]
            
            kwargs = {
                "model": self.model,
                "input": batch,
            }
            if self._request_dimensions:
                kwargs["dimensions"] = self._request_dimensions
            
            response = self._client.embeddings.create(**kwargs)
            
            # Extract embeddings in order
            batch_embeddings = [None] * len(batch)
            for item in response.data:
                batch_embeddings[item.index] = item.embedding
            
            all_embeddings.extend(batch_embeddings)
        
        return all_embeddings
    
    def embed_query(self, query: str) -> list[float]:
        """Embed a query (same as document for OpenAI)."""
        return self.embed([query])[0]
