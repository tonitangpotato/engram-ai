# Engram AI 🧠

**Neuroscience-grounded memory for AI agents — ACT-R activation, Hebbian learning, cognitive consolidation**

[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.9+-blue)](https://www.python.org/)
[![PyPI](https://img.shields.io/pypi/v/engramai)](https://pypi.org/project/engramai/)

> Give your AI agent a brain that actually remembers, associates, and forgets like a human.

### Available in 3 runtimes:

| Runtime | Package | Use When |
|---------|---------|----------|
| 🐍 **Python** | [`engramai` on PyPI](https://pypi.org/project/engramai/) | Python agents, MCP server, Claude Desktop |
| 🦀 **Rust** | [`engramai` on crates.io](https://crates.io/crates/engramai) — [GitHub](https://github.com/tonitangpotato/engram-ai-rust) | Performance-critical, embedded, zero-dependency |
| 📦 **TypeScript** | [`engram-ts/`](engram-ts/) | Node.js agents, OpenClaw |
| 🔌 **OpenClaw Plugin** | [`openclaw-plugin/`](openclaw-plugin/) | **Drop-in replacement** for OpenClaw's default context engine |

---

## What is Engram?

Most AI agents use a dumb FIFO window for context — keep the last N messages, throw away the rest. Engram replaces this with a **cognitive memory system** based on how human brains actually work:

- 🧠 **ACT-R Activation** — Memories strengthen with use, decay with time (not a fixed window)
- 🔗 **Hebbian Learning** — "Neurons that fire together, wire together" — related memories auto-associate
- 💤 **Consolidation** — Periodic forgetting + strengthening cycles (like sleep)
- 🎯 **Working Memory** — Miller's Law 7±2 chunks, automatic topic-change detection
- 🌍 **Cross-Language Search** — Recall memories across 50+ languages
- 💰 **Zero Cost** — Heuristic capture + prompt caching = $0 additional spend

## 🔌 OpenClaw Context Engine Plugin

**The fastest way to use Engram** — replace OpenClaw's default FIFO context with cognitive scoring in one config line:

```yaml
# openclaw.yaml
plugins:
  slots:
    contextEngine: engram
  entries:
    engram:
      autoCapture: true
```

Instead of keeping the last N messages, Engram scores every message by **ACT-R activation × Hebbian association × content importance**, then packs the most cognitively relevant ones into the token budget. Long-term memories persist across sessions automatically.

| | Default (Legacy) | Engram |
|--|--------|--------|
| Strategy | Keep last N messages (FIFO) | Score by cognitive relevance |
| Long-term memory | None | ACT-R + Hebbian |
| Cross-session | Compaction summaries (LLM cost) | Persistent recall (zero cost) |
| Topic awareness | None | Working memory + topic detection |
| Auto-capture | None | Preferences, facts, decisions, corrections |
| Cost | LLM summarization tokens | Zero (heuristic) |

→ **[Full plugin docs](openclaw-plugin/README.md)** | **[Source](openclaw-plugin/src/)**

---

## 🏆 Battle-Tested in Production

Real numbers from a live AI agent running 24/7:

| Metric | Value |
|--------|-------|
| **Memories stored** | 3,846 |
| **Recalls served** | 230,103 |
| **Hebbian links formed** | 12,510 (automatic) |
| **Consolidation layers** | 320 working → 224 core → 3,302 archive |
| **Database size** | 48 MB |
| **Recall latency** | ~90ms |
| **Additional cost** | **$0** (prompt caching absorbs overhead) |

---

## Quick Start

### Python

```bash
pip install engramai
```

```python
from engram import Memory

memory = Memory("./my-agent.db")

# Store
memory.add("User prefers detailed explanations", type="relational", importance=0.8)
memory.add("Project deadline: Feb 10", type="factual")

# Recall (ACT-R activation + Hebbian association + semantic similarity)
results = memory.recall("user preferences", limit=5)

# Consolidate (Ebbinghaus forgetting + strengthening)
memory.consolidate(days=1.0)
```

### TypeScript

```typescript
import { Memory } from 'neuromemory-ai';

const memory = new Memory('./my-agent.db');

memory.add("User prefers TypeScript over JavaScript", { type: "preference", importance: 0.8 });

const results = memory.recall("user language preferences", { limit: 5 });
```

### Rust

```rust
use engramai::Memory;

let mut memory = Memory::new("./my-agent.db", None)?;

memory.add("User prefers Rust for systems programming", "preference", 0.8, None)?;

let results = memory.recall("user preferences", 5, None, None)?;
```

### MCP Server (Claude Desktop, etc.)

```bash
export ENGRAM_DB_PATH=./my-agent.db
python3 -m engram.mcp_server
```

---

## Core Concepts

### 🧠 ACT-R Activation

Every memory has an activation level that decays over time but strengthens with each access:

```
activation = base_level + spreading + importance_boost

base_level = ln(Σ tᵢ^(-d))    # frequency × recency decay
spreading  = Σ wⱼ × Sⱼᵢ       # context similarity
```

This means frequently-accessed, recent memories surface first — but old important memories can still be recalled if they're relevant to the current context.

### 🔗 Hebbian Learning

Memories that are recalled together form automatic associations:

```
ΔW = η × aᵢ × aⱼ    # co-activation strengthens links
```

Ask about "Docker" and "deployment" in the same conversation → they become linked. Next time you ask about deployment, Docker memories get boosted automatically.

### 💤 Consolidation

Like human sleep, periodic consolidation:
1. **Decays weak memories** (Ebbinghaus forgetting curve)
2. **Strengthens frequently-used ones** (move working → core → long-term)
3. **Prunes noise** (below-threshold memories removed)

### 🎯 Working Memory (TypeScript / OpenClaw Plugin)

Session-level state based on Miller's Law (7±2 chunks):
- Tracks what's "active" in the current conversation
- Detects topic changes → triggers full recall
- Continuous topic → reuses cached memories (70-80% fewer DB queries)

---

## 🧩 Memory Types

| Type | Use For | Example |
|------|---------|---------|
| `factual` | Facts and knowledge | "Project uses Python 3.12" |
| `episodic` | Events | "Shipped v2.0 on Jan 15" |
| `preference` | User preferences | "Prefers concise answers" |
| `procedural` | How-to knowledge | "Deploy: run tests first, then push" |
| `semantic` | Concepts and relationships | "Causal inference relates to Pearl" |
| `causal` | Cause-effect relationships | "Rate hikes → USD strengthens" |

---

## Architecture

```
Query
  ↓
┌─────────────────────────┐
│  Vector Search (semantic)│  ← optional embedding provider
│  FTS5 Search (lexical)   │  ← always available, zero-cost
│  Merge & Dedupe          │
└──────────┬──────────────┘
           ↓
┌─────────────────────────┐
│  ACT-R Activation        │  ← frequency × recency decay
│  Hebbian Spreading       │  ← association boost from linked memories
│  Importance Weighting    │  ← user-set priority
│  Confidence Scoring      │  ← metacognition layer
└──────────┬──────────────┘
           ↓
     Ranked Results
```

---

## Configuration

### Embedding Providers

Engram works **without any embedding provider** (FTS5 keyword search). Add one for cross-language semantic search:

| Provider | Cost | Setup | Best For |
|----------|------|-------|----------|
| **None / FTS5** | Free | Zero config | Simple agents, testing |
| **Sentence Transformers** | Free | `pip install "engramai[sentence-transformers]"` | Privacy, offline |
| **Ollama** | Free | Ollama + embedding model | Already using Ollama |
| **OpenAI** | ~$0.0001/query | `OPENAI_API_KEY` | Highest quality |

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_EMBEDDING` | `auto` | Provider: `auto`, `sentence-transformers`, `ollama`, `openai`, `none` |
| `ENGRAM_DB_PATH` | `./engram.db` | Database path |
| `ENGRAM_ST_MODEL` | `paraphrase-multilingual-MiniLM-L12-v2` | Sentence Transformers model |
| `OPENAI_API_KEY` | — | Required for OpenAI embeddings |

---

## CLI

```bash
neuromem add "User prefers dark mode" --type preference --importance 0.8
neuromem recall "user preferences"
neuromem stats
neuromem consolidate
neuromem forget --threshold 0.01
neuromem hebbian "dark mode"
```

---

## 📚 Documentation

- **[OpenClaw Plugin Guide](openclaw-plugin/README.md)** — Drop-in context engine replacement
- **[Integration Guide](INTEGRATION-GUIDE.md)** — Level 3 auto-recall/store implementation
- **[Performance Analysis](PERFORMANCE.md)** — Production metrics and optimization
- **[Embedding Configuration](engram/EMBEDDING-CONFIG.md)** — Provider setup and tuning
- **[Vision](VISION.md)** — Where Engram is heading

---

## Development

```bash
git clone https://github.com/tonitangpotato/engram-ai.git
cd engram-ai

# Python
pip install -e ".[dev,all]"
pytest

# TypeScript
cd engram-ts && npm install && npm test

# OpenClaw Plugin
cd openclaw-plugin && npm install && npm test
```

---

## Credits

Built on research from:

- **ACT-R** (Adaptive Control of Thought-Rational) — Anderson, Carnegie Mellon
- **Hebbian Learning** — Donald Hebb, 1949
- **Memory Consolidation** — Walker & Stickgold (sleep research)
- **Forgetting Curve** — Hermann Ebbinghaus, 1885
- **Working Memory** — Baddeley & Hitch, 1974
- **Miller's Law** — George Miller, 1956 ("The Magical Number Seven")

---

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE) for details. Commercial licensing available, see [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md).

**[GitHub](https://github.com/tonitangpotato/engram-ai)** · **[PyPI](https://pypi.org/project/engramai/)** · **[crates.io](https://crates.io/crates/engramai)** · **[Issues](https://github.com/tonitangpotato/engram-ai/issues)**
