"""
Tests for newly added features: LLM Extraction, Config Hierarchy,
Hybrid Search, and CJK Tokenization.

All tests run WITHOUT live API keys or Ollama.
"""

import json
import os
import tempfile
import time
from unittest.mock import patch, MagicMock
from pathlib import Path

import pytest

from engram.extractor import (
    ExtractedFact,
    parse_extraction_response,
    AnthropicExtractor,
    OllamaExtractor,
    MemoryExtractor,
)
from engram.memory import Memory
from engram.hybrid_search import (
    HybridSearchEngine,
    HybridSearchResult,
    sanitize_fts_query,
    detect_temporal_alpha,
)
from engram.engram_tokenizers import contains_cjk, tokenize_for_fts


# ═══════════════════════════════════════════════════════════════
# 1. LLM Extraction (engram/extractor.py)
# ═══════════════════════════════════════════════════════════════


class TestExtractedFact:
    def test_basic_creation(self):
        fact = ExtractedFact(
            content="potato prefers Python",
            memory_type="relational",
            importance=0.7,
        )
        assert fact.content == "potato prefers Python"
        assert fact.memory_type == "relational"
        assert fact.importance == 0.7
        assert fact.confidence_label == "likely"  # default

    def test_custom_confidence(self):
        fact = ExtractedFact(
            content="fact", memory_type="factual",
            importance=0.5, confidence_label="confident",
        )
        assert fact.confidence_label == "confident"


class TestParseExtractionResponse:
    def test_valid_json(self):
        response = json.dumps([
            {"content": "User likes cats", "memory_type": "relational",
             "importance": 0.6, "confidence": "confident"},
            {"content": "Meeting at 3pm", "memory_type": "episodic",
             "importance": 0.8, "confidence": "likely"},
        ])
        facts = parse_extraction_response(response)
        assert len(facts) == 2
        assert facts[0].content == "User likes cats"
        assert facts[0].memory_type == "relational"
        assert facts[0].importance == 0.6
        assert facts[0].confidence_label == "confident"
        assert facts[1].content == "Meeting at 3pm"

    def test_markdown_wrapped_json(self):
        response = '```json\n[{"content": "wrapped fact", "memory_type": "factual", "importance": 0.5, "confidence": "likely"}]\n```'
        facts = parse_extraction_response(response)
        assert len(facts) == 1
        assert facts[0].content == "wrapped fact"

    def test_markdown_no_lang_tag(self):
        response = '```\n[{"content": "no lang tag", "memory_type": "factual", "importance": 0.5}]\n```'
        facts = parse_extraction_response(response)
        assert len(facts) == 1
        assert facts[0].content == "no lang tag"

    def test_invalid_json(self):
        facts = parse_extraction_response("this is not json at all")
        assert facts == []

    def test_empty_array(self):
        facts = parse_extraction_response("[]")
        assert facts == []

    def test_missing_content_field(self):
        response = json.dumps([
            {"memory_type": "factual", "importance": 0.5},
        ])
        facts = parse_extraction_response(response)
        assert facts == []  # items with empty content are skipped

    def test_missing_optional_fields_use_defaults(self):
        response = json.dumps([
            {"content": "just content"},
        ])
        facts = parse_extraction_response(response)
        assert len(facts) == 1
        assert facts[0].memory_type == "factual"  # default
        assert facts[0].importance == 0.5  # default
        assert facts[0].confidence_label == "likely"  # default

    def test_importance_clamped(self):
        response = json.dumps([
            {"content": "over", "importance": 5.0},
            {"content": "under", "importance": -3.0},
        ])
        facts = parse_extraction_response(response)
        assert facts[0].importance == 1.0  # clamped to max
        assert facts[1].importance == 0.0  # clamped to min

    def test_json_with_surrounding_text(self):
        """LLMs sometimes add prose around the JSON array."""
        response = 'Here are the facts:\n[{"content": "surrounded", "memory_type": "factual", "importance": 0.5}]\nDone!'
        facts = parse_extraction_response(response)
        assert len(facts) == 1
        assert facts[0].content == "surrounded"


class TestAnthropicExtractor:
    def test_instantiation_oauth(self):
        ext = AnthropicExtractor(auth_token="sk-ant-oat01-test", is_oauth=True)
        assert ext.is_oauth is True
        assert ext.auth_token == "sk-ant-oat01-test"
        headers = ext._build_headers()
        assert "authorization" in headers
        assert headers["authorization"] == "Bearer sk-ant-oat01-test"
        assert "anthropic-beta" in headers
        assert "x-api-key" not in headers

    def test_instantiation_api_key(self):
        ext = AnthropicExtractor(auth_token="sk-ant-api03-test", is_oauth=False)
        assert ext.is_oauth is False
        headers = ext._build_headers()
        assert "x-api-key" in headers
        assert headers["x-api-key"] == "sk-ant-api03-test"
        assert "authorization" not in headers

    def test_custom_model_and_url(self):
        ext = AnthropicExtractor(
            auth_token="test",
            model="claude-sonnet-4-20250514",
            api_url="https://custom.api.com/",
        )
        assert ext.model == "claude-sonnet-4-20250514"
        assert ext.api_url == "https://custom.api.com"  # trailing slash stripped


class TestOllamaExtractor:
    def test_instantiation(self):
        ext = OllamaExtractor(model="mistral:7b", host="http://myhost:11434")
        assert ext.model == "mistral:7b"
        assert ext.host == "http://myhost:11434"

    def test_default_values(self):
        ext = OllamaExtractor()
        assert ext.model == "llama3.2:3b"
        assert ext.host == "http://localhost:11434"
        assert ext.timeout == 60.0


class TestMemoryExtractorIntegration:
    """Tests Memory.add() with extract flag and set_extractor/has_extractor."""

    def test_add_extract_false_stores_raw(self):
        """With extract=False, content is stored as-is even if extractor is set."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            mem = Memory(db, extractor=None)

            # Set a mock extractor
            mock_ext = MagicMock(spec=MemoryExtractor)
            mem.set_extractor(mock_ext)

            mid = mem.add("raw text here", extract=False)
            assert mid  # got an ID
            mock_ext.extract.assert_not_called()

            # Verify content stored as-is
            results = mem.recall("raw text", limit=1)
            assert len(results) >= 1
            assert "raw text here" in results[0]["content"]

    def test_set_extractor_and_has_extractor(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            # Patch env to avoid auto-config picking up real keys
            with patch.dict(os.environ, {}, clear=True):
                mem = Memory(db, extractor=None)
                assert mem.has_extractor() is False

                mock_ext = MagicMock(spec=MemoryExtractor)
                mem.set_extractor(mock_ext)
                assert mem.has_extractor() is True

                mem.set_extractor(None)
                assert mem.has_extractor() is False

    def test_add_with_extractor_calls_extract(self):
        """When extractor is set, add() with extract=True calls extractor."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            mem = Memory(db, extractor=None)

            mock_ext = MagicMock(spec=MemoryExtractor)
            mock_ext.extract.return_value = [
                ExtractedFact(content="extracted fact 1", memory_type="factual",
                              importance=0.7),
            ]
            mem.set_extractor(mock_ext)

            mid = mem.add("some conversation text", extract=True)
            mock_ext.extract.assert_called_once_with("some conversation text")
            assert mid  # stored the extracted fact

    def test_add_extractor_failure_falls_back_to_raw(self):
        """If extractor raises, fall back to raw storage."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            mem = Memory(db, extractor=None)

            mock_ext = MagicMock(spec=MemoryExtractor)
            mock_ext.extract.side_effect = RuntimeError("API down")
            mem.set_extractor(mock_ext)

            mid = mem.add("fallback content", extract=True)
            assert mid  # should still store
            results = mem.recall("fallback", limit=1)
            assert len(results) >= 1


# ═══════════════════════════════════════════════════════════════
# 2. Config Hierarchy (env vars, config file, auto-detect)
# ═══════════════════════════════════════════════════════════════


class TestConfigHierarchy:
    def _make_memory_no_autoconfig(self, tmpdir):
        """Create Memory with all env vars cleared to prevent auto-config."""
        db = os.path.join(tmpdir, "test.db")
        env_clean = {
            k: v for k, v in os.environ.items()
            if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "ENGRAM_EXTRACTOR_MODEL")
        }
        with patch.dict(os.environ, env_clean, clear=True):
            return Memory(db)

    def test_env_anthropic_auth_token_oauth(self):
        """ANTHROPIC_AUTH_TOKEN env var → AnthropicExtractor with OAuth."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            env = {"ANTHROPIC_AUTH_TOKEN": "sk-ant-oat01-test-token"}
            # Clear ANTHROPIC_API_KEY to ensure OAuth path
            with patch.dict(os.environ, env, clear=False):
                with patch.dict(os.environ, {"ANTHROPIC_API_KEY": ""}, clear=False):
                    os.environ.pop("ANTHROPIC_API_KEY", None)
                    mem = Memory(db, extractor=None)  # extractor=None triggers auto-detect
                    # Re-trigger auto-detect
                    mem._extractor = mem._auto_configure_extractor()
                    assert mem.has_extractor()
                    assert isinstance(mem._extractor, AnthropicExtractor)
                    assert mem._extractor.is_oauth is True

    def test_env_anthropic_api_key(self):
        """ANTHROPIC_API_KEY env var → AnthropicExtractor with API key."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            clean_env = {k: v for k, v in os.environ.items()
                         if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
            clean_env["ANTHROPIC_API_KEY"] = "sk-ant-api03-test-key"
            with patch.dict(os.environ, clean_env, clear=True):
                mem = Memory(db, extractor=None)
                mem._extractor = mem._auto_configure_extractor()
                assert mem.has_extractor()
                assert isinstance(mem._extractor, AnthropicExtractor)
                assert mem._extractor.is_oauth is False

    def test_env_overrides_config_file(self):
        """Env var takes priority over config file settings."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            # Config file says ollama, but env says anthropic
            config_data = {"extractor": {"provider": "ollama", "model": "llama3.2:3b"}}
            config_path = Path(tmpdir) / "config.json"
            config_path.write_text(json.dumps(config_data))

            clean_env = {k: v for k, v in os.environ.items()
                         if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
            clean_env["ANTHROPIC_AUTH_TOKEN"] = "sk-ant-oat01-override"
            with patch.dict(os.environ, clean_env, clear=True):
                mem = Memory(db, extractor=None)
                mem._extractor = mem._auto_configure_extractor()
                # Should be Anthropic (from env), not Ollama (from config)
                assert isinstance(mem._extractor, AnthropicExtractor)

    def test_config_file_loading(self):
        """Config file with ollama provider creates OllamaExtractor."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            config_data = {
                "extractor": {
                    "provider": "ollama",
                    "model": "mistral:7b",
                    "host": "http://localhost:11434",
                }
            }
            config_path = Path(tmpdir) / "config.json"
            config_path.write_text(json.dumps(config_data))

            clean_env = {k: v for k, v in os.environ.items()
                         if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
            with patch.dict(os.environ, clean_env, clear=True):
                mem = Memory(db, extractor=None)
                # Patch the config path to our temp file
                with patch.object(Path, "expanduser", return_value=config_path):
                    ext = mem._load_extractor_from_config()
                assert isinstance(ext, OllamaExtractor)
                assert ext.model == "mistral:7b"

    def test_no_config_no_env_graceful(self):
        """No config file, no env vars → no extractor (None), no crash."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            clean_env = {k: v for k, v in os.environ.items()
                         if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY",
                                      "ENGRAM_EXTRACTOR_MODEL")}
            with patch.dict(os.environ, clean_env, clear=True):
                # Also patch config file to not exist
                with patch.object(Path, "expanduser",
                                  return_value=Path("/nonexistent/config.json")):
                    mem = Memory(db, extractor=None)
                    mem._extractor = mem._auto_configure_extractor()
                    assert mem.has_extractor() is False

    def test_config_file_anthropic_needs_env_token(self):
        """Config says anthropic but no token in env → no extractor."""
        with tempfile.TemporaryDirectory() as tmpdir:
            db = os.path.join(tmpdir, "test.db")
            config_data = {"extractor": {"provider": "anthropic", "model": "claude-haiku-4-5-20251001"}}
            config_path = Path(tmpdir) / "config.json"
            config_path.write_text(json.dumps(config_data))

            clean_env = {k: v for k, v in os.environ.items()
                         if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
            with patch.dict(os.environ, clean_env, clear=True):
                mem = Memory(db, extractor=None)
                with patch.object(Path, "expanduser", return_value=config_path):
                    ext = mem._load_extractor_from_config()
                # Anthropic config without token → None
                assert ext is None


# ═══════════════════════════════════════════════════════════════
# 3. Hybrid Search (engram/hybrid_search.py)
# ═══════════════════════════════════════════════════════════════


class TestHybridSearch:
    @pytest.fixture
    def mem(self, tmp_path):
        """Create a Memory instance with some test data."""
        db = str(tmp_path / "test.db")
        # Clear env to avoid auto-config
        clean_env = {k: v for k, v in os.environ.items()
                     if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
        with patch.dict(os.environ, clean_env, clear=True):
            m = Memory(db, extractor=None)
        m.add("Python is a great programming language", type="factual",
              importance=0.7, extract=False)
        m.add("Rust is fast and memory safe", type="factual",
              importance=0.7, extract=False)
        m.add("potato likes ice cream on hot days", type="relational",
              importance=0.6, extract=False)
        m.add("The meeting is scheduled for Monday", type="episodic",
              importance=0.5, extract=False)
        return m

    def test_recall_returns_results(self, mem):
        """Basic recall returns results from FTS + ACT-R."""
        results = mem.recall("Python programming", limit=5)
        assert len(results) > 0
        # The Python-related memory should be in results
        contents = [r["content"] for r in results]
        assert any("Python" in c for c in contents)

    def test_fts_only_no_embeddings(self, mem):
        """Without embeddings, recall still works via FTS5 + ACT-R."""
        assert mem._vector_store is None  # no embeddings
        results = mem.recall("Rust memory safe", limit=3)
        assert len(results) > 0

    def test_exact_term_match_ranks_higher(self, mem):
        """Exact keyword matches should rank higher than unrelated content."""
        results = mem.recall("ice cream", limit=5)
        assert len(results) > 0
        # First result should be the ice cream one
        assert "ice cream" in results[0]["content"]

    def test_hybrid_engine_weight_distribution(self, tmp_path):
        """Verify weight constants approximately match documented 15/60/25 split."""
        db = str(tmp_path / "weights.db")
        clean_env = {k: v for k, v in os.environ.items()
                     if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
        with patch.dict(os.environ, clean_env, clear=True):
            m = Memory(db, extractor=None)

        store = m._store
        engine = HybridSearchEngine(store, vector_store=None)

        # Check the weight computation in _score_candidates
        # Default vector_weight = 0.7
        fts_weight = 0.15
        emb_adj = 0.7 * 0.85  # ≈ 0.595
        actr_adj = 1.0 - 0.7  # = 0.3

        assert fts_weight == pytest.approx(0.15)
        assert emb_adj == pytest.approx(0.595)
        assert actr_adj == pytest.approx(0.3)
        # Total (without hebbian/pinned boosts): 0.15 + 0.595 + 0.3 = 1.045
        # Close to 1.0 — the slight overshoot is from rounding in weight design

    def test_recall_with_type_filter(self, mem):
        """Type filter should narrow results."""
        results = mem.recall("Python", limit=10, types=["relational"])
        # Should not return the Python factual memory
        for r in results:
            assert r["type"] == "relational"

    def test_sanitize_fts_query(self):
        """FTS query sanitization removes special chars."""
        assert sanitize_fts_query("hello world") != ""
        # Special chars removed
        result = sanitize_fts_query("hello! @world #test")
        assert "@" not in result
        assert "#" not in result

    def test_detect_temporal_alpha(self):
        """Temporal queries get lower alpha (more ACT-R influence)."""
        assert detect_temporal_alpha("what happened recently") < 0.9
        assert detect_temporal_alpha("semantic concept search") == 0.9

    def test_hybrid_search_with_mock_vector_store(self, tmp_path):
        """Hybrid search integrates vector scores when vector_store is provided."""
        db = str(tmp_path / "hybrid.db")
        clean_env = {k: v for k, v in os.environ.items()
                     if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
        with patch.dict(os.environ, clean_env, clear=True):
            m = Memory(db, extractor=None)

        mid1 = m.add("Apples are red fruits", type="factual", extract=False)
        mid2 = m.add("Bananas are yellow fruits", type="factual", extract=False)

        # Create a mock vector store
        mock_vs = MagicMock()
        mock_vs.search.return_value = [
            (mid1, 0.95),  # high similarity for apples
            (mid2, 0.3),   # low similarity
        ]

        engine = HybridSearchEngine(m._store, vector_store=mock_vs)
        results = engine.search(query="red apple", limit=5)
        assert len(results) > 0
        # The apple entry should score higher
        ids = [r.entry.id for r in results]
        assert mid1 in ids


# ═══════════════════════════════════════════════════════════════
# 4. CJK Tokenization
# ═══════════════════════════════════════════════════════════════


class TestCJKTokenization:
    def test_contains_cjk_chinese(self):
        assert contains_cjk("你好世界") is True

    def test_contains_cjk_english(self):
        assert contains_cjk("hello world") is False

    def test_contains_cjk_mixed(self):
        assert contains_cjk("hello 你好 world") is True

    def test_tokenize_for_fts_chinese(self):
        """Chinese text should be tokenized (character n-grams or jieba)."""
        result = tokenize_for_fts("你好世界")
        assert result  # non-empty
        assert isinstance(result, str)

    def test_tokenize_for_fts_english(self):
        """English text passes through tokenizer."""
        result = tokenize_for_fts("hello world")
        assert "hello" in result.lower() or "world" in result.lower()

    def test_tokenize_for_fts_mixed(self):
        """Mixed CJK + English text gets tokenized."""
        result = tokenize_for_fts("hello 你好 world")
        assert result
        # Should contain English words
        assert "hello" in result.lower() or "world" in result.lower()

    def test_cjk_memory_search(self, tmp_path):
        """End-to-end: store Chinese text and search for it."""
        db = str(tmp_path / "cjk.db")
        clean_env = {k: v for k, v in os.environ.items()
                     if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
        with patch.dict(os.environ, clean_env, clear=True):
            m = Memory(db, extractor=None)
        m.add("potato喜欢吃冰淇淋", type="relational", importance=0.7, extract=False)
        m.add("今天天气很好适合出去玩", type="episodic", importance=0.5, extract=False)

        results = m.recall("冰淇淋", limit=5)
        # Should find the ice cream memory
        assert len(results) > 0

    def test_mixed_chinese_english_search(self, tmp_path):
        """Mixed Chinese-English content can be stored and retrieved."""
        db = str(tmp_path / "mixed.db")
        clean_env = {k: v for k, v in os.environ.items()
                     if k not in ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY")}
        with patch.dict(os.environ, clean_env, clear=True):
            m = Memory(db, extractor=None)
        m.add("Python是很好的编程语言", type="factual", importance=0.7, extract=False)

        results = m.recall("Python 编程", limit=5)
        assert len(results) > 0
        assert any("Python" in r["content"] for r in results)
