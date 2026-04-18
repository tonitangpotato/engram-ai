#!/usr/bin/env python3
"""
Repeated Access Benchmark (RAB)

Tests ACT-R's core claim: memories accessed more often become more accessible.

Scenario: Simulates an agent conversation where some facts are:
1. Mentioned once (low frequency)
2. Mentioned/recalled multiple times (high frequency)
3. Co-recalled together (Hebbian association)

Then tests: does the system correctly prioritize frequently-accessed memories?
"""

import json
import sys
import time
import random
from pathlib import Path
from dataclasses import dataclass, field
from typing import List, Dict, Optional
from collections import defaultdict

sys.path.insert(0, str(Path(__file__).parent.parent))


@dataclass
class AccessPattern:
    """Defines how a memory should be accessed during simulation"""
    content: str
    initial_add: bool = True  # Add at start
    recall_times: int = 0     # How many times recalled during simulation
    importance: float = 0.5
    category: str = "fact"
    

@dataclass
class TestCase:
    """A test case for repeated access"""
    id: str
    description: str
    memories: List[AccessPattern]
    query: str
    expected_top: str          # What should rank #1
    expected_low: List[str]    # What should rank lower
    test_type: str             # "frequency", "hebbian", "combined"


def generate_frequency_tests() -> List[TestCase]:
    """Generate tests for frequency-based recall boosting"""
    cases = []
    
    # Test 1: Food preference with repeated mentions
    cases.append(TestCase(
        id="freq_01",
        description="User mentions pizza 5x, sushi 1x - pizza should win",
        memories=[
            AccessPattern("User likes sushi", recall_times=0),
            AccessPattern("User loves pizza", recall_times=5),
        ],
        query="What food does the user prefer?",
        expected_top="pizza",
        expected_low=["sushi"],
        test_type="frequency"
    ))
    
    # Test 2: Project priority with access frequency
    cases.append(TestCase(
        id="freq_02", 
        description="Project Alpha accessed 8x, Beta 1x - Alpha should be primary",
        memories=[
            AccessPattern("Working on Project Alpha - ML pipeline", recall_times=8),
            AccessPattern("Project Beta - documentation task", recall_times=1),
        ],
        query="What is the user's main project?",
        expected_top="Alpha",
        expected_low=["Beta"],
        test_type="frequency"
    ))
    
    # Test 3: Contact preference
    cases.append(TestCase(
        id="freq_03",
        description="Mentions calling Mom 6x, Dad 1x",
        memories=[
            AccessPattern("Need to call Mom", recall_times=6),
            AccessPattern("Should call Dad sometime", recall_times=1),
        ],
        query="Who does the user call most often?",
        expected_top="Mom",
        expected_low=["Dad"],
        test_type="frequency"
    ))
    
    # Test 4: Tool preference
    cases.append(TestCase(
        id="freq_04",
        description="VSCode mentioned 10x, Vim 2x",
        memories=[
            AccessPattern("Uses VSCode for development", recall_times=10),
            AccessPattern("Tried Vim once", recall_times=2),
        ],
        query="What editor does the user prefer?",
        expected_top="VSCode",
        expected_low=["Vim"],
        test_type="frequency"
    ))
    
    # Test 5: Location frequency
    cases.append(TestCase(
        id="freq_05",
        description="Coffee shop A visited 7x, B visited 1x",
        memories=[
            AccessPattern("Works from Blue Bottle coffee shop", recall_times=7),
            AccessPattern("Went to Starbucks once", recall_times=1),
        ],
        query="Where does the user usually work from?",
        expected_top="Blue Bottle",
        expected_low=["Starbucks"],
        test_type="frequency"
    ))
    
    # Generate more programmatically
    templates = [
        {
            "high": "User's favorite color is blue",
            "low": "User mentioned liking green once", 
            "query": "What's the user's favorite color?",
            "expected": "blue",
            "wrong": ["green"]
        },
        {
            "high": "User runs every morning",
            "low": "User tried swimming once",
            "query": "What exercise does the user do?", 
            "expected": "runs",
            "wrong": ["swimming"]
        },
        {
            "high": "User reads sci-fi books",
            "low": "User bought a mystery novel",
            "query": "What genre does the user read?",
            "expected": "sci-fi",
            "wrong": ["mystery"]
        },
    ]
    
    for i, t in enumerate(templates):
        cases.append(TestCase(
            id=f"freq_{i+6:02d}",
            description=f"Frequency test: {t['expected']} should win",
            memories=[
                AccessPattern(t["high"], recall_times=random.randint(5, 10)),
                AccessPattern(t["low"], recall_times=random.randint(0, 1)),
            ],
            query=t["query"],
            expected_top=t["expected"],
            expected_low=t["wrong"],
            test_type="frequency"
        ))
    
    return cases


def generate_hebbian_tests() -> List[TestCase]:
    """Generate tests for Hebbian co-activation learning"""
    cases = []
    
    # Test: Co-recalled items should associate
    cases.append(TestCase(
        id="hebb_01",
        description="Coffee and morning routine co-recalled 5x",
        memories=[
            AccessPattern("User drinks coffee", recall_times=5),
            AccessPattern("User has morning standup at 9am", recall_times=5),
            AccessPattern("User eats lunch at noon", recall_times=1),
        ],
        query="What's the user's morning routine?",
        expected_top="coffee",  # Should pull in associated coffee memory
        expected_low=["lunch"],
        test_type="hebbian"
    ))
    
    cases.append(TestCase(
        id="hebb_02",
        description="Python and data science co-recalled",
        memories=[
            AccessPattern("User knows Python", recall_times=6),
            AccessPattern("User does data science work", recall_times=6),
            AccessPattern("User learned Java in college", recall_times=1),
        ],
        query="What does the user use for data science?",
        expected_top="Python",
        expected_low=["Java"],
        test_type="hebbian"
    ))
    
    cases.append(TestCase(
        id="hebb_03",
        description="Weekend hiking co-recalled",
        memories=[
            AccessPattern("User goes hiking", recall_times=4),
            AccessPattern("User's weekends are for outdoor activities", recall_times=4),
            AccessPattern("User watches TV sometimes", recall_times=1),
        ],
        query="What does the user do on weekends?",
        expected_top="hiking",
        expected_low=["TV"],
        test_type="hebbian"
    ))
    
    return cases


def generate_combined_tests() -> List[TestCase]:
    """Tests combining frequency + recency + importance"""
    cases = []
    
    # High frequency should beat recent-but-rare
    cases.append(TestCase(
        id="comb_01",
        description="Old but frequently accessed vs new but rare",
        memories=[
            AccessPattern("User's main skill is Python (mentioned often)", recall_times=10),
            AccessPattern("User just tried Rust (mentioned once)", recall_times=1),
        ],
        query="What's the user's primary programming language?",
        expected_top="Python",
        expected_low=["Rust"],
        test_type="combined"
    ))
    
    # Important + frequent should beat everything
    cases.append(TestCase(
        id="comb_02",
        description="Important allergy info should persist",
        memories=[
            AccessPattern("User is allergic to peanuts", recall_times=3, importance=0.9),
            AccessPattern("User had a sandwich for lunch", recall_times=1, importance=0.1),
        ],
        query="Any food safety concerns for the user?",
        expected_top="peanuts",
        expected_low=["sandwich"],
        test_type="combined"
    ))
    
    return cases


class RepeatedAccessBenchmark:
    """Run the repeated access benchmark"""
    
    def __init__(self, memory_class):
        self.memory_class = memory_class
        self.results = defaultdict(list)
        
    def run_test(self, case: TestCase, verbose: bool = False) -> Dict:
        """Run a single test case"""
        # Create fresh memory
        mem = self.memory_class()
        
        # Add all memories
        memory_ids = {}
        for pattern in case.memories:
            if pattern.initial_add:
                mid = mem.add(
                    content=pattern.content,
                    importance=pattern.importance,
                )
                memory_ids[pattern.content] = mid
        
        # Simulate recalls (this is the key part!)
        for pattern in case.memories:
            for _ in range(pattern.recall_times):
                # Recall memories related to this content
                # This should boost access count in ACT-R
                mem.recall(pattern.content[:20], limit=3)
        
        # Final query
        results = mem.recall(case.query, limit=5)
        
        # Check if expected is in top result
        top_content = results[0]["content"].lower() if results else ""
        correct = case.expected_top.lower() in top_content
        
        # Check wrong answers aren't ranked higher
        for wrong in case.expected_low:
            if wrong.lower() in top_content:
                correct = False
                break
        
        if verbose:
            status = "✓" if correct else "✗"
            print(f"  {status} [{case.test_type}] {case.id}: {case.description[:50]}...")
            if not correct:
                print(f"    Expected '{case.expected_top}' in top, got: {top_content[:60]}...")
        
        return {
            "id": case.id,
            "test_type": case.test_type,
            "correct": correct,
            "expected": case.expected_top,
            "got": top_content[:100],
        }
    
    def run_all(self, verbose: bool = False) -> Dict:
        """Run all test cases"""
        all_cases = (
            generate_frequency_tests() + 
            generate_hebbian_tests() + 
            generate_combined_tests()
        )
        
        print(f"Running {len(all_cases)} test cases...")
        
        results_by_type = defaultdict(lambda: {"correct": 0, "total": 0})
        all_results = []
        
        for case in all_cases:
            result = self.run_test(case, verbose)
            all_results.append(result)
            
            results_by_type[case.test_type]["total"] += 1
            if result["correct"]:
                results_by_type[case.test_type]["correct"] += 1
        
        # Summary
        total_correct = sum(r["correct"] for r in results_by_type.values())
        total = sum(r["total"] for r in results_by_type.values())
        
        return {
            "total_correct": total_correct,
            "total": total,
            "accuracy": total_correct / total if total > 0 else 0,
            "by_type": {
                k: {
                    "accuracy": v["correct"] / v["total"] if v["total"] > 0 else 0,
                    **v
                }
                for k, v in results_by_type.items()
            },
            "details": all_results,
        }


def run_with_engram():
    """Run benchmark with engram memory"""
    from engram import Memory
    
    class EngramWrapper:
        def __init__(self):
            self.mem = Memory(":memory:")
            
        def add(self, content: str, importance: float = 0.5) -> str:
            return self.mem.add(content, importance=importance)
            
        def recall(self, query: str, limit: int = 5) -> List[Dict]:
            # Sanitize query for FTS5 (remove special chars)
            import re
            sanitized = re.sub(r'[^\w\s]', ' ', query)
            sanitized = ' '.join(sanitized.split())
            results = self.mem.recall(sanitized, limit=limit)
            return [{"content": r.get("content", ""), **r} for r in results]
    
    print("\n" + "=" * 60)
    print("REPEATED ACCESS BENCHMARK - engram (ACT-R)")
    print("=" * 60)
    
    benchmark = RepeatedAccessBenchmark(EngramWrapper)
    results = benchmark.run_all(verbose=True)
    
    print("\n" + "-" * 60)
    print("RESULTS")
    print("-" * 60)
    print(f"Overall: {results['accuracy']:.1%} ({results['total_correct']}/{results['total']})")
    print()
    for test_type, data in results["by_type"].items():
        print(f"  {test_type}: {data['accuracy']:.1%} ({data['correct']}/{data['total']})")
    
    return results


def run_with_baseline():
    """Run benchmark with simple vector-like baseline (no access tracking)"""
    
    class BaselineMemory:
        """Simple memory with no access count tracking"""
        def __init__(self):
            self.memories = []
            
        def add(self, content: str, importance: float = 0.5) -> str:
            self.memories.append({"content": content, "importance": importance})
            return str(len(self.memories))
            
        def recall(self, query: str, limit: int = 5) -> List[Dict]:
            # Simple keyword matching (no access boost)
            query_words = set(query.lower().split())
            scored = []
            for m in self.memories:
                content_words = set(m["content"].lower().split())
                overlap = len(query_words & content_words)
                scored.append((m, overlap))
            scored.sort(key=lambda x: x[1], reverse=True)
            return [{"content": m["content"]} for m, _ in scored[:limit]]
    
    print("\n" + "=" * 60)
    print("REPEATED ACCESS BENCHMARK - Baseline (No Access Tracking)")
    print("=" * 60)
    
    benchmark = RepeatedAccessBenchmark(BaselineMemory)
    results = benchmark.run_all(verbose=True)
    
    print("\n" + "-" * 60)
    print("RESULTS")
    print("-" * 60)
    print(f"Overall: {results['accuracy']:.1%} ({results['total_correct']}/{results['total']})")
    print()
    for test_type, data in results["by_type"].items():
        print(f"  {test_type}: {data['accuracy']:.1%} ({data['correct']}/{data['total']})")
    
    return results


def main():
    print("\n" + "=" * 70)
    print("REPEATED ACCESS BENCHMARK")
    print("Tests whether access frequency boosts memory retrieval (ACT-R claim)")
    print("=" * 70)
    
    engram_results = run_with_engram()
    baseline_results = run_with_baseline()
    
    print("\n" + "=" * 70)
    print("COMPARISON")
    print("=" * 70)
    print(f"{'System':<25} {'Frequency':<12} {'Hebbian':<12} {'Combined':<12} {'Overall':<12}")
    print("-" * 70)
    
    for name, results in [("engram (ACT-R)", engram_results), ("Baseline (no ACT-R)", baseline_results)]:
        freq = results["by_type"].get("frequency", {}).get("accuracy", 0)
        hebb = results["by_type"].get("hebbian", {}).get("accuracy", 0)
        comb = results["by_type"].get("combined", {}).get("accuracy", 0)
        overall = results["accuracy"]
        print(f"{name:<25} {freq:<12.1%} {hebb:<12.1%} {comb:<12.1%} {overall:<12.1%}")
    
    # Calculate improvement
    improvement = engram_results["accuracy"] - baseline_results["accuracy"]
    print(f"\nACT-R improvement: {improvement:+.1%}")


if __name__ == "__main__":
    main()
