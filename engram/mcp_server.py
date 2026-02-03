"""
Engram MCP Server — Expose the neuroscience-grounded memory system as MCP tools.

Usage:
    python3 -m engram.mcp_server

Configure DB path via ENGRAM_DB_PATH env var (default: ./engram.db).

Add to Claude Desktop / Clawdbot MCP config:
    {
      "mcpServers": {
        "engram": {
          "command": "python3",
          "args": ["-m", "engram.mcp_server"],
          "env": {"ENGRAM_DB_PATH": "./my-agent.db"}
        }
      }
    }
"""

import os
import sys

# Ensure parent dir is on path for imports (memory_core.py lives there)
_parent = os.path.join(os.path.dirname(__file__), "..")
sys.path.insert(0, _parent)
# activation.py, consolidation.py etc. live inside engram/ but are imported as bare modules
sys.path.insert(0, os.path.join(_parent, "engram"))

from mcp.server.fastmcp import FastMCP

from engram.memory import Memory

DB_PATH = os.environ.get("ENGRAM_DB_PATH", "./engram.db")

mcp = FastMCP("engram")

# Lazy singleton
_mem: Memory | None = None


def _get_mem() -> Memory:
    global _mem
    if _mem is None:
        _mem = Memory(DB_PATH)
    return _mem


@mcp.tool(name="engram.store", description="Store a new memory in the Engram system")
def store_memory(
    content: str,
    type: str = "factual",
    importance: float | None = None,
    source: str = "",
) -> dict:
    """Store a new memory. Types: factual, episodic, relational, emotional, procedural, opinion."""
    mem = _get_mem()
    mid = mem.add(content, type=type, importance=importance, source=source)
    entry = mem._store.get(mid)
    return {
        "id": mid,
        "content": content,
        "type": type,
        "layer": entry.layer.value if entry else "working",
    }


@mcp.tool(name="engram.recall", description="Retrieve relevant memories using neuroscience-based activation retrieval")
def recall_memories(
    query: str,
    limit: int = 5,
    types: list[str] | None = None,
    min_confidence: float = 0.0,
) -> list[dict]:
    """Recall memories matching a query. Returns ranked results with confidence scores."""
    mem = _get_mem()
    results = mem.recall(query, limit=limit, types=types, min_confidence=min_confidence)
    return [
        {
            "id": r["id"],
            "content": r["content"],
            "type": r["type"],
            "confidence": r["confidence"],
            "confidence_label": r["confidence_label"],
            "strength": r["strength"],
            "age_days": r["age_days"],
        }
        for r in results
    ]


@mcp.tool(name="engram.consolidate", description="Run memory consolidation (sleep cycle) to strengthen and organize memories")
def consolidate_memories(days: float = 1.0) -> dict:
    """Run consolidation. Call periodically to maintain memory health."""
    mem = _get_mem()
    mem.consolidate(days=days)
    stats = mem.stats()
    return {
        "consolidated": True,
        "stats": {
            "total_memories": stats["total_memories"],
            "layers": stats["layers"],
            "pinned": stats["pinned"],
        },
    }


@mcp.tool(name="engram.forget", description="Forget a specific memory or prune weak ones below threshold")
def forget_memory(memory_id: str | None = None, threshold: float = 0.01) -> dict:
    """Forget a memory by ID, or prune all weak memories below threshold."""
    mem = _get_mem()
    before = set(e.id for e in mem._store.all())
    mem.forget(memory_id=memory_id, threshold=threshold)
    after = set(e.id for e in mem._store.all())
    pruned = before - after
    return {
        "forgotten_count": len(pruned),
        "pruned_ids": list(pruned),
    }


@mcp.tool(name="engram.reward", description="Process feedback to adjust memory weights (dopaminergic reward signal)")
def reward_memories(feedback: str, recent_n: int = 3) -> dict:
    """Apply positive/negative feedback to recent memories."""
    mem = _get_mem()
    from engram.reward import detect_feedback
    polarity, conf = detect_feedback(feedback)
    mem.reward(feedback, recent_n=recent_n)
    return {
        "polarity": polarity,
        "confidence": round(conf, 3),
        "affected_memories": recent_n,
    }


@mcp.tool(name="engram.stats", description="Get memory system statistics")
def memory_stats() -> dict:
    """Return comprehensive memory system statistics."""
    return _get_mem().stats()


@mcp.tool(name="engram.export", description="Export memory database to a file")
def export_memories(path: str) -> dict:
    """Export the memory database to the given path."""
    mem = _get_mem()
    mem.export(path)
    size = os.path.getsize(path) if os.path.exists(path) else 0
    return {
        "exported_to": path,
        "size_bytes": size,
    }


if __name__ == "__main__":
    mcp.run()
