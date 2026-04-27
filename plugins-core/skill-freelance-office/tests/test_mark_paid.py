"""Phase 2 — ``freelance-office mark-paid`` coverage.

Covers every pi review gap + corruption risk:

- Full-paid single call flips status and appends Zahlungseingänge.
- Partial payment leaves status ``💰 teilweise`` with the right
  balance.
- Two-tranche accumulation auto-flips to ``✅ bezahlt`` (pi gap #2).
- Over-payment guard errors cleanly (pi gap #2).
- Unknown-invoice error wording hints at reconcile (pi Q3).
- Idempotent no-op on an already-paid invoice.
- Brain journal line is produced.
- Project tracker's Bezahlt cell flips to ``[✅]`` / ``[💰]``.
"""
from __future__ import annotations

import argparse

import pytest

from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import mark_paid as mark_paid_cmd
from src.commands import onboard_client as onboard_cmd
from src.core.errors import FreelanceError


def _scaffold(tmp_freelance_home, no_brain):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="acme", name="Acme GmbH", sector="",
        contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1200, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="acme", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))


def _generate_invoice(net: float = 1000.0):
    return invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, amount_net=net, days=None,
        description="x", leistungszeitraum="", invoice_number=None,
        issued="2026-04-01", client="acme", project="p1",
        pdf=False, force=False,
    ))


def _mark_paid_ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("paid_date", "2026-05-01")
    kw.setdefault("amount", None)
    kw.setdefault("bank_entry_date", None)
    return argparse.Namespace(**kw)


def _earnings_text(tmp_freelance_home, year: int = 2026) -> str:
    return (tmp_freelance_home / "finances" / str(year) / "EARNINGS.md").read_text(encoding="utf-8")


def _tracker_text(tmp_freelance_home) -> str:
    return (
        tmp_freelance_home / "clients" / "acme" / "projects" / "p1" / "_project-tracker.md"
    ).read_text(encoding="utf-8")


def test_mark_paid_full_flips_status_and_appends_zahlungseingang(
    tmp_freelance_home, no_brain
):
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    out = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no))
    assert out["status"] == "ok"
    assert out["paid_status"] == "✅ bezahlt"
    assert out["fully_paid"] is True
    assert out["balance"] == 0.0
    # Status cell flipped in-place
    text = _earnings_text(tmp_freelance_home)
    assert "✅ bezahlt" in text
    assert "⏳ offen" not in text or text.count("⏳ offen") == 0
    # Zahlungseingänge row present (per-row match: INV + amount + date)
    assert inv_no in text.split("Zahlungseingänge")[-1]
    assert "2026-05-01" in text.split("Zahlungseingänge")[-1]


def test_mark_paid_partial_leaves_balance(tmp_freelance_home, no_brain):
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    out = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=400.0))
    assert out["paid_status"] == "💰 teilweise"
    assert out["fully_paid"] is False
    assert out["balance"] == 600.0
    text = _earnings_text(tmp_freelance_home)
    assert "💰 teilweise" in text


def test_mark_paid_two_tranche_accumulation_auto_flips(tmp_freelance_home, no_brain):
    """pi gap #2: two partial calls summing to net must auto-flip
    status to ``✅ bezahlt``."""
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    r1 = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=400.0))
    r2 = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=600.0, paid_date="2026-05-15"))
    assert r1["fully_paid"] is False
    assert r2["fully_paid"] is True
    assert r2["paid_status"] == "✅ bezahlt"
    assert r2["accumulated"] == 1000.0
    text = _earnings_text(tmp_freelance_home)
    # Two Zahlungseingänge rows, one per tranche
    zahl_block = text.split("Zahlungseingänge")[-1]
    assert zahl_block.count(inv_no) == 2


def test_mark_paid_over_payment_guard(tmp_freelance_home, no_brain):
    """pi gap #2: cannot apply more than the remaining balance."""
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=1000.0))
    with pytest.raises(FreelanceError, match="already paid"):
        mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=50.0))


def test_mark_paid_unknown_invoice_hints_at_reconcile(tmp_freelance_home, no_brain):
    """pi Q3: unknown invoice should link to the (future) reconcile
    command instead of blaming the user."""
    _scaffold(tmp_freelance_home, no_brain)
    # Generate one invoice so EARNINGS exists for 2026, but ask about
    # a different invoice number.
    _generate_invoice(1000.0)
    with pytest.raises(FreelanceError) as exc:
        mark_paid_cmd.run(_mark_paid_ns(invoice="INV-2026-999"))
    msg = str(exc.value)
    assert "INV-2026-999" in msg
    assert "reconcile" in msg


def test_mark_paid_already_paid_is_idempotent(tmp_freelance_home, no_brain):
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no))
    again = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no))
    assert again["status"] == "ok"
    assert again["paid_status"] == "✅ bezahlt"
    assert again["amount_paid"] == 0.0
    assert "already paid" in again["message"].lower()


def test_mark_paid_writes_brain_journal_line(tmp_freelance_home, no_brain):
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    before = len(no_brain)
    mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no))
    after = no_brain
    # At least one new journal line was produced.
    assert len(after) > before
    new_lines = after[before:]
    assert any(f"[[{inv_no}]]" in ln and "paid" in ln.lower() for ln in new_lines), new_lines


def test_mark_paid_updates_project_tracker_bezahlt_cell(tmp_freelance_home, no_brain):
    """Project tracker's Bezahlt cell flips to [✅] on full, [💰] on
    partial. Covers the two-phase verify happy-path (tracker write
    lands; mark-paid returns without the verify error)."""
    _scaffold(tmp_freelance_home, no_brain)
    r = _generate_invoice(1000.0)
    inv_no = r["invoice_number"]
    r1 = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=300.0))
    assert r1["tracker_verified"] is True
    txt = _tracker_text(tmp_freelance_home)
    # Find the invoice's tracker row.
    row = next(ln for ln in txt.splitlines() if inv_no in ln and ln.startswith("|"))
    assert "[💰]" in row
    r2 = mark_paid_cmd.run(_mark_paid_ns(invoice=inv_no, amount=700.0))
    assert r2["tracker_verified"] is True
    txt = _tracker_text(tmp_freelance_home)
    row = next(ln for ln in txt.splitlines() if inv_no in ln and ln.startswith("|"))
    assert "[✅]" in row
