"""
Memory Summarizer - Context Compression for Memory Blocks

Multi-level compression using LLM-powered summarization via localhost:18080 gateway.
Compression levels: none (<2k), light (2-8k), medium (8-32k), heavy (>32k).
"""

import os
import re
import requests
from typing import Optional

SUMMARIZATION_PROMPT = """You are a memory compression AI. Given a memory block, extract only the most important, non-obvious facts that would be useful for future context.

Rules:
- Preserve specific numbers, names, dates (these are high-value)
- Remove verbose explanations
- Keep the original perspective/tone if it matters
- Discard anything that can be trivially inferred
- Output ONLY the compressed summary, no preamble

Input memory:
{memory}

Output: Concise summary (max {max_words} words) containing only essential facts."""

LLM_GATEWAY_URL = "http://localhost:18080/v1/messages"


class MemorySummarizer:
    """
    LLM-powered memory compression with multi-level strategies.

    Compression levels:
    - none (< 2k chars): Direct use, no compression
    - light (2-8k chars): 500 word target
    - medium (8-32k chars): 1k word target
    - heavy (> 32k chars): 2k word target
    """

    def __init__(self, gateway_url: str = LLM_GATEWAY_URL):
        self.gateway_url = gateway_url

    def summarize(self, memory_text: str, max_words: int = 200) -> str:
        """
        Compress a memory block using LLM summarization.

        Args:
            memory_text: Raw memory content
            max_words: Target word count for compression

        Returns:
            Compressed summary string
        """
        # Skip empty or very short content
        if not memory_text or len(memory_text) < 100:
            return memory_text

        # Check content size and apply appropriate compression
        char_count = len(memory_text)

        if char_count < 2000:
            # No compression needed
            return memory_text

        # Determine compression level
        if char_count < 8000:
            compression_level = "light"
            max_words = 150
        elif char_count < 32000:
            compression_level = "medium"
            max_words = 400
        else:
            compression_level = "heavy"
            max_words = 600

        # Call LLM gateway
        prompt = SUMMARIZATION_PROMPT.format(
            memory=memory_text,
            max_words=max_words
        )

        try:
            response = self._call_llm(prompt, max_tokens=max_words * 2)
            if response:
                return response
        except Exception as e:
            print(f"LLM summarization failed: {e}")

        # Fallback: simple extraction if LLM fails
        return self._fallback_summarize(memory_text, max_words)

    def summarize_journal(self, journal_path: str, max_words: int = 300) -> str:
        """
        Compress an old journal entry into a digest.

        Keeps: decisions made, tasks completed, important discoveries.

        Args:
            journal_path: Path to journal file
            max_words: Target word count

        Returns:
            Journal digest string
        """
        if not os.path.exists(journal_path):
            return f"Journal not found: {journal_path}"

        with open(journal_path, "r") as f:
            content = f.read()

        if len(content) < 2000:
            return content

        # Add journal-specific prompt context
        journal_prompt = f"""This is a daily journal entry. Extract and summarize:
1. Key decisions made
2. Tasks completed
3. Important discoveries or insights
4. Any commitments or follow-ups made

Journal content:
{content}

Provide a concise digest (max {max_words} words):"""

        try:
            response = self._call_llm(journal_prompt, max_tokens=max_words * 2)
            if response:
                return response
        except Exception as e:
            print(f"Journal summarization failed: {e}")

        return self._fallback_summarize(content, max_words)

    def _call_llm(self, prompt: str, max_tokens: int = 500) -> Optional[str]:
        """
        Call LLM via localhost:18080 gateway.

        Args:
            prompt: Full prompt to send
            max_tokens: Max tokens in response

        Returns:
            LLM response text or None on failure
        """
        payload = {
            "model": "claude",
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "max_tokens": max_tokens,
            "temperature": 0.3  # Low temperature for summarization
        }

        try:
            response = requests.post(
                self.gateway_url,
                json=payload,
                timeout=30
            )
            if response.status_code == 200:
                result = response.json()
                # Handle different response formats
                if "content" in result:
                    if isinstance(result["content"], list):
                        return result["content"][0].get("text", "")
                    return result["content"]
                elif "text" in result:
                    return result["text"]
                elif "completion" in result:
                    return result["completion"]
        except Exception as e:
            print(f"LLM gateway call failed: {e}")

        return None

    def _fallback_summarize(self, text: str, max_words: int) -> str:
        """
        Simple fallback summarization without LLM.
        Extracts first sentences and key facts.
        """
        # Split into lines/sentences
        sentences = re.split(r'[.!?]+\s*', text)

        # Take first N sentences
        target_sentences = min(10, len(sentences))
        summary = '. '.join(s.strip() for s in sentences[:target_sentences] if s.strip())

        # If still too long, truncate
        words = summary.split()
        if len(words) > max_words:
            summary = ' '.join(words[:max_words]) + "..."

        return summary

    def get_compression_level(self, text: str) -> str:
        """Determine compression level for given text."""
        char_count = len(text)
        if char_count < 2000:
            return "none"
        elif char_count < 8000:
            return "light"
        elif char_count < 32000:
            return "medium"
        return "heavy"


def summarize_text(text: str, max_words: int = 200) -> str:
    """Convenience function for quick summarization."""
    summarizer = MemorySummarizer()
    return summarizer.summarize(text, max_words)
