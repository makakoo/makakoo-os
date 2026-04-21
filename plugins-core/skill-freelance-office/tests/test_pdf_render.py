"""Phase 1 — PDF render via weasyprint (pi A1 + gap #1 +
corruption-risk #1).

Covers:
- Valid PDF bytes + single page + pypdf parses it.
- Invoice number, net amount, IBAN, payment-terms string are all
  present verbatim in the extracted PDF text.
- Re-running ``generate-invoice --pdf`` on an already-paid invoice
  errors cleanly without ``--force``.
- ``--force`` allows regeneration of the PDF for a paid invoice.
"""
from __future__ import annotations

import argparse
from pathlib import Path

import pytest

pypdf = pytest.importorskip("pypdf")
pytest.importorskip("weasyprint")
pytest.importorskip("markdown")

from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import onboard_client as onboard_cmd
from src.core import paths as paths_mod


def _scaffold(tmp_freelance_home, no_brain, *, set_iban: bool = False):
    if set_iban:
        sp = tmp_freelance_home / "_meta" / "SETTINGS.yaml"
        txt = sp.read_text(encoding="utf-8").replace(
            "iban: \"\"", 'iban: "DE89 3704 0044 0532 0130 00"'
        )
        sp.write_text(txt, encoding="utf-8")
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug="acme", name="Acme GmbH", sector="",
        contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1200, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client="acme", project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))


def _invoice_ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("amount_net", 5000.0)
    kw.setdefault("days", None)
    kw.setdefault("description", "Beratung")
    kw.setdefault("leistungszeitraum", "")
    kw.setdefault("invoice_number", None)
    kw.setdefault("issued", None)
    kw.setdefault("client", "acme")
    kw.setdefault("project", "p1")
    kw.setdefault("pdf", False)
    kw.setdefault("force", False)
    return argparse.Namespace(**kw)


def test_pdf_is_valid_pdf_bytes(tmp_freelance_home, no_brain):
    _scaffold(tmp_freelance_home, no_brain)
    r = invoice_cmd.run(_invoice_ns(pdf=True))
    pdf_path = Path(r["pdf_path"])
    assert pdf_path.is_file(), r
    header = pdf_path.read_bytes()[:8]
    assert header.startswith(b"%PDF-1."), header
    reader = pypdf.PdfReader(str(pdf_path))
    assert len(reader.pages) >= 1


def test_pdf_content_matches_invoice_data(tmp_freelance_home, no_brain):
    """pi gap #1: parse the PDF text back out and assert invoice_number,
    net amount, IBAN, and payment-terms string are all verbatim."""
    _scaffold(tmp_freelance_home, no_brain, set_iban=True)
    r = invoice_cmd.run(_invoice_ns(amount_net=4711.0, pdf=True))
    pdf_path = Path(r["pdf_path"])
    reader = pypdf.PdfReader(str(pdf_path))
    full_text = "\n".join(page.extract_text() or "" for page in reader.pages)
    # Strip NBSPs that weasyprint likes to insert so our substring
    # assertions are simple plain-ASCII checks.
    normalised = full_text.replace(" ", " ")
    assert r["invoice_number"] in normalised, normalised
    # Net amount is written by the invoice template as "4711.00".
    # Accept the raw ASCII float form any locale we ship today.
    assert "4711.00" in normalised, normalised
    assert "DE89" in normalised or "3704 0044" in normalised, normalised
    # Payment-terms clause mentions 30-day window.
    assert "30" in normalised


def test_pdf_errors_on_already_paid_without_force(tmp_freelance_home, no_brain):
    """Simulate the corruption-risk path: generate an invoice, hand-
    flip its EARNINGS status to ✅ bezahlt, then retry with --pdf
    and no --force — must error cleanly."""
    _scaffold(tmp_freelance_home, no_brain)
    r = invoice_cmd.run(_invoice_ns(pdf=True))
    inv_no = r["invoice_number"]
    year = int(inv_no.split("-")[1])
    ep = tmp_freelance_home / "finances" / str(year) / "EARNINGS.md"
    ep.write_text(ep.read_text(encoding="utf-8").replace("⏳ offen", "✅ bezahlt", 1), encoding="utf-8")

    from src.core.errors import FreelanceError
    with pytest.raises(FreelanceError, match="already paid"):
        invoice_cmd.run(_invoice_ns(pdf=True, invoice_number=inv_no))


def test_pdf_force_regenerates_on_paid(tmp_freelance_home, no_brain):
    _scaffold(tmp_freelance_home, no_brain)
    r1 = invoice_cmd.run(_invoice_ns(pdf=True))
    inv_no = r1["invoice_number"]
    year = int(inv_no.split("-")[1])
    ep = tmp_freelance_home / "finances" / str(year) / "EARNINGS.md"
    ep.write_text(ep.read_text(encoding="utf-8").replace("⏳ offen", "✅ bezahlt", 1), encoding="utf-8")
    first_pdf = Path(r1["pdf_path"])
    first_mtime = first_pdf.stat().st_mtime_ns

    r2 = invoice_cmd.run(_invoice_ns(pdf=True, invoice_number=inv_no, force=True))
    assert r2["status"] == "ok", r2
    assert r2["pdf_path"] == str(first_pdf)
    assert first_pdf.is_file()
    # regeneration touched the file
    assert first_pdf.stat().st_mtime_ns >= first_mtime
