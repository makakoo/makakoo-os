"""Phase 5 — ``freelance_daily_digest_tick`` SANCHO handler.

- Fires when at least one overdue or threshold event happened in
  the last 24h.
- Stays silent on a zero-activity day (no new events).
- Message correctly lists offices + amounts when multiple events
  are in play.
"""
from __future__ import annotations

from datetime import date, timedelta
from pathlib import Path

import pytest

from src.sancho_handlers import _state as state_mod
from src.sancho_handlers import _telegram as tg
from src.sancho_handlers import daily_digest_tick


@pytest.fixture
def _isolated_state(monkeypatch, tmp_path):
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path / "makakoo"))


@pytest.fixture
def _capture_tg():
    sent = []
    tg.set_sender(lambda text, **kw: sent.append(text))
    yield sent
    tg.set_sender(None)


@pytest.fixture
def _capture_journal():
    lines = []
    daily_digest_tick.set_journal_fn(lambda l: lines.append(l))
    yield lines
    from src.core import brain
    daily_digest_tick.set_journal_fn(brain.append_journal_line)


def test_digest_fires_when_overdue_activity_in_last_24h(
    _isolated_state, _capture_tg, _capture_journal
):
    state_mod.save(
        "overdue_notifications.json",
        {
            "INV-2026-001": {
                "last_notified": "2026-04-20", "office_id": "de-main",
                "amount": 2400.0,
            }
        },
    )
    out = daily_digest_tick.tick(today=date(2026, 4, 20))
    assert out["fired"] is True
    assert _capture_tg, "telegram digest should have fired"
    assert "2,400.00" in _capture_tg[0] or "2400.00" in _capture_tg[0]


def test_digest_silent_on_zero_activity_day(
    _isolated_state, _capture_tg, _capture_journal
):
    out = daily_digest_tick.tick(today=date(2026, 4, 20))
    assert out["fired"] is False
    assert _capture_tg == []


def test_digest_lists_multiple_offices_and_amounts(
    _isolated_state, _capture_tg, _capture_journal
):
    state_mod.save(
        "overdue_notifications.json",
        {
            "INV-2026-001": {
                "last_notified": "2026-04-20", "office_id": "de-main",
                "amount": 2400.0,
            },
            "INV-2026-007": {
                "last_notified": "2026-04-20", "office_id": "de-main",
                "amount": 7100.0,
            },
        },
    )
    state_mod.save(
        "threshold_notifications.json",
        {
            "de-main": {
                "2026": {
                    "last_level": "yellow",
                    "fired_yellow_at": "2026-04-20",
                },
            },
        },
    )
    out = daily_digest_tick.tick(today=date(2026, 4, 20))
    assert out["fired"] is True
    msg = _capture_tg[0]
    assert "2 overdue" in msg
    assert "de-main" in msg
    # 2400 + 7100 = 9500
    assert "9,500.00" in msg or "9500.00" in msg
    assert "80%" in msg or "crossed 80%" in msg
