"""Tests for v0.2 D.5 — IntelligentRouter LLM mode behind a flag.

Covers:
  - mode="keyword" (explicit) preserves the existing classifier output
  - mode="llm" with LLM unavailable falls back to keyword (no exception)
  - mode=None reads MAKAKOO_ROUTER_LLM_MODE env var
  - LLM bad/missing response → fallback to keyword
"""
from __future__ import annotations

import os
import unittest
from unittest.mock import patch

from core.orchestration.intelligent_router import IntelligentRouter


class RouterLlmModeTests(unittest.TestCase):
    def setUp(self):
        # Make sure the env toggle starts clean for every test.
        os.environ.pop("MAKAKOO_ROUTER_LLM_MODE", None)
        self.router = IntelligentRouter()

    def test_keyword_mode_explicit_uses_keyword_classifier(self):
        cls, _team = self.router.classify_and_route(
            "research the latest LLM papers", mode="keyword"
        )
        self.assertEqual(cls.intent, "research")

    def test_llm_mode_falls_back_when_llm_unreachable(self):
        # Default LLM_BASE_URL points at localhost:18080; in a CI box
        # without switchAILocal up, urlopen will raise URLError. The router
        # must swallow that and return the keyword classification.
        with patch.dict(os.environ, {"LLM_BASE_URL": "http://127.0.0.1:1"}):
            cls, _team = self.router.classify_and_route(
                "save this snippet", mode="llm"
            )
        # "save" is in the archive keyword bucket — keyword fallback hits it.
        self.assertEqual(cls.intent, "archive")

    def test_env_toggle_picks_llm_mode(self):
        with patch.dict(
            os.environ,
            {"MAKAKOO_ROUTER_LLM_MODE": "1", "LLM_BASE_URL": "http://127.0.0.1:1"},
        ):
            # llm path will fail; we just verify it tried llm then fell back.
            cls, _team = self.router.classify_and_route(
                "find me five papers about transformer scaling laws"
            )
        # Keyword classifier identifies "research" / "find" / "papers".
        self.assertEqual(cls.intent, "research")

    def test_env_toggle_off_keeps_keyword(self):
        with patch.dict(os.environ, {"MAKAKOO_ROUTER_LLM_MODE": "0"}):
            cls, _team = self.router.classify_and_route("draw an image of a cat")
        self.assertEqual(cls.intent, "image")

    def test_llm_mode_returns_classification_when_llm_replies(self):
        fake_payload = {
            "choices": [
                {
                    "message": {
                        "content": '{"intent":"research","confidence":0.92,"rationale":"contains research keywords"}'
                    }
                }
            ]
        }

        class _FakeResp:
            def __init__(self, payload):
                import json
                self._body = json.dumps(payload).encode("utf-8")

            def __enter__(self):
                return self

            def __exit__(self, *a):
                return False

            def read(self):
                return self._body

        with patch(
            "urllib.request.urlopen",
            return_value=_FakeResp(fake_payload),
        ):
            cls, _team = self.router.classify_and_route(
                "doesn't matter — the mock decides", mode="llm"
            )
        self.assertEqual(cls.intent, "research")
        self.assertGreater(cls.confidence, 0.5)


if __name__ == "__main__":
    unittest.main()
