"""The router must send the measured prompt — and roll back on one env var."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from core.orchestration import router_compiled_prompt as rcp  # noqa: E402
from core.orchestration.intelligent_router import IntelligentRouter  # noqa: E402


def test_compiled_prompt_is_default(monkeypatch):
    monkeypatch.delenv("MAKAKOO_ROUTER_COMPILED_PROMPT", raising=False)
    body = IntelligentRouter()._llm_request_body("check the logs", "some-model")
    assert body["messages"] == rcp.render("check the logs")
    assert body["max_tokens"] == rcp.MAX_TOKENS
    assert body["temperature"] == rcp.TEMPERATURE
    assert body["model"] == "some-model"


def test_rollback_flag_restores_legacy_prompt(monkeypatch):
    monkeypatch.setenv("MAKAKOO_ROUTER_COMPILED_PROMPT", "off")
    body = IntelligentRouter()._llm_request_body("check the logs", "some-model")
    assert body["max_tokens"] == 200
    assert body["messages"][0]["content"] == "You are a strict JSON classifier."
    assert IntelligentRouter._LLM_PROMPT in body["messages"][1]["content"]


def test_request_text_reaches_the_model_verbatim(monkeypatch):
    monkeypatch.delenv("MAKAKOO_ROUTER_COMPILED_PROMPT", raising=False)
    request = 'odd "request" with\nnewlines'
    body = IntelligentRouter()._llm_request_body(request, "m")
    assert request in body["messages"][-1]["content"]
