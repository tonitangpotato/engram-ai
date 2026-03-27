"""
LLM-based memory extraction.

Converts raw text into structured facts using LLMs. Optional feature
that preserves backward compatibility — if no extractor is set,
memories are stored as-is.

Ported from Rust: engram-ai-rust/src/extractor.rs
"""

import json
import logging
import re
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Optional

logger = logging.getLogger(__name__)


@dataclass
class ExtractedFact:
    """A single extracted fact from a conversation."""
    content: str
    memory_type: str  # factual, episodic, relational, procedural, emotional, opinion, causal
    importance: float  # 0.0 - 1.0
    confidence_label: str = "likely"  # confident, likely, uncertain


EXTRACTION_PROMPT = """You are a memory extraction system. Extract key facts from the following conversation that are worth remembering long-term.

Rules:
- Extract concrete facts, preferences, decisions, and commitments
- Each fact should be self-contained (understandable without context)
- Skip greetings, filler, acknowledgments
- Classify each fact: factual, episodic, relational, procedural, emotional, opinion, causal
- Rate importance 0.0-1.0 (preferences=0.6, decisions=0.8, commitments=0.9)
- Rate confidence: "confident" (direct statement, clear fact), "likely" (reasonable inference), "uncertain" (vague mention, speculation)
- If nothing worth remembering, return empty array
- Respond in the SAME LANGUAGE as the input

Respond with ONLY a JSON array (no markdown, no explanation):
[{"content": "...", "memory_type": "...", "importance": 0.X, "confidence": "confident|likely|uncertain"}]

Conversation:
"""


class MemoryExtractor(ABC):
    """Base class for memory extraction — converts raw text into structured facts."""

    @abstractmethod
    def extract(self, text: str) -> list[ExtractedFact]:
        """
        Extract key facts from raw conversation text.

        Returns empty list if nothing worth remembering.
        Raises on network/parsing errors.
        """
        ...


class AnthropicExtractor(MemoryExtractor):
    """
    Extracts facts using Anthropic Claude API.

    Supports both OAuth tokens (Claude Max) and API keys.
    Haiku is recommended for cost/speed balance.
    """

    def __init__(
        self,
        auth_token: str,
        is_oauth: bool = False,
        model: str = "claude-haiku-4-5-20251001",
        api_url: str = "https://api.anthropic.com",
        max_tokens: int = 1024,
        timeout: float = 30.0,
    ):
        self.auth_token = auth_token
        self.is_oauth = is_oauth
        self.model = model
        self.api_url = api_url.rstrip("/")
        self.max_tokens = max_tokens
        self.timeout = timeout

    def _build_headers(self) -> dict[str, str]:
        headers = {
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        }
        if self.is_oauth:
            headers.update({
                "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
                "authorization": f"Bearer {self.auth_token}",
                "user-agent": "claude-cli/2.1.39 (external, cli)",
                "x-app": "cli",
                "anthropic-dangerous-direct-browser-access": "true",
            })
        else:
            headers["x-api-key"] = self.auth_token
        return headers

    def extract(self, text: str) -> list[ExtractedFact]:
        import httpx

        prompt = f"{EXTRACTION_PROMPT}{text}"
        body = {
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": [{"role": "user", "content": prompt}],
        }
        url = f"{self.api_url}/v1/messages"

        resp = httpx.post(
            url,
            headers=self._build_headers(),
            json=body,
            timeout=self.timeout,
        )
        if resp.status_code != 200:
            raise RuntimeError(f"Anthropic API error {resp.status_code}: {resp.text}")

        data = resp.json()
        content_text = data.get("content", [{}])[0].get("text", "")
        return parse_extraction_response(content_text)


class OllamaExtractor(MemoryExtractor):
    """
    Extracts facts using a local Ollama chat model.

    Useful for local/private extraction without API costs.
    """

    def __init__(
        self,
        model: str = "llama3.2:3b",
        host: str = "http://localhost:11434",
        timeout: float = 60.0,
    ):
        self.model = model
        self.host = host.rstrip("/")
        self.timeout = timeout

    def extract(self, text: str) -> list[ExtractedFact]:
        import httpx

        prompt = f"{EXTRACTION_PROMPT}{text}"
        body = {
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": False,
        }
        url = f"{self.host}/api/chat"

        resp = httpx.post(
            url,
            headers={"content-type": "application/json"},
            json=body,
            timeout=self.timeout,
        )
        if resp.status_code != 200:
            raise RuntimeError(f"Ollama API error {resp.status_code}: {resp.text}")

        data = resp.json()
        content_text = data.get("message", {}).get("content", "")
        return parse_extraction_response(content_text)


def parse_extraction_response(content: str) -> list[ExtractedFact]:
    """
    Parse LLM extraction response into ExtractedFacts.

    Handles common LLM quirks:
    - Markdown-wrapped JSON (```json ... ```)
    - Extra whitespace
    - Invalid JSON (returns empty list with warning)
    """
    text = content.strip()

    # Strip markdown code blocks
    if text.startswith("```json"):
        text = text[7:]
    elif text.startswith("```"):
        text = text[3:]
    if text.endswith("```"):
        text = text[:-3]
    text = text.strip()

    if text == "[]":
        return []

    # Find JSON array in the response
    start = text.find("[")
    end = text.rfind("]")
    if start is None or end is None or start >= end:
        logger.warning("No JSON array found in extraction response: %s", text[:200])
        return []

    json_str = text[start : end + 1]

    try:
        raw = json.loads(json_str)
    except json.JSONDecodeError as e:
        logger.warning("Failed to parse extraction JSON: %s - content: %s", e, json_str[:200])
        return []

    facts: list[ExtractedFact] = []
    for item in raw:
        c = item.get("content", "")
        if not c:
            continue
        facts.append(
            ExtractedFact(
                content=c,
                memory_type=item.get("memory_type", "factual").lower(),
                importance=max(0.0, min(1.0, float(item.get("importance", 0.5)))),
                confidence_label=item.get("confidence", "likely"),
            )
        )
    return facts
