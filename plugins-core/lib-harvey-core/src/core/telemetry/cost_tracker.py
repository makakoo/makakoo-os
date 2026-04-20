#!/usr/bin/env python3
"""
Harvey Cost Tracker — Token usage and API cost tracking per session.

Inspired by claurst's cost-tracker.ts. Tracks:
  - Token counts (input, output, cache read, cache write)
  - API costs per model
  - Session duration
  - Cumulative totals across sessions

Persists to data/telemetry/costs.jsonl for historical analysis.

Usage:
    from core.telemetry.cost_tracker import CostTracker

    tracker = CostTracker(session_id="abc123")
    tracker.record_usage(model="claude-opus-4-6", input_tokens=1000, output_tokens=500)
    tracker.record_usage(model="auto", input_tokens=2000, output_tokens=1000)

    print(tracker.session_summary())
    print(tracker.total_cost())

    # CLI: superbrain costs
"""

import json
import logging
import os
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Optional

log = logging.getLogger("harvey.costs")

HARVEY_HOME = os.environ.get("HARVEY_HOME", os.path.expanduser("~/MAKAKOO"))
COSTS_DIR = Path(HARVEY_HOME) / "data" / "telemetry"
COSTS_FILE = COSTS_DIR / "costs.jsonl"

# Pricing per 1M tokens (approximate, as of 2026)
MODEL_PRICING: Dict[str, Dict[str, float]] = {
    "claude-opus-4-6": {"input": 15.0, "output": 75.0, "cache_read": 1.5, "cache_write": 18.75},
    "claude-sonnet-4-6": {"input": 3.0, "output": 15.0, "cache_read": 0.3, "cache_write": 3.75},
    "claude-haiku-4-5": {"input": 0.80, "output": 4.0, "cache_read": 0.08, "cache_write": 1.0},
    "auto": {"input": 0.0, "output": 0.0},  # auto-routed via switchAILocal
    "qwen2.5-7b-instruct": {"input": 0.0, "output": 0.0},   # local, free
}


@dataclass
class UsageRecord:
    """Single API call usage."""
    model: str
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    timestamp: float = field(default_factory=time.time)
    cost_usd: float = 0.0
    agent: str = "harvey"


@dataclass
class SessionCosts:
    """Aggregated costs for a session."""
    session_id: str
    start_time: float = field(default_factory=time.time)
    records: List[UsageRecord] = field(default_factory=list)
    total_input: int = 0
    total_output: int = 0
    total_cache_read: int = 0
    total_cache_write: int = 0
    total_cost_usd: float = 0.0
    api_calls: int = 0


class CostTracker:
    """Track and persist API costs per session."""

    def __init__(self, session_id: str = None):
        self.session_id = session_id or f"session-{int(time.time())}"
        self.session = SessionCosts(session_id=self.session_id)
        COSTS_DIR.mkdir(parents=True, exist_ok=True)

    def record_usage(self, model: str, input_tokens: int = 0,
                     output_tokens: int = 0, cache_read_tokens: int = 0,
                     cache_write_tokens: int = 0, agent: str = "harvey"):
        """Record a single API call's usage."""
        cost = self._calculate_cost(
            model, input_tokens, output_tokens,
            cache_read_tokens, cache_write_tokens,
        )

        record = UsageRecord(
            model=model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_read_tokens=cache_read_tokens,
            cache_write_tokens=cache_write_tokens,
            cost_usd=cost,
            agent=agent,
        )

        self.session.records.append(record)
        self.session.total_input += input_tokens
        self.session.total_output += output_tokens
        self.session.total_cache_read += cache_read_tokens
        self.session.total_cache_write += cache_write_tokens
        self.session.total_cost_usd += cost
        self.session.api_calls += 1

        # Persist
        self._append_record(record)

    def _calculate_cost(self, model: str, input_t: int, output_t: int,
                        cache_read: int, cache_write: int) -> float:
        """Calculate cost in USD for a single API call."""
        # Find pricing (exact match or prefix match)
        pricing = MODEL_PRICING.get(model)
        if not pricing:
            for key, p in MODEL_PRICING.items():
                if model.startswith(key.split("-")[0]):
                    pricing = p
                    break
        if not pricing:
            return 0.0  # Unknown model, likely local

        cost = 0.0
        cost += (input_t / 1_000_000) * pricing.get("input", 0)
        cost += (output_t / 1_000_000) * pricing.get("output", 0)
        cost += (cache_read / 1_000_000) * pricing.get("cache_read", 0)
        cost += (cache_write / 1_000_000) * pricing.get("cache_write", 0)
        return round(cost, 6)

    def _append_record(self, record: UsageRecord):
        """Append a record to the JSONL costs file."""
        entry = {
            "session_id": self.session_id,
            "model": record.model,
            "input": record.input_tokens,
            "output": record.output_tokens,
            "cache_read": record.cache_read_tokens,
            "cache_write": record.cache_write_tokens,
            "cost_usd": record.cost_usd,
            "agent": record.agent,
            "ts": record.timestamp,
            "iso": datetime.fromtimestamp(record.timestamp).isoformat(),
        }
        with open(COSTS_FILE, "a") as f:
            f.write(json.dumps(entry) + "\n")

    # ── Reporting ─────────────────────────────────────────────

    def session_summary(self) -> dict:
        """Get current session summary."""
        s = self.session
        duration = time.time() - s.start_time

        # Per-model breakdown
        by_model: Dict[str, dict] = {}
        for r in s.records:
            if r.model not in by_model:
                by_model[r.model] = {"calls": 0, "input": 0, "output": 0, "cost": 0.0}
            by_model[r.model]["calls"] += 1
            by_model[r.model]["input"] += r.input_tokens
            by_model[r.model]["output"] += r.output_tokens
            by_model[r.model]["cost"] += r.cost_usd

        return {
            "session_id": s.session_id,
            "duration_min": round(duration / 60, 1),
            "api_calls": s.api_calls,
            "total_input_tokens": s.total_input,
            "total_output_tokens": s.total_output,
            "total_tokens": s.total_input + s.total_output,
            "cache_read_tokens": s.total_cache_read,
            "cache_write_tokens": s.total_cache_write,
            "total_cost_usd": round(s.total_cost_usd, 4),
            "by_model": by_model,
        }

    def total_cost(self) -> float:
        """Get total session cost in USD."""
        return round(self.session.total_cost_usd, 4)

    # ── Historical Analysis ───────────────────────────────────

    @staticmethod
    def load_history(days: int = 30) -> List[dict]:
        """Load historical cost records from JSONL file."""
        if not COSTS_FILE.exists():
            return []

        cutoff = time.time() - (days * 86400)
        records = []
        for line in COSTS_FILE.read_text().strip().split("\n"):
            if not line:
                continue
            try:
                record = json.loads(line)
                if record.get("ts", 0) >= cutoff:
                    records.append(record)
            except json.JSONDecodeError:
                continue
        return records

    @staticmethod
    def historical_summary(days: int = 30) -> dict:
        """Get aggregated historical costs."""
        records = CostTracker.load_history(days)
        if not records:
            return {"days": days, "total_cost_usd": 0, "api_calls": 0}

        total_cost = sum(r.get("cost_usd", 0) for r in records)
        total_input = sum(r.get("input", 0) for r in records)
        total_output = sum(r.get("output", 0) for r in records)
        sessions = len(set(r.get("session_id", "") for r in records))

        by_model: Dict[str, float] = {}
        for r in records:
            model = r.get("model", "unknown")
            by_model[model] = by_model.get(model, 0) + r.get("cost_usd", 0)

        return {
            "days": days,
            "api_calls": len(records),
            "sessions": sessions,
            "total_input_tokens": total_input,
            "total_output_tokens": total_output,
            "total_cost_usd": round(total_cost, 4),
            "avg_cost_per_session": round(total_cost / max(sessions, 1), 4),
            "by_model": {k: round(v, 4) for k, v in sorted(by_model.items(), key=lambda x: -x[1])},
        }

    # ── Display ───────────────────────────────────────────────

    def print_session(self):
        """Pretty-print session costs."""
        s = self.session_summary()
        print(f"\n{'=' * 45}")
        print(f"  Session Cost Report")
        print(f"{'=' * 45}")
        print(f"  Duration:    {s['duration_min']} min")
        print(f"  API calls:   {s['api_calls']}")
        print(f"  Tokens:      {s['total_tokens']:,} ({s['total_input_tokens']:,} in / {s['total_output_tokens']:,} out)")
        if s['cache_read_tokens']:
            print(f"  Cache:       {s['cache_read_tokens']:,} read / {s['cache_write_tokens']:,} write")
        print(f"  Cost:        ${s['total_cost_usd']:.4f}")
        if s['by_model']:
            print(f"\n  By model:")
            for model, data in s['by_model'].items():
                print(f"    {model}: {data['calls']} calls, ${data['cost']:.4f}")
        print(f"{'=' * 45}\n")

    @staticmethod
    def print_history(days: int = 30):
        """Pretty-print historical costs."""
        s = CostTracker.historical_summary(days)
        print(f"\n{'=' * 45}")
        print(f"  Cost History ({s['days']} days)")
        print(f"{'=' * 45}")
        print(f"  Sessions:    {s['sessions']}")
        print(f"  API calls:   {s['api_calls']}")
        print(f"  Total cost:  ${s['total_cost_usd']:.4f}")
        print(f"  Avg/session: ${s['avg_cost_per_session']:.4f}")
        if s.get('by_model'):
            print(f"\n  By model:")
            for model, cost in s['by_model'].items():
                print(f"    {model}: ${cost:.4f}")
        print(f"{'=' * 45}\n")
