#!/usr/bin/env python3
"""
Engram — Full Feature Demo
===========================
A walkthrough of every feature, showing how an AI agent's memory works.
"""

import time
import tempfile
import os

from engram.memory import Memory
from engram.config import MemoryConfig

# Use temp dir so it's clean every run
tmpdir = tempfile.mkdtemp()
db_path = os.path.join(tmpdir, "demo.db")


def section(title):
    print(f"\n{'='*60}")
    print(f"  {title}")
    print(f"{'='*60}\n")


def show_results(results, max=5):
    for i, r in enumerate(results[:max]):
        flag = " ⚠️CONTRADICTED" if r.get("contradicted") else ""
        print(f"  {i+1}. [{r['type']:11s}] conf={r['confidence']:.2f} | {r['content'][:65]}{flag}")
    print()


# ═══════════════════════════════════════════════════
section("1. INIT — Personal Assistant Preset")
# ═══════════════════════════════════════════════════

mem = Memory(db_path, config=MemoryConfig.personal_assistant())
cfg = mem.config
print(f"  Preset: personal_assistant")
print(f"  Core decay rate: {cfg.mu2}/day (very slow — memories last months)")
print(f"  Forget threshold: {cfg.forget_threshold} (hard to lose memories)")
print(f"  Importance weight: {cfg.importance_weight} (important memories prioritized)")


# ═══════════════════════════════════════════════════
section("2. ADD MEMORIES — 6 types with entities")
# ═══════════════════════════════════════════════════

ids = {}

ids["supabase"] = mem.add(
    "SaltyHall uses Supabase for its database backend",
    type="factual", importance=0.5,
    entities=[("SaltyHall", "uses"), ("Supabase", "backend")],
)
print("  + [factual]    SaltyHall uses Supabase")

ids["debug"] = mem.add(
    "Had a great late-night debugging session with potato on Feb 2",
    type="episodic", importance=0.6,
    entities=[("potato", "debugging_with")],
)
print("  + [episodic]   Late-night debugging session")

ids["moltbook"] = mem.add(
    "Always use www.moltbook.com not moltbook.com for API calls",
    type="procedural", importance=0.8,
    entities=[("Moltbook", "api_url")],
)
print("  + [procedural] Moltbook URL rule")

ids["preference"] = mem.add(
    "potato prefers action over discussion and hates long meetings",
    type="relational", importance=0.7,
    entities=[("potato", "preference")],
)
print("  + [relational] potato's preferences")

ids["kinda_like"] = mem.add(
    "potato said 'I kinda like you' after the late night coding session",
    type="emotional", importance=0.95,
    entities=[("potato", "sentiment")],
)
print("  + [emotional]  'I kinda like you' (importance=0.95)")

ids["opinion"] = mem.add(
    "I think graph plus text hybrid search is the best approach",
    type="opinion", importance=0.4,
)
print("  + [opinion]    Hybrid search opinion")

ids["trivial"] = mem.add(
    "Random thought about weather being nice today",
    type="episodic", importance=0.05,
)
print("  + [episodic]   Trivial weather thought (importance=0.05)")

print(f"\n  Total: {len(mem)} memories")


# ═══════════════════════════════════════════════════
section("3. RECALL — ACT-R activation ranking")
# ═══════════════════════════════════════════════════

print("  Query: 'potato preferences'\n")
results = mem.recall("potato preferences", limit=5)
show_results(results)

print("  → High importance + keyword match = top ranking")
print("  → Emotional memory surfaces due to potato entity link")


# ═══════════════════════════════════════════════════
section("4. GRAPH SEARCH — entity expansion")
# ═══════════════════════════════════════════════════

print("  Query: 'Supabase' (with graph expansion)\n")
results = mem.recall("Supabase", limit=5, graph_expand=True)
show_results(results)
print("  → Graph expansion pulls in related SaltyHall memories via entity links")


# ═══════════════════════════════════════════════════
section("5. REWARD — reinforcement learning")
# ═══════════════════════════════════════════════════

# Access moltbook memory first
mem.recall("moltbook URL", limit=1)

print("  Giving positive feedback: 'great, exactly right!'")
mem.reward("great, that's exactly the right URL!")

results = mem.recall("moltbook URL", limit=1)
print(f"  Moltbook memory importance after reward: {results[0]['importance']:.3f}")
print("  → Positive reward boosts importance of recently recalled memories")


# ═══════════════════════════════════════════════════
section("6. CONSOLIDATION — sleep cycles")
# ═══════════════════════════════════════════════════

entry_before = mem._store.get(ids["preference"])
print(f"  Before sleep:")
print(f"    working_strength: {entry_before.working_strength:.4f}")
print(f"    core_strength:    {entry_before.core_strength:.4f}")

print(f"\n  Running 5 days of consolidation (sleep)...")
for _ in range(5):
    mem.consolidate(days=1.0)

entry_after = mem._store.get(ids["preference"])
print(f"\n  After 5 days:")
print(f"    working_strength: {entry_after.working_strength:.4f} (decayed)")
print(f"    core_strength:    {entry_after.core_strength:.4f} (grew from consolidation)")
print(f"    layer:            {entry_after.layer.value}")
print("\n  → Working memory decays, core memory strengthens (Memory Chain model)")


# ═══════════════════════════════════════════════════
section("7. TIME DECAY — Ebbinghaus forgetting curve")
# ═══════════════════════════════════════════════════

trivial_before = mem._store.get(ids["trivial"])
emotional_before = mem._store.get(ids["kinda_like"])
t_str = f"{trivial_before.working_strength + trivial_before.core_strength:.4f}"
e_str = f"{emotional_before.working_strength + emotional_before.core_strength:.4f}"
print(f"  Before aging (day 5):")
print(f"    Trivial weather thought:  total={t_str}")
print(f"    'I kinda like you':       total={e_str}")

print(f"\n  Simulating 25 more days...")
for _ in range(25):
    mem.consolidate(days=1.0)

trivial_after = mem._store.get(ids["trivial"])
emotional_after = mem._store.get(ids["kinda_like"])
t_str2 = f"{trivial_after.working_strength + trivial_after.core_strength:.4f}"
e_str2 = f"{emotional_after.working_strength + emotional_after.core_strength:.4f}"
print(f"\n  After 30 total days:")
print(f"    Trivial weather thought:  total={t_str2} (nearly gone)")
print(f"    'I kinda like you':       total={e_str2} (still strong)")
print("\n  → High importance memories resist decay. Trivial ones fade.")


# ═══════════════════════════════════════════════════
section("8. CONTRADICTION — memory correction")
# ═══════════════════════════════════════════════════

print("  Original: 'SaltyHall uses Supabase'")
ids["planetscale"] = mem.add(
    "SaltyHall migrated from Supabase to PlanetScale",
    type="factual", importance=0.6,
    entities=[("SaltyHall", "uses"), ("PlanetScale", "backend")],
    contradicts=ids["supabase"],
)
print("  New:      'SaltyHall migrated to PlanetScale' (contradicts old)")

print(f"\n  Recalling 'SaltyHall database':\n")
results = mem.recall("SaltyHall database", limit=5)
show_results(results)
print("  → Contradicted memories are flagged and get 0.3x confidence penalty")


# ═══════════════════════════════════════════════════
section("9. UPDATE MEMORY — correction chain")
# ═══════════════════════════════════════════════════

print("  Original: 'potato prefers action over discussion'")
new_id = mem.update_memory(ids["preference"], 
    "potato prefers action over discussion but enjoys brainstorming sessions")
print("  Updated:  'potato prefers action... but enjoys brainstorming'")

old = mem._store.get(ids["preference"])
new = mem._store.get(new_id)
print(f"\n  Old memory contradicted_by: {old.contradicted_by[:8]}...")
print(f"  New memory contradicts:     {new.contradicts[:8]}...")
print("  → Creates a correction chain, preserving history")


# ═══════════════════════════════════════════════════
section("10. PIN — protect important memories")
# ═══════════════════════════════════════════════════

mem.pin(ids["kinda_like"])
entry = mem._store.get(ids["kinda_like"])
ws = entry.working_strength
print(f"  Pinned 'I kinda like you' — strength: {ws:.4f}")

mem.consolidate(days=10.0)
entry2 = mem._store.get(ids["kinda_like"])
print(f"  After 10 more days:          strength: {entry2.working_strength:.4f}")
print("  → Pinned memories don't decay. They're permanent.")

mem.unpin(ids["kinda_like"])
print("  Unpinned.")


# ═══════════════════════════════════════════════════
section("11. DOWNSCALING — synaptic homeostasis")
# ═══════════════════════════════════════════════════

all_before = mem._store.all()
total_before = sum(e.working_strength + e.core_strength for e in all_before)
print(f"  Total system strength before: {total_before:.4f}")

result = mem.downscale(factor=0.9)
print(f"  Downscaled {result['n_scaled']} memories by 0.9x")

all_after = mem._store.all()
total_after = sum(e.working_strength + e.core_strength for e in all_after)
print(f"  Total system strength after:  {total_after:.4f}")
print("\n  → Like sleep: globally reduces activation to prevent overload")


# ═══════════════════════════════════════════════════
section("12. FORGETTING — prune weak memories")
# ═══════════════════════════════════════════════════

trivial = mem._store.get(ids["trivial"])
print(f"  Trivial memory strength: {trivial.working_strength + trivial.core_strength:.6f}")
print(f"  Trivial memory layer:    {trivial.layer.value}")

mem.forget(threshold=0.01)
trivial2 = mem._store.get(ids["trivial"])
print(f"  After forget():          {trivial2.layer.value}")
print("\n  → Weak memories get archived (not deleted — just inaccessible)")


# ═══════════════════════════════════════════════════
section("13. EXPORT & STATS")
# ═══════════════════════════════════════════════════

stats = mem.stats()
print(f"  Total memories: {stats['total_memories']}")
print(f"  Types: {list(stats['by_type'].keys())}")
print(f"  Layers: {stats['layers']}")

export_path = os.path.join(tmpdir, "export.db")
mem.export(export_path)
print(f"\n  Exported to: {export_path}")
print(f"  File size: {os.path.getsize(export_path)} bytes")
print("  → Single .db file. Portable. Copy it anywhere.")


# ═══════════════════════════════════════════════════
section("14. CONFIG PRESETS")
# ═══════════════════════════════════════════════════

presets = {
    "chatbot": MemoryConfig.chatbot(),
    "task_agent": MemoryConfig.task_agent(),
    "personal_assistant": MemoryConfig.personal_assistant(),
    "researcher": MemoryConfig.researcher(),
}

print(f"  {'Preset':<22s} {'μ₁ (work decay)':<16s} {'μ₂ (core decay)':<16s} {'Replay %':<10s} {'Forget θ'}")
print(f"  {'─'*22} {'─'*16} {'─'*16} {'─'*10} {'─'*10}")
for name, cfg in presets.items():
    print(f"  {name:<22s} {cfg.mu1:<16.3f} {cfg.mu2:<16.4f} {cfg.interleave_ratio:<10.0%} {cfg.forget_threshold}")


# ═══════════════════════════════════════════════════
section("✨ DONE")
# ═══════════════════════════════════════════════════

print("  Engram: neuroscience-grounded memory for AI agents.")
print("  Zero dependencies. Pure Python. Single SQLite file.")
print(f"  https://github.com/tonitangpotato/engram")
print()

mem.close()

# Cleanup
import shutil
shutil.rmtree(tmpdir, ignore_errors=True)
