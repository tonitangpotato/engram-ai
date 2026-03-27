# Changelog

## [2.0.0] - 2026-03-11

### Added - Multi-Agent Intelligence

**Namespace Isolation & ACL:**
- Namespace-based memory isolation for multi-agent systems
- Fine-grained access control (read/write/admin permissions)
- Cross-agent subscriptions and notifications
- CEO pattern support (supervisor monitors specialist agents)

**New modules:**
- `engram.acl` — Access Control Lists for namespace permissions
- `engram.subscriptions` — Pub/sub notifications for cross-agent coordination
- `engram.bus` — Emotional Bus for memory ↔ personality feedback loops

**Emotional Bus:**
- Drive alignment system (SOUL.md → memory importance boost)
- Emotional trend tracking (accumulates valence per domain)
- Behavior feedback logging (success/failure stats per action)
- Automatic SOUL/HEARTBEAT update suggestions
- Workspace file integration (SOUL.md, HEARTBEAT.md, IDENTITY.md)

**New Memory methods:**
- `Memory.with_emotional_bus()` — Create memory with emotional tracking
- `Memory.set_agent_id()` — Set agent identity for ACL
- `Memory.add_to_namespace()` — Store memory in specific namespace
- `Memory.add_with_emotion()` — Store with emotional valence tracking
- `Memory.recall_from_namespace()` — Namespace-aware retrieval
- `Memory.grant()` / `revoke()` — Manage permissions
- `Memory.subscribe()` / `check_notifications()` — Cross-agent coordination

**SQLiteStore enhancements:**
- `namespace` column in memories table
- `search_fts_ns()` — Namespace-filtered FTS search
- `all_in_namespace()` — Get all memories in a namespace
- `get_namespace()` — Get namespace for a memory

### Changed

- Version bumped to 2.0.0
- README updated with v2 feature documentation
- Added comprehensive v2 test suite (`tests/test_v2.py`)

### Backward Compatibility

- All v1 APIs remain unchanged
- Namespace defaults to "default" if not specified
- Existing databases auto-migrate with `namespace` column
- No breaking changes to existing code

## [1.1.0] - Previous

- Semantic search with embeddings (OpenAI, Ollama, Sentence Transformers)
- Auto-fallback from semantic to FTS when embeddings unavailable
- Hybrid search combining semantic + FTS + Hebbian
- Session working memory with topic detection
- Adaptive parameter tuning

## [1.0.0] - Previous

- Initial release with ACT-R activation
- Hebbian learning
- Memory consolidation (Memory Chain Model)
- FTS5 full-text search
- Cognitive models (forgetting, reward, downscaling)
