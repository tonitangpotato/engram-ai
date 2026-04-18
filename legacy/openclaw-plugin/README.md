# Engram Context Engine for OpenClaw

> Replace FIFO context windowing with neuroscience-grounded cognitive memory.

## What It Does

Instead of keeping the **last N messages** in context (dumb FIFO), Engram scores every message using:

1. **ACT-R Activation Decay** — memories strengthen with use, decay with time
2. **Hebbian Association** — co-activated memories form links (related context clusters together)
3. **Working Memory** — Miller's Law 7±2 chunks, automatic topic-change detection
4. **Ebbinghaus Consolidation** — periodic forgetting + strengthening cycles

The result: your agent remembers what matters, not just what's recent.

## Quick Start

```yaml
# openclaw.yaml
plugins:
  slots:
    contextEngine: engram
  entries:
    engram:
      autoCapture: true
```

## How It Works

### `assemble` — The Core

When OpenClaw asks "what context should the model see?", Engram:

1. Checks working memory — is the topic continuing or changed?
2. If changed: recalls relevant long-term memories via ACT-R + Hebbian scoring
3. Scores all messages: `importance × engram_activation × recency`
4. Greedily packs the highest-scored messages into the token budget
5. Re-sorts by time order (preserves conversation coherence)
6. Injects high-confidence long-term memories as `systemPromptAddition`

### `ingest` — Auto-Capture

Every message is analyzed for capture-worthy content:
- **Preferences**: "I like...", "Don't use...", "我喜欢..."
- **Facts**: "My name is...", "We deploy on..."
- **Decisions**: "Let's use...", "From now on..."
- **Corrections**: "Actually...", "No, it should be..."

Captured memories persist across sessions and are recalled by cognitive relevance.

### `compact` — Consolidation

Instead of summarizing and throwing away context, Engram runs a consolidation cycle:
- **Ebbinghaus forgetting**: weak memories decay naturally
- **Hebbian strengthening**: frequently co-activated memories strengthen their links
- **Layer migration**: important memories move from working → core → long-term

## Configuration

| Key | Default | Description |
|-----|---------|-------------|
| `dbPath` | `~/.openclaw/engram.db` | SQLite database path |
| `autoCapture` | `true` | Auto-detect and store important messages |
| `consolidateAfterTurns` | `20` | Run consolidation every N turns |
| `workingMemoryCapacity` | `7` | Max active memory chunks (Miller's Law) |
| `workingMemoryDecaySec` | `300` | WM item decay timeout (5 min) |
| `maxRecallResults` | `10` | Max memories to recall per query |
| `minConfidence` | `0.3` | Minimum recall confidence threshold |
| `embedding.provider` | `none` | Optional: `openai`, `ollama`, `mcp` for vector search |

## Architecture

```
OpenClaw Runtime
  └─ ContextEngine slot: "engram"
       └─ EngramContextEngine
            ├─ assemble() ← cognitive scoring (ACT-R + Hebbian + importance)
            ├─ ingest()   ← auto-capture (heuristic, zero LLM cost)
            ├─ compact()  ← consolidation (Ebbinghaus + Hebbian)
            └─ Memory (neuromemory-ai)
                 ├─ SQLite (FTS5 search + memory storage)
                 ├─ ACT-R model (activation decay)
                 ├─ Hebbian links (association network)
                 └─ Working Memory (session state)
```

## vs. Default (Legacy) Context Engine

| | Legacy | Engram |
|--|--------|--------|
| Strategy | Keep last N messages | Score by cognitive relevance |
| Long-term memory | None | ACT-R + Hebbian |
| Cross-session | Compaction summaries | Persistent recall |
| Topic awareness | None | Working memory + topic detection |
| Cost | LLM summarization | Zero (heuristic capture) |
| Dependencies | None | SQLite (embedded) |

## Credits

Built on [neuromemory-ai](https://github.com/tonitangpotato/engram-ai) — neuroscience-grounded memory for AI agents.

Based on research from ACT-R (Anderson), Hebbian Learning (Hebb), Ebbinghaus Forgetting Curve, Miller's Law, and Baddeley's Working Memory Model.
