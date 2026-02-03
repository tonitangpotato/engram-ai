"""
Engram — Basic Usage Example

Shows: add memories, recall with context, consolidate, check stats.
"""

from engram import Memory

# Create a memory system (SQLite-backed, persistent)
mem = Memory("./example.db")

# Store memories with types and importance
mem.add("Python 3.12 introduced type parameter syntax", type="factual", importance=0.5)
mem.add("Alice prefers functional programming", type="relational", importance=0.7)
mem.add("Always run tests before deploying", type="procedural", importance=0.9)
mem.add("Shipped v2.0 on launch day — the team was ecstatic", type="emotional", importance=0.8)
mem.add("The API timeout is set to 30 seconds", type="factual", importance=0.3)

print(f"Stored {len(mem)} memories\n")

# Recall memories relevant to a query
# Uses ACT-R activation scoring (recency × frequency × context match)
print("--- Recall: 'deployment process' ---")
results = mem.recall("deployment process", limit=3)
for r in results:
    print(f"  [{r['confidence_label']:10s}] {r['content']}")

print()

# Recall with context keywords for spreading activation
print("--- Recall: 'Alice' with context ['coding', 'style'] ---")
results = mem.recall("Alice", context=["coding", "style"], limit=3)
for r in results:
    print(f"  [{r['confidence_label']:10s}] {r['content']}")

print()

# Run consolidation (transfers working → core memory)
mem.consolidate(days=1.0)
print("Consolidated (1 day cycle)")

# Check stats
stats = mem.stats()
print(f"\nStats: {stats['total_memories']} memories across {len(stats['by_type'])} types")
for t, info in stats["by_type"].items():
    print(f"  {t}: {info['count']} entries, avg strength {info['avg_strength']}")

# Positive feedback strengthens recent memories
mem.reward("great, that's exactly what I needed!")
print("\nApplied positive reward signal")

# Clean up
mem.close()
print("Done.")
