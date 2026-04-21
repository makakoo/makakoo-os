"""ISO-8601 week boundary — pin the convention the plugin relies on.

ISO week 1 of a year is the week containing the first Thursday. For
2027 (starts on Friday), the first Thursday is 2027-01-07, so
**KW01 of 2027 = Mon 2027-01-04 – Sun 2027-01-10**, NOT the naïve
"week containing Jan 1." The days Dec 28 2026 – Jan 3 2027 belong
to **KW53 of 2026**.

These tests lock that interpretation against the ``datetime.date
.isocalendar()`` API which the dashboard + pipeline use for
``hours_this_week`` and the KW column rendering.
"""
from __future__ import annotations

from datetime import date, timedelta


def iso_week_dates(year: int, week: int):
    """Return (monday_date, sunday_date) for ISO year/week."""
    jan4 = date(year, 1, 4)
    jan4_weekday = jan4.isoweekday()  # Mon=1
    week1_monday = jan4 - timedelta(days=jan4_weekday - 1)
    monday = week1_monday + timedelta(weeks=week - 1)
    sunday = monday + timedelta(days=6)
    return monday, sunday


def test_iso_week_01_2027_starts_jan_4():
    monday, sunday = iso_week_dates(2027, 1)
    assert monday == date(2027, 1, 4), f"got {monday}"
    assert sunday == date(2027, 1, 10), f"got {sunday}"


def test_dec_28_2026_is_kw53_of_2026():
    # Dec 28 2026 (Monday) — per ISO-8601, this is KW53 of 2026, not KW01 of 2027.
    iso = date(2026, 12, 28).isocalendar()
    assert iso[0] == 2026
    assert iso[1] == 53
    assert iso[2] == 1  # Monday


def test_iso_week_17_2026_midyear():
    monday, sunday = iso_week_dates(2026, 17)
    assert monday.isocalendar() == (2026, 17, 1)
    assert sunday.isocalendar() == (2026, 17, 7)


def test_iso_week_53_handling():
    # 2026 has exactly 53 ISO weeks (Thursday rule)
    monday, sunday = iso_week_dates(2026, 53)
    assert monday.year in (2026, 2027)
