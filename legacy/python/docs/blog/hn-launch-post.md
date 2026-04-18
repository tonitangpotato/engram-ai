# Show HN: Engram – Neuroscience-grounded memory for AI agents (ACT-R, not cosine similarity)

Every AI agent framework bolts on memory as an afterthought: embed text → store in vector DB → retrieve by cosine similarity → hope for the best.

This ignores everything we know about how memory actually works.

I built **Engram** — a memory system for AI agents that uses actual cognitive science models instead of naive embeddings.

## The Problem

Vector similarity is great for finding semantically similar content. It's terrible at deciding *which* memories to surface when many are relevant. Human memory doesn't work by finding the closest vector — it's a dynamic system where:

- Recent memories are more accessible than old ones
- Frequently accessed memories strengthen
- Unused memories fade over time
- Important experiences consolidate during "sleep"
- Related concepts activate each other

Current agent memory systems give you a memory from 6 months ago with the same weight as one from 5 minutes ago. They never forget anything, drowning in noise. They can't distinguish between a life lesson and a random observation.

## The Solution

Engram implements three peer-reviewed models from cognitive science:

**1. ACT-R Activation** (Anderson et al.)
```
Activation = Base + Context + Importance
Base = ln(Σ t^(-0.5))  ← power law of recency/frequency
```

Instead of ranking by cosine similarity, we rank by *activation* — a formula that naturally handles when you last accessed a memory, how often, and how relevant it is to current context.

**2. Memory Chain Model** (Murre & Chessa, 2011)
```
dr₁/dt = -μ₁·r₁          (working memory decays fast)
dr₂/dt = α·r₁ - μ₂·r₂    (core memory consolidates slowly)
```

Two-trace memory system. New memories start in "working memory" (like hippocampus), then consolidate into "core memory" (neocortex) over time. Important memories consolidate faster.

**3. Ebbinghaus Forgetting Curves** (1885)
```
R(t) = e^(-t/S)
```

Retrievability decays exponentially, but stability increases with each successful retrieval (spaced repetition effect).

## Show Me the Code

```python
from engram import Memory

mem = Memory("./agent.db")  # Zero external dependencies, just SQLite

# Store with type and importance
mem.add("User prefers functional programming", type="relational", importance=0.7)
mem.add("Always validate input before DB queries", type="procedural", importance=0.9)

# Recall — ranked by ACT-R activation, not cosine similarity
results = mem.recall("coding best practices", limit=3)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# Run "sleep" — transfers working → core memory
mem.consolidate()

# Learn from feedback
mem.reward("perfect, that's what I needed!")  # Strengthens recent memories
```

## Benchmark Results

**Semantic Retrieval (LoCoMo benchmark):**
- With embeddings: MRR 0.255, Hit@5 38.9% — competitive with Mem0/Zep

**Temporal Dynamics (our benchmark):**

| Method | Recency Override | Frequency | Importance | Overall |
|--------|------------------|-----------|------------|---------|
| Cosine-only | 0% | 18% | 50% | 22% |
| **ACT-R** | **60%** | **100%** | **100%** | **80%** |

The key insight: semantic retrieval and temporal reasoning are *different problems*. Use embeddings to *find* relevant memories; use ACT-R to *prioritize* which ones to surface.

## Extra Features

Beyond the core models:

- **6 memory types** (factual, episodic, relational, emotional, procedural, opinion) with distinct decay rates
- **Hebbian learning** — "neurons that fire together wire together" — memories that get recalled together automatically form associations
- **Confidence scoring** — metacognitive monitoring tells you how much to trust each retrieval
- **Reward learning** — positive/negative feedback shapes which memories get strengthened
- **Contradiction detection** — handles memory updates by marking old memories as superseded
- **Session working memory** — cognitive chunking model reduces recall API calls by 70-80%

## What Engram Doesn't Do

It's not a replacement for vector search when you need pure semantic matching. If your query is "find all documents mentioning X", use a vector DB.

Engram is for *agent memory* — when you need to decide which of many relevant memories to surface, when to forget, and how memories should interact over time.

## Comparison with Mem0/Zep

| | Engram | Mem0 | Zep |
|---|---|---|---|
| Retrieval model | ACT-R activation | Cosine similarity | Cosine + MMR |
| Forgetting | Ebbinghaus curves | Manual deletion | TTL-based |
| Consolidation | Memory Chain Model | None | None |
| Additional infra | None (SQLite) | Embedding API + Vector DB | Embedding API + Postgres |
| Associative links | Hebbian (automatic) | Manual graph | None |
| Core code | ~500 lines | ~5,000+ lines | ~10,000+ lines |

## Why I Built This

I run an AI agent that helps me daily. It kept forgetting important things, or worse, surfacing irrelevant old memories because they happened to be semantically similar. 

The existing solutions felt like engineering heuristics stacked on top of each other. I wanted something grounded in actual science — models that have been validated through decades of cognitive psychology research.

The total core is about 500 lines of Python. The math isn't complicated. The insight is connecting it to agent memory.

## Try It

```bash
pip install engramai
```

- GitHub: https://github.com/tonitangpotato/engramai
- PyPI: https://pypi.org/project/engramai/
- TypeScript: `npm install neuromemory-ai`
- MCP Server included for Claude/Cursor integration

Zero required dependencies. Optional embedding support. Works offline.

---

Happy to answer questions about the cognitive science models, implementation details, or how it compares to other approaches. The entire codebase is open source (AGPL-3.0).
