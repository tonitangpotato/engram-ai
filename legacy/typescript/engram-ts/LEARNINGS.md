# Engram Learnings & Operational Notes

> Real-world lessons from running Engram in production (OpenClaw agent, daily use since 2026-02).
> These inform product improvements for external users.

---

## 2026-03-15: Major Memory Infrastructure Overhaul

### Problem: Memory System Was Underperforming

Despite having Engram installed and configured, the agent's memory quality was actually coming from a **40KB MEMORY.md file** injected into every session context — not from Engram recall. Engram was essentially unused for active recall.

**Root causes identified:**

1. **No active recall habit** — No instruction in agent config (AGENTS.md) told the agent to use `engram recall` before answering questions. MEMORY.md was passive (always in context), so the agent never needed to actively recall.

2. **CLI was using pure FTS5** — The Python CLI (`engram` command) was not connected to any embedding provider. It used keyword matching only, which returned irrelevant results (searching "causal engine" returned "GID workflow" and "potato preferences").

3. **MCP was too slow** — The MCP server path (`mcporter call engram.recall`) took ~5 seconds per call (Node.js startup + mcporter + MCP JSON-RPC + stdio). This discouraged active recall.

4. **Embedding was working but inaccessible** — The MCP server auto-detected `sentence-transformers` (all-MiniLM-L6-v2, 384d) and had semantic search. But since the agent used the faster CLI (no embedding), this capability was wasted.

5. **Store quality was poor** — Memories were stored as raw dialogue fragments and task notifications, not structured knowledge. No pre-processing or quality gate.

### Changes Made

#### 1. CLI Embedding Support
- Installed Ollama + `nomic-embed-text` (768d, ~500MB RAM)
- Created `/opt/homebrew/lib/python3.14/site-packages/engram/embeddings/ollama.py`
- Patched CLI's `get_memory()` to auto-use Ollama
- Result: CLI recall now has semantic search, 72ms-1s response time

#### 2. Backfilled All Existing Embeddings
- Re-embedded all 5,772 memories with Ollama nomic-embed-text (768d)
- Replaced old 384d sentence-transformer embeddings
- Script: iterate all memories, call Ollama `/api/embeddings`, INSERT OR REPLACE
- **TODO: Make `engram reindex` a built-in CLI command**

#### 3. Performance Comparison
| Method | Time | Embedding |
|--------|------|-----------|
| mcporter (MCP) | ~5,000ms | sentence-transformers 384d |
| Python CLI (before) | ~90ms | None (FTS5 only) |
| Python CLI (after) | ~1,000ms | Ollama nomic-embed-text 768d |
| Rust CLI | ~13ms | N/A (schema incompatible) |

#### 4. MEMORY.md Slimmed Down
- 39KB → 3.5KB (91% reduction)
- Archived learnings to `memory/archived-learnings.md`
- Forces agent to actively use Engram recall instead of relying on passive context injection

#### 5. Active Recall Rule Added to AGENTS.md
```
Before answering questions about history, preferences, project details,
past decisions, or learnings: run memory_search + engram.recall FIRST.
Don't rely only on what's already in context.
```

#### 6. Store Quality Rule Added
```
Before engram add, spend 1 second crafting the content.
Include: date, project name, key facts/numbers, decision made.
No raw dialogue, no "好的", no task notifications.
Write it like a knowledge card future-you can use directly.
```

---

## Discussion: Store Quality Approaches

We evaluated several approaches to improving what gets stored in Engram:

### Option A: LLM Pre-Processing Pipeline
- Add Haiku call before every `engram add` to distill content
- **Pro**: Automated, consistent quality
- **Con**: Adds ~100ms + cost per store, lossy compression (might discard future-useful context)

### Option B: Agent Self-Discipline (CHOSEN)
- Agent crafts quality content before storing, using its own judgment
- **Pro**: Zero cost, zero latency, agent has full context to decide what matters
- **Con**: Depends on habit formation, may drift over time
- **Mitigation**: Rule written into AGENTS.md, checked every session

### Option C: Store Raw, Clean Later
- Store everything as-is, periodically batch-clean with LLM
- **Pro**: No information loss at store time
- **Con**: Recall quality stays poor until cleanup runs, cleanup is complex

### Option D: Store Raw + Better Search
- Don't change store quality, just improve search (embedding upgrade)
- **Pro**: Simple, no store-time changes
- **Con**: "Garbage in, garbage out" — better search finds garbage faster

### Recommendation for Users
**Option B** (agent self-discipline) is best for single-agent setups. The agent already understands context — it just needs a rule to craft quality entries.

For **multi-agent or automated pipelines** where memories are stored programmatically, **Option A** (LLM pre-processing) may be necessary since there's no "agent judgment" in the loop.

---

## Discussion: Memory Architecture (Triple-Write)

### Current Architecture
```
Event/Learning occurs
    ├→ MEMORY.md (core project status, always in context)
    ├→ memory/YYYY-MM-DD.md (daily log, full detail, timeline)
    └→ engram add (structured knowledge card, semantic recall)
```

### Why Triple-Write?
- **MEMORY.md**: Safety net — always available even if Engram DB corrupts or recall misses
- **Daily logs**: Permanent human-readable backup with full context and timeline
- **Engram**: Fast semantic recall across all history

### Future: MEMORY.md Retirement?
- After Engram proves reliable over ~2 weeks of active use, MEMORY.md may be slimmed further
- Currently still accumulating learnings in MEMORY.md as insurance
- Goal: MEMORY.md becomes minimal identity/project list, all knowledge recall via Engram

---

## Product Implications (TODO for Engram)

### Must-Have
- [ ] `engram reindex` / `engram backfill` CLI command — re-embed all memories when switching providers
- [ ] CLI auto-detect embedding provider (like MCP server does) — don't require code patching
- [ ] Document the Ollama setup path clearly in README

### Should-Have
- [ ] Store quality gate — reject memories below N chars or matching noise patterns
- [ ] Deduplication — detect and merge memories with embedding similarity > 0.95
- [ ] Rust CLI schema migration — add namespace column to existing DBs without data loss

### Nice-to-Have
- [ ] Embedding provider config via env var or config file (not code changes)
- [ ] `engram doctor` command — check DB health, embedding coverage, provider status
- [ ] Batch import with LLM distillation for legacy data migration
