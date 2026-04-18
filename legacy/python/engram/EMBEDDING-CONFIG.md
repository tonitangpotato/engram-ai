# Embedding Configuration Guide

Engram supports multiple embedding providers for semantic memory search. By default, it **automatically detects and uses the best available provider** (Ollama → Sentence Transformers → OpenAI → FTS5-only).

## Quick Start

```bash
# Default: Auto-detect best available provider
python3 -m engram.mcp_server

# Explicitly use Sentence Transformers (with auto-fallback)
ENGRAM_EMBEDDING=sentence-transformers python3 -m engram.mcp_server

# Explicitly use Ollama (with auto-fallback if unavailable)
ENGRAM_EMBEDDING=ollama python3 -m engram.mcp_server

# Explicitly use OpenAI (with auto-fallback if no API key)
ENGRAM_EMBEDDING=openai python3 -m engram.mcp_server

# Disable embeddings completely (FTS5-only, no fallback)
ENGRAM_EMBEDDING=none python3 -m engram.mcp_server
```

## Automatic Fallback Chain

When `ENGRAM_EMBEDDING=auto` (default), Engram tries providers in order:

```
1. Ollama        → Check if running + has embedding models
   ↓ not available
2. Sentence Transformers → Check if pip package installed
   ↓ not available  
3. OpenAI        → Check if OPENAI_API_KEY set
   ↓ not available
4. FTS5-only     → Keyword search fallback (always works)
```

**Even with explicit provider selection**, Engram will auto-fallback if that provider is unavailable. For example:

```bash
ENGRAM_EMBEDDING=ollama python3 -m engram.mcp_server
# If Ollama not running → auto-falls back to Sentence Transformers
# If ST not installed → auto-falls back to OpenAI
# If no API key → falls back to FTS5-only
```

To **disable fallback** and require a specific provider, check provider status first:

```python
status = mcporter.call("engram.embedding_status")
if status["provider"] != "expected":
    raise Error("Required provider not available")
```

## Configuration Options

### Environment Variables

| Variable | Values | Description |
|----------|--------|-------------|
| `ENGRAM_EMBEDDING` | `auto` (default), `sentence-transformers`, `ollama`, `openai`, `none` | Embedding provider (with auto-fallback) |
| `ENGRAM_ST_MODEL` | Model name (default: `paraphrase-multilingual-MiniLM-L12-v2`) | Sentence Transformers model |
| `ENGRAM_OLLAMA_MODEL` | Model name (default: `nomic-embed-text`) | Ollama embedding model |
| `OPENAI_API_KEY` | API key | Required for OpenAI embeddings |

### Provider Comparison

| Provider | Pros | Cons | Use When |
|----------|------|------|----------|
| **Auto** ⭐ (default) | Zero config, adapts to environment, always works | Non-deterministic provider selection | Production deployments, distribution to users |
| **Sentence Transformers** | Free, offline, multilingual (50+ langs), fast (~250 mem/sec) | ~118MB model download, CPU-based | Explicit choice, privacy-sensitive, no API costs |
| **Ollama** | Free, offline, customizable models, very fast | Requires Ollama running, model download (varies) | You already run Ollama, need custom models |
| **OpenAI** | Highest quality, no local compute | Costs API credits, requires internet, slower | Prototyping, cloud-only environments |
| **None (FTS5-only)** | Minimal dependencies, instant startup | Keyword-only matching, no semantic search | Testing, minimal setups, embedded systems |

## Recommended Models

### Sentence Transformers

| Model | Size | Dimensions | Languages | Use Case |
|-------|------|------------|-----------|----------|
| `paraphrase-multilingual-MiniLM-L12-v2` ⭐ | 118MB | 384 | 50+ | **Default** - Best balance for multilingual |
| `all-MiniLM-L6-v2` | 90MB | 384 | English | English-only, smaller/faster |
| `paraphrase-multilingual-mpnet-base-v2` | 420MB | 768 | 50+ | Higher quality, slower |
| `multilingual-e5-large` | 560MB | 1024 | 100+ | Best quality, slowest |

### Ollama

```bash
# Pull embedding models
ollama pull nomic-embed-text      # 274MB, English
ollama pull mxbai-embed-large     # 670MB, multilingual
```

## Migration Guide

### Switching Providers

When you change embedding providers, you need to **regenerate vectors** for existing memories:

```bash
# 1. Clear old vectors
python3 /tmp/clear_vectors.py /path/to/engram.db

# 2. Set new provider
export ENGRAM_EMBEDDING=ollama

# 3. Regenerate vectors
cd /path/to/agent-memory-prototype
python3 migrate_vectors.py --db-path /path/to/engram.db
```

### Checking Status

```python
import mcporter
status = mcporter.call("engram.embedding_status")
print(status)
# Output:
# {
#   "enabled": true,
#   "provider": "SentenceTransformerAdapter",
#   "model": "paraphrase-multilingual-MiniLM-L12-v2",
#   "vector_count": 385,
#   "config_env": "sentence-transformers"
# }
```

## MCP Configuration Examples

### Claude Desktop / Clawdbot

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "./my-agent.db",
        "ENGRAM_EMBEDDING": "sentence-transformers",
        "ENGRAM_ST_MODEL": "paraphrase-multilingual-MiniLM-L12-v2"
      }
    }
  }
}
```

### With Ollama

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "./my-agent.db",
        "ENGRAM_EMBEDDING": "ollama",
        "ENGRAM_OLLAMA_MODEL": "nomic-embed-text"
      }
    }
  }
}
```

### FTS5-Only (No Embeddings)

```json
{
  "mcpServers": {
    "engram": {
      "command": "python3",
      "args": ["-m", "engram.mcp_server"],
      "env": {
        "ENGRAM_DB_PATH": "./my-agent.db",
        "ENGRAM_EMBEDDING": "none"
      }
    }
  }
}
```

## Performance Notes

- **First Run**: Model downloads may take 1-3 minutes depending on provider
- **Subsequent Runs**: Models are cached locally (~200ms startup)
- **Vector Generation**: ~250 mem/sec for Sentence Transformers on CPU
- **Search Latency**: 
  - Sentence Transformers: ~10-50ms for 1000 memories
  - Ollama: ~5-20ms (local inference)
  - OpenAI: ~100-500ms (network latency)

## Troubleshooting

### "ModuleNotFoundError: No module named 'sentence_transformers'"

```bash
pip install sentence-transformers
```

### "ConnectionError: [Errno 111] Connection refused" (Ollama)

Start Ollama first:
```bash
ollama serve
```

### "AuthenticationError: No API key provided" (OpenAI)

Set your API key:
```bash
export OPENAI_API_KEY=sk-...
```

### Poor cross-language recall

- Use `paraphrase-multilingual-*` models for Sentence Transformers
- Or switch to `ENGRAM_EMBEDDING=ollama` with `mxbai-embed-large`

## See Also

- [Sentence Transformers Model Hub](https://huggingface.co/sentence-transformers)
- [Ollama Embedding Models](https://ollama.com/library)
- [OpenAI Embeddings API](https://platform.openai.com/docs/guides/embeddings)
