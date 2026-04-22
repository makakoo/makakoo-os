"""freelance-office init — bootstrap ~/freelance-office/ from bundled templates.

Safe to re-run: existing files are never overwritten. Missing files
are rendered from ``src/templates/*.j2`` with a minimal context.
"""
from __future__ import annotations

from datetime import date
from pathlib import Path
from typing import Any, Dict, List

from jinja2 import Environment, FileSystemLoader, StrictUndefined

from ..core import paths
from ..core.errors import FreelanceError
from ..core.migration import DEFAULT_COUNTRY_PROFILE
from ..core import registry as reg_mod
from ..core.tax import UnsupportedCountryError, get_regime


def _plugin_templates() -> Path:
    return Path(__file__).resolve().parent.parent / "templates"


def _render(template_name: str, context: Dict[str, Any]) -> str:
    env = Environment(
        loader=FileSystemLoader(str(_plugin_templates())),
        trim_blocks=True,
        lstrip_blocks=True,
        undefined=StrictUndefined,
        keep_trailing_newline=True,
    )
    return env.get_template(template_name).render(**context)


def add_arguments(parser):  # registered from commands package if ever used
    parser.add_argument("--dba", default="")
    parser.add_argument("--email", default="")


def _require_args(args):
    # argparse at top-level doesn't define subcommand args for Phase-1 stubs,
    # so we tolerate missing attrs. Interactive-prompt flow is deferred.
    return {
        "dba": getattr(args, "dba", "") or "",
        "email": getattr(args, "email", "") or "",
    }


def run(args) -> Dict[str, Any]:
    # Prefer office-aware resolution. For init on a completely fresh
    # machine the registry may be empty — in that case the resolver
    # falls back to $FREELANCE_OFFICE_HOME or ~/freelance-office.
    home = paths.resolve_office_root(args)

    # Country for this office (from the registry entry, if any).
    country = "DE"
    reg = reg_mod.OfficeRegistry.load()
    for e in reg.offices.values():
        if Path(e.path) == home:
            country = e.country
            break
    profile = DEFAULT_COUNTRY_PROFILE.get(country, DEFAULT_COUNTRY_PROFILE["DE"])

    created: List[str] = []
    skipped: List[str] = []
    preview: List[Dict[str, str]] = []
    dry = bool(getattr(args, "dry_run", False))

    layout = [
        home,
        home / "_meta",
        home / "clients" / "_template" / "projects",
        home / "clients" / "_template" / "communications",
        home / "templates",
        home / "admin",
        home / "finances" / str(date.today().year),
    ]
    for d in layout:
        if not d.exists():
            if dry:
                preview.append({"mkdir": str(d)})
            else:
                d.mkdir(parents=True, exist_ok=True)
                created.append(f"[dir] {d}")
        else:
            skipped.append(f"[dir] {d}")

    ctx_opts = _require_args(args)
    ctx: Dict[str, Any] = {
        "identity_name": "",
        "identity_dba": ctx_opts["dba"],
        "identity_email": ctx_opts["email"],
        "freelance_since": str(date.today().year),
        "today": str(date.today()),
        "year": date.today().year,
        # office-aware defaults for template rendering
        "office_id": getattr(args, "office", None) or "default",
        "office_country": country,
        "office_locale": profile["locale"],
        "office_currency": profile["currency"],
        "office_invoice_language": profile["invoice_language"],
        "slug": "_template",
        "name": "Client Display Name",
        "sector": "",
        "contact_email": "",
        "ust_id": "",
        "b2b": True,
        "client_country": country,
        "currency": profile["currency"],
        "day_rate_agreed": 1200,
        "hourly_rate_agreed": 150,
        "payment_terms_days": 30,
        "started": str(date.today()),
        "project_title": "[Project Name]",
        "client_slug": "_template-slug",
        "project_slug": "_template-project",
        "agreed_days": 0,
        "day_rate": 0,
        "total_contract_value": 0,
    }

    wrote: List[str] = []

    # Pick the country-appropriate template family from the tax regime.
    # Unknown / unsupported country falls back to the DE base templates.
    try:
        regime = get_regime(country)
        invoice_tmpl = getattr(regime, "INVOICE_TEMPLATE", "INVOICE.md.j2")
        expenses_tmpl = getattr(regime, "EXPENSES_TEMPLATE", "EXPENSES.md.j2")
        earnings_tmpl = getattr(regime, "EARNINGS_TEMPLATE", "EARNINGS.md.j2")
    except UnsupportedCountryError:
        invoice_tmpl = "INVOICE.md.j2"
        expenses_tmpl = "EXPENSES.md.j2"
        earnings_tmpl = "EARNINGS.md.j2"

    # Tuple: (template, dest_relative_to_home, copy_raw)
    # copy_raw=True → copy the .j2 file verbatim (used later by generate-*)
    # copy_raw=False → render with `ctx` now and drop the .j2 suffix
    files = [
        ("SETTINGS.yaml.j2",           "_meta/SETTINGS.yaml",                                  False),
        ("RATES.yaml.j2",              "_meta/RATES.yaml",                                     False),
        (invoice_tmpl,                 f"templates/{invoice_tmpl}",                            True),
        ("PROJECT_VEREINBARUNG.md.j2", "templates/PROJECT_VEREINBARUNG.md.j2",                 True),
        ("CLIENT_META.yaml.j2",        "clients/_template/meta.yaml",                          False),
        ("PROJECT_TRACKER.md.j2",      "clients/_template/projects/_project-tracker.md",       False),
        (earnings_tmpl,                f"finances/{date.today().year}/EARNINGS.md",            False),
        (expenses_tmpl,                f"finances/{date.today().year}/EXPENSES.md",            False),
    ]
    for tmpl_name, rel, copy_raw in files:
        dst = home / rel
        if dst.exists():
            skipped.append(f"[file] {rel}")
            continue
        if dry:
            preview.append({"write": str(dst), "template": tmpl_name, "raw": copy_raw})
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        if copy_raw:
            raw = (_plugin_templates() / tmpl_name).read_text(encoding="utf-8")
            dst.write_text(raw, encoding="utf-8")
        else:
            try:
                rendered = _render(tmpl_name, ctx)
            except Exception as e:
                raise FreelanceError(f"failed to render {tmpl_name}: {e}") from e
            dst.write_text(rendered, encoding="utf-8")
        wrote.append(f"[file] {rel}")
        created.append(f"[file] {rel}")

    message = (
        f"dry-run: would create {len(preview)} entries in {home}"
        if dry
        else f"created {len(created)} entries, skipped {len(skipped)} at {home}"
    )
    return {
        "status": "ok" if not dry else "preview",
        "exit_code": 0,
        "dry_run": dry,
        "home": str(home),
        "created": created if not dry else preview,
        "skipped": skipped,
        "message": message,
    }
