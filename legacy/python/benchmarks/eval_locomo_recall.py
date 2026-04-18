#!/usr/bin/env python3
"""
LoCoMo Recall@K Evaluation for NeuromemoryAI (Engram)

Evaluates memory recall quality WITHOUT requiring an LLM.
Measures whether relevant memories appear in top-K results.

Metrics:
- Recall@K: % of questions where relevant memory is in top-K
- MRR (Mean Reciprocal Rank): Average position of first relevant memory
- Precision@K: % of retrieved memories that are relevant

Usage:
    source .venv/bin/activate
    python benchmarks/eval_locomo_recall.py [--k 5,10,20]
"""

import json
import sys
from pathlib import Path
from typing import Dict, List, Set
import argparse
import time

sys.path.insert(0, str(Path(__file__).parent.parent))

from engram import Memory


def parse_args():
    parser = argparse.ArgumentParser(description="Evaluate Engram memory recall quality (no LLM needed)")
    parser.add_argument(
        "--data-file",
        type=str,
        default="benchmarks/locomo/data/locomo10.json",
        help="Path to LoCoMo data file"
    )
    parser.add_argument(
        "--k-values",
        type=str,
        default="5,10,20",
        help="Comma-separated K values for Recall@K (e.g., '5,10,20')"
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=None,
        help="Limit number of conversations to evaluate"
    )
    parser.add_argument(
        "--output",
        type=str,
        default="benchmarks/LOCOMO_RECALL_RESULTS.md",
        help="Output file for results"
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print verbose output"
    )
    return parser.parse_args()


def load_conversation_into_memory(conversation: Dict, mem: Memory, verbose: bool = False) -> Dict[str, str]:
    """
    Load a LoCoMo conversation into memory.
    Returns mapping of dialogue_id -> memory_id for relevance checking.
    """
    dia_id_to_mem_id = {}
    session_num = 1
    
    if verbose:
        print(f"  Loading conversation...")
    
    while f"session_{session_num}" in conversation:
        session_key = f"session_{session_num}"
        session_turns = conversation[session_key]
        
        for turn in session_turns:
            speaker = turn["speaker"]
            text = turn["text"]
            dia_id = turn["dia_id"]
            
            content = f"{speaker} said: {text}"
            
            mem_id = mem.add(
                content=content,
                type="episodic",
                source=dia_id,  # Store dialogue ID as source for matching
                tags=[session_key, speaker],
                importance=0.5,
            )
            
            dia_id_to_mem_id[dia_id] = mem_id
        
        session_num += 1
    
    if verbose:
        print(f"  Loaded {len(dia_id_to_mem_id)} dialogue turns across {session_num-1} sessions")
    
    return dia_id_to_mem_id


def sanitize_query(query: str) -> str:
    """Sanitize query for FTS5."""
    import re
    sanitized = re.sub(r'[^a-zA-Z0-9\s]', ' ', query)
    sanitized = re.sub(r'\s+', ' ', sanitized).strip()
    if not sanitized:
        sanitized = "memory"
    stop_words = {'the', 'a', 'an', 'to', 'of', 'in', 'on', 'at', 'for', 'with', 'is', 'was', 'are', 'were', 'be', 'been'}
    words = [w for w in sanitized.lower().split() if w not in stop_words and len(w) > 2]
    return ' '.join(words) if words else sanitized


def evaluate_recall_at_k(
    qa_pairs: List[Dict],
    mem: Memory,
    k_values: List[int],
    verbose: bool = False
) -> Dict:
    """
    Evaluate Recall@K, MRR, and Precision@K.
    
    Returns:
        {
            'recall_at_k': {5: 0.45, 10: 0.62, ...},
            'mrr': 0.35,
            'precision_at_k': {5: 0.12, 10: 0.08, ...},
            'per_question_results': [...]
        }
    """
    max_k = max(k_values)
    
    recall_at_k = {k: 0 for k in k_values}
    precision_at_k_sum = {k: 0 for k in k_values}
    reciprocal_ranks = []
    per_question_results = []
    
    questions_processed = 0
    total_recall_time = 0.0
    
    for i, qa in enumerate(qa_pairs):
        question = qa["question"]
        evidence_ids = set(qa.get("evidence", []))  # e.g., {"D1:3", "D2:5"}
        
        if not evidence_ids:
            continue  # Skip questions without evidence
        
        questions_processed += 1
        
        # Recall memories
        sanitized_q = sanitize_query(question)
        start_time = time.time()
        recalled = mem.recall(query=sanitized_q, limit=max_k, min_confidence=0.0)
        recall_time = time.time() - start_time
        total_recall_time += recall_time
        
        # Check which recalled memories are relevant
        relevant_positions = []
        for pos, memory in enumerate(recalled, start=1):
            # Memory is relevant if its source matches any evidence dialogue ID
            if memory.get('source') in evidence_ids:
                relevant_positions.append(pos)
        
        # Calculate metrics
        found_at_k = {}
        precision_at_k_vals = {}
        
        for k in k_values:
            # Recall@K: Was any relevant memory in top-K?
            found = any(pos <= k for pos in relevant_positions)
            found_at_k[k] = found
            if found:
                recall_at_k[k] += 1
            
            # Precision@K: What % of top-K are relevant?
            num_relevant_in_k = sum(1 for pos in relevant_positions if pos <= k)
            precision_at_k_vals[k] = num_relevant_in_k / min(k, len(recalled)) if recalled else 0
            precision_at_k_sum[k] += precision_at_k_vals[k]
        
        # MRR: Reciprocal rank of first relevant memory
        if relevant_positions:
            first_relevant_pos = min(relevant_positions)
            reciprocal_ranks.append(1.0 / first_relevant_pos)
        else:
            reciprocal_ranks.append(0.0)
        
        # Store per-question result
        per_question_results.append({
            "question": question,
            "category": qa.get("category"),
            "evidence_count": len(evidence_ids),
            "relevant_positions": relevant_positions,
            "found_at_k": found_at_k,
            "precision_at_k": precision_at_k_vals,
            "recall_time_ms": round(recall_time * 1000, 1),
        })
        
        if verbose and (i < 5 or i % 50 == 0):
            print(f"  Q{i+1}: {question[:60]}...")
            print(f"    Evidence: {len(evidence_ids)} dialogue(s)")
            print(f"    Found at positions: {relevant_positions if relevant_positions else 'NOT FOUND'}")
            for k in k_values:
                print(f"    Recall@{k}: {'✓' if found_at_k[k] else '✗'}")
    
    # Calculate averages
    avg_recall_at_k = {k: count / questions_processed for k, count in recall_at_k.items()}
    avg_precision_at_k = {k: total / questions_processed for k, total in precision_at_k_sum.items()}
    mrr = sum(reciprocal_ranks) / len(reciprocal_ranks) if reciprocal_ranks else 0
    avg_recall_time_ms = (total_recall_time / questions_processed * 1000) if questions_processed else 0
    
    return {
        "recall_at_k": avg_recall_at_k,
        "mrr": mrr,
        "precision_at_k": avg_precision_at_k,
        "avg_recall_time_ms": avg_recall_time_ms,
        "questions_processed": questions_processed,
        "per_question_results": per_question_results,
    }


def evaluate_conversation(sample: Dict, k_values: List[int], verbose: bool = False) -> Dict:
    """Evaluate a single conversation."""
    sample_id = sample.get("sample_id", "unknown")
    conversation = sample["conversation"]
    qa_pairs = sample.get("qa", [])
    
    if verbose:
        print(f"\n{'='*80}")
        print(f"Evaluating sample: {sample_id}")
        print(f"{'='*80}")
    
    # Create fresh memory
    mem = Memory(":memory:")
    
    # Load conversation
    dia_id_to_mem_id = load_conversation_into_memory(conversation, mem, verbose=verbose)
    
    # Run consolidation
    if verbose:
        print(f"  Running consolidation...")
    mem.consolidate(days=1.0)
    
    # Evaluate recall quality
    if verbose:
        print(f"  Evaluating {len(qa_pairs)} questions...")
    
    results = evaluate_recall_at_k(qa_pairs, mem, k_values, verbose=verbose)
    results["sample_id"] = sample_id
    results["num_questions"] = len(qa_pairs)
    results["num_memories"] = len(dia_id_to_mem_id)
    
    return results


def compute_aggregate_stats(all_results: List[Dict], k_values: List[int]) -> Dict:
    """Compute aggregate statistics across all conversations."""
    # Collect all per-question results
    all_questions = []
    for conv_result in all_results:
        all_questions.extend(conv_result["per_question_results"])
    
    # Overall metrics
    total_questions = sum(r["questions_processed"] for r in all_results)
    
    # Aggregate Recall@K
    overall_recall_at_k = {}
    for k in k_values:
        total_found = sum(
            sum(1 for q in r["per_question_results"] if q["found_at_k"][k])
            for r in all_results
        )
        overall_recall_at_k[k] = total_found / total_questions if total_questions else 0
    
    # Aggregate MRR
    overall_mrr = sum(r["mrr"] * r["questions_processed"] for r in all_results) / total_questions if total_questions else 0
    
    # Aggregate Precision@K
    overall_precision_at_k = {}
    for k in k_values:
        total_precision = sum(
            sum(q["precision_at_k"][k] for q in r["per_question_results"])
            for r in all_results
        )
        overall_precision_at_k[k] = total_precision / total_questions if total_questions else 0
    
    # Average recall time
    avg_recall_time = sum(r["avg_recall_time_ms"] * r["questions_processed"] for r in all_results) / total_questions if total_questions else 0
    
    # Per-category stats
    category_stats = {}
    for cat in [1, 2, 3, 4, 5]:
        cat_questions = [q for q in all_questions if q.get("category") == cat]
        if cat_questions:
            cat_recall_at_k = {
                k: sum(1 for q in cat_questions if q["found_at_k"][k]) / len(cat_questions)
                for k in k_values
            }
            category_stats[cat] = {
                "count": len(cat_questions),
                "recall_at_k": cat_recall_at_k,
            }
    
    return {
        "total_questions": total_questions,
        "recall_at_k": overall_recall_at_k,
        "mrr": overall_mrr,
        "precision_at_k": overall_precision_at_k,
        "avg_recall_time_ms": avg_recall_time,
        "category_stats": category_stats,
    }


def format_results_table(stats: Dict, k_values: List[int]) -> str:
    """Format results as markdown."""
    lines = []
    lines.append("# LoCoMo Recall Quality Evaluation - NeuromemoryAI (Engram)")
    lines.append("")
    lines.append("## Overview")
    lines.append("")
    lines.append(f"**Total Questions**: {stats['total_questions']}")
    lines.append(f"**Mean Reciprocal Rank (MRR)**: {stats['mrr']:.3f}")
    lines.append(f"**Average Recall Latency**: {stats['avg_recall_time_ms']:.1f}ms")
    lines.append("")
    
    lines.append("## Recall@K Performance")
    lines.append("")
    lines.append("| K | Recall@K | Precision@K |")
    lines.append("|---|----------|-------------|")
    for k in k_values:
        recall = stats['recall_at_k'][k]
        precision = stats['precision_at_k'][k]
        lines.append(f"| {k} | {recall:.3f} | {precision:.3f} |")
    lines.append("")
    
    lines.append("## Category Breakdown")
    lines.append("")
    lines.append("### Recall@K by Question Category")
    lines.append("")
    
    category_names = {
        1: "single-hop",
        2: "temporal",
        3: "multi-hop",
        4: "open-domain-1",
        5: "open-domain-2",
    }
    
    for cat_num in sorted(stats["category_stats"].keys()):
        cat_name = category_names.get(cat_num, f"category-{cat_num}")
        cat_data = stats["category_stats"][cat_num]
        lines.append(f"**{cat_name}** ({cat_data['count']} questions):")
        lines.append("")
        for k in k_values:
            recall = cat_data["recall_at_k"][k]
            lines.append(f"- Recall@{k}: {recall:.3f}")
        lines.append("")
    
    return "\n".join(lines)


def main():
    args = parse_args()
    
    k_values = [int(k.strip()) for k in args.k_values.split(",")]
    
    print(f"LoCoMo Recall@K Evaluation (K={k_values})")
    print("="*80)
    print(f"\nLoading data from: {args.data_file}")
    
    with open(args.data_file) as f:
        conversations = json.load(f)
    
    if args.limit:
        conversations = conversations[:args.limit]
        print(f"Limiting to {args.limit} conversations")
    
    print(f"Evaluating {len(conversations)} conversations...\n")
    
    # Evaluate each conversation
    all_results = []
    for i, sample in enumerate(conversations):
        sample_id = sample.get("sample_id", f"sample_{i}")
        print(f"[{i+1}/{len(conversations)}] Processing {sample_id}...")
        
        result = evaluate_conversation(sample, k_values, verbose=args.verbose)
        all_results.append(result)
        
        if not args.verbose:
            # Print quick summary
            print(f"  Questions: {result['questions_processed']}")
            print(f"  Recall@10: {result['recall_at_k'][10]:.3f}")
            print(f"  MRR: {result['mrr']:.3f}")
    
    # Compute aggregate stats
    print("\nComputing aggregate statistics...")
    stats = compute_aggregate_stats(all_results, k_values)
    
    # Format and display
    results_table = format_results_table(stats, k_values)
    print("\n" + "="*80)
    print(results_table)
    print("="*80)
    
    # Save results
    print(f"\nSaving results to: {args.output}")
    with open(args.output, "w") as f:
        f.write(results_table)
        f.write("\n\n---\n\n")
        f.write("## Interpretation\n\n")
        f.write(f"- **Recall@K**: Percentage of questions where at least one relevant memory appears in top-K results\n")
        f.write(f"- **Precision@K**: Average percentage of retrieved memories (in top-K) that are relevant\n")
        f.write(f"- **MRR**: Mean Reciprocal Rank - average of 1/rank for first relevant memory\n\n")
        f.write(f"**Note**: This evaluation measures memory recall quality WITHOUT an LLM. ")
        f.write(f"It shows whether the memory system can find relevant information, ")
        f.write(f"regardless of whether it can synthesize a correct answer.\n")
    
    print("\n✓ Evaluation complete!")


if __name__ == "__main__":
    main()
