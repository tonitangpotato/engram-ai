# Engram × OpenClaw Integration Guide 🧠⚡

Give your OpenClaw agent neuroscience-grounded memory — activation decay, Hebbian learning, consolidation, and reward-driven learning.

## What You Get

| Before (flat files) | After (Engram) |
|---------------------|----------------|
| `MEMORY.md` — static, no retrieval ranking | ACT-R activation ranks by recency × frequency × importance |
| Manual tagging | Hebbian learning auto-links related memories |
| Everything saved forever | Ebbinghaus forgetting + consolidation (working → core → archive) |
| No feedback loop | Reward learning — user feedback strengthens/suppresses memories |
| Search = Ctrl+F | Semantic search across 50+ languages |

## Quick Setup (5 minutes)

### 1. Install Engram

```bash
pip install engramai

# Optional: semantic search (recommended)
pip install "engramai[sentence-transformers]"
```

### 2. Install the OpenClaw Skill

Copy the `skill/` directory from this example into your OpenClaw skills folder:

```bash
cp -r skill/ ~/.openclaw/skills/engramai/
```

Or install from ClawhHub:
```bash
openclaw skills install engramai
```

### 3. Configure MCP Server

Add to your `openclaw.json` (or use `mcporter`):

```json
{
  "mcp": {
    "servers": {
      "engram": {
        "command": "python3",
        "args": ["-m", "engram.mcp_server"],
        "env": {
          "ENGRAM_DB_PATH": "~/.openclaw/agents/YOUR_AGENT/engram.db"
        }
      }
    }
  }
}
```

### 4. Update Your Agent Files

See the sample files in this directory for copy-paste templates:

- **[sample-AGENTS.md](sample-AGENTS.md)** — Agent config with Engram memory behavior
- **[sample-SOUL.md](sample-SOUL.md)** — Personality with memory-aware instructions
- **[sample-HEARTBEAT.md](sample-HEARTBEAT.md)** — Automated consolidation schedule
- **[sample-TOOLS.md](sample-TOOLS.md)** — Tool configuration notes

## Architecture

```
┌──────────────────────────────────────────────┐
│  Your OpenClaw Agent                         │
│                                              │
│  SOUL.md ──→ personality + memory guidance   │
│  AGENTS.md ─→ when to store/recall           │
│                                              │
│  ┌─────────────┐    ┌──────────────────────┐ │
│  │ MCP Server  │◄──►│ Engram Memory System │ │
│  │ (engram.*)  │    │  ┌─── working ───┐   │ │
│  │             │    │  │    ↓ consolidate   │ │
│  │ .store      │    │  ├─── core ──────┤   │ │
│  │ .recall     │    │  │    ↓ archive      │ │
│  │ .consolidate│    │  ├─── archive ───┤   │ │
│  │ .reward     │    │  └───────────────┘   │ │
│  │ .forget     │    │                      │ │
│  │ .stats      │    │  SQLite + FTS5       │ │
│  └─────────────┘    └──────────────────────┘ │
│                                              │
│  MEMORY.md ──→ still works for transparency  │
│  memory/*.md ─→ manual logs + daily notes    │
└──────────────────────────────────────────────┘
```

## Hybrid Memory (Recommended)

Engram doesn't replace file-based memory — it augments it:

| Use Engram for | Use files (MEMORY.md) for |
|----------------|---------------------------|
| Active retrieval (what's relevant NOW) | Audit trail (what happened) |
| Associative recall (related memories) | Manual review & editing |
| Preference tracking | Structured TODO lists |
| Cross-session context | Daily logs & notes |

**Best practice:** Store important info in BOTH Engram (for smart retrieval) and files (for transparency).

## Production Stats

Real numbers from an OpenClaw agent running Engram for 30+ days:

| Metric | Value |
|--------|-------|
| Total memories | 3,846 |
| Total recalls | 230,103 |
| Hebbian links | 12,510 |
| Memory layers | 320 working / 224 core / 3,302 archive |
| Database size | 48 MB |
| Avg recall latency | ~90ms |
| External API cost | $0 (FTS5 mode) |

## MCP Tools Reference

| Tool | When to Use |
|------|-------------|
| `engram.store` | Learn a preference, fact, or lesson |
| `engram.recall` | Before answering questions about history/context |
| `engram.consolidate` | Heartbeat maintenance (1-2x daily) |
| `engram.forget` | Prune weak memories (weekly) |
| `engram.reward` | User says "great!" or "that's wrong" |
| `engram.stats` | Check memory health |
| `engram.pin` | Protect critical memories from decay |

## Troubleshooting

### "MCP server not found"
Make sure `engramai` is installed in the Python that OpenClaw uses:
```bash
which python3  # Check which Python
python3 -c "import engram; print(engram.__version__)"
```

### Memories not consolidating
Check your HEARTBEAT.md includes the consolidate task. Run manually:
```bash
python3 -m engram.mcp_server  # Start server
mcporter call engram.consolidate  # Run consolidation
mcporter call engram.stats  # Check layer counts
```

### High memory count, low recall quality
Run forget to prune weak memories:
```bash
mcporter call engram.forget threshold=0.01
```

## Links

- [Engram GitHub](https://github.com/tonitangpotato/engram-ai)
- [OpenClaw](https://github.com/openclaw/openclaw)
- [Full Engram docs](https://github.com/tonitangpotato/engram-ai/blob/main/docs/USAGE.md)
- [ClawhHub Skills](https://clawhub.com)
