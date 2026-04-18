#!/usr/bin/env python3
"""
Example: Simple Chatbot with neuromemory-ai

Demonstrates using cognitive memory for a conversational agent.

Run: python examples/chatbot.py
"""

import os
import sys
import tempfile

# Add parent directory for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig


def main():
    # Use chatbot preset for long conversation memory
    config = MemoryConfig.chatbot()
    
    # Create temporary database for demo
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    print("=" * 60)
    print("Chatbot Memory Demo")
    print("=" * 60)
    
    try:
        mem = Memory(db_path, config=config)
        
        # Simulate conversation over multiple exchanges
        print("\n📝 Session 1: Introduction")
        print("-" * 40)
        
        # User introduces themselves
        mem.add("User's name is Alex", type="relational", importance=0.9)
        print("Stored: User's name is Alex (relational, importance=0.9)")
        
        mem.add("Alex is a data scientist at a startup", type="relational", importance=0.7)
        print("Stored: Alex is a data scientist (relational, importance=0.7)")
        
        mem.add("Alex prefers concise answers", type="relational", importance=0.6)
        print("Stored: Preference for concise answers (relational, importance=0.6)")
        
        # Simulate overnight
        mem.consolidate(days=1)
        print("\n💤 [Consolidation: 1 day passed]")
        
        print("\n📝 Session 2: Technical Discussion")
        print("-" * 40)
        
        mem.add("Discussed Python pandas performance issues", type="episodic", importance=0.5)
        print("Stored: Discussed pandas performance (episodic, importance=0.5)")
        
        mem.add("Recommended using Polars for better performance", type="procedural", importance=0.6)
        print("Stored: Polars recommendation (procedural, importance=0.6)")
        
        mem.add("Alex was interested in Polars, wants to try it", type="episodic", importance=0.4)
        print("Stored: Alex interested in Polars (episodic, importance=0.4)")
        
        # Positive feedback strengthens recent memories
        mem.reward("Thanks, that was helpful!")
        print("\n✨ Received positive feedback - strengthening recent memories")
        
        mem.consolidate(days=1)
        print("\n💤 [Consolidation: 1 day passed]")
        
        print("\n📝 Session 3: Follow-up")
        print("-" * 40)
        
        # Now test recall
        print("\n🔍 Recalling: 'What is the user's name?'")
        results = mem.recall("user name Alex", limit=3)
        for r in results:
            print(f"   [{r['confidence_label']:8}] {r['content'][:60]}")
        
        print("\n🔍 Recalling: 'What did we discuss about performance?'")
        results = mem.recall("performance pandas Polars", limit=3)
        for r in results:
            print(f"   [{r['confidence_label']:8}] {r['content'][:60]}")
        
        print("\n🔍 Recalling: 'What does the user prefer?'")
        results = mem.recall("Alex prefers concise", limit=3)
        for r in results:
            print(f"   [{r['confidence_label']:8}] {r['content'][:60]}")
        
        # Show final stats
        stats = mem.stats()
        print("\n📊 Memory Statistics")
        print("-" * 40)
        print(f"   Total memories: {stats['total_memories']}")
        print(f"   By layer: working={stats['layers'].get('working', {}).get('count', 0)}, "
              f"core={stats['layers'].get('core', {}).get('count', 0)}")
        
        print("\n✅ Demo complete!")
        
    finally:
        os.unlink(db_path)


if __name__ == "__main__":
    main()
