"""
Automatic embedding provider detection and fallback chain.

Priority chain: Ollama → Sentence Transformers → FTS5-only

This enables zero-config deployments where Engram automatically uses
the best available embedding provider without requiring manual configuration.
"""

import os
import logging
from typing import Optional, Tuple

logger = logging.getLogger(__name__)


def detect_ollama() -> bool:
    """Check if Ollama is available and has embedding models."""
    try:
        import requests
        response = requests.get("http://localhost:11434/api/tags", timeout=2)
        if response.status_code == 200:
            models = response.json().get("models", [])
            # Check for common embedding models
            embedding_models = [
                "nomic-embed-text",
                "mxbai-embed-large",
                "all-minilm",
            ]
            for model in models:
                model_name = model.get("name", "")
                if any(em in model_name.lower() for em in embedding_models):
                    logger.info(f"✅ Ollama detected with embedding model: {model_name}")
                    return True
            logger.info("⚠️  Ollama running but no embedding models found")
            return False
    except Exception as e:
        logger.debug(f"Ollama not available: {e}")
        return False


def detect_sentence_transformers() -> bool:
    """Check if sentence-transformers is installed."""
    try:
        import sentence_transformers
        logger.info("✅ sentence-transformers library available")
        return True
    except ImportError:
        logger.info("❌ sentence-transformers not installed")
        return False


def detect_openai() -> bool:
    """Check if OpenAI API key is configured."""
    api_key = os.environ.get("OPENAI_API_KEY")
    if api_key and api_key.startswith("sk-"):
        logger.info("✅ OpenAI API key configured")
        return True
    logger.debug("OpenAI API key not found or invalid")
    return False


def auto_select_provider() -> Tuple[Optional[str], Optional[str]]:
    """
    Automatically select the best available embedding provider.
    
    Priority:
    1. Ollama (if running with embedding models)
    2. Sentence Transformers (if installed)
    3. OpenAI (if API key configured)
    4. None (FTS5-only fallback)
    
    Returns:
        Tuple of (provider_name, model_name) or (None, None) for FTS5-only
    """
    logger.info("🔍 Auto-detecting embedding provider...")
    
    # 1. Try Ollama first (fastest, local, free)
    if detect_ollama():
        default_model = "nomic-embed-text"
        logger.info(f"✅ Selected: Ollama ({default_model})")
        return ("ollama", default_model)
    
    # 2. Try Sentence Transformers (good balance, offline)
    if detect_sentence_transformers():
        default_model = "paraphrase-multilingual-MiniLM-L12-v2"
        logger.info(f"✅ Selected: Sentence Transformers ({default_model})")
        return ("sentence-transformers", default_model)
    
    # 3. Try OpenAI (requires API key)
    if detect_openai():
        logger.info("✅ Selected: OpenAI embeddings")
        return ("openai", None)
    
    # 4. Fallback to FTS5-only
    logger.warning("⚠️  No embedding provider available, using FTS5-only mode")
    logger.info("💡 To enable semantic search, install: pip install sentence-transformers")
    return (None, None)


def get_provider_with_fallback(requested: Optional[str] = None) -> Tuple[Optional[str], Optional[str], str]:
    """
    Get embedding provider with automatic fallback on errors.
    
    Args:
        requested: Explicitly requested provider (or None for auto)
        
    Returns:
        Tuple of (provider, model, reason)
    """
    # If explicit provider requested, try it first
    if requested and requested != "auto":
        logger.info(f"🎯 Explicit provider requested: {requested}")
        
        if requested == "ollama":
            if detect_ollama():
                model = os.environ.get("ENGRAM_OLLAMA_MODEL", "nomic-embed-text")
                return ("ollama", model, "explicit")
            logger.warning("⚠️  Ollama requested but not available, falling back...")
            
        elif requested == "sentence-transformers":
            if detect_sentence_transformers():
                model = os.environ.get("ENGRAM_ST_MODEL", "paraphrase-multilingual-MiniLM-L12-v2")
                return ("sentence-transformers", model, "explicit")
            logger.warning("⚠️  sentence-transformers requested but not installed, falling back...")
            
        elif requested == "openai":
            if detect_openai():
                return ("openai", None, "explicit")
            logger.warning("⚠️  OpenAI requested but API key not configured, falling back...")
            
        elif requested == "none":
            return (None, None, "explicit_fts5")
    
    # Auto-select with fallback chain
    provider, model = auto_select_provider()
    return (provider, model, "auto")


if __name__ == "__main__":
    # Test detection
    logging.basicConfig(level=logging.INFO, format="%(message)s")
    
    print("\n=== Provider Detection Test ===\n")
    
    print("1. Individual checks:")
    print(f"   Ollama: {'✅' if detect_ollama() else '❌'}")
    print(f"   Sentence Transformers: {'✅' if detect_sentence_transformers() else '❌'}")
    print(f"   OpenAI: {'✅' if detect_openai() else '❌'}")
    
    print("\n2. Auto-selection:")
    provider, model = auto_select_provider()
    if provider:
        print(f"   Selected: {provider}")
        if model:
            print(f"   Model: {model}")
    else:
        print("   Fallback: FTS5-only")
    
    print("\n3. Fallback chain test:")
    for requested in ["ollama", "sentence-transformers", "openai", "none", None]:
        provider, model, reason = get_provider_with_fallback(requested)
        req_str = requested or "auto"
        result = provider or "FTS5"
        print(f"   Request: {req_str:20s} → {result:20s} ({reason})")
