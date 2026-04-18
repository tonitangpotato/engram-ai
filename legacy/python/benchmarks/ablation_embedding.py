#!/usr/bin/env python3
"""
Ablation Study with Embeddings

Compares:
1. Keyword-only (baseline)
2. Embedding-only 
3. Embedding + ACT-R (our approach)
"""

import json
import sys
import time
import argparse
import numpy as np
from pathlib import Path
from dataclasses import dataclass
from typing import Dict, List, Optional
from collections import defaultdict

sys.path.insert(0, str(Path(__file__).parent.parent))


# Try to load embedding model
try:
    from sentence_transformers import SentenceTransformer
    EMBEDDING_MODEL = SentenceTransformer('all-MiniLM-L6-v2')
    print("Loaded embedding model: all-MiniLM-L6-v2")
except Exception as e:
    print(f"Warning: Could not load embedding model: {e}")
    EMBEDDING_MODEL = None


def keyword_similarity(query: str, content: str) -> float:
    """Simple Jaccard-like similarity."""
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 
                  'is', 'was', 'are', 'were', 'be', 'been', 'said', 'that', 'this',
                  'i', 'you', 'he', 'she', 'it', 'we', 'they', 'my', 'your', 'do',
                  'does', 'did', 'have', 'has', 'had', 'what', 'when', 'where', 'who'}
    
    query_words = set(query.lower().split()) - stop_words
    content_words = set(content.lower().split()) - stop_words
    
    if not query_words or not content_words:
        return 0.0
    
    intersection = query_words & content_words
    union = query_words | content_words
    return len(intersection) / len(union) if union else 0.0


def cosine_similarity(a: np.ndarray, b: np.ndarray) -> float:
    """Cosine similarity between two vectors."""
    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-8))


@dataclass
class Config:
    name: str
    use_embedding: bool = False
    use_actr: bool = False
    use_recency: bool = False


class Memory:
    def __init__(self, config: Config):
        self.config = config
        self._memories: List[dict] = []
        self._embeddings: List[np.ndarray] = []
        self._access_counts: Dict[int, int] = {}
        
    def add(self, content: str, dia_id: str):
        idx = len(self._memories)
        self._memories.append({
            "idx": idx,
            "content": content,
            "dia_id": dia_id,
        })
        self._access_counts[idx] = 1
        
        if self.config.use_embedding and EMBEDDING_MODEL:
            emb = EMBEDDING_MODEL.encode(content, convert_to_numpy=True)
            self._embeddings.append(emb)
    
    def recall(self, query: str, limit: int = 10) -> List[dict]:
        if not self._memories:
            return []
        
        # Compute query embedding if needed
        query_emb = None
        if self.config.use_embedding and EMBEDDING_MODEL:
            query_emb = EMBEDDING_MODEL.encode(query, convert_to_numpy=True)
        
        scored = []
        for i, mem in enumerate(self._memories):
            score = 0.0
            
            if self.config.use_embedding and query_emb is not None:
                # Semantic similarity (0 to 1, usually 0.3-0.8 for related)
                sim = cosine_similarity(query_emb, self._embeddings[i])
                score += sim * 10.0
            else:
                # Keyword similarity
                sim = keyword_similarity(query, mem["content"])
                score += sim * 10.0
            
            if self.config.use_actr:
                # ACT-R: small boost for access count
                import math
                access = self._access_counts.get(i, 1)
                score += math.log(1 + access) * 0.3
            
            if self.config.use_recency:
                # Small recency bonus (newer = higher idx = small boost)
                recency = i / max(len(self._memories), 1)
                score += recency * 0.1  # Very small!
            
            scored.append((mem, score))
        
        scored.sort(key=lambda x: x[1], reverse=True)
        
        # Update access counts for top results
        for mem, _ in scored[:limit]:
            self._access_counts[mem["idx"]] = self._access_counts.get(mem["idx"], 0) + 1
        
        return [{"dia_id": m["dia_id"], "content": m["content"], "score": s} 
                for m, s in scored[:limit]]


CONFIGS = [
    Config(name="keyword-only", use_embedding=False, use_actr=False, use_recency=False),
    Config(name="embedding-only", use_embedding=True, use_actr=False, use_recency=False),
    Config(name="embedding+actr", use_embedding=True, use_actr=True, use_recency=False),
    Config(name="embedding+recency", use_embedding=True, use_actr=False, use_recency=True),
    Config(name="embedding+all", use_embedding=True, use_actr=True, use_recency=True),
]


def calculate_metrics(recalled: List[dict], evidence_ids: List[str]) -> dict:
    evidence_set = set(evidence_ids)
    
    # MRR
    first_rank = None
    for rank, mem in enumerate(recalled, 1):
        if mem["dia_id"] in evidence_set:
            first_rank = rank
            break
    mrr = 1.0 / first_rank if first_rank else 0.0
    
    # Hit@K
    results = {"mrr": mrr}
    for k in [1, 3, 5, 10]:
        top_k = {m["dia_id"] for m in recalled[:k]}
        results[f"hit@{k}"] = 1 if evidence_set & top_k else 0
    
    return results


def run_eval(config: Config, data: List[dict], verbose: bool = False) -> dict:
    metrics = defaultdict(list)
    category_metrics = defaultdict(lambda: defaultdict(list))
    
    for sample in data:
        conversation = sample.get("conversation", {})
        qa_pairs = sample.get("qa", [])
        
        mem = Memory(config)
        
        # Load conversation
        session_num = 1
        while f"session_{session_num}" in conversation:
            for turn in conversation[f"session_{session_num}"]:
                content = f"{turn['speaker']}: {turn['text']}"
                mem.add(content=content, dia_id=turn.get("dia_id", ""))
            session_num += 1
        
        # Evaluate
        for qa in qa_pairs:
            question = qa.get("question", "")
            evidence = qa.get("evidence", [])
            category = str(qa.get("category", "unknown"))
            
            if not question or not evidence:
                continue
            
            recalled = mem.recall(question, limit=10)
            result = calculate_metrics(recalled, evidence)
            
            for k, v in result.items():
                metrics[k].append(v)
                category_metrics[category][k].append(v)
            
            if verbose and result["hit@5"] == 0:
                print(f"  MISS [{category}]: {question[:50]}...")
                print(f"    Evidence: {evidence}")
                print(f"    Top 3: {[r['dia_id'] for r in recalled[:3]]}")
    
    results = {"config": config.name}
    for k, values in metrics.items():
        results[k] = sum(values) / len(values) if values else 0
    results["total"] = len(metrics["mrr"])
    
    results["by_category"] = {}
    for cat, cat_m in category_metrics.items():
        results["by_category"][cat] = {
            k: sum(v) / len(v) if v else 0 for k, v in cat_m.items()
        }
        results["by_category"][cat]["n"] = len(cat_m["mrr"])
    
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()
    
    data_path = Path(__file__).parent / "locomo" / "data" / "locomo10.json"
    with open(data_path) as f:
        data = json.load(f)
    
    if args.limit:
        data = data[:args.limit]
    
    total_qs = sum(len(s.get("qa", [])) for s in data)
    print(f"\nLoaded {len(data)} conversations, {total_qs} questions")
    
    print("\n" + "=" * 80)
    print("EMBEDDING ABLATION - RETRIEVAL METRICS")
    print("=" * 80)
    
    all_results = []
    
    for config in CONFIGS:
        print(f"\nTesting: {config.name}...")
        start = time.time()
        result = run_eval(config, data, args.verbose)
        elapsed = time.time() - start
        all_results.append(result)
        print(f"  MRR: {result['mrr']:.3f} | Hit@5: {result['hit@5']:.3f} | Time: {elapsed:.1f}s")
    
    print("\n" + "=" * 80)
    print("RESULTS")
    print("=" * 80)
    print(f"{'Config':<20} {'MRR':<8} {'H@1':<8} {'H@5':<8} {'H@10':<8}")
    print("-" * 60)
    
    for r in sorted(all_results, key=lambda x: x['mrr'], reverse=True):
        print(f"{r['config']:<20} {r['mrr']:<8.3f} {r['hit@1']:<8.3f} {r['hit@5']:<8.3f} {r['hit@10']:<8.3f}")
    
    best = max(all_results, key=lambda x: x['mrr'])
    baseline = next(r for r in all_results if r['config'] == 'keyword-only')
    
    print(f"\nBest: {best['config']} (MRR={best['mrr']:.3f})")
    print(f"vs keyword baseline: +{(best['mrr'] - baseline['mrr']) / max(baseline['mrr'], 0.001) * 100:.1f}%")
    
    # Category breakdown
    print(f"\n--- Category breakdown for '{best['config']}' ---")
    for cat, d in sorted(best["by_category"].items()):
        print(f"  Cat {cat}: MRR={d['mrr']:.3f}, Hit@5={d['hit@5']:.3f}, n={d['n']}")


if __name__ == "__main__":
    main()
