"""
Engram — Neuroscience-grounded memory for AI agents.

Usage:
    from engram import Memory

    # Basic usage (FTS5 only)
    mem = Memory("./agent.db")
    mem.add("the sky is blue", type="factual")
    results = mem.recall("sky color")
    mem.consolidate()
    
    # With embeddings (recommended for semantic search)
    from engram.embeddings import OpenAIAdapter
    
    mem = Memory("./agent.db", embedding=OpenAIAdapter())
    # or simply:
    mem = Memory("./agent.db", embedding="openai")
"""

from engram.memory import Memory
from engram.config import MemoryConfig
from engram.core import MemoryType, MemoryLayer, MemoryEntry, MemoryStore
from engram.adaptive_tuning import AdaptiveTuner, AdaptiveMetrics
from engram.session_wm import SessionWorkingMemory, get_session_wm, clear_session, list_sessions
from engram.acl import AclManager, Permission, AclEntry
from engram.subscriptions import SubscriptionManager, Subscription, Notification
from engram.extractor import MemoryExtractor, AnthropicExtractor, OllamaExtractor, ExtractedFact
from engram.bus import (
    EmotionalBus,
    EmotionalAccumulator,
    BehaviorFeedback,
    Drive,
    EmotionalTrend,
    ActionStats,
    SoulUpdate,
    HeartbeatUpdate,
)

__all__ = [
    "Memory",
    "MemoryConfig",
    "MemoryType",
    "MemoryLayer",
    "MemoryEntry",
    "MemoryStore",
    "AdaptiveTuner",
    "AdaptiveMetrics",
    "SessionWorkingMemory",
    "get_session_wm",
    "clear_session",
    "list_sessions",
    # v2: Multi-agent & ACL
    "AclManager",
    "Permission",
    "AclEntry",
    "SubscriptionManager",
    "Subscription",
    "Notification",
    # v2.1: LLM Extraction
    "MemoryExtractor",
    "AnthropicExtractor",
    "OllamaExtractor",
    "ExtractedFact",
    # v2: Emotional Bus
    "EmotionalBus",
    "EmotionalAccumulator",
    "BehaviorFeedback",
    "Drive",
    "EmotionalTrend",
    "ActionStats",
    "SoulUpdate",
    "HeartbeatUpdate",
]
__version__ = "2.1.0"  # v2.1: LLM extraction, hybrid search, config hierarchy
