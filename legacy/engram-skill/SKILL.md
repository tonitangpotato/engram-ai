---
name: engram
description: Neuroscience-grounded memory system — store, recall, consolidate, and forget memories with cognitive science models.
homepage: https://github.com/potato/engram
metadata: {"clawdbot":{"emoji":"🧠","requires":{"bins":["python3"],"python":["engram"]}}}
---

# Engram Memory System

Persistent memory with neuroscience-backed retrieval, consolidation, and forgetting.
Replaces flat-file MEMORY.md with a searchable, decaying, self-maintaining memory store.

## When to Use

- **Storing facts, preferences, lessons** → `add` with appropriate type
- **Recalling context** → `recall` with natural language query
- **Periodic maintenance** → `consolidate` on heartbeat (1-2x/day)
- **Feedback loops** → `reward` after user confirms something was helpful/wrong
- **Auditing** → `stats` to see memory health

## Quick CLI

All commands via the bundled CLI (no MCP required):

```bash
# Store a memory
python3 engram-skill/scripts/engram-cli.py add "potato prefers Opus" --type relational --importance 0.7

# Recall memories
python3 engram-skill/scripts/engram-cli.py recall "potato preferences" --limit 5

# Run consolidation (sleep cycle)
python3 engram-skill/scripts/engram-cli.py consolidate

# View stats
python3 engram-skill/scripts/engram-cli.py stats

# Export database backup
python3 engram-skill/scripts/engram-cli.py export ./backup.db

# Pin/unpin a memory (pinned memories never decay)
python3 engram-skill/scripts/engram-cli.py pin <memory-id>
python3 engram-skill/scripts/engram-cli.py unpin <memory-id>

# Forget a specific memory or prune weak ones
python3 engram-skill/scripts/engram-cli.py forget --id <memory-id>
python3 engram-skill/scripts/engram-cli.py forget --prune --threshold 0.01

# Apply reward feedback
python3 engram-skill/scripts/engram-cli.py reward "great, that was correct!"
```

Default database: `./engram.db` (override with `--db /path/to/file.db`).

## Python API (Direct)

```python
from engram import Memory

mem = Memory("./agent.db")

# Store
mem.add("potato prefers Opus", type="relational", importance=0.7)
mem.add("Use www.moltbook.com not moltbook.com", type="procedural", importance=0.8)

# Recall — returns list of dicts with confidence scores
results = mem.recall("moltbook URL", limit=5)
for r in results:
    print(f"[{r['confidence_label']}] {r['content']}")

# Consolidate (run daily)
mem.consolidate(days=1.0)

# Stats
mem.stats()  # → dict with counts, layer breakdown, anomaly metrics
```

## Memory Types

| Type | Use For | Default Importance |
|------|---------|-------------------|
| `factual` | Facts, technical info | 0.5 |
| `episodic` | Events, conversations | 0.3 |
| `relational` | Preferences, relationships | 0.6 |
| `procedural` | How-to, workflows | 0.7 |
| `emotional` | Feelings, reactions | 0.8 |
| `opinion` | Beliefs, assessments | 0.4 |

Always pick the most specific type. When in doubt, use `factual`.

## Confidence Labels

Recall results include a confidence score (0–1) and human-readable label:

| Score Range | Label | Meaning |
|-------------|-------|---------|
| 0.8 – 1.0 | `certain` | Strong, well-consolidated, recently accessed |
| 0.6 – 0.8 | `confident` | Reliable, good strength |
| 0.4 – 0.6 | `moderate` | Usable but verify if critical |
| 0.2 – 0.4 | `uncertain` | Weak — may be outdated or decayed |
| 0.0 – 0.2 | `guess` | Very weak — treat as unreliable |

**Decision guide:**
- `certain`/`confident` → use directly
- `moderate` → use but mention uncertainty if stakes are high
- `uncertain`/`guess` → cross-reference or ask the user to confirm

## Consolidation Schedule

Run `consolidate` during heartbeats, ideally 1–2 times per day. This:
1. Decays hippocampal (working) traces
2. Strengthens neocortical (core) traces for important memories
3. Replays archived memories to prevent catastrophic forgetting
4. Runs synaptic downscaling (prevents unbounded growth)

**Recommended heartbeat integration:**
```
# In HEARTBEAT.md or heartbeat handler:
python3 engram-skill/scripts/engram-cli.py consolidate
```

## Migration from MEMORY.md

Import existing Clawdbot memory files into Engram:

```bash
python3 engram-skill/scripts/migrate-memory.py \
  --memory-dir /Users/potato/clawd \
  --output ./engram.db
```

This reads `MEMORY.md` and `memory/YYYY-MM-DD.md` files, classifies entries, and imports them.

## Architecture

Engram is backed by real cognitive science models:
- **ACT-R** activation for retrieval (frequency × recency power law)
- **Memory Chain Model** for consolidation (Murre & Chessa 2011)
- **Ebbinghaus** forgetting curves + interference
- **Synaptic Homeostasis** (Tononi & Cirelli) for downscaling
- **Dopaminergic reward** for feedback-driven learning

Memories flow through layers: `working` → `core` → `archive` (like hippocampus → neocortex).
