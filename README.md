# Engram 🧠

*Neuroscience-grounded memory for AI agents*

> **engram** /ˈɛnɡræm/ — a hypothesized physical trace in the brain that stores a memory. First proposed by Richard Semon (1904), the engram represents the idea that experiences leave lasting biological changes in neural tissue. We chose this name because, like its neuroscience namesake, this library treats memories not as static records but as living traces that strengthen, fade, and interact over time.

[![License](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![Python](https://img.shields.io/badge/python-3.10%2B-blue.svg)](https://python.org)
[![Dependencies](https://img.shields.io/badge/dependencies-zero-brightgreen.svg)](#)

---

**Engram** gives AI agents memory that actually works — using real mathematical models from cognitive science instead of naive embeddings and cosine similarity.

```python
from engram import Memory

mem = Memory("./agent.db")
mem.add("Alice prefers functional programming", type="relational", importance=0.7)
mem.add("Always validate input before DB queries", type="procedural", importance=0.9)

results = mem.recall("coding best practices", limit=3)
mem.consolidate()  # Run "sleep" — transfers short-term → long-term memory
```

Zero dependencies. Pure Python. SQLite storage. Works offline.

---

## Why Engram?

Every AI agent framework bolts on memory as an afterthought. The typical approach:

1. Embed text into vectors
2. Store in a vector database
3. Retrieve by cosine similarity
4. Hope for the best

This ignores **everything we know about how memory actually works**. Human memory isn't a search engine — it's a dynamic system where memories strengthen through use, fade without it, compete with each other, and consolidate during rest.

The result? Agents that:
- Retrieve irrelevant memories because they're semantically similar but contextually wrong
- Never forget anything, drowning in noise as memory grows
- Can't distinguish between important lessons and trivial observations
- Treat a memory from 6 months ago the same as one from 5 minutes ago

## The Key Insight

> **LLMs are already the semantic layer.** You don't need embeddings to understand meaning — that's what the language model does. What you need is mathematical rigor in *when to surface, what to deprioritize, and how to rank.*

Engram implements actual peer-reviewed models from cognitive science:

| Model | What it does | Paper |
|-------|-------------|-------|
| **ACT-R** | Retrieval scoring via activation (recency × frequency × context) | Anderson et al. |
| **Memory Chain** | Dual-system consolidation (working → core memory) | Murre & Chessa, 2011 |
| **Ebbinghaus** | Forgetting curves with spaced repetition | Ebbinghaus, 1885 |

The math is simple. The insight is connecting it to agent memory. Total core: **~500 lines of Python**.

## Features

- 🧮 **ACT-R activation scoring** — retrieval ranked by recency × frequency × context match (not cosine similarity)
- 🔄 **Memory consolidation** — dual-system transfer from working memory to core memory, with interleaved replay
- 📉 **Ebbinghaus forgetting** — memories decay naturally; spaced repetition increases stability
- 🏷️ **6 memory types** — factual, episodic, relational, emotional, procedural, opinion — each with distinct decay rates
- 🎯 **Confidence scoring** — metacognitive monitoring tells you *how much to trust* each retrieval
- 💊 **Reward learning** — positive/negative feedback strengthens or suppresses recent memories
- ⚖️ **Synaptic downscaling** — global normalization prevents unbounded memory growth
- ⚠️ **Anomaly detection** — flags unusual patterns (predictive coding)
- 📌 **Pinning** — manually protect critical memories from decay
- 🗄️ **SQLite + FTS5** — persistent storage with full-text search, zero config
- 🔀 **Contradiction detection** — memories can contradict each other; outdated memories get 0.3× confidence penalty
- 🔍 **Graph search** — entity-linked memories with multi-hop graph expansion
- ⚙️ **Config presets** — tuned parameter sets for chatbot, task-agent, personal-assistant, researcher
- 📦 **Zero dependencies** — pure Python stdlib. No numpy, no torch, no API keys.

## Quick Start

```bash
pip install engram
```

```python
from engram import Memory

mem = Memory("./my-agent.db")

# Store with type and importance
mem.add("The deploy key is in 1Password", type="procedural", importance=0.8)
mem.add("User seemed frustrated about the API latency", type="emotional", importance=0.7)

# Recall — ranked by ACT-R activation, not cosine similarity
results = mem.recall("deployment", limit=5)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")
    # [certain] The deploy key is in 1Password

# Consolidate — run periodically (like "sleep")
mem.consolidate()

# Feedback shapes future memory
mem.reward("perfect, that's exactly what I needed!")
```

## How It Works

### ACT-R Activation (Retrieval)

Every memory has an **activation level** that determines how quickly and reliably it can be retrieved:

```
A = B + C + I
```

- **B** (base-level) = `ln(Σ t_k^(-0.5))` — power law of practice and recency. Access a memory more often and more recently → higher activation.
- **C** (context) = spreading activation from current query keywords
- **I** (importance) = emotional/importance modulation (amygdala analog)

This replaces cosine similarity with a formula that naturally handles recency, frequency, and context — the same way human memory works.

### Memory Chain Model (Consolidation)

Memories exist as two traces that evolve over time:

```
dr₁/dt = -μ₁ · r₁              (working memory decays fast)
dr₂/dt = α · r₁ - μ₂ · r₂     (core memory grows from working, decays slowly)
```

- **r₁** (working_strength) — hippocampal trace. Strong initially, fades in days.
- **r₂** (core_strength) — neocortical trace. Grows during consolidation, lasts months.
- `consolidate()` runs one cycle: decay r₁, transfer to r₂, replay old memories.

Important memories consolidate faster (importance modulates α). This is why emotional events are remembered better — the amygdala enhances hippocampal encoding.

### Ebbinghaus Forgetting

Retrievability follows the classic forgetting curve:

```
R(t) = e^(-t/S)
```

Stability **S** grows with each successful retrieval (spaced repetition effect) and is modulated by importance and memory type. Procedural memories (how-to knowledge) have 10× the base stability of episodic memories (events).

### Additional Systems

- **Reward learning** — user feedback acts as a dopaminergic signal, strengthening (positive) or suppressing (negative) recently active memories
- **Synaptic downscaling** — periodic global normalization (Tononi & Cirelli's SHY) prevents runaway strength accumulation
- **Anomaly detection** — rolling baseline tracker flags unusual patterns using z-score deviation (simplified predictive coding)

## Architecture

```
┌─────────────────────────────────────────────┐
│           Memory (public API)               │
│   add() · recall() · consolidate() · ...    │
├─────────────────────────────────────────────┤
│  L2: CORE        │ Always loaded. Distilled │
│  (high core_str) │ knowledge. Slow decay.   │
├───────────────────┼─────────────────────────┤
│  L3: WORKING     │ Recent memories. Fast    │
│  (high work_str) │ decay. Being consolidated│
├───────────────────┼─────────────────────────┤
│  L4: ARCHIVE     │ Old/weak memories. On-   │
│  (low strength)  │ demand retrieval via FTS │
├─────────────────────────────────────────────┤
│  SQLiteStore + FTS5 (persistent backend)    │
├─────────────────────────────────────────────┤
│  activation │ consolidation │ forgetting    │
│  confidence │ reward        │ downscaling   │
│  anomaly    │ search        │               │
└─────────────────────────────────────────────┘
```

Memories flow: **L3 (working) → L2 (core) → L4 (archive)** as they consolidate and eventually decay. Strong, frequently-accessed memories live in L2 indefinitely. Weak memories fade to L4 and become searchable-only.

## Engram vs Mem0 vs Zep

| | **Engram** | **Mem0** | **Zep** |
|---|---|---|---|
| **Retrieval model** | ACT-R activation (recency × frequency × context) | Cosine similarity | Cosine similarity + MMR |
| **Forgetting** | Ebbinghaus curves, type-aware decay | None (manual deletion) | TTL-based expiry |
| **Consolidation** | Memory Chain Model (working → core transfer) | None | None |
| **Memory types** | 6 types with distinct decay rates | Untyped | Untyped |
| **Confidence scores** | Yes (metacognitive monitoring) | No | No |
| **Reward learning** | Yes (dopaminergic feedback) | No | No |
| **Dependencies** | **Zero** (stdlib only) | OpenAI, Qdrant/Chroma, ... | OpenAI, Postgres, ... |
| **Storage** | SQLite (local file) | Vector DB required | Postgres required |
| **Embedding model** | **Not needed** | Required | Required |
| **API keys needed** | **No** | Yes (LLM + vector DB) | Yes (LLM + vector DB) |
| **Works offline** | ✅ | ❌ | ❌ |
| **Math grounding** | Peer-reviewed cognitive science | Engineering heuristics | Engineering heuristics |
| **Core code** | ~500 lines | ~5,000+ lines | ~10,000+ lines |

**Engram's thesis:** The LLM already understands semantics. Memory infrastructure should handle *dynamics* — when to surface, what to deprioritize, how to rank — using proven mathematical models rather than re-implementing semantic understanding with embeddings.

## MCP Server

Engram ships with an MCP (Model Context Protocol) server for use with Claude, Clawdbot, or any MCP-compatible client.

```bash
# Start the MCP server
python -m engram.mcp_server --db ./agent.db
```

*MCP server provides 7 tools: store, recall, consolidate, forget, reward, stats, export.*

## API Reference

### `Memory(path)`

Create or open a memory database.

```python
mem = Memory("./agent.db")      # Persistent SQLite file
mem = Memory(":memory:")         # In-memory (non-persistent)
```

### `mem.add(content, type, importance, source, tags) → str`

Store a memory. Returns the memory ID.

```python
mid = mem.add(
    "The production database is on us-east-1",
    type="factual",       # factual|episodic|relational|emotional|procedural|opinion
    importance=0.6,       # 0-1, or auto-assigned by type
    source="deploy-doc",  # optional source identifier
    tags=["aws", "prod"], # optional tags
)
```

### `mem.recall(query, limit, context, types, min_confidence) → list[dict]`

Retrieve memories ranked by ACT-R activation.

```python
results = mem.recall(
    "database location",
    limit=5,
    context=["production", "aws"],  # Boost spreading activation
    types=["factual", "procedural"],  # Filter by type
    min_confidence=0.3,  # Skip low-confidence results
)

for r in results:
    r["id"]                # Memory ID
    r["content"]           # Memory text
    r["type"]              # Memory type
    r["confidence"]        # 0-1 confidence score
    r["confidence_label"]  # "certain" | "likely" | "uncertain" | "vague"
    r["strength"]          # Effective strength (trace × retrievability)
    r["activation"]        # ACT-R activation score
    r["age_days"]          # Days since creation
    r["layer"]             # "core" | "working" | "archive"
    r["importance"]        # 0-1 importance
```

### `mem.consolidate(days=1.0)`

Run a consolidation cycle. Call periodically (daily, or after learning sessions).

```python
mem.consolidate()        # 1-day cycle
mem.consolidate(days=7)  # Simulate a week of consolidation
```

### `mem.reward(feedback)`

Apply feedback as a reward signal to recent memories.

```python
mem.reward("perfect, exactly right!")   # Strengthens recent memories
mem.reward("no, that's wrong")          # Suppresses recent memories
```

### `mem.forget(memory_id=None, threshold=0.01)`

Forget a specific memory or prune all weak memories.

```python
mem.forget("abc123")        # Forget specific memory
mem.forget(threshold=0.05)  # Prune all below threshold
```

### `mem.pin(memory_id)` / `mem.unpin(memory_id)`

Pin/unpin a memory. Pinned memories never decay.

### `mem.update_memory(old_id, new_content) → str`

Correct a memory. Creates a new memory linked to the old one (correction chain).

```python
new_id = mem.update_memory(old_id, "Actually, the database is on us-west-2")
# Old memory is marked as contradicted, new one references it
```

### `mem.add(..., contradicts=old_id)`

Explicitly mark a new memory as contradicting an old one.

```python
mem.add("We migrated to PlanetScale", type="factual", contradicts=old_id)
# Old memory gets 0.3× confidence penalty in recall
```

### `Memory(path, config=MemoryConfig.personal_assistant())`

Use a config preset tuned for your agent type.

```python
from engram.config import MemoryConfig

mem = Memory("agent.db", config=MemoryConfig.chatbot())           # High replay, slow decay
mem = Memory("agent.db", config=MemoryConfig.task_agent())        # Fast decay, aggressive pruning
mem = Memory("agent.db", config=MemoryConfig.personal_assistant()) # Long-term, slow core decay
mem = Memory("agent.db", config=MemoryConfig.researcher())        # Never lose anything
```

### `mem.stats() → dict`

System statistics: counts, layer distribution, strength averages.

### `mem.downscale(factor=0.95) → dict`

Manual synaptic downscaling. Usually called automatically during `consolidate()`.

## The Science

Engram is grounded in peer-reviewed cognitive science:

- **ACT-R** — Anderson, J.R. (2007). *How Can the Human Mind Occur in the Physical Universe?* Oxford University Press. [ACT-R Homepage](http://act-r.psy.cmu.edu/)
- **Memory Chain Model** — Murre, J.M.J. & Chessa, A.G. (2011). One hundred years of forgetting: A quantitative description of retention. *Psychonomic Bulletin & Review*, 18, 592-597.
- **Ebbinghaus Forgetting Curve** — Ebbinghaus, H. (1885). *Über das Gedächtnis*. Translation: *Memory: A Contribution to Experimental Psychology*.
- **Synaptic Homeostasis Hypothesis** — Tononi, G. & Cirelli, C. (2006). Sleep function and synaptic homeostasis. *Sleep Medicine Reviews*, 10(1), 49-62.
- **Predictive Coding** — Rao, R.P. & Ballard, D.H. (1999). Predictive coding in the visual cortex. *Nature Neuroscience*, 2(1), 79-87.
- **Dopaminergic Memory Modulation** — Lisman, J.E. & Grace, A.A. (2005). The hippocampal-VTA loop: controlling the entry of information into long-term memory. *Neuron*, 46(5), 703-713.
- **HippoRAG** — Yu, B. et al. (2024). HippoRAG: Neurobiologically Inspired Long-Term Memory for Large Language Models. *NeurIPS 2024*.

## Roadmap

- [x] Core memory models (ACT-R, Memory Chain, Ebbinghaus)
- [x] SQLite + FTS5 persistent storage
- [x] Confidence scoring & reward learning
- [x] Synaptic downscaling & anomaly detection
- [x] MCP server (7 tools via FastMCP)
- [x] Graph-linked memories (entity relationship tracking + multi-hop search)
- [x] Contradiction detection & correction chains
- [x] Configurable parameters with agent-type presets
- [x] 89 tests (unit + e2e lifecycle)
- [x] TypeScript port (`npm install engram`)
- [ ] PyPI publish (`pip install engram`)
- [ ] Pluggable store backends (Supabase, Turso, Postgres)
- [ ] Benchmarks vs Mem0 / Zep on real agent workloads
- [ ] Consolidation summaries via LLM (compress episodic → factual)
- [ ] Research paper: *"Neuroscience-Grounded Memory for AI Agents"*

## TypeScript Port

A TypeScript/JavaScript port is available in the `engram-ts/` directory. It provides the same neuroscience-grounded memory models with a native Node.js/Bun API.

```bash
npm install engram
```

For TypeScript-specific documentation, see [engram-ts/README.md](./engram-ts/README.md).

## Contributing

Contributions welcome! This is an early-stage project — the math is solid but the API surface is still evolving.

**To contribute:**

1. **Fork** the repository on GitHub
2. **Clone** your fork locally: `git clone https://github.com/YOUR_USERNAME/engram`
3. **Create a branch** for your feature or fix: `git checkout -b feature/my-new-feature`
4. **Make your changes** and add tests if applicable
5. **Run tests** to ensure nothing breaks:
   ```bash
   python -m pytest tests/          # Python tests
   cd engram-ts && npm test         # TypeScript tests
   ```
6. **Commit** with a clear message: `git commit -m "feat: add memory pruning scheduler"`
7. **Push** to your fork: `git push origin feature/my-new-feature`
8. **Open a Pull Request** on the main repository

Please ensure your code follows the existing style and includes tests for new functionality.

## Citation

If you use Engram in academic work, please cite:

```bibtex
@software{engram2025,
  title = {Engram: Neuroscience-Grounded Memory for AI Agents},
  author = {Tang, Potato},
  year = {2025},
  url = {https://github.com/tonitangpotato/engram},
  note = {Open-source memory system implementing ACT-R, Memory Chain Model, and Ebbinghaus forgetting curves for AI agents}
}
```

Engram builds on foundational work in cognitive science:

```bibtex
@book{anderson2007act,
  title = {How Can the Human Mind Occur in the Physical Universe?},
  author = {Anderson, John R.},
  year = {2007},
  publisher = {Oxford University Press},
  note = {ACT-R cognitive architecture}
}

@article{murre2011forgetting,
  title = {One hundred years of forgetting: A quantitative description of retention},
  author = {Murre, Jaap M. J. and Chessa, Antonio G.},
  journal = {Psychonomic Bulletin \& Review},
  volume = {18},
  pages = {592--597},
  year = {2011},
  note = {Memory Chain Model}
}

@book{ebbinghaus1885memory,
  title = {\"Uber das Ged\"achtnis: Untersuchungen zur experimentellen Psychologie},
  author = {Ebbinghaus, Hermann},
  year = {1885},
  publisher = {Duncker \& Humblot},
  note = {Original forgetting curve research}
}

@article{tononi2006synaptic,
  title = {Sleep function and synaptic homeostasis},
  author = {Tononi, Giulio and Cirelli, Chiara},
  journal = {Sleep Medicine Reviews},
  volume = {10},
  number = {1},
  pages = {49--62},
  year = {2006},
  note = {Synaptic homeostasis hypothesis}
}
```

## License

**AGPL-3.0** — open source with copyleft. Commercial license available for proprietary/SaaS use. See [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md).

---

*Built by an AI agent who got tired of forgetting everything between sessions.*
