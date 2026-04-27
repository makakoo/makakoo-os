"""Phase 5 E2E — two active offices (DE + AR), full invoicing flow,
cross-office isolation across templates, currency, counter, earnings,
and dashboard."""
from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from src.commands import dashboard as dashboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.commands import init as init_cmd
from src.commands import onboard_client as onboard_cmd
from src.core import client_meta
from src.core.registry import OfficeRegistry


@pytest.fixture
def two_country_offices(tmp_path, monkeypatch, no_brain):
    reg_path = tmp_path / "freelance_offices.json"
    monkeypatch.setenv("FREELANCE_OFFICES_REGISTRY", str(reg_path))

    home_de = tmp_path / "fo-de"
    home_ar = tmp_path / "fo-ar"
    reg = OfficeRegistry.load(reg_path)
    reg.add("de-main", home_de, "DE", default=True)
    reg.add("ar-main", home_ar, "AR")

    for office_id in ("de-main", "ar-main"):
        init_cmd.run(argparse.Namespace(
            json=False, dry_run=False, dba="", email="", office=office_id,
        ))
    return home_de, home_ar


def test_de_and_ar_full_flow_isolated(two_country_offices, no_brain):
    home_de, home_ar = two_country_offices

    # Onboard one client per office (same slug — must not collide)
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", slug="nb", name="Nb GmbH",
        sector="", contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1400, hourly_rate=None, payment_terms_days=30,
    ))
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", slug="nb", name="Nb SRL",
        sector="", contact_email="", ust_id="", b2b="true", client_country="AR",
        day_rate=2_000_000, hourly_rate=None, payment_terms_days=30,
    ))
    # currency inheritance (pi blocker #3)
    de_flat = client_meta.ClientMeta.load(home_de / "clients" / "nb" / "meta.yaml").flat()
    ar_flat = client_meta.ClientMeta.load(home_ar / "clients" / "nb" / "meta.yaml").flat()
    assert de_flat["currency"] == "EUR"
    assert ar_flat["currency"] == "ARS"

    # Generate contracts
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="nb", project="p",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="nb", project="p",
        title="T", description="", meilensteine="[]", total_days=10, rate=None,
    ))

    # Generate one invoice per office
    r_de = invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="nb", project="p",
        amount_net=None, days=10, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    r_ar = invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="nb", project="p",
        amount_net=None, days=5, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
    ))
    # Both get INV-2026-001 (isolated counters — pi blocker #1)
    assert r_de["invoice_number"] == "INV-2026-001"
    assert r_ar["invoice_number"] == "INV-2026-001"
    # DE: 10 days × 1400 = 14000 EUR
    assert r_de["net"] == 14000.0
    assert r_de["ust"] == 2660.0  # 19% USt
    # AR: 5 days × 2M = 10M ARS, no IVA (Monotributo default)
    assert r_ar["net"] == 10_000_000.0
    assert r_ar["ust"] == 0.0

    # Dashboards isolated
    d_de = dashboard_cmd.run(argparse.Namespace(json=False, dry_run=False, office="de-main"))
    d_ar = dashboard_cmd.run(argparse.Namespace(json=False, dry_run=False, office="ar-main"))
    assert d_de["earnings_ytd"] == 14000.0
    assert d_ar["earnings_ytd"] == 10_000_000.0

    # Per-country invoice file templates present on disk (Phase 4)
    de_tpls = list((home_de / "templates").iterdir())
    ar_tpls = list((home_ar / "templates").iterdir())
    assert any(f.name == "INVOICE.md.j2" for f in de_tpls)
    assert any(f.name == "INVOICE_AR.md.j2" for f in ar_tpls)

    # Rendered invoices carry language markers
    de_inv = (home_de / "clients" / "nb" / "projects" / "p" / "invoices" / "INV-2026-001.md").read_text()
    ar_inv = (home_ar / "clients" / "nb" / "projects" / "p" / "invoices" / "INV-2026-001.md").read_text()
    assert "RECHNUNG" in de_inv and "Leistungszeitraum" in de_inv
    assert "FACTURA" in ar_inv and "Monotributo" in ar_inv


def test_unknown_office_raises_clear_error(two_country_offices):
    from src.core.paths import resolve_office_root
    from src.core.registry import UnknownOfficeError
    with pytest.raises(UnknownOfficeError) as exc:
        resolve_office_root(argparse.Namespace(office="us-main"))
    msg = str(exc.value)
    # Error names registered offices with id, country, path (pi Q2 refinement)
    assert "de-main" in msg and "DE" in msg
    assert "ar-main" in msg and "AR" in msg


def test_cross_office_pdf_smoke(two_country_offices, no_brain):
    """v0.3 Phase 1 — DE + AR offices both produce valid PDFs at
    their own rates. Guards against a weasyprint pipeline that
    silently only works for the default locale."""
    pytest.importorskip("weasyprint")
    pytest.importorskip("markdown")
    pypdf = pytest.importorskip("pypdf")

    home_de, home_ar = two_country_offices
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", slug="pdfclient", name="PDF DE",
        sector="", contact_email="", ust_id="", b2b="true", client_country="DE",
        day_rate=1400, hourly_rate=None, payment_terms_days=30,
    ))
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", slug="pdfclient", name="PDF AR",
        sector="", contact_email="", ust_id="", b2b="true", client_country="AR",
        day_rate=2_000_000, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="pdfclient", project="p",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="pdfclient", project="p",
        title="T", description="", meilensteine="[]", total_days=10, rate=None,
    ))

    r_de = invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="de-main", client="pdfclient", project="p",
        amount_net=None, days=5, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
        pdf=True, force=False,
    ))
    r_ar = invoice_cmd.run(argparse.Namespace(
        json=False, dry_run=False, office="ar-main", client="pdfclient", project="p",
        amount_net=None, days=2, description="x",
        leistungszeitraum="", invoice_number=None, issued="2026-04-01",
        pdf=True, force=False,
    ))
    for r in (r_de, r_ar):
        pdf = Path(r["pdf_path"])
        assert pdf.is_file()
        assert pdf.read_bytes()[:8].startswith(b"%PDF-1.")
        assert len(pypdf.PdfReader(str(pdf)).pages) >= 1
