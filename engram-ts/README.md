# engram-ts (neuromemory-ai)

TypeScript port of [engram](https://github.com/tonitangpotato/neuromemory-ai), a neuroscience-grounded memory system for AI agents.

Uses the same cognitive models (ACT-R activation, Ebbinghaus forgetting, synaptic consolidation) as the Python version, with native TypeScript types and SQLite storage.

## 🎉 v2.1.0 — New Features

### LLM Extraction (NEW in v2.1.0)
- **Auto-extract key facts** from conversations using Claude Haiku or Ollama
- **Config hierarchy**: Code > env vars > config file > no extractor (backward compatible)
- **Confidence labels**: `certain`, `likely`, `uncertain` (judged by LLM)

### Hybrid Search (NEW in v2.1.0)
- `recall()` now uses **triple scoring**: 15% FTS (exact terms) + 60% embedding (semantics) + 25% ACT-R (temporal)
- **No config needed** — automatically uses best available search strategy

### Other v2.1.0 Updates
- Renamed `recallCausal()` → `recallAssociated()` (old name deprecated but still works)
- Session WM capacity increased 7 → 15 (matches Rust version)

### v2.0.0 Features
- **Namespace isolation** — Multi-agent shared memory with namespaced store/recall
- **Emotional Bus** — Connects memory to agent workspace files (SOUL.md, HEARTBEAT.md, IDENTITY.md)
- **ACL (Access Control)** — Grant/revoke/check permissions for cross-agent access
- **Subscriptions** — Subscribe to namespaces, receive notifications on high-importance memories

See [README_V2_FEATURES.md](./README_V2_FEATURES.md) for full v2 documentation.

## Install

```bash
npm install neuromemory-ai
```

**Note:** Uses `better-sqlite3` (native SQLite binding) — not zero-dependency like the Python version.

## Quick Start

```typescript
import { Memory } from 'neuromemory-ai';

const memory = new Memory('agent-memory.db');

// Store memories
memory.add('The user prefers Python for scripting.', {
  type: 'relational',
  importance: 0.8
});

// Retrieve relevant memories (ranked by ACT-R activation)
const results = memory.recall('What does the user prefer?', { limit: 5 });

// Memories decay over time — run consolidation periodically
memory.consolidate();
```

### With LLM Extraction (v2.1.0)

```typescript
import { Memory, AnthropicExtractor } from 'neuromemory-ai';

const memory = new Memory('agent.db');

// Option 1: Auto-configure from environment
// Just set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN and it works automatically

// Option 2: Explicit setup
memory.setExtractor(new AnthropicExtractor({ apiKey: 'sk-ant-...' }));

// Now add() extracts facts automatically
memory.add("I love pizza but my girlfriend hates it");
// Stores two separate facts:
// - "User loves pizza"
// - "User's girlfriend hates pizza"

// Check if extractor is configured
if (memory.hasExtractor) {
  console.log('Using LLM extraction for smart memory storage');
}
```

## Session Working Memory

Reduce API calls by 70-80% with cognitive working memory:

```typescript
import { Memory, SessionWorkingMemory, getSessionWM } from 'neuromemory-ai';

const memory = new Memory('agent.db');

// Smart recall — only hits DB when topic changes
const result = memory.sessionRecall('coffee brewing', { sessionId: 'chat-123' });

// Returns:
// {
//   results: [...],
//   fullRecallTriggered: true/false,
//   workingMemorySize: 3,
//   reason: 'empty_wm' | 'topic_change' | 'topic_continuous'
// }
```

**How it works:**
- Maintains ~7 active memory chunks (Miller's Law: 7±2)
- Checks if new query overlaps with current working memory + Hebbian neighbors
- If ≥60% overlap → topic is continuous, reuse cached memories
- If <60% overlap → topic changed, do fresh recall

## Features

### Core (v1)
- 🧮 **ACT-R activation scoring** — retrieval ranked by recency × frequency × context
- 🔄 **Memory consolidation** — dual-system transfer from working to core memory
- 📉 **Ebbinghaus forgetting** — memories decay naturally with spaced repetition
- 🏷️ **6 memory types** — factual, episodic, relational, emotional, procedural, opinion
- 🎯 **Confidence scoring** — metacognitive monitoring
- 💊 **Reward learning** — positive/negative feedback shapes memory
- 🧠 **Hebbian learning** — automatic association from co-activation patterns
- 🧩 **Session Working Memory** — reduces recall API calls by 70-80%
- ⚙️ **Config presets** — tuned for chatbot, task-agent, personal-assistant, researcher

### Multi-Agent (v2)
- 🗂️ **Namespace isolation** — Separate memory spaces per agent/domain
- 🎭 **Emotional Bus** — Drive alignment, emotional tracking, SOUL/HEARTBEAT feedback loops
- 🔐 **Access Control Lists** — Fine-grained permissions (read/write/admin)
- 📬 **Subscriptions** — Real-time notifications on high-importance memories

## Documentation

See the [main engram repository](https://github.com/tonitangpotato/neuromemory-ai) for:
- Full API reference
- Memory model details (activation, forgetting, consolidation)
- Advanced usage (spreading activation, anomaly detection, reward signals)

## License

AGPL-3.0-or-later
