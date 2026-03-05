# Engram AI 🧠

**Neuroscience-grounded memory system for AI agents — semantic search, Hebbian learning, and cognitive consolidation**

[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.9+-blue)](https://www.python.org/)
[![PyPI](https://img.shields.io/pypi/v/engramai)](https://pypi.org/project/engramai/)

> Give your AI agent a brain that actually remembers, associates, and forgets like a human.

## What is Engram?

Engram is a production-ready memory system for AI agents, inspired by cognitive neuroscience. It provides:

- 🧠 **ACT-R Activation** — Memory recall based on frequency, recency, and importance
- 🔗 **Hebbian Learning** — Automatic association between co-activated memories
- 💤 **Consolidation** — Transfer memories from working → long-term storage
- 🌍 **Semantic Search** — Cross-language memory recall (50+ languages)
- 🔄 **Auto-Fallback** — Zero-config deployment with automatic provider detection

## 📚 Documentation

- **[Integration Guide](INTEGRATION-GUIDE.md)** — Level 3 auto-recall/store implementation
- **[Performance Analysis](PERFORMANCE.md)** — Real production metrics and optimization
- **[Embedding Configuration](engram/EMBEDDING-CONFIG.md)** — Provider setup and tuning

---

## Quick Start

### Installation

```bash
# Basic installation (FTS5 keyword search only)
pip install engramai

# With semantic search (recommended)
pip install "engramai[sentence-transformers]"

# With all embedding providers
pip install "engramai[all]"
```

### Basic Usage

```python
from engram import Memory

# Create memory system (auto-detects best embedding provider)
memory = Memory("./my-agent.db")

# Store memories
memory.add("User prefers detailed explanations", type="relational", importance=0.8)
memory.add("Project deadline: Feb 10", type="factual")

# Recall memories (semantic search)
results = memory.recall("user preferences", limit=5)
for r in results:
    print(f"{r['confidence']:.2f}: {r['content']}")

# Run consolidation (strengthens important memories)
memory.consolidate(days=1.0)
```

### MCP Server (for OpenClaw, Claude Desktop, etc.)

```bash
# Set database path
export ENGRAM_DB_PATH=./my-agent.db

# Start MCP server (auto-detects embedding provider)
python3 -m engram.mcp_server

# Or configure specific provider
export ENGRAM_EMBEDDING=sentence-transformers  # or ollama, openai, none, auto
python3 -m engram.mcp_server
```

---

## 🏆 Battle-Tested in Production

These aren't benchmarks — they're real numbers from a live AI agent running 24/7 in [OpenClaw](https://github.com/anthropics/openclaw):

| Metric | Value |
|--------|-------|
| **Memories stored** | 3,846 |
| **Recalls served** | 230,103 |
| **Hebbian links formed** | 12,510 (automatic) |
| **Max co-activations** | 91 |
| **Consolidation layers** | 320 working → 224 core → 3,302 archive |
| **Database size** | 48 MB |
| **Recall latency** | ~90ms |
| **Additional cost** | **$0** (prompt caching absorbs overhead) |
| **Continuous uptime** | 29.5 days |

**How is it $0?** Anthropic caches entire system prompts including injected memories. Memory injection adds ~250-500 tokens/turn in theory, but cache hits (87,726+ reads/session) make it free in practice. Infinite context, zero cost.

---

## Features

### 🎯 Zero-Config Deployment

Engram automatically detects and uses the best available embedding provider:

1. **Ollama** (if running locally with embedding models)
2. **Sentence Transformers** (if installed)
3. **OpenAI** (if API key configured)
4. **FTS5** (always available as fallback)

```bash
# Just install and go — no configuration needed!
pip install "engramai[sentence-transformers]"
python3 -m engram.mcp_server
```

### 🌍 Cross-Language Semantic Search

Find memories across languages with zero additional configuration:

```python
memory.add("marketing是个大难题")  # Chinese

# Query in English — finds the Chinese memory!
results = memory.recall("marketing is difficult")
# ✅ Returns: "marketing是个大难题"
```

Supports 50+ languages including English, Chinese, Spanish, French, German, Russian, Japanese, Korean, Arabic, Hindi, and many more.

### 🔬 Neuroscience-Grounded

Based on cognitive science models:

- **ACT-R** — Activation from frequency, recency, spreading activation
- **Hebbian Learning** — "Neurons that fire together, wire together"
- **Memory Consolidation** — Simulates sleep-based memory strengthening
- **Forgetting Curve** — Natural decay based on Ebbinghaus' research

### 📊 Session-Aware Working Memory

Reduces API calls by 70-80% for continuous conversations:

```python
# First query — full retrieval
results = memory.session_recall("user preferences", session_id="chat_123")

# Follow-up query on same topic — uses cached working memory!
results = memory.session_recall("what does user like?", session_id="chat_123")
# ⚡ No database query — instant response
```

---

## 🧩 Memory Types

| Type | Description | Example |
|------|-------------|---------|
| `factual` | Facts and knowledge | "Project uses Python 3.12" |
| `episodic` | Events and experiences | "Shipped v2.0 on Jan 15" |
| `relational` | Relationships and preferences | "User prefers concise answers" |
| `emotional` | Emotional moments | "User was frustrated with deploy" |
| `procedural` | How-to knowledge | "Deploy requires running tests first" |
| `opinion` | Beliefs and opinions | "User thinks React is better than Vue" |

---

## 🤖 AI Agent Best Practices

Building an AI agent with memory? Here's what we learned running Engram in production.

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

### What to Store vs. What to Skip

✅ **Store:** User preferences & habits, important facts & decisions, lessons learned, procedural knowledge

❌ **Don't store:** Every single message (too noisy), temporary info ("remind me in 5 min"), publicly available facts (Wikipedia-level stuff)

### Importance Guide

| Level | Use For |
|-------|---------|
| 0.9–1.0 | Critical — API keys location, absolute preferences |
| 0.7–0.8 | Important — code style, project structure |
| 0.5–0.6 | Normal — general facts, experiences |
| 0.3–0.4 | Low priority — casual chat, temporary notes |

### 🔀 Hybrid Memory Pattern

For production agents, we recommend pairing Engram with file-based memory:

| Layer | Purpose | Strengths |
|-------|---------|-----------|
| **Engram** | Active memory | Retrieval, associations, dynamic weighting, consolidation |
| **Files** (`memory/*.md`) | Memory logs | Transparency, debugging, manual editing, version control |

Engram handles the *thinking* — which memories matter, how they connect, when to forget. Files handle the *record* — what happened, in order, readable by humans. Use both.

---

## 💻 CLI Usage

Engram includes the `neuromem` CLI for quick operations:

```bash
# Add a memory
neuromem add "User prefers dark mode" --type preference --importance 0.8

# Recall memories
neuromem recall "user preferences"

# View database stats
neuromem stats

# Run consolidation (strengthen important memories)
neuromem consolidate

# Prune weak memories
neuromem forget --threshold 0.01

# List recent memories
neuromem list --limit 20

# Inspect Hebbian links for a concept
neuromem hebbian "dark mode"
```

---

## ⚙️ Configuration

### Environment Variables

| Variable | Values | Description |
|----------|--------|-------------|
| `ENGRAM_EMBEDDING` | `auto` (default), `sentence-transformers`, `ollama`, `openai`, `none` | Embedding provider |
| `ENGRAM_ST_MODEL` | Model name (default: `paraphrase-multilingual-MiniLM-L12-v2`) | Sentence Transformers model |
| `ENGRAM_OLLAMA_MODEL` | Model name (default: `nomic-embed-text`) | Ollama embedding model |
| `OPENAI_API_KEY` | API key | Required for OpenAI embeddings |
| `ENGRAM_DB_PATH` | File path | Database location (for MCP server) |

### Provider Comparison

| Provider | Pros | Cons | Use When |
|----------|------|------|----------|
| **Auto** ⭐ (default) | Zero config, adapts to environment | Non-deterministic selection | Production, distribution |
| **Sentence Transformers** | Free, offline, 50+ languages | ~118MB model download | Privacy-sensitive, no API costs |
| **Ollama** | Free, offline, customizable | Requires Ollama running | You already use Ollama |
| **OpenAI** | Highest quality | Costs API credits, needs internet | Prototyping, cloud-only |
| **None (FTS5)** | No dependencies, instant | Keyword-only, no semantic search | Testing, minimal setups |

---

## Examples

### Store Different Memory Types

```python
# Factual knowledge
memory.add("Paris is the capital of France", type="factual")

# Personal relationships
memory.add("User likes detailed technical explanations", type="relational", importance=0.9)

# Procedural knowledge (how-to)
memory.add("To deploy: git push origin main", type="procedural", importance=0.8)

# Episodic memories (events)
memory.add("Shipped feature X on Jan 15", type="episodic")
```

### Recall with Filters

```python
# Only relational memories
results = memory.recall("user preferences", types=["relational"], limit=3)

# High-confidence only
results = memory.recall("deadlines", min_confidence=0.7)

# Context-aware (spreading activation)
results = memory.recall("project status", context=["planning", "timeline"])
```

### Memory Consolidation

```python
# Simulate one day of sleep (strengthens important memories)
memory.consolidate(days=1.0)

# Prune weak memories below threshold
memory.forget(threshold=0.01)

# Apply reward/punishment
memory.reward("Great job!", recent_n=3)  # Strengthens last 3 memories
```

### Export/Import

```python
# Export to file
memory.export("backup.db")

# Import from file
from shutil import copyfile
copyfile("backup.db", "./my-agent.db")
memory = Memory("./my-agent.db")
```

---

## Architecture

```
User Query
    ↓
Vector Search (semantic)
    ↓
FTS5 Search (lexical)
    ↓
Merge & Dedupe
    ↓
ACT-R Activation (cognitive dynamics)
    ↓
Hebbian Spreading (association boost)
    ↓
Confidence Scoring (metacognition)
    ↓
Ranked Results
```

---

## ⚡ Performance

| Metric | Value | Notes |
|--------|-------|-------|
| Model size | 118MB | One-time download (Sentence Transformers) |
| Startup time | ~200ms | After first download |
| Vector generation | ~250 mem/sec | CPU (M2 chip) |
| Search latency | 10–50ms | 1,000 memories |
| Cross-language accuracy | 100% | Test cases: 3/3 ✅ |

---

## Development

```bash
# Clone repository
git clone https://github.com/tonitangpotato/engram-ai.git
cd engram-ai

# Install in development mode
pip install -e ".[dev,all]"

# Run tests
pytest

# Run provider detection test
python3 engram/provider_detection.py
```

---

## Integration

### With OpenClaw

Engram is the default memory system for OpenClaw agents. Just configure the MCP server and it works out of the box.

### With Claude Desktop

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "./my-agent.db"
      }
    }
  }
}
```

### Standalone Python

```python
from engram import Memory

memory = Memory("./agent.db")
memory.add("Remember this", importance=0.8)
results = memory.recall("what to remember?")
```

### Any MCP Client

Any MCP-compatible client can use Engram via the standard protocol.

---

## Credits

Engram is inspired by:

- **ACT-R** (Adaptive Control of Thought-Rational) — Carnegie Mellon
- **Hebbian Learning** — Donald Hebb
- **Memory Consolidation** — Sleep research by Walker, Stickgold
- **Forgetting Curve** — Hermann Ebbinghaus

---

## License

MIT License — see [LICENSE](LICENSE) for details.

## Support

- Issues: [github.com/tonitangpotato/engram-ai/issues](https://github.com/tonitangpotato/engram-ai/issues)
- Discussions: [github.com/tonitangpotato/engram-ai/discussions](https://github.com/tonitangpotato/engram-ai/discussions)
