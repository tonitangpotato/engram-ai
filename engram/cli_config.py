"""
Engram CLI configuration — ~/.config/engram/config.json

Stores user preferences for embedding provider, model, DB path, etc.
Created by `engram init`, read by all commands.

Priority: --flag > $ENV > config.json > auto-detect
"""

import json
import os
from pathlib import Path

CONFIG_DIR = Path(os.environ.get("ENGRAM_CONFIG_DIR", "~/.config/engram")).expanduser()
CONFIG_FILE = CONFIG_DIR / "config.json"


def load_config() -> dict:
    """Load config from disk. Returns empty dict if not found."""
    if CONFIG_FILE.exists():
        try:
            return json.loads(CONFIG_FILE.read_text())
        except (json.JSONDecodeError, OSError):
            return {}
    return {}


def save_config(cfg: dict) -> None:
    """Save config to disk."""
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    CONFIG_FILE.write_text(json.dumps(cfg, indent=2) + "\n")


def resolve_embedding(flag_value: str | None) -> str | None:
    """
    Resolve embedding provider with priority:
    1. CLI flag (--embedding)
    2. Environment variable (ENGRAM_EMBEDDING)
    3. Config file
    4. None (caller decides fallback)
    """
    # 1. Flag
    if flag_value:
        return flag_value

    # 2. Env
    env = os.environ.get("ENGRAM_EMBEDDING")
    if env:
        return env

    # 3. Config
    cfg = load_config()
    if cfg.get("embedding"):
        return cfg["embedding"]

    return None


def resolve_db(flag_value: str | None) -> str:
    """Resolve DB path: flag > env > config > default."""
    if flag_value and flag_value != os.environ.get("NEUROMEM_DB", "./neuromem.db"):
        return flag_value

    env = os.environ.get("NEUROMEM_DB") or os.environ.get("ENGRAM_DB")
    if env:
        return env

    cfg = load_config()
    if cfg.get("db"):
        return cfg["db"]

    return "./neuromem.db"


def detect_ollama() -> bool:
    """Check if Ollama is running on localhost."""
    try:
        import urllib.request
        resp = urllib.request.urlopen("http://localhost:11434/api/tags", timeout=0.5)
        data = json.loads(resp.read())
        models = [m["name"] for m in data.get("models", [])]
        return any("embed" in m for m in models)
    except Exception:
        return False


def detect_embedding_models() -> list[dict]:
    """Detect available embedding providers and models."""
    found = []

    # Ollama
    try:
        import urllib.request
        resp = urllib.request.urlopen("http://localhost:11434/api/tags", timeout=0.5)
        data = json.loads(resp.read())
        for m in data.get("models", []):
            if "embed" in m["name"]:
                found.append({
                    "provider": "ollama",
                    "model": m["name"],
                    "local": True,
                    "free": True,
                })
    except Exception:
        pass

    # OpenAI
    if os.environ.get("OPENAI_API_KEY"):
        found.append({
            "provider": "openai",
            "model": "text-embedding-3-small",
            "local": False,
            "free": False,
        })

    return found
