# Industry Standard Benchmarks

This document tracks the industry-standard benchmarks for evaluating agent memory systems.

---

## 1. LoCoMo (ACL 2024)

**Paper**: "Evaluating Very Long-Term Conversational Memory of LLM Agents"
**Authors**: Snap Research (Maharana et al.)
**GitHub**: https://github.com/snap-research/locomo

### Dataset
- **10 conversations** with very long-term dialogue
- **1,986 questions** across categories
- Includes timestamps, multi-session data

### Question Categories
1. **Single-hop** - Direct factual recall
2. **Multi-hop** - Reasoning across multiple memories
3. **Temporal** - Time-based reasoning
4. **Open-domain** - General knowledge + conversation

### Evaluation Method
- RAG-based evaluation
- LLM-as-judge scoring
- Uses observations/session summaries as retrieval database

### How to Run
```bash
git clone https://github.com/snap-research/locomo
cd locomo

# Data is in ./data/locomo10.json

# Evaluate RAG model:
bash scripts/evaluate_rag_gpts.sh
```

### Integration Plan for NeuromemoryAI
1. Load LoCoMo conversations into Memory
2. Use each session as add() calls with timestamps
3. Answer questions using recall() + LLM
4. Compare against Mem0's published scores

---

## 2. LongMemEval (ICLR 2025)

**Paper**: "Benchmarking Chat Assistants on Long-Term Interactive Memory"
**GitHub**: https://github.com/xiaowu0162/LongMemEval

### Dataset
- Dynamic multi-session interactions
- Tests online memorization during chat
- Questions answered after all interaction sessions

### Key Features
- Tests **incremental** memory (not just batch recall)
- Closer to real-world agent usage patterns
- MCQ format available for objective evaluation

### Integration Plan for NeuromemoryAI
1. Simulate interactive sessions
2. add() memories during conversation
3. consolidate() between sessions
4. Answer final questions with recall()

---

## 3. Competitor Published Results

### On LoCoMo

| System | Single-hop | Multi-hop | Temporal | Overall |
|--------|------------|-----------|----------|---------|
| Mem0 | ? | ? | ? | ? |
| MemMachine | 85%+ | ? | ? | SOTA |
| HippoRAG | ? | ? | ? | ? |
| Zep | ? | ? | ? | ? |

*(Need to fill in from papers/blog posts)*

### On LongMemEval

| System | Accuracy |
|--------|----------|
| EmergenceMem (GPT-4o mini) | 76.8% |
| ... | ... |

---

## Priority Action Items

### Phase 1: Reproduce Baselines
- [ ] Clone LoCoMo repo
- [ ] Run their baseline RAG eval
- [ ] Understand scoring methodology

### Phase 2: Integrate NeuromemoryAI
- [ ] Write loader for LoCoMo format → engram.add()
- [ ] Implement recall + LLM answering pipeline
- [ ] Score using their evaluation scripts

### Phase 3: Report Results
- [ ] Single-hop accuracy
- [ ] Multi-hop accuracy  
- [ ] Temporal accuracy
- [ ] Latency comparison
- [ ] Zero-dependency advantage

### Phase 4: Home Turf Benchmark
Design benchmark that showcases NeuromemoryAI's unique strengths:
- Forgetting effectiveness (noise reduction)
- Consolidation (important memory persistence)
- Hebbian emergence (association formation)

---

## Key Insight from Feedback

> "哪怕你的分数比Mem0低，只要你能展示'在零embedding依赖条件下达到X%的准确率，同时latency低15倍'，这就是一个有意义的结果。"

The goal is NOT to beat Mem0 on raw accuracy. The goal is to demonstrate a **meaningful tradeoff**:
- Zero dependencies
- 15x lower latency
- Comparable accuracy (even if slightly lower)
- Unique capabilities (forgetting, consolidation, Hebbian)

---

## Optional Embedding Layer

The 40% Recall@1 result suggests FTS5 has limitations with semantic matching.

**Decision needed**: Add optional embedding layer?

Pros:
- Better semantic recall
- Competitive with Mem0 on accuracy
- Can use local models (no API dependency)

Cons:
- Breaks "zero dependency" claim
- Adds complexity
- May reduce speed advantage

**Proposed solution**: 
- Keep FTS5 as default (zero-dep)
- Optional `pip install neuromemory-ai[embeddings]` for semantic search
- Let users choose tradeoff

---

*Created: 2026-02-03*
