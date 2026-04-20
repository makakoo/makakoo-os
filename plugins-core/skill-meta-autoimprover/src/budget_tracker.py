#!/usr/bin/env python3
"""
Budget Tracker — Sprint 4

Tracks token usage per session for budget enforcement.
Based on Paperclip's cost tracking model.

Token types:
  - input_tokens: prompt tokens sent to the model
  - output_tokens: response tokens received
  - cache_tokens: cached prompt tokens (if provider supports)
  - reasoning_tokens: reasoning/thinking tokens (if provider supports)
"""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional, Dict
import threading


@dataclass
class TokenCounter:
    """Tracks tokens for one session."""
    session_id: str
    input_tokens: int = 0
    output_tokens: int = 0
    cache_tokens: int = 0
    reasoning_tokens: int = 0
    _lock: threading.Lock = field(default_factory=threading.Lock)

    @property
    def total_tokens(self) -> int:
        """Total tokens used (in + out + cache + reasoning)."""
        return self.input_tokens + self.output_tokens + self.cache_tokens + self.reasoning_tokens

    @property
    def cost_estimate_usd(self) -> float:
        """Estimated cost in USD (rough averages)."""
        # Gemini 3 Flash: $0.000075/input M, $0.0003/output M
        # Use these rates as defaults
        return (self.input_tokens / 1_000_000 * 0.075 +
                self.output_tokens / 1_000_000 * 0.30)

    def add(self, input_tokens: int = 0, output_tokens: int = 0,
            cache_tokens: int = 0, reasoning_tokens: int = 0) -> None:
        """Add tokens from one API call. Thread-safe."""
        with self._lock:
            self.input_tokens += input_tokens
            self.output_tokens += output_tokens
            self.cache_tokens += cache_tokens
            self.reasoning_tokens += reasoning_tokens

    def reset(self) -> None:
        """Reset all counters to zero."""
        with self._lock:
            self.input_tokens = 0
            self.output_tokens = 0
            self.cache_tokens = 0
            self.reasoning_tokens = 0


class BudgetTracker:
    """Manages token tracking across all sessions."""

    # Rough token pricing (per 1M tokens)
    PRICING = {
        "gemini-flash": {"input": 0.075, "output": 0.30},   # USD
        "gemini-pro": {"input": 0.50, "output": 2.00},
        "claude-opus": {"input": 15.0, "output": 75.0},
        "claude-sonnet": {"input": 3.0, "output": 15.0},
        "claude-haiku": {"input": 0.25, "output": 1.25},
        "gpt-4o": {"input": 5.0, "output": 15.0},
        "gpt-4o-mini": {"input": 0.15, "output": 0.60},
    }

    def __init__(self):
        self._sessions: Dict[str, TokenCounter] = {}
        self._lock = threading.Lock()

    def get_session(self, session_id: str) -> TokenCounter:
        """Get or create a TokenCounter for a session."""
        with self._lock:
            if session_id not in self._sessions:
                self._sessions[session_id] = TokenCounter(session_id=session_id)
            return self._sessions[session_id]

    def record_call(
        self,
        session_id: str,
        input_tokens: int,
        output_tokens: int,
        cache_tokens: int = 0,
        reasoning_tokens: int = 0,
        model: Optional[str] = None,
    ) -> TokenCounter:
        """Record tokens from one API call."""
        counter = self.get_session(session_id)
        counter.add(
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_tokens=cache_tokens,
            reasoning_tokens=reasoning_tokens,
        )
        return counter

    def get_session_cost(self, session_id: str) -> float:
        """Get estimated cost for a session."""
        with self._lock:
            if session_id not in self._sessions:
                return 0.0
            counter = self._sessions[session_id]
            # Fall back to default gemini-flash rates
            rate_input = 0.075
            rate_output = 0.30
            return (counter.input_tokens / 1_000_000 * rate_input +
                    counter.output_tokens / 1_000_000 * rate_output)

    def get_all_session_costs(self) -> Dict[str, float]:
        """Get estimated costs for all sessions."""
        with self._lock:
            return {
                sid: self.get_session_cost(sid)
                for sid in self._sessions
            }

    def reset_session(self, session_id: str) -> None:
        """Reset counters for a session."""
        with self._lock:
            if session_id in self._sessions:
                self._sessions[session_id].reset()
