"""
Sentence Transformers embedding adapter.

Requires: pip install sentence-transformers
Models are downloaded automatically on first use (~90-300MB depending on model).
"""

from typing import Optional

from engram.embeddings.base import BaseEmbeddingAdapter


# Common model dimensions
MODEL_DIMENSIONS = {
    "all-MiniLM-L6-v2": 384,
    "all-MiniLM-L12-v2": 384,
    "all-mpnet-base-v2": 768,
    "paraphrase-MiniLM-L6-v2": 384,
    "nomic-ai/nomic-embed-text-v1": 768,
    "BAAI/bge-small-en-v1.5": 384,
    "BAAI/bge-base-en-v1.5": 768,
}


class SentenceTransformerAdapter(BaseEmbeddingAdapter):
    """
    Sentence Transformers adapter for local embedding.
    
    Usage:
        # Default (all-MiniLM-L6-v2, ~90MB, fast)
        adapter = SentenceTransformerAdapter()
        
        # Better quality (~274MB)
        adapter = SentenceTransformerAdapter("nomic-ai/nomic-embed-text-v1")
        
        # With Memory
        mem = Memory("./agent.db", embedding=adapter)
    """
    
    def __init__(
        self,
        model: str = "all-MiniLM-L6-v2",
        device: Optional[str] = None,
        normalize: bool = True,
    ):
        """
        Initialize Sentence Transformers adapter.
        
        Args:
            model: Model name from HuggingFace or sentence-transformers
            device: Device to use ("cpu", "cuda", "mps"). None = auto-detect.
            normalize: Whether to L2-normalize embeddings (recommended for cosine sim)
        """
        try:
            from sentence_transformers import SentenceTransformer
        except ImportError:
            raise ImportError(
                "SentenceTransformerAdapter requires sentence-transformers. "
                "Install with: pip install sentence-transformers"
            )
        
        self.model_name = model
        self.normalize = normalize
        
        # Load model
        self._model = SentenceTransformer(model, device=device)
        
        # Set dimension
        if model in MODEL_DIMENSIONS:
            self._dimension = MODEL_DIMENSIONS[model]
        else:
            # Infer from model
            self._dimension = self._model.get_sentence_embedding_dimension()
    
    def embed(self, texts: list[str]) -> list[list[float]]:
        """Embed multiple texts."""
        if not texts:
            return []
        
        # SentenceTransformer handles batching internally
        embeddings = self._model.encode(
            texts,
            normalize_embeddings=self.normalize,
            show_progress_bar=False,
        )
        
        # Convert numpy arrays to lists
        return [emb.tolist() for emb in embeddings]
    
    def embed_query(self, query: str) -> list[float]:
        """Embed a single query."""
        return self.embed([query])[0]
