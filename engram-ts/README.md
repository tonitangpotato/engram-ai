# engram-ts

TypeScript port of [engram](https://github.com/tonitangpotato/neuromemory-ai), a neuroscience-grounded memory system for AI agents.

Uses the same cognitive models (ACT-R activation, Ebbinghaus forgetting, synaptic consolidation) as the Python version, with native TypeScript types and async-friendly SQLite storage.

## Install

```bash
npm install neuromemory-ai
```

**Note:** Uses `better-sqlite3` (native SQLite binding) — not zero-dependency like the Python version.

## Quick Start

```typescript
import { Memory } from 'engram';

const memory = new Memory({ dbPath: 'agent-memory.db' });

// Store a memory
await memory.store({
  content: 'The user prefers Python for scripting.',
  context: { source: 'conversation', timestamp: Date.now() },
  type: 'episodic',
  layer: 'working',
  importance: 0.8
});

// Retrieve relevant memories
const results = await memory.retrieve('What does the user prefer?', { limit: 5 });

// Memories decay over time — run consolidation periodically
await memory.consolidate();
```

## Documentation

See the [main engram repository](https://github.com/tonitangpotato/neuromemory-ai) for:
- Full API reference
- Memory model details (activation, forgetting, consolidation)
- Advanced usage (spreading activation, anomaly detection, reward signals)

## License

AGPL-3.0-or-later
