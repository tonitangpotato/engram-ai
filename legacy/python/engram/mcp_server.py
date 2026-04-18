"""
Engram MCP Server — Model Context Protocol interface to the Engram memory system.

Exposes the full Engram API as MCP tools for use by AI agents and IDEs.

Usage:
    engram mcp              # Start MCP server (stdio transport)
    engram --db ./my.db mcp # Start with custom DB path
"""

import json
import time
from typing import Optional

from mcp.server.fastmcp import FastMCP

from engram import Memory
from engram.cli_config import resolve_db, resolve_embedding
from engram.forgetting import effective_strength


# ---------------------------------------------------------------------------
# Server setup
# ---------------------------------------------------------------------------

mcp = FastMCP(
    "engram",
    instructions=(
        "Engram is a neuroscience-grounded memory system for AI agents. "
        "Use engram_add to store memories, engram_recall to search, "
        "engram_consolidate to run maintenance, and engram_stats for overview."
    ),
)

# Lazy-initialized Memory singleton
_memory: Optional[Memory] = None


def _get_memory() -> Memory:
    """Get or create the Memory instance using config priority chain."""
    global _memory
    if _memory is None:
        db_path = resolve_db(None)
        embedding = resolve_embedding(None)
        _memory = Memory(db_path, embedding=embedding)
    return _memory


# ---------------------------------------------------------------------------
# Tools
# ---------------------------------------------------------------------------


@mcp.tool()
def engram_add(
    content: str,
    type: str = "factual",
    importance: float = None,
) -> str:
    """Add a memory to the engram store.

    Args:
        content: The memory content (natural language).
        type: Memory type — one of: factual, episodic, relational, emotional, procedural, opinion, causal.
        importance: 0-1 importance score. None = auto from type.

    Returns:
        JSON with the new memory ID.
    """
    mem = _get_memory()
    kwargs = {"type": type}
    if importance is not None:
        kwargs["importance"] = importance
    mem_id = mem.add(content, **kwargs)
    return json.dumps({"id": mem_id, "content": content[:120]})


@mcp.tool()
def engram_recall(query: str, limit: int = 5) -> str:
    """Recall memories matching a query using ACT-R activation-based retrieval.

    Args:
        query: Natural language search query.
        limit: Maximum number of results (default 5).

    Returns:
        JSON array of matching memories with confidence scores.
    """
    mem = _get_memory()
    results = mem.recall(query, limit=limit)
    return json.dumps(results, ensure_ascii=False)


@mcp.tool()
def engram_stats() -> str:
    """Show memory system statistics.

    Returns:
        JSON with total count, breakdown by type/layer, pinned count, uptime.
    """
    mem = _get_memory()
    stats = mem.stats()
    return json.dumps(stats, ensure_ascii=False)


@mcp.tool()
def engram_consolidate(days: float = 1.0) -> str:
    """Run a memory consolidation cycle (simulates sleep replay).

    Decays working strength, transfers to core strength, replays archived
    memories, and rebalances layers. Call periodically for maintenance.

    Args:
        days: Simulated time step in days (default 1.0).

    Returns:
        JSON confirmation with post-consolidation stats.
    """
    mem = _get_memory()
    mem.consolidate(days=days)
    stats = mem.stats()
    return json.dumps({
        "status": "ok",
        "days": days,
        "total_memories": stats["total_memories"],
        "layers": stats["layers"],
    })


@mcp.tool()
def engram_forget(threshold: float = 0.01) -> str:
    """Prune weak memories below a strength threshold.

    Memories aren't truly deleted — they become archived/inaccessible,
    mirroring natural forgetting.

    Args:
        threshold: Strength threshold (default 0.01). Memories below this are pruned.

    Returns:
        JSON with number of memories pruned.
    """
    mem = _get_memory()
    before = mem.stats()["total_memories"]
    mem.forget(threshold=threshold)
    after = mem.stats()["total_memories"]
    return json.dumps({
        "pruned": before - after,
        "remaining": after,
        "threshold": threshold,
    })


@mcp.tool()
def engram_update(id: str, new_content: str) -> str:
    """Update a memory's content. Creates a corrected version and marks the old one as contradicted.

    Args:
        id: ID of the memory to update.
        new_content: The corrected/updated content.

    Returns:
        JSON with the new memory ID.
    """
    mem = _get_memory()
    try:
        new_id = mem.update_memory(id, new_content)
        return json.dumps({"old_id": id, "new_id": new_id, "content": new_content[:120]})
    except ValueError as e:
        return json.dumps({"error": str(e)})


@mcp.tool()
def engram_pin(id: str) -> str:
    """Pin a memory so it won't decay or be pruned.

    Args:
        id: Memory ID to pin.

    Returns:
        JSON confirmation.
    """
    mem = _get_memory()
    entry = mem._store.get(id)
    if entry is None:
        return json.dumps({"error": f"Memory {id} not found"})
    mem.pin(id)
    return json.dumps({"id": id, "pinned": True})


@mcp.tool()
def engram_unpin(id: str) -> str:
    """Unpin a memory so it resumes normal decay.

    Args:
        id: Memory ID to unpin.

    Returns:
        JSON confirmation.
    """
    mem = _get_memory()
    entry = mem._store.get(id)
    if entry is None:
        return json.dumps({"error": f"Memory {id} not found"})
    mem.unpin(id)
    return json.dumps({"id": id, "pinned": False})


@mcp.tool()
def engram_info(id: str) -> str:
    """Get full details of a memory by ID.

    Args:
        id: Memory ID.

    Returns:
        JSON with all memory metadata including strengths, layer, importance, age, etc.
    """
    mem = _get_memory()
    entry = mem._store.get(id)
    if entry is None:
        return json.dumps({"error": f"Memory {id} not found"})

    now = time.time()
    strength = effective_strength(entry, now=now)
    from engram.confidence import confidence_score, confidence_label

    conf = confidence_score(entry)
    return json.dumps({
        "id": entry.id,
        "content": entry.content,
        "type": entry.memory_type.value,
        "layer": entry.layer.value,
        "importance": round(entry.importance, 3),
        "pinned": entry.pinned,
        "working_strength": round(entry.working_strength, 4),
        "core_strength": round(entry.core_strength, 4),
        "effective_strength": round(strength, 4),
        "confidence": round(conf, 3),
        "confidence_label": confidence_label(conf),
        "age_days": round(entry.age_days(), 2),
        "created_at": entry.created_at,
        "access_count": len(entry.access_times),
        "last_access": entry.access_times[-1] if entry.access_times else None,
        "consolidation_count": entry.consolidation_count,
        "source": entry.source_file,
        "contradicts": entry.contradicts or None,
        "contradicted_by": entry.contradicted_by or None,
        "metadata": entry.metadata,
    }, ensure_ascii=False)


@mcp.tool()
def engram_hebbian(query: str) -> str:
    """Show Hebbian links (co-activation associations) for a memory.

    Finds the best-matching memory for the query, then returns its
    Hebbian neighbors — memories frequently recalled together.

    Args:
        query: Search query to find the source memory.

    Returns:
        JSON with the source memory and its linked neighbors.
    """
    mem = _get_memory()
    results = mem.recall(query, limit=1)
    if not results:
        return json.dumps({"error": f"No memory found matching: {query}"})

    source = results[0]
    mem_id = source["id"]
    links = mem.hebbian_links(mem_id)

    neighbors = []
    for src_id, tgt_id, strength in links:
        entry = mem._store.get(tgt_id)
        if entry:
            neighbors.append({
                "id": tgt_id,
                "content": entry.content,
                "link_strength": round(strength, 4),
            })

    return json.dumps({
        "source": {"id": mem_id, "content": source["content"]},
        "links": neighbors,
    }, ensure_ascii=False)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def run_server():
    """Start the MCP server on stdio transport."""
    mcp.run(transport="stdio")


if __name__ == "__main__":
    run_server()
