# LoCoMo Benchmark Results - NeuromemoryAI

*Generated: 2026-02-03T12:23:02.677393*

## Stage 1: Retrieval Quality (No LLM)

This measures if NeuromemoryAI retrieves the correct memories.

| Metric | Value | Description |
|--------|-------|-------------|
| **MRR** | **0.082** | Mean Reciprocal Rank |
| Hit@1 | 7.5% | Correct in top 1 |
| Hit@5 | 10.1% | Correct in top 5 |
| Avg Latency | 16.5ms | Retrieval speed |

## Stage 2: End-to-End QA (With LLM)

This measures answer quality using Claude Haiku.

| Metric | Value |
|--------|-------|
| **F1 Score** | **0.032** |
| Exact Match | 26.1% |

## Key Insights

- **Retrieval metrics** measure NeuromemoryAI's unique contribution
- **QA metrics** measure the combined Memory + LLM system
- Latency is consistently low (<10ms) due to local FTS5
