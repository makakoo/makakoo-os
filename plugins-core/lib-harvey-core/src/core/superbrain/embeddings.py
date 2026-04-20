"""Superbrain embedding service — routes through switchAILocal (local, no rate limits).

Gemini Embedding 2 is ONLY for the multimodal/OCR agent (separate skill).
Core superbrain text embeddings use switchAILocal's qwen3-embedding:0.6b (local via Ollama).

Conventions (aligned with superbrain.py:_embed, Phase 2 of SPRINT-SUPERBRAIN-REPAIR):
  - SUPERBRAIN_EMBED_TIMEOUT env var (default 120s) — CPU-only Ollama can
    take 5-15s per 2000-char embed under load; a 30s timeout bogs down.
  - SUPERBRAIN_EMBED_MAX_CHARS env var (default 2000) — most embed models
    have 512-token training windows; 8000-char inputs run ~30× slower on
    CPU with zero retrieval-quality gain.
  - EMBEDDING_MODEL env var propagated to callers via CURRENT_MODEL so the
    `model` column in brain_vectors gets the real name, not "auto" /
    "switchAILocal" literals.
"""

import os
import logging
import time
from typing import List, Optional

log = logging.getLogger("superbrain.embeddings")

# Embedding config — switchAILocal is the default, always local
_BASE_URL = os.environ.get("EMBEDDING_BASE_URL",
            os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1"))
_API_KEY = os.environ.get("EMBEDDING_API_KEY",
           os.environ.get("LLM_API_KEY",
           os.environ.get("SWITCHAI_KEY", "sk-test-123")))
_MODEL = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:0.6b")

# Public constant — callers tag stored vectors with this so the brain_vectors
# model column holds the real name.
CURRENT_MODEL = _MODEL


def embed_text(text: str, retries: int = 3) -> Optional[List[float]]:
    """Embed text via switchAILocal. Local, fast, no rate limits."""
    if not text or len(text.strip()) < 10:
        log.warning("Text too short to embed: %d chars", len(text or ""))
        return None

    import requests
    max_chars = int(os.environ.get("SUPERBRAIN_EMBED_MAX_CHARS", "2000"))
    timeout = float(os.environ.get("SUPERBRAIN_EMBED_TIMEOUT", "120"))
    text = text[:max_chars]

    for attempt in range(retries):
        try:
            resp = requests.post(
                f"{_BASE_URL}/embeddings",
                headers={"Authorization": f"Bearer {_API_KEY}"},
                json={"model": _MODEL, "input": text},
                timeout=timeout,
            )
            if resp.status_code == 200:
                return resp.json()["data"][0]["embedding"]

            elif resp.status_code == 429 or resp.status_code == 503:
                wait = 2 ** attempt
                log.warning("Embedding rate limited (%d), retrying in %ds...",
                            resp.status_code, wait)
                time.sleep(wait)
                continue

            else:
                log.error("Embedding error %d: %s", resp.status_code, resp.text[:200])
                return None

        except requests.exceptions.Timeout:
            log.warning("Embedding timeout (attempt %d/%d)", attempt + 1, retries)
            time.sleep(1)
        except Exception as e:
            log.error("Embedding failed: %s", e)
            return None

    log.error("All %d embedding attempts failed", retries)
    return None


def embed_batch(texts: List[str], delay: float = 0.05) -> List[Optional[List[float]]]:
    """Embed multiple texts. Delay between calls (lower than Gemini needed)."""
    results = []
    for i, text in enumerate(texts):
        vec = embed_text(text)
        results.append(vec)
        if i < len(texts) - 1:
            time.sleep(delay)
    return results
