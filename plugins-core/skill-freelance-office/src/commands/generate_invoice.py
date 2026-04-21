"""freelance-office generate-invoice — atomic INV-YYYY-NNN allocation + render.

Rate resolution precedence (pi recommendation 5):
  1. ``--amount-net <N>``  → use as-is, no rate lookup.
  2. ``--days <D>``        → ``amount_net = D * client.meta.day_rate_agreed``.
                             Error if ``day_rate_agreed`` is missing.
  3. Neither passed       → error ``"provide --amount-net or --days"``.

VAT regime:
  - SETTINGS.tax.kleinunternehmer = true  → §19 UStG block, no VAT.
  - client.b2b && client.client_country != "DE" && client.ust_id → Reverse Charge, no VAT.
  - else → +19% USt.
"""
from __future__ import annotations

from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Dict, Optional

from ..core import brain, client_meta, earnings, invoice_counter, paths, render, settings, tracker
from ..core.errors import FreelanceError, NotInitialisedError
from ..core.tax import get_regime


def add_arguments(parser):
    parser.add_argument("--client", required=True)
    parser.add_argument("--project", required=True)
    parser.add_argument("--amount-net", type=float, default=None)
    parser.add_argument("--days", type=float, default=None)
    parser.add_argument("--description", required=True)
    parser.add_argument("--leistungszeitraum", default="", help="YYYY-MM-DD bis YYYY-MM-DD")
    parser.add_argument("--invoice-number", default=None, help="manual INV-YYYY-NNN override")
    parser.add_argument("--issued", default=None, help="YYYY-MM-DD, default today")


def _resolve_amount(args, meta_flat: Dict[str, Any]):
    if args.amount_net is not None:
        return float(args.amount_net), None
    if args.days is not None:
        rate = meta_flat.get("day_rate_agreed")
        if rate is None:
            raise FreelanceError(
                f"--days given but client.meta.day_rate_agreed is not set; "
                "onboard-client should populate this field or pass --amount-net"
            )
        return float(args.days) * float(rate), float(args.days)
    raise FreelanceError("provide --amount-net or --days")


def _vat_regime(s, meta_flat: Dict[str, Any]):
    """v0.1-compatible envelope. Delegates to the per-country tax module
    via ``get_regime(s.office.country)``. DE regime matches v0.1 byte-
    for-byte; other countries return their country-specific shape."""
    regime = get_regime(s.office.country)
    vr = regime.vat_regime(s, meta_flat)
    return {
        "kleinunternehmer": bool(getattr(s.tax, "kleinunternehmer", False))
            and s.office.country.upper() == "DE",
        "reverse_charge": vr.reverse_charge,
        "apply_vat": vr.apply_vat,
        "vat_rate": vr.vat_rate,
        "label": vr.label,
    }


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    s = settings.load_settings_at(home)

    client_dir = home / "clients" / args.client
    if not client_dir.is_dir():
        raise NotInitialisedError(f"client '{args.client}' not found under clients/")
    meta_path = client_dir / "meta.yaml"
    meta = client_meta.ClientMeta.load(meta_path).flat()

    project_dir = client_dir / "projects" / args.project
    if not project_dir.is_dir():
        raise NotInitialisedError(
            f"project '{args.project}' missing — run `freelance-office generate-contract` first"
        )

    amount_net, days_billed = _resolve_amount(args, meta)
    amount_net = round(float(amount_net), 2)
    day_rate = float(meta.get("day_rate_agreed") or 0)
    regime = _vat_regime(s, meta)

    issued_str = args.issued or str(date.today())
    try:
        issued_d = datetime.strptime(issued_str, "%Y-%m-%d").date()
    except ValueError as e:
        raise FreelanceError(f"--issued must be YYYY-MM-DD: {e}") from e
    payment_terms = int(meta.get("payment_terms_days") or s.finance.payment_terms_days)
    due_d = issued_d + timedelta(days=payment_terms)
    year = issued_d.year

    dry = bool(getattr(args, "dry_run", False))

    # Allocate number (unless user-forced)
    seeded = False
    if args.invoice_number:
        inv_no = args.invoice_number
        numeric_n = None
    else:
        if dry:
            # Dry-run preview: just peek — do not bump
            next_n = invoice_counter.peek(year, home) + 1
            inv_no = f"INV-{year}-{next_n:03d}"
            numeric_n = next_n
        else:
            inv_no, numeric_n, seeded = invoice_counter.allocate(year, home)

    # VAT numbers — route through the country regime's vat_rate.
    rate = float(regime.get("vat_rate", 0.19)) if regime["apply_vat"] else 0.0
    ust = round(amount_net * rate, 2) if regime["apply_vat"] else 0.0
    brutto = round(amount_net + ust, 2)

    warnings = []
    floor = settings.load_rates_at(home).floor_day_rate
    if floor is not None and day_rate and day_rate < floor:
        warnings.append(f"client.day_rate_agreed={day_rate} < RATES.yaml floor {floor}")

    ctx: Dict[str, Any] = {
        "invoice_number": inv_no,
        "issued": str(issued_d),
        "due": str(due_d),
        "leistungszeitraum": args.leistungszeitraum or str(issued_d),
        "project_slug": args.project,
        "description": args.description,
        "days_billed": days_billed,
        "day_rate": day_rate,
        "net": amount_net,
        "ust": ust,
        "brutto": brutto,
        "payment_terms_days": payment_terms,
        "kleinunternehmer": regime["kleinunternehmer"],
        "reverse_charge": regime["reverse_charge"],
        "apply_vat": regime["apply_vat"],
        "vat_rate": regime.get("vat_rate", 0.0),
        "regime_label": regime.get("label", ""),
        "from_name": s.identity.name,
        "from_dba": s.identity.dba,
        "from_email": s.identity.email,
        "from_ust_id": s.tax.ust_id,
        "to_name": str(meta.get("name") or args.client),
        "to_email": str(meta.get("contact_email") or ""),
        "to_ust_id": str(meta.get("ust_id") or ""),
        "bank_iban": s.finance.bank.iban,
        "bank_bic": s.finance.bank.bic,
        "bank_name": s.finance.bank.name,
    }

    # Pick the per-country invoice template (falls back to base if missing).
    try:
        regime_mod = get_regime(s.office.country)
        invoice_template = getattr(regime_mod, "INVOICE_TEMPLATE", "INVOICE.md.j2")
    except Exception:
        invoice_template = "INVOICE.md.j2"
    rendered = render.render_invoice(ctx, template_name=invoice_template, office_root=home)

    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "invoice_number": inv_no,
            "net": amount_net,
            "ust": ust,
            "brutto": brutto,
            "regime": regime,
            "due": str(due_d),
            "preview_bytes": len(rendered),
            "warnings": warnings,
            "message": f"dry-run: would write {inv_no} (€{amount_net} net, due {due_d})",
        }

    invoices_dir = project_dir / "invoices"
    invoices_dir.mkdir(parents=True, exist_ok=True)
    out_path = invoices_dir / f"{inv_no}.md"
    if out_path.exists():
        raise FreelanceError(f"invoice file already exists: {out_path}")
    out_path.write_text(rendered, encoding="utf-8")

    # Book to EARNINGS.md
    try:
        rec = earnings.EarningRecord(
            inv_no=inv_no,
            client=args.client,
            project=args.project,
            issued=str(issued_d),
            net=amount_net,
            ust=ust,
            status="⏳ offen",
        )
        _, ytd = earnings.append_earning(year, rec, home)
    except FreelanceError as e:
        warnings.append(f"EARNINGS.md update failed: {e}")
        ytd = 0.0

    # Add to tracker's Rechnungen table
    tracker_path = project_dir / "_project-tracker.md"
    if tracker_path.is_file():
        try:
            t = tracker.Tracker.load(tracker_path)
            t.append_invoice(inv_no, amount_net, str(issued_d), str(due_d))
            t.write()
        except FreelanceError as e:
            warnings.append(f"tracker invoice-row update failed: {e}")

    journal_line = (
        f"Generated [[{inv_no}]] for [[{args.client}]] — €{amount_net:.2f} net, "
        f"due {due_d}. [[freelance-office]]"
    )
    try:
        journaled = brain.append_journal_line(journal_line)
    except FreelanceError as e:
        warnings.append(f"brain journal failed: {e}")
        journaled = None

    return {
        "status": "ok",
        "exit_code": 0,
        "invoice_number": inv_no,
        "path": str(out_path),
        "counter_seeded_from_disk": seeded,
        "net": amount_net,
        "ust": ust,
        "brutto": brutto,
        "regime": regime,
        "due": str(due_d),
        "earnings_ytd": ytd,
        "journal": journaled,
        "warnings": warnings,
        "message": f"{inv_no}: €{amount_net:.2f} net → {out_path}. Due {due_d}.",
    }
