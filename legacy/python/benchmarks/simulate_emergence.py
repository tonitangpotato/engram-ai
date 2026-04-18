#!/usr/bin/env python3
"""
Emergence Simulation for NeuromemoryAI

Simulates 100 days of agent usage to demonstrate how Hebbian learning
creates emergent associative structure over time.

Key metrics:
- Link formation curve (how network grows)
- Cluster purity (do links form within topics?)
- Retrieval improvement (early vs late performance)
- Spreading activation effect (indirect recall)

Usage:
    python simulate_emergence.py
    python simulate_emergence.py --days 200 --visualize
"""

import argparse
import json
import os
import random
import sys
import tempfile
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from typing import List, Dict, Set, Tuple, Optional

# Add parent to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig
from engram.hebbian import get_all_hebbian_links, get_hebbian_neighbors


@dataclass
class SimulationMetrics:
    """Metrics collected at each checkpoint."""
    day: int
    total_memories: int
    total_links: int
    within_topic_links: int
    cross_topic_links: int
    cluster_purity: float  # within / total
    avg_cluster_size: float
    retrieval_precision_direct: float  # Direct keyword match
    retrieval_precision_spreading: float  # Via spreading activation
    
    
@dataclass
class EmergenceResult:
    """Final results from emergence simulation."""
    days_simulated: int
    final_memories: int
    final_links: int
    final_cluster_purity: float
    retrieval_improvement: float  # Late vs early precision
    metrics_timeline: List[SimulationMetrics] = field(default_factory=list)
    network_visualization: Optional[str] = None


class EmergenceSimulator:
    """Simulate long-term memory usage to demonstrate emergence."""
    
    # Topic clusters - memories within a topic should link together
    TOPICS = {
        "machine_learning": [
            "Neural networks learn patterns from data through backpropagation",
            "Gradient descent optimizes model parameters by following the loss gradient",
            "PyTorch provides automatic differentiation for deep learning",
            "Transformers use attention mechanisms for sequence modeling",
            "Overfitting occurs when models memorize training data",
            "Regularization techniques like dropout prevent overfitting",
            "Learning rate controls the step size during optimization",
            "Batch normalization stabilizes training of deep networks",
        ],
        "cooking": [
            "Italian pasta should be cooked al dente for best texture",
            "A good tomato sauce needs San Marzano tomatoes and basil",
            "Mise en place means preparing all ingredients before cooking",
            "Sautéing requires high heat and quick stirring",
            "French mother sauces form the basis of classical cuisine",
            "Emulsification creates creamy sauces like mayonnaise",
            "Caramelization brings out sweetness through heat",
            "Brining adds moisture and flavor to proteins",
        ],
        "travel": [
            "Japan's Shinkansen bullet trains are incredibly punctual",
            "Kyoto temples showcase traditional Japanese architecture",
            "European hostels offer budget-friendly accommodation",
            "Southeast Asia has tropical monsoon climate patterns",
            "Travel insurance covers medical emergencies abroad",
            "Jet lag affects circadian rhythm for several days",
            "Currency exchange rates vary between airports and banks",
            "Visa requirements depend on passport nationality",
        ],
        "programming": [
            "Python uses indentation for code block structure",
            "Rust prevents memory errors through ownership rules",
            "Git branches enable parallel feature development",
            "Docker containers package applications with dependencies",
            "REST APIs use HTTP methods for CRUD operations",
            "SQL joins combine data from multiple tables",
            "Async programming handles I/O without blocking",
            "Unit tests verify individual function behavior",
        ],
    }
    
    # Query patterns - simulate realistic user behavior
    QUERY_PATTERNS = [
        # Within-topic queries (should cause co-activation)
        ("machine_learning", "neural network training optimization"),
        ("machine_learning", "deep learning PyTorch gradient"),
        ("machine_learning", "overfitting regularization dropout"),
        ("cooking", "pasta Italian sauce tomato"),
        ("cooking", "cooking technique sauté heat"),
        ("cooking", "French sauce classical cuisine"),
        ("travel", "Japan train travel efficient"),
        ("travel", "budget travel Europe hostel"),
        ("travel", "international travel visa insurance"),
        ("programming", "Python code development"),
        ("programming", "Git Docker deployment"),
        ("programming", "API database SQL REST"),
    ]
    
    def __init__(self, db_path: str = None, days: int = 100):
        self.db_path = db_path or tempfile.mktemp(suffix=".db")
        self.days = days
        self.memory_to_topic: Dict[str, str] = {}
        self.metrics: List[SimulationMetrics] = []
        
        # Configure for emergence testing
        self.config = MemoryConfig.researcher()
        self.config.hebbian_enabled = True
        self.config.hebbian_threshold = 2  # Form links after 2 co-activations
        
    def run(self) -> EmergenceResult:
        """Run the full emergence simulation."""
        print("=" * 60)
        print("NeuromemoryAI Emergence Simulation")
        print(f"Simulating {self.days} days of agent usage")
        print("=" * 60)
        
        # Phase 1: Seed memories
        print("\n[Phase 1] Seeding initial memories...")
        self._seed_memories()
        
        # Phase 2: Simulate usage over time
        print(f"\n[Phase 2] Simulating {self.days} days of usage...")
        self._simulate_usage()
        
        # Phase 3: Analyze emergence
        print("\n[Phase 3] Analyzing emergent structure...")
        result = self._analyze_emergence()
        
        # Phase 4: Generate visualizations
        print("\n[Phase 4] Generating visualizations...")
        self._generate_visualizations(result)
        
        return result
    
    def _seed_memories(self):
        """Add initial memories from all topics."""
        mem = Memory(self.db_path, config=self.config)
        
        for topic, memories in self.TOPICS.items():
            for content in memories:
                mid = mem.add(content, type="factual", importance=0.7)
                self.memory_to_topic[mid] = topic
                
        print(f"  Seeded {len(self.memory_to_topic)} memories across {len(self.TOPICS)} topics")
        del mem
        
    def _simulate_usage(self):
        """Simulate daily usage patterns."""
        checkpoint_interval = max(1, self.days // 10)  # 10 checkpoints
        
        for day in range(1, self.days + 1):
            mem = Memory(self.db_path, config=self.config)
            
            # Simulate 5-10 queries per day
            num_queries = random.randint(5, 10)
            
            for _ in range(num_queries):
                # 80% chance: query within a single topic (realistic usage)
                # 20% chance: random/cross-topic query
                if random.random() < 0.8:
                    topic, query = random.choice(self.QUERY_PATTERNS)
                else:
                    query = random.choice([
                        "general information facts",
                        "how to learn something new",
                        "best practices recommendations",
                    ])
                
                # Perform recall (triggers Hebbian co-activation)
                mem.recall(query, limit=5)
            
            # Consolidate at end of day
            mem.consolidate(days=1)
            
            # Collect metrics at checkpoints
            if day % checkpoint_interval == 0 or day == self.days:
                metrics = self._collect_metrics(mem, day)
                self.metrics.append(metrics)
                print(f"  Day {day:3d}: {metrics.total_links} links, "
                      f"{metrics.cluster_purity:.1%} purity, "
                      f"spreading precision: {metrics.retrieval_precision_spreading:.1%}")
            
            del mem
            
    def _collect_metrics(self, mem: Memory, day: int) -> SimulationMetrics:
        """Collect metrics at a checkpoint."""
        links = get_all_hebbian_links(mem._store)
        
        # Count within-topic vs cross-topic links
        within_topic = 0
        cross_topic = 0
        
        for source_id, target_id, strength in links:
            source_topic = self.memory_to_topic.get(source_id)
            target_topic = self.memory_to_topic.get(target_id)
            
            if source_topic and target_topic:
                if source_topic == target_topic:
                    within_topic += 1
                else:
                    cross_topic += 1
                    
        total_links = len(links)
        cluster_purity = within_topic / total_links if total_links > 0 else 0
        
        # Measure cluster sizes
        clusters = self._find_clusters(links)
        avg_cluster_size = sum(len(c) for c in clusters) / len(clusters) if clusters else 0
        
        # Test retrieval precision
        direct_precision = self._test_direct_retrieval(mem)
        spreading_precision = self._test_spreading_retrieval(mem)
        
        return SimulationMetrics(
            day=day,
            total_memories=mem.stats()["total_memories"],
            total_links=total_links,
            within_topic_links=within_topic,
            cross_topic_links=cross_topic,
            cluster_purity=cluster_purity,
            avg_cluster_size=avg_cluster_size,
            retrieval_precision_direct=direct_precision,
            retrieval_precision_spreading=spreading_precision,
        )
        
    def _find_clusters(self, links: List[Tuple[str, str, float]]) -> List[Set[str]]:
        """Find connected components (clusters) in the link graph."""
        if not links:
            return []
            
        # Build adjacency list
        adj = defaultdict(set)
        for source, target, _ in links:
            adj[source].add(target)
            adj[target].add(source)
            
        # BFS to find connected components
        visited = set()
        clusters = []
        
        for node in adj:
            if node in visited:
                continue
            cluster = set()
            queue = [node]
            while queue:
                current = queue.pop(0)
                if current in visited:
                    continue
                visited.add(current)
                cluster.add(current)
                queue.extend(adj[current] - visited)
            clusters.append(cluster)
            
        return clusters
        
    def _test_direct_retrieval(self, mem: Memory) -> float:
        """Test retrieval using direct keyword queries."""
        correct = 0
        total = 0
        
        test_queries = [
            ("neural network backpropagation", "machine_learning"),
            ("pasta Italian tomato", "cooking"),
            ("Japan train bullet", "travel"),
            ("Python Git Docker", "programming"),
        ]
        
        for query, expected_topic in test_queries:
            results = mem.recall(query, limit=3)
            for r in results:
                mid = r.get("id")
                if mid and self.memory_to_topic.get(mid) == expected_topic:
                    correct += 1
                total += 1
                
        return correct / total if total > 0 else 0
        
    def _test_spreading_retrieval(self, mem: Memory) -> float:
        """Test retrieval that requires spreading activation.
        
        This measures whether Hebbian links help recall related memories
        that don't directly match the query keywords.
        
        We compare retrieval with graph_expand=True vs False to isolate
        the contribution of Hebbian spreading.
        """
        spreading_helped = 0
        total = 0
        
        # Test queries designed to match specific memories
        test_cases = [
            ("Neural networks learn", "machine_learning"),
            ("Gradient descent optimizes", "machine_learning"),
            ("Italian pasta cooked", "cooking"),
            ("tomato sauce needs", "cooking"),
            ("Japan Shinkansen bullet", "travel"),
            ("Python indentation code", "programming"),
        ]
        
        for query, expected_topic in test_cases:
            # Retrieve WITH Hebbian expansion
            results_with = mem.recall(query, limit=5, graph_expand=True)
            
            # Retrieve WITHOUT Hebbian expansion  
            results_without = mem.recall(query, limit=5, graph_expand=False)
            
            # Spreading helped if we got more results with expansion
            if len(results_with) > len(results_without):
                spreading_helped += 1
            total += 1
            
        return spreading_helped / total if total > 0 else 0
        
    def _analyze_emergence(self) -> EmergenceResult:
        """Analyze the final emergent structure."""
        if not self.metrics:
            raise RuntimeError("No metrics collected")
            
        early = self.metrics[0]
        late = self.metrics[-1]
        
        # Calculate improvement
        spreading_improvement = (
            (late.retrieval_precision_spreading - early.retrieval_precision_spreading)
            / max(early.retrieval_precision_spreading, 0.01)
        ) if early.retrieval_precision_spreading > 0 else late.retrieval_precision_spreading
        
        return EmergenceResult(
            days_simulated=self.days,
            final_memories=late.total_memories,
            final_links=late.total_links,
            final_cluster_purity=late.cluster_purity,
            retrieval_improvement=spreading_improvement,
            metrics_timeline=self.metrics,
        )
        
    def _generate_visualizations(self, result: EmergenceResult):
        """Generate ASCII visualizations of the results."""
        
        # 1. Link formation curve
        print("\n📈 Link Formation Over Time")
        print("-" * 50)
        max_links = max(m.total_links for m in self.metrics) or 1
        for m in self.metrics:
            bar_len = int(40 * m.total_links / max_links)
            bar = "█" * bar_len
            print(f"Day {m.day:3d}: {bar} {m.total_links}")
            
        # 2. Cluster purity over time
        print("\n📊 Cluster Purity Over Time (within-topic links / total)")
        print("-" * 50)
        for m in self.metrics:
            bar_len = int(40 * m.cluster_purity)
            bar = "█" * bar_len
            print(f"Day {m.day:3d}: {bar} {m.cluster_purity:.1%}")
            
        # 3. Retrieval improvement
        print("\n🎯 Retrieval Precision (Spreading Activation)")
        print("-" * 50)
        for m in self.metrics:
            bar_len = int(40 * m.retrieval_precision_spreading)
            bar = "█" * bar_len
            print(f"Day {m.day:3d}: {bar} {m.retrieval_precision_spreading:.1%}")
            
        # 4. Summary
        print("\n" + "=" * 60)
        print("EMERGENCE SUMMARY")
        print("=" * 60)
        early = self.metrics[0]
        late = self.metrics[-1]
        print(f"""
Simulation: {result.days_simulated} days

Network Growth:
  • Links formed: {early.total_links} → {late.total_links} ({late.total_links - early.total_links:+d})
  • Cluster purity: {early.cluster_purity:.1%} → {late.cluster_purity:.1%}
  
Retrieval Improvement:
  • Direct precision: {early.retrieval_precision_direct:.1%} → {late.retrieval_precision_direct:.1%}
  • Spreading precision: {early.retrieval_precision_spreading:.1%} → {late.retrieval_precision_spreading:.1%}
  • Improvement: {result.retrieval_improvement:+.1%}

Key Insight:
  The system learned {late.total_links} associative links through usage alone.
  {late.within_topic_links} of these ({late.cluster_purity:.1%}) correctly connect
  related memories within the same topic - demonstrating emergent structure
  from co-activation patterns, not explicit programming.
""")
        
    def save_results(self, output_dir: str = None):
        """Save results to JSON."""
        output_dir = output_dir or os.path.dirname(self.db_path)
        output_file = os.path.join(output_dir, "emergence_results.json")
        
        data = {
            "timestamp": datetime.now().isoformat(),
            "days_simulated": self.days,
            "metrics": [
                {
                    "day": m.day,
                    "total_memories": m.total_memories,
                    "total_links": m.total_links,
                    "within_topic_links": m.within_topic_links,
                    "cross_topic_links": m.cross_topic_links,
                    "cluster_purity": m.cluster_purity,
                    "retrieval_precision_spreading": m.retrieval_precision_spreading,
                }
                for m in self.metrics
            ]
        }
        
        with open(output_file, "w") as f:
            json.dump(data, f, indent=2)
        print(f"\nResults saved to: {output_file}")


def main():
    parser = argparse.ArgumentParser(description="NeuromemoryAI Emergence Simulation")
    parser.add_argument("--days", type=int, default=100, help="Days to simulate")
    parser.add_argument("--output", type=str, help="Output directory")
    parser.add_argument("--seed", type=int, default=42, help="Random seed for reproducibility")
    
    args = parser.parse_args()
    
    # Set random seed for reproducibility
    random.seed(args.seed)
    
    # Run simulation
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = os.path.join(tmpdir, "emergence.db")
        
        sim = EmergenceSimulator(db_path=db_path, days=args.days)
        result = sim.run()
        
        if args.output:
            sim.save_results(args.output)


if __name__ == "__main__":
    main()
