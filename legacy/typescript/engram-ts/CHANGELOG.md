# Changelog

## [1.0.0] - 2026-02-04

### 🎉 Major Release: Semantic Embedding Support

**Breaking Changes:** None (fully backward compatible)

### ✨ New Features

- **Semantic Embedding Support**
  - Multiple embedding providers: OpenAI, Ollama, MCP (Python server)
  - Auto-fallback chain: Ollama → MCP → OpenAI → FTS5
  - Zero-config deployment with automatic provider detection
  
- **Hybrid Search**
  - Combines vector similarity + FTS5 text search
  - Adaptive weight adjustment based on result overlap
  - Better cross-language and semantic recall
  
- **New Methods**
  - `addWithEmbedding()` - Add memory with automatic embedding generation
  - `recallWithEmbedding()` - Hybrid search with vector + text
  - `embeddingStatus()` - Check provider status and vector count

- **Vector Store**
  - SQLite-based vector storage (JSON serialization)
  - Cosine similarity search
  - Automatic schema migration

### 🔧 API Changes

- `Memory` constructor now accepts optional `embeddingConfig`
- All existing APIs remain compatible (no breaking changes)

### 📦 Exports

New exports added:
- `EmbeddingProvider`, `EmbeddingConfig`, `EmbeddingResult`
- `OpenAIEmbeddingProvider`, `OllamaEmbeddingProvider`, `MCPEmbeddingProvider`
- `detectProvider`, `getAvailableProviders`
- `vectorSearch`, `hybridSearch`, `adaptiveHybridSearch`

### 🐛 Bug Fixes

- Made `SQLiteStore.db` public for vector search access

### 📚 Documentation

- Added embedding configuration guide
- Updated examples for new features

---

## [0.3.1] - Previous Release

See previous CHANGELOG for details.
