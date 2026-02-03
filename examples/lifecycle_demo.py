"""
Engram — Memory Lifecycle Demo

Simulates 30 days of agent operation showing:
- Memory formation and encoding
- Consolidation cycles (daily "sleep")
- Forgetting curves (episodic vs procedural decay)
- Strength evolution over time
- Layer transitions (working → core → archive)
"""

import time
import tempfile
import os

from engram import Memory
from engram.core import MemoryLayer
from engram.forgetting import effective_strength

# Use temp DB for demo
db_path = os.path.join(tempfile.mkdtemp(), "lifecycle.db")
mem = Memory(db_path)

print("=" * 60)
print("  Engram — 30-Day Memory Lifecycle Simulation")
print("=" * 60)

# Day 0: Initial memories
print("\n📝 Day 0: Encoding initial memories\n")

ids = {}
ids["proc"] = mem.add("Always validate input before database queries",
                       type="procedural", importance=0.8)
ids["episodic"] = mem.add("Had a great debugging session with the team",
                          type="episodic", importance=0.4)
ids["relational"] = mem.add("Alice is the go-to person for infrastructure",
                            type="relational", importance=0.7)
ids["factual"] = mem.add("The API rate limit is 100 requests per minute",
                         type="factual", importance=0.3)
ids["emotional"] = mem.add("Got promoted — feeling proud and motivated",
                           type="emotional", importance=0.95)

print(f"  Stored {len(ids)} memories\n")

# Simulate 30 days
for day in range(1, 31):
    # Consolidate each day (the "sleep" cycle)
    mem.consolidate(days=1.0)

    # Simulate some accesses on specific days
    if day == 5:
        mem.recall("database queries")  # Access procedural memory
    if day == 10:
        mem.recall("database queries")  # Spaced repetition
    if day == 15:
        mem.reward("good advice on input validation!")
    if day == 20:
        mem.recall("database queries")  # Another spaced repetition

    # Print status on key days
    if day in (1, 3, 7, 14, 21, 30):
        print(f"📅 Day {day}:")
        all_memories = mem._store.all()
        for label, mid in ids.items():
            entry = next((m for m in all_memories if m.id == mid), None)
            if entry:
                eff = effective_strength(entry)
                layer = entry.layer.value
                ws = entry.working_strength
                cs = entry.core_strength
                bar = "█" * int(eff * 20) + "░" * (20 - int(eff * 20))
                print(f"  {label:12s} [{layer:7s}] |{bar}| "
                      f"eff={eff:.3f} (w={ws:.3f} c={cs:.3f})")
        print()

# Final summary
print("=" * 60)
print("  Summary after 30 days")
print("=" * 60)

stats = mem.stats()
print(f"\n  Total memories: {stats['total_memories']}")
print(f"  Pinned: {stats['pinned']}")
print(f"\n  By layer:")
for layer_name, info in stats["layers"].items():
    if info["count"] > 0:
        print(f"    {layer_name:10s}: {info['count']} memories "
              f"(avg_w={info['avg_working']:.3f}, avg_c={info['avg_core']:.3f})")

print(f"\n  Key observations:")
print(f"  • Procedural memory (accessed 3x): maintained through spaced repetition")
print(f"  • Episodic memory (no access): decayed naturally — episodes fade")
print(f"  • Emotional memory: high importance slowed decay significantly")
print(f"  • Factual memory (low importance): archived quickly")

mem.close()
print(f"\n  Done. DB at: {db_path}")
