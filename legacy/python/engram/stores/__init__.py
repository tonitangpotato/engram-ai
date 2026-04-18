"""
Pluggable store backends for neuromemory-ai.

- SQLiteStore (default, local)
- SupabaseStore (cloud, planned)
- CloudflareStore (edge, planned)
"""

from engram.store import SQLiteStore

__all__ = ["SQLiteStore"]
