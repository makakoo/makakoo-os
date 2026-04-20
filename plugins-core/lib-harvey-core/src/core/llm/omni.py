"""Xiaomi MiMo omni-modal client for image / audio / video understanding.

Routes through switchAILocal to `xiaomi-tp:mimo-v2-omni` using the
OpenAI-compatible `/v1/chat/completions` endpoint. Xiaomi extends the
OpenAI spec with two custom content-block types:

- `input_audio` — pass `{"data": "<url_or_data_uri>"}` alongside a text
  prompt to transcribe, analyze tone, or answer questions about speech.
- `video_url` — pass `{"url": "<url_or_data_uri>"}` with optional `fps`
  (default 2, max 10) and `media_resolution` (`default` or `max`).

Image understanding uses the standard OpenAI `image_url` block — no
Xiaomi-specific extension needed.

Media sources can be either public URLs (HTTP/HTTPS) or local file
paths. Local files are read and base64-encoded with a MIME-prefixed
data URI so the same function signature covers both cases.

Python 3.9 compatible — no PEP 604 unions, no `match` statements.

## Convenience API

    from core.llm.omni import describe_image, describe_audio, describe_video

    text = describe_image("https://example.com/cat.jpg", "What is in this image?")
    text = describe_audio("/path/to/speech.wav", "Transcribe this.")
    text = describe_video("clip.mp4", "What happens?", fps=2, media_resolution="default")

## Raw API

    from core.llm.omni import OmniClient

    client = OmniClient()
    result = client.chat(
        messages=[{"role": "user", "content": [ ...raw blocks... ]}],
        model="xiaomi-tp:mimo-v2-omni",
    )

Agents that already build multi-block messages can use `OmniClient.chat`
directly; agents that just want to understand one media file should use
the `describe_*` helpers.
"""

from __future__ import annotations

import base64
import mimetypes
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

import httpx

# ─── Config ─────────────────────────────────────────────────────

LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "http://localhost:18080/v1").rstrip("/")

# Canonical omni model per Xiaomi usage guide, overridable for tests
# or future version bumps.
OMNI_MODEL = os.environ.get("OMNI_MODEL", "xiaomi-tp:mimo-v2-omni")

# Same API-key precedence as anchor_extractor — AIL_API_KEY is the
# canonical name, SWITCHAI_KEY and LLM_API_KEY are legacy aliases.
_KEY_NAMES = ("AIL_API_KEY", "SWITCHAI_KEY", "LLM_API_KEY")

REQUEST_TIMEOUT_DEFAULT = float(os.environ.get("OMNI_TIMEOUT", "120.0"))
RETRIES_DEFAULT = int(os.environ.get("OMNI_RETRIES", "3"))
MAX_TOKENS_DEFAULT = int(os.environ.get("OMNI_MAX_TOKENS", "1024"))

# MIME hints for local-file detection. mimetypes.guess_type handles most
# of this but falls back to None on exotic extensions — this table is the
# override for the common cases the Xiaomi docs call out.
_EXT_MIME = {
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".bmp": "image/bmp",
    ".wav": "audio/wav",
    ".mp3": "audio/mpeg",
    ".flac": "audio/flac",
    ".ogg": "audio/ogg",
    ".m4a": "audio/mp4",
    ".mp4": "video/mp4",
    ".mov": "video/quicktime",
    ".webm": "video/webm",
    ".mkv": "video/x-matroska",
}


# ─── Errors ─────────────────────────────────────────────────────


class OmniError(Exception):
    """Base class for omni-client failures that callers can catch."""


class OmniAuthError(OmniError):
    """Raised when no API key is resolvable at call time."""


class OmniAPIError(OmniError):
    """Raised when switchAILocal returns a non-2xx response."""


# ─── Helpers ────────────────────────────────────────────────────


def _resolve_api_key() -> str:
    for name in _KEY_NAMES:
        val = os.environ.get(name)
        if val:
            return val
    return ""


def _is_url(source: str) -> bool:
    return source.startswith("http://") or source.startswith("https://")


def _is_data_uri(source: str) -> bool:
    return source.startswith("data:")


def _mime_for(path: Path) -> str:
    """Best-effort MIME detection for a local file.

    Order: extension override table → mimetypes.guess_type → generic
    `application/octet-stream`. Callers who need a specific MIME should
    pass a data URI directly instead of a local path.
    """
    ext = path.suffix.lower()
    if ext in _EXT_MIME:
        return _EXT_MIME[ext]
    guessed, _ = mimetypes.guess_type(str(path))
    if guessed:
        return guessed
    return "application/octet-stream"


def _encode_local_file(source: str) -> str:
    """Read a local file and return a `data:<mime>;base64,<payload>` URI."""
    path = Path(os.path.expanduser(source)).resolve()
    if not path.is_file():
        raise OmniError("local file not found: {}".format(path))
    mime = _mime_for(path)
    data = path.read_bytes()
    encoded = base64.b64encode(data).decode("ascii")
    return "data:{};base64,{}".format(mime, encoded)


def _resolve_media_source(source: str) -> str:
    """URL-or-data-URI passthrough; local paths get base64-encoded."""
    if _is_url(source) or _is_data_uri(source):
        return source
    return _encode_local_file(source)


# ─── Content-block builders ─────────────────────────────────────


def image_block(source: str) -> Dict[str, Any]:
    """Build an OpenAI-compatible image_url content block from URL or path."""
    return {"type": "image_url", "image_url": {"url": _resolve_media_source(source)}}


def audio_block(source: str) -> Dict[str, Any]:
    """Build a Xiaomi input_audio content block from URL or path."""
    return {"type": "input_audio", "input_audio": {"data": _resolve_media_source(source)}}


def video_block(
    source: str,
    fps: Optional[int] = None,
    media_resolution: Optional[str] = None,
) -> Dict[str, Any]:
    """Build a Xiaomi video_url content block from URL or path.

    `fps` defaults to 2 (Xiaomi's own default) and caps at 10.
    `media_resolution` is `"default"` or `"max"` per the Xiaomi docs.
    Both are passed through untouched when set so callers can override
    without the client second-guessing.
    """
    block: Dict[str, Any] = {
        "type": "video_url",
        "video_url": {"url": _resolve_media_source(source)},
    }
    if fps is not None:
        block["fps"] = fps
    if media_resolution is not None:
        block["media_resolution"] = media_resolution
    return block


def text_block(text: str) -> Dict[str, Any]:
    """Build a text content block."""
    return {"type": "text", "text": text}


# ─── Client ─────────────────────────────────────────────────────


@dataclass
class OmniResult:
    """Parsed omni-chat response — raw dict + extracted message text."""

    text: str
    model: str
    usage: Dict[str, Any]
    raw: Dict[str, Any]


class OmniClient:
    """Thin wrapper around switchAILocal's chat/completions endpoint.

    Handles auth-header construction, retries, and response parsing.
    Exposes both the high-level `describe_*` helpers and a lower-level
    `chat()` method for callers that build their own message arrays.
    """

    def __init__(
        self,
        base_url: Optional[str] = None,
        api_key: Optional[str] = None,
        model: Optional[str] = None,
        timeout: float = REQUEST_TIMEOUT_DEFAULT,
        retries: int = RETRIES_DEFAULT,
    ):
        self.base_url = (base_url or LLM_BASE_URL).rstrip("/")
        self._api_key = api_key if api_key is not None else _resolve_api_key()
        self.model = model or OMNI_MODEL
        self.timeout = timeout
        self.retries = max(1, retries)

    # ─── Raw chat ────────────────────────────────────────────────

    def chat(
        self,
        messages: List[Dict[str, Any]],
        model: Optional[str] = None,
        max_completion_tokens: int = MAX_TOKENS_DEFAULT,
        **extra: Any,
    ) -> OmniResult:
        """POST a pre-built OpenAI-compatible messages array.

        `extra` kwargs are passed straight into the JSON payload so
        callers can set `temperature`, `response_format`, provider-
        specific flags, etc. without the client guessing.
        """
        if not self._api_key:
            raise OmniAuthError(
                "no API key — set AIL_API_KEY (or SWITCHAI_KEY / LLM_API_KEY)"
            )

        url = "{}/chat/completions".format(self.base_url)
        headers = {
            "Authorization": "Bearer {}".format(self._api_key),
            "Content-Type": "application/json",
        }
        payload: Dict[str, Any] = {
            "model": model or self.model,
            "messages": messages,
            "max_completion_tokens": max_completion_tokens,
        }
        payload.update(extra)

        last_error: Optional[Exception] = None
        for attempt in range(self.retries):
            try:
                with httpx.Client(timeout=self.timeout) as client:
                    resp = client.post(url, json=payload, headers=headers)
                if resp.status_code >= 400:
                    raise OmniAPIError(
                        "omni {} {} ({}): {}".format(
                            resp.status_code,
                            resp.reason_phrase,
                            payload["model"],
                            resp.text[:400],
                        )
                    )
                data = resp.json()
                choices = data.get("choices") or []
                if not choices:
                    raise OmniAPIError(
                        "omni empty choices for {}: {}".format(
                            payload["model"], str(data)[:400]
                        )
                    )
                message = choices[0].get("message") or {}
                content = message.get("content")
                if isinstance(content, list):
                    # Multi-block responses — concat text blocks.
                    text = "".join(
                        block.get("text", "")
                        for block in content
                        if isinstance(block, dict) and block.get("type") == "text"
                    )
                else:
                    text = content or ""
                return OmniResult(
                    text=text,
                    model=data.get("model", payload["model"]),
                    usage=data.get("usage") or {},
                    raw=data,
                )
            except (httpx.HTTPError, OmniAPIError) as exc:
                last_error = exc
                if attempt == self.retries - 1:
                    break

        assert last_error is not None  # loop always sets it on failure
        raise OmniAPIError(
            "omni chat failed after {} attempts: {}".format(self.retries, last_error)
        )

    # ─── Convenience helpers ─────────────────────────────────────

    def describe_image(
        self,
        source: str,
        prompt: str,
        max_completion_tokens: int = MAX_TOKENS_DEFAULT,
        **extra: Any,
    ) -> str:
        """One-shot image understanding — returns the model's text answer."""
        messages = [
            {
                "role": "user",
                "content": [image_block(source), text_block(prompt)],
            }
        ]
        return self.chat(
            messages,
            max_completion_tokens=max_completion_tokens,
            **extra,
        ).text

    def describe_audio(
        self,
        source: str,
        prompt: str,
        max_completion_tokens: int = MAX_TOKENS_DEFAULT,
        **extra: Any,
    ) -> str:
        """One-shot audio understanding — transcribe or answer questions about audio."""
        messages = [
            {
                "role": "user",
                "content": [audio_block(source), text_block(prompt)],
            }
        ]
        return self.chat(
            messages,
            max_completion_tokens=max_completion_tokens,
            **extra,
        ).text

    def describe_video(
        self,
        source: str,
        prompt: str,
        fps: Optional[int] = None,
        media_resolution: Optional[str] = None,
        max_completion_tokens: int = MAX_TOKENS_DEFAULT,
        **extra: Any,
    ) -> str:
        """One-shot video understanding — accepts Xiaomi `fps` and `media_resolution`."""
        messages = [
            {
                "role": "user",
                "content": [
                    video_block(source, fps=fps, media_resolution=media_resolution),
                    text_block(prompt),
                ],
            }
        ]
        return self.chat(
            messages,
            max_completion_tokens=max_completion_tokens,
            **extra,
        ).text


# ─── Module-level convenience wrappers ──────────────────────────

# Callers that don't need to hold a client instance can import these
# top-level functions directly. Each builds an ephemeral OmniClient per
# call — cheap since httpx.Client is opened inside chat() anyway.


def describe_image(
    source: str,
    prompt: str,
    model: Optional[str] = None,
    max_completion_tokens: int = MAX_TOKENS_DEFAULT,
    **extra: Any,
) -> str:
    """One-shot image description. See `OmniClient.describe_image`."""
    return OmniClient(model=model).describe_image(
        source, prompt, max_completion_tokens=max_completion_tokens, **extra
    )


def describe_audio(
    source: str,
    prompt: str,
    model: Optional[str] = None,
    max_completion_tokens: int = MAX_TOKENS_DEFAULT,
    **extra: Any,
) -> str:
    """One-shot audio description. See `OmniClient.describe_audio`."""
    return OmniClient(model=model).describe_audio(
        source, prompt, max_completion_tokens=max_completion_tokens, **extra
    )


def describe_video(
    source: str,
    prompt: str,
    fps: Optional[int] = None,
    media_resolution: Optional[str] = None,
    model: Optional[str] = None,
    max_completion_tokens: int = MAX_TOKENS_DEFAULT,
    **extra: Any,
) -> str:
    """One-shot video description. See `OmniClient.describe_video`."""
    return OmniClient(model=model).describe_video(
        source,
        prompt,
        fps=fps,
        media_resolution=media_resolution,
        max_completion_tokens=max_completion_tokens,
        **extra,
    )


def omni_chat(
    messages: List[Dict[str, Any]],
    model: Optional[str] = None,
    max_completion_tokens: int = MAX_TOKENS_DEFAULT,
    **extra: Any,
) -> OmniResult:
    """Raw chat call for callers building their own content blocks."""
    return OmniClient(model=model).chat(
        messages,
        max_completion_tokens=max_completion_tokens,
        **extra,
    )
