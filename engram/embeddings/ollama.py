"""
Ollama embedding adapter — local, free, zero API key.

Uses nomic-embed-text by default (768 dimensions).
Requires Ollama running locally: https://ollama.ai
"""

import json
import urllib.request
from engram.embeddings.base import BaseEmbeddingAdapter

# Model → dimension mapping
MODEL_DIMS = {
    "nomic-embed-text": 768,
    "mxbai-embed-large": 1024,
    "all-minilm": 384,
    "snowflake-arctic-embed": 1024,
}

DEFAULT_MODEL = "nomic-embed-text"
DEFAULT_BASE_URL = "http://localhost:11434"


class OllamaAdapter(BaseEmbeddingAdapter):
    """
    Ollama embedding adapter.

    Usage:
        adapter = OllamaAdapter()  # Uses nomic-embed-text
        adapter = OllamaAdapter(model="mxbai-embed-large")
        vectors = adapter.embed(["hello world"])
    """

    def __init__(
        self,
        model: str = DEFAULT_MODEL,
        base_url: str = DEFAULT_BASE_URL,
        timeout: float = 10.0,
    ):
        self.model = model
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self._dimension = MODEL_DIMS.get(model, 768)

    def embed(self, texts: list[str]) -> list[list[float]]:
        """Embed multiple texts via Ollama API."""
        results = []
        for text in texts:
            data = json.dumps({"model": self.model, "prompt": text}).encode()
            req = urllib.request.Request(
                f"{self.base_url}/api/embeddings",
                data=data,
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                body = json.loads(resp.read())
                results.append(body["embedding"])

        # Update dimension from actual response
        if results and len(results[0]) != self._dimension:
            self._dimension = len(results[0])

        return results
