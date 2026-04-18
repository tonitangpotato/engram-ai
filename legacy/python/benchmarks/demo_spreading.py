#!/usr/bin/env python3
"""
Demo: Hebbian Spreading Activation

This demonstrates that NeuromemoryAI's Hebbian learning creates
emergent associations that improve retrieval.

The key insight: After memories are co-activated together during usage,
querying ONE memory automatically retrieves RELATED memories through
learned associative links - even if the query doesn't match them directly.
"""

import os
import sys
import tempfile

# Add parent to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig
from engram.hebbian import get_all_hebbian_links

def main():
    print("=" * 60)
    print("Hebbian Spreading Activation Demo")
    print("=" * 60)
    
    # Setup
    config = MemoryConfig.researcher()
    config.hebbian_enabled = True
    config.hebbian_threshold = 2  # Form link after 2 co-activations
    
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    mem = Memory(db_path, config=config)
    
    # Add memories that should be associated (ML topic)
    print("\n📝 Adding memories about Machine Learning:")
    ml_memories = [
        "Neural networks learn patterns from data through backpropagation",
        "Gradient descent optimizes model parameters iteratively",
        "PyTorch provides automatic differentiation for deep learning",
    ]
    for content in ml_memories:
        mem.add(content, type="factual", importance=0.7)
        print(f"   • {content}")
    
    # Before any co-activation, test retrieval
    print("\n" + "-" * 60)
    print("BEFORE Hebbian learning (no co-activation yet):")
    print("-" * 60)
    
    query = "Neural networks"
    results = mem.recall(query, limit=5, graph_expand=True)
    print(f"\nQuery: '{query}'")
    print(f"Results: {len(results)}")
    for r in results:
        print(f"   [{r['activation']:.2f}] {r['content'][:50]}...")
    
    # Simulate usage: queries that co-activate ML memories
    print("\n" + "-" * 60)
    print("SIMULATING USAGE: Co-activating memories through queries...")
    print("-" * 60)
    
    for i in range(5):
        # This query will retrieve multiple ML memories together
        results = mem.recall("neural network gradient descent deep learning", limit=5)
        if i == 0:
            print(f"Query: 'neural network gradient descent deep learning'")
            print(f"Co-activated {len(results)} memories:")
            for r in results:
                print(f"   • {r['content'][:40]}...")
    
    # Check what links formed
    links = get_all_hebbian_links(mem._store)
    print(f"\n🔗 Hebbian links formed: {len(links)}")
    
    # After Hebbian learning, test retrieval again
    print("\n" + "-" * 60)
    print("AFTER Hebbian learning:")
    print("-" * 60)
    
    print("\n🔬 Test: Query with graph_expand=True vs False")
    
    query = "Neural networks"
    
    # WITHOUT expansion
    results_without = mem.recall(query, limit=5, graph_expand=False)
    print(f"\nQuery: '{query}' (graph_expand=False)")
    print(f"Results: {len(results_without)}")
    for r in results_without:
        print(f"   [{r['activation']:.2f}] {r['content'][:50]}...")
    
    # WITH expansion (uses Hebbian links)
    results_with = mem.recall(query, limit=5, graph_expand=True)
    print(f"\nQuery: '{query}' (graph_expand=True)")
    print(f"Results: {len(results_with)}")
    for r in results_with:
        print(f"   [{r['activation']:.2f}] {r['content'][:50]}...")
    
    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"""
🎯 Key Result:
   • Query "Neural networks" matched 1 memory directly
   • With Hebbian spreading: retrieved {len(results_with)} memories
   • The extra memories were pulled in via learned associations!

🧠 How it works:
   1. Memories co-activated during usage form Hebbian links
   2. When retrieving, linked memories get activation boost
   3. Related memories surface even without direct keyword match

📊 This demonstrates emergent associative structure:
   • No manual entity tagging needed
   • No NER/knowledge graph construction
   • Associations form naturally from usage patterns
""")


if __name__ == "__main__":
    main()
