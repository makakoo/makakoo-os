"""Tests for the env-driven LLM config reader."""

from __future__ import annotations

from plugins_core.agent_harveychat.python.llm_config import LlmConfig


def test_from_env_reads_all_locked_keys():
    env = {
        "MAKAKOO_LLM_MODEL": "claude-opus-4-7",
        "MAKAKOO_LLM_MAX_TOKENS": "8192",
        "MAKAKOO_LLM_TEMPERATURE": "0.3",
        "MAKAKOO_LLM_REASONING_EFFORT": "high",
        "MAKAKOO_LLM_TOP_P": "0.9",
    }
    cfg = LlmConfig.from_env(env)
    assert cfg.model == "claude-opus-4-7"
    assert cfg.max_tokens == 8192
    assert cfg.temperature == 0.3
    assert cfg.reasoning_effort == "high"
    assert cfg.top_p == 0.9


def test_from_env_falls_back_to_defaults_when_missing():
    cfg = LlmConfig.from_env({})
    assert cfg.model == "ail-compound"
    assert cfg.max_tokens == 4096
    assert cfg.temperature == 0.7
    assert cfg.reasoning_effort == "medium"
    assert cfg.top_p == 1.0


def test_from_env_partial_override_keeps_default_for_unset_keys():
    cfg = LlmConfig.from_env({"MAKAKOO_LLM_MODEL": "custom-model"})
    assert cfg.model == "custom-model"
    assert cfg.max_tokens == 4096  # default
    assert cfg.reasoning_effort == "medium"  # default
