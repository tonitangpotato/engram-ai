#!/usr/bin/env python3
"""
NeuromemoryAI Benchmark Suite

Generates results for the paper by running controlled experiments
comparing cognitive memory dynamics against baselines.

Usage:
    python run_benchmark.py --all
    python run_benchmark.py --task multi-session
    python run_benchmark.py --task relevance-recency
    python run_benchmark.py --task forgetting
    python run_benchmark.py --task hebbian
"""

import argparse
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass, asdict
from datetime import datetime
from typing import List, Dict, Any, Optional

# Add parent to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig
from engram.hebbian import get_all_hebbian_links


@dataclass
class BenchmarkResult:
    """Result from a single benchmark run."""
    task: str
    metric: str
    value: float
    details: Optional[Dict[str, Any]] = None


class BenchmarkSuite:
    """Run all benchmarks for the NeuromemoryAI paper."""

    def __init__(self, output_dir: str = None):
        self.output_dir = output_dir or tempfile.mkdtemp(prefix="neuromem_bench_")
        self.results: List[BenchmarkResult] = []

    def run_all(self) -> List[BenchmarkResult]:
        """Run all benchmarks."""
        print("=" * 60)
        print("NeuromemoryAI Benchmark Suite")
        print("=" * 60)
        
        self.benchmark_multi_session()
        self.benchmark_relevance_vs_recency()
        self.benchmark_forgetting_benefits()
        self.benchmark_hebbian_emergence()
        self.benchmark_performance()
        
        self._save_results()
        self._print_summary()
        return self.results

    def benchmark_multi_session(self):
        """
        Benchmark 1: Multi-Session Continuity
        
        Tests whether the system can recall user preferences stated
        in early sessions after many intervening sessions.
        """
        print("\n[1/5] Multi-Session Continuity...")
        
        db_path = os.path.join(self.output_dir, "multi_session.db")
        config = MemoryConfig.chatbot()
        
        # Ground truth preferences to remember
        preferences = [
            ("User name is Alice Zhang", "Alice"),
            ("Alice works as a data scientist at Spotify", "Spotify"),
            ("Alice prefers tea over coffee", "tea"),
            ("Alice's favorite programming language is Rust", "Rust"),
            ("Alice has two cats named Luna and Nova", "Luna"),
        ]
        
        # Session 1: Add preferences
        mem = Memory(db_path, config=config)
        for content, _ in preferences:
            mem.add(content, type="relational", importance=0.8)
        del mem
        
        # Sessions 2-9: Intervening conversations (noise)
        noise_topics = [
            "Discussed weather forecast for the weekend",
            "Talked about a new movie that came out",
            "Helped with a Python debugging issue",
            "Recommended some restaurants nearby",
            "Discussed travel plans for summer",
            "Chatted about recent tech news",
            "Helped organize calendar events",
            "Discussed fitness goals",
        ]
        
        for i, topic in enumerate(noise_topics):
            mem = Memory(db_path, config=config)
            mem.add(topic, type="episodic", importance=0.3 + (i % 3) * 0.1)
            # Simulate time passage
            mem.consolidate(days=0.5 + i * 0.2)
            del mem
        
        # Session 10: Test recall
        mem = Memory(db_path, config=config)
        
        queries = [
            ("user name", "Alice"),
            ("works job company", "Spotify"),
            ("drink preference tea coffee", "tea"),
            ("programming language favorite", "Rust"),
            ("pets cats", "Luna"),
        ]
        
        recall_at_1 = 0
        recall_at_3 = 0
        mrr_sum = 0
        
        for query, expected in queries:
            results = mem.recall(query, limit=5)
            
            # Check ranks
            for rank, r in enumerate(results, 1):
                if expected.lower() in r["content"].lower():
                    mrr_sum += 1 / rank
                    if rank == 1:
                        recall_at_1 += 1
                    if rank <= 3:
                        recall_at_3 += 1
                    break
        
        n = len(queries)
        self.results.append(BenchmarkResult(
            task="multi_session",
            metric="recall@1",
            value=recall_at_1 / n,
            details={"correct": recall_at_1, "total": n}
        ))
        self.results.append(BenchmarkResult(
            task="multi_session",
            metric="recall@3",
            value=recall_at_3 / n,
            details={"correct": recall_at_3, "total": n}
        ))
        self.results.append(BenchmarkResult(
            task="multi_session",
            metric="mrr",
            value=mrr_sum / n,
        ))
        
        print(f"  Recall@1: {recall_at_1/n:.1%}")
        print(f"  Recall@3: {recall_at_3/n:.1%}")
        print(f"  MRR: {mrr_sum/n:.3f}")

    def benchmark_relevance_vs_recency(self):
        """
        Benchmark 2: Relevance vs Recency
        
        Tests whether the system properly balances relevance
        (semantic match) against recency (time since access).
        """
        print("\n[2/5] Relevance vs Recency...")
        
        db_path = os.path.join(self.output_dir, "relevance_recency.db")
        config = MemoryConfig.personal_assistant()
        mem = Memory(db_path, config=config)
        
        # Old but highly relevant memory
        old_relevant = mem.add(
            "Project Phoenix deployment procedure: 1) Run test suite 2) Build container 3) Push to ECR 4) Update ECS service",
            type="procedural",
            importance=0.9
        )
        
        # Simulate it being 30 days old
        mem.consolidate(days=30)
        
        # Recent but tangentially related memory
        recent_tangent = mem.add(
            "Had a meeting about general CI/CD practices",
            type="episodic",
            importance=0.4
        )
        
        # Query that should match the old deployment procedure
        results = mem.recall("deploy Project Phoenix ECS", limit=3)
        
        # Check which ranks higher
        old_rank = None
        recent_rank = None
        for i, r in enumerate(results, 1):
            if "Phoenix" in r["content"]:
                old_rank = i
            if "CI/CD" in r["content"]:
                recent_rank = i
        
        # Success: old relevant memory ranks higher than recent tangent
        relevance_wins = old_rank is not None and (recent_rank is None or old_rank < recent_rank)
        
        self.results.append(BenchmarkResult(
            task="relevance_recency",
            metric="relevance_prioritized",
            value=1.0 if relevance_wins else 0.0,
            details={"old_rank": old_rank, "recent_rank": recent_rank}
        ))
        
        print(f"  Old relevant memory rank: {old_rank}")
        print(f"  Recent tangent rank: {recent_rank}")
        print(f"  Relevance prioritized: {'✓' if relevance_wins else '✗'}")

    def benchmark_forgetting_benefits(self):
        """
        Benchmark 3: Forgetting Benefits
        
        Compares retrieval quality with and without forgetting.
        Forgetting should improve signal-to-noise ratio.
        """
        print("\n[3/5] Forgetting Benefits...")
        
        # Relevant memories (signal)
        signal_memories = [
            "Machine learning model training requires GPUs",
            "Neural networks use backpropagation for learning",
            "Deep learning frameworks include PyTorch and TensorFlow",
            "Gradient descent optimizes model parameters",
            "Overfitting can be prevented with regularization",
        ]
        
        # Noise memories
        noise_memories = [
            f"Random thought #{i}: Lorem ipsum dolor sit amet" for i in range(20)
        ]
        
        results_with_forgetting = self._run_forgetting_test(
            signal_memories, noise_memories, forgetting=True
        )
        results_without_forgetting = self._run_forgetting_test(
            signal_memories, noise_memories, forgetting=False
        )
        
        self.results.append(BenchmarkResult(
            task="forgetting",
            metric="precision@5_with_forgetting",
            value=results_with_forgetting["precision"],
        ))
        self.results.append(BenchmarkResult(
            task="forgetting",
            metric="precision@5_without_forgetting",
            value=results_without_forgetting["precision"],
        ))
        self.results.append(BenchmarkResult(
            task="forgetting",
            metric="storage_reduction",
            value=1 - (results_with_forgetting["memory_count"] / results_without_forgetting["memory_count"]),
            details={
                "with": results_with_forgetting["memory_count"],
                "without": results_without_forgetting["memory_count"]
            }
        ))
        
        print(f"  Precision@5 with forgetting: {results_with_forgetting['precision']:.1%}")
        print(f"  Precision@5 without forgetting: {results_without_forgetting['precision']:.1%}")
        print(f"  Storage reduction: {1 - (results_with_forgetting['memory_count'] / results_without_forgetting['memory_count']):.1%}")

    def _run_forgetting_test(self, signal: List[str], noise: List[str], forgetting: bool) -> Dict:
        """Run forgetting test with or without forgetting enabled."""
        db_path = os.path.join(self.output_dir, f"forgetting_{'on' if forgetting else 'off'}.db")
        config = MemoryConfig.chatbot()
        mem = Memory(db_path, config=config)
        
        # Add signal (important)
        for s in signal:
            mem.add(s, type="factual", importance=0.8)
        
        # Add noise (unimportant)
        for n in noise:
            mem.add(n, type="episodic", importance=0.1)
        
        # Simulate 30 days with consolidation
        for day in range(30):
            mem.consolidate(days=1)
            if forgetting and day % 7 == 0:
                mem.forget(threshold=0.01)  # Weekly forgetting
        
        # Query for ML topics
        results = mem.recall("machine learning neural network", limit=5)
        
        # Count how many of top-5 are signal
        signal_in_top5 = sum(
            1 for r in results
            if any(s_word in r["content"].lower() for s_word in ["machine", "neural", "deep", "gradient", "overfitting"])
        )
        
        return {
            "precision": signal_in_top5 / 5,
            "memory_count": mem.stats()["total_memories"],
        }

    def benchmark_hebbian_emergence(self):
        """
        Benchmark 4: Hebbian Emergence
        
        Tests whether meaningful associations form automatically
        through co-activation, without manual entity linking.
        """
        print("\n[4/5] Hebbian Emergence...")
        
        db_path = os.path.join(self.output_dir, "hebbian.db")
        config = MemoryConfig.researcher()
        config.hebbian_enabled = True
        config.hebbian_threshold = 2  # Lower threshold for demo
        mem = Memory(db_path, config=config)
        
        # Add memories from 3 distinct topics
        ml_memories = [
            "Machine learning uses statistical methods",
            "Neural networks have multiple layers",
            "Gradient descent minimizes loss functions",
        ]
        cooking_memories = [
            "Italian pasta requires al dente cooking",
            "Sautéing uses high heat and quick movement",
            "French sauces are based on mother sauces",
        ]
        travel_memories = [
            "Japan has efficient rail systems",
            "European hostels are budget friendly",
            "Southeast Asia has tropical climate",
        ]
        
        ml_ids = [mem.add(m, type="factual", importance=0.7) for m in ml_memories]
        cooking_ids = [mem.add(m, type="factual", importance=0.7) for m in cooking_memories]
        travel_ids = [mem.add(m, type="factual", importance=0.7) for m in travel_memories]
        
        # Simulate usage patterns - co-recall within topics
        for _ in range(5):
            mem.recall("machine learning neural network optimization", limit=3)
        for _ in range(5):
            mem.recall("cooking pasta sauce technique", limit=3)
        for _ in range(5):
            mem.recall("travel Japan Europe Asia budget", limit=3)
        
        # Analyze Hebbian links
        links = get_all_hebbian_links(mem._store)  # Returns [(source_id, target_id, strength), ...]
        
        def is_within_topic(id1, id2):
            """Check if both IDs are from the same topic."""
            for topic_ids in [ml_ids, cooking_ids, travel_ids]:
                if id1 in topic_ids and id2 in topic_ids:
                    return True
            return False
        
        total_links = len(links)
        within_topic_links = sum(1 for (id1, id2, _) in links if is_within_topic(id1, id2))
        cross_topic_links = total_links - within_topic_links
        
        precision = within_topic_links / total_links if total_links > 0 else 0
        
        self.results.append(BenchmarkResult(
            task="hebbian",
            metric="total_links_formed",
            value=total_links,
        ))
        self.results.append(BenchmarkResult(
            task="hebbian",
            metric="within_topic_precision",
            value=precision,
            details={"within": within_topic_links, "cross": cross_topic_links}
        ))
        
        print(f"  Total links formed: {total_links}")
        print(f"  Within-topic links: {within_topic_links}")
        print(f"  Cross-topic (noise): {cross_topic_links}")
        print(f"  Precision: {precision:.1%}")

    def benchmark_performance(self):
        """
        Benchmark 5: Performance
        
        Measures latency for common operations.
        """
        print("\n[5/5] Performance...")
        
        db_path = os.path.join(self.output_dir, "performance.db")
        mem = Memory(db_path)
        
        # Add 100 memories
        for i in range(100):
            mem.add(f"Memory content number {i} with some additional text for bulk", 
                    type="factual", importance=0.5)
        
        # Benchmark recall latency
        latencies = []
        for _ in range(50):
            start = time.perf_counter()
            mem.recall("memory content number", limit=10)
            latencies.append((time.perf_counter() - start) * 1000)  # ms
        
        avg_latency = sum(latencies) / len(latencies)
        p99_latency = sorted(latencies)[int(len(latencies) * 0.99)]
        
        # Benchmark consolidation time
        start = time.perf_counter()
        mem.consolidate(days=7)
        consolidate_time = (time.perf_counter() - start) * 1000
        
        self.results.append(BenchmarkResult(
            task="performance",
            metric="recall_latency_avg_ms",
            value=avg_latency,
        ))
        self.results.append(BenchmarkResult(
            task="performance",
            metric="recall_latency_p99_ms",
            value=p99_latency,
        ))
        self.results.append(BenchmarkResult(
            task="performance",
            metric="consolidate_100_memories_ms",
            value=consolidate_time,
        ))
        
        print(f"  Recall latency (avg): {avg_latency:.2f}ms")
        print(f"  Recall latency (p99): {p99_latency:.2f}ms")
        print(f"  Consolidation time: {consolidate_time:.2f}ms")

    def _save_results(self):
        """Save results to JSON file."""
        output_file = os.path.join(self.output_dir, "benchmark_results.json")
        with open(output_file, "w") as f:
            json.dump({
                "timestamp": datetime.now().isoformat(),
                "results": [asdict(r) for r in self.results]
            }, f, indent=2)
        print(f"\nResults saved to: {output_file}")

    def _print_summary(self):
        """Print summary table for paper."""
        print("\n" + "=" * 60)
        print("SUMMARY FOR PAPER")
        print("=" * 60)
        print("""
| Metric | Value |
|--------|-------|""")
        for r in self.results:
            if isinstance(r.value, float):
                if "ms" in r.metric or "latency" in r.metric:
                    print(f"| {r.task}/{r.metric} | {r.value:.2f}ms |")
                elif r.value <= 1 and ("precision" in r.metric or "recall" in r.metric or "mrr" in r.metric):
                    print(f"| {r.task}/{r.metric} | {r.value:.1%} |")
                elif "reduction" in r.metric:
                    print(f"| {r.task}/{r.metric} | {r.value:.1%} |")
                else:
                    print(f"| {r.task}/{r.metric} | {r.value:.2f} |")
            else:
                print(f"| {r.task}/{r.metric} | {r.value} |")


def main():
    parser = argparse.ArgumentParser(description="NeuromemoryAI Benchmark Suite")
    parser.add_argument("--all", action="store_true", help="Run all benchmarks")
    parser.add_argument("--task", choices=["multi-session", "relevance-recency", "forgetting", "hebbian", "performance"],
                        help="Run specific benchmark")
    parser.add_argument("--output", type=str, help="Output directory for results")
    
    args = parser.parse_args()
    
    suite = BenchmarkSuite(output_dir=args.output)
    
    if args.all or not args.task:
        suite.run_all()
    else:
        if args.task == "multi-session":
            suite.benchmark_multi_session()
        elif args.task == "relevance-recency":
            suite.benchmark_relevance_vs_recency()
        elif args.task == "forgetting":
            suite.benchmark_forgetting_benefits()
        elif args.task == "hebbian":
            suite.benchmark_hebbian_emergence()
        elif args.task == "performance":
            suite.benchmark_performance()
        
        suite._save_results()
        suite._print_summary()


if __name__ == "__main__":
    main()
