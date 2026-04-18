# Embedding Integration for Engram

## Problem Statement

Engram currently uses FTS5 (keyword matching) for memory recall, which has critical limitations:
- Cannot understand synonyms ("marketing" ≠ "推广")
- No cross-language support (English query won't find Chinese memory)
- Fails on fuzzy queries ("困难" vs "难题")
- Not aligned with "neuroscience-grounded" positioning (human memory is semantic, not keyword)

## Solution: Default Enable Semantic Embedding

Enable sentence embeddings by default for all Engram instances, with optional configuration for advanced users.

## Requirements

### Phase 1: Default Sentence Transformers (Priority: Critical)
**Goal**: Make Engram work with semantic search out-of-the-box

**Tasks**:
1. Modify `engram/mcp_server.py` to enable SentenceTransformerAdapter by default
2. Set default model: `all-MiniLM-L6-v2` (90MB, 384-dim, fast)
3. Generate vectors for existing memories on first startup with embedding
4. Test recall quality with cross-language and synonym queries

**Acceptance Criteria**:
- User installs Engram → embedding works automatically
- Query "marketing challenge" finds "推广很难" memory
- Query "困难" finds "难题" memory
- No breaking changes to existing API

**Model Choice**:
- all-MiniLM-L6-v2 (default)
  - Size: ~90MB
  - Speed: 5-10ms/query (CPU)
  - Quality: Good (85%+ recall accuracy)
  - Rationale: Best balance for product (small, fast, good enough)

### Phase 2: Configuration Support (Priority: High)
**Goal**: Allow advanced users to choose embedding provider

**Tasks**:
1. Add environment variable `ENGRAM_EMBEDDING` support
   - `sentence-transformers` (default)
   - `ollama` (for users with Ollama installed)
   - `openai` (for users with API key)
   - `none` (disable embedding, FTS5 only)
2. Document configuration in README
3. Add error handling for missing dependencies

**Acceptance Criteria**:
- `export ENGRAM_EMBEDDING=ollama` switches to Ollama
- `export ENGRAM_EMBEDDING=none` falls back to FTS5
- Clear error messages if provider unavailable

**Config Example**:
```bash
# Default (sentence-transformers)
python3 -m engram.mcp_server

# Use Ollama
export ENGRAM_EMBEDDING=ollama
python3 -m engram.mcp_server

# Disable embedding
export ENGRAM_EMBEDDING=none
python3 -m engram.mcp_server
```

### Phase 3: Auto-Fallback (Priority: Medium)
**Goal**: Graceful degradation when preferred provider unavailable

**Tasks**:
1. Implement provider detection logic
2. Auto-fallback chain: Ollama → Sentence Transformers → FTS5
3. Log provider selection
4. Add `engram.embedding_status` tool to check current provider

**Acceptance Criteria**:
- If Ollama configured but not running → fallback to Sentence Transformers
- If Sentence Transformers import fails → fallback to FTS5
- User can query `embedding_status` to see which provider is active

**Fallback Logic**:
```
1. Check user config (ENGRAM_EMBEDDING)
2. If "ollama" → try connect → fail? → fallback
3. If "sentence-transformers" → try import → fail? → fallback
4. If "openai" → check API key → missing? → fallback
5. Final fallback: FTS5 (always available)
```

## Technical Details

### Model Specifications

| Model | Dimensions | Size | Speed (CPU) | Quality |
|-------|-----------|------|-------------|---------|
| all-MiniLM-L6-v2 | 384 | 90MB | 5-10ms | Good |
| all-mpnet-base-v2 | 768 | 420MB | 20-30ms | Better |
| nomic-embed-text (Ollama) | 768 | 200MB | ~20ms | Best |

### Migration Strategy

**For existing Engram instances**:
1. First run with embedding enabled → detect missing vectors
2. Generate vectors for all memories in background
3. Progress: "Generating embeddings: 123/376 memories..."
4. Cache vectors in SQLite (no re-generation needed)

**SQL Schema**:
```sql
-- Already exists in vector_store.py
CREATE TABLE IF NOT EXISTS memory_vectors (
    memory_id TEXT PRIMARY KEY,
    vector BLOB NOT NULL,  -- Pickled numpy array
    model TEXT NOT NULL,
    created_at REAL NOT NULL
);
```

### Performance Impact

**Initial Setup** (one-time):
- 376 memories × 5ms = 1.9 seconds
- Acceptable for user experience

**Runtime**:
- Recall: +5-10ms per query
- Storage: +577KB for vectors (376 memories)
- Memory: +90MB for model (loaded once)

### Deployment

**PyPI Package**:
```python
# setup.py or pyproject.toml
dependencies = [
    "sentence-transformers>=2.2.0",  # Default embedding
    # ... other deps
]
```

**First Run**:
```
$ python3 -m engram.mcp_server
Initializing Engram memory system...
Loading embedding model (all-MiniLM-L6-v2)...
Model cached at: ~/.cache/huggingface/sentence-transformers/
Generating vectors for 376 memories... Done (1.9s)
Engram MCP server ready.
```

## Success Metrics

1. **Recall Quality**:
   - Cross-language queries work (English → Chinese memory)
   - Synonym matching improves (>90% accuracy)
   - Fuzzy queries succeed

2. **User Experience**:
   - 0 additional configuration for 90% of users
   - <3 seconds startup time (including model load)
   - <10ms query latency increase

3. **Adoption**:
   - Default setup works for all new users
   - Existing users can migrate with 1 command
   - No breaking changes to API

## Open Questions

1. Should we provide multiple model sizes?
   - Default: all-MiniLM-L6-v2 (90MB)
   - Option: all-mpnet-base-v2 (420MB, better quality)
   
2. How to handle model updates?
   - Auto-detect new model version?
   - Re-generate vectors on model change?

3. Support for custom models?
   - Let users provide HuggingFace model name?
   - `ENGRAM_MODEL=custom/my-model`

## References

- Sentence Transformers: https://www.sbert.net/
- Model Benchmark: https://www.sbert.net/docs/pretrained_models.html
- Ollama Embeddings: https://github.com/ollama/ollama/blob/main/docs/api.md#embeddings
