# LoCoMo Recall Quality Evaluation - NeuromemoryAI (Engram)

## Overview

**Total Questions**: 1982
**Mean Reciprocal Rank (MRR)**: 0.007
**Average Recall Latency**: 6.3ms

## Recall@K Performance

| K | Recall@K | Precision@K |
|---|----------|-------------|
| 5 | 0.007 | 0.004 |
| 10 | 0.016 | 0.005 |
| 20 | 0.031 | 0.005 |

## Category Breakdown

### Recall@K by Question Category

**single-hop** (282 questions):

- Recall@5: 0.007
- Recall@10: 0.011
- Recall@20: 0.035

**temporal** (321 questions):

- Recall@5: 0.000
- Recall@10: 0.009
- Recall@20: 0.037

**multi-hop** (92 questions):

- Recall@5: 0.022
- Recall@10: 0.033
- Recall@20: 0.054

**open-domain-1** (841 questions):

- Recall@5: 0.007
- Recall@10: 0.018
- Recall@20: 0.027

**open-domain-2** (446 questions):

- Recall@5: 0.009
- Recall@10: 0.018
- Recall@20: 0.027


---

## Interpretation

- **Recall@K**: Percentage of questions where at least one relevant memory appears in top-K results
- **Precision@K**: Average percentage of retrieved memories (in top-K) that are relevant
- **MRR**: Mean Reciprocal Rank - average of 1/rank for first relevant memory

**Note**: This evaluation measures memory recall quality WITHOUT an LLM. It shows whether the memory system can find relevant information, regardless of whether it can synthesize a correct answer.
