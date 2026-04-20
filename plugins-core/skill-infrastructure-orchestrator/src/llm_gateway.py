"""
LLM Gateway client for calling OpenAI-compatible and Anthropic-compatible endpoints.
"""

import os
import time
from typing import Optional

import requests


def call_llm(
    prompt: str,
    model: str = "minimax:M2",
    endpoint: str = "http://localhost:18080",
    timeout: float = 120,
    max_retries: int = 3,
) -> str:
    """
    Call OpenAI-compatible LLM gateway.

    Args:
        prompt: The prompt to send
        model: Model identifier (e.g., "minimax:M2")
        endpoint: Gateway URL (e.g., "http://localhost:18080")
        timeout: Request timeout in seconds
        max_retries: Number of retry attempts on failure

    Returns:
        The model's response text

    Raises:
        requests.RequestException: On repeated failure
    """
    if endpoint == "anthropic":
        return call_llm_anthropic(prompt, model, timeout, max_retries)

    api_key = os.environ.get("LLM_API_KEY", os.environ.get("SWITCHAI_KEY", ""))
    backoff = 1.0

    for attempt in range(max_retries):
        try:
            response = requests.post(
                f"{endpoint}/v1/chat/completions",
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {api_key}",
                },
                json={
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": 0.7,
                },
                timeout=timeout,
            )
            response.raise_for_status()
            result = response.json()

            if "choices" in result and len(result["choices"]) > 0:
                return result["choices"][0]["message"]["content"]
            elif "content" in result:
                return result["content"]
            else:
                return str(result)

        except requests.RequestException as e:
            if attempt == max_retries - 1:
                raise
            time.sleep(backoff)
            backoff *= 2

    return ""


def call_llm_anthropic(
    prompt: str,
    model: str = "claude-sonnet-4-20250514",
    timeout: float = 120,
    max_retries: int = 3,
) -> str:
    """
    Call Anthropic-compatible endpoint directly.

    Args:
        prompt: The prompt to send
        model: Model identifier (e.g., "claude-sonnet-4-20250514")
        timeout: Request timeout in seconds
        max_retries: Number of retry attempts on failure

    Returns:
        The model's response text

    Raises:
        requests.RequestException: On repeated failure
    """
    api_key = os.environ.get("ANTHROPIC_API_KEY", os.environ.get("LLM_API_KEY", ""))
    if not api_key:
        api_key = "sk-ant-api03-placeholder"  # Fallback for testing

    backoff = 1.0

    for attempt in range(max_retries):
        try:
            response = requests.post(
                "https://api.anthropic.com/v1/messages",
                headers={
                    "Content-Type": "application/json",
                    "x-api-key": api_key,
                    "anthropic-version": "2023-06-01",
                    "anthropic-dangerous-direct-browser-access": "true",
                },
                json={
                    "model": model,
                    "max_tokens": 4096,
                    "messages": [{"role": "user", "content": prompt}],
                },
                timeout=timeout,
            )
            response.raise_for_status()
            result = response.json()

            if "content" in result and len(result["content"]) > 0:
                return result["content"][0]["text"]
            else:
                return str(result)

        except requests.RequestException as e:
            if attempt == max_retries - 1:
                raise
            time.sleep(backoff)
            backoff *= 2

    return ""


def call_with_fallback(
    prompt: str,
    primary_model: str,
    primary_endpoint: str,
    fallback_model: str = "minimax:M2",
    fallback_endpoint: str = "http://localhost:18080",
    timeout: float = 120,
) -> tuple[str, str]:
    """
    Call LLM with automatic fallback on failure.
    Returns (response, model_used) tuple.
    """
    try:
        response = call_llm(prompt, primary_model, primary_endpoint, timeout)
        return response, primary_model
    except Exception as e:
        # Fallback to default model
        try:
            response = call_llm(prompt, fallback_model, fallback_endpoint, timeout)
            return response, fallback_model
        except Exception:
            return f"Both primary and fallback failed. Primary error: {str(e)}", "failed"
