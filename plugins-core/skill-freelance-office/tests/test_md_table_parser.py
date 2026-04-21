"""Phase 0 — md-table parser hardening (pi blocker #1).

``earnings._iter_rows`` and ``expenses._scan_categories`` used to
split every row on ``|`` and index cells positionally. A pipe
character inside a client or project name shifted every downstream
index, so ``_parse_money(cells[5])`` silently skipped real invoices
and understated ``ytd_total``. The fix: ``parse_row(line,
expected_cols)`` returns cells only when the count matches; mis-
counted rows route to
``$MAKAKOO_HOME/state/skill-freelance-office/malformed_rows.jsonl``
so the user can find them later.
"""
from __future__ import annotations

import argparse
import json

import pytest

from src.commands import onboard_client as onboard_cmd
from src.commands import generate_contract as contract_cmd
from src.commands import generate_invoice as invoice_cmd
from src.core import earnings as earnings_mod
from src.core import md_table
from src.core import paths


def _scaffold_client(slug: str = "acme", country: str = "DE", rate: float = 1200.0):
    onboard_cmd.run(argparse.Namespace(
        json=False, dry_run=False, slug=slug, name="Acme", sector="",
        contact_email="", ust_id="", b2b="true", client_country=country,
        day_rate=rate, hourly_rate=None, payment_terms_days=30,
    ))
    contract_cmd.run(argparse.Namespace(
        json=False, dry_run=False, client=slug, project="p1",
        title="T", description="", meilensteine="[]", total_days=20, rate=None,
    ))


def _ns(**kw):
    kw.setdefault("json", False)
    kw.setdefault("dry_run", False)
    kw.setdefault("amount_net", None)
    kw.setdefault("days", None)
    kw.setdefault("description", "x")
    kw.setdefault("leistungszeitraum", "")
    kw.setdefault("invoice_number", None)
    kw.setdefault("issued", None)
    return argparse.Namespace(**kw)


@pytest.fixture
def _isolated_state(monkeypatch, tmp_path):
    """Route malformed_rows.jsonl into the test's tmp dir so the
    sentinel file can be asserted without touching the real
    $MAKAKOO_HOME/state tree."""
    monkeypatch.setenv("MAKAKOO_HOME", str(tmp_path / "makakoo"))
    return tmp_path / "makakoo" / "state" / "skill-freelance-office" / "malformed_rows.jsonl"


def test_earnings_project_name_with_pipe_does_not_silently_shift_indices(
    tmp_freelance_home, no_brain, _isolated_state
):
    """pi blocker #1: a project or client name containing ``|`` used
    to shift ``cells[5]`` (netto) onto a non-money cell, silently
    dropping the row from ``ytd_total``. With the parser hardened,
    the row is either parsed correctly OR routed to the malformed
    sentinel — but never silently shifted.
    """
    _scaffold_client(rate=1000.0)
    # Produce one valid invoice so the table has a real row.
    r = invoice_cmd.run(_ns(client="acme", project="p1", amount_net=1000))
    assert r["exit_code"] == 0

    # Hand-edit the earnings row to inject a literal pipe in the
    # project-name cell — simulating a raw Markdown edit by Sebastian.
    year = int(r["invoice_number"].split("-")[1])
    earnings_path = tmp_freelance_home / "finances" / str(year) / "EARNINGS.md"
    text = earnings_path.read_text(encoding="utf-8")
    # Replace "p1" in the data row with "Website | SEO" — that is 1
    # extra pipe, which would shift every downstream cell.
    assert " p1 " in text
    text_poisoned = text.replace(" p1 ", " Website | SEO ", 1)
    earnings_path.write_text(text_poisoned, encoding="utf-8")

    # Recompute ytd_total — the hardened parser MUST NOT silently
    # report a non-zero value built from a misaligned row.
    ytd = earnings_mod.ytd_total(year, root=tmp_freelance_home)
    # The injected pipe row has 10 cells instead of 9 → routed to the
    # sentinel. ytd_total reflects only well-formed rows. With the
    # single injected row now malformed, ytd == 0.0.
    assert ytd == 0.0, (
        "misaligned row must route to sentinel, not silently drop "
        "into ytd_total with a wrong column"
    )

    # Sentinel must have at least one record for this row.
    assert _isolated_state.is_file(), "malformed_rows.jsonl was not written"
    records = [
        json.loads(line)
        for line in _isolated_state.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert any(r["kind"] == "earnings" for r in records), records
    assert any("Website | SEO" in r["line"] for r in records), records


def test_earnings_malformed_row_routes_to_sentinel_and_is_skipped(
    tmp_freelance_home, no_brain, _isolated_state
):
    """pi gap #3: a row with an extra ``|`` must (a) not count toward
    ``ytd_total`` and (b) land in the sentinel file with the raw line
    preserved verbatim so ``doctor`` can surface it."""
    _scaffold_client()
    r = invoice_cmd.run(_ns(client="acme", project="p1", amount_net=500))
    year = int(r["invoice_number"].split("-")[1])
    ep = tmp_freelance_home / "finances" / str(year) / "EARNINGS.md"
    text = ep.read_text(encoding="utf-8")
    # Inject a bogus 10-cell row INSIDE the earnings section, right
    # before the Summe row (otherwise it falls outside
    # _section_bounds and _iter_rows never sees it).
    bogus = "| 99 | INV-2099-999 | Other | proj | 2099-01-01 | 100,00 | 0,00 | extra | 100,00 | ⏳ offen |"
    lines = text.splitlines()
    summe_idx = next(
        i for i, ln in enumerate(lines) if "**Summe**" in ln or "**Suma**" in ln or "**Total**" in ln
    )
    lines.insert(summe_idx, bogus)
    ep.write_text("\n".join(lines) + "\n", encoding="utf-8")

    # ytd_total re-scans — the bogus 10-cell row must not count.
    ytd = earnings_mod.ytd_total(year, root=tmp_freelance_home)
    assert ytd == 500.0, f"expected 500.0 (only the real invoice), got {ytd}"

    # Sentinel contains the exact malformed line.
    assert _isolated_state.is_file()
    records = [
        json.loads(line)
        for line in _isolated_state.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert any(bogus == rec["line"] for rec in records), (
        "malformed line was not persisted verbatim into sentinel"
    )
    assert md_table.malformed_row_count() >= 1
