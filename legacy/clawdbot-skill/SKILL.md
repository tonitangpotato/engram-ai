---
name: neuromemory-ai
description: Neuroscience-grounded memory for AI agents. Add, recall, and manage memories with ACT-R activation, Hebbian learning, and cognitive consolidation.
homepage: https://github.com/tonitangpotato/neuromemory-ai
metadata: {"clawdbot":{"emoji":"🧠","requires":{"bins":["python3"],"packages":{"pip":["neuromemory-ai"]}}}}
---

# neuromemory-ai

Cognitive memory system implementing ACT-R activation, Memory Chain consolidation, Ebbinghaus forgetting, and Hebbian learning.

## Installation

```bash
pip install neuromemory-ai
```

## CLI Usage

The `neuromem` command is available after installation:

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

# Use specific database
neuromem --db ./custom.db add "Memory"
```

## Python API

```python
from engram import Memory
from engram.config import MemoryConfig

# Create/open memory database
mem = Memory("./agent.db")  # or use preset:
# mem = Memory("./agent.db", config=MemoryConfig.personal_assistant())

# Add memories with type and importance
mem.add("User prefers concise answers", type="relational", importance=0.8)
mem.add("Always check calendar before scheduling", type="procedural", importance=0.9)

# Recall (ranked by ACT-R activation)
results = mem.recall("user preferences", limit=5)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# User feedback shapes memory
mem.reward("Great answer!")  # strengthens recent memories
mem.reward("That's wrong")   # suppresses recent memories

# Daily maintenance
mem.consolidate()  # transfer working → core memory
mem.forget()       # prune weak memories
```

## Memory Types

- `factual` — Facts and knowledge
- `episodic` — Events and experiences  
- `relational` — Relationships and preferences
- `emotional` — Emotional moments
- `procedural` — How-to knowledge
- `opinion` — Beliefs and opinions

## Presets

```python
from engram.config import MemoryConfig

MemoryConfig.chatbot()           # High replay, slow decay
MemoryConfig.task_agent()        # Fast decay, procedural focus
MemoryConfig.personal_assistant() # Long-term, relationships matter
MemoryConfig.researcher()        # Never forget, archive everything
```

## Hebbian Learning

Memories recalled together automatically form associations:

```python
# No need to manually tag entities
mem.add("Python is great for ML")
mem.add("PyTorch is my favorite framework")
mem.add("TensorFlow has better production support")

# Query multiple times
for _ in range(3):
    mem.recall("machine learning", limit=3)

# Now querying "Python" will surface PyTorch/TensorFlow via Hebbian links
results = mem.recall("Python tools", graph_expand=True)
```

## MCP Server

For Claude/Cursor integration:

```bash
python -m engram.mcp_server --db ./agent.db
```

Tools: `store`, `recall`, `consolidate`, `forget`, `reward`, `stats`, `export`

## Key Features

| Feature | Description |
|---------|-------------|
| **ACT-R Activation** | Retrieval ranked by recency × frequency × context |
| **Memory Chain** | Dual-system consolidation (working → core) |
| **Ebbinghaus Forgetting** | Natural decay with spaced repetition |
| **Hebbian Learning** | "Neurons that fire together wire together" |
| **Confidence Scoring** | Metacognitive monitoring (certain/likely/uncertain/vague) |
| **Reward Learning** | User feedback shapes future memory |
| **Contradiction Detection** | Flags conflicting memories |
| **Zero Dependencies** | Pure Python stdlib + SQLite |

## Database Location

Default: `./neuromem.db`
Override with `NEUROMEM_DB` environment variable or `--db` flag.

## Links

- PyPI: https://pypi.org/project/neuromemory-ai/
- npm: https://www.npmjs.com/package/neuromemory-ai
- GitHub: https://github.com/tonitangpotato/neuromemory-ai
- Docs: See README.md in repository
