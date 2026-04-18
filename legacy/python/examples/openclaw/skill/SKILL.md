---
name: engramai
description: Neuroscience-grounded memory for AI agents. Add, recall, and manage memories with ACT-R activation, Hebbian learning, and cognitive consolidation.
homepage: https://github.com/tonitangpotato/neuromemory-ai
metadata: {"clawdbot":{"emoji":"🧠","requires":{"bins":["python3"],"packages":{"pip":["engramai"]}}}}
---

# engramai 🧠

Cognitive memory system implementing ACT-R activation, Memory Chain consolidation, Ebbinghaus forgetting, and Hebbian learning.

## Installation

```bash
pip install engramai
```

## Quick Start

```python
from engram import Memory

mem = Memory("./agent.db")
mem.add("User prefers concise answers", type="relational", importance=0.8)
results = mem.recall("user preferences", limit=5)
mem.consolidate()  # Daily maintenance
```

## CLI Usage

```bash
# Add a memory
neuromem add "User prefers dark mode" --type preference --importance 0.8

# Recall memories
neuromem recall "user preferences"

# View statistics
neuromem stats

# Run consolidation (like sleep)
neuromem consolidate

# Prune weak memories
neuromem forget --threshold 0.01

# List memories
neuromem list --limit 20

# Show Hebbian links
neuromem hebbian "dark mode"
```

## AI Agent Integration (Important!)

For AI agents to use engram correctly, follow these patterns:

### When to Call What

| Trigger | Action | Example |
|---------|--------|---------|
| Learn user preference | `store(type="relational")` | "User prefers concise answers" |
| Learn important fact | `store(type="factual")` | "Project uses Python 3.12" |
| Learn how to do something | `store(type="procedural")` | "Deploy requires running tests first" |
| Question about history | `recall()` first, then answer | "What did I say about X?" |
| User satisfied | `reward("positive feedback")` | Strengthens recent memories |
| User unsatisfied | `reward("negative feedback")` | Suppresses recent memories |
| Daily maintenance | `consolidate()` + `forget()` | Run via cron or heartbeat |

### What to Store

**✅ Store:**
- User preferences and habits
- Important facts and decisions
- Lessons learned
- Procedural knowledge

**❌ Don't store:**
- Every conversation message (too noisy)
- Temporary information
- Publicly available facts
- Sensitive data (unless requested)

### Importance Guide

| Level | Use For |
|-------|---------|
| 0.9-1.0 | Critical info (API keys location, absolute preferences) |
| 0.7-0.8 | Important (code style, project structure) |
| 0.5-0.6 | Normal (general facts, experiences) |
| 0.3-0.4 | Low priority (casual chat, temp notes) |

### Hybrid Mode (Recommended)

Use engram alongside file-based memory:

- **engram**: Active memory — retrieval, associations, dynamic weighting
- **Files (memory/*.md)**: Logs — transparency, debugging, manual editing

### Heartbeat Maintenance

Add to your heartbeat or cron:

```markdown
## Memory Maintenance (Daily)
- [ ] engram.consolidate
- [ ] engram.forget --threshold 0.01
```

## Memory Types

- `factual` — Facts and knowledge
- `episodic` — Events and experiences  
- `relational` — Relationships and preferences
- `emotional` — Emotional moments
- `procedural` — How-to knowledge
- `opinion` — Beliefs and opinions

## MCP Server

For Claude/Cursor/Clawdbot integration:

```bash
python -m engram.mcp_server --db ./agent.db
```

**MCP Config (Clawdbot):**

```yaml
mcp:
  servers:
    engram:
      command: python3
      args: ["-m", "engram.mcp_server"]
      env:
        ENGRAM_DB_PATH: ~/.clawdbot/agents/main/memory.db
```

**Tools:** `engram.store`, `engram.recall`, `engram.consolidate`, `engram.forget`, `engram.reward`, `engram.stats`, `engram.export`

## Key Features

| Feature | Description |
|---------|-------------|
| **ACT-R Activation** | Retrieval ranked by recency × frequency × context |
| **Memory Chain** | Dual-system consolidation (working → core) |
| **Ebbinghaus Forgetting** | Natural decay with spaced repetition |
| **Hebbian Learning** | "Neurons that fire together wire together" |
| **Confidence Scoring** | Metacognitive monitoring |
| **Reward Learning** | User feedback shapes memory |
| **Zero Dependencies** | Pure Python stdlib + SQLite |

## Links

- PyPI: https://pypi.org/project/engramai/
- npm: https://www.npmjs.com/package/neuromemory-ai
- GitHub: https://github.com/tonitangpotato/neuromemory-ai
- Docs: https://github.com/tonitangpotato/neuromemory-ai/blob/main/docs/USAGE.md
