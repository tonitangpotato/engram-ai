# engram Integration Guide

> Turn any AI agent into one that remembers.

## What is engram?

engram is a **memory engine for AI agents**. Not a vector database — a cognitive memory system with ACT-R activation decay, Hebbian learning, consolidation, and synthesis. Memories that matter get stronger. Memories that don't, fade.

**One binary. One SQLite file. Works with any agent framework.**

## Architecture

```
┌─────────────────────────────────────────────────┐
│              Agent Framework                     │
│  (Claude Code / Cursor / Hermes / custom)        │
│                                                  │
│  ┌──────────────────────────────────────────┐    │
│  │  Integration Layer (pick one)            │    │
│  │                                          │    │
│  │  • CLI binary (engram — universal)       │    │
│  │  • Rust crate (engramai — native embed)  │    │
│  └──────────┬───────────────────────────────┘    │
└─────────────┼────────────────────────────────────┘
              │
   ┌──────────▼──────────┐
   │     engramai core   │
   │                     │
   │  • ACT-R activation │
   │  • Hebbian linking  │
   │  • Consolidation    │
   │  • Hybrid search    │
   │  • Synthesis        │
   │                     │
   │  SQLite + embeddings│
   └─────────────────────┘
```

Two integration paths:
- **CLI binary** — any agent that can exec shell commands. Universal.
- **Rust crate** — native embedding for Rust agent frameworks.

Both are the same codebase. The CLI is a thin wrapper over the crate.

---

## Install

```bash
cargo install engramai
```

This gives you the `engram` binary. That's the only dependency.

---

## CLI Reference

```bash
# Store a memory
engram --db ./memory.db store "User prefers dark mode" --type factual --importance 0.7

# Recall relevant memories (hybrid: FTS5 + embeddings + ACT-R)
engram --db ./memory.db recall "user preferences" --limit 5 --json

# Recent memories (chronological, no query needed)
engram --db ./memory.db recall-recent --limit 10 --json

# Associated/causal memories (Hebbian links)
engram --db ./memory.db recall-associated "deployment failed" --json

# Maintenance
engram --db ./memory.db consolidate          # strengthen + decay
engram --db ./memory.db sleep                # consolidation + synthesis (full cycle)

# Stats
engram --db ./memory.db stats --json
```

**Zero-config start:**
```bash
engram --db ./memory.db store "hello world"
# Database created. Defaults applied. No configuration needed.
```

Embeddings are optional. Without them, engram uses FTS5 full-text search + ACT-R activation scoring. Add Ollama or OpenAI for semantic recall:
```bash
engram --db ./memory.db --embedding ollama store "semantic content"
# Uses nomic-embed-text by default (768 dims, runs locally)
```

**Memory types:** `factual`, `episodic`, `procedural`, `relational`, `emotional`, `opinion`, `causal`

**JSON output** (`--json` flag on recall/stats/list):
```json
[
  {
    "id": "uuid",
    "content": "memory content",
    "memory_type": "factual",
    "importance": 0.7,
    "confidence": 0.85,
    "created_at": "2026-04-19T12:00:00Z",
    "access_count": 3
  }
]
```

**Full command list:** `store`, `recall`, `recall-recent`, `recall-associated`, `stats`, `consolidate`, `sleep`, `forget`, `list`, `get`, `update`, `pin`, `unpin`, `reward`, `export`, `synthesize`, `insights`, `entities`, `knowledge`, `bus`, `grant`/`revoke`, `subscribe`/`notifications`, `reindex`, `init`

---

## Framework Integration Recipes

### Claude Code (via Hooks)

Claude Code has a [hooks system](https://docs.anthropic.com/en/docs/claude-code/hooks) that runs shell commands at lifecycle events: `SessionStart`, `Stop`, `PreToolUse`, `PostToolUse`, etc. engram hooks run automatically — the agent doesn't need to "remember" to use it.

**1. Install:**
```bash
cargo install engramai
```

**2. Add hooks** to `.claude/settings.json` (project-level) or `~/.claude/settings.json` (global):

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "engram --db ~/.engram/memory.db consolidate 2>/dev/null || true",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

Available hook events for engram integration:
- `SessionStart` — recall context at session start
- `Stop` — consolidate at session end
- `PostToolUse` — auto-store after tool calls

**3. Add recall rules** to `CLAUDE.md` (Claude Code reads this file automatically):
```markdown
## Memory

You have persistent memory via engram.

At the start of every conversation:
\`\`\`bash
engram --db ~/.engram/memory.db recall "{user's first message}" --limit 5
\`\`\`

When you learn something important about the user or project:
\`\`\`bash
engram --db ~/.engram/memory.db store "what you learned" --type factual --importance 0.7
\`\`\`
```

**What this gives you:**
- Consolidation runs automatically at end of every session (hook)
- Agent recalls relevant context at conversation start (CLAUDE.md rule)
- Important learnings are stored explicitly

---

### Cursor (via Rules)

Cursor uses [Rules](https://docs.cursor.com/docs/rules) to customize agent behavior. Workspace rules live in `.cursor/rules/*.mdc` — one file per rule, with YAML frontmatter for metadata and activation.

**1. Install:**
```bash
cargo install engramai
```

**2. Create `.cursor/rules/memory.mdc`:**

> ⚠️ Cursor's rule format uses `.mdc` extension with YAML frontmatter. Verify the exact frontmatter schema in [Cursor docs](https://docs.cursor.com/docs/rules) — it may evolve.

```markdown
---
description: Persistent cognitive memory via engram
globs: *
alwaysApply: true
---

## Memory

You have persistent memory powered by engram. Use it to remember across sessions.

### Recall before answering
At the start of every conversation, recall relevant context:
\`\`\`bash
engram --db ~/.engram/memory.db recall "{topic}" --limit 5 --json
\`\`\`

### Store important learnings
When you discover something important about the project, user, or codebase:
\`\`\`bash
engram --db ~/.engram/memory.db store "what you learned" --type factual --importance 0.7
\`\`\`

### Memory types
- `factual` — facts, preferences, config details
- `episodic` — events, what happened, decisions made
- `procedural` — how-to, workflows, patterns
- `causal` — cause/effect relationships

### Maintenance (run occasionally)
\`\`\`bash
engram --db ~/.engram/memory.db consolidate
\`\`\`
```

Cursor also supports [Hooks](https://docs.cursor.com/docs/hooks) — check if they support `Stop`/`SessionEnd` events for auto-consolidation.

---

### Windsurf (via Rules)

Windsurf has [Memories & Rules](https://docs.windsurf.com/windsurf/cascade/memories). Rules are stored per-workspace in `.windsurf/rules/*.md` with YAML frontmatter, or globally at `~/.codeium/windsurf/memories/global_rules.md`.

Activation modes: `always_on`, `glob` (file-pattern match), `model_decision` (AI decides when to apply), `manual`.

Windsurf also reads `AGENTS.md` — root-level = always-on, subdirectory = auto-scoped to that directory.

**1. Install:**
```bash
cargo install engramai
```

**2. Create `.windsurf/rules/memory.md`:**

```markdown
---
trigger: always_on
---

## Memory

You have persistent memory powered by engram.

Recall before answering:
\`\`\`bash
engram --db ~/.engram/memory.db recall "{topic}" --limit 5 --json
\`\`\`

Store important learnings:
\`\`\`bash
engram --db ~/.engram/memory.db store "what you learned" --type factual --importance 0.7
\`\`\`

Run maintenance periodically:
\`\`\`bash
engram --db ~/.engram/memory.db consolidate
\`\`\`
```

**Alternative: `AGENTS.md`** — if you prefer zero-config, add memory instructions to `AGENTS.md` in your repo root. Windsurf treats it as an always-on rule automatically.

Windsurf also has [Cascade Hooks](https://docs.windsurf.com/windsurf/cascade/hooks) — check for session lifecycle events for auto-consolidation.

---

### Aider (via Conventions)

Aider reads [convention files](https://aider.chat/docs/usage/conventions.html) — markdown files loaded via `--read` flag or configured in `.aider.conf.yml`. Convention files are marked read-only and cached with prompt caching.

**1. Install:**
```bash
cargo install engramai
```

**2. Create `CONVENTIONS.md`:**
```markdown
## Memory

You have persistent memory via engram. Use shell commands to recall and store.

Before starting work on a task, recall relevant context:
\`\`\`bash
engram --db ~/.engram/memory.db recall "{task description}" --limit 5
\`\`\`

After completing work, store what you learned:
\`\`\`bash
engram --db ~/.engram/memory.db store "what changed and why" --type episodic --importance 0.6
\`\`\`

Store architectural decisions with high importance:
\`\`\`bash
engram --db ~/.engram/memory.db store "decision and reasoning" --type factual --importance 0.9
\`\`\`
```

**3. Configure auto-load** in `.aider.conf.yml`:
```yaml
read:
  - CONVENTIONS.md
```

Or load manually: `aider --read CONVENTIONS.md`

---

### Hermes Agent (via Skill + Tool)

Hermes Agent uses skills (`skill/<name>/SKILL.md`) and tools (`tools/*.py`). There's an existing integration at [hermes-engram](https://github.com/user/hermes-engram) that provides both.

**Current state:** The existing integration uses the **Python `engramai` package**. To migrate to the Rust CLI:

**1. Install the Rust binary:**
```bash
cargo install engramai
```

**2. Create `skill/engramai/SKILL.md`:**
```yaml
---
name: engramai
description: "Cognitive memory — ACT-R activation, Hebbian learning, Ebbinghaus forgetting."
version: 2.0.0
metadata:
  hermes:
    tags: [memory, cognitive-science]
    category: memory
    requires:
      bins: [engram]
---

# Engram — Cognitive Memory 🧠

## When to Use
- Before answering: recall relevant context
- After conversations: store important learnings
- Periodically: run consolidation

## Store
\`\`\`bash
engram --db ~/.hermes/engram.db store "CONTENT" --type TYPE --importance 0.7
\`\`\`
Types: `factual`, `episodic`, `procedural`, `causal`, `relational`

## Recall
\`\`\`bash
engram --db ~/.hermes/engram.db recall "QUERY" --limit 5 --json
\`\`\`

## Associated memories (Hebbian links)
\`\`\`bash
engram --db ~/.hermes/engram.db recall-associated "QUERY" --json
\`\`\`

## Consolidation (run daily)
\`\`\`bash
engram --db ~/.hermes/engram.db consolidate
\`\`\`
```

**3. Update the tool wrapper.** Replace the Python `engram_cli.py` script that calls `from engram import Memory` with subprocess calls to the Rust binary:

```python
#!/usr/bin/env python3
"""Engram CLI wrapper for Hermes Agent — uses Rust binary."""
import subprocess, json, os, sys

DB = os.environ.get("ENGRAM_DB", os.path.expanduser("~/.hermes/engram.db"))
BIN = "engram"

def run(*args):
    result = subprocess.run([BIN, "--db", DB] + list(args), capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else f"Error: {result.stderr}"

def main():
    if len(sys.argv) < 2:
        print("Usage: engram_cli.py <command> [args]")
        sys.exit(1)
    cmd = sys.argv[1]
    if cmd == "add":
        content = sys.argv[2]
        mtype = sys.argv[3] if len(sys.argv) > 3 else "factual"
        importance = sys.argv[4] if len(sys.argv) > 4 else "0.5"
        print(run("store", content, "--type", mtype, "--importance", importance))
    elif cmd == "recall":
        query = sys.argv[2] if len(sys.argv) > 2 else ""
        limit = sys.argv[3] if len(sys.argv) > 3 else "5"
        print(run("recall", query, "--limit", limit, "--json"))
    elif cmd == "consolidate":
        print(run("consolidate"))
    elif cmd == "stats":
        print(run("stats", "--json"))
    else:
        print(f"Unknown command: {cmd}")
        sys.exit(1)

if __name__ == "__main__":
    main()
```

**Migration note:** The existing `EngramMemoryStore` class in `tools/engram_memory_store.py` depends on the Python `engramai` package. To fully migrate, replace `from engram import Memory` calls with subprocess calls to the Rust binary. The SQLite DB format is compatible — your existing memories carry over.

---

### OpenClaw / Any Shell-Based Agent

If your agent can run shell commands, it can use engram. Add three rules to the system prompt or config:

```
## Memory Rules

1. Start of conversation: `engram --db $ENGRAM_DB recall "{context}" --limit 5 --json`
2. Learn something important: `engram --db $ENGRAM_DB store "content" --type factual --importance 0.7`
3. End of session: `engram --db $ENGRAM_DB consolidate`
```

That's the entire integration. Three commands.

---

### Python Agent Frameworks (LangChain, CrewAI, AutoGen)

For Python frameworks, wrap the CLI in subprocess calls. ~10 lines of glue code:

```python
import subprocess
import json

ENGRAM_DB = "~/.engram/memory.db"

def engram_store(content: str, memory_type: str = "factual", importance: float = 0.7):
    """Store a memory in engram."""
    subprocess.run([
        "engram", "--db", ENGRAM_DB,
        "store", content,
        "--type", memory_type,
        "--importance", str(importance),
    ], capture_output=True)

def engram_recall(query: str, limit: int = 5) -> list:
    """Recall memories from engram."""
    result = subprocess.run([
        "engram", "--db", ENGRAM_DB,
        "recall", query,
        "--limit", str(limit),
        "--json",
    ], capture_output=True, text=True)
    if result.returncode == 0:
        return json.loads(result.stdout)
    return []

def engram_consolidate():
    """Run memory consolidation."""
    subprocess.run(["engram", "--db", ENGRAM_DB, "consolidate"], capture_output=True)
```

#### LangChain Tool

```python
from langchain.tools import tool

@tool
def remember(content: str, importance: float = 0.7) -> str:
    """Store an important fact, preference, or learning for future recall."""
    engram_store(content, importance=importance)
    return f"Stored: {content}"

@tool
def recall(query: str) -> str:
    """Recall relevant memories about a topic."""
    memories = engram_recall(query)
    if not memories:
        return "No relevant memories found."
    return "\n".join(f"- [{m['memory_type']}] {m['content']}" for m in memories)
```

#### CrewAI Tool

```python
from crewai.tools import tool

@tool("Remember")
def remember(content: str) -> str:
    """Store something important for future sessions."""
    engram_store(content)
    return f"Memorized: {content}"

@tool("Recall")
def recall_memory(query: str) -> str:
    """Search cognitive memory for relevant past knowledge."""
    memories = engram_recall(query)
    return json.dumps(memories, indent=2) if memories else "Nothing found."
```

> ⚠️ **Note on Python framework examples:** These show the integration pattern. Verify the exact decorator syntax against the current version of each framework — `@tool` APIs evolve.

---

### Rust Agent Frameworks (Native Embed)

For Rust-native agents, embed the crate directly — no CLI overhead:

```toml
[dependencies]
engramai = "0.2.3"
```

```rust
use engramai::{Memory, MemoryConfig, MemoryType};

// Zero-config
let mem = Memory::new("./memory.db", None)?;

// Or with a preset
let config = MemoryConfig::chatbot();
let mem = Memory::new("./memory.db", Some(config))?;

// Store
mem.store("User prefers dark mode", MemoryType::Factual, 0.7, None)?;

// Recall (hybrid: FTS5 + embeddings + ACT-R scoring)
let results = mem.recall("user preferences", 5, None)?;
for r in &results {
    println!("[{:.2}] {}", r.confidence, r.content);
}

// Maintenance (run periodically)
mem.consolidate(None)?;
```

---

## Integration Pattern: The Three-Command Core

Every integration above follows the same pattern. Regardless of framework:

```
1. RECALL  — before answering, search for relevant memories
2. STORE   — after learning something important, save it
3. CONSOLIDATE — periodically, run maintenance
```

That's it. Everything else (Hebbian links, synthesis, decay) happens automatically inside engram. The agent doesn't need to understand neuroscience — it just needs these three commands.

---

## What Makes engram Different

| Feature | Vector DB | engram |
|---------|-----------|--------|
| Recall scoring | Cosine similarity | ACT-R: recency × frequency × relevance |
| Forgetting | Manual delete | Ebbinghaus decay (automatic) |
| Associations | Manual tags | Hebbian learning (automatic) |
| Consolidation | None | Strengthen important, decay weak |
| Synthesis | None | Discovers patterns across memories |
| Cold start | Needs embeddings | Works with FTS5 alone |
| Multi-agent | Separate DBs | Shared DB + namespace isolation |

---

## Deployment

### Embedding Provider

| Provider | Setup | Quality | Cost |
|----------|-------|---------|------|
| None | Nothing | Keyword-only | Free |
| Ollama | `ollama pull nomic-embed-text` | Good | Free (local) |
| OpenAI | `OPENAI_API_KEY` env | Best | ~$0.02/1M tokens |

### Database Location

- Per-project: `./.engram/memory.db`
- Per-user: `~/.engram/memory.db`
- Multi-agent shared: single DB + namespaces (`--ns agent1`, `--ns agent2`)

### Maintenance

```bash
# Every few hours or end-of-session
engram --db $DB consolidate

# Daily
engram --db $DB sleep              # Full cycle: consolidate + synthesize

# Weekly
engram --db $DB forget             # Prune truly weak memories
```

---

## Quick Start (30 seconds)

```bash
# Install
cargo install engramai

# Store
engram --db ~/memory.db store "My project uses React and TypeScript" --type factual
engram --db ~/memory.db store "Deploy to Vercel, not AWS" --type factual --importance 0.8
engram --db ~/memory.db store "User hates verbose error messages" --type opinion

# Recall
engram --db ~/memory.db recall "deployment"
# → "Deploy to Vercel, not AWS" (confidence: 0.92)

# That's it. Add these commands to your agent's workflow.
```
