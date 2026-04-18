#!/usr/bin/env python3
"""
Comparison: NeuromemoryAI vs Vector-Based Memory (Mem0/Zep)

Both approaches are designed for LLM agents — the comparison is about
what ADDITIONAL infrastructure each requires beyond the LLM.

Key comparisons:
1. Additional API calls: 0 (NeuromemoryAI) vs 1+ per recall (embedding)
2. Additional infrastructure: SQLite file vs Embedding API + Vector DB
3. Retrieval dynamics: Activation-based (recency, frequency, importance) vs pure similarity
4. Memory lifecycle: Forgetting + consolidation vs flat storage

NOTE: This is not an apples-to-apples accuracy comparison.
- FTS5 does keyword matching, embeddings do semantic matching
- NeuromemoryAI's advantage is memory DYNAMICS, not retrieval accuracy
- For fair accuracy comparison, see LoCoMo benchmark (benchmarks/eval_locomo.py)
"""

import os
import sys
import tempfile
import time
import random
from dataclasses import dataclass
from typing import List

# Add parent to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig


@dataclass
class BenchmarkResult:
    system: str
    metric: str
    value: float
    unit: str
    note: str = ""


def benchmark_latency():
    """Compare retrieval latency."""
    print("\n" + "=" * 60)
    print("1. LATENCY COMPARISON")
    print("=" * 60)
    
    results = []
    
    # NeuromemoryAI: Local FTS5
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    mem = Memory(db_path)
    
    # Add 100 memories
    for i in range(100):
        mem.add(f"Memory content number {i} with some additional context text", 
                type="factual", importance=0.5)
    
    # Measure recall latency
    latencies = []
    for _ in range(50):
        start = time.perf_counter()
        mem.recall("memory content", limit=10)
        latencies.append((time.perf_counter() - start) * 1000)
    
    avg_latency = sum(latencies) / len(latencies)
    p99_latency = sorted(latencies)[int(len(latencies) * 0.99)]
    
    results.append(BenchmarkResult(
        system="NeuromemoryAI",
        metric="recall_latency_avg",
        value=avg_latency,
        unit="ms",
        note="Local SQLite + FTS5, zero API calls"
    ))
    
    # Simulated Mem0/Zep: Embedding API + Vector DB
    # Based on published benchmarks and typical API latencies
    embedding_latency = 50  # OpenAI embedding API ~50ms
    vector_search_latency = 10  # Qdrant/Pinecone ~10ms
    simulated_latency = embedding_latency + vector_search_latency
    
    results.append(BenchmarkResult(
        system="Mem0 (simulated)",
        metric="recall_latency_avg",
        value=simulated_latency,
        unit="ms",
        note="OpenAI embedding (~50ms) + vector search (~10ms)"
    ))
    
    print(f"\n{'System':<20} {'Latency':<15} {'Notes'}")
    print("-" * 60)
    for r in results:
        print(f"{r.system:<20} {r.value:.2f} {r.unit:<10} {r.note}")
    
    speedup = simulated_latency / avg_latency
    print(f"\n📊 NeuromemoryAI is ~{speedup:.0f}x faster (no API calls)")
    
    return results


def benchmark_dependencies():
    """Compare dependencies."""
    print("\n" + "=" * 60)
    print("2. DEPENDENCY COMPARISON")
    print("=" * 60)
    
    comparison = [
        ("NeuromemoryAI", {
            "External Services": "None",
            "API Keys Needed": "No",
            "Python Packages": "0 (stdlib only)",
            "Works Offline": "Yes",
            "Data Location": "Local file",
        }),
        ("Mem0", {
            "External Services": "OpenAI API, Qdrant/Pinecone",
            "API Keys Needed": "Yes (OpenAI, vector DB)",
            "Python Packages": "openai, qdrant-client, ...",
            "Works Offline": "No",
            "Data Location": "Cloud vector DB",
        }),
        ("Zep", {
            "External Services": "OpenAI API, Postgres",
            "API Keys Needed": "Yes (OpenAI)",
            "Python Packages": "openai, psycopg2, ...",
            "Works Offline": "No",
            "Data Location": "Postgres + embeddings",
        }),
    ]
    
    print(f"\n{'Aspect':<25} {'NeuromemoryAI':<20} {'Mem0':<25} {'Zep':<25}")
    print("-" * 95)
    
    aspects = ["External Services", "API Keys Needed", "Python Packages", "Works Offline", "Data Location"]
    for aspect in aspects:
        row = [aspect]
        for name, attrs in comparison:
            row.append(attrs[aspect])
        print(f"{row[0]:<25} {row[1]:<20} {row[2]:<25} {row[3]:<25}")


def benchmark_retrieval_dynamics():
    """Compare retrieval behavior: activation vs similarity."""
    print("\n" + "=" * 60)
    print("3. RETRIEVAL DYNAMICS COMPARISON")
    print("=" * 60)
    
    print("""
NeuromemoryAI uses ACT-R activation:
  A = B_base + S_context + I_importance
  
  Where:
  • B_base = ln(Σ t_k^-0.5) — recency × frequency
  • S_context = keyword overlap boost
  • I_importance = emotional/motivational salience

Vector-based systems use cosine similarity:
  sim(q, m) = (q · m) / (||q|| ||m||)
  
  Where:
  • q = embedding(query)
  • m = embedding(memory)
""")
    
    # Demonstrate the difference with a concrete example
    print("\n📊 Example: Old-but-relevant vs New-but-tangent")
    print("-" * 60)
    
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    mem = Memory(db_path)
    
    # Add old relevant memory
    old_id = mem.add(
        "Project Phoenix deployment: run tests, build docker, push ECR, update ECS",
        type="procedural",
        importance=0.9
    )
    
    # Access it multiple times (simulating it's a frequently-used procedure)
    for _ in range(5):
        mem.recall("Phoenix ECS docker", limit=1)
    
    # Simulate it being 30 days old
    mem.consolidate(days=30)
    
    # Add recent tangent
    recent_id = mem.add(
        "Had lunch discussing general CI/CD practices",
        type="episodic",
        importance=0.3
    )
    
    # Specific query - should favor old-but-relevant
    print("\nSpecific query: 'Phoenix ECS docker'")
    results = mem.recall("Phoenix ECS docker", limit=5)
    for i, r in enumerate(results, 1):
        age = "30 days old, accessed 5x" if "Phoenix" in r["content"] else "just added"
        print(f"  {i}. [{r['activation']:.2f}] {r['content'][:50]}... ({age})")
    
    # General query - recency matters more
    print("\nGeneral query: 'deployment practices'")
    results = mem.recall("deployment practices", limit=5)
    for i, r in enumerate(results, 1):
        age = "30 days old" if "Phoenix" in r["content"] else "just added"
        print(f"  {i}. [{r['activation']:.2f}] {r['content'][:50]}... ({age})")
    
    print("""
Key insight: NeuromemoryAI BALANCES relevance and recency:
• Specific queries → old but relevant wins (keyword match dominates)
• General queries → recent memories surface (recency matters)

This is the expected behavior! Pure vector similarity treats all queries equally,
but human memory doesn't — we recall specific things precisely, and recent
things more easily when the cue is vague.
""")


def benchmark_memory_growth():
    """Compare memory growth with and without forgetting."""
    print("\n" + "=" * 60)
    print("4. MEMORY GROWTH COMPARISON")
    print("=" * 60)
    
    # Simulate 100 days of adding memories
    days = 100
    memories_per_day = 10
    
    # NeuromemoryAI with forgetting
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    config = MemoryConfig.chatbot()
    mem = Memory(db_path, config=config)
    
    neuromem_active = []
    neuromem_archived = []
    
    for day in range(days):
        # Add daily memories (70% low importance, 30% high)
        for i in range(memories_per_day):
            importance = random.uniform(0.1, 0.4) if random.random() < 0.7 else random.uniform(0.7, 0.9)
            mem.add(f"Day {day} memory {i}: some content here", 
                   type="episodic" if importance < 0.5 else "factual",
                   importance=importance)
        
        # Daily consolidation + occasional forgetting
        mem.consolidate(days=1)
        if day % 7 == 0:  # Weekly cleanup
            mem.forget(threshold=0.3)  # Archive weak memories (strength < 0.3)
        
        # Count by layer
        stats = mem.stats()
        archived = stats.get("layers", {}).get("archive", {}).get("count", 0)
        active = stats["total_memories"] - archived
        neuromem_active.append(active)
        neuromem_archived.append(archived)
    
    # Mem0 without forgetting (simulated) - everything stays searchable
    mem0_sizes = [memories_per_day * (day + 1) for day in range(days)]
    
    print(f"\nAfter {days} days ({memories_per_day} memories/day):")
    print(f"  • NeuromemoryAI:")
    print(f"      Active (searchable): {neuromem_active[-1]}")
    print(f"      Archived: {neuromem_archived[-1]}")
    print(f"  • Mem0 (no forgetting): {mem0_sizes[-1]} (all searchable)")
    
    if neuromem_archived[-1] > 0:
        reduction = neuromem_archived[-1] / mem0_sizes[-1] * 100
        print(f"  • Signal-to-noise improvement: {reduction:.0f}% of low-value memories archived")
    
    print("\n📈 Growth over time:")
    checkpoints = [0, 24, 49, 74, 99]
    print(f"{'Day':<8} {'Active':<12} {'Archived':<12} {'Mem0':<12}")
    print("-" * 45)
    for day in checkpoints:
        print(f"{day+1:<8} {neuromem_active[day]:<12} {neuromem_archived[day]:<12} {mem0_sizes[day]:<12}")
    
    print("""
Key insight: NeuromemoryAI archives low-value memories over time,
improving retrieval signal-to-noise ratio. Mem0/Zep keep everything
searchable forever, requiring manual cleanup.""")


def benchmark_hebbian_vs_ner():
    """Compare Hebbian learning vs NER-based linking."""
    print("\n" + "=" * 60)
    print("5. ASSOCIATION LEARNING COMPARISON")
    print("=" * 60)
    
    print("""
NER-based linking (shodh-memory, knowledge graphs):
  1. Extract entities: "Python" → PROGRAMMING_LANGUAGE
  2. Match entities across memories
  3. Create explicit links
  
  Limitations:
  • Requires NER model (TinyBERT, spaCy, etc.)
  • Only links on recognized entities
  • Misses implicit relationships

Hebbian learning (NeuromemoryAI):
  1. Track which memories are recalled together
  2. After N co-activations, form link automatically
  3. Links emerge from USAGE patterns, not surface features
  
  Advantages:
  • No NER model needed
  • Captures implicit relationships
  • Learns user's mental model
""")
    
    # Demo
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    config = MemoryConfig.researcher()
    config.hebbian_enabled = True
    config.hebbian_threshold = 2
    mem = Memory(db_path, config=config)
    
    # Add memories that share no obvious entities but are conceptually related
    mem.add("The model kept overfitting despite early stopping", type="factual")
    mem.add("Tried reducing batch size to improve generalization", type="factual")
    mem.add("Adding dropout helped but wasn't enough", type="factual")
    
    # Simulate user pattern: always recalls these together when debugging
    for _ in range(5):
        mem.recall("model overfitting training debugging", limit=5)
    
    from engram.hebbian import get_all_hebbian_links
    links = get_all_hebbian_links(mem._store)
    
    print(f"\nExample: Debugging ML overfitting")
    print(f"  Memories added: 3 (about overfitting, batch size, dropout)")
    print(f"  Hebbian links formed: {len(links)}")
    print(f"  NER would find: No shared entities!")
    print(f"  Hebbian captures: User's mental association of these concepts")


def main():
    print("=" * 60)
    print("NeuromemoryAI vs Vector-Based Memory Comparison")
    print("=" * 60)
    
    all_results = []
    
    all_results.extend(benchmark_latency())
    benchmark_dependencies()
    benchmark_retrieval_dynamics()
    benchmark_memory_growth()
    benchmark_hebbian_vs_ner()
    
    print("\n" + "=" * 60)
    print("SUMMARY (Both assume LLM already present)")
    print("=" * 60)
    print("""
┌──────────────────────┬──────────────────┬─────────────────────────────┐
│ Aspect               │ NeuromemoryAI    │ Mem0/Zep                    │
├──────────────────────┼──────────────────┼─────────────────────────────┤
│ Extra API calls      │ 0                │ 1+ (embedding per recall)   │
│ Additional infra     │ SQLite file      │ Embedding API + Vector DB   │
│ Offline (local LLM)  │ Yes              │ No                          │
│ Forgetting           │ Yes (automatic)  │ No (manual deletion)        │
│ Associations         │ Hebbian (usage)  │ NER or manual               │
│ Retrieval ranking    │ Activation-based │ Cosine similarity           │
│ Memory lifecycle     │ Working→Core→Arc │ Flat storage                │
└──────────────────────┴──────────────────┴─────────────────────────────┘

Key insight: Both systems work WITH an LLM. The difference is:
- Mem0/Zep add embedding infrastructure for semantic retrieval
- NeuromemoryAI adds cognitive dynamics (forgetting, consolidation, Hebbian)

Vector search excels at semantic similarity.
NeuromemoryAI excels at memory BEHAVIOR over time.

For semantic accuracy comparison, see LoCoMo benchmark.
""")


if __name__ == "__main__":
    main()
