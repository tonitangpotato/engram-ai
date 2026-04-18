# Benchmarks for NeuromemoryAI (Engram)

This directory contains benchmark evaluations for the Engram memory system.

## LoCoMo Benchmark

**LoCoMo** (Long-term Conversational Memory) is a benchmark from Snap Research (ACL 2024) for evaluating very long-term conversational memory in LLM agents.

### Quick Start

```bash
# From project root
source .venv/bin/activate

# Run evaluation (requires ANTHROPIC_API_KEY for full results)
python benchmarks/eval_locomo.py

# Test with limited conversations
python benchmarks/eval_locomo.py --limit 2 --verbose

# See all options
python benchmarks/eval_locomo.py --help
```

### Requirements

**Core Dependencies** (already in environment):
- `engram` package (the memory system being tested)
- SQLite with FTS5 (for text search)

**Optional** (for full LLM-based evaluation):
- `anthropic` package: `pip install anthropic`
- `ANTHROPIC_API_KEY` environment variable

Without the Anthropic API, the evaluation runs but uses simple memory extraction rather than LLM-based answer generation, resulting in artificially low F1 scores.

### What the Benchmark Tests

The LoCoMo benchmark evaluates:

1. **Memory Storage**: Can the system store long conversations (19 sessions spanning months)?
2. **Memory Consolidation**: Does "sleep" consolidation improve memory organization?
3. **Retrieval Accuracy**: Can the system recall relevant information for questions?
4. **Answer Quality**: Can the system synthesize correct answers from memories?
5. **Latency**: How fast is memory recall?

### Question Categories

- **Single-hop** (282 questions): Direct factual recall
  - Example: "What is Caroline's identity?"
  
- **Temporal** (321 questions): Time-based reasoning
  - Example: "When did Caroline go to the LGBTQ support group?"
  
- **Multi-hop** (96 questions): Inference across multiple memories
  - Example: "What fields would Caroline likely pursue in her education?"
  
- **Open-domain** (1,287 questions): Complex reasoning and synthesis
  - Example: "Would Caroline likely have Dr. Seuss books on her bookshelf?"

### Results

See **[LOCOMO_RESULTS.md](LOCOMO_RESULTS.md)** for current benchmark results.

**Current Status** (without Claude API):
- ✅ Memory storage and retrieval working
- ✅ Very fast recall (5.1ms average)
- ⚠️ F1 scores artificially low due to missing LLM component
- ❌ Not yet comparable to other systems (Mem0, etc.)

**With Claude API** (expected):
- Comparable or better F1 scores due to neuroscience-grounded retrieval
- Still fast recall latency
- Potential advantages in temporal and multi-hop reasoning

### Files

```
benchmarks/
├── README.md                      # This file
├── eval_locomo.py                 # Main evaluation script
├── LOCOMO_RESULTS.md             # Results report
├── locomo_predictions.json       # Detailed predictions (generated)
└── locomo/                       # LoCoMo dataset (cloned from GitHub)
    ├── README.MD
    ├── data/
    │   └── locomo10.json         # 10 conversations with QA annotations
    └── task_eval/
        └── ...                   # Original evaluation scripts
```

### Running with Claude API

```bash
# Set your API key
export ANTHROPIC_API_KEY="sk-ant-..."

# Run full evaluation
python benchmarks/eval_locomo.py

# This will use Claude to generate answers from recalled memories
# and produce comparable F1 scores
```

### Customization

The evaluation script supports several options:

```bash
# Limit number of conversations (for testing)
python benchmarks/eval_locomo.py --limit 2

# Verbose output (show recalled memories, sample questions)
python benchmarks/eval_locomo.py --verbose

# Custom output path
python benchmarks/eval_locomo.py --output my_results.md

# Save detailed predictions
python benchmarks/eval_locomo.py --save-predictions predictions.json
```

### Adding More Benchmarks

To add a new benchmark:

1. Create a new script: `benchmarks/eval_<benchmark_name>.py`
2. Follow the pattern from `eval_locomo.py`:
   - Load data into Engram Memory
   - Run consolidation if appropriate
   - Evaluate recall quality
   - Generate metrics report
3. Document results in `benchmarks/<BENCHMARK>_RESULTS.md`

### References

- **LoCoMo Paper**: [Evaluating Very Long-Term Conversational Memory of LLM Agents](https://github.com/snap-research/locomo) (ACL 2024)
- **Mem0**: [Comparison benchmark system](https://github.com/mem0ai/mem0)
- **Engram**: [NeuromemoryAI documentation](../README.md)

---

**Last Updated**: 2025-02-03
