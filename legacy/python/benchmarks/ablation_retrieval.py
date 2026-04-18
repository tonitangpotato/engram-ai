#!/usr/bin/env python3
"""
Ablation Study with Retrieval Metrics

Tests memory system retrieval quality using MRR, Hit@K
instead of F1 (which requires LLM answer generation).

Evidence-based evaluation: checks if retrieved memories
contain the ground-truth evidence dialogue IDs.
"""

import json
import sys
import time
import argparse
from pathlib import Path
from dataclasses import dataclass
from typing import Dict, List, Optional
from collections import defaultdict

sys.path.insert(0, str(Path(__file__).parent.parent))

from engram.store import SQLiteStore
from engram.core import MemoryEntry, MemoryType


def keyword_similarity(query: str, content: str) -> float:
    """Simple Jaccard-like similarity based on word overlap."""
    query_words = set(query.lower().split())
    content_words = set(content.lower().split())
    
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 
                  'is', 'was', 'are', 'were', 'be', 'been', 'said', 'that', 'this',
                  'i', 'you', 'he', 'she', 'it', 'we', 'they', 'my', 'your', 'his',
                  'her', 'its', 'our', 'their', 'have', 'has', 'had', 'do', 'does',
                  'did', 'will', 'would', 'could', 'should', 'may', 'might', 'must',
                  'what', 'when', 'where', 'who', 'how', 'why', 'which'}
    query_words -= stop_words
    content_words -= stop_words
    
    if not query_words or not content_words:
        return 0.0
    
    intersection = query_words & content_words
    union = query_words | content_words
    
    return len(intersection) / len(union) if union else 0.0


@dataclass
class AblationConfig:
    """Configuration for ablation variants."""
    name: str
    use_actr: bool = True
    use_importance: bool = True
    use_hebbian: bool = True
    use_decay: bool = True
    use_recency: bool = True


class AblationMemory:
    """Memory system with configurable features for ablation testing."""
    
    def __init__(self, config: AblationConfig):
        self.config = config
        self._memories: List[dict] = []
        self._access_counts: Dict[str, int] = {}
        self._coactivations: Dict[tuple, int] = {}
        self._id_counter = 0
        
    def add(self, content: str, dia_id: str, importance: float = 0.5) -> str:
        """Add a memory with dialogue ID for evidence tracking."""
        self._id_counter += 1
        mem_id = f"mem_{self._id_counter}"
        
        self._memories.append({
            "id": mem_id,
            "content": content,
            "dia_id": dia_id,  # Track the original dialogue ID
            "importance": importance if self.config.use_importance else 0.5,
            "created_at": time.time() + self._id_counter * 0.001,  # Slight offset for ordering
            "access_count": 1,
        })
        self._access_counts[mem_id] = 1
        return mem_id
    
    def recall(self, query: str, limit: int = 10) -> List[dict]:
        """Recall memories based on configured scoring method."""
        now = time.time() + 1000  # Future reference point
        scored = []
        
        for mem in self._memories:
            score = self._compute_score(query, mem, now)
            scored.append((mem, score))
        
        scored.sort(key=lambda x: x[1], reverse=True)
        
        # Hebbian co-activation tracking
        if self.config.use_hebbian and len(scored) >= 2:
            top_ids = [s[0]["id"] for s in scored[:limit]]
            for i in range(len(top_ids)):
                for j in range(i + 1, len(top_ids)):
                    pair = tuple(sorted([top_ids[i], top_ids[j]]))
                    self._coactivations[pair] = self._coactivations.get(pair, 0) + 1
        
        # Update access counts
        for mem, _ in scored[:limit]:
            self._access_counts[mem["id"]] = self._access_counts.get(mem["id"], 0) + 1
        
        return [{"dia_id": m["dia_id"], "content": m["content"], "score": s} 
                for m, s in scored[:limit]]
    
    def _compute_score(self, query: str, mem: dict, now: float) -> float:
        """Compute retrieval score based on enabled features."""
        import math
        score = 0.0
        
        # Base relevance (keyword similarity) - primary signal
        relevance = keyword_similarity(query, mem["content"])
        score += relevance * 10.0
        
        # Memory index (older memories have higher index = were added earlier)
        mem_index = int(mem["id"].split("_")[1])
        total_memories = len(self._memories)
        
        if self.config.use_actr:
            # ACT-R: boost frequently accessed memories
            # Use normalized recency + access count boost
            access_count = self._access_counts.get(mem["id"], 1)
            
            # Recency: newer memories get slight boost (0 to 1)
            recency = mem_index / max(total_memories, 1)
            
            # Access boost: log scale, normalized (0 to ~2)
            access_boost = math.log(1 + access_count) 
            
            # Combined ACT-R score (small positive contribution)
            actr_score = (recency * 0.5) + (access_boost * 0.3)
            score += actr_score
            
        elif self.config.use_recency:
            # Simple recency: newer = higher (0 to 1)
            recency = mem_index / max(total_memories, 1)
            score += recency * 0.5
        
        if self.config.use_importance:
            # Importance: 0-1 range, small boost
            score += mem["importance"] * 1.0
        
        if self.config.use_decay:
            # Decay: older memories get penalized slightly
            age_ratio = 1 - (mem_index / max(total_memories, 1))
            score -= age_ratio * 0.3
        
        if self.config.use_hebbian:
            # Hebbian: co-activated memories get boosted
            for (id1, id2), count in self._coactivations.items():
                if mem["id"] in (id1, id2) and count >= 2:
                    score += 0.1 * min(count, 5)
        
        return score


# Ablation configurations
ABLATION_CONFIGS = [
    AblationConfig(name="full", use_actr=True, use_importance=True, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-actr", use_actr=False, use_importance=True, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-importance", use_actr=True, use_importance=False, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-hebbian", use_actr=True, use_importance=True, use_hebbian=False, use_decay=True),
    AblationConfig(name="no-decay", use_actr=True, use_importance=True, use_hebbian=True, use_decay=False),
    AblationConfig(name="simple-recency", use_actr=False, use_importance=False, use_hebbian=False, use_decay=False, use_recency=True),
    AblationConfig(name="keyword-only", use_actr=False, use_importance=False, use_hebbian=False, use_decay=False, use_recency=False),
]


def calculate_retrieval_metrics(recalled: List[dict], evidence_ids: List[str], k_values=[1, 3, 5, 10]) -> dict:
    """
    Calculate retrieval metrics.
    
    Args:
        recalled: List of recalled memories with dia_id
        evidence_ids: Ground truth evidence dialogue IDs
        k_values: K values for Hit@K
    
    Returns:
        Dict with MRR, Hit@K metrics
    """
    evidence_set = set(evidence_ids)
    
    # Find first relevant result position (for MRR)
    first_relevant_rank = None
    for rank, mem in enumerate(recalled, 1):
        if mem["dia_id"] in evidence_set:
            first_relevant_rank = rank
            break
    
    mrr = 1.0 / first_relevant_rank if first_relevant_rank else 0.0
    
    # Hit@K
    hits = {}
    for k in k_values:
        top_k_ids = {m["dia_id"] for m in recalled[:k]}
        hits[f"hit@{k}"] = 1 if evidence_set & top_k_ids else 0
    
    # Recall@K (what fraction of evidence was retrieved)
    recall = {}
    for k in k_values:
        top_k_ids = {m["dia_id"] for m in recalled[:k]}
        found = len(evidence_set & top_k_ids)
        recall[f"recall@{k}"] = found / len(evidence_set) if evidence_set else 0
    
    return {"mrr": mrr, **hits, **recall}


def run_ablation(config: AblationConfig, data: List[dict], verbose: bool = False) -> dict:
    """Run evaluation with a specific ablation config."""
    metrics = defaultdict(list)
    category_metrics = defaultdict(lambda: defaultdict(list))
    
    for sample in data:
        conversation = sample.get("conversation", {})
        qa_pairs = sample.get("qa", [])
        
        # Create fresh memory
        mem = AblationMemory(config)
        
        # Load conversation
        session_num = 1
        while f"session_{session_num}" in conversation:
            for turn in conversation[f"session_{session_num}"]:
                content = f"{turn['speaker']}: {turn['text']}"
                dia_id = turn.get("dia_id", f"unknown_{session_num}")
                mem.add(content=content, dia_id=dia_id)
            session_num += 1
        
        # Evaluate each question
        for qa in qa_pairs:
            question = qa.get("question", "")
            evidence = qa.get("evidence", [])
            category = str(qa.get("category", "unknown"))
            
            if not question or not evidence:
                continue
            
            recalled = mem.recall(question, limit=10)
            result = calculate_retrieval_metrics(recalled, evidence)
            
            for k, v in result.items():
                metrics[k].append(v)
                category_metrics[category][k].append(v)
            
            if verbose and result["hit@5"] == 0:
                print(f"  MISS [{category}]: {question[:60]}...")
                print(f"    Evidence: {evidence}")
                print(f"    Top 3: {[r['dia_id'] for r in recalled[:3]]}")
    
    # Aggregate
    results = {"config": config.name}
    for k, values in metrics.items():
        results[k] = sum(values) / len(values) if values else 0
    results["total_questions"] = len(metrics["mrr"])
    
    # Category breakdown
    results["by_category"] = {}
    for cat, cat_metrics in category_metrics.items():
        results["by_category"][cat] = {
            k: sum(v) / len(v) if v else 0 
            for k, v in cat_metrics.items()
        }
        results["by_category"][cat]["count"] = len(cat_metrics["mrr"])
    
    return results


def main():
    parser = argparse.ArgumentParser(description="Ablation study with retrieval metrics")
    parser.add_argument("--limit", type=int, default=None, help="Limit conversations")
    parser.add_argument("--verbose", "-v", action="store_true", help="Show misses")
    args = parser.parse_args()
    
    # Load data
    data_path = Path(__file__).parent / "locomo" / "data" / "locomo10.json"
    with open(data_path) as f:
        data = json.load(f)
    
    if args.limit:
        data = data[:args.limit]
    
    print(f"Loaded {len(data)} conversations")
    total_qs = sum(len(s.get("qa", [])) for s in data)
    print(f"Total questions: {total_qs}")
    
    print("\n" + "=" * 80)
    print("ABLATION STUDY - RETRIEVAL METRICS")
    print("=" * 80)
    
    all_results = []
    
    for config in ABLATION_CONFIGS:
        print(f"\nTesting: {config.name}...")
        result = run_ablation(config, data, args.verbose)
        all_results.append(result)
        print(f"  MRR: {result['mrr']:.3f} | Hit@5: {result['hit@5']:.3f} | Recall@5: {result['recall@5']:.3f}")
    
    # Summary table
    print("\n" + "=" * 80)
    print("COMPARISON TABLE")
    print("=" * 80)
    print(f"{'Config':<18} {'MRR':<8} {'Hit@1':<8} {'Hit@5':<8} {'R@5':<8} {'Questions':<10}")
    print("-" * 70)
    
    for r in sorted(all_results, key=lambda x: x['mrr'], reverse=True):
        print(f"{r['config']:<18} {r['mrr']:<8.3f} {r['hit@1']:<8.3f} {r['hit@5']:<8.3f} {r['recall@5']:<8.3f} {r['total_questions']:<10}")
    
    # Best/worst
    best = max(all_results, key=lambda x: x['mrr'])
    worst = min(all_results, key=lambda x: x['mrr'])
    
    print(f"\nBest: {best['config']} (MRR={best['mrr']:.3f})")
    print(f"Worst: {worst['config']} (MRR={worst['mrr']:.3f})")
    print(f"Improvement: {(best['mrr'] - worst['mrr']) / max(worst['mrr'], 0.001) * 100:.1f}%")
    
    # Category breakdown for best config
    print(f"\n--- Category breakdown for '{best['config']}' ---")
    for cat, cat_data in sorted(best["by_category"].items()):
        print(f"  Cat {cat}: MRR={cat_data['mrr']:.3f}, Hit@5={cat_data['hit@5']:.3f}, n={cat_data['count']}")


if __name__ == "__main__":
    main()
