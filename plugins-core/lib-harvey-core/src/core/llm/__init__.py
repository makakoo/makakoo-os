"""LLM client utilities for Makakoo OS.
 
Small reusable clients that wrap switchAILocal with type-safe Python
APIs. Importable from any agent or skill that wants to talk to an LLM
without rebuilding retry, auth, and payload-construction code.

Currently exposes:

- `omni` — multimodal client for image / audio / video understanding
  via OMNI_MODEL (default: `kimi:kimi-k2.6`). Handles URL-or-base64
  media sources, OpenAI-compatible chat/completions format, and the
  Xiaomi-specific `input_audio` / `video_url` content-block types
  (legacy, pending migration).
"""

from . import omni

__all__ = ["omni"]
