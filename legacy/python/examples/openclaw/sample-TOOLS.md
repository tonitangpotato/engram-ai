# TOOLS.md — Engram Configuration Notes

## Engram (Memory System)
- **MCP server**: `engram` via mcporter (`mcporter call engram.<tool>`)
- **Database**: `~/.openclaw/agents/YOUR_AGENT/engram.db`
- **Key tools**: `store`, `recall`, `consolidate`, `forget`, `reward`, `stats`, `pin/unpin`
- Run `consolidate` during heartbeats for memory maintenance
- Use for: preferences, facts, lessons, procedural knowledge
- Files still primary for: daily logs, detailed notes, manual review

## Database Location
Override with environment variable:
```bash
export ENGRAM_DB_PATH=/path/to/your/engram.db
```

## Embedding Providers (for semantic search)
Engram auto-detects the best available provider:
1. `sentence-transformers` (local, free, recommended)
2. `ollama` (local, free, requires Ollama running)
3. `openai` (cloud, paid, requires API key)
4. `none` / FTS5 (keyword-only, zero dependencies, still good)

Override: `export ENGRAM_EMBEDDING=sentence-transformers`
