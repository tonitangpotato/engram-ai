# Engram Integration Guide

> How to give your AI agent cognitive memory in 5 minutes.

---

## Quick Start

```bash
pip install engramai
engram init          # One-time setup (detects Ollama, configures DB)
```

---

## Choose Your Integration

### 1. CLI (Any bot, any language)

The simplest integration. Your bot calls `engram` as a subprocess.

```python
# After your bot generates a reply:
import subprocess

def store_memory(content, type="factual", importance=0.7):
    subprocess.run(["engram", "add", content, "--type", type, "-i", str(importance)])

def recall_memories(query, limit=5):
    result = subprocess.run(["engram", "recall", query, "-l", str(limit)], capture_output=True, text=True)
    return result.stdout
```

**Latency**: ~100ms per call (process startup). Fine for post-reply hooks, too slow for hot paths.

### 2. Python SDK (Zero overhead)

```python
from engram import Memory

mem = Memory("./agent.db", embedding="ollama")  # or embedding=None for FTS5-only

# Store
mem.add("User prefers concise answers", type="preference", importance=0.8)

# Recall
results = mem.recall("user preferences", limit=5)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# Maintenance (run periodically)
mem.consolidate()
```

### 3. Rust Crate (Native, zero IPC)

```toml
# Cargo.toml
[dependencies]
engramai = "1.2"  # after unified schema release
```

```rust
use engramai::{Memory, MemoryConfig, MemoryType};

let mut mem = Memory::new("./agent.db", None)?;
mem.add("Server is in eu-west-2", MemoryType::Factual, Some(0.8), None, None)?;

let results = mem.recall("server location", 5, None, None)?;
```

### 4. TypeScript/npm

```bash
npm install neuromemory-ai
```

```typescript
import { Memory } from 'neuromemory-ai';

const mem = new Memory('./agent.db');
await mem.add('User timezone is EST', { type: 'factual', importance: 0.7 });
const results = await mem.recall('timezone', { limit: 5 });
```

### 5. MCP Server (Claude Code, Cursor, Windsurf, any MCP client)

```bash
engram mcp          # Starts MCP server on stdio
```

Add to your MCP client config:
```json
{
  "mcpServers": {
    "engram": {
      "command": "engram",
      "args": ["mcp"],
      "env": {
        "ENGRAM_DB": "/path/to/agent.db"
      }
    }
  }
}
```

Available MCP tools:
- `engram_add` — Store a memory
- `engram_recall` — Recall relevant memories
- `engram_stats` — Memory statistics
- `engram_consolidate` — Run consolidation
- `engram_entity_add` — Add an entity (person, project, etc.)
- `engram_entity_link` — Create a relationship between entities
- `engram_entity_query` — Query entity relationships

---

## Bot Framework Integration

### The Pattern

Every bot framework has a "post-reply" hook. Use it to:
1. Let your LLM decide if this turn had important information
2. If yes, call engram to store it

### OpenClaw

In your `afterTurn()` context engine hook:

```typescript
async afterTurn(params) {
  const { messages, sessionId } = params;
  
  // Get the latest assistant + user messages
  const recent = messages.slice(-4);
  
  // Your LLM already processed these — use the assistant's judgment
  // Or: make a cheap LLM call to extract memories
  for (const msg of recent) {
    if (msg.role === 'assistant' && isImportant(msg.content)) {
      await engram.add(extractKeyInfo(msg.content), {
        type: 'factual',
        importance: 0.7,
        source: `session:${sessionId}`
      });
    }
  }
}
```

### RustClaw

Already built-in. Enable in config:

```toml
[memory]
auto_recall = true   # Inject relevant memories before each reply
auto_store = true     # Store important info after each reply
recall_limit = 5
engram_db = "./engram-memory.db"
```

RustClaw's `BeforeOutbound` hook automatically calls `engram.store()` with the assistant's reply.
The `BeforeInbound` hook calls `engram.recall()` and injects relevant memories into context.

### LangChain

```python
from langchain.callbacks.base import BaseCallbackHandler
from engram import Memory

class EngramCallback(BaseCallbackHandler):
    def __init__(self, db_path="./agent.db"):
        self.mem = Memory(db_path, embedding="ollama")
    
    def on_llm_end(self, response, **kwargs):
        # Store if the response contains important information
        text = response.generations[0][0].text
        if len(text) > 50:  # Simple heuristic, replace with LLM judgment
            self.mem.add(text, type="episodic", importance=0.5)
    
    def on_chain_start(self, serialized, inputs, **kwargs):
        # Recall relevant memories and inject into context
        if "input" in inputs:
            memories = self.mem.recall(inputs["input"], limit=5)
            if memories:
                context = "\n".join(f"- {m['content']}" for m in memories)
                inputs["context"] = f"[Relevant memories]:\n{context}"

# Usage
handler = EngramCallback()
chain = LLMChain(..., callbacks=[handler])
```

### Custom Bot (any framework)

```python
from engram import Memory

mem = Memory("./agent.db", embedding="ollama")

def handle_message(user_input):
    # 1. Recall before reply
    memories = mem.recall(user_input, limit=5)
    context = format_memories(memories)
    
    # 2. Generate reply (your LLM call)
    reply = llm.generate(user_input, context=context)
    
    # 3. Store after reply (let LLM judge importance)
    importance = llm.judge_importance(user_input, reply)
    if importance > 0.5:
        mem.add(
            llm.extract_key_info(user_input, reply),
            type="factual",
            importance=importance
        )
    
    return reply
```

---

## Best Practices

### What to Store
- **Decisions**: "We decided to use Rust for the backend"
- **Facts**: "The API endpoint is api.v2.example.com"
- **Preferences**: "User prefers concise answers"
- **Corrections**: "Actually, the server is in London, not Dublin"
- **Lessons**: "Latency arbitrage doesn't work because..."

### What NOT to Store
- Casual conversation ("nice", "ok", "thanks")
- Heartbeat/system messages
- Raw data dumps (store conclusions, not data)
- Sensitive credentials (use secrets manager instead)

### Memory Quality Rule
Before storing, ask: **"Would future-me find this useful without any other context?"**

Good: `"2026-03-21: CLOB API real latency is 33ms round-trip (2ms to Cloudflare edge, 31ms to London backend)"`
Bad: `"the latency is 33ms"`

### Consolidation Schedule
```python
# Run daily or every N conversations
mem.consolidate(days=1.0)  # Simulates 1 day of forgetting + strengthening
```

---

## Architecture

```
┌─────────────────────────────────────┐
│            Your Bot (LLM)           │
│                                     │
│  "What should I remember from this  │
│   conversation?"                    │
│                                     │
│  Decides: what to store, entities,  │
│  importance, type                   │
└──────────────┬──────────────────────┘
               │ SDK / CLI / MCP
               ▼
┌─────────────────────────────────────┐
│            Engram                    │
│                                     │
│  ┌─────────┐ ┌────────┐ ┌────────┐ │
│  │ ACT-R   │ │Hebbian │ │Embedd- │ │
│  │ Scoring │ │ Links  │ │ ings   │ │
│  └────┬────┘ └───┬────┘ └───┬────┘ │
│       └──────────┴───────────┘      │
│              SQLite DB              │
│         (the universal protocol)    │
└─────────────────────────────────────┘
```

**Engram is the memory. Your bot is the brain.**
