#!/usr/bin/env python3
"""
Example: Research Agent with Hebbian Learning

Demonstrates how memories form associative links through co-activation.

Run: python examples/research_agent.py
"""

import os
import sys
import tempfile

# Add parent directory for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig
from engram.hebbian import get_all_hebbian_links


def main():
    # Use researcher preset (minimal forgetting, high replay)
    config = MemoryConfig.researcher()
    config.hebbian_threshold = 2  # Lower threshold for demo
    
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    print("=" * 60)
    print("Research Agent with Hebbian Learning")
    print("=" * 60)
    
    try:
        mem = Memory(db_path, config=config)
        
        # Simulate research session: collecting findings about LLMs
        print("\n📚 Phase 1: Collecting Research Findings")
        print("-" * 40)
        
        findings = [
            ("GPT4 uses transformer architecture", "factual", 0.8),
            ("Transformers use attention mechanisms", "factual", 0.8),
            ("Attention computes query key value operations", "factual", 0.7),
            ("Claude uses Constitutional AI training", "factual", 0.7),
            ("RLHF improves model alignment", "factual", 0.8),
            ("Constitutional AI is a form of RLHF", "factual", 0.7),
            ("Scaling laws predict model performance", "factual", 0.6),
            ("Chinchilla showed optimal compute allocation", "factual", 0.6),
        ]
        
        for content, mtype, importance in findings:
            mid = mem.add(content, type=mtype, importance=importance)
            print(f"   + {content}")
        
        # Simulate research queries that co-activate related memories
        print("\n🔬 Phase 2: Research Sessions (co-activating memories)")
        print("-" * 40)
        
        queries = [
            ("transformer attention architecture", "Studying transformer internals"),
            ("GPT4 transformer attention", "Comparing GPT4 architecture"),
            ("attention query key value", "Deep dive into attention"),
            ("Claude Constitutional AI RLHF", "Studying Claude training"),
            ("RLHF alignment Constitutional", "Alignment methods comparison"),
        ]
        
        for query, desc in queries:
            print(f"\n   Query: '{query}'")
            print(f"   ({desc})")
            results = mem.recall(query, limit=3)
            for r in results:
                print(f"      -> {r['content'][:50]}...")
        
        # Check Hebbian links
        print("\n🔗 Phase 3: Emergent Associations (Hebbian Links)")
        print("-" * 40)
        
        links = get_all_hebbian_links(mem._store)
        
        if links:
            print(f"\n   Found {len(links)} Hebbian links:")
            for source_id, target_id, strength in links[:10]:
                source = mem._store.get(source_id)
                target = mem._store.get(target_id)
                if source and target:
                    print(f"\n   '{source.content[:40]}...'")
                    print(f"      ↔ '{target.content[:40]}...'")
                    print(f"      (strength: {strength:.2f})")
        else:
            print("   No links formed yet (need more co-activations)")
        
        # Demonstrate how links affect retrieval
        print("\n🎯 Phase 4: Link-Enhanced Retrieval")
        print("-" * 40)
        
        # Query about transformers should also surface attention info
        print("\n   Query: 'transformer'")
        results = mem.recall("transformer", limit=5)
        for r in results:
            print(f"      [{r['confidence_label']:8}] {r['content'][:50]}...")
        
        # Show stats
        stats = mem.stats()
        print("\n📊 Final Statistics")
        print("-" * 40)
        print(f"   Total memories: {stats['total_memories']}")
        print(f"   Total Hebbian links: {len(links)}")
        
        print("\n💡 Key Insight:")
        print("   Memories that are frequently recalled together")
        print("   automatically form associative links (Hebbian learning).")
        print("   No manual entity tagging required!")
        
        print("\n✅ Demo complete!")
        
    finally:
        os.unlink(db_path)


if __name__ == "__main__":
    main()
