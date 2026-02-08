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
        # Parse ENGRAM_EMBEDDING config (default: auto)
        embedding_config = os.environ.get("ENGRAM_EMBEDDING", "auto").lower()
        
        import traceback
        import logging
        debug_log = "/tmp/engram-mcp-debug.log"
        
        # Set up logging
        logging.basicConfig(
            level=logging.INFO,
            format="%(asctime)s [Engram] %(message)s",
            handlers=[logging.FileHandler(debug_log, mode='a')]
        )
        logger = logging.getLogger(__name__)
        
        embedding = None
        
        logger.info("=== Engram MCP Init ===")
        logger.info(f"Python: {sys.executable}")
        logger.info(f"ENGRAM_EMBEDDING: {embedding_config}")
        
        # Use auto-detection with fallback chain
        from engram.provider_detection import get_provider_with_fallback
        
        try:
            # Get provider (with auto-fallback if requested provider unavailable)
            provider, model, reason = get_provider_with_fallback(embedding_config)
            
            logger.info(f"Provider selection: {provider or 'FTS5'} (reason: {reason})")
            
            if provider is None:
                # FTS5-only mode
                logger.info("✅ Memory initialized (FTS5-only mode)")
                _mem = Memory(DB_PATH)
                return _mem
            
            # Initialize selected provider
            if provider == "sentence-transformers":
                from engram.embeddings import SentenceTransformerAdapter
                logger.info(f"✅ Loading Sentence Transformers: {model}")
                embedding = SentenceTransformerAdapter(model)
                
            elif provider == "ollama":
                from engram.embeddings import OllamaAdapter
                logger.info(f"✅ Connecting to Ollama: {model}")
                embedding = OllamaAdapter(model=model)
                
            elif provider == "openai":
                from engram.embeddings import OpenAIAdapter
                logger.info("✅ Initializing OpenAI embeddings")
                embedding = OpenAIAdapter()
            
            _mem = Memory(DB_PATH, embedding=embedding)
            logger.info(f"✅ Memory initialized with {provider}")
                
        except Exception as e:
            logger.error(f"❌ Error initializing {provider}: {type(e).__name__}: {e}")
            logger.error(traceback.format_exc())
            logger.warning("⚠️  Falling back to FTS5-only mode")
            _mem = Memory(DB_PATH)
    return _mem


@mcp.tool(name="store", description="Store a new memory in the Engram system")
def store_memory(
    content: str,
    type: str = "factual",
    importance: float | None = None,
    source: str = "",
    metadata: dict | None = None,
) -> dict:
    """Store a new memory. Types: factual, episodic, relational, emotional, procedural, opinion, causal.
    
    For causal memories, use metadata to provide structured cause/effect info:
    metadata={"cause": "...", "effect": "...", "confidence": 0.9, "domain": "code"}
    """
    mem = _get_mem()
    mid = mem.add(content, type=type, importance=importance, source=source, metadata=metadata)
    entry = mem._store.get(mid)
    return {
        "id": mid,
        "content": content,
        "type": type,
        "layer": entry.layer.value if entry else "working",
        "metadata": metadata,
    }


@mcp.tool(name="recall", description="Retrieve relevant memories using neuroscience-based activation retrieval")
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


@mcp.tool(name="recall_causal", description="Retrieve causal memories (cause→effect relationships)")
def recall_causal_memories(
    cause_query: str | None = None,
    limit: int = 10,
    min_confidence: float = 0.0,
) -> list[dict]:
    """Recall causal memories. Optionally filter by cause description.
    
    Causal memories are either manually stored (type=causal) or 
    auto-discovered by STDP during consolidation.
    """
    mem = _get_mem()
    results = mem.recall_causal(
        cause_query=cause_query,
        limit=limit,
        min_confidence=min_confidence,
    )
    return [
        {
            "id": r["id"],
            "content": r["content"],
            "confidence": r.get("confidence", 0),
            "strength": r.get("strength", 0),
            "metadata": r.get("metadata"),
        }
        for r in results
    ]


@mcp.tool(name="session_recall", description="Session-aware recall — only retrieves when topic changes (saves API calls)")
def session_recall(
    query: str,
    session_id: str = "default",
    limit: int = 5,
    types: list[str] | None = None,
    min_confidence: float = 0.0,
) -> dict:
    """
    Session-aware recall using cognitive working memory model.
    
    Instead of always doing expensive retrieval, this:
    1. Checks if the query topic overlaps with current working memory
    2. If yes (continuous topic) → returns cached working memory items
    3. If no (topic switch) → does full recall and updates working memory
    
    Based on Miller's Law (7±2 chunks) and Baddeley's Working Memory Model.
    Reduces API calls by 70-80% for continuous conversation topics.
    
    Args:
        query: Natural language query
        session_id: Unique session/conversation identifier
        limit: Maximum results for full recall
        types: Filter by memory types
        min_confidence: Minimum confidence threshold
        
    Returns:
        Dict with results and metadata about whether full recall was triggered.
    """
    from engram.session_wm import get_session_wm
    
    mem = _get_mem()
    session_wm = get_session_wm(session_id)
    
    # Track whether this triggers a full recall
    was_empty = session_wm.is_empty()
    needs_full = session_wm.needs_recall(query, mem) if not was_empty else True
    
    results = mem.session_recall(
        query,
        session_wm=session_wm,
        limit=limit,
        types=types,
        min_confidence=min_confidence,
    )
    
    return {
        "results": [
            {
                "id": r["id"],
                "content": r["content"],
                "type": r["type"],
                "confidence": r["confidence"],
                "confidence_label": r["confidence_label"],
                "strength": r["strength"],
                "age_days": r["age_days"],
                "from_working_memory": r.get("_from_wm", False),
            }
            for r in results
        ],
        "session_id": session_id,
        "full_recall_triggered": needs_full or was_empty,
        "working_memory_size": session_wm.size(),
        "reason": "empty_wm" if was_empty else ("topic_change" if needs_full else "topic_continuous"),
    }


@mcp.tool(name="session_status", description="Get session working memory status")
def session_status(session_id: str = "default") -> dict:
    """Get the current state of a session's working memory."""
    from engram.session_wm import get_session_wm
    
    session_wm = get_session_wm(session_id)
    mem = _get_mem()
    
    active_memories = session_wm.get_active_memories(mem)
    
    return {
        "session_id": session_id,
        "size": session_wm.size(),
        "capacity": session_wm.capacity,
        "decay_seconds": session_wm.decay_seconds,
        "active_memory_ids": session_wm.get_active_ids(),
        "active_memories": [
            {"id": m["id"], "content": m["content"][:100] + "..." if len(m["content"]) > 100 else m["content"]}
            for m in active_memories
        ],
    }


@mcp.tool(name="session_clear", description="Clear a session's working memory")
def session_clear(session_id: str = "default") -> dict:
    """Clear a session's working memory, forcing next recall to do full retrieval."""
    from engram.session_wm import clear_session, get_session_wm
    
    # Check if it existed
    session_wm = get_session_wm(session_id)
    size_before = session_wm.size()
    
    cleared = clear_session(session_id)
    
    return {
        "session_id": session_id,
        "cleared": cleared,
        "items_removed": size_before,
    }


@mcp.tool(name="session_list", description="List all active sessions")
def session_list() -> dict:
    """List all sessions with active working memory."""
    from engram.session_wm import list_sessions, get_session_wm
    
    sessions = list_sessions()
    
    return {
        "sessions": [
            {
                "session_id": sid,
                "size": get_session_wm(sid).size(),
            }
            for sid in sessions
        ],
        "total": len(sessions),
    }


@mcp.tool(name="consolidate", description="Run memory consolidation (sleep cycle) to strengthen and organize memories")
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


@mcp.tool(name="forget", description="Forget a specific memory or prune weak ones below threshold")
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


@mcp.tool(name="reward", description="Process feedback to adjust memory weights (dopaminergic reward signal)")
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


@mcp.tool(name="stats", description="Get memory system statistics")
def memory_stats() -> dict:
    """Return comprehensive memory system statistics."""
    return _get_mem().stats()


@mcp.tool(name="export", description="Export memory database to a file")
def export_memories(path: str) -> dict:
    """Export the memory database to the given path."""
    mem = _get_mem()
    mem.export(path)
    size = os.path.getsize(path) if os.path.exists(path) else 0
    return {
        "exported_to": path,
        "size_bytes": size,
    }


@mcp.tool(name="hebbian_links", description="Get Hebbian associations for a memory")
def hebbian_links(memory_id: str) -> dict:
    """Get memories linked via Hebbian learning (co-activation patterns)."""
    mem = _get_mem()
    from engram.hebbian import get_hebbian_neighbors
    neighbor_ids = get_hebbian_neighbors(mem._store, memory_id)
    # Get content for linked memories
    linked_memories = []
    for neighbor_id in neighbor_ids:
        entry = mem._store.get(neighbor_id)
        if entry:
            linked_memories.append({
                "id": neighbor_id,
                "content": entry.content[:100] + "..." if len(entry.content) > 100 else entry.content,
            })
    return {
        "source_id": memory_id,
        "links": linked_memories,
        "total_links": len(linked_memories),
    }


@mcp.tool(name="all_hebbian", description="Get all Hebbian links in the memory system")
def all_hebbian_links() -> dict:
    """Get all Hebbian associations formed through co-activation."""
    mem = _get_mem()
    from engram.hebbian import get_all_hebbian_links
    links = get_all_hebbian_links(mem._store)
    return {
        "total_links": len(links),
        "links": [
            {
                "source": source_id,
                "target": target_id,
                "strength": strength,
            }
            for source_id, target_id, strength in links[:50]  # Limit to 50 for response size
        ],
    }


@mcp.tool(name="pin", description="Pin a memory to prevent forgetting")
def pin_memory(memory_id: str) -> dict:
    """Pin a memory so it won't be forgotten during pruning."""
    mem = _get_mem()
    entry = mem._store.get(memory_id)
    if not entry:
        return {"error": f"Memory {memory_id} not found"}
    entry.pinned = True
    mem._store.update(entry)
    return {"pinned": True, "memory_id": memory_id}


@mcp.tool(name="unpin", description="Unpin a memory to allow normal forgetting")
def unpin_memory(memory_id: str) -> dict:
    """Unpin a memory to allow normal decay and forgetting."""
    mem = _get_mem()
    entry = mem._store.get(memory_id)
    if not entry:
        return {"error": f"Memory {memory_id} not found"}
    entry.pinned = False
    mem._store.update(entry)
    return {"pinned": False, "memory_id": memory_id}


@mcp.tool(name="embedding_status", description="Get current embedding provider status")
def embedding_status() -> dict:
    """Check which embedding provider is active and its configuration."""
    mem = _get_mem()
    
    if mem._embedding_adapter is None:
        return {
            "enabled": False,
            "provider": "none",
            "mode": "FTS5-only",
            "vector_count": 0,
            "config_env": os.environ.get("ENGRAM_EMBEDDING", "auto"),
            "auto_selected": False,
        }
    
    provider_name = type(mem._embedding_adapter).__name__
    vector_count = mem._vector_store.count() if mem._vector_store else 0
    
    # Get model info if available
    model_info = None
    if hasattr(mem._embedding_adapter, "model_name"):
        model_info = mem._embedding_adapter.model_name
    elif hasattr(mem._embedding_adapter, "model"):
        model_info = mem._embedding_adapter.model
    
    # Determine if this was auto-selected
    config_env = os.environ.get("ENGRAM_EMBEDDING", "auto")
    auto_selected = config_env.lower() == "auto"
    
    result = {
        "enabled": True,
        "provider": provider_name,
        "model": model_info,
        "vector_count": vector_count,
        "config_env": config_env,
        "auto_selected": auto_selected,
    }
    
    # Add detection info if auto-selected
    if auto_selected:
        from engram.provider_detection import detect_ollama, detect_sentence_transformers, detect_openai
        result["available_providers"] = {
            "ollama": detect_ollama(),
            "sentence_transformers": detect_sentence_transformers(),
            "openai": detect_openai(),
        }
    
    return result


@mcp.tool(name="get", description="Get a specific memory by ID")
def get_memory(memory_id: str) -> dict:
    """Get full details of a specific memory."""
    mem = _get_mem()
    entry = mem._store.get(memory_id)
    if not entry:
        return {"error": f"Memory {memory_id} not found"}
    return {
        "id": entry.id,
        "content": entry.content,
        "type": entry.memory_type.value,
        "layer": entry.layer.value,
        "importance": entry.importance,
        "strength": entry.strength,
        "stability": entry.stability,
        "access_count": entry.access_count,
        "created_at": entry.created_at.isoformat() if entry.created_at else None,
        "last_accessed": entry.last_accessed.isoformat() if entry.last_accessed else None,
        "pinned": entry.pinned,
        "source": entry.source,
        "tags": entry.tags,
    }


if __name__ == "__main__":
    mcp.run()
