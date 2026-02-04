# engram-ts

TypeScript port of [engram](https://github.com/tonitangpotato/neuromemory-ai), a neuroscience-grounded memory system for AI agents.

Uses the same cognitive models (ACT-R activation, Ebbinghaus forgetting, synaptic consolidation) as the Python version, with native TypeScript types and SQLite storage.

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

- 🧮 **ACT-R activation scoring** — retrieval ranked by recency × frequency × context
- 🔄 **Memory consolidation** — dual-system transfer from working to core memory
- 📉 **Ebbinghaus forgetting** — memories decay naturally with spaced repetition
- 🏷️ **6 memory types** — factual, episodic, relational, emotional, procedural, opinion
- 🎯 **Confidence scoring** — metacognitive monitoring
- 💊 **Reward learning** — positive/negative feedback shapes memory
- 🧠 **Hebbian learning** — automatic association from co-activation patterns
- 🧩 **Session Working Memory** — reduces recall API calls by 70-80%
- ⚙️ **Config presets** — tuned for chatbot, task-agent, personal-assistant, researcher

## Documentation

See the [main engram repository](https://github.com/tonitangpotato/neuromemory-ai) for:
- Full API reference
- Memory model details (activation, forgetting, consolidation)
- Advanced usage (spreading activation, anomaly detection, reward signals)

## License

AGPL-3.0-or-later
