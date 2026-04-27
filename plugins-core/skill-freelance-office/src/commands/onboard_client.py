"""freelance-office onboard-client — sign a new client."""
from __future__ import annotations

import shutil
from datetime import date
from pathlib import Path
from typing import Any, Dict

from ..core import brain, client_meta, paths, settings
from ..core.errors import DuplicateClientError, FreelanceError, NotInitialisedError


def add_arguments(parser):
    parser.add_argument("--slug", required=True, help="lowercase client slug")
    parser.add_argument("--name", required=True, help="client display name")
    parser.add_argument("--sector", default="")
    parser.add_argument("--contact-email", default="")
    parser.add_argument("--ust-id", default="")
    parser.add_argument("--b2b", default="true", choices=("true", "false"))
    parser.add_argument("--client-country", default="DE", help="ISO-2 country code")
    parser.add_argument("--day-rate", type=int, required=True, help="EUR net per day (canonical day_rate_agreed)")
    parser.add_argument("--hourly-rate", type=int, default=None)
    parser.add_argument("--payment-terms-days", type=int, default=30)


def run(args) -> Dict[str, Any]:
    home = paths.resolve_office_root(args)
    paths.require_initialised_at(home)
    s = settings.load_settings_at(home)
    rates = settings.load_rates_at(home)
    template_dir = home / "clients" / "_template"
    target = home / "clients" / args.slug
    dry = bool(getattr(args, "dry_run", False))

    # pi blocker #3 — inherit currency + language from the target office,
    # never from hardcoded EUR/de defaults.
    office_currency = s.office.currency or "EUR"
    office_language = s.office.invoice_language or s.finance.invoice_language or "de"

    if target.exists():
        raise DuplicateClientError(f"client '{args.slug}' already exists at {target}")
    if not template_dir.is_dir():
        raise NotInitialisedError(
            f"client template missing at {template_dir} — run `freelance-office init`"
        )

    warnings = []
    floor = rates.floor_day_rate
    if floor is not None and args.day_rate < floor:
        warnings.append(
            f"negotiated day_rate_agreed={args.day_rate} is below RATES.yaml floor ({floor})"
        )

    hourly = args.hourly_rate
    if hourly is None:
        hourly = max(100, args.day_rate // 8)

    if dry:
        return {
            "status": "preview",
            "exit_code": 0,
            "dry_run": True,
            "would_copy": f"{template_dir} → {target}",
            "would_fill": {
                "slug": args.slug, "name": args.name, "sector": args.sector,
                "day_rate_agreed": args.day_rate, "hourly_rate_agreed": hourly,
                "payment_terms_days": args.payment_terms_days,
                "contact_email": args.contact_email, "ust_id": args.ust_id,
                "b2b": args.b2b == "true", "client_country": args.client_country,
                "current_status": "prospecting", "contract_signed": False,
                "started": str(date.today()),
            },
            "warnings": warnings,
            "message": f"dry-run: would onboard {args.slug}",
        }

    shutil.copytree(template_dir, target)
    meta_path = target / "meta.yaml"
    try:
        meta = client_meta.ClientMeta.load(meta_path)
        meta.set_value("slug", args.slug)
        meta.set_value("name", args.name)
        meta.set_value("sector", args.sector)
        meta.set_value("contact_email", args.contact_email)
        meta.set_value("ust_id", args.ust_id)
        meta.set_value("b2b", args.b2b == "true")
        meta.set_value("client_country", args.client_country)
        meta.set_value("started", str(date.today()))
        meta.set_value("current_status", "prospecting")
        meta.set_value("contract_signed", False)
        meta.set_value("day_rate_agreed", args.day_rate)
        meta.set_value("hourly_rate_agreed", hourly)
        meta.set_value("payment_terms_days", args.payment_terms_days)
        # pi blocker #3 — override any template default with office currency.
        meta.set_value("currency", office_currency)
        meta.write()
    except Exception as e:
        shutil.rmtree(target, ignore_errors=True)
        raise FreelanceError(f"failed to fill meta.yaml, rolled back: {e}") from e

    projects_dir = target / "projects"
    for entry in list(projects_dir.iterdir()) if projects_dir.is_dir() else []:
        if entry.name.startswith("_"):
            continue
        # Nothing to do — template only contains the tracker-template file.

    journal_line = (
        f"Onboarded [[{args.slug}]] client — EUR {args.day_rate}/day, "
        f"{args.payment_terms_days}-day terms. [[freelance-office]]"
    )
    try:
        journaled_to = brain.append_journal_line(journal_line)
    except FreelanceError as e:
        warnings.append(f"brain journal write failed: {e}")
        journaled_to = None

    # Create Makakoo Brain client page
    brain_page_path = None
    try:
        brain_page_path = brain.create_client_brain_page(
            client_slug=args.slug,
            client_name=args.name,
            sector=args.sector,
            contact_email=args.contact_email,
            day_rate=args.day_rate,
            payment_terms_days=args.payment_terms_days,
            status="prospecting",
        )
    except FreelanceError as e:
        warnings.append(f"brain page write failed: {e}")
        brain_page_path = None

    # Append brain link to meta.yaml
    try:
        meta_content = meta_path.read_text(encoding="utf-8")
        brain_section = (
            "\n---\n"
            "## 🧠 Makakoo Brain\n\n"
            "| Was | Pfad |\n"
            "|-----|------|\n"
            f"| Client Overview | `~/MAKAKOO/data/Brain/pages/{args.slug}/{args.slug}.md` |\n"
        )
        if "## 🧠 Makakoo Brain" not in meta_content:
            meta_path.write_text(meta_content + brain_section, encoding="utf-8")
    except OSError:
        pass

    return {
        "status": "ok",
        "exit_code": 0,
        "client": args.slug,
        "path": str(target),
        "meta": str(meta_path),
        "journal": journaled_to,
        "brain_page": brain_page_path,
        "warnings": warnings,
        "next_step": (
            f"freelance-office generate-contract --client {args.slug} "
            "--project <project-slug> --title '...' --days <N>"
        ),
        "message": f"onboarded {args.slug} at {target} — brain page at {brain_page_path}",
    }
