#!/usr/bin/env python3
"""
One-time migration: Generate embeddings for existing memories.

Usage:
    python3 migrate_vectors.py [--db-path PATH]
"""

import argparse
import sys
import os
from pathlib import Path

# Add engram to path
sys.path.insert(0, str(Path(__file__).parent))

from engram.memory import Memory
from engram.embeddings import SentenceTransformerAdapter


def migrate(db_path: str):
    """Generate vectors for all memories without embeddings."""
    print(f"🔧 Migrating database: {db_path}")
    
    # Initialize with embedding
    model_name = "paraphrase-multilingual-MiniLM-L12-v2"
    print(f"📦 Loading Sentence Transformers model ({model_name})...")
    embedding = SentenceTransformerAdapter(model_name)
    
    print("🧠 Initializing Memory with embedding support...")
    mem = Memory(db_path, embedding=embedding)
    
    # Get all memories
    all_memories = mem._store.all()
    total = len(all_memories)
    print(f"📊 Found {total} memories")
    
    # Check how many already have vectors in VectorStore
    vector_count = mem._vector_store.count()
    without_vectors = total - vector_count
    
    print(f"✅ With vectors: {vector_count}")
    print(f"❌ Without vectors: {without_vectors}")
    
    if without_vectors == 0:
        print("✨ All memories already have vectors!")
        return
    
    # Find memories without embeddings
    print(f"\n🔍 Finding memories without embeddings...")
    to_process = []
    for entry in all_memories:
        if not mem._vector_store.has_embedding(entry.id):
            to_process.append((entry.id, entry.content))
    
    print(f"   Found {len(to_process)} memories to process")
    
    # Batch-generate vectors
    print(f"\n🚀 Generating vectors...")
    
    import time
    start = time.time()
    
    # Process in batches of 50 for progress reporting
    batch_size = 50
    for i in range(0, len(to_process), batch_size):
        batch = to_process[i:i+batch_size]
        mem._vector_store.add_batch(batch)
        
        elapsed = time.time() - start
        processed = min(i + batch_size, len(to_process))
        rate = processed / elapsed if elapsed > 0 else 0
        remaining = (len(to_process) - processed) / rate if rate > 0 else 0
        print(f"  Progress: {processed}/{len(to_process)} ({rate:.1f} mem/sec, ~{remaining:.0f}s remaining)")
    
    elapsed = time.time() - start
    print(f"\n✅ Migration complete!")
    print(f"   Updated: {len(to_process)} memories")
    print(f"   Time: {elapsed:.2f}s ({len(to_process)/elapsed:.1f} mem/sec)")
    
    # Verify
    final_vector_count = mem._vector_store.count()
    print(f"   Final vector count: {final_vector_count}/{total}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate embeddings for existing memories")
    parser.add_argument(
        "--db-path",
        default=os.environ.get("ENGRAM_DB_PATH", "/Users/potato/clawd/engram-memory.db"),
        help="Path to engram database",
    )
    args = parser.parse_args()
    
    migrate(args.db_path)
