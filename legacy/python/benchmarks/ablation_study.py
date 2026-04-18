#!/usr/bin/env python3
"""
Ablation Study: Testing the value of each cognitive science module

Tests:
1. engram-full: All features enabled
2. engram-no-actr: Disable ACT-R activation formula (use simple recency)
3. engram-no-importance: Disable importance weighting
4. engram-no-hebbian: Disable Hebbian learning
5. engram-no-decay: Disable forgetting/decay
6. engram-simple: Only embedding similarity (baseline)
"""

import json
import sys
import time
from pathlib import Path
from datetime import datetime
from dataclasses import dataclass
from typing import Dict, List, Optional
import re

sys.path.insert(0, str(Path(__file__).parent.parent))

from engram.store import SQLiteStore
from engram.core import MemoryEntry, MemoryType
from engram.config import MemoryConfig


# Simple embedding-free similarity using keyword overlap
def keyword_similarity(query: str, content: str) -> float:
    """Simple Jaccard-like similarity based on word overlap."""
    query_words = set(query.lower().split())
    content_words = set(content.lower().split())
    
    # Remove stop words
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 
                  'is', 'was', 'are', 'were', 'be', 'been', 'said', 'that', 'this'}
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
    use_actr: bool = True           # Use ACT-R activation formula
    use_importance: bool = True      # Use importance weighting
    use_hebbian: bool = True         # Use Hebbian learning
    use_decay: bool = True           # Use forgetting/decay
    use_recency: bool = True         # Use recency in scoring


class AblationMemory:
    """
    Memory system with configurable features for ablation testing.
    """
    
    def __init__(self, config: AblationConfig):
        self.config = config
        self._store = SQLiteStore(":memory:")
        self._memories: List[dict] = []
        self._access_counts: Dict[str, int] = {}
        self._coactivations: Dict[tuple, int] = {}
        
    def add(self, content: str, importance: float = 0.5, **kwargs) -> str:
        """Add a memory."""
        entry = self._store.add(
            content=content,
            memory_type=MemoryType.EPISODIC,
            importance=importance if self.config.use_importance else 0.5,
        )
        self._memories.append({
            "id": entry.id,
            "content": content,
            "importance": importance if self.config.use_importance else 0.5,
            "created_at": entry.created_at,
            "access_count": 1,
        })
        self._access_counts[entry.id] = 1
        return entry.id
    
    def recall(self, query: str, limit: int = 5, **kwargs) -> List[dict]:
        """Recall memories based on configured scoring method."""
        now = time.time()
        scored = []
        
        for mem in self._memories:
            score = self._compute_score(query, mem, now)
            scored.append((mem, score))
        
        # Sort by score descending
        scored.sort(key=lambda x: x[1], reverse=True)
        
        # Record co-activation for Hebbian learning
        if self.config.use_hebbian and len(scored) >= 2:
            top_ids = [s[0]["id"] for s in scored[:limit]]
            for i in range(len(top_ids)):
                for j in range(i + 1, len(top_ids)):
                    pair = tuple(sorted([top_ids[i], top_ids[j]]))
                    self._coactivations[pair] = self._coactivations.get(pair, 0) + 1
        
        # Update access counts
        for mem, _ in scored[:limit]:
            self._access_counts[mem["id"]] = self._access_counts.get(mem["id"], 0) + 1
        
        results = []
        for mem, score in scored[:limit]:
            results.append({
                "id": mem["id"],
                "content": mem["content"],
                "confidence": min(1.0, max(0.0, (score + 10) / 20)),  # Normalize to 0-1
                "confidence_label": "certain" if score > 0 else "uncertain",
                "activation": score,
                "importance": mem["importance"],
            })
        
        return results
    
    def _compute_score(self, query: str, mem: dict, now: float) -> float:
        """Compute retrieval score based on enabled features."""
        score = 0.0
        
        # Base relevance (always on - keyword similarity)
        relevance = keyword_similarity(query, mem["content"])
        score += relevance * 5.0
        
        if self.config.use_actr:
            # ACT-R style activation: log of summed recency-weighted accesses
            import math
            age_seconds = max(1, now - mem["created_at"])
            access_count = self._access_counts.get(mem["id"], 1)
            
            # Base-level activation approximation
            # B = ln(n / t^d) where n = access count, t = age, d = decay
            decay = 0.5
            base_activation = math.log(access_count / (age_seconds ** decay) + 0.001)
            score += base_activation
        elif self.config.use_recency:
            # Simple recency: newer = higher score
            age_hours = (now - mem["created_at"]) / 3600
            recency_score = max(0, 10 - age_hours * 0.1)  # Decays over ~100 hours
            score += recency_score * 0.5
        
        if self.config.use_importance:
            # Importance boost
            score += mem["importance"] * 2.0
        
        if self.config.use_decay:
            # Forgetting penalty for old memories
            age_days = (now - mem["created_at"]) / 86400
            decay_penalty = age_days * 0.1
            score -= decay_penalty
        
        if self.config.use_hebbian:
            # Hebbian boost: memories often recalled together get boosted
            for (id1, id2), count in self._coactivations.items():
                if mem["id"] in (id1, id2) and count >= 2:
                    score += 0.3 * min(count, 5)
        
        return score


# Define ablation variants
ABLATION_CONFIGS = [
    AblationConfig(name="full", use_actr=True, use_importance=True, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-actr", use_actr=False, use_importance=True, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-importance", use_actr=True, use_importance=False, use_hebbian=True, use_decay=True),
    AblationConfig(name="no-hebbian", use_actr=True, use_importance=True, use_hebbian=False, use_decay=True),
    AblationConfig(name="no-decay", use_actr=True, use_importance=True, use_hebbian=True, use_decay=False),
    AblationConfig(name="simple", use_actr=False, use_importance=False, use_hebbian=False, use_decay=False, use_recency=True),
    AblationConfig(name="baseline", use_actr=False, use_importance=False, use_hebbian=False, use_decay=False, use_recency=False),
]


def load_locomo_data(data_path: str) -> List[dict]:
    """Load LoCoMo benchmark data."""
    with open(data_path) as f:
        data = json.load(f)
    return data if isinstance(data, list) else [data]


def normalize_answer(s: str) -> str:
    """Normalize answer for comparison."""
    import string
    s = s.replace(',', "")
    s = re.sub(r'\b(a|an|the|and)\b', ' ', s, flags=re.IGNORECASE)
    s = ' '.join(s.split())
    s = ''.join(ch for ch in s if ch not in set(string.punctuation))
    return s.lower()


def f1_score(prediction: str, ground_truth: str) -> float:
    """Calculate token-level F1 score."""
    from collections import Counter
    
    pred_tokens = normalize_answer(prediction).split()
    truth_tokens = normalize_answer(ground_truth).split()
    
    if not pred_tokens or not truth_tokens:
        return 0.0
    
    common = Counter(pred_tokens) & Counter(truth_tokens)
    num_same = sum(common.values())
    
    if num_same == 0:
        return 0.0
    
    precision = num_same / len(pred_tokens)
    recall = num_same / len(truth_tokens)
    return (2 * precision * recall) / (precision + recall)


def run_ablation_test(config: AblationConfig, conversations: List[dict], limit: int = None) -> dict:
    """Run evaluation with a specific ablation config."""
    results = {
        "config": config.name,
        "total_questions": 0,
        "total_f1": 0.0,
        "category_scores": {},
    }
    
    for conv_idx, sample in enumerate(conversations):
        if limit and conv_idx >= limit:
            break
        
        # Get conversation dict (LoCoMo format)
        conversation = sample.get("conversation", sample)
        qa_pairs = sample.get("qa", [])
        
        # Create fresh memory for this conversation
        mem = AblationMemory(config)
        
        # Load conversation into memory
        session_num = 1
        while f"session_{session_num}" in conversation:
            session_key = f"session_{session_num}"
            for turn in conversation[session_key]:
                content = f"{turn['speaker']} said: {turn['text']}"
                mem.add(content=content, importance=0.5)
            session_num += 1
        
        # Answer questions
        for qa in qa_pairs:
            question = qa.get("question", "")
            ground_truth = str(qa.get("answer", ""))
            category = str(qa.get("category", "unknown"))
            
            if category not in results["category_scores"]:
                results["category_scores"][category] = {"count": 0, "f1_sum": 0.0}
            
            # Get answer from memory
            recalled = mem.recall(question, limit=5)
            
            if recalled:
                # Use top memory as answer
                prediction = recalled[0]["content"]
            else:
                prediction = "Unknown"
            
            # Calculate F1
            f1 = f1_score(prediction, ground_truth)
            
            results["total_questions"] += 1
            results["total_f1"] += f1
            results["category_scores"][category]["count"] += 1
            results["category_scores"][category]["f1_sum"] += f1
    
    # Calculate averages
    if results["total_questions"] > 0:
        results["avg_f1"] = results["total_f1"] / results["total_questions"]
    else:
        results["avg_f1"] = 0.0
    
    for cat in results["category_scores"]:
        cat_data = results["category_scores"][cat]
        if cat_data["count"] > 0:
            cat_data["avg_f1"] = cat_data["f1_sum"] / cat_data["count"]
        else:
            cat_data["avg_f1"] = 0.0
    
    return results


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Ablation study for memory system")
    parser.add_argument("--data", default="benchmarks/locomo/data/locomo10.json", help="LoCoMo data file")
    parser.add_argument("--limit", type=int, default=5, help="Limit conversations to test")
    parser.add_argument("--verbose", "-v", action="store_true")
    args = parser.parse_args()
    
    # Check if data exists
    if not Path(args.data).exists():
        print(f"Data file not found: {args.data}")
        print("Falling back to TDB benchmark...")
        run_tdb_ablation()
        return
    
    # Load data
    print(f"Loading LoCoMo data from {args.data}...")
    conversations = load_locomo_data(args.data)
    print(f"Loaded {len(conversations)} conversations")
    
    # Run ablation tests
    print("\n" + "=" * 70)
    print("ABLATION STUDY RESULTS")
    print("=" * 70)
    
    all_results = []
    for config in ABLATION_CONFIGS:
        print(f"\nTesting: {config.name}...")
        result = run_ablation_test(config, conversations, limit=args.limit)
        all_results.append(result)
        print(f"  Average F1: {result['avg_f1']:.3f}")
    
    # Print comparison table
    print("\n" + "=" * 70)
    print("COMPARISON TABLE")
    print("=" * 70)
    print(f"{'Config':<20} {'Avg F1':<10} {'Questions':<10}")
    print("-" * 40)
    
    for r in sorted(all_results, key=lambda x: x['avg_f1'], reverse=True):
        print(f"{r['config']:<20} {r['avg_f1']:<10.3f} {r['total_questions']:<10}")
    
    # Print category breakdown for best and worst
    best = max(all_results, key=lambda x: x['avg_f1'])
    worst = min(all_results, key=lambda x: x['avg_f1'])
    
    print(f"\nBest config: {best['config']} (F1={best['avg_f1']:.3f})")
    print(f"Worst config: {worst['config']} (F1={worst['avg_f1']:.3f})")
    print(f"Improvement from simplest to best: {(best['avg_f1'] - worst['avg_f1']) * 100:.1f}%")


def run_tdb_ablation():
    """Run ablation on TDB benchmark instead."""
    print("\n" + "=" * 70)
    print("ABLATION STUDY ON TDB BENCHMARK")
    print("=" * 70)
    
    # Load TDB benchmark
    tdb_path = Path(__file__).parent / "temporal_benchmark.json"
    if not tdb_path.exists():
        print(f"TDB benchmark not found at {tdb_path}")
        return
    
    with open(tdb_path) as f:
        benchmark = json.load(f)
    
    cases = benchmark["cases"][:40]  # Limit for speed
    
    results = []
    
    for config in ABLATION_CONFIGS:
        correct = 0
        total = 0
        
        base_time = time.time() - (30 * 24 * 3600)
        
        for case in cases:
            mem = AblationMemory(config)
            
            # Add memories with timestamps
            for event in case["setup"]:
                event_time = base_time + (event["day"] * 24 * 3600)
                mem.add(
                    content=event["memory"],
                    importance=event.get("importance", 0.5),
                )
                # Manually set timestamp (hacky but works for test)
                if mem._memories:
                    mem._memories[-1]["created_at"] = event_time
            
            # Query
            recalled = mem.recall(case["query"], limit=5)
            
            if recalled:
                top_content = recalled[0]["content"].lower()
                expected = case["expected"].lower()
                wrong = [w.lower() for w in case.get("wrong", [])]
                
                if expected in top_content and not any(w in top_content for w in wrong):
                    correct += 1
            
            total += 1
        
        accuracy = correct / total if total > 0 else 0
        results.append({
            "config": config.name,
            "correct": correct,
            "total": total,
            "accuracy": accuracy,
        })
        print(f"{config.name}: {accuracy:.1%} ({correct}/{total})")
    
    # Print comparison
    print("\n" + "-" * 40)
    print(f"{'Config':<20} {'Accuracy':<10}")
    print("-" * 40)
    for r in sorted(results, key=lambda x: x['accuracy'], reverse=True):
        print(f"{r['config']:<20} {r['accuracy']:<10.1%}")
    
    best = max(results, key=lambda x: x['accuracy'])
    worst = min(results, key=lambda x: x['accuracy'])
    print(f"\nBest: {best['config']} ({best['accuracy']:.1%})")
    print(f"Worst: {worst['config']} ({worst['accuracy']:.1%})")


if __name__ == "__main__":
    main()
