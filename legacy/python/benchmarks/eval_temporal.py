#!/usr/bin/env python3
"""
Temporal Dynamics Benchmark Evaluator
Tests engram's ACT-R temporal reasoning capabilities
"""

import json
import re
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path

# Add parent to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from engram import Memory


def sanitize_fts_query(query: str) -> str:
    """Sanitize query for FTS5 by keeping only alphanumeric characters and spaces."""
    sanitized = re.sub(r'[^a-zA-Z0-9\s]', ' ', query)
    sanitized = re.sub(r'\s+', ' ', sanitized).strip()
    if not sanitized:
        sanitized = "memory"
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 'is', 'was', 'are', 'were', 'be', 'been', 'what', 'where', 'when', 'who', 'does', 'do', 'did', 'has', 'have'}
    words = [w for w in sanitized.lower().split() if w not in stop_words and len(w) > 2]
    return ' '.join(words) if words else sanitized


def evaluate_temporal_benchmark(
    benchmark_path: str = "benchmarks/temporal_benchmark.json",
    limit: int | None = None,
    verbose: bool = False,
) -> dict:
    """
    Run temporal dynamics benchmark evaluation
    
    Args:
        benchmark_path: Path to benchmark JSON file
        limit: Limit number of cases per category (for testing)
        verbose: Print detailed output
    
    Returns:
        Results dictionary with metrics per category
    """
    # Load benchmark
    with open(benchmark_path) as f:
        benchmark = json.load(f)
    
    cases = benchmark["cases"]
    if limit:
        # Take first N from each category
        filtered = []
        for cat in ["recency_override", "frequency", "importance", "contradiction"]:
            cat_cases = [c for c in cases if c["category"] == cat][:limit]
            filtered.extend(cat_cases)
        cases = filtered
    
    print(f"Evaluating {len(cases)} cases...")
    
    # Results storage
    results = {
        "recency_override": {"correct": 0, "total": 0, "details": []},
        "frequency": {"correct": 0, "total": 0, "details": []},
        "importance": {"correct": 0, "total": 0, "details": []},
        "contradiction": {"correct": 0, "total": 0, "details": []},
    }
    
    base_time = time.time() - (30 * 24 * 3600)  # 30 days ago in epoch seconds
    
    for case in cases:
        # Fresh memory instance per case
        client = Memory(":memory:")
        
        # Add memories with simulated timestamps
        # CRITICAL: Use created_at to properly simulate temporal order
        memory_ids = []
        for event in case["setup"]:
            event_timestamp = base_time + (event["day"] * 24 * 3600)  # Convert days to seconds
            # Store memory with proper timestamp
            mem_id = client.add(
                content=event["memory"],
                importance=event.get("importance", 0.5),
                created_at=event_timestamp,  # KEY FIX: pass the simulated timestamp
            )
            memory_ids.append(mem_id)
        
        # Query at "day 30"
        sanitized_query = sanitize_fts_query(case["query"])
        retrieved = client.recall(sanitized_query, limit=5)
        
        # Check if expected answer is in top result
        # recalled returns list of MemoryEntry objects or dicts
        def get_content(m):
            if hasattr(m, 'content'):
                return m.content
            elif isinstance(m, dict):
                return m.get('content', '')
            return str(m)
        
        expected_lower = case["expected"].lower()
        wrong_lower = [w.lower() for w in case["wrong"]]
        
        # Score: expected in top-1 and no wrong answers in top-1
        correct = False
        top_content = None
        if retrieved:
            top_content = get_content(retrieved[0])
            top_text = top_content.lower()
            if expected_lower in top_text:
                # Check no wrong answer
                if not any(w in top_text for w in wrong_lower):
                    correct = True
        
        results[case["category"]]["total"] += 1
        if correct:
            results[case["category"]]["correct"] += 1
        
        results[case["category"]]["details"].append({
            "id": case["id"],
            "correct": correct,
            "expected": case["expected"],
            "retrieved": top_content,
            "query": case["query"],
        })
        
        if verbose:
            status = "✓" if correct else "✗"
            print(f"{status} [{case['category']}] {case['id']}: {case['query'][:40]}...")
            if not correct and top_content:
                print(f"   Expected '{expected_lower}' in: {top_content[:60]}...")
    
    # Calculate accuracy
    for cat in results:
        total = results[cat]["total"]
        correct = results[cat]["correct"]
        results[cat]["accuracy"] = correct / total if total > 0 else 0
    
    overall_correct = sum(r["correct"] for r in results.values())
    overall_total = sum(r["total"] for r in results.values())
    
    return {
        "timestamp": datetime.now().isoformat(),
        "system": "engram",
        "total_cases": overall_total,
        "overall_accuracy": overall_correct / overall_total if overall_total > 0 else 0,
        "categories": results,
    }


def print_results(results: dict):
    """Pretty print evaluation results"""
    print("\n" + "=" * 60)
    print("TEMPORAL DYNAMICS BENCHMARK RESULTS")
    print("=" * 60)
    print(f"System: {results['system']}")
    print(f"Total Cases: {results['total_cases']}")
    print(f"Overall Accuracy: {results['overall_accuracy']:.1%}")
    print()
    
    print("| Category | Correct | Total | Accuracy |")
    print("|----------|---------|-------|----------|")
    for cat, data in results["categories"].items():
        print(f"| {cat:16} | {data['correct']:7} | {data['total']:5} | {data['accuracy']:7.1%} |")
    
    print()
    print("=" * 60)


def save_results(results: dict, path: str):
    """Save results to markdown"""
    md = f"""# Temporal Dynamics Benchmark Results

*Generated: {results['timestamp']}*

## Summary

| Metric | Value |
|--------|-------|
| **System** | {results['system']} |
| **Total Cases** | {results['total_cases']} |
| **Overall Accuracy** | {results['overall_accuracy']:.1%} |

## Results by Category

| Category | Correct | Total | Accuracy |
|----------|---------|-------|----------|
"""
    for cat, data in results["categories"].items():
        md += f"| {cat} | {data['correct']} | {data['total']} | {data['accuracy']:.1%} |\n"
    
    md += """
## Category Descriptions

- **recency_override**: Newer information should override older (e.g., job changes)
- **frequency**: Frequently mentioned items should rank higher (e.g., favorite foods)
- **importance**: High-importance memories should persist despite age (e.g., allergies)
- **contradiction**: Direct contradictions where latest state wins (e.g., relationship status)

## Analysis

"""
    # Add analysis based on results
    best_cat = max(results["categories"].items(), key=lambda x: x[1]["accuracy"])
    worst_cat = min(results["categories"].items(), key=lambda x: x[1]["accuracy"])
    
    md += f"- **Best category**: {best_cat[0]} ({best_cat[1]['accuracy']:.1%})\n"
    md += f"- **Worst category**: {worst_cat[0]} ({worst_cat[1]['accuracy']:.1%})\n"
    
    Path(path).write_text(md)
    print(f"Results saved to {path}")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, help="Limit cases per category")
    parser.add_argument("--verbose", "-v", action="store_true")
    parser.add_argument("--output", "-o", default="benchmarks/TEMPORAL_RESULTS.md")
    args = parser.parse_args()
    
    results = evaluate_temporal_benchmark(
        limit=args.limit,
        verbose=args.verbose,
    )
    
    print_results(results)
    save_results(results, args.output)
