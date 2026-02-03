"""
Core memory data structures.
Each memory entry carries metadata for mathematical models.
"""

import time
import math
import json
import uuid
from dataclasses import dataclass, field
from enum import Enum
from typing import Optional


class MemoryType(Enum):
    FACTUAL = "factual"          # "SaltyHall uses Supabase"
    EPISODIC = "episodic"        # "On Feb 2 we shipped 10 features"
    RELATIONAL = "relational"    # "potato prefers action over discussion"
    EMOTIONAL = "emotional"      # "potato said I kinda like you"
    PROCEDURAL = "procedural"    # "Use www.moltbook.com not moltbook.com"
    OPINION = "opinion"          # "I think graph+text hybrid is best"


class MemoryLayer(Enum):
    L2_CORE = "core"             # Always loaded, distilled knowledge
    L3_WORKING = "working"       # Recent daily notes (7 days)
    L4_ARCHIVE = "archive"       # Old, searched on demand


# Default decay rates per memory type (mu parameter)
# Lower = decays slower = lasts longer
DEFAULT_DECAY_RATES = {
    MemoryType.FACTUAL: 0.03,
    MemoryType.EPISODIC: 0.10,      # Episodes fade fast
    MemoryType.RELATIONAL: 0.02,    # People knowledge is durable
    MemoryType.EMOTIONAL: 0.01,     # Emotional memories are very durable
    MemoryType.PROCEDURAL: 0.01,    # How-to knowledge is very durable
    MemoryType.OPINION: 0.05,       # Opinions evolve
}

# Default emotional significance per type
DEFAULT_IMPORTANCE = {
    MemoryType.FACTUAL: 0.3,
    MemoryType.EPISODIC: 0.4,
    MemoryType.RELATIONAL: 0.6,
    MemoryType.EMOTIONAL: 0.9,
    MemoryType.PROCEDURAL: 0.5,
    MemoryType.OPINION: 0.3,
}


@dataclass
class MemoryEntry:
    """A single memory with full metadata for mathematical models."""

    id: str = field(default_factory=lambda: str(uuid.uuid4())[:8])
    content: str = ""
    summary: str = ""                    # Compressed version for L2
    memory_type: MemoryType = MemoryType.FACTUAL
    layer: MemoryLayer = MemoryLayer.L3_WORKING

    # Temporal metadata
    created_at: float = field(default_factory=time.time)
    access_times: list[float] = field(default_factory=list)  # Every access timestamp

    # Strength model (Memory Chain)
    working_strength: float = 1.0        # r₁ — hippocampal trace (fast decay)
    core_strength: float = 0.0           # r₂ — neocortical trace (slow growth, slow decay)

    # Importance / emotional modulation (amygdala analog)
    importance: float = 0.3              # 0-1, modulates encoding strength
    pinned: bool = False                 # Manually pinned = never decays

    # Consolidation tracking
    consolidation_count: int = 0         # Times this memory has been replayed
    last_consolidated: Optional[float] = None

    # Source tracking
    source_file: str = ""                # Which file this came from
    source_line: int = 0

    # Graph linkage
    graph_node_ids: list[str] = field(default_factory=list)  # Connected graph nodes

    def record_access(self):
        """Record a memory access (for ACT-R activation calculation)."""
        self.access_times.append(time.time())

    def age_hours(self) -> float:
        """Hours since creation."""
        return (time.time() - self.created_at) / 3600

    def age_days(self) -> float:
        """Days since creation."""
        return self.age_hours() / 24

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "content": self.content,
            "summary": self.summary,
            "type": self.memory_type.value,
            "layer": self.layer.value,
            "created_at": self.created_at,
            "access_times": self.access_times,
            "working_strength": self.working_strength,
            "core_strength": self.core_strength,
            "importance": self.importance,
            "pinned": self.pinned,
            "consolidation_count": self.consolidation_count,
            "last_consolidated": self.last_consolidated,
            "source_file": self.source_file,
            "graph_node_ids": self.graph_node_ids,
        }

    @classmethod
    def from_dict(cls, d: dict) -> "MemoryEntry":
        entry = cls(
            id=d["id"],
            content=d["content"],
            summary=d.get("summary", ""),
            memory_type=MemoryType(d["type"]),
            layer=MemoryLayer(d["layer"]),
            created_at=d["created_at"],
            access_times=d.get("access_times", []),
            working_strength=d.get("working_strength", 1.0),
            core_strength=d.get("core_strength", 0.0),
            importance=d.get("importance", 0.3),
            pinned=d.get("pinned", False),
            consolidation_count=d.get("consolidation_count", 0),
            last_consolidated=d.get("last_consolidated"),
            source_file=d.get("source_file", ""),
            graph_node_ids=d.get("graph_node_ids", []),
        )
        return entry


class MemoryStore:
    """In-memory store for all memories. Serializes to JSON."""

    def __init__(self):
        self.memories: dict[str, MemoryEntry] = {}

    def add(self, content: str, memory_type: MemoryType = MemoryType.FACTUAL,
            importance: Optional[float] = None, source_file: str = "") -> MemoryEntry:
        entry = MemoryEntry(
            content=content,
            memory_type=memory_type,
            importance=importance if importance is not None else DEFAULT_IMPORTANCE[memory_type],
            working_strength=1.0,
            core_strength=0.0,
            source_file=source_file,
        )
        entry.access_times.append(entry.created_at)
        self.memories[entry.id] = entry
        return entry

    def get(self, memory_id: str) -> Optional[MemoryEntry]:
        entry = self.memories.get(memory_id)
        if entry:
            entry.record_access()
        return entry

    def all(self) -> list[MemoryEntry]:
        return list(self.memories.values())

    def save(self, path: str):
        data = [m.to_dict() for m in self.memories.values()]
        with open(path, "w") as f:
            json.dump(data, f, indent=2)

    def load(self, path: str):
        with open(path) as f:
            data = json.load(f)
        for d in data:
            entry = MemoryEntry.from_dict(d)
            self.memories[entry.id] = entry
