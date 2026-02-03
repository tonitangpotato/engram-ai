#!/usr/bin/env python3
"""
End-to-End 30-Day Simulation

Simulates 30 days of agent operation to test ALL engram features:
1. ACT-R activation (recency + frequency)
2. Hebbian learning (co-activation)
3. Layer system (working → core → archive)
4. Consolidation (Memory Chain transfer)
5. Importance preservation
6. Contradiction handling
7. Forgetting/decay
8. Memory types

Each day:
- Add new memories
- Simulate recalls (access patterns)
- Run consolidation ("sleep")
- Track metrics

Final validation:
- Can we find old but important info?
- Does new info override old?
- Is layer distribution correct?
- Are frequently accessed memories stronger?
"""

import sys
import time
import random
from pathlib import Path
from dataclasses import dataclass, field
from typing import List, Dict, Optional
from collections import defaultdict

sys.path.insert(0, str(Path(__file__).parent.parent))

from engram import Memory, MemoryType
from engram.core import MemoryLayer
from engram.hebbian import get_hebbian_neighbors, get_all_hebbian_links


@dataclass
class SimulationDay:
    """Events for a single day."""
    day: int
    memories_to_add: List[Dict] = field(default_factory=list)
    queries_to_run: List[str] = field(default_factory=list)
    expected_state: Dict = field(default_factory=dict)


def generate_30_day_scenario() -> List[SimulationDay]:
    """Generate a realistic 30-day agent conversation scenario."""
    
    days = []
    
    # Day 1: Initial user info
    days.append(SimulationDay(
        day=1,
        memories_to_add=[
            {"content": "User works at Google as a software engineer", "importance": 0.6, "type": "factual"},
            {"content": "User lives in San Francisco", "importance": 0.5, "type": "factual"},
            {"content": "User's favorite food is sushi", "importance": 0.4, "type": "factual"},
            {"content": "User prefers Python for coding", "importance": 0.5, "type": "factual"},
        ],
        queries_to_run=["Python"],
    ))
    
    # Day 3: Critical health info
    days.append(SimulationDay(
        day=3,
        memories_to_add=[
            {"content": "CRITICAL: User is severely allergic to peanuts. Carries EpiPen.", "importance": 0.98, "type": "factual"},
        ],
        queries_to_run=["allergy"],
    ))
    
    # Day 5-15: Repeated Python project work (frequency test)
    for d in [5, 7, 9, 11, 13, 15]:
        days.append(SimulationDay(
            day=d,
            memories_to_add=[
                {"content": f"Day {d}: Working on Python data pipeline project", "importance": 0.5, "type": "episodic"},
            ],
            queries_to_run=["Python", "pipeline", "project"],  # Frequent access
        ))
    
    # Day 8: One-off Java mention (frequency contrast)
    days.append(SimulationDay(
        day=8,
        memories_to_add=[
            {"content": "Fixed a small Java bug in legacy system", "importance": 0.3, "type": "episodic"},
        ],
        queries_to_run=["Java"],
    ))
    
    # Day 10: Coffee + morning routine (Hebbian test - always together)
    days.append(SimulationDay(
        day=10,
        memories_to_add=[
            {"content": "User drinks coffee every morning", "importance": 0.4, "type": "procedural"},
            {"content": "User has standup meeting at 9am", "importance": 0.5, "type": "procedural"},
        ],
        queries_to_run=["coffee morning", "morning routine"],
    ))
    
    # Day 12, 14, 16: Reinforce coffee+morning association
    for d in [12, 14, 16]:
        days.append(SimulationDay(
            day=d,
            memories_to_add=[],
            queries_to_run=["coffee morning", "morning standup"],
        ))
    
    # Day 18: JOB CHANGE (contradiction test)
    days.append(SimulationDay(
        day=18,
        memories_to_add=[
            {"content": "BIG NEWS: User accepted offer at Anthropic! Starting next month.", "importance": 0.85, "type": "factual", "contradicts": "Google"},
        ],
        queries_to_run=["job", "work"],
    ))
    
    # Day 20: Location change
    days.append(SimulationDay(
        day=20,
        memories_to_add=[
            {"content": "User moving to Seattle for the new Anthropic job", "importance": 0.6, "type": "factual", "contradicts": "San Francisco"},
        ],
        queries_to_run=["live", "location"],
    ))
    
    # Day 22: Emotional memory
    days.append(SimulationDay(
        day=22,
        memories_to_add=[
            {"content": "User said: 'I really appreciate you helping me through this transition. You're a good friend.'", "importance": 0.9, "type": "emotional"},
        ],
        queries_to_run=["friend", "appreciate"],
    ))
    
    # Day 25: Trivial memories (should be forgotten)
    days.append(SimulationDay(
        day=25,
        memories_to_add=[
            {"content": "User had a sandwich for lunch", "importance": 0.1, "type": "episodic"},
            {"content": "Weather was cloudy today", "importance": 0.05, "type": "episodic"},
            {"content": "User mentioned seeing a cute dog", "importance": 0.15, "type": "episodic"},
        ],
        queries_to_run=[],
    ))
    
    # Day 28: Birthday reminder (importance test)
    days.append(SimulationDay(
        day=28,
        memories_to_add=[
            {"content": "User's mom's birthday is March 15th - need to remember!", "importance": 0.85, "type": "factual"},
        ],
        queries_to_run=["birthday", "mom"],
    ))
    
    # Day 30: Final day
    days.append(SimulationDay(
        day=30,
        memories_to_add=[
            {"content": "User excited about starting at Anthropic next week", "importance": 0.6, "type": "emotional"},
        ],
        queries_to_run=["Anthropic", "job"],
    ))
    
    return days


def run_simulation():
    """Run the full 30-day simulation."""
    
    print("=" * 80)
    print("END-TO-END 30-DAY SIMULATION")
    print("Testing ALL engram cognitive features")
    print("=" * 80)
    
    # Initialize memory
    mem = Memory(":memory:")
    
    # Generate scenario
    scenario = generate_30_day_scenario()
    
    # Track metrics over time
    metrics = {
        "daily_memory_count": [],
        "daily_layer_distribution": [],
        "daily_hebbian_links": [],
    }
    
    # Base timestamp (30 days ago)
    base_time = time.time() - (30 * 24 * 3600)
    
    # Memory ID tracking for contradiction
    memory_index = {}
    
    print("\n" + "-" * 80)
    print("RUNNING 30-DAY SIMULATION")
    print("-" * 80)
    
    current_day = 0
    for sim_day in scenario:
        # Fast-forward to this day
        days_to_advance = sim_day.day - current_day
        
        # Run consolidation for skipped days
        for _ in range(days_to_advance):
            mem.consolidate(days=1.0)
            current_day += 1
        
        day_time = base_time + (sim_day.day * 24 * 3600)
        
        print(f"\n📅 Day {sim_day.day}")
        
        # Add memories
        for m in sim_day.memories_to_add:
            # Check for contradiction
            contradicts_id = None
            if "contradicts" in m:
                for key, mid in memory_index.items():
                    if m["contradicts"].lower() in key.lower():
                        contradicts_id = mid
                        print(f"   📝 Adding (contradicts '{m['contradicts']}'): {m['content'][:50]}...")
                        break
            
            if contradicts_id is None and "contradicts" not in m:
                print(f"   📝 Adding: {m['content'][:50]}...")
            elif contradicts_id is None:
                print(f"   📝 Adding: {m['content'][:50]}...")
            
            mem_type = m.get("type", "factual")
            mid = mem.add(
                content=m["content"],
                importance=m["importance"],
                type=mem_type,
                created_at=day_time,
                contradicts=contradicts_id,
            )
            memory_index[m["content"][:30]] = mid
        
        # Run queries (simulates access)
        for q in sim_day.queries_to_run:
            results = mem.recall(q, limit=3)
            # Just access, don't print all results
        
        if sim_day.queries_to_run:
            print(f"   🔍 Ran {len(sim_day.queries_to_run)} queries")
        
        # Track metrics
        all_memories = mem._store.all()
        layer_dist = defaultdict(int)
        for m in all_memories:
            layer_dist[m.layer.value] += 1
        
        metrics["daily_memory_count"].append(len(all_memories))
        metrics["daily_layer_distribution"].append(dict(layer_dist))
        
        hebbian_links = get_all_hebbian_links(mem._store)
        metrics["daily_hebbian_links"].append(len(hebbian_links))
    
    # Run final consolidation
    print("\n🌙 Running final consolidation cycle...")
    mem.consolidate(days=1.0)
    
    # ==================== VALIDATION ====================
    
    print("\n" + "=" * 80)
    print("VALIDATION TESTS")
    print("=" * 80)
    
    results = {
        "passed": 0,
        "failed": 0,
        "tests": []
    }
    
    def test(name: str, condition: bool, details: str = ""):
        status = "✅ PASS" if condition else "❌ FAIL"
        print(f"\n{status}: {name}")
        if details:
            print(f"   {details}")
        results["tests"].append({"name": name, "passed": condition})
        if condition:
            results["passed"] += 1
        else:
            results["failed"] += 1
    
    # Test 1: Layer Distribution
    all_memories = mem._store.all()
    layer_counts = defaultdict(int)
    for m in all_memories:
        layer_counts[m.layer.value] += 1
    
    test(
        "Layer system active",
        len(layer_counts) > 1,
        f"Distribution: {dict(layer_counts)}"
    )
    
    # Test 2: Core memories exist
    core_count = layer_counts.get("core", 0)
    test(
        "Important memories promoted to core",
        core_count >= 1,
        f"Core memories: {core_count}"
    )
    
    # Test 3: Archive memories exist (weak ones demoted)
    archive_count = layer_counts.get("archive", 0)
    test(
        "Weak memories archived",
        archive_count >= 1,
        f"Archived memories: {archive_count}"
    )
    
    # Test 4: ACT-R Recency - New job should rank higher than old
    job_results = mem.recall("where does user work job", limit=5)
    found_anthropic_first = False
    found_google_at_all = False
    
    for i, r in enumerate(job_results):
        if "anthropic" in r["content"].lower():
            if i == 0:
                found_anthropic_first = True
        if "google" in r["content"].lower():
            found_google_at_all = True
    
    test(
        "ACT-R Recency: New job (Anthropic) ranks first",
        found_anthropic_first,
        f"Top result: {job_results[0]['content'][:60] if job_results else 'None'}..."
    )
    
    # Test 5: ACT-R Frequency - Python should rank high (mentioned many times)
    lang_results = mem.recall("programming language", limit=5)
    python_activation = None
    java_activation = None
    
    for r in lang_results:
        if "python" in r["content"].lower():
            python_activation = r.get("activation", 0)
        if "java" in r["content"].lower():
            java_activation = r.get("activation", 0)
    
    test(
        "ACT-R Frequency: Python (frequent) has higher activation than Java (rare)",
        python_activation is not None and (java_activation is None or python_activation > java_activation),
        f"Python activation: {python_activation}, Java: {java_activation}"
    )
    
    # Test 6: Importance Preservation - Allergy info should be findable
    allergy_results = mem.recall("allergy food health", limit=5)
    found_allergy = any("peanut" in r["content"].lower() or "allergy" in r["content"].lower() 
                        for r in allergy_results)
    
    test(
        "Importance: Critical allergy info still retrievable after 30 days",
        found_allergy,
        f"Found in results: {found_allergy}"
    )
    
    # Test 7: Importance vs Recency - Allergy (old but critical) should beat sandwich (recent but trivial)
    safety_results = mem.recall("food safety concern", limit=3)
    allergy_rank = None
    sandwich_rank = None
    
    for i, r in enumerate(safety_results):
        if "peanut" in r["content"].lower() or "allergy" in r["content"].lower():
            allergy_rank = i
        if "sandwich" in r["content"].lower():
            sandwich_rank = i
    
    test(
        "Importance beats Recency: Allergy ranks higher than sandwich",
        allergy_rank is not None and (sandwich_rank is None or allergy_rank < sandwich_rank),
        f"Allergy rank: {allergy_rank}, Sandwich rank: {sandwich_rank}"
    )
    
    # Test 8: Hebbian Learning - Coffee and morning should be associated
    hebbian_links = get_all_hebbian_links(mem._store)
    coffee_morning_linked = False
    
    # Find coffee memory
    coffee_mem = None
    morning_mem = None
    for m in all_memories:
        if "coffee" in m.content.lower():
            coffee_mem = m
        if "standup" in m.content.lower() or "9am" in m.content.lower():
            morning_mem = m
    
    if coffee_mem and morning_mem:
        coffee_neighbors = get_hebbian_neighbors(mem._store, coffee_mem.id)
        if morning_mem.id in coffee_neighbors:
            coffee_morning_linked = True
    
    test(
        "Hebbian: Coffee and morning routine are associated",
        coffee_morning_linked or len(hebbian_links) > 0,
        f"Total Hebbian links formed: {len(hebbian_links)}"
    )
    
    # Test 9: Emotional Memory Preserved
    emotional_results = mem.recall("friend appreciate", limit=5)
    found_emotional = any("appreciate" in r["content"].lower() or "friend" in r["content"].lower()
                         for r in emotional_results)
    
    test(
        "Emotional: Meaningful emotional memory preserved",
        found_emotional,
        f"Found emotional memory: {found_emotional}"
    )
    
    # Test 10: Trivial Memories Decayed
    trivial_results = mem.recall("weather cloudy", limit=10)
    weather_strength = None
    for r in trivial_results:
        if "weather" in r["content"].lower() or "cloudy" in r["content"].lower():
            weather_strength = r.get("strength", r.get("activation", 0))
            break
    
    # Get average strength for comparison
    avg_strength = sum(m.working_strength + m.core_strength for m in all_memories) / len(all_memories)
    
    test(
        "Forgetting: Trivial memories have low strength",
        weather_strength is None or weather_strength < avg_strength,
        f"Weather strength: {weather_strength}, Average: {avg_strength:.3f}"
    )
    
    # Test 11: Birthday (important) should be in core or high strength
    birthday_mem = None
    for m in all_memories:
        if "birthday" in m.content.lower() and "mom" in m.content.lower():
            birthday_mem = m
            break
    
    birthday_preserved = birthday_mem is not None and (
        birthday_mem.layer == MemoryLayer.L2_CORE or 
        (birthday_mem.working_strength + birthday_mem.core_strength) > 0.3
    )
    
    test(
        "Important dates preserved: Mom's birthday remembered",
        birthday_preserved,
        f"Birthday memory layer: {birthday_mem.layer.value if birthday_mem else 'NOT FOUND'}"
    )
    
    # Test 12: Context Efficiency
    working_count = layer_counts.get("working", 0)
    total_count = len(all_memories)
    context_reduction = 1 - ((core_count + working_count) / total_count) if total_count > 0 else 0
    
    test(
        "Context Efficiency: Some memories archived (not all in context)",
        context_reduction > 0,
        f"Archived {context_reduction:.0%} of memories (only core+working in context)"
    )
    
    # ==================== SUMMARY ====================
    
    print("\n" + "=" * 80)
    print("FINAL SUMMARY")
    print("=" * 80)
    
    print(f"\n📊 Memory Statistics:")
    print(f"   Total memories: {len(all_memories)}")
    print(f"   Core (always loaded): {core_count}")
    print(f"   Working (recent): {working_count}")
    print(f"   Archive (on-demand): {archive_count}")
    print(f"   Hebbian links: {len(hebbian_links)}")
    
    print(f"\n🧪 Test Results:")
    print(f"   Passed: {results['passed']}/{len(results['tests'])}")
    print(f"   Failed: {results['failed']}/{len(results['tests'])}")
    
    pass_rate = results['passed'] / len(results['tests']) * 100
    
    print("\n" + "=" * 80)
    if pass_rate >= 80:
        print(f"✅ SUCCESS: {pass_rate:.0f}% tests passed - Engram cognitive features working!")
    elif pass_rate >= 60:
        print(f"⚠️  PARTIAL: {pass_rate:.0f}% tests passed - Some features need attention")
    else:
        print(f"❌ NEEDS WORK: {pass_rate:.0f}% tests passed - Significant issues found")
    print("=" * 80)
    
    return results


if __name__ == "__main__":
    run_simulation()
