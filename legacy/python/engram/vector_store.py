"""
Simple vector store for embedding-based retrieval.

Uses SQLite for storage and Python for cosine similarity.
For production, consider sqlite-vec, pgvector, or dedicated vector DBs.
"""

import json
import math
import sqlite3
from typing import Optional

from engram.embeddings.base import EmbeddingAdapter


def cosine_similarity(a: list[float], b: list[float]) -> float:
    """Compute cosine similarity between two vectors."""
    if len(a) != len(b):
        return 0.0
    
    dot_product = sum(x * y for x, y in zip(a, b))
    norm_a = math.sqrt(sum(x * x for x in a))
    norm_b = math.sqrt(sum(x * x for x in b))
    
    if norm_a == 0 or norm_b == 0:
        return 0.0
    
    return dot_product / (norm_a * norm_b)


class VectorStore:
    """
    Simple vector store backed by SQLite.
    
    Stores embeddings as JSON and computes similarity in Python.
    Not optimized for large scale, but works well for <100k memories.
    """
    
    def __init__(self, conn: sqlite3.Connection, adapter: EmbeddingAdapter):
        """
        Initialize vector store.
        
        Args:
            conn: SQLite connection (shared with MemoryStore)
            adapter: Embedding adapter to use
        """
        self.conn = conn
        self.adapter = adapter
        self._init_tables()
    
    def _init_tables(self):
        """Create vector storage tables."""
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS memory_embeddings (
                memory_id TEXT PRIMARY KEY,
                embedding TEXT NOT NULL,
                FOREIGN KEY (memory_id) REFERENCES memories(id)
            )
        """)
        self.conn.commit()
    
    def add(self, memory_id: str, text: str):
        """
        Add embedding for a memory.
        
        Args:
            memory_id: ID of the memory
            text: Text to embed
        """
        embedding = self.adapter.embed([text])[0]
        self.conn.execute(
            "INSERT OR REPLACE INTO memory_embeddings (memory_id, embedding) VALUES (?, ?)",
            (memory_id, json.dumps(embedding))
        )
        self.conn.commit()
    
    def add_batch(self, items: list[tuple[str, str]]):
        """
        Add embeddings for multiple memories.
        
        Args:
            items: List of (memory_id, text) tuples
        """
        if not items:
            return
        
        memory_ids = [item[0] for item in items]
        texts = [item[1] for item in items]
        
        embeddings = self.adapter.embed(texts)
        
        self.conn.executemany(
            "INSERT OR REPLACE INTO memory_embeddings (memory_id, embedding) VALUES (?, ?)",
            [(mid, json.dumps(emb)) for mid, emb in zip(memory_ids, embeddings)]
        )
        self.conn.commit()
    
    def search(
        self,
        query: str,
        limit: int = 20,
        min_similarity: float = 0.0,
    ) -> list[tuple[str, float]]:
        """
        Search for similar memories.
        
        Args:
            query: Query text
            limit: Maximum results to return
            min_similarity: Minimum cosine similarity threshold
            
        Returns:
            List of (memory_id, similarity_score) tuples, sorted by similarity
        """
        query_embedding = self.adapter.embed_query(query)
        
        # Get all embeddings (not efficient for large scale, but simple)
        rows = self.conn.execute(
            "SELECT memory_id, embedding FROM memory_embeddings"
        ).fetchall()
        
        results = []
        for memory_id, embedding_data in rows:
            # Support both JSON string (Python-written) and binary blob (Rust-written)
            if isinstance(embedding_data, bytes):
                import struct
                # Rust stores Vec<f32> as little-endian binary blob
                n_floats = len(embedding_data) // 4
                embedding = list(struct.unpack(f'<{n_floats}f', embedding_data))
            else:
                embedding = json.loads(embedding_data)
            similarity = cosine_similarity(query_embedding, embedding)
            if similarity >= min_similarity:
                results.append((memory_id, similarity))
        
        # Sort by similarity descending
        results.sort(key=lambda x: x[1], reverse=True)
        
        return results[:limit]
    
    def delete(self, memory_id: str):
        """Delete embedding for a memory."""
        self.conn.execute(
            "DELETE FROM memory_embeddings WHERE memory_id = ?",
            (memory_id,)
        )
        self.conn.commit()
    
    def has_embedding(self, memory_id: str) -> bool:
        """Check if a memory has an embedding."""
        row = self.conn.execute(
            "SELECT 1 FROM memory_embeddings WHERE memory_id = ?",
            (memory_id,)
        ).fetchone()
        return row is not None
    
    def count(self) -> int:
        """Count total embeddings stored."""
        row = self.conn.execute(
            "SELECT COUNT(*) FROM memory_embeddings"
        ).fetchone()
        return row[0] if row else 0
