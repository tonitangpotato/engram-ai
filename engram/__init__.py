"""
Engram — Neuroscience-grounded memory for AI agents.

Usage:
    from engram import Memory

    mem = Memory("./agent.db")
    mem.add("the sky is blue", type="factual")
    results = mem.recall("sky color")
    mem.consolidate()
"""

from engram.memory import Memory
from engram.core import MemoryType, MemoryLayer, MemoryEntry, MemoryStore

__all__ = ["Memory", "MemoryType", "MemoryLayer", "MemoryEntry", "MemoryStore"]
__version__ = "0.1.0"
